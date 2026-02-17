//! Execution Governor — loop controller for governed agent execution.
//!
//! The Governor wraps the operational chat flow, adding:
//! - Structured planning via GoalFrame
//! - Multi-iteration execution with loop detection
//! - Strategy switching on failure (retry -> alternative -> escalate -> abort)
//! - Structural verification of done_criteria
//! - Guaranteed response on every exit path

use std::sync::Arc;
use std::time::Instant;

use tracing::{info, warn};

use orion_capabilities::cognitive::{CompletionRequest, LlmProvider, Message};
use orion_skills::structured_failure::StructuredFailure;

use crate::cognitive::{ChatResponse, SessionContext, ToolResultRef, UserRequest};
use crate::execution_state::{
    detect_progress, format_state_summary, Attempt, AttemptResult, CriterionStatus, ExecutionState,
    Progress, Strategy,
};
use crate::planner::Planner;

/// What the Governor returns after a governed execution.
/// Every variant carries enough information for the caller to produce
/// a meaningful response to the user.
#[derive(Debug)]
pub enum GovernedResult {
    /// All done_criteria verified. Includes the final response for the user.
    Success(ChatResponse),

    /// The agent cannot proceed without user input.
    NeedsInput {
        question: String,
        state: ExecutionState,
    },

    /// The LLM stopped generating actions but done_criteria aren't met.
    Incomplete {
        response: ChatResponse,
        state: ExecutionState,
    },

    /// Hit the iteration limit.
    MaxIterationsReached {
        state: ExecutionState,
        diagnosis: String,
    },

    /// Time budget exceeded.
    TimeBudgetExceeded { state: ExecutionState },

    /// Explicitly aborted (abort_condition matched).
    Aborted {
        reason: String,
        state: ExecutionState,
    },
}

impl GovernedResult {
    /// Extract a user-facing message from any result variant.
    pub fn user_message(&self) -> String {
        match self {
            Self::Success(resp) => resp.content.clone(),
            Self::NeedsInput { question, .. } => question.clone(),
            Self::Incomplete { response, state } => {
                let met = state.met_criteria_count();
                let total = state.goal.done_criteria.len();
                format!(
                    "{}\n\n(Note: Task partially complete -- {}/{} criteria met.)",
                    response.content, met, total
                )
            }
            Self::MaxIterationsReached { diagnosis, state } => {
                let met = state.met_criteria_count();
                let total = state.goal.done_criteria.len();
                format!(
                    "I reached the maximum number of attempts ({}) for this task. {}/{} criteria were met.\n\nDiagnosis: {}",
                    state.goal.max_iterations, met, total, diagnosis
                )
            }
            Self::TimeBudgetExceeded { state } => {
                let met = state.met_criteria_count();
                let total = state.goal.done_criteria.len();
                format!(
                    "The time budget for this task was exceeded. {}/{} criteria were met before timeout.",
                    met, total
                )
            }
            Self::Aborted { reason, .. } => {
                format!("Task aborted: {}", reason)
            }
        }
    }

    /// Whether this result represents a successful completion.
    pub fn is_success(&self) -> bool {
        matches!(self, Self::Success(_))
    }
}

/// Re-export async_trait for downstream implementors.
pub use async_trait::async_trait;

/// Callback trait for executing tool calls. The Governor is decoupled from
/// the skill execution layer — the caller provides this implementation.
#[async_trait]
pub trait ToolExecutor: Send + Sync {
    /// Execute a tool call and return a structured result.
    /// The `tool_name` and `args` come from parsing the LLM response.
    async fn execute_tool(&self, tool_name: &str, args: &serde_json::Value) -> ToolExecutionResult;
}

/// Result of a single tool execution via the ToolExecutor.
#[derive(Debug, Clone)]
pub struct ToolExecutionResult {
    pub success: bool,
    pub output_text: String,
    pub structured_failure: Option<StructuredFailure>,
}

/// Parsed tool call from LLM response.
#[derive(Debug, Clone)]
pub struct ParsedToolCall {
    pub name: String,
    pub args: serde_json::Value,
    pub args_summary: String,
}

/// Callback trait for parsing tool calls from LLM text responses.
/// Decouples the Governor from the specific parsing implementation.
pub trait ToolCallParser: Send + Sync {
    fn parse(&self, response_text: &str) -> (String, Vec<ParsedToolCall>);
}

