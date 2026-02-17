//! Error types for the Genesis system.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum GenesisError {
    #[error("Genesis path not found: {0}")]
    PathNotFound(String),

    #[error("Invalid state: {0}")]
    InvalidState(String),

    #[error("Strategy not initialized")]
    NotInitialized,

    #[error("Strategy already complete")]
    AlreadyComplete,

    #[error("Invalid input: {0}")]
    InvalidInput(String),

    #[error("Chat function not available")]
    NoChatFunction,

    #[error("Snapshot/restore error: {0}")]
    Snapshot(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Serialization error: {0}")]
    Serde(#[from] serde_json::Error),

    #[error("{0}")]
    Other(String),
}
