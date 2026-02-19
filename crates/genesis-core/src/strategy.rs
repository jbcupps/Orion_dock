//! GenesisStrategy trait: step-based protocol for interactive Genesis paths.

use crate::error::GenesisError;
use crate::manifest::SoulManifest;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Metadata about a genesis path, used for listing and selection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenesisPathInfo {
    /// Unique path identifier (e.g., "soul_forge", "direct_discovery").
    pub id: String,
    /// Human-readable label (e.g., "Soul Forge").
    pub label: String,
    /// Description for path selector UI.
    pub description: String,
    /// Estimated time (e.g., "~2 minutes").
    pub estimated_time: String,
    /// Whether this path requires LLM chat.
    pub requires_chat: bool,
    /// Optional sub-variants (e.g., crystallization depth levels).
    #[serde(default)]
    pub variants: Vec<GenesisPathVariant>,
}

/// A sub-variant of a genesis path (e.g., depth levels).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenesisPathVariant {
    pub id: String,
    pub label: String,
    pub description: String,
    pub estimated_time: String,
}

/// Context injected into a strategy at initialization.
pub struct GenesisContext {
    /// Agent name (already chosen at agent creation).
    pub agent_name: String,
    /// Mentor name (collected at Genesis start or defaulted).
    pub mentor_name: String,
    /// Path to constitutional documents directory.
    pub docs_dir: PathBuf,
    /// Path to agent data directory.
    pub data_dir: PathBuf,
    /// Optional chat function for LLM interaction (decoupled from router).
    pub chat_fn: Option<Box<dyn ChatFunction>>,
    /// Optional path variant (e.g., depth level for crystallization).
    pub variant: Option<String>,
    /// Number of scenarios for forge (default 5).
    pub scenario_count: Option<usize>,
}

/// Decoupled chat function: sends messages, receives assistant response.
#[async_trait]
pub trait ChatFunction: Send + Sync {
    async fn chat(&self, messages: Vec<ChatMessage>) -> Result<String, GenesisError>;
}

/// A chat message for the chat function.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

/// What the strategy needs from the caller (UI/API).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum StepRequest {
    /// Genesis path is complete. Contains the final SoulManifest.
    Complete { manifest: Box<SoulManifest> },
    /// Path needs a free-form chat message from the user.
    NeedUserMessage {
        /// Display prompt / assistant message to show.
        prompt: String,
        /// Full conversation so far (for UI display).
        conversation: Vec<ChatMessage>,
    },
    /// Path needs the user to pick from discrete choices.
    NeedChoice {
        /// Prompt text / scenario narrative.
        prompt: String,
        /// Available choices.
        choices: Vec<ChoiceOption>,
        /// Optional metadata (scenario index, total, category, etc.)
        metadata: Option<serde_json::Value>,
    },
    /// Path needs structured form input.
    NeedForm {
        /// Description of what the form collects.
        prompt: String,
        /// Field definitions.
        fields: Vec<FormField>,
    },
}

/// A discrete choice option.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChoiceOption {
    pub label: String,
    pub description: String,
}

/// A form field.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FormField {
    pub name: String,
    pub label: String,
    pub required: bool,
    pub default_value: Option<String>,
}

/// What the caller (UI/API) sends back.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum StepResponse {
    /// Free-form chat message.
    Message { text: String },
    /// Index into choices array.
    Choice { index: usize },
    /// Form field values.
    Form {
        values: std::collections::HashMap<String, String>,
    },
}

/// The trait all genesis paths implement.
#[async_trait]
pub trait GenesisStrategy: Send + Sync {
    /// Return metadata about this path.
    fn info(&self) -> GenesisPathInfo;

    /// Initialize with context. Called once before begin().
    async fn initialize(&mut self, ctx: GenesisContext) -> Result<(), GenesisError>;

    /// Start the path — returns the first step request.
    async fn begin(&mut self) -> Result<StepRequest, GenesisError>;

    /// Advance the path with user input — returns the next step or Complete.
    async fn advance(&mut self, input: StepResponse) -> Result<StepRequest, GenesisError>;

    /// Serialize current state for persistence/recovery.
    fn snapshot(&self) -> Result<serde_json::Value, GenesisError>;

    /// Restore from a previously serialized snapshot.
    fn restore(&mut self, snapshot: serde_json::Value) -> Result<(), GenesisError>;

    /// Cancel the path (cleanup).
    fn cancel(&mut self);
}
