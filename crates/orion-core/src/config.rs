use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::str::FromStr;

/// Current config schema version. Increment when making breaking changes.
pub const CONFIG_SCHEMA_VERSION: u32 = 5;

/// Memory store backend: SQLite (file) or PostgreSQL.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum MemoryBackend {
    #[default]
    Sqlite,
    Postgres,
}

impl MemoryBackend {
    pub fn as_str(&self) -> &'static str {
        match self {
            MemoryBackend::Sqlite => "sqlite",
            MemoryBackend::Postgres => "postgres",
        }
    }
}

impl FromStr for MemoryBackend {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        Ok(match s.trim().to_lowercase().as_str() {
            "postgres" | "postgresql" => MemoryBackend::Postgres,
            _ => MemoryBackend::Sqlite,
        })
    }
}

/// Routing mode determines how messages are routed between Id (local) and Ego (cloud).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum RoutingMode {
    /// Id (local) classifies, routes complex to Ego (legacy behavior)
    IdPrimary,
    /// Ego (cloud) is primary when available, Id is fallback (new default)
    #[default]
    EgoPrimary,
}

fn default_schema_version() -> u32 {
    CONFIG_SCHEMA_VERSION
}

/// MCP server definition for Model Context Protocol integration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerDefinition {
    /// Unique id (e.g. "filesystem", "github").
    pub id: String,
    /// Human-readable name.
    pub name: String,
    /// Transport: "stdio" (subprocess) or "http".
    #[serde(default = "default_mcp_transport")]
    pub transport: String,
    /// For stdio: command line (e.g. "npx", "-y", "mcp-server-foo"). For http: base URL (e.g. "http://localhost:3000/mcp").
    pub command_or_url: String,
    /// Optional env vars for stdio (e.g. API keys). Keys are secret names; values are not stored in plaintext in config.
    #[serde(default)]
    pub env: std::collections::HashMap<String, String>,
}

fn default_mcp_transport() -> String {
    "http".to_string()
}

/// Trust policy for MCP servers (e.g. which domains are allowed for HTTP).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct McpTrustPolicy {
    /// If true, only servers in the configured list are allowed; no ad-hoc URLs.
    #[serde(default)]
    pub allow_list_only: bool,
    /// For HTTP transport: allowed hostnames (e.g. "localhost", "127.0.0.1"). Empty means no HTTP allowed or use default localhost.
    #[serde(default)]
    pub allowed_http_hosts: Vec<String>,
}

/// Trinity configuration: maps providers to Superego/Ego/Id roles.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TrinityConfig {
    /// Local LLM URL for Id
    #[serde(default)]
    pub id_url: Option<String>,
    /// Cloud provider name for Ego (e.g. "openai", "anthropic")
    #[serde(default)]
    pub ego_provider: Option<String>,
    /// API key for Ego provider
    #[serde(default)]
    pub ego_api_key: Option<String>,
    /// Cloud provider name for Superego (e.g. "anthropic", "openai")
    #[serde(default)]
    pub superego_provider: Option<String>,
    /// API key for Superego provider
    #[serde(default)]
    pub superego_api_key: Option<String>,
}

