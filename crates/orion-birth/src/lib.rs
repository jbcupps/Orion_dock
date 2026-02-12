pub mod chat;
pub mod prompts;
pub mod stages;

pub use chat::{
    birth_chat_turn, build_birth_messages, build_birth_router, build_genesis_messages,
    detect_provider_from_key, execute_store_provider_key, extract_api_keys_from_text,
    parse_tool_requests, redact_api_keys, BirthChatResponse, BirthToolRequest,
};
pub use stages::{BirthOrchestrator, BirthStage, GenesisPath, SoulCrystallizationDepth};
