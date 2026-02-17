//! GoalFrame — the structured plan produced by the Planner before execution begins.
//!
//! The GoalFrame is the single source of truth for what the agent is trying to accomplish.
//! It defines verifiable "done" and "good" criteria so the Execution Governor can
//! determine completion, detect stalls, and report structured results.

use serde::{Deserialize, Serialize};
use std::time::Duration;

/// The structured plan produced by the Planner before execution begins.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoalFrame {
    /// What the user wants, in structured form.
    pub intent: Intent,

    /// What "done" looks like — each criterion must be met for the task to be complete.
    /// Every criterion SHOULD have a verifier (a tool call that can confirm it was met).
    pub done_criteria: Vec<Criterion>,

    /// What "good" looks like — quality criteria beyond the bare minimum.
    pub good_criteria: Vec<QualityCriterion>,

    /// Anticipated failure modes and what to do about them.
    /// The planner's "pre-mortem" — runs on the best model to predict what will go wrong.
    pub risk_assessment: Vec<Risk>,

    /// The planner's suggested approach — ordered steps for the executor to follow.
    pub suggested_approach: Approach,

    /// When to give up and ask the user for help.
    pub abort_conditions: Vec<String>,

    /// Maximum execution iterations before forced escalation.
    pub max_iterations: u8,

    /// Time budget for the entire execution. None = no limit.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "optional_duration_secs"
    )]
    pub time_budget: Option<Duration>,
}

/// Serde helper: serialize/deserialize `Option<Duration>` as optional seconds (f64).
mod optional_duration_secs {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::time::Duration;