/// Central application configuration for an Orion agent, loaded from `config.json`.
///
/// Contains paths, LLM settings, routing mode, email accounts, MCP servers, and
/// birth-stage tracking. Serialized/deserialized with schema versioning for
/// forward-compatible migrations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    /// Schema version for config migration
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,

    pub data_dir: PathBuf,
    pub models_dir: PathBuf,
    pub docs_dir: PathBuf,
    pub db_path: PathBuf,

    /// OpenAI API key (optional - enables Ego)
    pub openai_api_key: Option<String>,

    /// Legacy single email config (migrated to email_accounts in v5).
    pub email: Option<EmailConfig>,

    /// Multi-account email config (provider-aware). Preferred over email.
    #[serde(default)]
    pub email_accounts: Vec<EmailAccountConfig>,

    /// Whether birth sequence has completed
    pub birth_complete: bool,

    /// Current birth stage if birth is in progress (for diagnostics and recovery)
    /// Values: "Darkness", "Ignition", "Connectivity", "Genesis", "Emergence"
    #[serde(default)]
    pub birth_stage: Option<String>,

    /// Path to external public key file for signature verification.
    /// This file should be outside Abigail's data directory and read-only.
    /// If None, falls back to internal keyring (legacy/dev mode).
    #[serde(default)]
    pub external_pubkey_path: Option<PathBuf>,

    /// Base URL for local LLM (LiteLLM/Ollama/etc), e.g. "http://localhost:1234".
    /// If None, uses in-process Candle stub.
    #[serde(default)]
    pub local_llm_base_url: Option<String>,

    /// Routing mode: ego_primary (default) or id_primary
    #[serde(default)]
    pub routing_mode: RoutingMode,

    /// Trinity configuration: Superego/Ego/Id provider mapping
    #[serde(default)]
    pub trinity: Option<TrinityConfig>,

    /// Agent's chosen name (set during Genesis)
    #[serde(default)]
    pub agent_name: Option<String>,

    /// Timestamp when birth was completed (ISO 8601 format)
    #[serde(default)]
    pub birth_timestamp: Option<String>,

    /// MCP servers to connect (Model Context Protocol).
    #[serde(default)]
    pub mcp_servers: Vec<McpServerDefinition>,

    /// Trust policy for MCP (allowed hosts, allow-list-only).
    #[serde(default)]
    pub mcp_trust_policy: McpTrustPolicy,

    /// Skill IDs that are approved for execution. If non-empty, only these skills may run; if empty, all registered skills are allowed (backward compat).
    #[serde(default)]
    pub approved_skill_ids: Vec<String>,

    /// Trusted signer public keys (base64 Ed25519) for signed skill packages. Optional.
    #[serde(default)]
    pub trusted_skill_signers: Vec<String>,

    /// SAO orchestrator endpoint (e.g. "http://localhost:3030").
    /// When set, Abigail will register with SAO on startup and send
    /// periodic status heartbeats. When None, Abigail runs standalone.
    #[serde(default)]
    pub sao_endpoint: Option<String>,

    /// Memory store backend: sqlite (default) or postgres. Overridable via MEMORY_BACKEND env.
    #[serde(default)]
    pub memory_backend: MemoryBackend,

    /// PostgreSQL connection URL when memory_backend is postgres. Overridable via DATABASE_URL env.
    #[serde(default)]
    pub database_url: Option<String>,

    /// Model name for birth stages (e.g. "qwen2.5:3b-instruct"). Overridable via BIRTH_MODEL env.
    #[serde(default)]
    pub birth_model: Option<String>,

    /// Default model for Id (non-birth) when local LLM is used. If unset, auto-detect or "local-model".
    #[serde(default)]
    pub id_model_default: Option<String>,
}

/// Auth mechanism for an email account.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EmailAuthType {
    /// OAuth2 (e.g. Gmail, Outlook) — tokens stored in vault.
    OAuth2,
    /// SMTP/IMAP token (e.g. Proton SMTP token).
    SmtpToken,
    /// App password for IMAP/SMTP (e.g. Yahoo, iCloud).
    AppPassword,
}

/// Provider identifier for routing and adapter selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EmailProvider {
    Gmail,
    Outlook,
    Proton,
    Fastmail,
    /// Generic IMAP/SMTP (Yahoo, iCloud, custom).
    ImapFallback,
}

/// Status of an email account (for reconnect/reauth UX).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum EmailAccountStatus {
    /// Ready for use.
    #[default]
    Active,
    /// Token expired or revoked; reauth needed.
    ReauthRequired,
    /// Disabled by user.
    Disabled,
}

/// Single email account with provider and auth metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmailAccountConfig {
    /// Unique id (e.g. uuid or "gmail_primary").
    pub id: String,
    /// Provider for adapter routing.
    pub provider: EmailProvider,
    /// Auth mechanism; determines how credentials are stored and refreshed.
    pub auth_type: EmailAuthType,
    /// Display address (e.g. user@gmail.com).
    pub address: String,
    /// IMAP host (optional for API-only providers).
    #[serde(default)]
    pub imap_host: Option<String>,
    /// IMAP port (default 993).
    #[serde(default)]
    pub imap_port: Option<u16>,
    /// SMTP host (optional for API-only providers).
    #[serde(default)]
    pub smtp_host: Option<String>,
    /// SMTP port (default 587).
    #[serde(default)]
    pub smtp_port: Option<u16>,
    /// OAuth2 scopes granted (for display/audit; tokens in vault).
    #[serde(default)]
    pub scopes_granted: Vec<String>,
    /// Account status (active, reauth_required, disabled).
    #[serde(default)]
    pub status: EmailAccountStatus,
    /// When the account was last verified successfully (ISO 8601).
    #[serde(default)]
    pub last_verified_at: Option<String>,
}

