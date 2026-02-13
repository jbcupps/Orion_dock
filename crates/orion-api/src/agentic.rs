//! Autonomous agentic loop.
//!
//! Runs a multi-turn LLM loop where the agent receives a high-level goal,
//! autonomously researches/plans/executes/observes/iterates, and only
//! consults the mentor at genuine decision points via `ask_mentor`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, mpsc, oneshot, Mutex};

use orion_birth::parse_tool_requests;
use orion_capabilities::cognitive::Message;
use orion_core::system_prompt::{build_agentic_system_prompt, SkillToolEntry};
use orion_core::{AppConfig, SecretsVault};
use orion_skills::manifest::SkillId;
use orion_skills::skill::ToolDescriptor;
use orion_skills::{SkillExecutor, SkillRegistry};

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AgenticRouterMode {
    #[default]
    Auto,
    ThinkHard,
    ThinkHarder,
}

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
#[allow(dead_code)]
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
    pub cancel_tx: mpsc::Sender<()>,
    pub started_at: chrono::DateTime<chrono::Utc>,
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

/// Configuration for the agentic loop.
pub struct AgenticLoopConfig {
    pub task_id: String,
    pub goal: String,
    pub max_turns: u32,
    pub auto_approve_safe_tools: bool,
    pub router_mode: AgenticRouterMode,
    pub agent_dir: PathBuf,
    pub config: AppConfig,
    pub skill_registry: Arc<SkillRegistry>,
    pub skill_executor: Arc<SkillExecutor>,
    pub skill_tool_entries: Vec<SkillToolEntry>,
    pub stored_providers: Vec<String>,
    pub event_tx: broadcast::Sender<AgenticEvent>,
    pub cancel_rx: mpsc::Receiver<()>,
    pub task_handle: Arc<Mutex<AgenticTask>>,
    pub started_at: chrono::DateTime<chrono::Utc>,
    pub run_source: String,
}

/// Run the autonomous agentic loop. Call from `tokio::spawn`.
pub async fn run_agentic_loop(mut cfg: AgenticLoopConfig) {
    let system_prompt = build_agentic_system_prompt(
        &cfg.config.docs_dir,
        &cfg.config.agent_name,
        &cfg.skill_tool_entries,
        &cfg.stored_providers,
    );

    let mut messages: Vec<Message> = vec![
        Message::new("system", &system_prompt),
        Message::new(
            "user",
            build_goal_kickoff_prompt(&cfg.goal, cfg.router_mode, &cfg.run_source),
        ),
    ];

    // Build router
    let (ego_name, ego_key) = resolve_ego_credentials(&cfg.config);
    let routing_mode = match cfg.router_mode {
        AgenticRouterMode::Auto => cfg.config.routing_mode,
        AgenticRouterMode::ThinkHard | AgenticRouterMode::ThinkHarder => {
            // Thinking presets prioritize Ego for deeper reasoning while preserving Id fallback.
            orion_core::RoutingMode::EgoPrimary
        }
    };
    let router = orion_router::IdEgoRouter::with_provider_auto_detect(
        cfg.config.local_llm_base_url.clone(),
        ego_name.as_deref(),
        ego_key,
        routing_mode,
    )
    .await;

    let mut turn: u32 = 0;
    let mut total_tool_calls: u32 = 0;

    loop {
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
        trim_context(&mut messages, context_window_pairs(cfg.router_mode));

        // LLM call
        let response = match router.route(messages.clone()).await {
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
        };

        let (clean_content, tool_requests) = parse_tool_requests(&response.content);

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
        for tr in &tool_requests {
            // Skip synthetic tools already handled above
            if tr.name == "task_complete" || tr.name == "ask_mentor" {
                continue;
            }

            total_tool_calls += 1;

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

            // Check confirmation requirement
            if tool_desc.requires_confirmation && !cfg.auto_approve_safe_tools {
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

/// Trim conversation history to keep it within bounds while preserving
/// the system prompt and original goal message.
pub fn trim_context(messages: &mut Vec<Message>, max_pairs: usize) {
    // Keep: system (index 0), goal (index 1), then last N*2 messages
    let keep_prefix = 2; // system + goal
    let max_body = max_pairs * 2;

    if messages.len() <= keep_prefix + max_body {
        return;
    }

    let trim_count = messages.len() - keep_prefix - max_body;
    let trimmed_summary = format!(
        "[Context trimmed: {} earlier messages removed to save context window]",
        trim_count
    );

    // Remove the middle section
    messages.drain(keep_prefix..keep_prefix + trim_count);

    // Insert a summary message at position 2
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

fn resolve_ego_credentials(config: &AppConfig) -> (Option<String>, Option<String>) {
    let vault = SecretsVault::load(config.data_dir.clone())
        .unwrap_or_else(|_| SecretsVault::new(config.data_dir.clone()));
    let providers = vault.list_providers();
    let preferred = ["anthropic", "openai"];
    let mut found_name: Option<String> = None;
    let mut found_key: Option<String> = None;
    for pref in &preferred {
        if let Some(key) = vault.get_secret(pref) {
            found_name = Some(pref.to_string());
            found_key = Some(key.to_string());
            break;
        }
    }
    if found_name.is_none() {
        for p in &providers {
            if *p != "tavily" {
                if let Some(key) = vault.get_secret(p) {
                    found_name = Some(p.to_string());
                    found_key = Some(key.to_string());
                    break;
                }
            }
        }
    }
    (found_name, found_key)
}

fn context_window_pairs(mode: AgenticRouterMode) -> usize {
    match mode {
        AgenticRouterMode::Auto => 20,
        AgenticRouterMode::ThinkHard => 28,
        AgenticRouterMode::ThinkHarder => 36,
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

    // Append summary entry to operational_chat.json
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
        "role": "user",
        "content": format!("[Agentic Task]: {}", goal),
    }));
    history.push(serde_json::json!({
        "role": "assistant",
        "content": format!("[Task Complete]: {} ({} turns, {} tool calls)", summary, turns, tool_calls),
    }));

    let _ = std::fs::write(
        &chat_path,
        serde_json::to_string_pretty(&history).unwrap_or_default(),
    );
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
        trim_context(&mut msgs, 20);
        assert_eq!(msgs.len(), 4);
    }

    #[test]
    fn test_trim_context_removes_middle() {
        let mut msgs = vec![Message::new("system", "sys"), Message::new("user", "goal")];
        // Add 50 message pairs (100 messages)
        for i in 0..50 {
            msgs.push(Message::new("assistant", format!("a{}", i)));
            msgs.push(Message::new("user", format!("u{}", i)));
        }
        assert_eq!(msgs.len(), 102);

        trim_context(&mut msgs, 8);
        // Should be: system + goal + trim_notice + last 16 messages = 19
        assert_eq!(msgs.len(), 19);
        assert_eq!(msgs[0].role, "system");
        assert_eq!(msgs[1].role, "user");
        assert!(msgs[2].content.contains("Context trimmed"));
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
