//! Planner — produces a GoalFrame from a user request.
//!
//! The Planner runs ONCE per user request on the BEST available model (Pro tier).
//! It does NOT execute the user's request — it defines what success looks like
//! so the Execution Governor can pursue it.

use std::sync::Arc;

use anyhow::{Context, Result};
use async_trait::async_trait;
use tracing::{info, warn};

use orion_capabilities::cognitive::{CompletionRequest, LlmProvider, Message};

use crate::cognitive::{
    Agent, ArtifactKind, CognitiveArtifact, CognitiveTask, CognitiveTransform, SessionContext,
    UserRequest,
};
use crate::goal_frame::GoalFrame;

/// The planning prompt template. This is the most important prompt in the system.
const PLANNING_PROMPT: &str = r#"You are the Planning Module of an autonomous agent. Your job is NOT to execute the user's request. Your job is to DEFINE what success looks like so the execution system can pursue it.

You will receive:
1. The user's request
2. Available tools and their capabilities
3. Known constraints from previous experience (if any)
4. The agent's current state

You must produce a GoalFrame as JSON with these exact fields:

{
  "intent": {
    "category": "configure | search | create | diagnose | communicate | analyze | modify | monitor",
    "summary": "One sentence: what the user actually wants accomplished",
    "implicit_requirements": [
      "Things the user didn't say but clearly expects",
      "Security/persistence/feedback expectations"
    ]
  },
  "done_criteria": [
    {
      "id": "d1",
      "description": "Human-readable criterion",
      "verifier": {
        "tool": "tool_name_from_available_tools",
        "args": {},
        "expected": "What success looks like when this tool is called"
      }
    }
  ],
  "good_criteria": [
    {
      "id": "g1",
      "description": "Quality criterion beyond bare minimum",
      "priority": "Must | Should | Nice"
    }
  ],
  "risk_assessment": [
    {
      "failure_mode": "A specific thing that could go wrong",
      "mitigation": "A specific action to take if it happens",
      "fallback": "What to do if the mitigation also fails"
    }
  ],
  "suggested_approach": {
    "steps": ["Ordered list of high-level steps"],
    "rationale": "Why this approach over alternatives",
    "estimated_tool_calls": 3
  },
  "abort_conditions": [
    "Specific conditions under which to stop and ask the user for help"
  ],
  "max_iterations": 5,
  "time_budget": null
}

RULES:

1. done_criteria must be VERIFIABLE. Each criterion should have a tool call that can confirm it was met. "Email is working" is NOT verifiable. "fetch_emails returns at least 0 results without error" IS verifiable.

2. Think about what will go WRONG, not just the happy path. Your risk_assessment is what prevents the executor from looping on failures. For ANY task involving file I/O, consider permission errors. For ANY task involving network calls, consider connectivity and auth errors. For ANY task in a container, consider host-vs-container networking.

3. If the request is ambiguous, define done_criteria for the most conservative interpretation and note the ambiguity in implicit_requirements.

4. implicit_requirements should capture expectations like:
   - Credentials handled securely (not echoed, not logged)
   - Configuration persists across restarts
   - User is informed of what was accomplished
   - Errors are reported clearly, not silently swallowed

5. suggested_approach should SEQUENCE steps to fail fast:
   - Test connectivity BEFORE writing configs
   - Discover writable paths BEFORE attempting writes
   - Verify credentials BEFORE attempting authenticated operations

6. abort_conditions should be specific to the observed environment. "Something goes wrong" is useless. "IMAP connection fails on all candidate hostnames after 2 attempts" is useful.

7. estimated_tool_calls helps the Governor set expectations. If you estimate 3 calls and the executor has used 8, something is wrong.

RESPOND WITH ONLY THE JSON OBJECT. No markdown fences, no explanation, just valid JSON."#;

/// The Planner produces a GoalFrame from a user request.
pub struct Planner {
    provider: Arc<dyn LlmProvider>,
}

impl Planner {
    pub fn new(provider: Arc<dyn LlmProvider>) -> Self {
        Self { provider }
    }

    /// Build the full planning prompt with context injected.
    fn build_prompt(&self, user_request: &UserRequest, _ctx: &SessionContext) -> Vec<Message> {
        let tools_json = serde_json::to_string_pretty(&user_request.available_tools)
            .unwrap_or_else(|_| "[]".to_string());

        let constraints_text = if user_request.known_constraints.is_empty() {
            "None known.".to_string()
        } else {
            user_request.known_constraints.join("\n- ")
        };

        let full_system = format!(
            "{}\n\nAVAILABLE TOOLS:\n{}\n\nKNOWN CONSTRAINTS FROM PREVIOUS SESSIONS:\n{}\n\nAGENT STATE:\n{}",
            PLANNING_PROMPT,
            tools_json,
            constraints_text,
            user_request.agent_state_summary,
        );

        vec![
            Message::new("system", &full_system),
            Message::new("user", &user_request.message),
        ]
    }

    /// Attempt to parse a GoalFrame from the LLM response text.
    fn parse_goal_frame(text: &str) -> Result<GoalFrame> {
        // Strip markdown code fences if present
        let cleaned = text
            .trim()
            .strip_prefix("```json")
            .or_else(|| text.trim().strip_prefix("```"))
            .unwrap_or(text.trim());
        let cleaned = cleaned.strip_suffix("```").unwrap_or(cleaned).trim();

        serde_json::from_str::<GoalFrame>(cleaned)
            .context("Failed to parse GoalFrame JSON from planner response")
    }