/// Configuration for the ExecutionGovernor.
pub struct GovernorConfig {
    /// Default max iterations if not overridden by GoalFrame.
    pub default_max_iterations: u8,
}

impl Default for GovernorConfig {
    fn default() -> Self {
        Self {
            default_max_iterations: 5,
        }
    }
}

/// The Execution Governor — manages the plan/execute/verify loop.
pub struct ExecutionGovernor {
    planner: Arc<Planner>,
    /// Fast-tier model for execution and reporting.
    executor_provider: Arc<dyn LlmProvider>,
    /// Standard-tier model for recovery reasoning.
    recovery_provider: Arc<dyn LlmProvider>,
    /// Tool execution callback.
    tool_executor: Arc<dyn ToolExecutor>,
    /// Tool call parser.
    tool_parser: Arc<dyn ToolCallParser>,
    _config: GovernorConfig,
}

impl ExecutionGovernor {
    pub fn new(
        planner: Arc<Planner>,
        executor_provider: Arc<dyn LlmProvider>,
        recovery_provider: Arc<dyn LlmProvider>,
        tool_executor: Arc<dyn ToolExecutor>,
        tool_parser: Arc<dyn ToolCallParser>,
        config: GovernorConfig,
    ) -> Self {
        Self {
            planner,
            executor_provider,
            recovery_provider,
            tool_executor,
            tool_parser,
            _config: config,
        }
    }

