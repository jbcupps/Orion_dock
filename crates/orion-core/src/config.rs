use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::str::FromStr;

/// Current config schema version. Increment when making breaking changes.
pub const CONFIG_SCHEMA_VERSION: u32 = 8;

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

/// Thinking tier used by Fast/Standard/Pro UI modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThinkingModelTier {
    Fast,
    Standard,
    Pro,
}

impl ThinkingModelTier {
    /// Convert API/UI router mode values to tier values.
    /// Accepts both display names (fast/standard/pro) and legacy values
    /// (auto/think_hard/think_harder).
    pub fn from_mode(mode: &str) -> Self {
        match mode.trim().to_lowercase().as_str() {
            "pro" | "think_harder" => ThinkingModelTier::Pro,
            "standard" | "think_hard" => ThinkingModelTier::Standard,
            _ => ThinkingModelTier::Fast,
        }
    }
}

/// Per-provider model mapping for thinking tiers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TierModels {
    pub fast: String,
    pub standard: String,
    pub pro: String,
}

impl TierModels {
    pub fn for_tier(&self, tier: ThinkingModelTier) -> &str {
        match tier {
            ThinkingModelTier::Fast => &self.fast,
            ThinkingModelTier::Standard => &self.standard,
            ThinkingModelTier::Pro => &self.pro,
        }
    }
}

/// Cached provider model catalog entry with lifecycle metadata.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProviderCatalogEntry {
    /// Known available model IDs from the provider.
    #[serde(default)]
    pub available_models: Vec<String>,
    /// How the catalog was obtained: "api", "curated", or "unknown".
    #[serde(default = "default_catalog_source")]
    pub source: String,
    /// ISO 8601 timestamp of last successful refresh.
    #[serde(default)]
    pub last_refreshed: Option<String>,
    /// Deprecation or lifecycle warnings (e.g. "gemini-2.0-flash retiring March 2026").
    #[serde(default)]
    pub warnings: Vec<String>,
    /// Whether the currently selected tier models have been validated against the catalog.
    #[serde(default)]
    pub validated: bool,
}

fn default_catalog_source() -> String {
    "unknown".to_string()
}

