//! Autonomous agentic loop.
//!
//! Runs a multi-turn LLM loop where the agent receives a high-level goal,
//! autonomously researches/plans/executes/observes/iterates, and only
//! consults the mentor at genuine decision points via `ask_mentor`.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex as StdMutex};

use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, mpsc, oneshot, Mutex};

use orion_birth::{execute_store_provider_key, BirthToolRequest};
use orion_capabilities::cognitive::Message;
use orion_core::system_prompt::{build_agentic_system_prompt, RuntimeContext, SkillToolEntry};
use orion_core::{AppConfig, McpServerDefinition, ProviderKeyring, SkillKeychain, ThinkingModelTier};
use orion_skills::manifest::SkillId;
use orion_skills::protocol::mcp::McpSkillRuntime;
use orion_skills::skill::{Skill, ToolDescriptor};
use orion_skills::{SkillExecutor, SkillRegistry, TrustTier};

use crate::tool_extraction;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Request body for starting an autonomous agentic run.
#[derive(Debug, Clone, Deserialize)]
pub struct AgenticRunRequest {
    pub goal: String,
    #[serde(default = "default_max_turns")]
    pub max_turns: u32,
    #[serde(default)]
    pub auto_approve_safe_tools: bool,
    #[serde(default)]
    pub router_mode: AgenticRouterMode,
}

fn default_max_turns() -> u32 {
    15
}

/// Controls how deeply the agentic loop reasons: Auto, ThinkHard, or ThinkHarder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AgenticRouterMode {
    #[default]
    Auto,
    ThinkHard,
    ThinkHarder,
}

/// Lifecycle state of an in-flight agentic task.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgenticTaskStatus {
    Running,
    WaitingForMentor,
    WaitingForConfirmation,
    Completed,
    Failed,
    Cancelled,
}

/// A single step in the agentic run (audit trail).
#[derive(Debug, Clone, Serialize)]
pub struct AgenticStep {
    pub turn: u32,
    pub step_type: String,
    pub content: String,
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

/// SSE event sent to the frontend.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "event", content = "data")]
pub enum AgenticEvent {
    Thinking {
        turn: u32,
        content: String,
    },
    ToolCall {
        turn: u32,
        tool_name: String,
        arguments: serde_json::Value,
    },
    ToolResult {
        turn: u32,
        tool_name: String,
        success: bool,
        output: String,
    },
    MentorNeeded {
        turn: u32,
        question: String,
    },
    ConfirmationNeeded {
        turn: u32,
        tool_name: String,
        arguments: serde_json::Value,
    },
    Done {
        summary: String,
        status: String,
        turns_used: u32,
        tool_calls: u32,
    },
    Error {
        message: String,
    },
}

/// Holds the state of a running agentic task.
///
/// Fields are read and mutated via `Arc<Mutex<AgenticTask>>` across the
/// spawned task and the API handler threads.
#[allow(dead_code)] // Fields accessed via Arc<Mutex<>> in spawned tasks; compiler cannot trace cross-task reads
pub struct AgenticTask {
    pub id: String,
    pub agent_id: String,
    pub goal: String,
    pub status: AgenticTaskStatus,
    pub event_tx: broadcast::Sender<AgenticEvent>,
    pub mentor_response_tx: Option<oneshot::Sender<String>>,
    pub confirmation_tx: Option<oneshot::Sender<bool>>,
    pub steps: Vec<AgenticStep>,
    pub turn: u32,
    pub tool_calls: u32,
    pub cancel_tx: mpsc::Sender<()>,
    pub started_at: chrono::DateTime<chrono::Utc>,
    pub completed_at: Option<std::time::Instant>,
}

impl AgenticTask {
    /// Returns true when the task is in a terminal state (completed, failed, or cancelled).
    pub fn is_terminal(&self) -> bool {
        matches!(
            self.status,
            AgenticTaskStatus::Completed
                | AgenticTaskStatus::Failed
                | AgenticTaskStatus::Cancelled
        )
    }