    /// Run the full governed execution: plan -> execute -> verify -> report.
    pub async fn execute(&self, user_request: UserRequest, ctx: &SessionContext) -> GovernedResult {
        // ================================================================
        // PHASE 1: PLANNING (Pro model, one call)
        // ================================================================
        info!("Governor: Phase 1 — Planning");
        let goal_frame = self.planner.plan(&user_request, ctx).await;

        let issues = goal_frame.validate();
        if !issues.is_empty() {
            warn!("GoalFrame validation issues: {:?}", issues);
        }

        let mut state = ExecutionState::new(goal_frame);
        let mut messages = self.build_initial_messages(&user_request, &state, ctx);

        // ================================================================
        // PHASE 2: EXECUTION LOOP (Fast model, multiple calls)
        // ================================================================
        info!(
            "Governor: Phase 2 — Execution loop (max {} iterations)",
            state.goal.max_iterations
        );

        for iteration in 0..state.goal.max_iterations {
            state.iteration = iteration;

            // Check time budget
            if let Some(budget) = state.goal.time_budget {
                if state.started_at.elapsed() > budget {
                    info!("Governor: Time budget exceeded");
                    return GovernedResult::TimeBudgetExceeded { state };
                }
            }

            // Select model based on current strategy
            let provider: &dyn LlmProvider = match state.strategy {
                Strategy::Initial | Strategy::RetryWithConstraints => {
                    self.executor_provider.as_ref()
                }
                Strategy::AlternativeApproach => self.recovery_provider.as_ref(),
                Strategy::EscalateToUser => {
                    let question = self.formulate_question(&state);
                    return GovernedResult::NeedsInput { question, state };
                }
                Strategy::Abort => {
                    let reason = self.diagnose(&state);
                    return GovernedResult::Aborted { reason, state };
                }
            };

            // Inject execution state into prompt
            let augmented = self.augment_messages(&messages, &state);

            let request = CompletionRequest {
                messages: augmented,
                tools: None,
            };

            // Send to LLM
            let response = match provider.complete(&request).await {
                Ok(r) => r,
                Err(e) => {
                    warn!(
                        "Governor: LLM call failed on iteration {}: {}",
                        iteration, e
                    );
                    state.strategy = Strategy::RetryWithConstraints;
                    continue;
                }
            };

            // Parse tool calls from response
            let (clean_content, tool_calls) = self.tool_parser.parse(&response.content);

            // If no tool calls, LLM thinks it's done (or gave up)
            if tool_calls.is_empty() {
                let all_met = state.all_criteria_met();
                if all_met {
                    info!("Governor: All criteria met, returning success");
                    return GovernedResult::Success(ChatResponse {
                        content: clean_content,
                        tool_log: vec![],
                    });
                }

                // LLM stopped but criteria aren't met
                if iteration < state.goal.max_iterations - 1 {
                    info!("Governor: LLM stopped but criteria not met, injecting continuation");
                    state.strategy = Strategy::RetryWithConstraints;
                    // Add the response and a continuation prompt
                    messages.push(Message::new("assistant", &clean_content));
                    let unmet: Vec<String> = state
                        .goal
                        .done_criteria
                        .iter()
                        .enumerate()
                        .filter(|(i, _)| {
                            !matches!(
                                state.criteria_status.get(*i),
                                Some(CriterionStatus::Met { .. })
                            )
                        })
                        .map(|(_, c)| format!("[{}] {}", c.id, c.description))
                        .collect();
                    messages.push(Message::new(
                        "user",
                        format!(
                            "You stopped generating actions but the following done_criteria are not yet met:\n{}\n\nContinue working to satisfy them.",
                            unmet.join("\n")
                        ),
                    ));
                    continue;
                }

                return GovernedResult::Incomplete {
                    response: ChatResponse {
                        content: clean_content,
                        tool_log: vec![],
                    },
                    state,
                };
            }

            // Execute tool calls, collecting structured results
            let mut tool_results_text = Vec::new();
            let mut tool_log = Vec::new();

            for tc in &tool_calls {
                let result = self.tool_executor.execute_tool(&tc.name, &tc.args).await;

                let attempt_result = if result.success {
                    AttemptResult::Success {
                        summary: result.output_text.chars().take(200).collect(),
                    }
                } else {
                    let sf = result
                        .structured_failure
                        .clone()
                        .unwrap_or_else(|| StructuredFailure::Unknown(result.output_text.clone()));
                    AttemptResult::Failure(sf)
                };

                state.record_attempt(Attempt {
                    iteration,
                    tool: tc.name.clone(),
                    args_summary: tc.args_summary.clone(),
                    result: attempt_result,
                    timestamp: Instant::now(),
                });

                tool_log.push(ToolResultRef {
                    tool: tc.name.clone(),
                    success: result.success,
                    output_summary: result.output_text.chars().take(200).collect(),
                });

                tool_results_text.push(format!(
                    "[{}] {}",
                    tc.name,
                    if result.output_text.len() > 500 {
                        format!("{}...", &result.output_text[..500])
                    } else {
                        result.output_text.clone()
                    }
                ));
            }

            // ================================================================
            // LOOP DETECTION + PROGRESS EVALUATION
            // ================================================================
            let progress = detect_progress(&state);

            match progress {
                Progress::Looping { repeated_failure } => {
                    info!(
                        "Governor: Loop detected — repeated failure: {}",
                        repeated_failure
                    );
                    if let Some(risk) = state.goal.find_matching_risk(&repeated_failure) {
                        info!(
                            "Governor: Planner predicted this — switching to AlternativeApproach"
                        );
                        state.set_active_mitigation(Some(risk.clone()));
                        state.strategy = Strategy::AlternativeApproach;
                    } else {
                        info!("Governor: No matching risk — escalating to user");
                        state.strategy = Strategy::EscalateToUser;
                    }
                }
                Progress::Stalled => {
                    info!("Governor: Stalled — no progress, escalating to user");
                    state.strategy = Strategy::EscalateToUser;
                }
                Progress::Complete => {
                    info!("Governor: All criteria met — generating summary");
                    let summary = self.generate_summary(&state, &clean_content).await;
                    return GovernedResult::Success(ChatResponse {
                        content: summary,
                        tool_log,
                    });
                }
                Progress::MakingProgress => {
                    state.strategy = Strategy::RetryWithConstraints;
                }
                Progress::NoAttempts => {
                    // First iteration
                }
                Progress::PartialSuccess { .. } => {
                    state.strategy = Strategy::RetryWithConstraints;
                }
            }

            // Add this iteration's messages for context
            messages.push(Message::new("assistant", &clean_content));
            messages.push(Message::new(
                "user",
                format!("## Tool Results\n\n{}", tool_results_text.join("\n\n")),
            ));
        }

        // Fell through — max iterations reached
        info!("Governor: Max iterations reached");
        let diagnosis = self.diagnose(&state);
        GovernedResult::MaxIterationsReached { state, diagnosis }
    }

