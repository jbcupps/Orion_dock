//! Execution state tracking for the Governor's working memory.
//!
//! Tracks attempts, constraints discovered, strategy, and progress across
//! execution iterations. Injected into the executor's system prompt so the LLM
//! knows what has been tried and what constraints are in effect.

use std::collections::HashSet;
use std::time::Instant;

use orion_skills::structured_failure::{Constraint, StructuredFailure};

use crate::goal_frame::{GoalFrame, Risk};

/// The Governor's working memory across execution iterations.
#[derive(Debug, Clone)]
pub struct ExecutionState {
    pub goal: GoalFrame,
    pub iteration: u8,
    pub started_at: Instant,
    pub attempts: Vec<Attempt>,
    pub constraints_discovered: Vec<Constraint>,
    pub strategy: Strategy,
    pub criteria_status: Vec<CriterionStatus>,
    /// Currently active mitigation from risk_assessment, if any.
    active_mitigation: Option<Risk>,
}

impl ExecutionState {
    /// Create a new execution state from a GoalFrame.
    pub fn new(goal: GoalFrame) -> Self {
        let criteria_count = goal.done_criteria.len();
        Self {
            goal,
            iteration: 0,
            started_at: Instant::now(),
            attempts: Vec::new(),
            constraints_discovered: Vec::new(),
            strategy: Strategy::Initial,
            criteria_status: vec![CriterionStatus::NotAttempted; criteria_count],
            active_mitigation: None,
        }
    }

    /// Record an attempt and extract any constraint from a failure.
    pub fn record_attempt(&mut self, attempt: Attempt) {
        if let AttemptResult::Failure(ref sf) = attempt.result {
            if let Some(constraint) = sf.to_constraint() {
                self.constraints_discovered.push(constraint);
            }
        }
        self.attempts.push(attempt);
    }

    /// Set the active mitigation (from risk_assessment) for the current strategy.
    pub fn set_active_mitigation(&mut self, risk: Option<Risk>) {
        self.active_mitigation = risk;
    }

    /// Get the active mitigation, if any.
    pub fn active_mitigation(&self) -> Option<&Risk> {
        self.active_mitigation.as_ref()
    }

    /// Mark a criterion as met.
    pub fn mark_criterion_met(&mut self, index: usize) {
        if index < self.criteria_status.len() {
            self.criteria_status[index] = CriterionStatus::Met {
                verified_at: Instant::now(),
            };
        }
    }

    /// Mark a criterion as failed.
    pub fn mark_criterion_failed(&mut self, index: usize, reason: String) {
        if index < self.criteria_status.len() {
            self.criteria_status[index] = CriterionStatus::Failed { reason };
        }
    }

    /// Count how many done_criteria have been met.
    pub fn met_criteria_count(&self) -> usize {
        self.criteria_status
            .iter()
            .filter(|s| matches!(s, CriterionStatus::Met { .. }))
            .count()
    }

    /// Check if all done_criteria are met.
    pub fn all_criteria_met(&self) -> bool {
        self.met_criteria_count() == self.goal.done_criteria.len()
    }
}

/// A single execution attempt within an iteration.
#[derive(Debug, Clone)]
pub struct Attempt {
    pub iteration: u8,
    pub tool: String,
    /// Summary of args (not full args, which may contain secrets).
    pub args_summary: String,
    pub result: AttemptResult,
    pub timestamp: Instant,
}

/// Result of a single tool execution attempt.
#[derive(Debug, Clone)]
pub enum AttemptResult {
    Success {
        /// Brief description of what succeeded.
        summary: String,
    },
    Failure(StructuredFailure),
}

/// The Governor's current execution strategy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Strategy {
    /// First attempt — follow the plan as given.
    Initial,
    /// Same approach with constraint-aware modifications.
    RetryWithConstraints,
    /// Plan failed — try a fundamentally different approach (uses Standard model).
    AlternativeApproach,
    /// Agent cannot proceed without user input — ask a specific question.
    EscalateToUser,
    /// All options exhausted — report diagnosis and stop.
    Abort,
}

