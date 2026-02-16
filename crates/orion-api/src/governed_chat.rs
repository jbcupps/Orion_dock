//! Bridge between the Execution Governor and the existing operational chat infrastructure.
//!
//! Provides adapter implementations of `ToolExecutor` and `ToolCallParser` that
//! wrap the existing skill execution and tool request parsing systems.

use std::path::Path;
use std::sync::Arc;

use orion_birth::chat::parse_tool_requests;
use orion_capabilities::cognitive::LlmProvider;
use orion_core::config::{AppConfig, ThinkingModelTier};
use orion_router::constraint_store::ConstraintStore;
use orion_router::governor::{
    async_trait, ExecutionGovernor, GovernedResult, GovernorConfig, ParsedToolCall, ToolCallParser,
    ToolExecutionResult, ToolExecutor,
};
use orion_router::planner::Planner;
use orion_skills::structured_failure::StructuredFailure;
use orion_skills::{SkillExecutor, SkillRegistry};

use crate::agentic;

/// Adapter: wraps the existing SkillExecutor + SkillRegistry as a Governor ToolExecutor.
pub struct SkillToolExecutor {
    skill_registry: Arc<SkillRegistry>,
    skill_executor: Arc<SkillExecutor>,
}

impl SkillToolExecutor {
    pub fn new(skill_registry: Arc<SkillRegistry>, skill_executor: Arc<SkillExecutor>) -> Self {
        Self {
            skill_registry,
            skill_executor,
        }
    }
}

#[async_trait]
impl ToolExecutor for SkillToolExecutor {
    async fn execute_tool(&self, tool_name: &str, args: &serde_json::Value) -> ToolExecutionResult {
        // Find the skill that provides this tool
        let skill_match = agentic::find_skill_for_tool(&self.skill_registry, tool_name);
        let (skill_id, _tool_desc) = match skill_match {
            Some(m) => m,
            None => {
                return ToolExecutionResult {
                    success: false,
                    output_text: format!("Tool '{}' not found in any registered skill", tool_name),
                    structured_failure: Some(StructuredFailure::Unknown(format!(
                        "Tool '{}' not found",
                        tool_name
                    ))),
                };
            }
        };

        // Build ToolParams from JSON args
        let mut tool_params = orion_skills::skill::ToolParams::new();
        if let serde_json::Value::Object(map) = args {
            for (k, v) in map {
                tool_params = tool_params.with(k, v.clone());
            }
        }

        // Execute via the existing SkillExecutor
        match self
            .skill_executor
            .execute(&skill_id, tool_name, tool_params)
            .await
        {
            Ok(output) => {
                if output.success {
                    let text = if let Some(data) = &output.data {
                        serde_json::to_string_pretty(data).unwrap_or_else(|_| data.to_string())
                    } else {
                        "OK".to_string()
                    };
                    ToolExecutionResult {
                        success: true,
                        output_text: text,
                        structured_failure: None,
                    }
                } else {
                    let text = output
                        .error
                        .as_deref()
                        .unwrap_or("Unknown error")
                        .to_string();
                    ToolExecutionResult {
                        success: false,
                        output_text: text,
                        structured_failure: output.structured_failure,
                    }
                }
            }
            Err(e) => ToolExecutionResult {
                success: false,
                output_text: format!("Skill execution error: {}", e),
                structured_failure: Some(StructuredFailure::Unknown(e.to_string())),
            },
        }
    }
}

/// Adapter: wraps the existing `parse_tool_requests` from orion-birth as a Governor ToolCallParser.
pub struct BirthToolCallParser;

impl ToolCallParser for BirthToolCallParser {
    fn parse(&self, response_text: &str) -> (String, Vec<ParsedToolCall>) {
        let (clean, requests) = parse_tool_requests(response_text);
        let parsed = requests
            .into_iter()
            .map(|tr| {
                let args_summary = summarize_args(&tr.arguments);
                ParsedToolCall {
                    name: tr.name,
                    args: tr.arguments,
                    args_summary,
                }
            })
            .collect();
        (clean, parsed)
    }
}