impl EmailAccountConfig {
    pub fn imap_port(&self) -> u16 {
        self.imap_port.unwrap_or(993)
    }
    pub fn smtp_port(&self) -> u16 {
        self.smtp_port.unwrap_or(587)
    }
}

/// Legacy single-account email config (migrated into email_accounts in v5).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmailConfig {
    pub address: String,
    pub imap_host: String,
    pub imap_port: u16,
    pub smtp_host: String,
    pub smtp_port: u16,
    /// Encrypted via DPAPI (or plaintext stub on non-Windows)
    pub password_encrypted: Vec<u8>,
}

impl AppConfig {
    pub fn default_paths() -> Self {
        let base = directories::ProjectDirs::from("com", "orion", "Orion")
            .map(|d| d.data_local_dir().to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."));

        Self {
            schema_version: CONFIG_SCHEMA_VERSION,
            data_dir: base.clone(),
            models_dir: base.join("models"),
            docs_dir: base.join("docs"),
            db_path: base.join("orion_seed.db"),
            openai_api_key: None,
            email: None,
            email_accounts: Vec::new(),
            birth_complete: false,
            birth_stage: None,
            external_pubkey_path: None,
            local_llm_base_url: None,
            routing_mode: RoutingMode::default(),
            trinity: None,
            agent_name: None,
            birth_timestamp: None,
            mcp_servers: Vec::new(),
            mcp_trust_policy: McpTrustPolicy::default(),
            approved_skill_ids: Vec::new(),
            trusted_skill_signers: Vec::new(),
            sao_endpoint: None,
            memory_backend: MemoryBackend::default(),
            database_url: None,
            birth_model: None,
            id_model_default: None,
        }
    }

    /// Apply environment variable overrides. Call after load() or default_paths() so env takes precedence.
    pub fn apply_env_overrides(&mut self) {
        if let Ok(v) = std::env::var("MEMORY_BACKEND") {
            self.memory_backend = v.parse().unwrap_or_default();
        }
        if let Ok(v) = std::env::var("DATABASE_URL") {
            let s = v.trim().to_string();
            if !s.is_empty() {
                self.database_url = Some(s);
            }
        }
        if let Ok(v) = std::env::var("BIRTH_MODEL") {
            let s = v.trim().to_string();
            if !s.is_empty() {
                self.birth_model = Some(s);
            }
        }
        if let Ok(v) = std::env::var("ID_MODEL_DEFAULT") {
            let s = v.trim().to_string();
            if !s.is_empty() {
                self.id_model_default = Some(s);
            }
        }
        if let Ok(v) = std::env::var("LOCAL_LLM_BASE_URL") {
            let s = v.trim().to_string();
            if !s.is_empty() {
                self.local_llm_base_url = Some(s);
            }
        }
    }

    /// Effective birth model name: config/value or default "qwen2.5:3b-instruct".
    pub fn effective_birth_model(&self) -> String {
        self.birth_model
            .as_deref()
            .filter(|s| !s.is_empty())
            .unwrap_or("qwen2.5:3b-instruct")
            .to_string()
    }

    /// Path to the config file (data_dir/config.json).
    pub fn config_path(&self) -> PathBuf {
        self.data_dir.join("config.json")
    }

    /// Returns the effective external pubkey path.
    ///
    /// Priority:
    /// 1. Explicitly configured `external_pubkey_path`
    /// 2. Auto-detected `{data_dir}/external_pubkey.bin` if it exists
    /// 3. None (dev mode - verification will be skipped)
    pub fn effective_external_pubkey_path(&self) -> Option<PathBuf> {
        // If explicitly configured, use that
        if self.external_pubkey_path.is_some() {
            return self.external_pubkey_path.clone();
        }

        // Auto-detect in data_dir
        let auto_path = self.data_dir.join("external_pubkey.bin");
        if auto_path.exists() {
            return Some(auto_path);
        }

        None
    }

    pub fn load(path: &PathBuf) -> anyhow::Result<Self> {
        let content = std::fs::read_to_string(path)?;
        let mut config: Self = serde_json::from_str(&content)?;

        // Auto-migrate if needed
        if config.migrate() {
            // Save migrated config back to disk
            config.save(path)?;
            tracing::info!(
                "Config migrated to schema version {}",
                config.schema_version
            );
        }

        config.apply_env_overrides();
        Ok(config)
    }

    pub fn save(&self, path: &PathBuf) -> anyhow::Result<()> {
        let content = serde_json::to_string_pretty(self)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, content)?;
        Ok(())
    }

