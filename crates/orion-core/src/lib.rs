//! Core infrastructure for the Orion agent platform.
//!
//! Provides foundational building blocks shared across all Orion crates:
//! configuration management ([`AppConfig`]), Ed25519 cryptographic identity
//! ([`keyring`]), API-key secrets vault ([`SecretsVault`]), constitutional
//! document signing and verification ([`Verifier`]), system-prompt assembly,
//! and soul/ethics/instinct templates.

pub mod config;
pub mod document;
pub mod dpapi;
pub mod email_auth;
pub mod encrypted_storage;
pub mod error;
pub mod global_config;
pub mod keyring;
pub mod local_llm_url;
pub mod sao_bridge;
pub mod secrets;
pub mod superego;
pub mod system_prompt;
pub mod templates;
pub mod vault;
pub mod verifier;

pub use config::{
    curated_provider_models, AppConfig, EmailAccountConfig, EmailAccountStatus, EmailAuthType,
    EmailConfig, EmailProvider, McpServerDefinition, McpTrustPolicy, MemoryBackend,
    ProviderCatalogEntry, RoutingMode, ThinkingModelTier, TierModels, TrinityConfig,
    CONFIG_SCHEMA_VERSION,
};
pub use document::{CoreDocument, DocumentTier};
pub use email_auth::{
    access_token_valid, get_oauth_tokens, get_password, has_refresh_token, needs_reauth,
    revoke_account, store_oauth_tokens, store_password, vault_keys_for_account, OAuth2Tokens,
};
pub use error::{CoreError, Result};
pub use global_config::{AgentEntry, GlobalConfig};
pub use keyring::{
    delete_signing_key, generate_external_keypair, generate_master_key, load_master_key,
    load_signing_key, parse_private_key, persist_signing_key, sign_agent_key,
    sign_constitutional_documents, sign_document, verify_agent_signature, ExternalKeypairResult,
    Keyring, MasterKeyResult, SignatureMetadata,
};
pub use local_llm_url::validate_local_llm_url;
pub use sao_bridge::{AgentState, SaoBridgeClient, SaoBridgeError};
pub use secrets::SecretsVault;
pub use superego::{check_message, check_search_query, SuperegoVerdict};
pub use vault::{ExternalVault, ReadOnlyFileVault};
pub use verifier::{write_sig_file, SigMeta, Verifier};
