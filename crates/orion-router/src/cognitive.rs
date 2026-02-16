//! Cognitive transform traits and artifact types.
//!
//! Two-layer composition system:
//! - Layer 1 (`CognitiveTransform<I, O>`): statically-typed for known pipelines
//! - Layer 2 (`Agent`): dynamically-dispatched for Governor/Council orchestration

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::goal_frame::GoalFrame;

/// The common artifact envelope for dynamic dispatch.
/// Named `CognitiveArtifact` to avoid collision with the council's `Artifact` trait.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CognitiveArtifact {
    UserRequest(UserRequest),
    GoalFrame(GoalFrame),
    ToolCallSet(Vec<ToolCallRef>),
    ToolResultSet(Vec<ToolResultRef>),
    ExecutionReport(ExecutionReport),
    ChatResponse(ChatResponse),
    /// Escape hatch — tracked. Every Raw in production logs
    /// signals that a proper variant is needed.
    Raw(String),
}

/// Marker for what kind of artifact something is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ArtifactKind {
    UserRequest,
    GoalFrame,
    ToolCallSet,
    ToolResultSet,
    ExecutionReport,
    ChatResponse,
    Raw,
}

impl CognitiveArtifact {
    /// Return the kind marker for this artifact.
    pub fn kind(&self) -> ArtifactKind {
        match self {
            Self::UserRequest(_) => ArtifactKind::UserRequest,
            Self::GoalFrame(_) => ArtifactKind::GoalFrame,
            Self::ToolCallSet(_) => ArtifactKind::ToolCallSet,
            Self::ToolResultSet(_) => ArtifactKind::ToolResultSet,
            Self::ExecutionReport(_) => ArtifactKind::ExecutionReport,
            Self::ChatResponse(_) => ArtifactKind::ChatResponse,
            Self::Raw(_) => ArtifactKind::Raw,
        }
    }
}

/// A user request entering the cognitive pipeline.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserRequest {
    pub message: String,
    /// Available skill tool descriptors (serialized).
    pub available_tools: Vec<serde_json::Value>,
    /// Persistent constraints from previous sessions.
    pub known_constraints: Vec<String>,
    /// Summary of agent state (name, birth status, etc.).
    pub agent_state_summary: String,
}

/// Reference to a tool call (lightweight, for artifact passing).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallRef {
    pub tool: String,
    pub args: serde_json::Value,
}

/// Reference to a tool result (lightweight, for artifact passing).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResultRef {
    pub tool: String,
    pub success: bool,
    pub output_summary: String,
}

/// Summary report of a governed execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionReport {
    pub success: bool,
    pub iterations_used: u8,
    pub criteria_met: Vec<String>,
    pub criteria_unmet: Vec<String>,
    pub constraints_discovered: Vec<String>,
    pub summary: String,
}

/// Chat response for the user.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatResponse {
    pub content: String,
    pub tool_log: Vec<ToolResultRef>,
}

/// Cognitive task categories — determines which model tier is used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CognitiveTask {
    /// Understanding intent, anticipating failures, structured output.
    /// ALWAYS uses Pro tier.
    Planning,
    /// Following a plan, generating tool calls.
    /// Uses Fast tier (cheap cloud model).
    Execution,
    /// Diagnosing failures, proposing alternative approaches.
    /// Uses Standard tier.
    Recovery,
    /// Summarizing results for the user.
    /// Uses Fast tier.
    Reporting,
    /// Checking whether criteria are met. Prefer structural verification.
    /// Falls back to Fast tier LLM only when structural verification isn't possible.
    Verification,
    /// Multi-model debate/synthesis (existing Council Engine).
    /// Uses Pro tier.
    CouncilDebate,
}

/// Session context passed to all cognitive transforms.
#[derive(Debug, Clone)]
pub struct SessionContext {
    pub agent_id: String,
    pub agent_name: String,
    pub system_prompt: String,
    pub conversation_history: Vec<orion_capabilities::cognitive::Message>,
}

/// Layer 2: Dynamic dispatch for Governor/Council orchestration.
#[async_trait]
pub trait Agent: Send + Sync {
    fn name(&self) -> &str;

    /// What artifact kinds this agent can accept as input.
    fn accepts(&self) -> &[ArtifactKind];

    /// What artifact kind this agent produces.
    fn produces(&self) -> ArtifactKind;

    /// Which cognitive task this represents (for model selection).
    fn cognitive_task(&self) -> CognitiveTask;

    /// Execute with dynamic dispatch.
    async fn execute(
        &self,
        input: CognitiveArtifact,
        ctx: &SessionContext,
    ) -> anyhow::Result<CognitiveArtifact>;
}

/// Layer 1: Statically-typed composition for known pipelines.
#[async_trait]
pub trait CognitiveTransform<I, O>: Agent
where
    I: Send + 'static,
    O: Send + 'static,
{
    async fn transform(&self, input: I, ctx: &SessionContext) -> anyhow::Result<O>;
}