    /// Migrate config from older schema versions to the current version.
    /// Returns true if any migration was performed.
    pub fn migrate(&mut self) -> bool {
        let mut migrated = false;

        // Migration from no schema_version (pre-v1) to v1
        if self.schema_version < 1 {
            // v1 adds: schema_version, birth_stage
            // birth_stage defaults to None via serde, so just update version
            self.schema_version = 1;
            migrated = true;
            tracing::debug!("Migrated config from pre-v1 to v1");
        }

        // Migration from v1 to v2
        if self.schema_version < 2 {
            // v2 adds: birth_timestamp
            self.schema_version = 2;
            migrated = true;
            tracing::debug!("Migrated config from v1 to v2");
        }

        // Migration from v2 to v3
        if self.schema_version < 3 {
            // v3 adds: mcp_servers, mcp_trust_policy
            self.schema_version = 3;
            migrated = true;
            tracing::debug!("Migrated config from v2 to v3");
        }

        // Migration from v3 to v4
        if self.schema_version < 4 {
            // v4 adds: approved_skill_ids, trusted_skill_signers, sao_endpoint
            self.schema_version = 4;
            migrated = true;
            tracing::debug!("Migrated config from v3 to v4");
        }

        // Migration from v4 to v5
        if self.schema_version < 5 {
            // v5 adds: email_accounts; migrate single email into first account if present
            if let Some(ref legacy) = self.email {
                self.email_accounts.push(EmailAccountConfig {
                    id: format!("legacy_{}", legacy.address.replace(['@', '.'], "_")),
                    provider: EmailProvider::ImapFallback,
                    auth_type: EmailAuthType::AppPassword,
                    address: legacy.address.clone(),
                    imap_host: Some(legacy.imap_host.clone()),
                    imap_port: Some(legacy.imap_port),
                    smtp_host: Some(legacy.smtp_host.clone()),
                    smtp_port: Some(legacy.smtp_port),
                    scopes_granted: Vec::new(),
                    status: EmailAccountStatus::Active,
                    last_verified_at: None,
                });
                // Keep legacy email in config for backward compat; vault/password stays in keyring
            }
            self.schema_version = 5;
            migrated = true;
            tracing::debug!("Migrated config from v4 to v5 (email_accounts)");
        }

        migrated
    }

    /// Check if birth was interrupted (birth_stage set but birth_complete is false).
    /// If so, reset birth_stage and return true to indicate restart is needed.
    pub fn check_interrupted_birth(&mut self) -> bool {
        if self.birth_stage.is_some() && !self.birth_complete {
            tracing::warn!(
                "Birth was interrupted at stage {:?}. Resetting for restart.",
                self.birth_stage
            );
            self.birth_stage = None;
            true
        } else {
            false
        }
    }

    /// Set the current birth stage (for persistence/diagnostics).
    pub fn set_birth_stage(&mut self, stage: &str) {
        self.birth_stage = Some(stage.to_string());
    }

