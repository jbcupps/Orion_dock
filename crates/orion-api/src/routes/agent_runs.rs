use axum::{
    extract::{Path, Query, State},
    response::sse::{Event, KeepAlive, Sse},
    Json,
};
use serde::{Deserialize, Serialize};

use crate::agentic::{
    AgenticEvent, AgenticRunRequest, AgenticRunResponse, AgenticStatusResponse, AgenticTaskStatus,
    CancelRequest, ConfirmationResponseRequest, MentorResponseRequest,
};
use crate::{agent_dir, launch_agentic_task_internal, ApiError, AppState};

#[derive(Deserialize)]
pub(crate) struct AgenticStreamQuery {
    pub(crate) task: String,
}

/// Info about a single agentic run (active or historical).
#[derive(Debug, Serialize)]
pub(crate) struct AgenticRunInfo {
    pub(crate) task_id: String,
    pub(crate) goal: String,
    pub(crate) status: String,
    pub(crate) turns: u32,
    pub(crate) tool_calls: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) summary: Option<String>,
    pub(crate) started_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) completed_at: Option<String>,
}

/// GET /api/agents/{id}/agent/runs — list agentic runs (active + historical).
pub(crate) async fn api_list_agentic_runs(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Vec<AgenticRunInfo>>, ApiError> {
    let dir = agent_dir(&id).ok_or_else(|| ApiError::NotFound("Agent not found".to_string()))?;

    let mut runs: Vec<AgenticRunInfo> = Vec::new();

    // Collect in-memory active tasks for this agent
    {
        let tasks = state.agentic_tasks.lock().await;
        for task_arc in tasks.values() {
            let task = task_arc.lock().await;
            if task.agent_id == id {
                let status_str = match task.status {
                    AgenticTaskStatus::Running => "running",
                    AgenticTaskStatus::WaitingForMentor => "running",
                    AgenticTaskStatus::WaitingForConfirmation => "running",
                    AgenticTaskStatus::Completed => "completed",
                    AgenticTaskStatus::Failed => "failed",
                    AgenticTaskStatus::Cancelled => "cancelled",
                };
                runs.push(AgenticRunInfo {
                    task_id: task.id.clone(),
                    goal: task.goal.clone(),
                    status: status_str.to_string(),
                    turns: task.turn,
                    tool_calls: task.tool_calls,
                    source: None,
                    summary: None,
                    started_at: task.started_at.to_rfc3339(),
                    completed_at: None,
                });
            }
        }
    }

    // Collect historical runs from disk
    let runs_dir = dir.join("agentic_runs");
    if runs_dir.is_dir() {
        if let Ok(entries) = std::fs::read_dir(&runs_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) != Some("json") {
                    continue;
                }
                if let Ok(content) = std::fs::read_to_string(&path) {
                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&content) {
                        let task_id = v
                            .get("task_id")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        // Skip if already present from in-memory tasks
                        if runs.iter().any(|r| r.task_id == task_id) {
                            continue;
                        }
                        runs.push(AgenticRunInfo {
                            task_id,
                            goal: v
                                .get("goal")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string(),
                            status: v
                                .get("status")
                                .and_then(|v| v.as_str())
                                .unwrap_or("completed")
                                .to_string(),
                            turns: v.get("turns").and_then(|v| v.as_u64()).unwrap_or(0) as u32,
                            tool_calls: v.get("tool_calls").and_then(|v| v.as_u64()).unwrap_or(0)
                                as u32,
                            source: v.get("source").and_then(|v| v.as_str()).map(String::from),
                            summary: v.get("summary").and_then(|v| v.as_str()).map(String::from),
                            started_at: v
                                .get("started_at")
                                .and_then(|v| v.as_str())
                                .unwrap_or_default()
                                .to_string(),
                            completed_at: v
                                .get("completed_at")
                                .and_then(|v| v.as_str())
                                .map(String::from),
                        });
                    }
                }
            }
        }
    }

    // Sort reverse chronological by started_at
    runs.sort_by(|a, b| b.started_at.cmp(&a.started_at));

    Ok(Json(runs))
}

/// POST /api/agents/{id}/agent/run — start an agentic task.
pub(crate) async fn api_agentic_run(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<AgenticRunRequest>,
) -> Result<Json<AgenticRunResponse>, ApiError> {
    let response = launch_agentic_task_internal(&state, &id, body, "manual".to_string())
        .await
        .map_err(|e| {
            if e.contains("not found") || e.contains("Agent not found") {
                ApiError::NotFound(e)
            } else if e.contains("running agentic task") {
                ApiError::Conflict(e)
            } else if e.contains("goal is required") || e.contains("birth must be complete") {
                ApiError::BadRequest(e)
            } else {
                ApiError::Internal(e)
            }
        })?;

    tracing::info!(agent = %id, task = %response.task_id, "Started agentic task");
    Ok(Json(response))
}