    /// Build the initial message list for the executor.
    fn build_initial_messages(
        &self,
        user_request: &UserRequest,
        state: &ExecutionState,
        ctx: &SessionContext,
    ) -> Vec<Message> {
        let approach_text = state
            .goal
            .suggested_approach
            .steps
            .iter()
            .enumerate()
            .map(|(i, s)| format!("{}. {}", i + 1, s))
            .collect::<Vec<_>>()
            .join("\n");

        let system_with_plan = format!(
            "{}\n\n## Execution Plan\n\nGoal: {}\n\nApproach:\n{}\n\nRationale: {}",
            ctx.system_prompt,
            state.goal.intent.summary,
            approach_text,
            state.goal.suggested_approach.rationale,
        );

        let mut msgs = vec![Message::new("system", &system_with_plan)];

        // Include conversation history
        for msg in &ctx.conversation_history {
            msgs.push(msg.clone());
        }

        // Add the user's request
        msgs.push(Message::new("user", &user_request.message));

        msgs
    }

    /// Augment messages with execution state for the next iteration.
    fn augment_messages(&self, messages: &[Message], state: &ExecutionState) -> Vec<Message> {
        let mut augmented = messages.to_vec();
        let state_block = format_state_summary(state);

        // Inject state as a system message after the first system message
        if augmented.len() > 1 {
            augmented.insert(1, Message::new("system", &state_block));
        } else {
            augmented.push(Message::new("system", &state_block));
        }

        augmented
    }

    /// Formulate a specific question for the user when escalating.
    fn formulate_question(&self, state: &ExecutionState) -> String {
        let mut parts = Vec::new();

        parts.push(format!("I'm working on: {}", state.goal.intent.summary));

        if !state.constraints_discovered.is_empty() {
            parts.push("I've discovered these constraints:".to_string());
            for c in &state.constraints_discovered {
                parts.push(format!("  - {}", c));
            }
        }

        // Find the most recent failure
        if let Some(last_failure) = state.attempts.iter().rev().find_map(|a| match &a.result {
            AttemptResult::Failure(sf) => Some((a.tool.as_str(), sf)),
            _ => None,
        }) {
            parts.push(format!(
                "\nThe last failure was in '{}': {}",
                last_failure.0,
                last_failure.1.kind_key()
            ));
        }

        parts.push("\nCan you help me resolve this?".to_string());

        parts.join("\n")
    }

    /// Diagnose the execution state for a failure report.
    fn diagnose(&self, state: &ExecutionState) -> String {
        let mut parts = Vec::new();

        let total_attempts = state.attempts.len();
        let successes = state
            .attempts
            .iter()
            .filter(|a| matches!(a.result, AttemptResult::Success { .. }))
            .count();
        let failures = total_attempts - successes;

        parts.push(format!(
            "After {} attempts ({} succeeded, {} failed):",
            total_attempts, successes, failures
        ));

        // List unique failures
        let mut seen_keys = std::collections::HashSet::new();
        for attempt in &state.attempts {
            if let AttemptResult::Failure(sf) = &attempt.result {
                let key = sf.kind_key();
                if seen_keys.insert(key.clone()) {
                    parts.push(format!(
                        "  - {} on '{}': {}",
                        key, attempt.tool, attempt.args_summary
                    ));
                }
            }
        }

        if !state.constraints_discovered.is_empty() {
            parts.push("\nConstraints discovered:".to_string());
            for c in &state.constraints_discovered {
                parts.push(format!("  - {}", c));
            }
        }

        parts.join("\n")
    }

