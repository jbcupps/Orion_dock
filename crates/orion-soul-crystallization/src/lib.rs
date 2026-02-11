//! Soul Crystallization for Orion: depth-based psychometric profiling and soul generation.
//!
//! Three depth levels: Quick Start (~30s), Conversation (3-5 min), Deep Dive (10-15 min).
//! Engine is standalone; caller injects LLM and uses orion-birth-style tool_request blocks.

pub mod engine;
pub mod ethics_calibrator;
pub mod extraction;
pub mod models;

pub use engine::{CrystallizationEngine, CrystallizationStatus, ProcessResult, ToolRequest};
pub use ethics_calibrator::calibrate_triangle_ethic;
pub use extraction::{
    build_extraction_prompt, parse_extraction_response, ExtractError, ExtractedIdentity,
};
pub use models::{
    AttachmentStyle, CognitiveStyle, CommunicationPreference, ConversationTurn,
    CrystallizationPhase, DepthLevel, MentorProfile, MoralFoundations, OceanScores, Signal,
    ThinkingMode, TriangleEthicWeights,
};