    /// Clear the birth stage (called on completion or reset).
    pub fn clear_birth_stage(&mut self) {
        self.birth_stage = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn test_config(base: &std::path::Path) -> AppConfig {
        let data_dir = base.join("data");
        fs::create_dir_all(&data_dir).unwrap();
        AppConfig {
            schema_version: CONFIG_SCHEMA_VERSION,
            data_dir: data_dir.clone(),
            models_dir: data_dir.join("models"),
            docs_dir: data_dir.join("docs"),
            db_path: data_dir.join("test.db"),
            openai_api_key: None,
            email: None,
            email_accounts: Vec::new(),
            birth_complete: false,
            birth_stage: None,
            external_pubkey_path: None,
            local_llm_base_url: None,
            routing_mode: RoutingMode::default(),
            trinity: None,
            agent_name: None,
            birth_timestamp: None,
            mcp_servers: Vec::new(),
            mcp_trust_policy: McpTrustPolicy::default(),
            approved_skill_ids: Vec::new(),
            trusted_skill_signers: Vec::new(),
            sao_endpoint: None,
            memory_backend: MemoryBackend::default(),
            database_url: None,
            birth_model: None,
            id_model_default: None,
        }
    }

    #[test]
    fn test_migrate_from_pre_v1() {
        let mut config = AppConfig::default_paths();
        config.schema_version = 0; // Simulate pre-v1 config

        assert!(config.migrate());
        assert_eq!(config.schema_version, CONFIG_SCHEMA_VERSION);
    }

    #[test]
    fn test_no_migration_needed() {
        let mut config = AppConfig::default_paths();
        config.schema_version = CONFIG_SCHEMA_VERSION;

        assert!(!config.migrate());
        assert_eq!(config.schema_version, CONFIG_SCHEMA_VERSION);
    }

    #[test]
    fn test_load_legacy_config_without_schema_version() {
        let tmp = std::env::temp_dir().join("orion_config_legacy_load");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();

        let config_path = tmp.join("config.json");
        // Write a config without schema_version (simulates legacy config)
        let legacy_json = r#"{
            "data_dir": ".",
            "models_dir": "./models",
            "docs_dir": "./docs",
            "db_path": "./test.db",
            "openai_api_key": null,
            "email": null,
            "birth_complete": false,
            "routing_mode": "ego_primary"
        }"#;
        fs::write(&config_path, legacy_json).unwrap();

        // Load should auto-migrate
        let config = AppConfig::load(&config_path).unwrap();
        assert_eq!(config.schema_version, CONFIG_SCHEMA_VERSION);
        assert!(config.birth_stage.is_none());

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_check_interrupted_birth_not_interrupted() {
        let tmp = std::env::temp_dir().join("orion_config_no_interrupt");
        let _ = fs::remove_dir_all(&tmp);

        let mut config = test_config(&tmp);
        config.birth_stage = None;
        config.birth_complete = false;

        assert!(!config.check_interrupted_birth());

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_check_interrupted_birth_was_interrupted() {
        let tmp = std::env::temp_dir().join("orion_config_interrupted");
        let _ = fs::remove_dir_all(&tmp);

        let mut config = test_config(&tmp);
        config.birth_stage = Some("Ignition".to_string());
        config.birth_complete = false;

        assert!(config.check_interrupted_birth());
        assert!(config.birth_stage.is_none()); // Should be cleared

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_check_interrupted_birth_completed() {
        let tmp = std::env::temp_dir().join("orion_config_completed");
        let _ = fs::remove_dir_all(&tmp);

        let mut config = test_config(&tmp);
        config.birth_stage = Some("Emergence".to_string()); // Shouldn't happen, but test edge case
        config.birth_complete = true;

        // If birth is complete, it's not interrupted even if stage is set
        assert!(!config.check_interrupted_birth());

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_birth_stage_helpers() {
        let mut config = AppConfig::default_paths();

        assert!(config.birth_stage.is_none());

        config.set_birth_stage("Genesis");
        assert_eq!(config.birth_stage, Some("Genesis".to_string()));

        config.clear_birth_stage();
        assert!(config.birth_stage.is_none());
    }

    #[test]
    fn test_effective_birth_model() {
        let mut config = AppConfig::default_paths();
        assert_eq!(config.effective_birth_model(), "qwen2.5:3b-instruct");
        config.birth_model = Some("llama3.2:3b".to_string());
        assert_eq!(config.effective_birth_model(), "llama3.2:3b");
    }

    #[test]
    fn test_memory_backend_from_str() {
        assert_eq!(
            "postgres".parse::<MemoryBackend>().unwrap(),
            MemoryBackend::Postgres
        );
        assert_eq!(
            "sqlite".parse::<MemoryBackend>().unwrap(),
            MemoryBackend::Sqlite
        );
        assert_eq!("".parse::<MemoryBackend>().unwrap(), MemoryBackend::Sqlite);
        assert_eq!(
            "PostgreSQL".parse::<MemoryBackend>().unwrap(),
            MemoryBackend::Postgres
        );
    }

    #[test]
    fn test_migrate_v4_to_v5_email_accounts() {
        use super::{EmailAccountStatus, EmailAuthType, EmailConfig, EmailProvider};

        let mut config = AppConfig::default_paths();
        config.schema_version = 4;
        config.email = Some(EmailConfig {
            address: "user@proton.me".to_string(),
            imap_host: "mail.proton.me".to_string(),
            imap_port: 993,
            smtp_host: "mail.proton.me".to_string(),
            smtp_port: 587,
            password_encrypted: vec![],
        });

        assert!(config.migrate());
        assert_eq!(config.schema_version, 5);
        assert_eq!(config.email_accounts.len(), 1);
        let acc = &config.email_accounts[0];
        assert_eq!(acc.address, "user@proton.me");
        assert_eq!(acc.provider, EmailProvider::ImapFallback);
        assert_eq!(acc.auth_type, EmailAuthType::AppPassword);
        assert_eq!(acc.status, EmailAccountStatus::Active);
        assert_eq!(acc.imap_host.as_deref(), Some("mail.proton.me"));
        assert_eq!(acc.imap_port, Some(993));
    }
}