impl std::fmt::Display for Strategy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Initial => write!(f, "Follow the suggested approach"),
            Self::RetryWithConstraints => write!(
                f,
                "Previous attempt partially failed — retry with discovered constraints applied"
            ),
            Self::AlternativeApproach => write!(
                f,
                "Previous approach FAILED REPEATEDLY — try a fundamentally different approach"
            ),
            Self::EscalateToUser => write!(f, "Escalating to user for input"),
            Self::Abort => write!(f, "Aborting — all options exhausted"),
        }
    }
}

/// Status of a single done_criterion.
#[derive(Debug, Clone)]
pub enum CriterionStatus {
    NotAttempted,
    InProgress,
    Met { verified_at: Instant },
    Failed { reason: String },
}

/// The Governor's assessment of whether execution is advancing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Progress {
    /// No tool calls attempted yet.
    NoAttempts,
    /// At least one new success since last check.
    MakingProgress,
    /// No new successes AND no new unique failures — stuck.
    Stalled,
    /// Same failure repeated (same kind_key) — actively looping.
    Looping { repeated_failure: String },
    /// All done_criteria verified as met.
    Complete,
    /// Some criteria met, some not.
    PartialSuccess {
        met: Vec<String>,
        unmet: Vec<String>,
    },
}

/// Detect progress from the execution state.
pub fn detect_progress(state: &ExecutionState) -> Progress {
    if state.attempts.is_empty() {
        return Progress::NoAttempts;
    }

    // Check for completed criteria
    if state.all_criteria_met() {
        return Progress::Complete;
    }

    // Check for repeated identical failures (loop detection)
    let recent_failures: Vec<String> = state
        .attempts
        .iter()
        .rev()
        .take(3)
        .filter_map(|a| match &a.result {
            AttemptResult::Failure(f) => Some(f.kind_key()),
            _ => None,
        })
        .collect();

    if recent_failures.len() >= 2
        && recent_failures.windows(2).all(|w| w[0] == w[1])
    {
        return Progress::Looping {
            repeated_failure: recent_failures[0].clone(),
        };
    }

    // Check for progress within the current iteration
    let current_iter = state.iteration;
    let successes_this_iter = state
        .attempts
        .iter()
        .filter(|a| a.iteration == current_iter)
        .filter(|a| matches!(a.result, AttemptResult::Success { .. }))
        .count();

    if successes_this_iter > 0 {
        return Progress::MakingProgress;
    }

    // Check if we're seeing new failure types (learning) or just repeating
    let unique_failures_this_iter: HashSet<String> = state
        .attempts
        .iter()
        .filter(|a| a.iteration == current_iter)
        .filter_map(|a| match &a.result {
            AttemptResult::Failure(f) => Some(f.kind_key()),
            _ => None,
        })
        .collect();

    let prior_failures: HashSet<String> = state
        .attempts
        .iter()
        .filter(|a| a.iteration < current_iter)
        .filter_map(|a| match &a.result {
            AttemptResult::Failure(f) => Some(f.kind_key()),
            _ => None,
        })
        .collect();

    if !unique_failures_this_iter.is_empty()
        && unique_failures_this_iter.is_subset(&prior_failures)
    {
        Progress::Stalled
    } else {
        // New failure types or first iteration — at least we're learning
        Progress::MakingProgress
    }
}