    /// Build a repair prompt when the first parse attempt fails.
    fn repair_prompt(original_response: &str, parse_error: &str) -> Vec<Message> {
        vec![
            Message::new("system", PLANNING_PROMPT),
            Message::new(
                "user",
                format!(
                    "Your previous response could not be parsed as valid JSON.\n\nError: {}\n\nYour response was:\n{}\n\nPlease fix the JSON and respond with ONLY the corrected JSON object.",
                    parse_error, original_response
                ),
            ),
        ]
    }

    /// Execute the planning call. Returns a GoalFrame or falls back to a minimal one.
    pub async fn plan(&self, user_request: &UserRequest, ctx: &SessionContext) -> GoalFrame {
        let messages = self.build_prompt(user_request, ctx);

        let request = CompletionRequest {
            messages,
            tools: None,
        };

        // First attempt
        match self.provider.complete(&request).await {
            Ok(response) => {
                match Self::parse_goal_frame(&response.content) {
                    Ok(gf) => {
                        let issues = gf.validate();
                        if !issues.is_empty() {
                            warn!("GoalFrame validation warnings: {:?}", issues);
                        }
                        info!(
                            "Planner produced GoalFrame: intent={:?}, criteria={}, risks={}",
                            gf.intent.category,
                            gf.done_criteria.len(),
                            gf.risk_assessment.len()
                        );
                        return gf;
                    }
                    Err(first_err) => {
                        warn!("First GoalFrame parse failed: {}", first_err);

                        // Retry with repair prompt
                        let repair = Self::repair_prompt(&response.content, &first_err.to_string());
                        let repair_request = CompletionRequest {
                            messages: repair,
                            tools: None,
                        };

                        match self.provider.complete(&repair_request).await {
                            Ok(retry_response) => {
                                match Self::parse_goal_frame(&retry_response.content) {
                                    Ok(gf) => {
                                        info!("Planner repair succeeded");
                                        return gf;
                                    }
                                    Err(retry_err) => {
                                        warn!(
                                            "GoalFrame repair also failed: {}. Falling back.",
                                            retry_err
                                        );
                                    }
                                }
                            }
                            Err(e) => {
                                warn!("Planner repair call failed: {}. Falling back.", e);
                            }
                        }
                    }
                }
            }
            Err(e) => {
                warn!(
                    "Planner call failed: {}. Falling back to minimal GoalFrame.",
                    e
                );
            }
        }

        // Fallback
        GoalFrame::fallback(&user_request.message)
    }
}

#[async_trait]
impl Agent for Planner {
    fn name(&self) -> &str {
        "Planner"
    }

    fn accepts(&self) -> &[ArtifactKind] {
        &[ArtifactKind::UserRequest]
    }

    fn produces(&self) -> ArtifactKind {
        ArtifactKind::GoalFrame
    }

    fn cognitive_task(&self) -> CognitiveTask {
        CognitiveTask::Planning
    }

    async fn execute(
        &self,
        input: CognitiveArtifact,
        ctx: &SessionContext,
    ) -> Result<CognitiveArtifact> {
        let user_request = match input {
            CognitiveArtifact::UserRequest(req) => req,
            other => anyhow::bail!("Planner expected UserRequest, got {:?}", other.kind()),
        };

        let goal_frame = self.plan(&user_request, ctx).await;
        Ok(CognitiveArtifact::GoalFrame(goal_frame))
    }
}

#[async_trait]
impl CognitiveTransform<UserRequest, GoalFrame> for Planner {
    async fn transform(&self, input: UserRequest, ctx: &SessionContext) -> Result<GoalFrame> {
        Ok(self.plan(&input, ctx).await)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_goal_frame_valid_json() {
        let json = r#"{
            "intent": {
                "category": "Configure",
                "summary": "Set up email",
                "implicit_requirements": ["Config persists"]
            },
            "done_criteria": [{
                "id": "d1",
                "description": "Email config written",
                "verifier": {
                    "tool": "read_file",
                    "args": {"path": "/app/email.json"},
                    "expected": "file exists"
                }
            }],
            "good_criteria": [],
            "risk_assessment": [{
                "failure_mode": "permission denied",
                "mitigation": "use allowed path",
                "fallback": "ask user"
            }],
            "suggested_approach": {
                "steps": ["write config", "test connection"],
                "rationale": "fail fast",
                "estimated_tool_calls": 2
            },
            "abort_conditions": ["all hosts fail"],
            "max_iterations": 5,
            "time_budget": null
        }"#;

        let gf = Planner::parse_goal_frame(json).unwrap();
        assert_eq!(gf.done_criteria.len(), 1);
        assert_eq!(gf.risk_assessment.len(), 1);
        assert_eq!(gf.max_iterations, 5);
    }

    #[test]
    fn parse_goal_frame_with_markdown_fences() {
        let json = "```json\n{\"intent\":{\"category\":\"Search\",\"summary\":\"find files\",\"implicit_requirements\":[]},\"done_criteria\":[],\"good_criteria\":[],\"risk_assessment\":[],\"suggested_approach\":{\"steps\":[],\"rationale\":\"test\",\"estimated_tool_calls\":1},\"abort_conditions\":[],\"max_iterations\":3,\"time_budget\":null}\n```";

        let gf = Planner::parse_goal_frame(json).unwrap();
        assert_eq!(gf.intent.summary, "find files");
    }

    #[test]
    fn parse_goal_frame_invalid_json() {
        let result = Planner::parse_goal_frame("not json at all");
        assert!(result.is_err());
    }
}