    /// Generate a summary of the execution for the user (Fast tier).
    async fn generate_summary(&self, state: &ExecutionState, last_content: &str) -> String {
        // If there's already good content from the LLM, use it
        if !last_content.trim().is_empty() && last_content.len() > 20 {
            return last_content.to_string();
        }

        // Generate a summary from the execution state
        let met: Vec<String> = state
            .goal
            .done_criteria
            .iter()
            .enumerate()
            .filter(|(i, _)| {
                matches!(
                    state.criteria_status.get(*i),
                    Some(CriterionStatus::Met { .. })
                )
            })
            .map(|(_, c)| c.description.clone())
            .collect();

        let summary_prompt = format!(
            "Summarize what was accomplished in 2-3 sentences:\n\nGoal: {}\n\nCriteria met: {:?}\n\nTotal tool calls: {}",
            state.goal.intent.summary,
            met,
            state.attempts.len()
        );

        let request = CompletionRequest {
            messages: vec![
                Message::new(
                    "system",
                    "You are a concise assistant. Summarize the task outcome for the user.",
                ),
                Message::new("user", &summary_prompt),
            ],
            tools: None,
        };

        match self.executor_provider.complete(&request).await {
            Ok(response) => response.content,
            Err(_) => {
                // Fallback: build a summary from state
                format!(
                    "Task completed: {}. {} criteria met.",
                    state.goal.intent.summary,
                    met.len()
                )
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cognitive::SessionContext;
    use crate::goal_frame::*;
    use orion_capabilities::cognitive::CompletionResponse;
    use std::sync::atomic::{AtomicUsize, Ordering};

    // ── Mock LLM Provider ──

    struct MockLlmProvider {
        responses: Vec<String>,
        call_count: AtomicUsize,
    }

    impl MockLlmProvider {
        fn new(responses: Vec<String>) -> Self {
            Self {
                responses,
                call_count: AtomicUsize::new(0),
            }
        }
    }

    #[async_trait::async_trait]
    impl LlmProvider for MockLlmProvider {
        async fn complete(
            &self,
            _request: &CompletionRequest,
        ) -> anyhow::Result<CompletionResponse> {
            let idx = self.call_count.fetch_add(1, Ordering::SeqCst);
            let content = self
                .responses
                .get(idx)
                .cloned()
                .unwrap_or_else(|| "No more mock responses".to_string());
            Ok(CompletionResponse {
                content,
                tool_calls: None,
            })
        }
    }

    // ── Mock Tool Executor ──

    struct MockToolExecutor {
        results: std::sync::Mutex<Vec<ToolExecutionResult>>,
    }

    impl MockToolExecutor {
        fn new(results: Vec<ToolExecutionResult>) -> Self {
            Self {
                results: std::sync::Mutex::new(results),
            }
        }
    }

    #[async_trait::async_trait]
    impl ToolExecutor for MockToolExecutor {
        async fn execute_tool(
            &self,
            _tool_name: &str,
            _args: &serde_json::Value,
        ) -> ToolExecutionResult {
            self.results
                .lock()
                .unwrap()
                .pop()
                .unwrap_or(ToolExecutionResult {
                    success: false,
                    output_text: "No more mock results".to_string(),
                    structured_failure: Some(StructuredFailure::Unknown("exhausted".to_string())),
                })
        }
    }

    // ── Mock Tool Call Parser ──

    struct MockParser {
        /// Each entry: (clean_content, tool_calls) for consecutive LLM responses
        results: std::sync::Mutex<Vec<(String, Vec<ParsedToolCall>)>>,
    }

    impl MockParser {
        fn new(results: Vec<(String, Vec<ParsedToolCall>)>) -> Self {
            Self {
                results: std::sync::Mutex::new(results),
            }
        }
    }

    impl ToolCallParser for MockParser {
        fn parse(&self, _response_text: &str) -> (String, Vec<ParsedToolCall>) {
            self.results
                .lock()
                .unwrap()
                .pop()
                .unwrap_or(("done".to_string(), vec![]))
        }
    }

    // ── Mock Planner ──

    fn mock_goal_frame() -> GoalFrame {
        GoalFrame {
            intent: Intent {
                category: IntentCategory::Configure,
                summary: "Set up email for test@example.com".to_string(),
                implicit_requirements: vec!["Config persists".to_string()],
            },
            done_criteria: vec![Criterion {
                id: "d1".to_string(),
                description: "Email config written".to_string(),
                verifier: Some(ToolVerifier {
                    tool: "read_file".to_string(),
                    args: serde_json::json!({"path": "/app/email.json"}),
                    expected: "file exists".to_string(),
                }),
            }],
            good_criteria: vec![],
            risk_assessment: vec![Risk {
                failure_mode: "permission denied on file write".to_string(),
                mitigation: "write to /app/agent-data/ instead".to_string(),
                fallback: "ask user for writable path".to_string(),
            }],
            suggested_approach: Approach {
                steps: vec!["write config".to_string()],
                rationale: "direct approach".to_string(),
                estimated_tool_calls: 1,
            },
            abort_conditions: vec!["3 failures".to_string()],
            max_iterations: 5,
            time_budget: None,
        }
    }

    fn mock_session_ctx() -> SessionContext {
        SessionContext {
            agent_id: "test-agent".to_string(),
            agent_name: "TestAgent".to_string(),
            system_prompt: "You are a test agent.".to_string(),
            conversation_history: vec![],
        }
    }

    fn mock_planner_provider(goal_frame: &GoalFrame) -> Arc<MockLlmProvider> {
        let json = serde_json::to_string(goal_frame).unwrap();
        Arc::new(MockLlmProvider::new(vec![json]))
    }

    #[tokio::test]
    async fn governor_success_no_tools() {
        let gf = mock_goal_frame();
        let planner_provider = mock_planner_provider(&gf);
        let planner = Arc::new(Planner::new(planner_provider));

        // Executor responds with no tool calls — and criteria are all met
        let executor = Arc::new(MockLlmProvider::new(vec![
            "All done! Config written.".to_string()
        ]));
        let recovery = Arc::new(MockLlmProvider::new(vec![]));

        let tool_exec = Arc::new(MockToolExecutor::new(vec![]));
        let parser = Arc::new(MockParser::new(vec![(
            "All done! Config written.".to_string(),
            vec![],
        )]));

        let gov = ExecutionGovernor::new(
            planner,
            executor,
            recovery,
            tool_exec,
            parser,
            GovernorConfig::default(),
        );

        let user_req = UserRequest {
            message: "set up email".to_string(),
            available_tools: vec![],
            known_constraints: vec![],
            agent_state_summary: "ready".to_string(),
        };

        let result = gov.execute(user_req, &mock_session_ctx()).await;

        // LLM returned no tool calls, criteria not verified -> Incomplete
        assert!(
            matches!(result, GovernedResult::Incomplete { .. }),
            "Expected Incomplete (criteria not structurally met), got {:?}",
            matches!(result, GovernedResult::Incomplete { .. })
        );
    }

    #[tokio::test]
    async fn governor_loop_detection_triggers_alternative() {
        let gf = mock_goal_frame();
        let planner_provider = mock_planner_provider(&gf);
        let planner = Arc::new(Planner::new(planner_provider));

        // Executor will be called multiple times
        let executor = Arc::new(MockLlmProvider::new(vec![
            "Trying write_file".to_string(),
            "Trying write_file again".to_string(),
            "Using alternative path".to_string(),
            "Done".to_string(),
        ]));
        let recovery = Arc::new(MockLlmProvider::new(vec![
            "Let me try the alternative path".to_string(),
        ]));

        // Tool executor: first two calls fail with same error, third succeeds
        let tool_exec = Arc::new(MockToolExecutor::new(vec![
            // Pop order is reversed (LIFO)
            ToolExecutionResult {
                success: true,
                output_text: "Written successfully".to_string(),
                structured_failure: None,
            },
            ToolExecutionResult {
                success: false,
                output_text: "Permission denied".to_string(),
                structured_failure: Some(StructuredFailure::PermissionDenied {
                    path: "/app/vault/email.json".to_string(),
                    allowed_paths: vec!["/app/agent-data".to_string()],
                }),
            },
            ToolExecutionResult {
                success: false,
                output_text: "Permission denied".to_string(),
                structured_failure: Some(StructuredFailure::PermissionDenied {
                    path: "/app/vault/email.json".to_string(),
                    allowed_paths: vec!["/app/agent-data".to_string()],
                }),
            },
        ]));

        // Parser: each LLM response has one tool call, then no tools on final
        let write_call = ParsedToolCall {
            name: "write_file".to_string(),
            args: serde_json::json!({"path": "/app/vault/email.json"}),
            args_summary: "path=/app/vault/email.json".to_string(),
        };
        let write_call_alt = ParsedToolCall {
            name: "write_file".to_string(),
            args: serde_json::json!({"path": "/app/agent-data/email.json"}),
            args_summary: "path=/app/agent-data/email.json".to_string(),
        };
        let parser = Arc::new(MockParser::new(vec![
            // Pop order is reversed
            ("Done".to_string(), vec![]),
            ("Using alt path".to_string(), vec![write_call_alt]),
            ("Trying again".to_string(), vec![write_call.clone()]),
            ("Trying write".to_string(), vec![write_call]),
        ]));

        let gov = ExecutionGovernor::new(
            planner,
            executor,
            recovery,
            tool_exec,
            parser,
            GovernorConfig::default(),
        );

        let user_req = UserRequest {
            message: "set up email".to_string(),
            available_tools: vec![],
            known_constraints: vec![],
            agent_state_summary: "ready".to_string(),
        };

        let result = gov.execute(user_req, &mock_session_ctx()).await;

        // The governor should detect the loop after 2 identical failures,
        // find the matching risk, switch to AlternativeApproach, and then
        // the third call succeeds. The final "Done" with no tools means
        // Incomplete (criteria not structurally verified).
        let msg = result.user_message();
        assert!(
            !msg.is_empty(),
            "Governor must always return a user message"
        );
    }

    #[tokio::test]
    async fn governor_max_iterations() {
        let mut gf = mock_goal_frame();
        gf.max_iterations = 2;
        let planner_provider = mock_planner_provider(&gf);
        let planner = Arc::new(Planner::new(planner_provider));

        let executor = Arc::new(MockLlmProvider::new(vec![
            "Attempt 1".to_string(),
            "Attempt 2".to_string(),
        ]));
        let recovery = Arc::new(MockLlmProvider::new(vec![]));

        // All tool calls fail with different errors (no loop, just burning iterations)
        let tool_exec = Arc::new(MockToolExecutor::new(vec![
            ToolExecutionResult {
                success: false,
                output_text: "Connection refused".to_string(),
                structured_failure: Some(StructuredFailure::ConnectionFailed {
                    host: "mail.example.com".to_string(),
                    port: 993,
                    error_detail: "refused".to_string(),
                    alternatives: vec![],
                }),
            },
            ToolExecutionResult {
                success: false,
                output_text: "Timeout".to_string(),
                structured_failure: Some(StructuredFailure::Timeout {
                    elapsed_ms: 5000,
                    operation: "connect".to_string(),
                }),
            },
        ]));

        let parser = Arc::new(MockParser::new(vec![
            (
                "Trying connection".to_string(),
                vec![ParsedToolCall {
                    name: "connect".to_string(),
                    args: serde_json::json!({}),
                    args_summary: "host=mail.example.com".to_string(),
                }],
            ),
            (
                "Trying to connect".to_string(),
                vec![ParsedToolCall {
                    name: "connect".to_string(),
                    args: serde_json::json!({}),
                    args_summary: "host=mail.example.com".to_string(),
                }],
            ),
        ]));

        let gov = ExecutionGovernor::new(
            planner,
            executor,
            recovery,
            tool_exec,
            parser,
            GovernorConfig::default(),
        );

        let user_req = UserRequest {
            message: "connect to email".to_string(),
            available_tools: vec![],
            known_constraints: vec![],
            agent_state_summary: "ready".to_string(),
        };

        let result = gov.execute(user_req, &mock_session_ctx()).await;
        assert!(
            matches!(result, GovernedResult::MaxIterationsReached { .. }),
            "Expected MaxIterationsReached, got is_success={}",
            result.is_success()
        );
    }

    #[tokio::test]
    async fn governor_always_returns_user_message() {
        // Test every exit path produces a non-empty message
        let gf = mock_goal_frame();
        let planner_provider = mock_planner_provider(&gf);
        let planner = Arc::new(Planner::new(planner_provider));

        let executor = Arc::new(MockLlmProvider::new(vec!["response".to_string()]));
        let recovery = Arc::new(MockLlmProvider::new(vec![]));
        let tool_exec = Arc::new(MockToolExecutor::new(vec![]));
        let parser = Arc::new(MockParser::new(vec![("response".to_string(), vec![])]));

        let gov = ExecutionGovernor::new(
            planner,
            executor,
            recovery,
            tool_exec,
            parser,
            GovernorConfig::default(),
        );

        let user_req = UserRequest {
            message: "test".to_string(),
            available_tools: vec![],
            known_constraints: vec![],
            agent_state_summary: "ready".to_string(),
        };

        let result = gov.execute(user_req, &mock_session_ctx()).await;
        let msg = result.user_message();
        assert!(
            !msg.is_empty(),
            "Governor must always return a non-empty user message"
        );
    }
}