    pub fn serialize<S: Serializer>(val: &Option<Duration>, s: S) -> Result<S::Ok, S::Error> {
        match val {
            Some(d) => d.as_secs_f64().serialize(s),
            None => s.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Duration>, D::Error> {
        let opt: Option<f64> = Option::deserialize(d)?;
        Ok(opt.map(Duration::from_secs_f64))
    }
}

impl GoalFrame {
    /// Validate the GoalFrame for structural soundness.
    /// Returns a list of warnings/errors. An empty vec means the frame is valid.
    pub fn validate(&self) -> Vec<String> {
        let mut issues = Vec::new();

        if self.done_criteria.is_empty() {
            issues.push("GoalFrame has no done_criteria".to_string());
        }

        for criterion in &self.done_criteria {
            if criterion.verifier.is_none() {
                issues.push(format!(
                    "done_criteria '{}' has no verifier — completion cannot be structurally confirmed",
                    criterion.id
                ));
            }
        }

        let is_io_intent = matches!(
            self.intent.category,
            IntentCategory::Configure
                | IntentCategory::Create
                | IntentCategory::Modify
                | IntentCategory::Communicate
        );

        if is_io_intent && self.risk_assessment.is_empty() {
            issues.push(format!(
                "GoalFrame for {:?} intent has empty risk_assessment — I/O tasks should anticipate failures",
                self.intent.category
            ));
        }

        if self.max_iterations == 0 {
            issues.push("max_iterations must be at least 1".to_string());
        }

        issues
    }

    /// Find a risk whose failure_mode description matches a failure kind key.
    pub fn find_matching_risk(&self, failure_kind_key: &str) -> Option<&Risk> {
        self.risk_assessment.iter().find(|r| {
            let fm_lower = r.failure_mode.to_lowercase();
            let key_lower = failure_kind_key.to_lowercase();
            // Match on the failure category prefix (e.g., "permission_denied" matches
            // a risk about "permission denied" or "file write permission")
            let kind_prefix = key_lower.split(':').next().unwrap_or("");
            let prefix_words = kind_prefix.replace('_', " ");
            fm_lower.contains(&prefix_words) || fm_lower.contains(&key_lower)
        })
    }

    /// Build a minimal fallback GoalFrame for when the Planner fails to produce valid output.
    pub fn fallback(user_summary: &str) -> Self {
        Self {
            intent: Intent {
                category: IntentCategory::Other("unknown".to_string()),
                summary: user_summary.to_string(),
                implicit_requirements: vec![
                    "Report results clearly to the user".to_string(),
                    "Ask if something is unclear rather than guessing".to_string(),
                ],
            },
            done_criteria: vec![Criterion {
                id: "d1".to_string(),
                description: "User's request is addressed".to_string(),
                verifier: None,
            }],
            good_criteria: vec![],
            risk_assessment: vec![],
            suggested_approach: Approach {
                steps: vec!["Attempt the user's request directly".to_string()],
                rationale: "Fallback plan — Planner did not produce structured output".to_string(),
                estimated_tool_calls: 3,
            },
            abort_conditions: vec!["Three consecutive failures with the same error".to_string()],
            max_iterations: 5,
            time_budget: Some(Duration::from_secs(120)),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Intent {
    /// High-level category for pattern matching and telemetry.
    pub category: IntentCategory,
    /// One sentence: what the user actually wants accomplished.
    pub summary: String,
    /// Things the user didn't say but clearly expects.
    pub implicit_requirements: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum IntentCategory {
    Configure,
    Search,
    Create,
    Diagnose,
    Communicate,
    Analyze,
    Modify,
    Monitor,
    Other(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Criterion {
    /// Unique identifier within this GoalFrame (e.g., "d1", "d2").
    pub id: String,
    /// Human-readable description of what must be true.
    pub description: String,
    /// A tool call that can verify this criterion is met.
    pub verifier: Option<ToolVerifier>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolVerifier {
    /// The skill tool to call.
    pub tool: String,
    /// Arguments to pass.
    pub args: serde_json::Value,
    /// What a successful result looks like (human-readable, for logging).
    pub expected: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QualityCriterion {
    pub id: String,
    pub description: String,
    pub priority: QualityPriority,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum QualityPriority {
    Must,
    Should,
    Nice,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Risk {
    /// What could go wrong.
    pub failure_mode: String,
    /// What to try if it happens (specific enough for the executor to follow).
    pub mitigation: String,
    /// What to do if the mitigation also fails.
    pub fallback: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Approach {
    /// Ordered list of high-level steps.
    pub steps: Vec<String>,
    /// Why this approach over alternatives.
    pub rationale: String,
    /// Expected number of tool calls (for progress tracking).
    pub estimated_tool_calls: u8,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_goal_frame(category: IntentCategory, verifiers: bool, risks: bool) -> GoalFrame {
        GoalFrame {
            intent: Intent {
                category,
                summary: "Test goal".to_string(),
                implicit_requirements: vec![],
            },
            done_criteria: vec![Criterion {
                id: "d1".to_string(),
                description: "Something is done".to_string(),
                verifier: if verifiers {
                    Some(ToolVerifier {
                        tool: "check_tool".to_string(),
                        args: serde_json::json!({}),
                        expected: "success".to_string(),
                    })
                } else {
                    None
                },
            }],
            good_criteria: vec![],
            risk_assessment: if risks {
                vec![Risk {
                    failure_mode: "permission denied on file write".to_string(),
                    mitigation: "use allowed path".to_string(),
                    fallback: "ask user".to_string(),
                }]
            } else {
                vec![]
            },
            suggested_approach: Approach {
                steps: vec!["step 1".to_string()],
                rationale: "test".to_string(),
                estimated_tool_calls: 1,
            },
            abort_conditions: vec!["fail".to_string()],
            max_iterations: 5,
            time_budget: None,
        }
    }

    #[test]
    fn validate_rejects_missing_verifiers() {
        let gf = make_goal_frame(IntentCategory::Search, false, false);
        let issues = gf.validate();
        assert!(issues.iter().any(|i| i.contains("no verifier")));
    }

    #[test]
    fn validate_accepts_with_verifiers() {
        let gf = make_goal_frame(IntentCategory::Search, true, false);
        let issues = gf.validate();
        assert!(!issues.iter().any(|i| i.contains("no verifier")));
    }

    #[test]
    fn validate_rejects_empty_risk_on_io_intents() {
        for cat in [
            IntentCategory::Configure,
            IntentCategory::Create,
            IntentCategory::Modify,
            IntentCategory::Communicate,
        ] {
            let gf = make_goal_frame(cat, true, false);
            let issues = gf.validate();
            assert!(
                issues.iter().any(|i| i.contains("empty risk_assessment")),
                "Expected risk warning for {:?}",
                gf.intent.category
            );
        }
    }

    #[test]
    fn validate_accepts_risk_on_io_intents() {
        let gf = make_goal_frame(IntentCategory::Configure, true, true);
        let issues = gf.validate();
        assert!(issues.is_empty(), "Expected no issues, got: {:?}", issues);
    }

    #[test]
    fn validate_non_io_intents_dont_require_risks() {
        for cat in [
            IntentCategory::Search,
            IntentCategory::Analyze,
            IntentCategory::Diagnose,
            IntentCategory::Monitor,
        ] {
            let gf = make_goal_frame(cat, true, false);
            let issues = gf.validate();
            assert!(
                !issues.iter().any(|i| i.contains("risk_assessment")),
                "Unexpected risk warning for {:?}",
                gf.intent.category
            );
        }
    }

    #[test]
    fn validate_rejects_zero_max_iterations() {
        let mut gf = make_goal_frame(IntentCategory::Search, true, false);
        gf.max_iterations = 0;
        let issues = gf.validate();
        assert!(issues.iter().any(|i| i.contains("max_iterations")));
    }

    #[test]
    fn find_matching_risk_by_prefix() {
        let gf = make_goal_frame(IntentCategory::Configure, true, true);
        let risk = gf.find_matching_risk("permission_denied:/app/vault");
        assert!(risk.is_some());
    }

    #[test]
    fn find_matching_risk_returns_none_for_unknown() {
        let gf = make_goal_frame(IntentCategory::Configure, true, true);
        let risk = gf.find_matching_risk("timeout:fetch_emails");
        assert!(risk.is_none());
    }

    #[test]
    fn fallback_is_structurally_valid_enough() {
        let gf = GoalFrame::fallback("do something");
        assert_eq!(gf.max_iterations, 5);
        assert!(!gf.done_criteria.is_empty());
    }

    #[test]
    fn goalframe_serialization_roundtrip() {
        let gf = make_goal_frame(IntentCategory::Configure, true, true);
        let json = serde_json::to_string(&gf).unwrap();
        let deserialized: GoalFrame = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.intent.summary, gf.intent.summary);
        assert_eq!(deserialized.done_criteria.len(), gf.done_criteria.len());
    }
}