/// Format execution state as a summary block for prompt injection.
pub fn format_state_summary(state: &ExecutionState) -> String {
    let mut out = String::new();

    out.push_str(&format!(
        "## Execution State (Injected by Governor)\n\n### Goal\n{}\n\n### Iteration {} of {}\n\n### Strategy: {}\n\n",
        state.goal.intent.summary,
        state.iteration + 1,
        state.goal.max_iterations,
        state.strategy,
    ));

    if !state.attempts.is_empty() {
        out.push_str("### What You've Tried\n");
        for (i, attempt) in state.attempts.iter().enumerate() {
            let result_str = match &attempt.result {
                AttemptResult::Success { summary } => format!("OK: {}", summary),
                AttemptResult::Failure(sf) => format!("FAILED: {}", sf.kind_key()),
            };
            out.push_str(&format!(
                "{}. [iter {}] {}({}) → {}\n",
                i + 1,
                attempt.iteration + 1,
                attempt.tool,
                attempt.args_summary,
                result_str
            ));
        }
        out.push('\n');
    }

    if !state.constraints_discovered.is_empty() {
        out.push_str("### Constraints Discovered (RESPECT THESE)\n");
        for constraint in &state.constraints_discovered {
            out.push_str(&format!("- {}\n", constraint));
        }
        out.push('\n');
    }

    out.push_str("### Done Criteria Status\n");
    for (i, criterion) in state.goal.done_criteria.iter().enumerate() {
        let status = state
            .criteria_status
            .get(i)
            .map(|s| match s {
                CriterionStatus::NotAttempted => "NOT ATTEMPTED",
                CriterionStatus::InProgress => "IN PROGRESS",
                CriterionStatus::Met { .. } => "MET",
                CriterionStatus::Failed { .. } => "FAILED",
            })
            .unwrap_or("UNKNOWN");
        out.push_str(&format!("- [{}] {}: {}\n", criterion.id, status, criterion.description));
    }
    out.push('\n');

    out.push_str("### Rules for This Iteration\n");
    out.push_str("- NEVER repeat a tool call with identical parameters to a failed attempt above\n");
    out.push_str("- If a constraint says a path is blocked, use the alternative path listed\n");
    out.push_str("- If a constraint says a host is unreachable, use the alternative host listed\n");
    out.push_str("- State your reasoning before each tool call\n");

    if state.strategy == Strategy::AlternativeApproach {
        if let Some(risk) = state.active_mitigation() {
            out.push_str(&format!(
                "- The planner anticipated this failure.\n  Suggested mitigation: {}\n  If that fails: {}\n",
                risk.mitigation, risk.fallback
            ));
        } else {
            out.push_str("- The planner did not anticipate this failure. Think carefully about an alternative approach.\n");
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::goal_frame::*;

    fn make_test_goal() -> GoalFrame {
        GoalFrame {
            intent: Intent {
                category: IntentCategory::Configure,
                summary: "Set up email".to_string(),
                implicit_requirements: vec![],
            },
            done_criteria: vec![
                Criterion {
                    id: "d1".to_string(),
                    description: "Email config written".to_string(),
                    verifier: Some(ToolVerifier {
                        tool: "read_file".to_string(),
                        args: serde_json::json!({"path": "/app/email.json"}),
                        expected: "file exists".to_string(),
                    }),
                },
                Criterion {
                    id: "d2".to_string(),
                    description: "Email connection works".to_string(),
                    verifier: Some(ToolVerifier {
                        tool: "fetch_emails".to_string(),
                        args: serde_json::json!({}),
                        expected: "no error".to_string(),
                    }),
                },
            ],
            good_criteria: vec![],
            risk_assessment: vec![Risk {
                failure_mode: "permission denied on file write".to_string(),
                mitigation: "write to /app/agent-data/ instead".to_string(),
                fallback: "ask user for writable path".to_string(),
            }],
            suggested_approach: Approach {
                steps: vec!["write config".to_string(), "test connection".to_string()],
                rationale: "test".to_string(),
                estimated_tool_calls: 2,
            },
            abort_conditions: vec!["3 consecutive identical failures".to_string()],
            max_iterations: 5,
            time_budget: None,
        }
    }

    #[test]
    fn new_state_initializes_correctly() {
        let state = ExecutionState::new(make_test_goal());
        assert_eq!(state.iteration, 0);
        assert_eq!(state.attempts.len(), 0);
        assert_eq!(state.criteria_status.len(), 2);
        assert!(matches!(state.strategy, Strategy::Initial));
    }

    #[test]
    fn record_attempt_extracts_constraint() {
        let mut state = ExecutionState::new(make_test_goal());
        state.record_attempt(Attempt {
            iteration: 0,
            tool: "write_file".to_string(),
            args_summary: "path=/app/vault/email.json".to_string(),
            result: AttemptResult::Failure(StructuredFailure::PermissionDenied {
                path: "/app/vault".to_string(),
                allowed_paths: vec!["/app/agent-data".to_string()],
            }),
            timestamp: Instant::now(),
        });
        assert_eq!(state.constraints_discovered.len(), 1);
    }

    #[test]
    fn detect_progress_no_attempts() {
        let state = ExecutionState::new(make_test_goal());
        assert_eq!(detect_progress(&state), Progress::NoAttempts);
    }

    #[test]
    fn detect_progress_looping() {
        let mut state = ExecutionState::new(make_test_goal());
        let failure = StructuredFailure::PermissionDenied {
            path: "/app/vault".to_string(),
            allowed_paths: vec![],
        };
        for _ in 0..2 {
            state.record_attempt(Attempt {
                iteration: 0,
                tool: "write_file".to_string(),
                args_summary: "path=/app/vault/x".to_string(),
                result: AttemptResult::Failure(failure.clone()),
                timestamp: Instant::now(),
            });
        }
        let progress = detect_progress(&state);
        assert!(
            matches!(progress, Progress::Looping { .. }),
            "Expected Looping, got {:?}",
            progress
        );
    }

    #[test]
    fn detect_progress_making_progress() {
        let mut state = ExecutionState::new(make_test_goal());
        state.record_attempt(Attempt {
            iteration: 0,
            tool: "write_file".to_string(),
            args_summary: "path=/app/agent-data/email.json".to_string(),
            result: AttemptResult::Success {
                summary: "file written".to_string(),
            },
            timestamp: Instant::now(),
        });
        assert_eq!(detect_progress(&state), Progress::MakingProgress);
    }

    #[test]
    fn detect_progress_complete() {
        let mut state = ExecutionState::new(make_test_goal());
        state.mark_criterion_met(0);
        state.mark_criterion_met(1);
        // Need at least one attempt to not be NoAttempts
        state.record_attempt(Attempt {
            iteration: 0,
            tool: "check".to_string(),
            args_summary: "".to_string(),
            result: AttemptResult::Success {
                summary: "done".to_string(),
            },
            timestamp: Instant::now(),
        });
        assert_eq!(detect_progress(&state), Progress::Complete);
    }

    #[test]
    fn detect_progress_stalled() {
        let mut state = ExecutionState::new(make_test_goal());
        // Iteration 0: a failure
        state.record_attempt(Attempt {
            iteration: 0,
            tool: "write_file".to_string(),
            args_summary: "path=/app/vault/x".to_string(),
            result: AttemptResult::Failure(StructuredFailure::PermissionDenied {
                path: "/app/vault".to_string(),
                allowed_paths: vec![],
            }),
            timestamp: Instant::now(),
        });
        // Move to iteration 1 — same failure type, different tool call
        state.iteration = 1;
        state.record_attempt(Attempt {
            iteration: 1,
            tool: "write_file".to_string(),
            args_summary: "path=/app/vault/y".to_string(),
            result: AttemptResult::Failure(StructuredFailure::PermissionDenied {
                path: "/app/vault".to_string(),
                allowed_paths: vec![],
            }),
            timestamp: Instant::now(),
        });
        let progress = detect_progress(&state);
        // Same kind_key repeated → Looping (not Stalled, since the keys match)
        assert!(
            matches!(progress, Progress::Looping { .. }),
            "Expected Looping, got {:?}",
            progress
        );
    }

    #[test]
    fn format_state_summary_includes_key_sections() {
        let mut state = ExecutionState::new(make_test_goal());
        state.record_attempt(Attempt {
            iteration: 0,
            tool: "write_file".to_string(),
            args_summary: "path=/app/vault/email.json".to_string(),
            result: AttemptResult::Failure(StructuredFailure::PermissionDenied {
                path: "/app/vault".to_string(),
                allowed_paths: vec!["/app/agent-data".to_string()],
            }),
            timestamp: Instant::now(),
        });
        let summary = format_state_summary(&state);
        assert!(summary.contains("Set up email"));
        assert!(summary.contains("write_file"));
        assert!(summary.contains("PATH BLOCKED"));
        assert!(summary.contains("Done Criteria Status"));
    }
}
