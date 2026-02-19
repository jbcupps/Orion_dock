//! genesis-core: shared contract crate for the Genesis birth path system.
//!
//! Provides:
//! - `CompassEthicWeights` — 4-dimension ethical weight vector
//! - `SoulManifest` — standardized output from all Genesis paths
//! - `GenesisStrategy` trait — step-based protocol for interactive paths
//! - `GenesisRegistry` — compile-time factory registration
//! - `GenesisError` — error types

pub mod error;
pub mod ethics;
pub mod manifest;
pub mod registry;
pub mod strategy;

pub use error::GenesisError;
pub use ethics::CompassEthicWeights;
pub use manifest::SoulManifest;
pub use registry::GenesisRegistry;
pub use strategy::{
    ChatFunction, ChatMessage, ChoiceOption, FormField, GenesisContext, GenesisPathInfo,
    GenesisPathVariant, GenesisStrategy, StepRequest, StepResponse,
};