/// GET /api/agents/{id}/agent/stream?task=<id> — SSE event stream for an agentic task.
pub(crate) async fn api_agentic_stream(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<AgenticStreamQuery>,
) -> Result<Sse<impl futures_core::Stream<Item = Result<Event, std::convert::Infallible>>>, ApiError>
{
    let tasks = state.agentic_tasks.lock().await;
    let task_arc = tasks
        .get(&query.task)
        .ok_or_else(|| ApiError::NotFound("Task not found".to_string()))?;

    let rx = {
        let task = task_arc.lock().await;
        if task.agent_id != id {
            return Err(ApiError::Forbidden(
                "Task does not belong to this agent".to_string(),
            ));
        }
        task.event_tx.subscribe()
    };

    let stream = async_stream::stream! {
        let mut rx = rx;
        loop {
            match rx.recv().await {
                Ok(event) => {
                    let json = serde_json::to_string(&event).unwrap_or_default();
                    let event_name = match &event {
                        AgenticEvent::Thinking { .. } => "thinking",
                        AgenticEvent::ToolCall { .. } => "tool_call",
                        AgenticEvent::ToolResult { .. } => "tool_result",
                        AgenticEvent::MentorNeeded { .. } => "mentor_needed",
                        AgenticEvent::ConfirmationNeeded { .. } => "confirmation_needed",
                        AgenticEvent::Done { .. } => "done",
                        AgenticEvent::Warning { .. } => "warning",
                        AgenticEvent::Error { .. } => "error",
                    };
                    yield Ok(Event::default().event(event_name).data(json));
                    if matches!(event, AgenticEvent::Done { .. } | AgenticEvent::Error { .. }) {
                        break;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    let msg = format!("{{\"skipped\":{}}}", n);
                    yield Ok(Event::default().event("lagged").data(msg));
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    break;
                }
            }
        }
    };

    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}

/// POST /api/agents/{id}/agent/respond — send mentor response to paused agentic task.
pub(crate) async fn api_agentic_respond(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<MentorResponseRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let tasks = state.agentic_tasks.lock().await;
    let task_arc = tasks
        .get(&body.task_id)
        .ok_or_else(|| ApiError::NotFound("Task not found".to_string()))?;

    let mut task = task_arc.lock().await;
    if task.agent_id != id {
        return Err(ApiError::Unauthorized(
            "Task does not belong to this agent".to_string(),
        ));
    }
    if task.status != AgenticTaskStatus::WaitingForMentor {
        return Err(ApiError::BadRequest(
            "Task is not waiting for mentor response".to_string(),
        ));
    }

    if let Some(tx) = task.mentor_response_tx.take() {
        let _ = tx.send(body.response);
    }

    Ok(Json(serde_json::json!({ "ok": true })))
}

/// POST /api/agents/{id}/agent/confirm — approve or deny a tool confirmation request.
pub(crate) async fn api_agentic_confirm(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<ConfirmationResponseRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let tasks = state.agentic_tasks.lock().await;
    let task_arc = tasks
        .get(&body.task_id)
        .ok_or_else(|| ApiError::NotFound("Task not found".to_string()))?;

    let mut task = task_arc.lock().await;
    if task.agent_id != id {
        return Err(ApiError::Unauthorized(
            "Task does not belong to this agent".to_string(),
        ));
    }
    if task.status != AgenticTaskStatus::WaitingForConfirmation {
        return Err(ApiError::BadRequest(
            "Task is not waiting for confirmation".to_string(),
        ));
    }

    if let Some(tx) = task.confirmation_tx.take() {
        let _ = tx.send(body.approved);
    }

    Ok(Json(serde_json::json!({ "ok": true })))
}

/// POST /api/agents/{id}/agent/cancel — cancel a running agentic task.
pub(crate) async fn api_agentic_cancel(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<CancelRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let tasks = state.agentic_tasks.lock().await;
    let task_arc = tasks
        .get(&body.task_id)
        .ok_or_else(|| ApiError::NotFound("Task not found".to_string()))?;

    let task = task_arc.lock().await;
    if task.agent_id != id {
        return Err(ApiError::Unauthorized(
            "Task does not belong to this agent".to_string(),
        ));
    }
    if matches!(
        task.status,
        AgenticTaskStatus::Completed | AgenticTaskStatus::Failed | AgenticTaskStatus::Cancelled
    ) {
        return Err(ApiError::BadRequest("Task is already finished".to_string()));
    }

    let _ = task.cancel_tx.try_send(());

    Ok(Json(serde_json::json!({ "ok": true })))
}

/// GET /api/agents/{id}/agent/status?task=<id> — check task status.
pub(crate) async fn api_agentic_status(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<AgenticStreamQuery>,
) -> Result<Json<AgenticStatusResponse>, ApiError> {
    let tasks = state.agentic_tasks.lock().await;
    let task_arc = tasks
        .get(&query.task)
        .ok_or_else(|| ApiError::NotFound("Task not found".to_string()))?;

    let task = task_arc.lock().await;
    if task.agent_id != id {
        return Err(ApiError::Unauthorized(
            "Task does not belong to this agent".to_string(),
        ));
    }
    Ok(Json(AgenticStatusResponse {
        task_id: task.id.clone(),
        goal: task.goal.clone(),
        status: task.status,
        turn: task.turn,
        steps: task.steps.clone(),
    }))
}