/// Well-known curated model catalogs per provider (fallback when API listing is unavailable).
pub fn curated_provider_models(provider: &str) -> Vec<String> {
    match AppConfig::normalize_provider_name(provider).as_str() {
        "openai" => vec![
            "gpt-4.1-nano",
            "gpt-4.1-mini",
            "gpt-4.1",
            "gpt-4o-mini",
            "gpt-4o",
            "o3-mini",
            "o3",
            "o1",
        ],
        "anthropic" => vec![
            "claude-haiku-4-5-20251001",
            "claude-sonnet-4-5-20250929",
            "claude-opus-4-6",
        ],
        "perplexity" => vec![
            "sonar",
            "sonar-pro",
            "sonar-deep-research",
            "sonar-reasoning-pro",
        ],
        "xai" => vec![
            "grok-4-1-fast-reasoning",
            "grok-4-fast-reasoning",
            "grok-4",
            "grok-3",
            "grok-2-latest",
        ],
        "google" => vec![
            "gemini-3-pro-preview",
            "gemini-3-flash-preview",
            "gemini-2.5-pro",
            "gemini-2.5-flash",
        ],
        _ => vec![],
    }
    .into_iter()
    .map(String::from)
    .collect()
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

/// Default hostnames always permitted for MCP connections (Docker-internal).
const MCP_DEFAULT_ALLOWED_HOSTS: &[&str] = &[
    "localhost",
    "127.0.0.1",
    "orion-toolbox",
    "host.docker.internal",
];

/// Cloud metadata IP ranges that must never be contacted.
const MCP_BLOCKED_HOSTS: &[&str] = &["169.254.169.254", "metadata.google.internal"];

impl McpTrustPolicy {
    /// Validate an MCP server URL against this trust policy.
    ///
    /// Returns `Ok(())` if the URL is allowed, or an error message describing
    /// why the URL was rejected.
    pub fn validate_url(&self, url: &str) -> Result<(), String> {
        // Parse URL to extract host.
        let parsed = url::Url::parse(url).map_err(|e| format!("invalid URL: {}", e))?;
        let host = parsed
            .host_str()
            .ok_or_else(|| "URL has no host".to_string())?;

        // Block dangerous metadata endpoints unconditionally.
        for blocked in MCP_BLOCKED_HOSTS {
            if host == *blocked {
                return Err(format!("host '{}' is blocked (cloud metadata endpoint)", host));
            }
        }

        // If allow_list_only, the host must appear in allowed_http_hosts.
        if self.allow_list_only {
            if !self.allowed_http_hosts.iter().any(|h| h == host) {
                return Err(format!(
                    "host '{}' not in allowed list (allow_list_only is enabled)",
                    host
                ));
            }
            return Ok(());
        }

        // Otherwise, allow defaults + any configured hosts.
        let in_defaults = MCP_DEFAULT_ALLOWED_HOSTS.contains(&host);
        let in_configured = self.allowed_http_hosts.iter().any(|h| h == host);
        if in_defaults || in_configured {
            return Ok(());
        }

        Err(format!(
            "host '{}' is not in the default or configured allowed hosts. \
             Add it to mcp_trust_policy.allowed_http_hosts in agent config.",
            host
        ))
    }
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
    /// API key for Ego provider (legacy — migrated to vault in v8, never serialized)
    #[serde(default, skip_serializing)]
    pub ego_api_key: Option<String>,
    /// Cloud provider name for Superego (e.g. "anthropic", "openai")
    #[serde(default)]
    pub superego_provider: Option<String>,
    /// API key for Superego provider (legacy — migrated to vault in v8, never serialized)
    #[serde(default, skip_serializing)]
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

    /// Unique agent identifier (UUID v4). Used to scope Postgres birth/memory tables.
    /// If None (legacy configs), derived from data_dir path.
    #[serde(default)]
    pub agent_id: Option<String>,

    pub data_dir: PathBuf,
    pub models_dir: PathBuf,
    pub docs_dir: PathBuf,
    pub db_path: PathBuf,

    /// OpenAI API key (legacy — migrated to vault in v8, never serialized)
    #[serde(default, skip_serializing)]
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

    /// Per-provider model mapping for Fast/Standard/Pro thinking tiers.
    /// Key is provider name (openai, anthropic, perplexity, xai, google).
    #[serde(default)]
    pub tier_models: HashMap<String, TierModels>,

    /// Preferred Ego provider for routing (overrides automatic preference order).
    /// When set, this provider is tried first if its key is available.
    #[serde(default)]
    pub active_provider_preference: Option<String>,

    /// Cached provider model catalogs (keyed by normalized provider name).
    #[serde(default)]
    pub provider_catalog: HashMap<String, ProviderCatalogEntry>,
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
            agent_id: None,
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
            tier_models: HashMap::new(),
            active_provider_preference: None,
            provider_catalog: HashMap::new(),
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

    /// Normalize provider aliases to canonical names used in tier model maps.
    pub fn normalize_provider_name(provider: &str) -> String {
        match provider.trim().to_lowercase().as_str() {
            "pplx" => "perplexity".to_string(),
            "grok" => "xai".to_string(),
            "gemini" => "google".to_string(),
            other => other.to_string(),
        }
    }

    /// Built-in defaults for provider tier models.
    pub fn default_tier_models(provider: &str) -> TierModels {
        match Self::normalize_provider_name(provider).as_str() {
            "anthropic" => TierModels {
                fast: "claude-haiku-4-5-20251001".to_string(),
                standard: "claude-sonnet-4-5-20250929".to_string(),
                pro: "claude-opus-4-6".to_string(),
            },
            "perplexity" => TierModels {
                fast: "sonar".to_string(),
                standard: "sonar-pro".to_string(),
                pro: "sonar-reasoning-pro".to_string(),
            },
            "xai" => TierModels {
                fast: "grok-2-latest".to_string(),
                standard: "grok-4-fast-reasoning".to_string(),
                pro: "grok-4".to_string(),
            },
            "google" => TierModels {
                fast: "gemini-3-flash-preview".to_string(),
                standard: "gemini-2.5-pro".to_string(),
                pro: "gemini-3-pro-preview".to_string(),
            },
            // openai + fallback
            _ => TierModels {
                fast: "gpt-4.1-mini".to_string(),
                standard: "gpt-4.1".to_string(),
                pro: "o3".to_string(),
            },
        }
    }

    /// Effective tier mappings for a provider: explicit config override or built-in defaults.
    pub fn effective_tier_models(&self, provider: &str) -> TierModels {
        let normalized = Self::normalize_provider_name(provider);
        self.tier_models
            .get(&normalized)
            .cloned()
            .unwrap_or_else(|| Self::default_tier_models(&normalized))
    }

    /// Effective model for provider and tier.
    pub fn effective_tier_model(&self, provider: &str, tier: ThinkingModelTier) -> String {
        self.effective_tier_models(provider)
            .for_tier(tier)
            .to_string()
    }

    /// Path to the config file (data_dir/config.json).
    pub fn config_path(&self) -> PathBuf {
        self.data_dir.join("config.json")
    }

    /// Effective agent ID. Uses explicit agent_id if set, otherwise derives from data_dir path.
    pub fn effective_agent_id(&self) -> String {
        if let Some(ref id) = self.agent_id {
            return id.clone();
        }
        self.data_dir
            .file_name()
            .and_then(|f| f.to_str())
            .map(String::from)
            .unwrap_or_else(|| "default".to_string())
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

        // Migration from v5 to v6
        if self.schema_version < 6 {
            // v6 adds: tier_models
            self.schema_version = 6;
            migrated = true;
            tracing::debug!("Migrated config from v5 to v6 (tier_models)");
        }

        // Migration from v6 to v7
        if self.schema_version < 7 {
            // v7 adds: active_provider_preference, provider_catalog
            self.schema_version = 7;
            migrated = true;
            tracing::debug!(
                "Migrated config from v6 to v7 (provider_catalog, active_provider_preference)"
            );
        }

        // Migration from v7 to v8
        if self.schema_version < 8 {
            // v8: API keys removed from config.json (skip_serializing).
            // Keys are migrated to vault via migrate_keys_to_vault() at startup.
            self.schema_version = 8;
            migrated = true;
            tracing::debug!(
                "Migrated config from v7 to v8 (secrets moved to vault, keys removed from config)"
            );
        }

        migrated
    }

    /// Migrate any legacy plaintext API keys from config fields into the vault.
    /// Returns a list of providers that were migrated. Call `save()` after this
    /// to persist the config without the key fields.
    pub fn migrate_keys_to_vault(
        &mut self,
        vault: &mut crate::SecretsVault,
    ) -> Vec<String> {
        let mut migrated = Vec::new();

        // openai_api_key → provider:openai
        if let Some(ref key) = self.openai_api_key {
            if !key.is_empty() && vault.get_secret("provider:openai").is_none() {
                vault.set_secret("provider:openai", key);
                migrated.push("openai".to_string());
                tracing::info!("Migrated openai_api_key from config to vault");
            }
        }
        self.openai_api_key = None;

        // trinity.ego_api_key → provider:{ego_provider}
        if let Some(ref trinity) = self.trinity.clone() {
            if let Some(ref key) = trinity.ego_api_key {
                if !key.is_empty() {
                    let provider = trinity
                        .ego_provider
                        .as_deref()
                        .unwrap_or("openai");
                    let ns_key = format!("provider:{}", provider);
                    if vault.get_secret(&ns_key).is_none() {
                        vault.set_secret(&ns_key, key);
                        migrated.push(provider.to_string());
                        tracing::info!("Migrated ego_api_key from config to vault ({})", provider);
                    }
                }
            }
            if let Some(ref key) = trinity.superego_api_key {
                if !key.is_empty() {
                    let provider = trinity
                        .superego_provider
                        .as_deref()
                        .unwrap_or("anthropic");
                    let ns_key = format!("provider:{}", provider);
                    if vault.get_secret(&ns_key).is_none() {
                        vault.set_secret(&ns_key, key);
                        migrated.push(provider.to_string());
                        tracing::info!(
                            "Migrated superego_api_key from config to vault ({})",
                            provider
                        );
                    }
                }
            }
        }
        if let Some(ref mut trinity) = self.trinity {
            trinity.ego_api_key = None;
            trinity.superego_api_key = None;
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
            agent_id: None,
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
            tier_models: HashMap::new(),
            active_provider_preference: None,
            provider_catalog: HashMap::new(),
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
    fn test_effective_tier_models_defaults() {
        let config = AppConfig::default_paths();
        let anthropic = config.effective_tier_models("anthropic");
        assert_eq!(anthropic.fast, "claude-haiku-4-5-20251001");
        assert_eq!(anthropic.standard, "claude-sonnet-4-5-20250929");
        assert_eq!(anthropic.pro, "claude-opus-4-6");

        let openai = config.effective_tier_models("openai");
        assert_eq!(openai.fast, "gpt-4.1-mini");
        assert_eq!(openai.standard, "gpt-4.1");
        assert_eq!(openai.pro, "o3");
    }

    #[test]
    fn test_effective_tier_models_override() {
        let mut config = AppConfig::default_paths();
        config.tier_models.insert(
            "openai".to_string(),
            TierModels {
                fast: "a".to_string(),
                standard: "b".to_string(),
                pro: "c".to_string(),
            },
        );
        assert_eq!(
            config.effective_tier_model("openai", ThinkingModelTier::Fast),
            "a"
        );
        assert_eq!(
            config.effective_tier_model("openai", ThinkingModelTier::Standard),
            "b"
        );
        assert_eq!(
            config.effective_tier_model("openai", ThinkingModelTier::Pro),
            "c"
        );
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
        assert_eq!(config.schema_version, CONFIG_SCHEMA_VERSION);
        assert_eq!(config.email_accounts.len(), 1);
        let acc = &config.email_accounts[0];
        assert_eq!(acc.address, "user@proton.me");
        assert_eq!(acc.provider, EmailProvider::ImapFallback);
        assert_eq!(acc.auth_type, EmailAuthType::AppPassword);
        assert_eq!(acc.status, EmailAccountStatus::Active);
        assert_eq!(acc.imap_host.as_deref(), Some("mail.proton.me"));
        assert_eq!(acc.imap_port, Some(993));
    }

    // ── MCP trust policy URL validation ─────────────────────────────────

    #[test]
    fn test_mcp_trust_default_allows_localhost() {
        let policy = McpTrustPolicy::default();
        assert!(policy.validate_url("http://localhost:9090/mcp").is_ok());
        assert!(policy.validate_url("http://127.0.0.1:8080").is_ok());
        assert!(policy
            .validate_url("http://orion-toolbox:9090/mcp")
            .is_ok());
        assert!(policy
            .validate_url("http://host.docker.internal:3000")
            .is_ok());
    }

    #[test]
    fn test_mcp_trust_blocks_metadata_endpoints() {
        let policy = McpTrustPolicy::default();
        assert!(policy
            .validate_url("http://169.254.169.254/latest/meta-data")
            .is_err());
        assert!(policy
            .validate_url("http://metadata.google.internal/v1")
            .is_err());
    }

    #[test]
    fn test_mcp_trust_rejects_unknown_hosts() {
        let policy = McpTrustPolicy::default();
        assert!(policy.validate_url("http://evil.example.com/mcp").is_err());
    }

    #[test]
    fn test_mcp_trust_allow_list_only() {
        let policy = McpTrustPolicy {
            allow_list_only: true,
            allowed_http_hosts: vec!["myserver.local".to_string()],
        };
        assert!(policy.validate_url("http://myserver.local:8080").is_ok());
        assert!(policy.validate_url("http://localhost:9090").is_err());
    }

    #[test]
    fn test_mcp_trust_configured_hosts() {
        let policy = McpTrustPolicy {
            allow_list_only: false,
            allowed_http_hosts: vec!["custom.internal".to_string()],
        };
        // Configured host allowed
        assert!(policy.validate_url("http://custom.internal:5000").is_ok());
        // Default hosts still allowed
        assert!(policy.validate_url("http://localhost:9090").is_ok());
        // Unknown still rejected
        assert!(policy.validate_url("http://other.example.com").is_err());
    }

    #[test]
    fn test_mcp_trust_rejects_invalid_urls() {
        let policy = McpTrustPolicy::default();
        assert!(policy.validate_url("not-a-url").is_err());
        assert!(policy.validate_url("").is_err());
    }
}