    /// Returns elapsed time since the task reached a terminal state.
    /// Returns `Duration::ZERO` if not yet terminal.
    pub fn completed_at_elapsed(&self) -> std::time::Duration {
        self.completed_at
            .map(|t| t.elapsed())
            .unwrap_or(std::time::Duration::ZERO)
    }
}

/// Mentor response request body.
#[derive(Debug, Deserialize)]
pub struct MentorResponseRequest {
    pub task_id: String,
    pub response: String,
}

/// Confirmation response request body.
#[derive(Debug, Deserialize)]
pub struct ConfirmationResponseRequest {
    pub task_id: String,
    pub approved: bool,
}

/// Cancel request body.
#[derive(Debug, Deserialize)]
pub struct CancelRequest {
    pub task_id: String,
}

/// Response when starting an agentic run.
#[derive(Debug, Serialize)]
pub struct AgenticRunResponse {
    pub task_id: String,
    pub stream_url: String,
}

/// Response for status check.
#[derive(Debug, Serialize)]
pub struct AgenticStatusResponse {
    pub task_id: String,
    pub goal: String,
    pub status: AgenticTaskStatus,
    pub turn: u32,
    pub steps: Vec<AgenticStep>,
}

// ---------------------------------------------------------------------------
// Tool resolution helper (shared with single-turn chat)
// ---------------------------------------------------------------------------

/// Find which skill owns a given tool name and return its ID + descriptor.
pub fn find_skill_for_tool(
    registry: &SkillRegistry,
    tool_name: &str,
) -> Option<(SkillId, ToolDescriptor)> {
    let skills = registry.list_with_tiers().ok()?;
    for (manifest, _tier) in &skills {
        if let Ok((skill, _, _)) = registry.get_skill(&manifest.id) {
            for tool in skill.tools() {
                if tool.name == tool_name {
                    return Some((manifest.id.clone(), tool));
                }
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Core agentic loop
// ---------------------------------------------------------------------------

/// Configuration bundle passed to [`run_agentic_loop`] when spawning an autonomous task.
pub struct AgenticLoopConfig {
    pub task_id: String,
    pub agent_id: String,
    pub goal: String,
    pub max_turns: u32,
    pub auto_approve_safe_tools: bool,
    pub router_mode: AgenticRouterMode,
    pub agent_dir: PathBuf,
    pub config: AppConfig,
    pub skill_registry: Arc<SkillRegistry>,
    pub skill_executor: Arc<SkillExecutor>,
    pub provider_keyring: Arc<std::sync::RwLock<ProviderKeyring>>,
    pub skill_keychain: Arc<StdMutex<SkillKeychain>>,
    pub email_accounts: Arc<tokio::sync::RwLock<Vec<orion_core::config::EmailAccountConfig>>>,
    pub skill_tool_entries: Vec<SkillToolEntry>,
    pub stored_providers: Vec<String>,
    pub event_tx: broadcast::Sender<AgenticEvent>,
    pub cancel_rx: mpsc::Receiver<()>,
    pub task_handle: Arc<Mutex<AgenticTask>>,
    pub started_at: chrono::DateTime<chrono::Utc>,
    pub run_source: String,
    pub superego_l2_mode: orion_router::SuperegoL2Mode,
    /// When true and `router_mode` is `ThinkHarder`, the loop uses the Pro
    /// council (MoA DAG) to generate a reasoning plan, then feeds that plan
    /// into a standard tool-capable model for execution.  Requires 2+
    /// connected providers; degrades to standard routing otherwise.
    pub use_council: bool,
}

/// Run the autonomous agentic loop. Call from `tokio::spawn`.
pub async fn run_agentic_loop(mut cfg: AgenticLoopConfig) {
    let runtime_ctx = RuntimeContext {
        agent_id: cfg.agent_id.clone(),
        data_dir: cfg.agent_dir.to_string_lossy().to_string(),
    };
    let system_prompt = build_agentic_system_prompt(
        &cfg.config.docs_dir,
        &cfg.config.agent_name,
        &cfg.skill_tool_entries,
        &cfg.stored_providers,
        Some(&runtime_ctx),
    );

    let mut messages: Vec<Message> = vec![
        Message::new("system", &system_prompt),
        Message::new(
            "user",
            build_goal_kickoff_prompt(&cfg.goal, cfg.router_mode, &cfg.run_source),
        ),
    ];

    // Build structured tool definitions for tool-aware routing (includes synthetic agentic tools).
    let mut tool_defs = tool_extraction::build_agentic_tool_definitions(&cfg.skill_tool_entries);

    // Build initial router — credentials are re-resolved every turn inside the loop,
    // but we need an initial router for the pre-flight heartbeat check.
    let mut router = build_router_from_keyring(&cfg).await;

    // Pre-flight: verify at least one LLM is reachable before entering the loop.
    if let Err(e) = router.heartbeat().await {
        let err_msg = format!("LLM health check failed before starting task: {}", e);
        tracing::error!("{}", err_msg);
        let _ = cfg.event_tx.send(AgenticEvent::Error {
            message: err_msg.clone(),
        });
        update_status(&cfg.task_handle, AgenticTaskStatus::Failed, 0).await;
        persist_run_summary(
            &cfg.agent_dir,
            &cfg.task_id,
            &cfg.goal,
            &cfg.run_source,
            &err_msg,
            "failed",
            0,
            0,
            cfg.started_at,
        );
        return;
    }

    let mut turn: u32 = 0;
    let mut total_tool_calls: u32 = 0;
    let mut tools_changed = false;

    loop {
        // Re-resolve credentials from disk every turn so newly stored API keys
        // (e.g. via store_provider_key) are picked up immediately instead of
        // falling back to the local Id model with stale credentials.
        router = build_router_from_keyring(&cfg).await;
        // Refresh system prompt and tool definitions when tools have been added mid-run.
        if tools_changed {
            let refreshed_entries = crate::build_skill_tool_entries(&cfg.skill_registry);
            let refreshed_prompt = build_agentic_system_prompt(
                &cfg.config.docs_dir,
                &cfg.config.agent_name,
                &refreshed_entries,
                &cfg.stored_providers,
                Some(&runtime_ctx),
            );
            if !messages.is_empty() {
                messages[0] = Message::new("system", &refreshed_prompt);
            }
            tool_defs = tool_extraction::build_agentic_tool_definitions(&refreshed_entries);
            cfg.skill_tool_entries = refreshed_entries;
            tools_changed = false;
            tracing::info!(agent_dir = %cfg.agent_dir.display(), "agentic: refreshed tool list after skill registration");
        }

        if turn >= cfg.max_turns {
            let summary = format!(
                "Reached maximum turns limit ({}). Task may be incomplete.",
                cfg.max_turns
            );
            let _ = cfg.event_tx.send(AgenticEvent::Done {
                summary: summary.clone(),
                status: "partial".to_string(),
                turns_used: turn,
                tool_calls: total_tool_calls,
            });
            update_status(&cfg.task_handle, AgenticTaskStatus::Completed, turn).await;
            persist_run_summary(
                &cfg.agent_dir,
                &cfg.task_id,
                &cfg.goal,
                &cfg.run_source,
                &summary,
                "partial",
                turn,
                total_tool_calls,
                cfg.started_at,
            );
            break;
        }

        // Check for cancellation (non-blocking)
        if cfg.cancel_rx.try_recv().is_ok() {
            let _ = cfg.event_tx.send(AgenticEvent::Done {
                summary: "Task cancelled by mentor.".to_string(),
                status: "cancelled".to_string(),
                turns_used: turn,
                tool_calls: total_tool_calls,
            });
            update_status(&cfg.task_handle, AgenticTaskStatus::Cancelled, turn).await;
            persist_run_summary(
                &cfg.agent_dir,
                &cfg.task_id,
                &cfg.goal,
                &cfg.run_source,
                "Task cancelled by mentor.",
                "cancelled",
                turn,
                total_tool_calls,
                cfg.started_at,
            );
            break;
        }

        turn += 1;

        // Trim context if conversation is getting long
        trim_context(&mut messages, context_token_budget(cfg.router_mode));

        // LLM call — council path for ThinkHarder, standard routing otherwise.
        //
        // Council generates a deep-reasoning plan via multi-provider MoA DAG,
        // but cannot do structured tool calling. So when council produces a
        // plan we feed it into a standard tool-capable model for execution.
        let response = if cfg.use_council && cfg.router_mode == AgenticRouterMode::ThinkHarder {
            match council_then_execute(&cfg, &messages, &tool_defs, &router).await {
                Ok(r) => r,
                Err(e) => {
                    let err_msg = format!("Council+execution failed: {}", e);
                    let _ = cfg.event_tx.send(AgenticEvent::Error {
                        message: err_msg.clone(),
                    });
                    update_status(&cfg.task_handle, AgenticTaskStatus::Failed, turn).await;
                    persist_run_summary(
                        &cfg.agent_dir,
                        &cfg.task_id,
                        &cfg.goal,
                        &cfg.run_source,
                        &err_msg,
                        "failed",
                        turn,
                        total_tool_calls,
                        cfg.started_at,
                    );
                    break;
                }
            }
        } else {
            match router
                .route_with_tools(messages.clone(), tool_defs.clone())
                .await
            {
                Ok(r) => r,
                Err(e) => {
                    let err_msg = format!("LLM call failed: {}", e);
                    let _ = cfg.event_tx.send(AgenticEvent::Error {
                        message: err_msg.clone(),
                    });
                    update_status(&cfg.task_handle, AgenticTaskStatus::Failed, turn).await;
                    persist_run_summary(
                        &cfg.agent_dir,
                        &cfg.task_id,
                        &cfg.goal,
                        &cfg.run_source,
                        &err_msg,
                        "failed",
                        turn,
                        total_tool_calls,
                        cfg.started_at,
                    );
                    break;
                }
            }
        };

        // Unified tool extraction: structured first, legacy text fallback.
        let extraction = tool_extraction::extract_tool_calls(&response);
        let clean_content = extraction.clean_content;
        let tool_requests: Vec<BirthToolRequest> = extraction
            .tool_calls
            .iter()
            .map(BirthToolRequest::from)
            .collect();

        // Emit thinking event
        if !clean_content.trim().is_empty() {
            let _ = cfg.event_tx.send(AgenticEvent::Thinking {
                turn,
                content: clean_content.clone(),
            });
            record_step(&cfg.task_handle, turn, "thinking", &clean_content).await;
        }

        // Append assistant message
        messages.push(Message::new("assistant", &response.content));

        // Check for task_complete synthetic tool
        if let Some(tc) = tool_requests.iter().find(|t| t.name == "task_complete") {
            let summary = tc
                .arguments
                .get("summary")
                .and_then(|v| v.as_str())
                .unwrap_or("Task completed.")
                .to_string();
            let status = tc
                .arguments
                .get("status")
                .and_then(|v| v.as_str())
                .unwrap_or("success")
                .to_string();
            let _ = cfg.event_tx.send(AgenticEvent::Done {
                summary: summary.clone(),
                status: status.clone(),
                turns_used: turn,
                tool_calls: total_tool_calls,
            });
            record_step(&cfg.task_handle, turn, "done", &summary).await;
            update_status(&cfg.task_handle, AgenticTaskStatus::Completed, turn).await;

            // Persist agentic run summary
            persist_run_summary(
                &cfg.agent_dir,
                &cfg.task_id,
                &cfg.goal,
                &cfg.run_source,
                &summary,
                &status,
                turn,
                total_tool_calls,
                cfg.started_at,
            );
            break;
        }

        // Check for ask_mentor synthetic tool
        if let Some(am) = tool_requests.iter().find(|t| t.name == "ask_mentor") {
            let question = am
                .arguments
                .get("question")
                .and_then(|v| v.as_str())
                .unwrap_or("I need your input.")
                .to_string();

            let _ = cfg.event_tx.send(AgenticEvent::MentorNeeded {
                turn,
                question: question.clone(),
            });
            record_step(&cfg.task_handle, turn, "ask_mentor", &question).await;

            // Create a oneshot channel and store it in the task
            let (tx, rx) = oneshot::channel();
            {
                let mut task = cfg.task_handle.lock().await;
                task.status = AgenticTaskStatus::WaitingForMentor;
                task.mentor_response_tx = Some(tx);
                task.turn = turn;
            }

            // Wait for mentor response or cancellation
            tokio::select! {
                mentor_result = rx => {
                    match mentor_result {
                        Ok(response) => {
                            record_step(&cfg.task_handle, turn, "mentor_response", &response).await;
                            messages.push(Message::new(
                                "user",
                                format!("## Mentor Response\n\n{}", response),
                            ));
                            {
                                let mut task = cfg.task_handle.lock().await;
                                task.status = AgenticTaskStatus::Running;
                            }
                        }
                        Err(_) => {
                            // Channel dropped — task cancelled
                            let _ = cfg.event_tx.send(AgenticEvent::Done {
                                summary: "Task cancelled (mentor channel closed).".to_string(),
                                status: "cancelled".to_string(),
                                turns_used: turn,
                                tool_calls: total_tool_calls,
                            });
                            update_status(&cfg.task_handle, AgenticTaskStatus::Cancelled, turn).await;
                            persist_run_summary(
                                &cfg.agent_dir,
                                &cfg.task_id,
                                &cfg.goal,
                                &cfg.run_source,
                                "Task cancelled (mentor channel closed).",
                                "cancelled",
                                turn,
                                total_tool_calls,
                                cfg.started_at,
                            );
                            break;
                        }
                    }
                }
                _ = cfg.cancel_rx.recv() => {
                    let _ = cfg.event_tx.send(AgenticEvent::Done {
                        summary: "Task cancelled by mentor.".to_string(),
                        status: "cancelled".to_string(),
                        turns_used: turn,
                        tool_calls: total_tool_calls,
                    });
                    update_status(&cfg.task_handle, AgenticTaskStatus::Cancelled, turn).await;
                    persist_run_summary(
                        &cfg.agent_dir,
                        &cfg.task_id,
                        &cfg.goal,
                        &cfg.run_source,
                        "Task cancelled by mentor.",
                        "cancelled",
                        turn,
                        total_tool_calls,
                        cfg.started_at,
                    );
                    break;
                }
            }
            continue;
        }

        // Check for register_mcp_skill synthetic tool
        if let Some(reg) = tool_requests
            .iter()
            .find(|t| t.name == "register_mcp_skill")
        {
            let server_id = reg
                .arguments
                .get("server_id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let server_name = reg
                .arguments
                .get("server_name")
                .and_then(|v| v.as_str())
                .unwrap_or(&server_id)
                .to_string();
            let base_url = reg
                .arguments
                .get("base_url")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            let result_msg = if server_id.is_empty() || base_url.is_empty() {
                "register_mcp_skill failed: server_id and base_url are required.".to_string()
            } else {
                match register_mcp_skill_impl(
                    &server_id,
                    &server_name,
                    &base_url,
                    &cfg.config,
                    &cfg.skill_registry,
                    &cfg.agent_dir,
                )
                .await
                {
                    Ok(tool_names) => {
                        tools_changed = true;
                        format!(
                            "MCP skill '{}' registered successfully. Discovered tools: [{}]. They will be available on your next turn.",
                            server_name,
                            tool_names.join(", ")
                        )
                    }
                    Err(e) => {
                        format!("register_mcp_skill failed: {}", e)
                    }
                }
            };

            let _ = cfg.event_tx.send(AgenticEvent::ToolResult {
                turn,
                tool_name: "register_mcp_skill".to_string(),
                success: tools_changed,
                output: result_msg.clone(),
            });
            record_step(&cfg.task_handle, turn, "register_mcp_skill", &result_msg).await;
            messages.push(Message::new(
                "user",
                format!("## Tool Result: register_mcp_skill\n\n{}", result_msg),
            ));
            // Process remaining tool requests in this turn (don't skip to next turn).
        }

        // Check for register_email_account synthetic tool
        if let Some(email_req) = tool_requests
            .iter()
            .find(|t| t.name == "register_email_account")
        {
            let result_msg = match serde_json::from_value::<crate::routes::skills::RegisterEmailAccountRequest>(
                email_req.arguments.clone(),
            ) {
                Ok(request) => match crate::routes::skills::register_email_account_internal(
                    &cfg.agent_id,
                    &cfg.skill_keychain,
                    &cfg.email_accounts,
                    request,
                )
                .await
                {
                    Ok(outcome) => {
                        let probe_summary = outcome
                            .probes
                            .iter()
                            .map(|p| {
                                format!(
                                    "{}:{} => {} ({})",
                                    p.host,
                                    p.port,
                                    if p.reachable { "ok" } else { "failed" },
                                    p.message
                                )
                            })
                            .collect::<Vec<String>>()
                            .join("; ");
                        format!(
                            "Email account '{}' registered for {}. Connectivity: {}",
                            outcome.account_id, outcome.address, probe_summary
                        )
                    }
                    Err(e) => format!("register_email_account failed: {}", e),
                },
                Err(e) => format!("register_email_account arguments invalid: {}", e),
            };
            let success = !result_msg.starts_with("register_email_account failed")
                && !result_msg.starts_with("register_email_account arguments invalid");
            let _ = cfg.event_tx.send(AgenticEvent::ToolResult {
                turn,
                tool_name: "register_email_account".to_string(),
                success,
                output: result_msg.clone(),
            });
            record_step(
                &cfg.task_handle,
                turn,
                "register_email_account",
                &result_msg,
            )
            .await;
            messages.push(Message::new(
                "user",
                format!("## Tool Result: register_email_account\n\n{}", result_msg),
            ));
        }

        // Check for manage_job synthetic tool
        if let Some(mj) = tool_requests.iter().find(|t| t.name == "manage_job") {
            let result_msg = handle_manage_job(&cfg.agent_dir, &mj.arguments);
            let success = !result_msg.starts_with("manage_job failed");
            let _ = cfg.event_tx.send(AgenticEvent::ToolResult {
                turn,
                tool_name: "manage_job".to_string(),
                success,
                output: result_msg.clone(),
            });
            record_step(&cfg.task_handle, turn, "manage_job", &result_msg).await;
            messages.push(Message::new(
                "user",
                format!("## Tool Result: manage_job\n\n{}", result_msg),
            ));
        }

        // Check for manage_proxy synthetic tool
        if let Some(mp) = tool_requests.iter().find(|t| t.name == "manage_proxy") {
            let result_msg = handle_manage_proxy(&mp.arguments);
            let success = !result_msg.starts_with("manage_proxy failed");
            let _ = cfg.event_tx.send(AgenticEvent::ToolResult {
                turn,
                tool_name: "manage_proxy".to_string(),
                success,
                output: result_msg.clone(),
            });
            record_step(&cfg.task_handle, turn, "manage_proxy", &result_msg).await;
            messages.push(Message::new(
                "user",
                format!("## Tool Result: manage_proxy\n\n{}", result_msg),
            ));
        }

        // Check for write_review synthetic tool
        if let Some(wr) = tool_requests.iter().find(|t| t.name == "write_review") {
            let result_msg = handle_write_review(&cfg.agent_dir, &wr.arguments);
            let success = !result_msg.starts_with("write_review failed");
            let _ = cfg.event_tx.send(AgenticEvent::ToolResult {
                turn,
                tool_name: "write_review".to_string(),
                success,
                output: result_msg.clone(),
            });
            record_step(&cfg.task_handle, turn, "write_review", &result_msg).await;
            messages.push(Message::new(
                "user",
                format!("## Tool Result: write_review\n\n{}", result_msg),
            ));
        }

        // No tool calls and no synthetic tools — nudge the LLM
        if tool_requests.is_empty() {
            messages.push(Message::new(
                "user",
                "You didn't use any tools. Either use a tool to make progress on the goal, or use `task_complete` if you're done.",
            ));
            continue;
        }

        // Execute real tool calls
        let mut tool_results: Vec<String> = Vec::new();
        let last_user_for_policy = messages
            .iter()
            .rev()
            .find(|m| m.role == "user")
            .map(|m| m.content.clone())
            .unwrap_or_default();
        let advisory_code = match router.superego_check(&last_user_for_policy).await {
            orion_router::SuperegoResult::Advisory { code, .. } => code,
            _ => None,
        };
        let capability_envelope = orion_core::evaluate_user_message_for_capabilities(
            &last_user_for_policy,
            advisory_code.as_deref(),
        );
        let mut provider_config = cfg.config.clone();
        let mut provider_keyring = ProviderKeyring::load(provider_config.data_dir.clone())
            .unwrap_or_else(|_| ProviderKeyring::new(provider_config.data_dir.clone()));
        let mut skill_kc = SkillKeychain::load(provider_config.data_dir.clone())
            .unwrap_or_else(|_| SkillKeychain::new(provider_config.data_dir.clone()));
        for tr in &tool_requests {
            // Skip synthetic tools already handled above
            if tr.name == "task_complete"
                || tr.name == "ask_mentor"
                || tr.name == "register_mcp_skill"
                || tr.name == "register_email_account"
                || tr.name == "manage_job"
                || tr.name == "manage_proxy"
                || tr.name == "write_review"
            {
                continue;
            }

            total_tool_calls += 1;

            if tr.name == "store_secret" || tr.name == "store_provider_key" {
                let provider = tr
                    .arguments
                    .get("provider")
                    .and_then(|v| v.as_str())
                    .unwrap_or("auto");
                let key = tr
                    .arguments
                    .get("key")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let _ = cfg.event_tx.send(AgenticEvent::ToolCall {
                    turn,
                    tool_name: tr.name.clone(),
                    arguments: tr.arguments.clone(),
                });
                match execute_store_provider_key(
                    &mut provider_keyring,
                    &mut provider_config,
                    provider,
                    key,
                    Some(&mut skill_kc),
                ) {
                    Ok(resolved) => {
                        let output = format!("Stored key for provider '{}'.", resolved);
                        let _ = cfg.event_tx.send(AgenticEvent::ToolResult {
                            turn,
                            tool_name: tr.name.clone(),
                            success: true,
                            output: output.clone(),
                        });
                        tool_results.push(format!("**{}** (Success): {}", tr.name, output));
                    }
                    Err(e) => {
                        let output = format!("Failed to store provider key: {}", e);
                        let _ = cfg.event_tx.send(AgenticEvent::ToolResult {
                            turn,
                            tool_name: tr.name.clone(),
                            success: false,
                            output: output.clone(),
                        });
                        tool_results.push(format!("**{}** (Error): {}", tr.name, output));
                    }
                }
                continue;
            }

            if tr.name == "store_vault_secret" {
                let key_name = tr
                    .arguments
                    .get("key")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let value = tr
                    .arguments
                    .get("value")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let _ = cfg.event_tx.send(AgenticEvent::ToolCall {
                    turn,
                    tool_name: tr.name.clone(),
                    arguments: tr.arguments.clone(),
                });
                if key_name.is_empty() || value.is_empty() {
                    let output =
                        "Failed to store vault secret: key and value are required.".to_string();
                    let _ = cfg.event_tx.send(AgenticEvent::ToolResult {
                        turn,
                        tool_name: tr.name.clone(),
                        success: false,
                        output: output.clone(),
                    });
                    tool_results.push(format!("**{}** (Error): {}", tr.name, output));
                    continue;
                }
                skill_kc.set_secret(key_name, value);
                match skill_kc.save() {
                    Ok(()) => {
                        let output = format!("Stored vault secret '{}'.", key_name);
                        let _ = cfg.event_tx.send(AgenticEvent::ToolResult {
                            turn,
                            tool_name: tr.name.clone(),
                            success: true,
                            output: output.clone(),
                        });
                        tool_results.push(format!("**{}** (Success): {}", tr.name, output));
                    }
                    Err(e) => {
                        let output = format!("Failed to save skill secret '{}': {}", key_name, e);
                        let _ = cfg.event_tx.send(AgenticEvent::ToolResult {
                            turn,
                            tool_name: tr.name.clone(),
                            success: false,
                            output: output.clone(),
                        });
                        tool_results.push(format!("**{}** (Error): {}", tr.name, output));
                    }
                }
                continue;
            }

            // Find the skill for this tool
            let skill_match = find_skill_for_tool(&cfg.skill_registry, &tr.name);
            let (skill_id, tool_desc) = match skill_match {
                Some(s) => s,
                None => {
                    let err_msg = format!(
                        "Unknown tool: {}. Check available tools in your system prompt.",
                        tr.name
                    );
                    let _ = cfg.event_tx.send(AgenticEvent::ToolResult {
                        turn,
                        tool_name: tr.name.clone(),
                        success: false,
                        output: err_msg.clone(),
                    });
                    tool_results.push(format!("**{}**: Error — {}", tr.name, err_msg));
                    continue;
                }
            };

            let gate =
                orion_core::evaluate_tool_request(&tr.name, &tr.arguments, &capability_envelope);
            if !gate.allowed {
                let reason_code = gate.reason_code.unwrap_or("SAFETY_BLOCK");
                let reason_text = gate
                    .reason_text
                    .unwrap_or_else(|| "Blocked by capability safety policy".to_string());
                let err_msg = format!(
                    "Blocked by safety policy ({}): {}",
                    reason_code, reason_text
                );
                let _ = cfg.event_tx.send(AgenticEvent::ToolResult {
                    turn,
                    tool_name: tr.name.clone(),
                    success: false,
                    output: err_msg.clone(),
                });
                tool_results.push(format!("**{}**: Error — {}", tr.name, err_msg));
                continue;
            }

            // Check confirmation requirement
            if (tool_desc.requires_confirmation || gate.require_confirmation)
                && !cfg.auto_approve_safe_tools
            {
                let _ = cfg.event_tx.send(AgenticEvent::ConfirmationNeeded {
                    turn,
                    tool_name: tr.name.clone(),
                    arguments: tr.arguments.clone(),
                });
                record_step(
                    &cfg.task_handle,
                    turn,
                    "confirmation_needed",
                    &format!("{}: {}", tr.name, tr.arguments),
                )
                .await;

                let (tx, rx) = oneshot::channel();
                {
                    let mut task = cfg.task_handle.lock().await;
                    task.status = AgenticTaskStatus::WaitingForConfirmation;
                    task.confirmation_tx = Some(tx);
                }

                let approved = tokio::select! {
                    result = rx => result.unwrap_or(false),
                    _ = cfg.cancel_rx.recv() => {
                        let _ = cfg.event_tx.send(AgenticEvent::Done {
                            summary: "Task cancelled by mentor.".to_string(),
                            status: "cancelled".to_string(),
                            turns_used: turn,
                            tool_calls: total_tool_calls,
                        });
                        update_status(&cfg.task_handle, AgenticTaskStatus::Cancelled, turn).await;
                        persist_run_summary(
                            &cfg.agent_dir,
                            &cfg.task_id,
                            &cfg.goal,
                            &cfg.run_source,
                            "Task cancelled by mentor.",
                            "cancelled",
                            turn,
                            total_tool_calls,
                            cfg.started_at,
                        );
                        return;
                    }
                };

                {
                    let mut task = cfg.task_handle.lock().await;
                    task.status = AgenticTaskStatus::Running;
                }

                if !approved {
                    let denied_msg = format!("Tool {} was denied by mentor.", tr.name);
                    tool_results.push(format!("**{}**: Denied by mentor", tr.name));
                    let _ = cfg.event_tx.send(AgenticEvent::ToolResult {
                        turn,
                        tool_name: tr.name.clone(),
                        success: false,
                        output: denied_msg,
                    });
                    continue;
                }
            }

            // Emit tool call event
            let _ = cfg.event_tx.send(AgenticEvent::ToolCall {
                turn,
                tool_name: tr.name.clone(),
                arguments: tr.arguments.clone(),
            });

            // Build ToolParams
            let mut tool_params = orion_skills::skill::ToolParams::new();
            if let serde_json::Value::Object(map) = &tr.arguments {
                for (k, v) in map {
                    tool_params = tool_params.with(k, v.clone());
                }
            }

            // Execute
            match cfg
                .skill_executor
                .execute(&skill_id, &tr.name, tool_params)
                .await
            {
                Ok(output) => {
                    let output_str = if let Some(data) = &output.data {
                        serde_json::to_string_pretty(data).unwrap_or_else(|_| data.to_string())
                    } else if let Some(err) = &output.error {
                        format!("Error: {}", err)
                    } else {
                        "OK".to_string()
                    };

                    let _ = cfg.event_tx.send(AgenticEvent::ToolResult {
                        turn,
                        tool_name: tr.name.clone(),
                        success: output.success,
                        output: truncate_output(&output_str, 2000),
                    });
                    record_step(
                        &cfg.task_handle,
                        turn,
                        "tool_result",
                        &format!("{}: {}", tr.name, truncate_output(&output_str, 500)),
                    )
                    .await;

                    let status_label = if output.success { "Success" } else { "Error" };
                    tool_results.push(format!(
                        "**{}** ({}): {}",
                        tr.name,
                        status_label,
                        truncate_output(&output_str, 2000)
                    ));
                }
                Err(e) => {
                    let err_str = e.to_string();
                    let _ = cfg.event_tx.send(AgenticEvent::ToolResult {
                        turn,
                        tool_name: tr.name.clone(),
                        success: false,
                        output: err_str.clone(),
                    });
                    record_step(
                        &cfg.task_handle,
                        turn,
                        "tool_error",
                        &format!("{}: {}", tr.name, err_str),
                    )
                    .await;
                    tool_results.push(format!("**{}** (Error): {}", tr.name, err_str));
                }
            }
        }

        // Sync counters to task handle so API listings reflect live state.
        {
            let mut task = cfg.task_handle.lock().await;
            task.turn = turn;
            task.tool_calls = total_tool_calls;
        }

        // Feed tool results back to the LLM
        if !tool_results.is_empty() {
            let results_msg = format!("## Tool Results\n\n{}", tool_results.join("\n\n"));
            messages.push(Message::new("user", &results_msg));
        }
    }
}

// ---------------------------------------------------------------------------
// Helper functions
// ---------------------------------------------------------------------------

/// Trim conversation history to stay within a token budget while preserving
/// the system prompt and original goal message.
pub fn trim_context(messages: &mut Vec<Message>, max_tokens: usize) {
    let keep_prefix = 2; // system + goal
    if messages.len() <= keep_prefix {
        return;
    }

    let prefix_tokens: usize = messages[..keep_prefix]
        .iter()
        .map(|m| estimate_tokens(&m.content))
        .sum();

    let available = max_tokens.saturating_sub(prefix_tokens);

    // Count from the end: keep as many recent messages as fit in the budget.
    let mut tail_tokens = 0;
    let mut keep_from = messages.len();
    for i in (keep_prefix..messages.len()).rev() {
        let msg_tokens = estimate_tokens(&messages[i].content);
        if tail_tokens + msg_tokens > available {
            break;
        }
        tail_tokens += msg_tokens;
        keep_from = i;
    }

    if keep_from <= keep_prefix {
        return; // everything fits
    }

    let trim_count = keep_from - keep_prefix;
    let trimmed_tokens: usize = messages[keep_prefix..keep_from]
        .iter()
        .map(|m| estimate_tokens(&m.content))
        .sum();
    let trimmed_summary = format!(
        "[Context trimmed: {} earlier messages (~{} tokens) removed to save context window]",
        trim_count, trimmed_tokens
    );

    messages.drain(keep_prefix..keep_from);
    messages.insert(keep_prefix, Message::new("user", &trimmed_summary));
}

fn truncate_output(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        // Find a valid UTF-8 boundary at or before max_len to avoid panics
        let boundary = s
            .char_indices()
            .take_while(|(i, _)| *i <= max_len)
            .last()
            .map(|(i, _)| i)
            .unwrap_or(0);
        format!("{}... [truncated, {} total chars]", &s[..boundary], s.len())
    }
}

fn resolve_ego_credentials(config: &AppConfig, keyring: &ProviderKeyring) -> (Option<String>, Option<String>) {
    // Respect active provider preference if set.
    if let Some(ref pref) = config.active_provider_preference {
        let normalized = AppConfig::normalize_provider_name(pref);
        if let Some(key) = keyring.get_key_str(&normalized) {
            return (Some(normalized), Some(key.to_string()));
        }
    }
    let providers: Vec<String> = keyring
        .list_providers()
        .into_iter()
        .map(String::from)
        .collect();
    let preferred = ["anthropic", "openai"];
    let mut found_name: Option<String> = None;
    let mut found_key: Option<String> = None;
    for pref in &preferred {
        if let Some(key) = keyring.get_key_str(pref) {
            found_name = Some(pref.to_string());
            found_key = Some(key.to_string());
            break;
        }
    }
    if found_name.is_none() {
        for p in &providers {
            if let Some(key) = keyring.get_key_str(p) {
                found_name = Some(p.clone());
                found_key = Some(key.to_string());
                break;
            }
        }
    }
    (found_name, found_key)
}

fn resolve_superego_credentials(
    config: &AppConfig,
    keyring: &ProviderKeyring,
    ego_name: Option<&str>,
    ego_key: Option<&str>,
) -> (Option<String>, Option<String>) {
    if let Some(trinity) = &config.trinity {
        if let Some(provider) = trinity.superego_provider.as_deref() {
            let normalized = AppConfig::normalize_provider_name(provider);
            if let Some(key) = keyring.get_key_str(&normalized) {
                return (Some(normalized), Some(key.to_string()));
            }
        }
    }
    (ego_name.map(str::to_string), ego_key.map(str::to_string))
}

/// Build (or rebuild) the IdEgoRouter from the current on-disk keyring.
///
/// Called once before the loop for the pre-flight heartbeat and then at the top
/// of every turn so that credentials stored mid-run (e.g. via `store_provider_key`)
/// are picked up immediately.
async fn build_router_from_keyring(cfg: &AgenticLoopConfig) -> orion_router::IdEgoRouter {
    let (ego_name, ego_key, sup_creds) = {
        let keyring = cfg.provider_keyring.read().unwrap();
        let (ego_n, ego_k) = resolve_ego_credentials(&cfg.config, &keyring);
        let sup = if cfg.superego_l2_mode != orion_router::SuperegoL2Mode::Off {
            Some(resolve_superego_credentials(
                &cfg.config,
                &keyring,
                ego_n.as_deref(),
                ego_k.as_deref(),
            ))
        } else {
            None
        };
        (ego_n, ego_k, sup)
    };
    let routing_mode = match cfg.router_mode {
        AgenticRouterMode::Auto
        | AgenticRouterMode::ThinkHard
        | AgenticRouterMode::ThinkHarder => orion_core::RoutingMode::EgoPrimary,
    };
    let tier = match cfg.router_mode {
        AgenticRouterMode::Auto => ThinkingModelTier::Fast,
        AgenticRouterMode::ThinkHard => ThinkingModelTier::Standard,
        AgenticRouterMode::ThinkHarder => ThinkingModelTier::Pro,
    };
    let ego_model = ego_name
        .as_deref()
        .map(|provider| cfg.config.effective_tier_model(provider, tier));
    let mut router = orion_router::IdEgoRouter::with_provider_auto_detect(
        cfg.config.local_llm_base_url.clone(),
        ego_name.as_deref(),
        ego_key,
        ego_model,
        routing_mode,
    )
    .await;
    if let Some((sup_name, Some(key))) = sup_creds {
        let (provider, _) =
            orion_router::build_ego_provider(sup_name.as_deref(), Some(key), None);
        if let Some(provider) = provider {
            router = router.with_superego_config(provider, sup_name, cfg.superego_l2_mode);
        }
    }
    router
}

// ---------------------------------------------------------------------------
// Council helpers (ThinkHarder in agentic loop)
// ---------------------------------------------------------------------------

/// Resolve connected providers for council execution, priority-ordered.
///
/// Returns up to 3 `(provider_name, api_key)` tuples.  The active provider
/// preference is tried first, followed by the hardcoded preference order.
fn resolve_council_providers(
    keyring: &ProviderKeyring,
    preference: Option<&str>,
) -> Vec<(String, String)> {
    let mut result: Vec<(String, String)> = Vec::new();

    if let Some(pref) = preference {
        let normalized = AppConfig::normalize_provider_name(pref);
        if provider_supports_council(&normalized) {
            if let Some(key) = keyring.get_key_str(&normalized) {
                result.push((normalized, key.to_string()));
            }
        }
    }

    let preferred = ["anthropic", "openai", "google", "xai", "perplexity"];
    for pref in &preferred {
        if result.iter().any(|(n, _)| n == *pref) {
            continue;
        }
        if let Some(key) = keyring.get_key_str(pref) {
            result.push((pref.to_string(), key.to_string()));
            if result.len() >= 3 {
                break;
            }
        }
    }

    if result.len() < 3 {
        let providers: Vec<String> = keyring
            .list_providers()
            .into_iter()
            .map(String::from)
            .collect();
        for p in &providers {
            let normalized = AppConfig::normalize_provider_name(p);
            if !provider_supports_council(&normalized) {
                continue;
            }
            if result.iter().any(|(n, _)| *n == normalized) {
                continue;
            }
            if let Some(key) = keyring.get_key_str(p) {
                result.push((normalized, key.to_string()));
                if result.len() >= 3 {
                    break;
                }
            }
        }
    }

    result
}

fn provider_supports_council(provider: &str) -> bool {
    matches!(
        AppConfig::normalize_provider_name(provider).as_str(),
        "openai" | "anthropic" | "perplexity" | "xai" | "google"
    )
}

/// Run the Pro council to produce a deep-reasoning plan, then feed that plan
/// into a standard tool-capable model for execution.
///
/// Council cannot do structured tool calling, so the two-phase approach is:
///   1. Council (draft → critique → synthesis) produces a reasoning plan.
///   2. That plan is injected as context for a standard `route_with_tools`
///      call so the tool-capable model can translate the plan into concrete
///      tool invocations.
///
/// Degrades gracefully:
///   - <2 providers → skips council, uses standard `route_with_tools`.
///   - Council error → falls back to standard `route_with_tools`.
async fn council_then_execute(
    cfg: &AgenticLoopConfig,
    messages: &[Message],
    tool_defs: &[orion_capabilities::cognitive::ToolDefinition],
    router: &orion_router::IdEgoRouter,
) -> anyhow::Result<orion_capabilities::cognitive::CompletionResponse> {
    // Resolve council providers from current keyring snapshot.
    let council_providers = {
        let keyring = cfg.provider_keyring.read().unwrap();
        resolve_council_providers(&keyring, cfg.config.active_provider_preference.as_deref())
    };

    if council_providers.len() < 2 {
        tracing::info!(
            "council_then_execute: <2 providers ({}), using standard routing",
            council_providers.len()
        );
        return router
            .route_with_tools(messages.to_vec(), tool_defs.to_vec())
            .await;
    }

    let provider_configs: Vec<orion_router::council::ProviderConfig> = council_providers
        .into_iter()
        .map(|(name, key)| {
            let model = cfg
                .config
                .effective_tier_model(&name, ThinkingModelTier::Pro);
            orion_router::council::ProviderConfig {
                name,
                api_key: key,
                model,
            }
        })
        .collect();

    tracing::info!(
        providers = ?provider_configs.iter().map(|p| format!("{}:{}", p.name, p.model)).collect::<Vec<_>>(),
        "council_then_execute: running council for agentic turn"
    );

    // Open memory store for council persistence (Sqlite only; Postgres uses
    // block_on() which panics in async context).
    let council_memory: Option<orion_memory::MemoryStore> = match cfg.config.memory_backend {
        orion_core::MemoryBackend::Postgres => None,
        orion_core::MemoryBackend::Sqlite => {
            let config_for_store = cfg.config.clone();
            tokio::task::spawn_blocking(move || {
                orion_memory::MemoryStore::open_with_config(&config_for_store).ok()
            })
            .await
            .ok()
            .flatten()
        }
    };

    // Phase 1: Council reasoning.
    let council_result = orion_router::council::run_council(
        messages,
        &provider_configs,
        council_memory.as_ref(),
    )
    .await;

    let council_plan = match council_result {
        Ok(resp) => resp.content,
        Err(e) => {
            tracing::warn!(
                error = %e,
                "council_then_execute: council failed, falling back to standard routing"
            );
            return router
                .route_with_tools(messages.to_vec(), tool_defs.to_vec())
                .await;
        }
    };

    tracing::info!(
        plan_len = council_plan.len(),
        "council_then_execute: council produced plan, forwarding to tool-capable model"
    );

    // Phase 2: Feed the council plan into a tool-capable model.
    // We append the council plan as a "system-like" user message so the
    // tool-capable model treats it as authoritative reasoning to act on.
    let mut augmented_messages = messages.to_vec();
    augmented_messages.push(Message::new(
        "user",
        format!(
            "## Council Reasoning (Pro)\n\n\
             The following plan was produced by a multi-provider reasoning council. \
             Execute it using the available tools. Do not second-guess the plan; \
             translate it into concrete tool calls.\n\n\
             ---\n\n{}",
            council_plan
        ),
    ));

    router
        .route_with_tools(augmented_messages, tool_defs.to_vec())
        .await
}
/// ~4 characters per token is a standard approximation for English text,
/// plus a fixed overhead per message for role/formatting.
fn estimate_tokens(text: &str) -> usize {
    (text.len() / 4) + 4
}

/// Return a token budget for the agentic context window based on router mode.
fn context_token_budget(mode: AgenticRouterMode) -> usize {
    match mode {
        AgenticRouterMode::Auto => 16_000,
        AgenticRouterMode::ThinkHard => 24_000,
        AgenticRouterMode::ThinkHarder => 32_000,
    }
}

fn build_goal_kickoff_prompt(goal: &str, mode: AgenticRouterMode, run_source: &str) -> String {
    let source_context = if run_source.starts_with("scheduled:") {
        "Run source: scheduled orchestration job.\n\
         Disturb the mentor only for high-significance findings."
    } else {
        "Run source: mentor-initiated task."
    };

    match mode {
        AgenticRouterMode::Auto => format!(
            "## Your Goal\n\n{}\n\n{}\n\nBegin by assessing your environment and planning your approach.",
            goal, source_context
        ),
        AgenticRouterMode::ThinkHard => format!(
            "## Your Goal\n\n{}\n\nReasoning profile: THINK HARD.\n\
             Break the task into phases, compare at least two viable approaches when choices matter, \
             and verify key outputs before moving on.\n\n\
             {}\n\n\
             Begin by assessing your environment and drafting your plan.",
            goal, source_context
        ),
        AgenticRouterMode::ThinkHarder => format!(
            "## Your Goal\n\n{}\n\nReasoning profile: THINK HARDER.\n\
             Use deliberate multi-step decomposition, stress-test assumptions, and run secondary checks \
             before claiming completion. Favor correctness and evidence over speed.\n\n\
             {}\n\n\
             Begin by assessing your environment, then produce a robust execution plan.",
            goal, source_context
        ),
    }
}

async fn update_status(task: &Arc<Mutex<AgenticTask>>, status: AgenticTaskStatus, turn: u32) {
    let mut t = task.lock().await;
    t.status = status;
    t.turn = turn;
    if t.is_terminal() && t.completed_at.is_none() {
        t.completed_at = Some(std::time::Instant::now());
    }
}

async fn record_step(task: &Arc<Mutex<AgenticTask>>, turn: u32, step_type: &str, content: &str) {
    let mut t = task.lock().await;
    t.steps.push(AgenticStep {
        turn,
        step_type: step_type.to_string(),
        content: content.to_string(),
        timestamp: chrono::Utc::now(),
    });
}

/// Persist agentic run to disk for audit trail.
#[allow(clippy::too_many_arguments)]
fn persist_run_summary(
    agent_dir: &Path,
    task_id: &str,
    goal: &str,
    run_source: &str,
    summary: &str,
    status: &str,
    turns: u32,
    tool_calls: u32,
    started_at: chrono::DateTime<chrono::Utc>,
) {
    let runs_dir = agent_dir.join("agentic_runs");
    let _ = std::fs::create_dir_all(&runs_dir);

    let run_data = serde_json::json!({
        "task_id": task_id,
        "goal": goal,
        "source": run_source,
        "summary": summary,
        "status": status,
        "turns": turns,
        "tool_calls": tool_calls,
        "started_at": started_at.to_rfc3339(),
        "completed_at": chrono::Utc::now().to_rfc3339(),
    });

    let _ = std::fs::write(
        runs_dir.join(format!("{}.json", task_id)),
        serde_json::to_string_pretty(&run_data).unwrap_or_default(),
    );

    // Append a single assistant entry to operational_chat.json so the task
    // result persists across page reloads. Avoid adding a fake "user" message
    // which would appear as if the mentor said something.
    let chat_path = agent_dir.join("operational_chat.json");
    let mut history: Vec<serde_json::Value> = if chat_path.exists() {
        std::fs::read_to_string(&chat_path)
            .ok()
            .and_then(|c| serde_json::from_str(&c).ok())
            .unwrap_or_default()
    } else {
        Vec::new()
    };

    history.push(serde_json::json!({
        "role": "assistant",
        "content": format!("[Agentic Task Complete] Goal: {} — {} ({} turns, {} tool calls)", goal, summary, turns, tool_calls),
    }));

    let _ = std::fs::write(
        &chat_path,
        serde_json::to_string_pretty(&history).unwrap_or_default(),
    );
}

/// Connect to an MCP server, initialize it, register it as an AgentBuilt skill,
/// and persist the definition to the agent's config.
async fn register_mcp_skill_impl(
    server_id: &str,
    server_name: &str,
    base_url: &str,
    config: &AppConfig,
    registry: &SkillRegistry,
    agent_dir: &Path,
) -> Result<Vec<String>, String> {
    // Validate URL against trust policy.
    config.mcp_trust_policy.validate_url(base_url)?;

    // Create and initialize the MCP runtime.
    let mut runtime = McpSkillRuntime::new(server_id, server_name, base_url);
    let skill_config = orion_skills::skill::SkillConfig {
        values: Default::default(),
        secrets: Default::default(),
        limits: orion_skills::ResourceLimits::default(),
        permissions: vec![],
        event_sender: None,
    };
    runtime
        .initialize(skill_config)
        .await
        .map_err(|e| format!("MCP initialize failed: {}", e))?;

    // Collect discovered tool names before moving into Arc.
    let tool_names: Vec<String> = Skill::tools(&runtime)
        .iter()
        .map(|t| t.name.clone())
        .collect();

    if tool_names.is_empty() {
        return Err(format!("MCP server at {} exposed no tools", base_url));
    }

    // Register into live registry.
    let skill_id = SkillId(server_id.to_string());
    registry
        .register_with_tier(
            skill_id,
            std::sync::Arc::new(runtime),
            TrustTier::AgentBuilt,
        )
        .map_err(|e| format!("registry insert failed: {}", e))?;

    // Persist to agent config so the skill reloads on restart.
    let config_path = agent_dir.join("config.json");
    if let Ok(mut persisted_config) = AppConfig::load(&config_path) {
        // Avoid duplicate entries.
        persisted_config.mcp_servers.retain(|s| s.id != server_id);
        persisted_config.mcp_servers.push(McpServerDefinition {
            id: server_id.to_string(),
            name: server_name.to_string(),
            transport: "http".to_string(),
            command_or_url: base_url.to_string(),
            env: Default::default(),
        });
        if let Err(e) = persisted_config.save(&config_path) {
            tracing::warn!(error = %e, "failed to persist MCP server to config");
        }
    }

    tracing::info!(
        server_id = server_id,
        server_name = server_name,
        base_url = base_url,
        tools = ?tool_names,
        "registered MCP skill (AgentBuilt)"
    );

    Ok(tool_names)
}

// ---------------------------------------------------------------------------
// manage_job synthetic tool
// ---------------------------------------------------------------------------

fn proxy_allowlist_path() -> PathBuf {
    std::env::var("ORION_PROXY_ALLOWLIST_PATH")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("docker/proxy/external/allowlist_domains.txt"))
}

fn proxy_log_dir() -> PathBuf {
    std::env::var("ORION_PROXY_LOG_DIR")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(".orion/proxy"))
}

fn read_log_tail(path: &Path, lines: usize) -> String {
    if lines == 0 {
        return String::new();
    }
    let content = match std::fs::read_to_string(path) {
        Ok(content) => content,
        Err(_) => return String::new(),
    };
    let out = content
        .lines()
        .rev()
        .take(lines)
        .collect::<Vec<&str>>()
        .into_iter()
        .rev()
        .collect::<Vec<&str>>();
    out.join("\n")
}

fn handle_manage_proxy(args: &serde_json::Value) -> String {
    let action = args
        .get("action")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    match action.as_str() {
        "status" => {
            let mode = std::env::var("PROXY_MODE").unwrap_or_else(|_| "allow_all".to_string());
            let allow_host = std::env::var("PROXY_ALLOW_HOST_DOCKER_INTERNAL")
                .map(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
                .unwrap_or(true);
            let allowlist_path = proxy_allowlist_path();
            let log_dir = proxy_log_dir();
            let internal_log = log_dir.join("internal").join("access.log");
            let external_log = log_dir.join("external").join("access.log");
            format!(
                "Proxy status: mode={}, allow_host_docker_internal={}, allowlist={}, internal_log_exists={}, external_log_exists={}",
                mode,
                allow_host,
                allowlist_path.display(),
                internal_log.exists(),
                external_log.exists()
            )
        }
        "get_allowlist" => {
            let path = proxy_allowlist_path();
            let content = std::fs::read_to_string(&path).unwrap_or_default();
            if content.trim().is_empty() {
                format!("Allowlist at {} is empty.", path.display())
            } else {
                format!("Allowlist at {}:\n{}", path.display(), content)
            }
        }
        "update_allowlist" => {
            let mentor_approved = args
                .get("mentor_approved")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            if !mentor_approved {
                return "manage_proxy failed: mentor_approved=true is required for update_allowlist."
                    .to_string();
            }
            let domains = args
                .get("domains")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();
            let normalized = domains
                .iter()
                .filter_map(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
                .collect::<Vec<String>>();
            if normalized.is_empty() {
                return "manage_proxy failed: domains array is required for update_allowlist."
                    .to_string();
            }
            let path = proxy_allowlist_path();
            if let Some(parent) = path.parent() {
                if let Err(e) = std::fs::create_dir_all(parent) {
                    return format!("manage_proxy failed: create directory failed: {}", e);
                }
            }
            let mut body = String::from(
                "# Managed by manage_proxy tool. One domain per line.\n# Leading dot matches subdomains.\n",
            );
            for d in &normalized {
                body.push_str(d);
                body.push('\n');
            }
            if let Err(e) = std::fs::write(&path, body) {
                return format!("manage_proxy failed: write allowlist failed: {}", e);
            }
            let reload_note = std::process::Command::new("sh")
                .arg("-lc")
                .arg("docker compose -f docker/docker-compose.yml exec -T proxy_external squid -k reconfigure")
                .output()
                .ok()
                .map(|out| if out.status.success() { "reloaded proxy_external".to_string() } else { "allowlist updated but reload command failed".to_string() })
                .unwrap_or_else(|| "allowlist updated (reload command unavailable)".to_string());
            format!(
                "Proxy allowlist updated at {} with {} domain(s); {}.",
                path.display(),
                normalized.len(),
                reload_note
            )
        }
        "get_logs" => {
            let lines = args
                .get("lines")
                .and_then(|v| v.as_u64())
                .map(|v| (v as usize).clamp(1, 500))
                .unwrap_or(80);
            let log_dir = proxy_log_dir();
            let internal_log = log_dir.join("internal").join("access.log");
            let external_log = log_dir.join("external").join("access.log");
            let internal = read_log_tail(&internal_log, lines);
            let external = read_log_tail(&external_log, lines);
            format!(
                "Proxy logs (last {} lines)\n\n[internal]\n{}\n\n[external]\n{}",
                lines, internal, external
            )
        }
        other => format!(
            "manage_proxy failed: unknown action '{}'. Use status, get_allowlist, update_allowlist, or get_logs.",
            other
        ),
    }
}

fn handle_manage_job(agent_dir: &Path, args: &serde_json::Value) -> String {
    use crate::orchestration::{
        create_job, delete_job, list_jobs, save_jobs, set_job_enabled,
        CreateOrchestrationJobRequest, OrchestrationJobMode, SignificancePolicy,
    };

    let action = args
        .get("action")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    match action.as_str() {
        "create" => {
            let name = args
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let cron = args
                .get("cron")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let mode_str = args
                .get("mode")
                .and_then(|v| v.as_str())
                .unwrap_or("id_check");
            let mode = match mode_str {
                "agentic_run" => OrchestrationJobMode::AgenticRun,
                _ => OrchestrationJobMode::IdCheck,
            };
            let goal_template = args
                .get("goal_template")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let enabled = args
                .get("enabled")
                .and_then(|v| v.as_bool())
                .unwrap_or(true);
            let escalate_medium = args
                .get("escalate_medium")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let flag_high = args
                .get("flag_high_to_mentor")
                .and_then(|v| v.as_bool())
                .unwrap_or(true);

            if name.is_empty() || cron.is_empty() || goal_template.is_empty() {
                return "manage_job failed: name, cron, and goal_template are required for create."
                    .to_string();
            }

            let request = CreateOrchestrationJobRequest {
                name: name.clone(),
                cron,
                mode,
                goal_template,
                enabled,
                significance_policy: SignificancePolicy {
                    escalate_medium,
                    flag_high_to_mentor: flag_high,
                    ..Default::default()
                },
            };

            let mut jobs = match list_jobs(agent_dir) {
                Ok(j) => j,
                Err(e) => return format!("manage_job failed: {}", e),
            };

            match create_job(&mut jobs, request, chrono::Utc::now()) {
                Ok(job) => {
                    if let Err(e) = save_jobs(agent_dir, &jobs) {
                        return format!("manage_job failed: job created but save failed: {}", e);
                    }
                    format!(
                        "Job '{}' created (id: {}, cron: {}, mode: {:?}, enabled: {}).",
                        job.name, job.job_id, job.cron, job.mode, job.enabled
                    )
                }
                Err(e) => format!("manage_job failed: {}", e),
            }
        }

        "list" => match list_jobs(agent_dir) {
            Ok(jobs) => {
                if jobs.is_empty() {
                    "No orchestration jobs configured.".to_string()
                } else {
                    let lines: Vec<String> = jobs
                        .iter()
                        .map(|j| {
                            format!(
                                "- {} (id: {}, cron: {}, mode: {:?}, enabled: {}, last_status: {})",
                                j.name,
                                j.job_id,
                                j.cron,
                                j.mode,
                                j.enabled,
                                j.last_status.as_deref().unwrap_or("never run"),
                            )
                        })
                        .collect();
                    format!("Orchestration jobs ({}):\n{}", jobs.len(), lines.join("\n"))
                }
            }
            Err(e) => format!("manage_job failed: {}", e),
        },

        "enable" => {
            let job_id = args.get("job_id").and_then(|v| v.as_str()).unwrap_or("");
            let enabled = args
                .get("enabled")
                .and_then(|v| v.as_bool())
                .unwrap_or(true);

            if job_id.is_empty() {
                return "manage_job failed: job_id is required for enable.".to_string();
            }

            let mut jobs = match list_jobs(agent_dir) {
                Ok(j) => j,
                Err(e) => return format!("manage_job failed: {}", e),
            };

            match set_job_enabled(&mut jobs, job_id, enabled, chrono::Utc::now()) {
                Ok(job) => {
                    if let Err(e) = save_jobs(agent_dir, &jobs) {
                        return format!("manage_job failed: save failed: {}", e);
                    }
                    format!(
                        "Job '{}' {} (id: {}).",
                        job.name,
                        if enabled { "enabled" } else { "disabled" },
                        job.job_id
                    )
                }
                Err(e) => format!("manage_job failed: {}", e),
            }
        }

        "delete" => {
            let job_id = args.get("job_id").and_then(|v| v.as_str()).unwrap_or("");

            if job_id.is_empty() {
                return "manage_job failed: job_id is required for delete.".to_string();
            }

            let mut jobs = match list_jobs(agent_dir) {
                Ok(j) => j,
                Err(e) => return format!("manage_job failed: {}", e),
            };

            if delete_job(&mut jobs, job_id) {
                if let Err(e) = save_jobs(agent_dir, &jobs) {
                    return format!("manage_job failed: delete succeeded but save failed: {}", e);
                }
                format!("Job {} deleted.", job_id)
            } else {
                format!("manage_job failed: no job with id '{}'.", job_id)
            }
        }

        other => format!(
            "manage_job failed: unknown action '{}'. Use create, list, enable, or delete.",
            other
        ),
    }
}

// ---------------------------------------------------------------------------
// write_review synthetic tool
// ---------------------------------------------------------------------------

fn handle_write_review(agent_dir: &Path, args: &serde_json::Value) -> String {
    let content = args.get("content").and_then(|v| v.as_str()).unwrap_or("");

    if content.is_empty() {
        return "write_review failed: content is required.".to_string();
    }

    let reviews_path = agent_dir.join("reviews.md");
    let timestamp = chrono::Utc::now().format("%Y-%m-%d %H:%M UTC");

    let entry = if reviews_path.exists() {
        format!("\n\n---\n_Recorded {}_\n\n{}", timestamp, content)
    } else {
        format!("# Self-Reviews\n\n_Recorded {}_\n\n{}", timestamp, content)
    };

    use std::io::Write;
    match std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&reviews_path)
    {
        Ok(mut f) => match f.write_all(entry.as_bytes()) {
            Ok(_) => format!(
                "Review entry appended to {} ({} chars).",
                reviews_path.display(),
                content.len()
            ),
            Err(e) => format!("write_review failed: {}", e),
        },
        Err(e) => format!("write_review failed: {}", e),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_trim_context_no_op_when_small() {
        let mut msgs = vec![
            Message::new("system", "sys"),
            Message::new("user", "goal"),
            Message::new("assistant", "a1"),
            Message::new("user", "u1"),
        ];
        // Large budget — everything fits
        trim_context(&mut msgs, 16_000);
        assert_eq!(msgs.len(), 4);
    }

    #[test]
    fn test_trim_context_removes_by_token_budget() {
        let mut msgs = vec![Message::new("system", "sys"), Message::new("user", "goal")];
        // Add 50 message pairs with ~100 tokens each (400 chars / 4 + 4 overhead)
        for _ in 0..50 {
            msgs.push(Message::new("assistant", "a".repeat(400)));
            msgs.push(Message::new("user", "u".repeat(400)));
        }
        assert_eq!(msgs.len(), 102);

        // Small budget — should trim most messages
        trim_context(&mut msgs, 2000);
        assert!(msgs.len() < 102);
        assert_eq!(msgs[0].role, "system");
        assert_eq!(msgs[1].content, "goal");
        assert!(msgs[2].content.contains("Context trimmed"));
        assert!(msgs[2].content.contains("tokens"));
    }

    #[test]
    fn test_estimate_tokens() {
        assert_eq!(estimate_tokens(""), 4); // just overhead
        assert_eq!(estimate_tokens("hello world"), 6); // 11/4 + 4 = 6
        assert_eq!(estimate_tokens(&"x".repeat(400)), 104); // 400/4 + 4
    }

    #[test]
    fn test_truncate_output() {
        let short = "hello";
        assert_eq!(truncate_output(short, 100), "hello");

        let long = "a".repeat(200);
        let result = truncate_output(&long, 50);
        assert!(result.contains("truncated"));
        assert!(result.len() < 200);
    }

    #[test]
    fn test_find_skill_for_tool_returns_none_on_empty_registry() {
        let registry = SkillRegistry::new();
        assert!(find_skill_for_tool(&registry, "nonexistent").is_none());
    }
}