/// Create a concise summary of tool call arguments (for logging, not secrets).
fn summarize_args(args: &serde_json::Value) -> String {
    match args {
        serde_json::Value::Object(map) => map
            .iter()
            .map(|(k, v)| {
                let val_str = match v {
                    serde_json::Value::String(s) => {
                        if s.len() > 50 {
                            format!("{}...", &s[..50])
                        } else {
                            s.clone()
                        }
                    }
                    other => {
                        let s = other.to_string();
                        if s.len() > 50 {
                            format!("{}...", &s[..50])
                        } else {
                            s
                        }
                    }
                };
                format!("{}={}", k, val_str)
            })
            .collect::<Vec<_>>()
            .join(", "),
        _ => args.to_string(),
    }
}

/// Build the skill tool descriptors as JSON values for the Planner's available_tools.
pub fn build_planner_tool_descriptors(registry: &SkillRegistry) -> Vec<serde_json::Value> {
    let skills = match registry.list_with_tiers() {
        Ok(s) => s,
        Err(_) => return vec![],
    };

    let mut descriptors = Vec::new();
    for (manifest, _tier) in &skills {
        if let Ok((skill, _, _)) = registry.get_skill(&manifest.id) {
            for tool in skill.tools() {
                descriptors.push(serde_json::json!({
                    "name": tool.name,
                    "description": tool.description,
                    "parameters": tool.parameters,
                }));
            }
        }
    }
    descriptors
}

/// Build an ExecutionGovernor from the current operational chat context.
///
/// Creates the Planner with a Pro-tier provider, and wires up the executor/recovery
/// providers at Fast and Standard tiers respectively.
/// Returns None if no Ego provider is configured.
pub fn build_governor(
    config: &AppConfig,
    ego_name: &str,
    ego_key: &str,
    skill_registry: Arc<SkillRegistry>,
    skill_executor: Arc<SkillExecutor>,
) -> Option<ExecutionGovernor> {
    // Build three providers at different tiers using the same Ego credentials
    let pro_model = config.effective_tier_model(ego_name, ThinkingModelTier::Pro);
    let fast_model = config.effective_tier_model(ego_name, ThinkingModelTier::Fast);
    let standard_model = config.effective_tier_model(ego_name, ThinkingModelTier::Standard);

    let build_provider = |model: String| -> Option<Arc<dyn LlmProvider>> {
        let (provider, _) = orion_router::build_ego_provider(
            Some(ego_name),
            Some(ego_key.to_string()),
            Some(model),
        );
        provider
    };

    let pro_provider = build_provider(pro_model)?;
    let fast_provider = build_provider(fast_model)?;
    let standard_provider = build_provider(standard_model)?;

    let planner = Arc::new(Planner::new(pro_provider));
    let tool_executor_adapter = Arc::new(SkillToolExecutor::new(skill_registry, skill_executor));
    let tool_parser = Arc::new(BirthToolCallParser);

    Some(ExecutionGovernor::new(
        planner,
        fast_provider,
        standard_provider,
        tool_executor_adapter,
        tool_parser,
        GovernorConfig::default(),
    ))
}

/// Extract the user-facing message from a GovernedResult.
pub fn governed_result_to_content(result: &GovernedResult) -> String {
    result.user_message()
}

/// Load persistent constraints from the agent's data directory.
pub fn load_persistent_constraints(agent_data_dir: &Path) -> Vec<String> {
    let store = ConstraintStore::load(agent_data_dir);
    store.as_planner_constraints()
}

/// Save newly discovered constraints from a governed execution to disk.
pub fn save_discovered_constraints(
    agent_data_dir: &Path,
    new_constraints: &[orion_skills::structured_failure::Constraint],
) {
    if new_constraints.is_empty() {
        return;
    }
    let mut store = ConstraintStore::load(agent_data_dir);
    store.add(new_constraints);
    if let Err(e) = store.save() {
        tracing::warn!("Failed to save constraints: {}", e);
    } else {
        tracing::info!("Saved {} total constraints", store.len());
    }
}
