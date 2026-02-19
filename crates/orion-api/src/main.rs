mod agentic;
mod chat_attachments;
mod error;
mod governed_chat;
mod orchestration;
mod routes;
pub(crate) mod tool_extraction;

pub(crate) use error::ApiError;

use axum::http::{header, HeaderValue, Method};
use axum::{
    extract::{DefaultBodyLimit, Multipart, Path, Query, State},
    response::sse::{Event, KeepAlive, Sse},
    routing::{get, post},
    Json, Router,
};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use genesis_core::{
    ChatMessage as GenesisChatMessage, GenesisContext, GenesisRegistry, GenesisStrategy,
    StepRequest, StepResponse,
};
use orion_birth::{
    birth_chat_turn, build_birth_messages, build_birth_router, build_genesis_messages,
    detect_provider_from_key, execute_store_provider_key, extract_api_keys_from_text,
    redact_api_keys, BirthToolRequest, GenesisPath, SoulCrystallizationDepth,
};
use orion_core::system_prompt::{
    build_system_prompt, build_system_prompt_with_skills, RuntimeContext, SkillToolEntry,
};
use orion_core::templates::{fill_soul_template, GROWTH_MD};
use orion_core::{
    curated_provider_models, validate_local_llm_url, AgentEntry, AppConfig, GlobalConfig,
    MemoryBackend, ProviderCatalogEntry, ProviderKeyring, RoutingMode, SkillKeychain,
    ThinkingModelTier, TierModels, CONFIG_SCHEMA_VERSION,
};
use orion_skills::manifest::TrustTier;
use orion_skills::skill::Skill;
use orion_skills::{SkillExecutor, SkillRegistry};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::{Arc, Mutex, RwLock};
use tokio::sync::Mutex as TokioMutex;
use tower_http::cors::{Any, CorsLayer};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
use uuid::Uuid;

use agentic::{
    AgenticEvent, AgenticLoopConfig, AgenticRunRequest, AgenticRunResponse, AgenticTask,
    AgenticTaskStatus,
};
use chat_attachments::{
    build_attachment_context_block, build_attachment_storage_note, has_attachments,
    load_attachment_context, load_image_attachments, normalize_attachment_ids,
    should_block_tool_on_attachment_turn, store_uploaded_attachments, ChatAttachmentUploadResponse,
    UploadedAttachmentInput,
};
use orchestration::{
    append_log_entry, assess_significance, build_id_check_prompt, create_job, decide_action,
    delete_job, is_job_due, list_jobs, load_logs, make_mentor_attention_message, save_jobs,
    set_job_enabled, update_job, update_job_after_execution, CreateOrchestrationJobRequest,
    JobLogsQuery, JobRunNowResponse, OrchestrationDecision, OrchestrationJob,
    OrchestrationJobLogEntry, OrchestrationJobsResponse, OrchestrationLogsResponse,
    SetOrchestrationJobEnabledRequest, SignificanceLevel, UpdateOrchestrationJobRequest,
};

#[derive(Clone)]
pub(crate) struct AppState {
    pub(crate) memory_backend: MemoryBackend,
    pub(crate) local_llm_base_url: Option<String>,
    pub(crate) birth_model: Option<String>,
    /// Ed25519 signing key bytes held per agent (from Darkness to Emergence).
    /// Keys are stored here after generation and retrieved at Emergence for document signing.
    pub(crate) birth_keys: Arc<Mutex<HashMap<String, Vec<u8>>>>,
    /// Central skill registry with trust-tiered skills.
    pub(crate) skill_registry: Arc<SkillRegistry>,
    /// Skill executor for running tools with sandbox enforcement.
    pub(crate) skill_executor: Arc<SkillExecutor>,
    /// Active agentic tasks keyed by task_id.
    pub(crate) agentic_tasks: Arc<TokioMutex<HashMap<String, Arc<TokioMutex<AgenticTask>>>>>,
    /// Cached provider keyring — LLM API keys, read-heavy, write-rare.
    pub(crate) provider_keyring: Arc<RwLock<ProviderKeyring>>,
    /// Shared skill keychain for skill plugins — non-provider secrets.
    pub(crate) skill_keychain: Arc<Mutex<SkillKeychain>>,
    /// Shared email account list for the email skill — synced per-request from agent config.
    pub(crate) email_accounts:
        Arc<tokio::sync::RwLock<Vec<orion_core::config::EmailAccountConfig>>>,
    /// Scheduler interval for orchestration periodic jobs.
    pub(crate) orchestration_tick_seconds: u64,
    /// Pending skill execution confirmation nonces: nonce → (skill_id, tool_name, created_at).
    #[allow(clippy::type_complexity)]
    pub(crate) skill_confirm_nonces:
        Arc<TokioMutex<HashMap<String, (String, String, std::time::Instant)>>>,
    /// Pending high-impact operational chat actions that require mentor confirmation.
    pub(crate) pending_operational_actions:
        Arc<TokioMutex<HashMap<String, PendingOperationalAction>>>,
    /// Superego L2 behavior mode.
    pub(crate) superego_l2_mode: orion_router::SuperegoL2Mode,
    /// Genesis path registry — replaces hardcoded GenesisPath::all_paths().
    pub(crate) genesis_registry: Arc<GenesisRegistry>,
    /// Active genesis sessions per agent — unified step protocol.
    pub(crate) genesis_sessions: Arc<TokioMutex<HashMap<String, Box<dyn GenesisStrategy>>>>,
}

pub(crate) fn data_root() -> Option<PathBuf> {
    std::env::var("ORION_DATA_DIR")
        .ok()
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
}

pub(crate) fn current_agent_path() -> PathBuf {
    data_root()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("current_agent")
}

/// Resolve which config to read: Hive current agent, or legacy data_dir/config.json.
pub(crate) fn resolve_config_path() -> Option<PathBuf> {
    let root = data_root()?;
    let current_path = root.join("current_agent");
    if let Ok(id) = std::fs::read_to_string(&current_path) {
        let id = id.trim();
        if !id.is_empty() {
            let agent_config = root.join("identities").join(id).join("config.json");
            if agent_config.exists() {
                return Some(agent_config);
            }
        }
    }
    let legacy = root.join("config.json");
    if legacy.exists() {
        return Some(legacy);
    }
    None
}

pub(crate) fn agent_config_path(id: &str) -> Option<PathBuf> {
    let root = data_root()?;
    let path = root.join("identities").join(id).join("config.json");
    if path.exists() {
        Some(path)
    } else {
        None
    }
}

pub(crate) fn agent_dir(id: &str) -> Option<PathBuf> {
    data_root().map(|root| root.join("identities").join(id))
}

pub(crate) fn resolve_birth_timestamp(
    config: &AppConfig,
    agent_root: &std::path::Path,
) -> Option<String> {
    if let Some(ts) = config.birth_timestamp.clone() {
        return Some(ts);
    }
    if !config.birth_complete {
        return None;
    }

    let soul_path = agent_root.join("docs").join("soul.md");
    let modified = std::fs::metadata(soul_path).ok()?.modified().ok()?;
    Some(chrono::DateTime::<chrono::Utc>::from(modified).to_rfc3339())
}

/// Persist genesis path for an agent so it can be restored when loading orchestrator.
fn persist_genesis_path(id: &str, path: &GenesisPath) -> Result<(), std::io::Error> {
    let dir = agent_dir(id)
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "no data root"))?;
    let depth = match path {
        GenesisPath::SoulCrystallization { depth } => Some(depth.as_str().to_string()),
        _ => None,
    };
    let value = serde_json::json!({ "path": path.id(), "depth": depth });
    std::fs::write(
        dir.join("genesis_path.json"),
        serde_json::to_string_pretty(&value).unwrap(),
    )
}

/// Build the genesis registry with all available paths.
fn build_genesis_registry() -> GenesisRegistry {
    let mut registry = GenesisRegistry::new();
    registry.register("quick_start", || {
        Box::new(genesis_quick_start::QuickStartStrategy::new())
    });
    registry.register("direct_discovery", || {
        Box::new(genesis_direct_discovery::DirectDiscoveryStrategy::new())
    });
    registry.register("soul_crystallization", || {
        Box::new(genesis_soul_crystallization::SoulCrystallizationStrategy::new())
    });
    registry.register("soul_forge", || {
        Box::new(genesis_soul_forge::SoulForgeStrategy::new())
    });
    registry
}

/// ChatFunction adapter that routes through the birth router for LLM access.
struct BirthRouterChatFunction {
    config: AppConfig,
}

#[async_trait::async_trait]
impl genesis_core::ChatFunction for BirthRouterChatFunction {
    async fn chat(
        &self,
        messages: Vec<GenesisChatMessage>,
    ) -> Result<String, genesis_core::GenesisError> {
        let router = build_birth_router(&self.config).await;
        let conversation: Vec<(String, String)> = messages
            .iter()
            .map(|m| (m.role.clone(), m.content.clone()))
            .collect();
        let birth_messages = build_genesis_messages(&conversation);
        let response = birth_chat_turn(&router, birth_messages)
            .await
            .map_err(|e| genesis_core::GenesisError::Other(e.to_string()))?;
        Ok(response.assistant_content)
    }
}

/// Read birth_complete, birth_stage, and agent_name from config.json (no migrations).
fn read_birth_status() -> (bool, Option<String>, Option<String>) {
    let path = match resolve_config_path() {
        Some(p) => p,
        None => return (false, None, None),
    };
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return (false, None, None),
    };
    let value: serde_json::Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(_) => return (false, None, None),
    };
    let birth_complete = value
        .get("birth_complete")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let birth_stage = value
        .get("birth_stage")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let agent_name = value
        .get("agent_name")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    (birth_complete, birth_stage, agent_name)
}

#[derive(Serialize)]
struct HealthResponse {
    status: &'static str,
}

#[derive(Serialize)]
struct StatusResponse {
    memory_backend: String,
    local_llm_configured: bool,
    birth_model: Option<String>,
    /// When local LLM is configured and birth_model is set: true if model is available on Ollama, false otherwise.
    #[serde(skip_serializing_if = "Option::is_none")]
    birth_model_ready: Option<bool>,
    birth_complete: bool,
    birth_stage: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    agent_name: Option<String>,
}

#[derive(Serialize)]
struct AgentIdentityInfo {
    id: String,
    name: String,
    directory: String,
    birth_complete: bool,
    birth_date: Option<String>,
}

#[derive(Serialize)]
struct IdentitiesResponse {
    agents: Vec<AgentIdentityInfo>,
    mentor_name: Option<String>,
}

#[derive(Deserialize)]
struct CreateAgentRequest {
    name: String,
    /// If true, skip interactive birth and auto-generate standard documents from agent name.
    #[serde(default)]
    quick_start: bool,
}

#[derive(Serialize)]
struct GenesisPathListItem {
    id: String,
    label: String,
    description: String,
    estimated_time: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    depth: Option<String>,
}

#[derive(Deserialize)]
struct GenesisStartRequest {
    path: String,
    #[serde(default)]
    depth: Option<String>,
    #[serde(default)]
    mentor_name: Option<String>,
}

#[derive(Serialize)]
struct BirthStateResponse {
    stage: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    private_key_base64: Option<String>,
}

#[derive(Deserialize)]
struct IgnitionRequest {
    #[serde(default)]
    local_llm_base_url: Option<String>,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct BirthChatMessage {
    pub(crate) role: String,
    pub(crate) content: String,
}

#[derive(Deserialize)]
struct BirthChatRequest {
    message: String,
}

#[derive(Serialize)]
struct BirthChatResponseBody {
    assistant_content: String,
    tool_requests: Vec<BirthToolRequest>,
    crystallized: bool,
}

// ---- Connectivity API types ----

#[derive(Deserialize)]
struct ConnectivityChatRequest {
    message: String,
}

#[derive(Serialize)]
struct ConnectivityChatResponseBody {
    assistant_content: String,
    tool_requests: Vec<BirthToolRequest>,
    stored_providers: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    key_stored: Option<KeyStoredInfo>,
}

#[derive(Serialize)]
struct KeyStoredInfo {
    provider: String,
    validated: bool,
}

#[derive(Deserialize)]
struct StoreKeyRequest {
    provider: String,
    key: String,
    #[serde(default = "default_true")]
    validate: bool,
}

fn default_true() -> bool {
    true
}

pub(crate) fn provider_supports_tier_models(provider: &str) -> bool {
    matches!(
        AppConfig::normalize_provider_name(provider).as_str(),
        "openai" | "anthropic" | "perplexity" | "xai" | "google"
    )
}

fn filter_operational_tools_for_mode(
    entries: Vec<SkillToolEntry>,
    mode: agentic::AgenticRouterMode,
) -> Vec<SkillToolEntry> {
    match mode {
        // Fast mode should stay low-latency and avoid multi-hop browser navigation.
        agentic::AgenticRouterMode::Auto => entries
            .into_iter()
            .filter(|e| e.tool_name != "web_browse")
            .collect(),
        agentic::AgenticRouterMode::ThinkHard | agentic::AgenticRouterMode::ThinkHarder => entries,
    }
}

fn mode_profile_name(mode: agentic::AgenticRouterMode) -> &'static str {
    match mode {
        agentic::AgenticRouterMode::Auto => "fast",
        agentic::AgenticRouterMode::ThinkHard => "standard",
        agentic::AgenticRouterMode::ThinkHarder => "pro",
    }
}

fn tools_profile_name(mode: agentic::AgenticRouterMode) -> &'static str {
    match mode {
        agentic::AgenticRouterMode::Auto => "minimal",
        agentic::AgenticRouterMode::ThinkHard | agentic::AgenticRouterMode::ThinkHarder => "full",
    }
}

/// List provider names from the keyring.
pub(crate) fn provider_names_from_keyring(keyring: &ProviderKeyring) -> Vec<String> {
    keyring
        .list_providers()
        .into_iter()
        .map(String::from)
        .collect()
}

pub(crate) fn resolve_ego_credentials_with_preference(
    keyring: &ProviderKeyring,
    preference: Option<&str>,
) -> (Option<String>, Option<String>) {
    // If explicit preference is set and has a key, use it first.
    if let Some(pref) = preference {
        let normalized = AppConfig::normalize_provider_name(pref);
        if let Some(key) = keyring.get_key_str(&normalized) {
            return (Some(normalized), Some(key.to_string()));
        }
    }
    // Default: anthropic > openai > first available key.
    let providers = provider_names_from_keyring(keyring);
    let preferred = ["anthropic", "openai"];
    let mut found_name: Option<String> = None;
    let mut found_key: Option<String> = None;
    for pref in &preferred {
        if let Some(key) = keyring.get_key_str(pref) {
            found_name = Some(pref.to_string());
            found_key = Some(key.to_string());
            break;
        }
    }
    if found_name.is_none() {
        for p in &providers {
            if let Some(key) = keyring.get_key_str(p) {
                found_name = Some(p.clone());
                found_key = Some(key.to_string());
                break;
            }
        }
    }
    (found_name, found_key)
}

fn resolve_superego_credentials_with_preference(
    config: &AppConfig,
    keyring: &ProviderKeyring,
    ego_name: Option<&str>,
    ego_key: Option<&str>,
) -> (Option<String>, Option<String>) {
    if let Some(trinity) = &config.trinity {
        if let Some(provider) = trinity.superego_provider.as_deref() {
            let normalized = AppConfig::normalize_provider_name(provider);
            if let Some(key) = keyring.get_key_str(&normalized) {
                return (Some(normalized), Some(key.to_string()));
            }
        }
    }

    (ego_name.map(str::to_string), ego_key.map(str::to_string))
}

fn parse_superego_l2_mode() -> orion_router::SuperegoL2Mode {
    match std::env::var("SUPEREGO_L2_MODE")
        .unwrap_or_else(|_| "off".to_string())
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "advisory" => orion_router::SuperegoL2Mode::Advisory,
        "enforce" => orion_router::SuperegoL2Mode::Enforce,
        _ => orion_router::SuperegoL2Mode::Off,
    }
}

#[derive(Serialize)]
struct StoreKeyResponse {
    ok: bool,
    provider: String,
    validated: bool,
}

#[derive(Serialize)]
struct ProvidersResponse {
    providers: Vec<String>,
}

#[derive(Serialize)]
struct TierModelsResponse {
    #[serde(skip_serializing_if = "Option::is_none")]
    active_provider: Option<String>,
    models: HashMap<String, TierModels>,
    #[serde(skip_serializing_if = "Option::is_none")]
    catalog: Option<HashMap<String, ProviderCatalogEntry>>,
}

#[derive(Deserialize)]
struct TierModelsUpdateRequest {
    models: HashMap<String, TierModels>,
}

#[derive(Serialize)]
struct TierModelsUpdateResponse {
    ok: bool,
}

#[derive(Deserialize)]
struct RefreshCatalogRequest {
    #[serde(default)]
    provider: Option<String>,
}

#[derive(Serialize)]
struct RefreshCatalogResponse {
    ok: bool,
    refreshed: Vec<String>,
}

#[derive(Deserialize)]
struct ValidateModelsRequest {
    #[serde(default)]
    provider: Option<String>,
}

#[derive(Serialize)]
struct ValidateModelsResponse {
    results: HashMap<String, ProviderModelValidation>,
}

#[derive(Serialize)]
struct ProviderModelValidation {
    fast: ModelValidationResult,
    standard: ModelValidationResult,
    pro: ModelValidationResult,
}

#[derive(Serialize)]
struct ModelValidationResult {
    model: String,
    valid: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

#[derive(Serialize)]
struct CognitiveRegistryResponse {
    registry: orion_capabilities::cognitive::model_catalog::CognitiveRegistry,
}

#[derive(Deserialize)]
struct SetActiveProviderRequest {
    provider: String,
}

#[derive(Serialize)]
struct CreateAgentResponse {
    id: String,
}

async fn health() -> Json<HealthResponse> {
    Json(HealthResponse { status: "ok" })
}

async fn ready(State(_state): State<AppState>) -> Json<HealthResponse> {
    Json(HealthResponse { status: "ok" })
}

/// Check if the birth model is available on the local LLM (Ollama /api/tags).
async fn check_birth_model_ready(base_url: &str, birth_model: &str) -> bool {
    let url = format!("{}/api/tags", base_url.trim_end_matches('/'));
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build();
    let client = match client {
        Ok(c) => c,
        Err(_) => return false,
    };
    let res = match client.get(&url).send().await {
        Ok(r) => r,
        Err(_) => return false,
    };
    if !res.status().is_success() {
        return false;
    }
    let body: serde_json::Value = match res.json().await {
        Ok(b) => b,
        Err(_) => return false,
    };
    let models = match body.get("models").and_then(|m| m.as_array()) {
        Some(m) => m,
        None => return false,
    };
    for m in models {
        let name = m.get("name").and_then(|n| n.as_str()).unwrap_or("");
        if name == birth_model || name.starts_with(&format!("{}:", birth_model)) {
            return true;
        }
    }
    false
}

async fn api_cognitive_registry(
    State(state): State<AppState>,
) -> Result<Json<CognitiveRegistryResponse>, ApiError> {
    let mut registry = orion_capabilities::cognitive::model_catalog::CognitiveRegistry::new();
    registry
        .refresh(
            std::env::var("OPENAI_API_KEY").ok().as_deref(),
            state.local_llm_base_url.as_deref(),
            state.local_llm_base_url.as_deref(),
        )
        .await;

    Ok(Json(CognitiveRegistryResponse { registry }))
}

async fn api_status(State(state): State<AppState>) -> Json<StatusResponse> {
    let (birth_complete, birth_stage, agent_name) = read_birth_status();
    let birth_model_ready = match (&state.local_llm_base_url, &state.birth_model) {
        (Some(base), Some(model)) => Some(check_birth_model_ready(base, model).await),
        _ => None,
    };
    Json(StatusResponse {
        memory_backend: state.memory_backend.as_str().to_string(),
        local_llm_configured: state.local_llm_base_url.is_some(),
        birth_model: state.birth_model.clone(),
        birth_model_ready,
        birth_complete,
        birth_stage,
        agent_name,
    })
}

async fn api_identities() -> Result<Json<IdentitiesResponse>, ApiError> {
    let root = data_root()
        .ok_or_else(|| ApiError::ServiceUnavailable("ORION_DATA_DIR not set".to_string()))?;

    let gc_path = GlobalConfig::config_path(&root);
    let gc = if gc_path.exists() {
        GlobalConfig::load(&root)
            .map_err(|e| ApiError::Internal(format!("Failed to load global config: {}", e)))?
    } else {
        return Ok(Json(IdentitiesResponse {
            agents: Vec::new(),
            mentor_name: None,
        }));
    };

    let mut agents = Vec::new();
    for entry in &gc.agents {
        let agent_dir = if entry.directory.is_absolute() {
            entry.directory.clone()
        } else {
            root.join(&entry.directory)
        };
        let config_path = agent_dir.join("config.json");
        let (birth_complete, birth_date) = if config_path.exists() {
            match AppConfig::load(&config_path) {
                Ok(cfg) => (
                    cfg.birth_complete,
                    resolve_birth_timestamp(&cfg, &agent_dir),
                ),
                Err(_) => (false, None),
            }
        } else {
            (false, None)
        };
        agents.push(AgentIdentityInfo {
            id: entry.id.clone(),
            name: entry.name.clone(),
            directory: agent_dir.to_string_lossy().to_string(),
            birth_complete,
            birth_date,
        });
    }
    Ok(Json(IdentitiesResponse {
        agents,
        mentor_name: gc.mentor_name,
    }))
}

// ---- Mentor Name API ----

#[derive(Serialize)]
struct MentorNameResponse {
    mentor_name: Option<String>,
}

#[derive(Deserialize)]
struct SetMentorNameRequest {
    mentor_name: String,
}

async fn api_get_mentor_name() -> Result<Json<MentorNameResponse>, ApiError> {
    let root = data_root()
        .ok_or_else(|| ApiError::ServiceUnavailable("ORION_DATA_DIR not set".to_string()))?;
    let gc_path = GlobalConfig::config_path(&root);
    let mentor_name = if gc_path.exists() {
        GlobalConfig::load(&root).ok().and_then(|gc| gc.mentor_name)
    } else {
        None
    };
    Ok(Json(MentorNameResponse { mentor_name }))
}

async fn api_set_mentor_name(
    Json(body): Json<SetMentorNameRequest>,
) -> Result<Json<MentorNameResponse>, ApiError> {
    let root = data_root()
        .ok_or_else(|| ApiError::ServiceUnavailable("ORION_DATA_DIR not set".to_string()))?;
    let gc_path = GlobalConfig::config_path(&root);
    let mut gc = if gc_path.exists() {
        GlobalConfig::load(&root)
            .map_err(|e| ApiError::Internal(format!("Failed to load global config: {}", e)))?
    } else {
        GlobalConfig::new(&root)
    };
    let name = body.mentor_name.trim().to_string();
    gc.mentor_name = if name.is_empty() { None } else { Some(name) };
    gc.save(&root)
        .map_err(|e| ApiError::Internal(format!("Failed to save global config: {}", e)))?;
    Ok(Json(MentorNameResponse {
        mentor_name: gc.mentor_name,
    }))
}

async fn api_create_agent(
    Json(body): Json<CreateAgentRequest>,
) -> Result<Json<CreateAgentResponse>, ApiError> {
    let name = body.name.trim();
    if name.is_empty() {
        return Err(ApiError::BadRequest("Agent name is required".to_string()));
    }

    let root = data_root()
        .ok_or_else(|| ApiError::ServiceUnavailable("ORION_DATA_DIR not set".to_string()))?;

    std::fs::create_dir_all(root.join("identities"))
        .map_err(|e| ApiError::Internal(format!("Failed to create identities dir: {}", e)))?;

    let uuid = Uuid::new_v4().to_string();
    let agent_dir = root.join("identities").join(&uuid);
    std::fs::create_dir_all(&agent_dir)
        .map_err(|e| ApiError::Internal(format!("Failed to create agent dir: {}", e)))?;
    let docs_dir = agent_dir.join("docs");
    std::fs::create_dir_all(&docs_dir)
        .map_err(|e| ApiError::Internal(format!("Failed to create docs dir: {}", e)))?;

    let database_url = std::env::var("DATABASE_URL").ok().filter(|s| !s.is_empty());
    let memory_backend = std::env::var("MEMORY_BACKEND")
        .ok()
        .unwrap_or_else(|| "sqlite".to_string());
    let memory_backend = MemoryBackend::from_str(&memory_backend).unwrap_or_default();
    let birth_model = std::env::var("BIRTH_MODEL").ok().filter(|s| !s.is_empty());
    let local_llm_base_url = std::env::var("LOCAL_LLM_BASE_URL")
        .ok()
        .filter(|s| !s.is_empty());

    let config = AppConfig {
        schema_version: CONFIG_SCHEMA_VERSION,
        agent_id: Some(uuid.clone()),
        data_dir: agent_dir.clone(),
        models_dir: agent_dir.join("models"),
        docs_dir: docs_dir.clone(),
        db_path: agent_dir.join("orion_seed.db"),
        openai_api_key: None,
        email: None,
        email_accounts: Vec::new(),
        birth_complete: false,
        birth_stage: None,
        external_pubkey_path: None,
        local_llm_base_url,
        routing_mode: RoutingMode::default(),
        trinity: None,
        agent_name: Some(name.to_string()),
        birth_timestamp: None,
        mcp_servers: Vec::new(),
        mcp_trust_policy: Default::default(),
        approved_skill_ids: Vec::new(),
        trusted_skill_signers: Vec::new(),
        sao_endpoint: None,
        memory_backend,
        database_url,
        birth_model,
        id_model_default: None,
        tier_models: std::collections::HashMap::new(),
        active_provider_preference: None,
        provider_catalog: std::collections::HashMap::new(),
    };
    let config_path = agent_dir.join("config.json");
    config
        .save(&config_path)
        .map_err(|e| ApiError::Internal(format!("Failed to save config: {}", e)))?;

    let gc_path = GlobalConfig::config_path(&root);
    let mut gc = if gc_path.exists() {
        GlobalConfig::load(&root)
            .map_err(|e| ApiError::Internal(format!("Failed to load global config: {}", e)))?
    } else {
        let mut new_gc = GlobalConfig::new(&root);
        // Auto-generate Hive master key on first initialization.
        if !new_gc.master_key_path.exists() {
            match orion_core::generate_master_key(&root) {
                Ok(result) => {
                    new_gc.master_key_path = result.master_key_path;
                    tracing::info!("Generated Hive master key");
                }
                Err(e) => {
                    tracing::warn!("Failed to generate Hive master key: {}", e);
                }
            }
        }
        new_gc
    };

    gc.register_agent(AgentEntry {
        id: uuid.clone(),
        name: name.to_string(),
        directory: PathBuf::from(format!("identities/{}", uuid)),
    })
    .map_err(|e| ApiError::Conflict(e.to_string()))?;
    gc.save(&root)
        .map_err(|e| ApiError::Internal(format!("Failed to save global config: {}", e)))?;

    tracing::info!("Created new agent: {} ({})", name, uuid);

    // Quick-start: auto-generate identity, constitutional docs, and complete birth.
    if body.quick_start {
        let qs_name = name.to_string();
        let qs_config_path = config_path.clone();
        let qs_docs_dir = docs_dir.clone();
        let _qs_agent_dir = agent_dir.clone();

        tokio::task::spawn_blocking(move || -> Result<(), String> {
            use orion_core::templates::{
                fill_soul_template_default, ETHICS_MD, GROWTH_MD, INSTINCTS_MD,
            };

            // Load fresh config for mutation.
            let mut config = AppConfig::load(&qs_config_path)
                .map_err(|e| format!("Quick-start load config: {}", e))?;

            // 1. Generate Ed25519 identity (Darkness).
            // generate_external_keypair writes pubkey to external_pubkey.bin and returns base64 private key.
            let keypair = orion_core::generate_external_keypair(&qs_docs_dir)
                .map_err(|e| format!("Quick-start generate identity: {}", e))?;

            // Reconstruct SigningKey from the returned base64 private key for signing.
            let signing_key = orion_core::parse_private_key(&keypair.private_key_base64)
                .map_err(|e| format!("Quick-start parse signing key: {}", e))?;

            // 2. Write standard constitutional documents.
            let soul_content = fill_soul_template_default(&qs_name);
            std::fs::write(qs_docs_dir.join("soul.md"), &soul_content)
                .map_err(|e| format!("Write soul.md: {}", e))?;
            std::fs::write(qs_docs_dir.join("ethics.md"), ETHICS_MD)
                .map_err(|e| format!("Write ethics.md: {}", e))?;
            std::fs::write(qs_docs_dir.join("instincts.md"), INSTINCTS_MD)
                .map_err(|e| format!("Write instincts.md: {}", e))?;
            std::fs::write(qs_docs_dir.join("growth.md"), GROWTH_MD)
                .map_err(|e| format!("Write growth.md: {}", e))?;

            // 3. Sign constitutional documents with the signing key.
            orion_core::sign_constitutional_documents(&signing_key, &qs_docs_dir)
                .map_err(|e| format!("Quick-start sign docs: {}", e))?;

            // 4. Mark birth complete (pubkey already written by generate_external_keypair).
            config.birth_complete = true;
            config.birth_stage = None;
            config.birth_timestamp = Some(chrono::Utc::now().to_rfc3339());
            config
                .save(&qs_config_path)
                .map_err(|e| format!("Quick-start save config: {}", e))?;

            tracing::info!(agent = %qs_name, "Quick-start birth completed");
            Ok(())
        })
        .await
        .map_err(|e| ApiError::Internal(format!("Quick-start task join: {}", e)))?
        .map_err(ApiError::Internal)?;
    }

    Ok(Json(CreateAgentResponse { id: uuid }))
}

async fn api_load_agent(Path(id): Path<String>) -> Result<Json<HealthResponse>, ApiError> {
    let root = data_root()
        .ok_or_else(|| ApiError::ServiceUnavailable("ORION_DATA_DIR not set".to_string()))?;

    let gc_path = GlobalConfig::config_path(&root);
    if !gc_path.exists() {
        return Err(ApiError::NotFound("No identities found".to_string()));
    }
    let gc = GlobalConfig::load(&root)
        .map_err(|e| ApiError::Internal(format!("Failed to load global config: {}", e)))?;

    if gc.find_agent(&id).is_none() {
        return Err(ApiError::NotFound(format!("Agent {} not found", id)));
    }

    let current_path = current_agent_path();
    std::fs::write(&current_path, &id)
        .map_err(|e| ApiError::Internal(format!("Failed to set current agent: {}", e)))?;

    tracing::info!("Loaded agent {}", id);
    Ok(Json(HealthResponse { status: "ok" }))
}

/// GET /api/agents/{id}/birth/state — materialize Darkness if needed and return stage + one-time private key.
async fn api_birth_state(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<BirthStateResponse>, ApiError> {
    let config_path =
        agent_config_path(&id).ok_or_else(|| ApiError::NotFound("Agent not found".to_string()))?;

    let config = AppConfig::load(&config_path)
        .map_err(|e| ApiError::Internal(format!("Failed to load config: {}", e)))?;
    if config.birth_complete {
        return Err(ApiError::BadRequest(
            "Agent birth already complete".to_string(),
        ));
    }

    // Check if we already have the signing key in memory or on disk.
    let id_clone = id.clone();
    let has_key_in_memory = state
        .birth_keys
        .lock()
        .map(|keys| keys.contains_key(&id))
        .unwrap_or(false);

    // If key is not in memory, try to reload from disk (handles disconnect/reconnect).
    if !has_key_in_memory {
        if let Some(dir) = agent_dir(&id) {
            if let Ok(Some(key_bytes)) = orion_core::load_signing_key(&dir) {
                if let Ok(mut keys) = state.birth_keys.lock() {
                    keys.insert(id.clone(), key_bytes);
                }
            }
        }
    }

    // Capture data_root for master key lineage signing.
    let data_root_path = data_root();

    // Run orchestrator on a blocking thread (PostgresStore creates its own Tokio runtime).
    let cp = config_path.clone();
    let result = tokio::task::spawn_blocking(
        move || -> Result<(BirthStateResponse, Option<Vec<u8>>), String> {
            let mut orch = orion_birth::BirthOrchestrator::new(config.clone())
                .map_err(|e| format!("Orchestrator: {}", e))?;

            let stage = orch.current_stage();
            let stage_name = stage.name().to_string();

            let private_key_base64 = if stage == orion_birth::BirthStage::Darkness {
                let pubkey_path = config.data_dir.join("external_pubkey.bin");
                if !pubkey_path.exists() {
                    orch.generate_identity(&config.docs_dir)
                        .map_err(|e| format!("Generate identity: {}", e))?;
                    orch.config_mut()
                        .set_birth_stage(orion_birth::BirthStage::Darkness.name());
                    orch.config_mut()
                        .save(&cp)
                        .map_err(|e| format!("Save config: {}", e))?;

                    // Sign agent's pubkey with Hive master key for lineage.
                    if let Some(ref root) = data_root_path {
                        let gc_path = GlobalConfig::config_path(root);
                        if gc_path.exists() {
                            if let Ok(gc) = GlobalConfig::load(root) {
                                let pubkey_path_inner =
                                    config.data_dir.join("external_pubkey.bin");
                                let lineage_sig_path =
                                    config.data_dir.join("hive_lineage.sig");
                                match orion_core::sign_agent_lineage(
                                    &gc.master_key_path,
                                    &pubkey_path_inner,
                                    &lineage_sig_path,
                                ) {
                                    Ok(true) => {
                                        tracing::info!("Hive lineage signature written for agent");
                                    }
                                    Ok(false) => {
                                        tracing::debug!("Hive master key or agent pubkey not found; skipping lineage");
                                    }
                                    Err(e) => {
                                        tracing::warn!("Failed to sign agent lineage: {}", e);
                                    }
                                }
                            }
                        }
                    }

                    orch.get_private_key_base64().map(String::from)
                } else {
                    None
                }
            } else {
                None
            };

            // Extract signing key bytes so they can be stored in AppState across requests
            let key_bytes = orch.take_signing_key_bytes();

            Ok((
                BirthStateResponse {
                    stage: stage_name,
                    private_key_base64,
                },
                key_bytes,
            ))
        },
    )
    .await
    .map_err(|e| ApiError::Internal(format!("Task join: {}", e)))?
    .map_err(ApiError::Internal)?;

    let (response, key_bytes) = result;

    // Store signing key bytes in AppState so they survive across HTTP requests
    if let Some(ref bytes) = key_bytes {
        if let Ok(mut keys) = state.birth_keys.lock() {
            keys.insert(id_clone.clone(), bytes.clone());
        }
        // Also persist to disk so the key survives server restarts
        if let Some(dir) = agent_dir(&id_clone) {
            if let Err(e) = orion_core::persist_signing_key(&dir, bytes) {
                tracing::warn!("Failed to persist signing key for {}: {}", id_clone, e);
            }
        }
    }

    Ok(Json(response))
}

/// POST /api/agents/{id}/birth/advance-darkness — user has saved the private key; advance to Ignition.
async fn api_birth_advance_darkness(
    Path(id): Path<String>,
) -> Result<Json<HealthResponse>, ApiError> {
    let config_path =
        agent_config_path(&id).ok_or_else(|| ApiError::NotFound("Agent not found".to_string()))?;

    let config = AppConfig::load(&config_path)
        .map_err(|e| ApiError::Internal(format!("Failed to load config: {}", e)))?;

    if config.birth_complete {
        return Err(ApiError::BadRequest(
            "Agent birth already complete".to_string(),
        ));
    }

    // Run orchestrator on a blocking thread (PostgresStore creates its own Tokio runtime).
    tokio::task::spawn_blocking(move || -> Result<(), String> {
        let mut orch = orion_birth::BirthOrchestrator::new(config)
            .map_err(|e| format!("Orchestrator: {}", e))?;
        orch.advance_past_darkness()
            .map_err(|e| format!("Advance: {}", e))?;
        Ok(())
    })
    .await
    .map_err(|e| ApiError::Internal(format!("Task join: {}", e)))?
    .map_err(ApiError::Internal)?;

    Ok(Json(HealthResponse { status: "ok" }))
}

/// POST /api/agents/{id}/birth/ignition — set local LLM URL (optional) and advance to Connectivity.
async fn api_birth_ignition(
    Path(id): Path<String>,
    Json(body): Json<IgnitionRequest>,
) -> Result<Json<HealthResponse>, ApiError> {
    let config_path =
        agent_config_path(&id).ok_or_else(|| ApiError::NotFound("Agent not found".to_string()))?;

    let mut config = AppConfig::load(&config_path)
        .map_err(|e| ApiError::Internal(format!("Failed to load config: {}", e)))?;

    if config.birth_complete {
        return Err(ApiError::BadRequest(
            "Agent birth already complete".to_string(),
        ));
    }

    if let Some(ref url) = body.local_llm_base_url {
        let validated = validate_local_llm_url(url)
            .map_err(|e| ApiError::BadRequest(format!("Invalid local LLM URL: {}", e)))?;
        config.local_llm_base_url = Some(validated);
        config
            .save(&config_path)
            .map_err(|e| ApiError::Internal(format!("Save config: {}", e)))?;
    }

    // Run orchestrator on a blocking thread (PostgresStore creates its own Tokio runtime).
    tokio::task::spawn_blocking(move || -> Result<(), String> {
        let mut orch = orion_birth::BirthOrchestrator::new(config)
            .map_err(|e| format!("Orchestrator: {}", e))?;
        orch.advance_to_connectivity()
            .map_err(|e| format!("Advance: {}", e))?;
        Ok(())
    })
    .await
    .map_err(|e| ApiError::Internal(format!("Task join: {}", e)))?
    .map_err(ApiError::Internal)?;

    Ok(Json(HealthResponse { status: "ok" }))
}

/// GET /api/agents/{id}/genesis/state — read persisted genesis path (for session recovery).
async fn api_genesis_state(Path(id): Path<String>) -> Result<Json<serde_json::Value>, ApiError> {
    let dir = agent_dir(&id).ok_or_else(|| ApiError::NotFound("Agent not found".to_string()))?;
    let gp_path = dir.join("genesis_path.json");
    if !gp_path.exists() {
        return Ok(Json(serde_json::json!({ "path": null })));
    }
    let content = std::fs::read_to_string(&gp_path)
        .map_err(|e| ApiError::Internal(format!("Read genesis_path.json: {}", e)))?;
    let value: serde_json::Value = serde_json::from_str(&content)
        .map_err(|e| ApiError::Internal(format!("Parse genesis_path.json: {}", e)))?;
    Ok(Json(value))
}

async fn api_genesis_paths(State(state): State<AppState>) -> Json<Vec<GenesisPathListItem>> {
    let list = state
        .genesis_registry
        .list_paths()
        .iter()
        .map(|info| {
            let depth = if !info.variants.is_empty() {
                Some(info.variants[0].id.clone())
            } else {
                None
            };
            GenesisPathListItem {
                id: info.id.clone(),
                label: info.label.clone(),
                description: info.description.clone(),
                estimated_time: info.estimated_time.clone(),
                depth,
            }
        })
        .collect();
    Json(list)
}

async fn api_genesis_start(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<GenesisStartRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let config_path = agent_config_path(&id)
        .ok_or_else(|| ApiError::NotFound(format!("Agent {} not found", id)))?;

    let config = AppConfig::load(&config_path)
        .map_err(|e| ApiError::Internal(format!("Failed to load config: {}", e)))?;

    if config.birth_complete {
        return Err(ApiError::BadRequest(
            "Agent birth already complete".to_string(),
        ));
    }

    let path = match body.path.as_str() {
        "quick_start" => GenesisPath::QuickStart,
        "direct" | "direct_discovery" => GenesisPath::Direct,
        "soul_crystallization" => {
            let depth = match body.depth.as_deref().unwrap_or("quick_start") {
                "conversation" => SoulCrystallizationDepth::Conversation,
                "deep_dive" => SoulCrystallizationDepth::DeepDive,
                _ => SoulCrystallizationDepth::QuickStart,
            };
            GenesisPath::SoulCrystallization { depth }
        }
        "soul_forge" => GenesisPath::SoulForge,
        _ => {
            return Err(ApiError::BadRequest(format!("Unknown path: {}", body.path)));
        }
    };

    // For QuickStart, auto-fill soul template and advance through Genesis → Emergence.
    if let GenesisPath::QuickStart = path {
        let id_qs = id.clone();
        let key_bytes = state
            .birth_keys
            .lock()
            .map_err(|_| ApiError::Internal("Lock failed".to_string()))?
            .remove(&id)
            .or_else(|| {
                agent_dir(&id).and_then(|dir| orion_core::load_signing_key(&dir).ok().flatten())
            })
            .ok_or_else(|| {
                ApiError::BadRequest(
                    "No signing key found. The signing key may have been lost.".to_string(),
                )
            })?;

        tokio::task::spawn_blocking(move || -> Result<(), String> {
            use orion_core::templates::{
                fill_soul_template_default, ETHICS_MD, GROWTH_MD, INSTINCTS_MD,
            };

            let mut config = AppConfig::load(&config_path)
                .map_err(|e| format!("Quick-start load config: {}", e))?;
            let agent_name = config
                .agent_name
                .clone()
                .unwrap_or_else(|| "Agent".to_string());
            let docs_dir = config.docs_dir.clone();
            std::fs::create_dir_all(&docs_dir).map_err(|e| format!("Create docs dir: {}", e))?;

            let soul_content = fill_soul_template_default(&agent_name);
            std::fs::write(docs_dir.join("soul.md"), &soul_content)
                .map_err(|e| format!("Write soul.md: {}", e))?;
            std::fs::write(docs_dir.join("ethics.md"), ETHICS_MD)
                .map_err(|e| format!("Write ethics.md: {}", e))?;
            std::fs::write(docs_dir.join("instincts.md"), INSTINCTS_MD)
                .map_err(|e| format!("Write instincts.md: {}", e))?;
            std::fs::write(docs_dir.join("growth.md"), GROWTH_MD)
                .map_err(|e| format!("Write growth.md: {}", e))?;

            let signing_key = orion_core::parse_private_key(&BASE64.encode(&key_bytes))
                .map_err(|e| format!("Parse signing key: {}", e))?;
            orion_core::sign_constitutional_documents(&signing_key, &docs_dir)
                .map_err(|e| format!("Sign docs: {}", e))?;

            config.birth_complete = true;
            config.birth_stage = None;
            config.birth_timestamp = Some(chrono::Utc::now().to_rfc3339());
            config
                .save(&config.config_path())
                .map_err(|e| format!("Save config: {}", e))?;

            persist_genesis_path(&id_qs, &GenesisPath::QuickStart)
                .map_err(|e| format!("Persist path: {}", e))?;

            tracing::info!(agent = %agent_name, "Quick-start genesis completed");
            Ok(())
        })
        .await
        .map_err(|e| ApiError::Internal(format!("Task join: {}", e)))?
        .map_err(ApiError::Internal)?;

        // Clean up persisted signing key
        if let Some(dir) = agent_dir(&id) {
            let _ = orion_core::delete_signing_key(&dir);
        }

        return Ok(Json(serde_json::json!({
            "ok": true,
            "path": "quick_start",
            "completed": true,
        })));
    }

    // Run orchestrator on a blocking thread (PostgresStore creates its own Tokio runtime).
    let path_clone = path.clone();
    let id_clone2 = id.clone();
    tokio::task::spawn_blocking(move || -> Result<(), String> {
        let mut orch = orion_birth::BirthOrchestrator::new(config.clone())
            .map_err(|e| format!("Orchestrator: {}", e))?;

        if orch.current_stage() != orion_birth::BirthStage::Connectivity {
            return Err(format!(
                "Agent must be in Connectivity stage to start Genesis (current: {:?})",
                orch.current_stage()
            ));
        }

        orch.advance_to_genesis_with_path(path_clone.clone())
            .map_err(|e| format!("Failed to advance: {}", e))?;

        persist_genesis_path(&id_clone2, &path_clone)
            .map_err(|e| format!("Failed to persist path: {}", e))?;
        Ok(())
    })
    .await
    .map_err(|e| ApiError::Internal(format!("Task join: {}", e)))?
    .map_err(ApiError::Internal)?;

    let mut genesis_response = serde_json::json!({ "ok": true, "path": path.id() });

    // --- Unified genesis session creation ---
    // Map old path ids to registry ids for backward compat.
    let registry_id = match body.path.as_str() {
        "direct" => "direct_discovery",
        other => other,
    };
    if state.genesis_registry.has_path(registry_id) {
        let config_for_session = AppConfig::load(&config_path)
            .map_err(|e| ApiError::Internal(format!("Load config for session: {}", e)))?;
        let agent_name = config_for_session
            .agent_name
            .clone()
            .unwrap_or_else(|| "Agent".to_string());
        let mentor_name = body.mentor_name.unwrap_or_else(|| "Mentor".to_string());

        let mut strategy = state
            .genesis_registry
            .create(registry_id)
            .map_err(|e| ApiError::Internal(format!("Create strategy: {}", e)))?;

        let chat_fn: Option<Box<dyn genesis_core::ChatFunction>> = if strategy.info().requires_chat
        {
            Some(Box::new(BirthRouterChatFunction {
                config: config_for_session.clone(),
            }))
        } else {
            None
        };

        let ctx = GenesisContext {
            agent_name,
            mentor_name,
            docs_dir: config_for_session.docs_dir.clone(),
            data_dir: agent_dir(&id).unwrap_or_default(),
            chat_fn,
            variant: body.depth,
            scenario_count: Some(5),
        };

        strategy
            .initialize(ctx)
            .await
            .map_err(|e| ApiError::Internal(format!("Initialize strategy: {}", e)))?;

        let first_step = strategy
            .begin()
            .await
            .map_err(|e| ApiError::Internal(format!("Begin strategy: {}", e)))?;

        // Include the first step in the response for new clients
        if let Ok(step_json) = serde_json::to_value(&first_step) {
            genesis_response["step"] = step_json;
        }

        // Store session unless already Complete
        if !matches!(first_step, StepRequest::Complete { .. }) {
            state
                .genesis_sessions
                .lock()
                .await
                .insert(id.clone(), strategy);
        }
    }

    tracing::info!("Genesis started for agent {} with path {:?}", id, path.id());
    Ok(Json(genesis_response))
}

/// POST /api/agents/{id}/genesis/step — unified step endpoint for all genesis paths.
///
/// Accepts a StepResponse (Message, Choice, or Form) and advances the genesis session.
/// Returns a StepRequest (NeedUserMessage, NeedChoice, NeedForm, or Complete).
/// When Complete is returned, the agent is advanced to Emergence stage.
async fn api_genesis_step(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<StepResponse>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let mut sessions = state.genesis_sessions.lock().await;
    let strategy = sessions.get_mut(&id).ok_or_else(|| {
        ApiError::NotFound(
            "No active genesis session for this agent. Call genesis/start first.".to_string(),
        )
    })?;

    let step_result = strategy
        .advance(body)
        .await
        .map_err(|e| ApiError::BadRequest(format!("Genesis advance failed: {}", e)))?;

    // If Complete, crystallize and advance to Emergence
    if let StepRequest::Complete { ref manifest } = step_result {
        let manifest_clone = *manifest.clone();
        let id_clone = id.clone();
        let _key_bytes_opt = state
            .birth_keys
            .lock()
            .ok()
            .and_then(|keys| keys.get(&id).cloned());

        tokio::task::spawn_blocking(move || -> Result<(), String> {
            let config_path = agent_config_path(&id_clone).ok_or("Agent not found")?;
            let config =
                AppConfig::load(&config_path).map_err(|e| format!("Load config: {}", e))?;
            let mut orch = orion_birth::BirthOrchestrator::new(config)
                .map_err(|e| format!("Orchestrator: {}", e))?;
            let mut manifest = manifest_clone;
            orch.crystallize_from_manifest(&mut manifest)
                .map_err(|e| format!("crystallize_from_manifest: {}", e))?;
            Ok(())
        })
        .await
        .map_err(|e| ApiError::Internal(format!("Task join: {}", e)))?
        .map_err(ApiError::Internal)?;

        // Clean up session
        sessions.remove(&id);
        tracing::info!(agent = %id, "Genesis step → Complete, advanced to Emergence");
    }

    // Serialize the StepRequest to JSON
    let response = serde_json::to_value(&step_result)
        .map_err(|e| ApiError::Internal(format!("Serialize step: {}", e)))?;
    Ok(Json(response))
}

/// GET /api/agents/{id}/genesis/session/state — read current genesis session state (unified).
async fn api_genesis_session_state(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let sessions = state.genesis_sessions.lock().await;
    match sessions.get(&id) {
        Some(strategy) => {
            let snapshot = strategy
                .snapshot()
                .map_err(|e| ApiError::Internal(format!("Snapshot failed: {}", e)))?;
            Ok(Json(serde_json::json!({
                "active": true,
                "path": strategy.info().id,
                "snapshot": snapshot,
            })))
        }
        None => Ok(Json(serde_json::json!({ "active": false }))),
    }
}

/// GET /api/agents/{id}/birth/chat/history — return persisted birth chat messages (for Direct Discovery).
async fn api_birth_chat_history(
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let dir = agent_dir(&id).ok_or_else(|| ApiError::NotFound("Agent not found".to_string()))?;
    let path = dir.join("birth_chat.json");
    if !path.exists() {
        return Ok(Json(serde_json::json!({ "messages": [] })));
    }
    let content = std::fs::read_to_string(&path)
        .map_err(|e| ApiError::Internal(format!("Read birth_chat.json: {}", e)))?;
    let messages: Vec<BirthChatMessage> = serde_json::from_str(&content).unwrap_or_default();
    Ok(Json(serde_json::json!({ "messages": messages })))
}

/// POST /api/agents/{id}/birth/chat — one turn of Direct Discovery Genesis chat; handles recommend_crystallize.
async fn api_birth_chat(
    Path(id): Path<String>,
    Json(body): Json<BirthChatRequest>,
) -> Result<Json<BirthChatResponseBody>, ApiError> {
    let dir = agent_dir(&id).ok_or_else(|| ApiError::NotFound("Agent not found".to_string()))?;
    let config_path =
        agent_config_path(&id).ok_or_else(|| ApiError::NotFound("Agent not found".to_string()))?;

    // Guard: only Direct Discovery or Soul Crystallization in Genesis stage.
    let gp_path = dir.join("genesis_path.json");
    let genesis_path_id = gp_path
        .exists()
        .then(|| {
            std::fs::read_to_string(&gp_path).ok().and_then(|s| {
                serde_json::from_str::<serde_json::Value>(&s)
                    .ok()
                    .and_then(|v| v.get("path").and_then(|p| p.as_str()).map(String::from))
            })
        })
        .flatten();
    let path_allows_chat = matches!(
        genesis_path_id.as_deref(),
        Some("direct") | Some("soul_crystallization")
    );
    if !path_allows_chat {
        return Err(ApiError::BadRequest("Birth chat is only for Direct Discovery or Soul Crystallization paths in Genesis stage.".to_string()));
    }

    let birth_chat_path = dir.join("birth_chat.json");
    let user_message = body.message.trim().to_string();
    if user_message.is_empty() {
        return Err(ApiError::BadRequest("message is required".to_string()));
    }

    // Blocking 1: load config, restore conversation, add user message, get conversation list.
    let config_path_1 = config_path.clone();
    let (conversation, config_path_clone) = tokio::task::spawn_blocking({
        let birth_chat_path = birth_chat_path.clone();
        let user_message = user_message.clone();
        move || -> Result<(Vec<(String, String)>, PathBuf), String> {
            let config =
                AppConfig::load(&config_path_1).map_err(|e| format!("Load config: {}", e))?;
            if config.birth_stage.as_deref() != Some("Genesis") {
                return Err("Agent must be in Genesis stage".to_string());
            }
            let mut orch = orion_birth::BirthOrchestrator::new(config)
                .map_err(|e| format!("Orchestrator: {}", e))?;
            if birth_chat_path.exists() {
                let content = std::fs::read_to_string(&birth_chat_path)
                    .map_err(|e| format!("Read birth_chat: {}", e))?;
                let messages: Vec<BirthChatMessage> =
                    serde_json::from_str(&content).unwrap_or_default();
                for m in &messages {
                    orch.add_message(&m.role, &m.content);
                }
            }
            orch.add_message("user", &user_message);
            let conversation: Vec<(String, String)> = orch
                .get_conversation()
                .iter()
                .map(|(r, c)| (r.clone(), c.clone()))
                .collect();
            Ok((conversation, config_path_1.clone()))
        }
    })
    .await
    .map_err(|e| ApiError::Internal(format!("Task join: {}", e)))?
    .map_err(ApiError::BadRequest)?;

    // Async: build messages, router, run chat turn.
    let config = tokio::task::spawn_blocking(move || AppConfig::load(&config_path_clone))
        .await
        .map_err(|e| ApiError::Internal(format!("Task join: {}", e)))?
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    let messages = build_genesis_messages(&conversation);
    if messages.len() <= 1 {
        return Err(ApiError::BadRequest("No messages to send".to_string()));
    }
    let router = build_birth_router(&config).await;
    let has_ego = router.has_ego();
    let ego_provider = router.ego_provider_name().map(|p| format!("{:?}", p));
    tracing::info!(
        agent = %id,
        local_llm = ?config.local_llm_base_url,
        birth_model = %config.effective_birth_model(),
        has_ego = has_ego,
        ego_provider = ?ego_provider,
        message_count = messages.len(),
        "genesis_chat: routing (ego_primary={}, provider={})",
        has_ego,
        ego_provider.as_deref().unwrap_or("local-only"),
    );
    let response = birth_chat_turn(&router, messages).await.map_err(|e| {
        tracing::error!(agent = %id, error = %e, "genesis_chat: chat turn failed");
        ApiError::Internal(format!("Birth chat: {}", e))
    })?;
    tracing::info!(
        agent = %id,
        content_len = response.assistant_content.len(),
        tool_count = response.tool_requests.len(),
        "genesis_chat: chat turn complete"
    );

    // Blocking 2: append user + assistant, handle recommend_crystallize, persist.
    let assistant_content = response.assistant_content.clone();
    let tool_requests = response.tool_requests.clone();
    let config_path_2 = config_path.clone();
    let crystallized = tokio::task::spawn_blocking({
        let birth_chat_path = birth_chat_path.clone();
        let user_message = user_message.clone();
        move || -> Result<bool, String> {
            let config =
                AppConfig::load(&config_path_2).map_err(|e| format!("Load config: {}", e))?;
            let mut orch = orion_birth::BirthOrchestrator::new(config)
                .map_err(|e| format!("Orchestrator: {}", e))?;
            if birth_chat_path.exists() {
                let content = std::fs::read_to_string(&birth_chat_path)
                    .map_err(|e| format!("Read birth_chat: {}", e))?;
                let messages: Vec<BirthChatMessage> =
                    serde_json::from_str(&content).unwrap_or_default();
                for m in &messages {
                    orch.add_message(&m.role, &m.content);
                }
            }
            orch.add_message("user", &user_message);
            orch.add_message("assistant", &assistant_content);

            let mut crystallized = false;
            for tr in &tool_requests {
                if tr.name == "recommend_crystallize" {
                    let name = tr
                        .arguments
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("Orion")
                        .to_string();
                    let purpose = tr
                        .arguments
                        .get("purpose")
                        .and_then(|v| v.as_str())
                        .unwrap_or("assist, retrieve, connect, and surface information.")
                        .to_string();
                    let personality = tr
                        .arguments
                        .get("personality")
                        .and_then(|v| v.as_str())
                        .unwrap_or("balanced; I adapt to context and your goals.")
                        .to_string();
                    let soul_content = fill_soul_template(&name, &purpose, &personality);
                    orch.crystallize_soul(&soul_content, GROWTH_MD)
                        .map_err(|e| format!("crystallize_soul: {}", e))?;
                    crystallized = true;
                    break;
                }
            }

            let updated: Vec<BirthChatMessage> = orch
                .get_conversation()
                .iter()
                .map(|(r, c)| BirthChatMessage {
                    role: r.clone(),
                    content: c.clone(),
                })
                .collect();
            std::fs::write(
                &birth_chat_path,
                serde_json::to_string_pretty(&updated).unwrap(),
            )
            .map_err(|e| format!("Write birth_chat: {}", e))?;
            Ok(crystallized)
        }
    })
    .await
    .map_err(|e| ApiError::Internal(format!("Task join: {}", e)))?
    .map_err(ApiError::Internal)?;

    Ok(Json(BirthChatResponseBody {
        assistant_content: response.assistant_content,
        tool_requests: response.tool_requests,
        crystallized,
    }))
}

// ============================================================================
// Connectivity API — dual-channel key provision
// ============================================================================

/// GET /api/agents/{id}/connectivity/providers — list currently stored provider names.
async fn api_connectivity_providers(
    Path(id): Path<String>,
) -> Result<Json<ProvidersResponse>, ApiError> {
    let dir = agent_dir(&id).ok_or_else(|| ApiError::NotFound("Agent not found".to_string()))?;

    let keyring = ProviderKeyring::load(dir)
        .unwrap_or_else(|_| ProviderKeyring::new(agent_dir(&id).unwrap_or_default()));
    let providers: Vec<String> = provider_names_from_keyring(&keyring);
    Ok(Json(ProvidersResponse { providers }))
}

/// GET /api/agents/{id}/tier-models — effective tier-model mapping for configured LLM providers.
async fn api_agent_tier_models(
    Path(id): Path<String>,
) -> Result<Json<TierModelsResponse>, ApiError> {
    let dir = agent_dir(&id).ok_or_else(|| ApiError::NotFound("Agent not found".to_string()))?;
    let config_path = agent_config_path(&id)
        .ok_or_else(|| ApiError::NotFound("Agent config not found".to_string()))?;
    let config = AppConfig::load(&config_path)
        .map_err(|e| ApiError::Internal(format!("Load config failed: {}", e)))?;

    let keyring = ProviderKeyring::load(dir.clone()).unwrap_or_else(|_| ProviderKeyring::new(dir));
    let (active_provider, _) = resolve_ego_credentials_with_preference(
        &keyring,
        config.active_provider_preference.as_deref(),
    );
    let active_provider = active_provider
        .map(|p| AppConfig::normalize_provider_name(&p))
        .filter(|p| provider_supports_tier_models(p));

    let mut providers: Vec<String> = Vec::new();
    for p in provider_names_from_keyring(&keyring) {
        let normalized = AppConfig::normalize_provider_name(&p);
        if provider_supports_tier_models(&normalized) && !providers.contains(&normalized) {
            providers.push(normalized);
        }
    }
    for p in config.tier_models.keys() {
        let normalized = AppConfig::normalize_provider_name(p);
        if provider_supports_tier_models(&normalized) && !providers.contains(&normalized) {
            providers.push(normalized);
        }
    }
    if let Some(active) = &active_provider {
        if !providers.contains(active) {
            providers.push(active.clone());
        }
    }

    let mut models: HashMap<String, TierModels> = HashMap::new();
    let mut catalog: HashMap<String, ProviderCatalogEntry> = HashMap::new();
    for provider in &providers {
        models.insert(provider.clone(), config.effective_tier_models(provider));
        // Include catalog data if cached, else provide curated baseline.
        let entry = config
            .provider_catalog
            .get(provider)
            .cloned()
            .unwrap_or_else(|| ProviderCatalogEntry {
                available_models: curated_provider_models(provider),
                source: "curated".to_string(),
                last_refreshed: None,
                warnings: vec![],
                validated: false,
            });
        catalog.insert(provider.clone(), entry);
    }

    Ok(Json(TierModelsResponse {
        active_provider,
        models,
        catalog: Some(catalog),
    }))
}

/// PUT /api/agents/{id}/tier-models — merge/update per-provider tier model mapping.
async fn api_agent_tier_models_update(
    Path(id): Path<String>,
    Json(body): Json<TierModelsUpdateRequest>,
) -> Result<Json<TierModelsUpdateResponse>, ApiError> {
    if body.models.is_empty() {
        return Err(ApiError::BadRequest(
            "models payload is required".to_string(),
        ));
    }

    let config_path = agent_config_path(&id)
        .ok_or_else(|| ApiError::NotFound("Agent config not found".to_string()))?;
    let mut config = AppConfig::load(&config_path)
        .map_err(|e| ApiError::Internal(format!("Load config failed: {}", e)))?;

    for (provider, incoming) in body.models {
        let normalized = AppConfig::normalize_provider_name(&provider);
        if !provider_supports_tier_models(&normalized) {
            return Err(ApiError::BadRequest(format!(
                "Unsupported provider for tier models: {}",
                provider
            )));
        }
        let fast = incoming.fast.trim().to_string();
        let standard = incoming.standard.trim().to_string();
        let pro = incoming.pro.trim().to_string();
        if fast.is_empty() || standard.is_empty() || pro.is_empty() {
            return Err(ApiError::BadRequest(format!(
                "All tier model values are required for provider '{}'",
                provider
            )));
        }
        config.tier_models.insert(
            normalized,
            TierModels {
                fast,
                standard,
                pro,
            },
        );
    }

    config
        .save(&config_path)
        .map_err(|e| ApiError::Internal(format!("Save config failed: {}", e)))?;

    Ok(Json(TierModelsUpdateResponse { ok: true }))
}

/// POST /api/agents/{id}/tier-models/refresh — refresh provider model catalogs from upstream APIs.
async fn api_agent_tier_models_refresh(
    Path(id): Path<String>,
    Json(body): Json<RefreshCatalogRequest>,
) -> Result<Json<RefreshCatalogResponse>, ApiError> {
    let dir = agent_dir(&id).ok_or_else(|| ApiError::NotFound("Agent not found".to_string()))?;
    let config_path = agent_config_path(&id)
        .ok_or_else(|| ApiError::NotFound("Agent config not found".to_string()))?;
    let mut config = AppConfig::load(&config_path)
        .map_err(|e| ApiError::Internal(format!("Load config failed: {}", e)))?;
    let keyring = ProviderKeyring::load(dir.clone()).unwrap_or_else(|_| ProviderKeyring::new(dir));

    // Determine which providers to refresh.
    let targets: Vec<String> = if let Some(ref p) = body.provider {
        vec![AppConfig::normalize_provider_name(p)]
    } else {
        provider_names_from_keyring(&keyring)
            .into_iter()
            .map(|p| AppConfig::normalize_provider_name(&p))
            .filter(|p| provider_supports_tier_models(p))
            .collect()
    };

    let mut refreshed: Vec<String> = Vec::new();
    for provider in &targets {
        if let Some(key) = keyring.get_key_str(provider) {
            let entry =
                orion_capabilities::cognitive::model_catalog::refresh_catalog(provider, key).await;
            config.provider_catalog.insert(provider.clone(), entry);
            refreshed.push(provider.clone());
        }
    }

    config
        .save(&config_path)
        .map_err(|e| ApiError::Internal(format!("Save config failed: {}", e)))?;

    Ok(Json(RefreshCatalogResponse {
        ok: true,
        refreshed,
    }))
}

/// POST /api/agents/{id}/tier-models/validate — validate selected tier models against provider APIs.
async fn api_agent_tier_models_validate(
    Path(id): Path<String>,
    Json(body): Json<ValidateModelsRequest>,
) -> Result<Json<ValidateModelsResponse>, ApiError> {
    let dir = agent_dir(&id).ok_or_else(|| ApiError::NotFound("Agent not found".to_string()))?;
    let config_path = agent_config_path(&id)
        .ok_or_else(|| ApiError::NotFound("Agent config not found".to_string()))?;
    let config = AppConfig::load(&config_path)
        .map_err(|e| ApiError::Internal(format!("Load config failed: {}", e)))?;
    let keyring = ProviderKeyring::load(dir.clone()).unwrap_or_else(|_| ProviderKeyring::new(dir));

    let targets: Vec<String> = if let Some(ref p) = body.provider {
        vec![AppConfig::normalize_provider_name(p)]
    } else {
        provider_names_from_keyring(&keyring)
            .into_iter()
            .map(|p| AppConfig::normalize_provider_name(&p))
            .filter(|p| provider_supports_tier_models(p))
            .collect()
    };

    let mut results: HashMap<String, ProviderModelValidation> = HashMap::new();
    for provider in &targets {
        let tiers = config.effective_tier_models(provider);
        let key = keyring
            .get_key_str(provider)
            .map(|s| s.to_string())
            .unwrap_or_default();

        let validate = |model: &str| {
            let p = provider.clone();
            let k = key.clone();
            let m = model.to_string();
            async move {
                match orion_capabilities::cognitive::model_catalog::validate_model(&p, &m, &k).await
                {
                    Ok(()) => ModelValidationResult {
                        model: m,
                        valid: true,
                        error: None,
                    },
                    Err(e) => ModelValidationResult {
                        model: m,
                        valid: false,
                        error: Some(e.to_string()),
                    },
                }
            }
        };

        let (fast, standard, pro) = tokio::join!(
            validate(&tiers.fast),
            validate(&tiers.standard),
            validate(&tiers.pro)
        );
        results.insert(
            provider.clone(),
            ProviderModelValidation {
                fast,
                standard,
                pro,
            },
        );
    }

    Ok(Json(ValidateModelsResponse { results }))
}

/// POST /api/agents/{id}/tier-models/reset — reset a provider's tier models to built-in defaults.
async fn api_agent_tier_models_reset(
    Path(id): Path<String>,
    Json(body): Json<RefreshCatalogRequest>,
) -> Result<Json<TierModelsUpdateResponse>, ApiError> {
    let config_path = agent_config_path(&id)
        .ok_or_else(|| ApiError::NotFound("Agent config not found".to_string()))?;
    let mut config = AppConfig::load(&config_path)
        .map_err(|e| ApiError::Internal(format!("Load config failed: {}", e)))?;

    if let Some(ref p) = body.provider {
        let normalized = AppConfig::normalize_provider_name(p);
        config.tier_models.remove(&normalized);
    } else {
        config.tier_models.clear();
    }

    config
        .save(&config_path)
        .map_err(|e| ApiError::Internal(format!("Save config failed: {}", e)))?;

    Ok(Json(TierModelsUpdateResponse { ok: true }))
}

/// PUT /api/agents/{id}/active-provider — set the preferred active provider for Ego routing.
async fn api_agent_set_active_provider(
    Path(id): Path<String>,
    Json(body): Json<SetActiveProviderRequest>,
) -> Result<Json<TierModelsUpdateResponse>, ApiError> {
    let config_path = agent_config_path(&id)
        .ok_or_else(|| ApiError::NotFound("Agent config not found".to_string()))?;
    let mut config = AppConfig::load(&config_path)
        .map_err(|e| ApiError::Internal(format!("Load config failed: {}", e)))?;

    let normalized = AppConfig::normalize_provider_name(&body.provider);
    if normalized.is_empty() || normalized == "auto" {
        config.active_provider_preference = None;
    } else {
        config.active_provider_preference = Some(normalized);
    }

    config
        .save(&config_path)
        .map_err(|e| ApiError::Internal(format!("Save config failed: {}", e)))?;

    Ok(Json(TierModelsUpdateResponse { ok: true }))
}

/// POST /api/agents/{id}/connectivity/keys — store an API key directly (button channel, no LLM).
async fn api_connectivity_store_key(
    Path(id): Path<String>,
    Json(body): Json<StoreKeyRequest>,
) -> Result<Json<StoreKeyResponse>, ApiError> {
    let config_path =
        agent_config_path(&id).ok_or_else(|| ApiError::NotFound("Agent not found".to_string()))?;

    let key = body.key.trim().to_string();
    if key.is_empty() {
        return Err(ApiError::BadRequest("API key is required".to_string()));
    }

    // Resolve provider (support "auto" detection)
    let provider = match body.provider.trim().to_lowercase().as_str() {
        "auto" => detect_provider_from_key(&key)
            .map(String::from)
            .ok_or_else(|| {
                ApiError::BadRequest(
                    "Could not detect provider from key prefix; specify provider explicitly"
                        .to_string(),
                )
            })?,
        p => p.to_string(),
    };

    // Validate if requested
    let validated = if body.validate {
        let prov = provider.clone();
        let k = key.clone();
        match tokio::task::spawn(async move {
            orion_capabilities::cognitive::validation::validate_api_key(&prov, &k).await
        })
        .await
        {
            Ok(Ok(())) => true,
            Ok(Err(e)) => {
                return Err(ApiError::BadRequest(format!(
                    "Key validation failed: {}",
                    e
                )));
            }
            Err(e) => {
                return Err(ApiError::Internal(format!("Validation task failed: {}", e)));
            }
        }
    } else {
        false
    };

    // Store key
    let provider_clone = provider.clone();
    tokio::task::spawn_blocking(move || -> Result<(), String> {
        let mut config =
            AppConfig::load(&config_path).map_err(|e| format!("Load config: {}", e))?;
        let mut keyring = ProviderKeyring::load(config.data_dir.clone())
            .unwrap_or_else(|_| ProviderKeyring::new(config.data_dir.clone()));
        let mut skill_kc = SkillKeychain::load(config.data_dir.clone())
            .unwrap_or_else(|_| SkillKeychain::new(config.data_dir.clone()));
        execute_store_provider_key(
            &mut keyring,
            &mut config,
            &provider_clone,
            &key,
            Some(&mut skill_kc),
        )
        .map_err(|e| format!("Store key: {}", e))?;
        Ok(())
    })
    .await
    .map_err(|e| ApiError::Internal(format!("Task join: {}", e)))?
    .map_err(ApiError::Internal)?;

    Ok(Json(StoreKeyResponse {
        ok: true,
        provider,
        validated,
    }))
}

/// DELETE /api/agents/{id}/connectivity/keys/{provider} — remove a stored provider key.
async fn api_connectivity_remove_key(
    Path((id, provider)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let dir = agent_dir(&id).ok_or_else(|| ApiError::NotFound("Agent not found".to_string()))?;

    let provider = provider.trim().to_lowercase();
    if provider.is_empty() {
        return Err(ApiError::BadRequest(
            "Provider name is required".to_string(),
        ));
    }

    tokio::task::spawn_blocking(move || -> Result<(), String> {
        let mut keyring =
            ProviderKeyring::load(dir.clone()).unwrap_or_else(|_| ProviderKeyring::new(dir));
        if !keyring.remove_key(&provider) {
            return Err(format!("No key stored for provider: {}", provider));
        }
        keyring.save().map_err(|e| format!("Save keyring: {}", e))?;
        Ok(())
    })
    .await
    .map_err(|e| ApiError::Internal(format!("Task join: {}", e)))?
    .map_err(ApiError::NotFound)?;

    Ok(Json(serde_json::json!({ "ok": true })))
}

/// GET /api/agents/{id}/connectivity/chat/history — return persisted connectivity chat messages.
async fn api_connectivity_chat_history(
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let dir = agent_dir(&id).ok_or_else(|| ApiError::NotFound("Agent not found".to_string()))?;
    let path = dir.join("connectivity_chat.json");
    if !path.exists() {
        return Ok(Json(serde_json::json!({ "messages": [] })));
    }
    let content = std::fs::read_to_string(&path)
        .map_err(|e| ApiError::Internal(format!("Read connectivity_chat.json: {}", e)))?;
    let messages: Vec<BirthChatMessage> = serde_json::from_str(&content).unwrap_or_default();
    Ok(Json(serde_json::json!({ "messages": messages })))
}

/// POST /api/agents/{id}/connectivity/chat — one turn of Connectivity stage chat.
async fn api_connectivity_chat(
    Path(id): Path<String>,
    Json(body): Json<ConnectivityChatRequest>,
) -> Result<Json<ConnectivityChatResponseBody>, ApiError> {
    let dir = agent_dir(&id).ok_or_else(|| ApiError::NotFound("Agent not found".to_string()))?;
    let config_path =
        agent_config_path(&id).ok_or_else(|| ApiError::NotFound("Agent not found".to_string()))?;

    let user_message = body.message.trim().to_string();
    if user_message.is_empty() {
        return Err(ApiError::BadRequest("message is required".to_string()));
    }

    // Redact API keys from user message before storing in history
    let redacted_user_message = redact_api_keys(&user_message);

    let connectivity_chat_path = dir.join("connectivity_chat.json");

    // Blocking 1: load config, restore conversation, get stored providers, build orchestrator state.
    let config_path_1 = config_path.clone();
    let (conversation, stored_providers) = tokio::task::spawn_blocking({
        let connectivity_chat_path = connectivity_chat_path.clone();
        let redacted = redacted_user_message.clone();
        #[allow(clippy::type_complexity)]
        move || -> Result<(Vec<(String, String)>, Vec<String>), String> {
            let config =
                AppConfig::load(&config_path_1).map_err(|e| format!("Load config: {}", e))?;
            if config.birth_stage.as_deref() != Some("Connectivity") {
                return Err("Agent must be in Connectivity stage".to_string());
            }
            let mut orch = orion_birth::BirthOrchestrator::new(config.clone())
                .map_err(|e| format!("Orchestrator: {}", e))?;
            if connectivity_chat_path.exists() {
                let content = std::fs::read_to_string(&connectivity_chat_path)
                    .map_err(|e| format!("Read connectivity_chat: {}", e))?;
                let messages: Vec<BirthChatMessage> =
                    serde_json::from_str(&content).unwrap_or_default();
                for m in &messages {
                    orch.add_message(&m.role, &m.content);
                }
            }
            orch.add_message("user", &redacted);

            let keyring = ProviderKeyring::load(config.data_dir.clone())
                .unwrap_or_else(|_| ProviderKeyring::new(config.data_dir.clone()));
            let stored: Vec<String> = provider_names_from_keyring(&keyring);

            let conversation: Vec<(String, String)> = orch
                .get_conversation()
                .iter()
                .map(|(r, c)| (r.clone(), c.clone()))
                .collect();
            Ok((conversation, stored))
        }
    })
    .await
    .map_err(|e| ApiError::Internal(format!("Task join: {}", e)))?
    .map_err(ApiError::BadRequest)?;

    // Async: build messages with stored providers context, router, run chat turn.
    let config = tokio::task::spawn_blocking({
        let cp = config_path.clone();
        move || AppConfig::load(&cp)
    })
    .await
    .map_err(|e| ApiError::Internal(format!("Task join: {}", e)))?
    .map_err(|e| ApiError::Internal(e.to_string()))?;

    // Build connectivity messages using the birth messages builder (which picks the right system prompt)
    let sp = stored_providers.clone();
    let messages = {
        // Reconstruct a temporary orchestrator to use build_birth_messages
        let config_for_msgs = config.clone();
        let conv = conversation.clone();
        tokio::task::spawn_blocking(move || -> Vec<orion_capabilities::cognitive::Message> {
            let mut orch = orion_birth::BirthOrchestrator::new(config_for_msgs).unwrap();
            // Restore conversation (minus the last user msg which build_birth_messages will add)
            for (role, content) in &conv[..conv.len().saturating_sub(1)] {
                orch.add_message(role, content);
            }
            let last_user = conv.last().map(|(_, c)| c.as_str());
            build_birth_messages(&orch, &sp, last_user)
        })
        .await
        .map_err(|e| ApiError::Internal(format!("Task join: {}", e)))?
    };

    if messages.len() <= 1 {
        return Err(ApiError::BadRequest("No messages to send".to_string()));
    }

    tracing::info!(
        agent = %id,
        local_llm = ?config.local_llm_base_url,
        birth_model = %config.effective_birth_model(),
        message_count = messages.len(),
        stored_providers = ?stored_providers,
        "connectivity_chat: building router"
    );
    let router = build_birth_router(&config).await;
    tracing::info!(agent = %id, "connectivity_chat: sending chat turn");
    let response = birth_chat_turn(&router, messages).await.map_err(|e| {
        tracing::error!(agent = %id, error = %e, "connectivity_chat: chat turn failed");
        ApiError::Internal(format!("Birth chat: {}", e))
    })?;
    tracing::info!(
        agent = %id,
        content_len = response.assistant_content.len(),
        tool_count = response.tool_requests.len(),
        "connectivity_chat: chat turn complete"
    );

    // Blocking 2: process tool requests (store_provider_key), persist conversation.
    let assistant_content = response.assistant_content.clone();
    let tool_requests = response.tool_requests.clone();
    let config_path_2 = config_path.clone();
    let raw_user_message = user_message.clone();

    let (final_stored_providers, key_stored) = tokio::task::spawn_blocking({
        let connectivity_chat_path = connectivity_chat_path.clone();
        let redacted = redacted_user_message.clone();
        let assistant = assistant_content.clone();
        let tools = tool_requests.clone();
        move || -> Result<(Vec<String>, Option<KeyStoredInfo>), String> {
            let mut config =
                AppConfig::load(&config_path_2).map_err(|e| format!("Load config: {}", e))?;
            let mut orch = orion_birth::BirthOrchestrator::new(config.clone())
                .map_err(|e| format!("Orchestrator: {}", e))?;
            if connectivity_chat_path.exists() {
                let content = std::fs::read_to_string(&connectivity_chat_path)
                    .map_err(|e| format!("Read connectivity_chat: {}", e))?;
                let messages: Vec<BirthChatMessage> =
                    serde_json::from_str(&content).unwrap_or_default();
                for m in &messages {
                    orch.add_message(&m.role, &m.content);
                }
            }
            orch.add_message("user", &redacted);

            // Redact assistant content before persisting
            let redacted_assistant = redact_api_keys(&assistant);
            orch.add_message("assistant", &redacted_assistant);

            // Execute store_provider_key tool calls
            let mut keyring = ProviderKeyring::load(config.data_dir.clone())
                .unwrap_or_else(|_| ProviderKeyring::new(config.data_dir.clone()));
            let mut skill_kc = SkillKeychain::load(config.data_dir.clone())
                .unwrap_or_else(|_| SkillKeychain::new(config.data_dir.clone()));
            let mut key_stored_info: Option<KeyStoredInfo> = None;

            for tr in &tools {
                if tr.name == "store_provider_key" {
                    let provider = tr
                        .arguments
                        .get("provider")
                        .and_then(|v| v.as_str())
                        .unwrap_or("auto");
                    let key = tr
                        .arguments
                        .get("key")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    match execute_store_provider_key(
                        &mut keyring,
                        &mut config,
                        provider,
                        key,
                        Some(&mut skill_kc),
                    ) {
                        Ok(resolved) => {
                            key_stored_info = Some(KeyStoredInfo {
                                provider: resolved,
                                validated: false, // Chat-channel keys are not pre-validated
                            });
                        }
                        Err(e) => {
                            tracing::warn!("store_provider_key failed: {}", e);
                        }
                    }
                }
            }

            // Auto-detect API keys in the raw user message if the LLM didn't emit tool calls
            if key_stored_info.is_none() {
                let detected = extract_api_keys_from_text(&raw_user_message);
                for (provider, key_str) in detected {
                    match execute_store_provider_key(
                        &mut keyring,
                        &mut config,
                        provider,
                        &key_str,
                        Some(&mut skill_kc),
                    ) {
                        Ok(resolved) => {
                            tracing::info!(
                                provider = %resolved,
                                "Auto-detected and stored API key from user message"
                            );
                            key_stored_info = Some(KeyStoredInfo {
                                provider: resolved,
                                validated: false,
                            });
                        }
                        Err(e) => {
                            tracing::warn!(
                                provider = %provider,
                                "Auto-detect store_provider_key failed: {}",
                                e
                            );
                        }
                    }
                }
            }

            let stored: Vec<String> = provider_names_from_keyring(&keyring);

            // Persist conversation
            let updated: Vec<BirthChatMessage> = orch
                .get_conversation()
                .iter()
                .map(|(r, c)| BirthChatMessage {
                    role: r.clone(),
                    content: c.clone(),
                })
                .collect();
            std::fs::write(
                &connectivity_chat_path,
                serde_json::to_string_pretty(&updated).unwrap(),
            )
            .map_err(|e| format!("Write connectivity_chat: {}", e))?;

            Ok((stored, key_stored_info))
        }
    })
    .await
    .map_err(|e| ApiError::Internal(format!("Task join: {}", e)))?
    .map_err(ApiError::Internal)?;

    // Redact assistant content in the response too
    let redacted_assistant_content = redact_api_keys(&response.assistant_content);

    Ok(Json(ConnectivityChatResponseBody {
        assistant_content: redacted_assistant_content,
        tool_requests: response.tool_requests,
        stored_providers: final_stored_providers,
        key_stored,
    }))
}

/// POST /api/agents/{id}/birth/complete-emergence — sign docs, write birth memory, drop key. Only when birth_stage is Emergence.
async fn api_birth_complete_emergence(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<HealthResponse>, ApiError> {
    let config_path =
        agent_config_path(&id).ok_or_else(|| ApiError::NotFound("Agent not found".to_string()))?;

    let config = AppConfig::load(&config_path)
        .map_err(|e| ApiError::Internal(format!("Failed to load config: {}", e)))?;

    if config.birth_complete {
        return Err(ApiError::BadRequest("Birth already complete".to_string()));
    }

    let stage = config.birth_stage.as_deref().unwrap_or("");
    if stage != "Emergence" {
        return Err(ApiError::BadRequest(format!(
            "Agent must be in Emergence stage to complete (current: {})",
            if stage.is_empty() { "unknown" } else { stage }
        )));
    }

    // Retrieve the signing key: try in-memory HashMap first, fall back to disk
    let key_bytes = state
        .birth_keys
        .lock()
        .map_err(|_| ApiError::Internal("Lock failed".to_string()))?
        .remove(&id)
        .or_else(|| {
            agent_dir(&id).and_then(|dir| orion_core::load_signing_key(&dir).ok().flatten())
        })
        .ok_or_else(|| {
            ApiError::BadRequest(
                "No signing key found for this agent. The signing key may have been lost. \
             You must re-create the agent."
                    .to_string(),
            )
        })?;

    let id_for_cleanup = id.clone();
    tokio::task::spawn_blocking(move || -> Result<(), String> {
        let mut orch = orion_birth::BirthOrchestrator::new(config)
            .map_err(|e| format!("Orchestrator: {}", e))?;
        orch.set_signing_key_bytes(&key_bytes)
            .map_err(|e| format!("Set signing key: {}", e))?;
        orch.complete_emergence()
            .map_err(|e| format!("complete_emergence: {}", e))?;
        Ok(())
    })
    .await
    .map_err(|e| ApiError::Internal(format!("Task join: {}", e)))?
    .map_err(ApiError::Internal)?;

    // Delete the persisted signing key — it's no longer needed after Emergence
    if let Some(dir) = agent_dir(&id_for_cleanup) {
        let _ = orion_core::delete_signing_key(&dir);
    }

    Ok(Json(HealthResponse { status: "ok" }))
}

// ============================================================================
// Operational Chat API — post-birth conversation
// ============================================================================

#[derive(Deserialize)]
struct OperationalChatRequest {
    message: String,
    #[serde(default)]
    router_mode: Option<agentic::AgenticRouterMode>,
    #[serde(default)]
    attachment_ids: Vec<String>,
    /// When true, routes through the Execution Governor for structured planning,
    /// loop detection, and recovery. Requires skills to be registered.
    #[serde(default)]
    use_governor: Option<bool>,
}

#[derive(Serialize)]
struct OperationalChatResponseBody {
    assistant_content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_executed: Option<OperationalToolResult>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stored_providers: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_log: Option<Vec<OperationalToolLogEntry>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    attachment_notice: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    context_warning: Option<ContextWarning>,
    #[serde(skip_serializing_if = "Option::is_none")]
    auto_archived: Option<AutoArchivedInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    routing_telemetry: Option<RoutingTelemetry>,
    #[serde(skip_serializing_if = "Option::is_none")]
    agentic_task_launched: Option<AgenticTaskLaunchedInfo>,
}

#[derive(Serialize, Clone)]
struct AgenticTaskLaunchedInfo {
    task_id: String,
    goal: String,
}

#[derive(Serialize, Clone)]
struct ContextWarning {
    estimated_tokens: usize,
    context_window: usize,
    usage_percent: u8,
    model: String,
}

#[derive(Serialize, Clone)]
struct AutoArchivedInfo {
    archive_id: String,
    message_count: usize,
}

#[derive(Serialize, Clone)]
struct RoutingTelemetry {
    mode_profile: String,
    tools_profile: String,
    requested_router_mode: String,
    routing_mode: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    active_provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    active_model: Option<String>,
    governor_enabled: bool,
}

#[derive(Serialize)]
struct OperationalToolResult {
    name: String,
    provider: String,
}

#[derive(Serialize)]
struct OperationalToolLogEntry {
    tool_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    skill_name: Option<String>,
    success: bool,
    output: String,
}

#[derive(Serialize)]
struct ProxyStatusResponse {
    mode: String,
    allow_host_docker_internal: bool,
    allowlist_path: String,
    internal_log_path: String,
    external_log_path: String,
    internal_log_exists: bool,
    external_log_exists: bool,
    internal_log_tail: String,
    external_log_tail: String,
}

#[derive(Serialize)]
struct ProxyAllowlistResponse {
    path: String,
    content: String,
}

#[derive(Deserialize)]
struct ProxyAllowlistUpdateRequest {
    content: String,
    mentor_approved: bool,
}

#[derive(Deserialize)]
struct ProxyLogsQuery {
    #[serde(default)]
    lines: Option<usize>,
}

#[derive(Serialize)]
struct ProxyLogsResponse {
    lines: usize,
    internal: String,
    external: String,
}

fn proxy_allowlist_path() -> PathBuf {
    std::env::var("ORION_PROXY_ALLOWLIST_PATH")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("docker/proxy/external/allowlist_domains.txt"))
}

fn proxy_log_dir() -> PathBuf {
    std::env::var("ORION_PROXY_LOG_DIR")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(".orion/proxy"))
}

fn read_file_or_empty(path: &std::path::Path) -> String {
    std::fs::read_to_string(path).unwrap_or_default()
}

fn read_log_tail(path: &std::path::Path, lines: usize) -> String {
    if lines == 0 {
        return String::new();
    }
    let content = match std::fs::read_to_string(path) {
        Ok(content) => content,
        Err(_) => return String::new(),
    };
    let collected = content
        .lines()
        .rev()
        .take(lines)
        .collect::<Vec<&str>>()
        .into_iter()
        .rev()
        .collect::<Vec<&str>>();
    collected.join("\n")
}

fn parse_bool_env(name: &str, default: bool) -> bool {
    match std::env::var(name) {
        Ok(v) => matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        ),
        Err(_) => default,
    }
}

fn contains_action_intent(content: &str) -> bool {
    let normalized = content.trim().to_ascii_lowercase();
    if normalized.is_empty() {
        return false;
    }
    let markers = [
        "i'll ",
        "i will ",
        "let me ",
        "i'm going to ",
        "im going to ",
        "proceeding now",
        "proceeding to ",
        "doing that now",
        "i can do that now",
    ];
    markers.iter().any(|m| normalized.contains(m))
}

fn strip_tool_result_block(content: &str) -> String {
    content
        .split_once("\n\n[Tool Result]\n")
        .map(|(clean, _)| clean.to_string())
        .unwrap_or_else(|| content.to_string())
}

fn looks_like_provisional_tool_message(content: &str) -> bool {
    let normalized = content.trim().to_ascii_lowercase();
    if normalized.is_empty() {
        return true;
    }
    let markers = [
        "let me check",
        "i'll check",
        "i will check",
        "let me look",
        "i'll do that",
        "i will do that",
        "i'll handle that",
        "one moment",
        "hang on",
        "searching",
        "looking that up",
    ];
    markers.iter().any(|m| normalized.contains(m))
}

fn format_fast_tool_reply(output_text: &str) -> String {
    let cleaned = output_text
        .lines()
        .map(|line| {
            if line.starts_with('[') {
                if let Some(idx) = line.find("] ") {
                    return line[idx + 2..].to_string();
                }
            }
            line.to_string()
        })
        .collect::<Vec<String>>()
        .join("\n");
    format!("Here's what I found:\n\n{}", cleaned.trim())
}

#[derive(Clone)]
enum PendingOperationalAction {
    CreateAgent {
        name: String,
        quick_start: bool,
    },
    CreateOrchestrationJob {
        request: CreateOrchestrationJobRequest,
    },
}

fn parse_confirmation_signal(message: &str) -> Option<bool> {
    let normalized = message.trim().to_ascii_lowercase();
    if normalized.is_empty() {
        return None;
    }
    let compact = normalized
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || c.is_whitespace())
        .collect::<String>();
    let positive = [
        "yes",
        "y",
        "confirm",
        "confirmed",
        "do it",
        "proceed",
        "approve",
        "go ahead",
        "sounds good",
    ];
    if positive
        .iter()
        .any(|s| compact == *s || compact.contains(s))
    {
        return Some(true);
    }
    let negative = [
        "no",
        "n",
        "cancel",
        "stop",
        "dont",
        "do not",
        "deny",
        "never mind",
    ];
    if negative
        .iter()
        .any(|s| compact == *s || compact.contains(s))
    {
        return Some(false);
    }
    None
}

fn parse_time_hh_mm(message: &str) -> Option<(u32, u32)> {
    let lowered = message.to_ascii_lowercase();
    let mut candidates = Vec::new();
    for token in lowered
        .split(|c: char| c.is_whitespace() || matches!(c, ',' | '.' | ';' | '(' | ')' | '[' | ']'))
    {
        let clean = token.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != ':');
        if clean.is_empty() {
            continue;
        }
        candidates.push(clean.to_string());
    }

    for raw in candidates {
        if let Some((h, m)) = parse_time_token(&raw) {
            return Some((h, m));
        }
    }
    None
}

fn parse_time_token(token: &str) -> Option<(u32, u32)> {
    let mut t = token.to_ascii_lowercase();
    t = t.trim().to_string();
    if t.is_empty() {
        return None;
    }
    let am = t.ends_with("am");
    let pm = t.ends_with("pm");
    if am || pm {
        t.truncate(t.len().saturating_sub(2));
    }
    let t = t.trim();
    if t.is_empty() {
        return None;
    }

    let (mut hour, minute) = if let Some((h, m)) = t.split_once(':') {
        let hour = h.parse::<u32>().ok()?;
        let minute = m.parse::<u32>().ok()?;
        (hour, minute)
    } else {
        let hour = t.parse::<u32>().ok()?;
        (hour, 0)
    };
    if minute > 59 {
        return None;
    }
    if am || pm {
        if hour == 0 || hour > 12 {
            return None;
        }
        if pm && hour != 12 {
            hour += 12;
        }
        if am && hour == 12 {
            hour = 0;
        }
        return Some((hour, minute));
    }
    if hour > 23 {
        return None;
    }
    Some((hour, minute))
}

fn extract_number_before_keyword(message: &str, keyword: &str) -> Option<u32> {
    let normalized = message.to_ascii_lowercase().replace(
        |c: char| !c.is_ascii_alphanumeric() && !c.is_whitespace(),
        " ",
    );
    let tokens: Vec<&str> = normalized.split_whitespace().collect();
    for idx in 1..tokens.len() {
        if tokens[idx] == keyword || tokens[idx].starts_with(keyword) {
            if let Ok(v) = tokens[idx - 1].parse::<u32>() {
                return Some(v);
            }
        }
    }
    None
}

fn detect_weekday(message: &str) -> Option<u8> {
    let m = message.to_ascii_lowercase();
    let days = [
        ("monday", 1_u8),
        ("tuesday", 2),
        ("wednesday", 3),
        ("thursday", 4),
        ("friday", 5),
        ("saturday", 6),
        ("sunday", 0),
    ];
    for (day, value) in days {
        if m.contains(day) {
            return Some(value);
        }
    }
    None
}

fn infer_job_mode(message: &str) -> orchestration::OrchestrationJobMode {
    let m = message.to_ascii_lowercase();
    if m.contains("id check")
        || m.contains("lightweight")
        || m.contains("monitor")
        || m.contains("watch")
    {
        return orchestration::OrchestrationJobMode::IdCheck;
    }
    orchestration::OrchestrationJobMode::AgenticRun
}

fn infer_goal_template(message: &str) -> String {
    let trimmed = message.trim();
    if trimmed.is_empty() {
        "Run a scheduled operational task".to_string()
    } else {
        trimmed.to_string()
    }
}

fn infer_job_name(message: &str) -> String {
    let normalized = message.trim();
    if normalized.is_empty() {
        return "Scheduled Task".to_string();
    }
    let lower = normalized.to_ascii_lowercase();
    if let Some(idx) = lower.find(" for ") {
        let tail = normalized[idx + 5..].trim();
        if !tail.is_empty() {
            let mut words = tail.split_whitespace().take(4).collect::<Vec<_>>();
            if !words.is_empty() {
                words.insert(0, "Job:");
                return words.join(" ");
            }
        }
    }
    let words = normalized.split_whitespace().take(4).collect::<Vec<_>>();
    if words.is_empty() {
        "Scheduled Task".to_string()
    } else {
        words.join(" ")
    }
}

fn infer_cron_expression(message: &str) -> String {
    let m = message.to_ascii_lowercase();
    let (hour, minute) = parse_time_hh_mm(message).unwrap_or((6, 0));

    if m.contains("every") && (m.contains(" minute") || m.contains(" minutes")) {
        let n = extract_number_before_keyword(&m, "minute")
            .or_else(|| extract_number_before_keyword(&m, "minutes"))
            .unwrap_or(15)
            .clamp(1, 59);
        return format!("0 */{} * * * * *", n);
    }
    if m.contains("every") && (m.contains(" hour") || m.contains(" hours")) {
        let n = extract_number_before_keyword(&m, "hour")
            .or_else(|| extract_number_before_keyword(&m, "hours"))
            .unwrap_or(1)
            .clamp(1, 23);
        return format!("0 0 */{} * * * *", n);
    }
    if m.contains("hourly") || m.contains("every hour") {
        return "0 0 * * * * *".to_string();
    }
    if m.contains("weekdays") || m.contains("weekday") {
        return format!("0 {} {} * * 1-5 *", minute, hour);
    }
    if let Some(dow) = detect_weekday(&m) {
        return format!("0 {} {} * * {} *", minute, hour, dow);
    }
    if m.contains("monthly") {
        let day = extract_number_before_keyword(&m, "day")
            .unwrap_or(1)
            .clamp(1, 28);
        return format!("0 {} {} {} * * *", minute, hour, day);
    }
    if m.contains("daily")
        || m.contains("every day")
        || m.contains("each day")
        || m.contains("every morning")
    {
        return format!("0 {} {} * * * *", minute, hour);
    }
    if m.contains("weekly") {
        return format!("0 {} {} * * 1 *", minute, hour);
    }
    format!("0 {} {} * * * *", minute, hour)
}

fn parse_agent_name_from_message(message: &str) -> Option<String> {
    let trimmed = message.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Some(start) = trimmed.find('"') {
        if let Some(end_rel) = trimmed[start + 1..].find('"') {
            let end = start + 1 + end_rel;
            let name = trimmed[start + 1..end].trim();
            if !name.is_empty() {
                return Some(name.to_string());
            }
        }
    }
    let lower = trimmed.to_ascii_lowercase();
    if let Some(idx) = lower.find("named ") {
        let tail = trimmed[idx + "named ".len()..].trim();
        if !tail.is_empty() {
            let mut out = tail.to_string();
            if let Some(stop_idx) = out.find([',', '.', ';']) {
                out.truncate(stop_idx);
            }
            let out = out.trim();
            if !out.is_empty() {
                return Some(out.to_string());
            }
        }
    }
    None
}

fn parse_pending_action_from_message(message: &str) -> Option<PendingOperationalAction> {
    let lower = message.to_ascii_lowercase();
    let asks_for_agent = (lower.contains("create agent")
        || lower.contains("new agent")
        || lower.contains("add agent"))
        && !lower.contains("agentic");
    if asks_for_agent {
        let name =
            parse_agent_name_from_message(message).unwrap_or_else(|| "New Agent".to_string());
        return Some(PendingOperationalAction::CreateAgent {
            name,
            quick_start: false,
        });
    }

    let schedule_like = lower.contains("schedule")
        && (lower.contains("every")
            || lower.contains("daily")
            || lower.contains("weekly")
            || lower.contains("monthly")
            || lower.contains("at "));
    let asks_for_job = lower.contains("create job")
        || lower.contains("schedule job")
        || lower.contains("set up job")
        || (lower.contains("scheduled") && lower.contains("job"))
        || (lower.contains("cron") && lower.contains("job"))
        || schedule_like;
    if asks_for_job {
        return Some(PendingOperationalAction::CreateOrchestrationJob {
            request: CreateOrchestrationJobRequest {
                name: infer_job_name(message),
                cron: infer_cron_expression(message),
                mode: infer_job_mode(message),
                goal_template: infer_goal_template(message),
                enabled: true,
                significance_policy: orchestration::SignificancePolicy::default(),
            },
        });
    }
    None
}

fn pending_action_preview(action: &PendingOperationalAction) -> String {
    match action {
        PendingOperationalAction::CreateAgent { name, quick_start } => format!(
            "I heard a request to create a new agent.\n\n- name: {}\n- quick start: {}\n\nReply `confirm` to create it, or `cancel` to abort.",
            name,
            if *quick_start { "yes" } else { "no" }
        ),
        PendingOperationalAction::CreateOrchestrationJob { request } => format!(
            "I heard a request to create a scheduled job.\n\n- name: {}\n- mode: {:?}\n- cron (UTC): {}\n- goal: {}\n\nReply `confirm` to create it, or `cancel` to abort.",
            request.name, request.mode, request.cron, request.goal_template
        ),
    }
}

fn append_chat_turn(
    chat_path: &std::path::Path,
    user: &str,
    assistant: &str,
) -> Result<(), String> {
    let mut updated: Vec<BirthChatMessage> = if chat_path.exists() {
        let content = std::fs::read_to_string(chat_path).map_err(|e| format!("Read: {}", e))?;
        serde_json::from_str(&content).unwrap_or_default()
    } else {
        Vec::new()
    };
    updated.push(BirthChatMessage {
        role: "user".to_string(),
        content: redact_api_keys(user),
    });
    updated.push(BirthChatMessage {
        role: "assistant".to_string(),
        content: redact_api_keys(assistant),
    });
    std::fs::write(chat_path, serde_json::to_string_pretty(&updated).unwrap())
        .map_err(|e| format!("Write operational_chat: {}", e))
}

/// POST /api/agents/{id}/chat/attachments — upload and parse attachments for operational chat.
async fn api_operational_chat_upload_attachments(
    Path(id): Path<String>,
    mut multipart: Multipart,
) -> Result<Json<ChatAttachmentUploadResponse>, ApiError> {
    let dir = agent_dir(&id).ok_or_else(|| ApiError::NotFound("Agent not found".to_string()))?;

    let mut uploads: Vec<UploadedAttachmentInput> = Vec::new();
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| ApiError::BadRequest(format!("Read multipart field: {}", e)))?
    {
        let Some(file_name) = field.file_name().map(|s| s.to_string()) else {
            continue;
        };
        let content_type = field.content_type().map(|s| s.to_string());
        let bytes = field
            .bytes()
            .await
            .map_err(|e| ApiError::BadRequest(format!("Read uploaded file bytes: {}", e)))?;
        uploads.push(UploadedAttachmentInput {
            file_name,
            content_type,
            bytes: bytes.to_vec(),
        });
    }

    let attachments = store_uploaded_attachments(&dir, uploads).map_err(|e| {
        if e.starts_with("upload validation:") {
            ApiError::BadRequest(e)
        } else {
            ApiError::Internal(e)
        }
    })?;

    Ok(Json(ChatAttachmentUploadResponse { attachments }))
}

/// GET /api/agents/{id}/chat/history — return persisted operational chat messages.
async fn api_operational_chat_history(
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let dir = agent_dir(&id).ok_or_else(|| ApiError::NotFound("Agent not found".to_string()))?;
    let path = dir.join("operational_chat.json");
    if !path.exists() {
        return Ok(Json(serde_json::json!({ "messages": [] })));
    }
    let content = std::fs::read_to_string(&path)
        .map_err(|e| ApiError::Internal(format!("Read operational_chat.json: {}", e)))?;
    let messages: Vec<BirthChatMessage> = serde_json::from_str(&content).unwrap_or_default();
    let display_messages: Vec<BirthChatMessage> = messages
        .into_iter()
        .map(|mut m| {
            if m.role == "assistant" {
                m.content = strip_tool_result_block(&m.content);
            }
            m
        })
        .collect();
    Ok(Json(serde_json::json!({ "messages": display_messages })))
}

// ---- Chat Archive API ----

#[derive(Serialize)]
struct ChatArchiveInfo {
    archive_id: String,
    timestamp: String,
    message_count: usize,
    preview: String,
}

/// POST /api/agents/{id}/chat/archive — archive the current conversation.
async fn api_archive_chat(Path(id): Path<String>) -> Result<Json<serde_json::Value>, ApiError> {
    let dir = agent_dir(&id).ok_or_else(|| ApiError::NotFound("Agent not found".to_string()))?;
    let chat_path = dir.join("operational_chat.json");
    if !chat_path.exists() {
        return Err(ApiError::BadRequest(
            "No conversation to archive".to_string(),
        ));
    }
    let content = std::fs::read_to_string(&chat_path)
        .map_err(|e| ApiError::Internal(format!("Read chat: {}", e)))?;
    let messages: Vec<BirthChatMessage> = serde_json::from_str(&content).unwrap_or_default();
    if messages.is_empty() {
        return Err(ApiError::BadRequest("No messages to archive".to_string()));
    }

    let archive_dir = dir.join("chat_archives");
    std::fs::create_dir_all(&archive_dir)
        .map_err(|e| ApiError::Internal(format!("Create archive dir: {}", e)))?;

    let ts = chrono::Utc::now().format("%Y%m%d_%H%M%S").to_string();
    let archive_id = format!("{}_{}", ts, messages.len());
    let archive_path = archive_dir.join(format!("{}.json", archive_id));

    let archive_content = serde_json::to_string_pretty(&messages)
        .map_err(|e| ApiError::Internal(format!("Serialize archive: {}", e)))?;
    std::fs::write(&archive_path, archive_content)
        .map_err(|e| ApiError::Internal(format!("Write archive: {}", e)))?;

    // Clear current conversation
    std::fs::write(&chat_path, "[]")
        .map_err(|e| ApiError::Internal(format!("Clear chat: {}", e)))?;

    Ok(Json(serde_json::json!({
        "archive_id": archive_id,
        "message_count": messages.len(),
    })))
}

/// GET /api/agents/{id}/chat/archives — list available archives.
async fn api_list_chat_archives(
    Path(id): Path<String>,
) -> Result<Json<Vec<ChatArchiveInfo>>, ApiError> {
    let dir = agent_dir(&id).ok_or_else(|| ApiError::NotFound("Agent not found".to_string()))?;
    let archive_dir = dir.join("chat_archives");
    if !archive_dir.exists() {
        return Ok(Json(Vec::new()));
    }
    let mut archives = Vec::new();
    let entries = std::fs::read_dir(&archive_dir)
        .map_err(|e| ApiError::Internal(format!("Read archive dir: {}", e)))?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let archive_id = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        if let Ok(content) = std::fs::read_to_string(&path) {
            let messages: Vec<BirthChatMessage> =
                serde_json::from_str(&content).unwrap_or_default();
            let preview = messages
                .first()
                .map(|m| {
                    let text = strip_tool_result_block(&m.content);
                    if text.len() > 100 {
                        format!("{}...", &text[..100])
                    } else {
                        text
                    }
                })
                .unwrap_or_default();
            // Parse timestamp from archive_id (YYYYMMDD_HHMMSS_count)
            let timestamp = archive_id
                .get(..15)
                .and_then(|s| {
                    chrono::NaiveDateTime::parse_from_str(s, "%Y%m%d_%H%M%S")
                        .ok()
                        .map(|dt| dt.format("%Y-%m-%d %H:%M:%S UTC").to_string())
                })
                .unwrap_or_else(|| archive_id.clone());
            archives.push(ChatArchiveInfo {
                archive_id,
                timestamp,
                message_count: messages.len(),
                preview,
            });
        }
    }
    archives.sort_by(|a, b| b.archive_id.cmp(&a.archive_id));
    Ok(Json(archives))
}

/// GET /api/agents/{id}/chat/archives/{archive_id} — load a specific archive.
async fn api_get_chat_archive(
    Path((id, archive_id)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let dir = agent_dir(&id).ok_or_else(|| ApiError::NotFound("Agent not found".to_string()))?;
    let archive_path = dir
        .join("chat_archives")
        .join(format!("{}.json", archive_id));
    if !archive_path.exists() {
        return Err(ApiError::NotFound("Archive not found".to_string()));
    }
    let content = std::fs::read_to_string(&archive_path)
        .map_err(|e| ApiError::Internal(format!("Read archive: {}", e)))?;
    let messages: Vec<BirthChatMessage> = serde_json::from_str(&content).unwrap_or_default();
    let display_messages: Vec<BirthChatMessage> = messages
        .into_iter()
        .map(|mut m| {
            if m.role == "assistant" {
                m.content = strip_tool_result_block(&m.content);
            }
            m
        })
        .collect();
    Ok(Json(serde_json::json!({
        "archive_id": archive_id,
        "messages": display_messages,
    })))
}

/// Estimate token count using character-based heuristic.
fn estimate_token_count(messages: &[orion_capabilities::cognitive::Message]) -> usize {
    messages.iter().map(|m| (m.content.len() / 4) + 4).sum()
}

/// Look up context window size for a model by name (well-known defaults).
fn lookup_context_window(model: &str) -> usize {
    let m = model.to_lowercase();
    if m.contains("gpt-4") || m.contains("gpt4") {
        128_000
    } else if m.contains("gpt-3.5") {
        16_384
    } else if m.contains("claude-opus")
        || m.contains("claude-sonnet")
        || m.contains("claude-haiku")
        || m.contains("o3")
        || m.contains("o4")
    {
        200_000
    } else if m.contains("gemini") {
        1_000_000
    } else if m.contains("grok") {
        131_072
    } else if m.contains("llama") || m.contains("qwen") || m.contains("mistral") {
        32_768
    } else {
        8_192
    }
}

/// Build SkillToolEntry list from the skill registry for system prompt injection.
pub(crate) fn build_skill_tool_entries(registry: &SkillRegistry) -> Vec<SkillToolEntry> {
    let mut entries = Vec::new();
    if let Ok(skills) = registry.list_with_tiers() {
        for (manifest, tier) in skills {
            let missing = registry.check_missing_secrets(&manifest);
            let has_required_missing = missing.iter().any(|m| m.required);
            let missing_names: Vec<String> =
                missing.iter().map(|m| m.secret_name.clone()).collect();
            let ready = !has_required_missing;
            if let Ok((skill, _, _)) = registry.get_skill(&manifest.id) {
                for tool in skill.tools() {
                    entries.push(SkillToolEntry {
                        skill_name: manifest.name.clone(),
                        skill_id: manifest.id.0.clone(),
                        trust_tier: tier.to_string(),
                        tool_name: tool.name,
                        tool_description: tool.description,
                        parameters: tool.parameters,
                        ready,
                        missing_secrets: missing_names.clone(),
                    });
                }
            }
        }
    }
    entries
}

/// POST /api/agents/{id}/chat — one turn of operational (post-birth) conversation.
async fn api_operational_chat(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<OperationalChatRequest>,
) -> Result<Json<OperationalChatResponseBody>, ApiError> {
    let dir = agent_dir(&id).ok_or_else(|| ApiError::NotFound("Agent not found".to_string()))?;
    let config_path =
        agent_config_path(&id).ok_or_else(|| ApiError::NotFound("Agent not found".to_string()))?;

    let user_message = body.message.trim().to_string();
    let requested_router_mode = body.router_mode.unwrap_or(agentic::AgenticRouterMode::Auto);
    let attachment_ids = normalize_attachment_ids(&body.attachment_ids);
    if user_message.is_empty() && attachment_ids.is_empty() {
        return Err(ApiError::BadRequest(
            "message or attachment_ids is required".to_string(),
        ));
    }

    let chat_path = dir.join("operational_chat.json");
    // Confirmation-first operational mutations:
    // allow chat to stage agent/job creation and execute only after explicit confirmation.
    if body.attachment_ids.is_empty() {
        if let Some(confirm) = parse_confirmation_signal(&user_message) {
            let pending = {
                let mut pending_map = state.pending_operational_actions.lock().await;
                pending_map.remove(&id)
            };
            if let Some(action) = pending {
                let assistant_content = match (confirm, action) {
                    (false, _) => "Canceled. No changes were applied.".to_string(),
                    (true, PendingOperationalAction::CreateAgent { name, quick_start }) => {
                        let created = api_create_agent(Json(CreateAgentRequest {
                            name: name.clone(),
                            quick_start,
                        }))
                        .await
                        .map_err(|e| ApiError::BadRequest(e.to_string()))?;
                        format!(
                            "Confirmed. Created agent \"{}\" with id `{}`. You can load it from the Hive screen.",
                            name, created.0.id
                        )
                    }
                    (true, PendingOperationalAction::CreateOrchestrationJob { request }) => {
                        let mut jobs = list_jobs(&dir).map_err(ApiError::Internal)?;
                        let job = create_job(&mut jobs, request, chrono::Utc::now())
                            .map_err(ApiError::BadRequest)?;
                        save_jobs(&dir, &jobs).map_err(ApiError::Internal)?;
                        format!(
                            "Confirmed. Scheduled job \"{}\" is now active.\n\n- mode: {:?}\n- cron (UTC): {}\n- next run: {}",
                            job.name,
                            job.mode,
                            job.cron,
                            job.next_run_at
                                .map(|dt| dt.to_rfc3339())
                                .unwrap_or_else(|| "n/a".to_string())
                        )
                    }
                };
                let _ = append_chat_turn(&chat_path, &user_message, &assistant_content);
                return Ok(Json(OperationalChatResponseBody {
                    assistant_content,
                    tool_executed: None,
                    stored_providers: None,
                    tool_log: None,
                    attachment_notice: None,
                    context_warning: None,
                    auto_archived: None,
                    routing_telemetry: None,
                    agentic_task_launched: None,
                }));
            }
        }

        if let Some(action) = parse_pending_action_from_message(&user_message) {
            let preview = pending_action_preview(&action);
            {
                let mut pending_map = state.pending_operational_actions.lock().await;
                pending_map.insert(id.clone(), action);
            }
            let _ = append_chat_turn(&chat_path, &user_message, &preview);
            return Ok(Json(OperationalChatResponseBody {
                assistant_content: preview,
                tool_executed: None,
                stored_providers: None,
                tool_log: None,
                attachment_notice: None,
                context_warning: None,
                auto_archived: None,
                routing_telemetry: None,
                agentic_task_launched: None,
            }));
        }
    }
    let attachment_context_entries = if attachment_ids.is_empty() {
        Vec::new()
    } else {
        load_attachment_context(&dir, &attachment_ids).map_err(ApiError::BadRequest)?
    };
    let has_attachment_context = !attachment_context_entries.is_empty();
    let attachment_context_block = if has_attachment_context {
        Some(build_attachment_context_block(&attachment_context_entries))
    } else {
        None
    };
    let message_for_model = if let Some(block) = &attachment_context_block {
        if user_message.is_empty() {
            block.clone()
        } else {
            format!("{}\n\n{}", user_message, block)
        }
    } else {
        user_message.clone()
    };
    let user_message_for_storage =
        if let Some(note) = build_attachment_storage_note(&attachment_context_entries) {
            if user_message.is_empty() {
                note
            } else {
                format!("{}\n\n{}", user_message, note)
            }
        } else {
            user_message.clone()
        };

    // Blocking 1: load config, verify birth complete, restore conversation, build messages.
    let (config, history) = tokio::task::spawn_blocking({
        let config_path = config_path.clone();
        let chat_path = chat_path.clone();
        move || -> Result<(AppConfig, Vec<BirthChatMessage>), String> {
            let config =
                AppConfig::load(&config_path).map_err(|e| format!("Load config: {}", e))?;
            if !config.birth_complete {
                return Err("Agent birth must be complete before chatting".to_string());
            }
            let history: Vec<BirthChatMessage> = if chat_path.exists() {
                let content = std::fs::read_to_string(&chat_path)
                    .map_err(|e| format!("Read operational_chat: {}", e))?;
                serde_json::from_str(&content).unwrap_or_default()
            } else {
                Vec::new()
            };
            Ok((config, history))
        }
    })
    .await
    .map_err(|e| ApiError::Internal(format!("Task join: {}", e)))?
    .map_err(ApiError::BadRequest)?;

    // Build system prompt from constitutional docs with dynamic skill tools
    let skill_tool_entries = filter_operational_tools_for_mode(
        build_skill_tool_entries(&state.skill_registry),
        requested_router_mode,
    );

    // Load provider names for system prompt awareness
    let vault_providers: Vec<String> = {
        let keyring = ProviderKeyring::load(config.data_dir.clone())
            .unwrap_or_else(|_| ProviderKeyring::new(config.data_dir.clone()));
        provider_names_from_keyring(&keyring)
    };

    // Sync email accounts from agent config into the shared email skill accounts list.
    routes::skills::sync_email_accounts(&config, &state.email_accounts, &state.skill_keychain)
        .await;

    let runtime_ctx = RuntimeContext {
        agent_id: id.clone(),
        data_dir: config.data_dir.to_string_lossy().to_string(),
    };

    let system_prompt = if skill_tool_entries.is_empty() {
        build_system_prompt(&config.docs_dir, &config.agent_name)
    } else {
        build_system_prompt_with_skills(
            &config.docs_dir,
            &config.agent_name,
            &skill_tool_entries,
            &vault_providers,
            Some(&runtime_ctx),
        )
    };

    // Load raw image bytes for vision model support
    let vision_images: Vec<orion_capabilities::cognitive::ImageContent> = if has_attachment_context
    {
        load_image_attachments(&dir, &attachment_ids)
            .into_iter()
            .map(|img| orion_capabilities::cognitive::ImageContent {
                data: BASE64.encode(&img.data),
                media_type: img.media_type,
            })
            .collect()
    } else {
        Vec::new()
    };

    // Build message list: system + history + user
    let mut messages: Vec<orion_capabilities::cognitive::Message> = Vec::new();
    messages.push(orion_capabilities::cognitive::Message::new(
        "system",
        &system_prompt,
    ));
    for m in &history {
        messages.push(orion_capabilities::cognitive::Message::new(
            &m.role, &m.content,
        ));
    }
    let vision_images_count = vision_images.len();
    if vision_images.is_empty() {
        messages.push(orion_capabilities::cognitive::Message::new(
            "user",
            &message_for_model,
        ));
    } else {
        messages.push(orion_capabilities::cognitive::Message::with_images(
            "user",
            &message_for_model,
            vision_images,
        ));
    }

    // Build operational router from config + ProviderKeyring
    let (ego_name, ego_key) = {
        let keyring = ProviderKeyring::load(config.data_dir.clone())
            .unwrap_or_else(|_| ProviderKeyring::new(config.data_dir.clone()));
        resolve_ego_credentials_with_preference(
            &keyring,
            config.active_provider_preference.as_deref(),
        )
    };
    let tier = match requested_router_mode {
        agentic::AgenticRouterMode::Auto => ThinkingModelTier::Fast,
        agentic::AgenticRouterMode::ThinkHard => ThinkingModelTier::Standard,
        agentic::AgenticRouterMode::ThinkHarder => ThinkingModelTier::Pro,
    };
    let ego_model = ego_name
        .as_deref()
        .map(|provider| config.effective_tier_model(provider, tier));

    // Orchestration policy: mentor-facing chat is Ego-primary.
    // Router still preserves built-in Id fallback when Ego is unavailable or fails.
    let effective_routing_mode = orion_core::RoutingMode::EgoPrimary;

    tracing::info!(
        agent = %id,
        local_llm = ?config.local_llm_base_url,
        ego_provider = ?ego_name,
        ego_model = ?ego_model,
        routing_mode = ?effective_routing_mode,
        requested_router_mode = ?requested_router_mode,
        attachment_count = attachment_ids.len(),
        message_count = messages.len(),
        "operational_chat: building router"
    );

    let active_model_name = ego_model.clone().unwrap_or_else(|| "unknown".to_string());

    let ego_name_for_sup = ego_name.clone();
    let ego_key_for_sup = ego_key.clone();
    let mut router = orion_router::IdEgoRouter::with_provider_auto_detect(
        config.local_llm_base_url.clone(),
        ego_name.as_deref(),
        ego_key,
        ego_model.clone(),
        effective_routing_mode,
    )
    .await;
    if state.superego_l2_mode != orion_router::SuperegoL2Mode::Off {
        let keyring = ProviderKeyring::load(config.data_dir.clone())
            .unwrap_or_else(|_| ProviderKeyring::new(config.data_dir.clone()));
        let (sup_name, sup_key) = resolve_superego_credentials_with_preference(
            &config,
            &keyring,
            ego_name_for_sup.as_deref(),
            ego_key_for_sup.as_deref(),
        );
        if let Some(key) = sup_key {
            let (provider, _) =
                orion_router::build_ego_provider(sup_name.as_deref(), Some(key), None);
            if let Some(provider) = provider {
                router = router.with_superego_config(provider, sup_name, state.superego_l2_mode);
            }
        }
    }

    // Context window tracking: estimate usage and detect if near limit
    let estimated_tokens = estimate_token_count(&messages);
    let context_window = lookup_context_window(&active_model_name);

    let usage_percent =
        ((estimated_tokens as f64 / context_window as f64) * 100.0).min(100.0) as u8;

    // Auto-archive at 90%+ context usage
    let mut auto_archived: Option<AutoArchivedInfo> = None;
    if usage_percent >= 90 && history.len() >= 4 {
        let archive_dir = dir.join("chat_archives");
        let _ = std::fs::create_dir_all(&archive_dir);
        let ts = chrono::Utc::now().format("%Y%m%d_%H%M%S").to_string();
        let archive_id = format!("{}_{}", ts, history.len());
        let archive_path = archive_dir.join(format!("{}.json", archive_id));
        if let Ok(archive_json) = serde_json::to_string_pretty(&history) {
            if std::fs::write(&archive_path, archive_json).is_ok() {
                // Clear history and rebuild messages with only the user message
                let _ = std::fs::write(&chat_path, "[]");
                messages.retain(|m| m.role == "system");
                if vision_images_count > 0 {
                    messages.push(orion_capabilities::cognitive::Message::with_images(
                        "user",
                        &message_for_model,
                        Vec::new(), // Images already consumed above
                    ));
                } else {
                    messages.push(orion_capabilities::cognitive::Message::new(
                        "user",
                        &message_for_model,
                    ));
                }
                auto_archived = Some(AutoArchivedInfo {
                    archive_id,
                    message_count: history.len(),
                });
                tracing::info!(agent = %id, usage_percent, "auto-archived chat due to context window pressure");
            }
        }
    }

    let context_warning = if usage_percent >= 75 && auto_archived.is_none() {
        Some(ContextWarning {
            estimated_tokens,
            context_window,
            usage_percent,
            model: active_model_name.clone(),
        })
    } else {
        None
    };

    // Build structured tool definitions for tool-aware routing.
    let tool_defs = tool_extraction::build_operational_tool_definitions(&skill_tool_entries);
    // Redact API keys from user message before storing
    let redacted_user_message = redact_api_keys(&user_message_for_storage);

    // ═══════════════════════════════════════════════════════════════════════
    // GOVERNED EXECUTION PATH: when skills are available and the user request
    // looks like it involves tool usage, route through the Execution Governor
    // for structured planning, loop detection, and recovery.
    // ═══════════════════════════════════════════════════════════════════════
    let use_governor =
        !skill_tool_entries.is_empty() && ego_name.is_some() && body.use_governor.unwrap_or(false);
    let routing_telemetry = RoutingTelemetry {
        mode_profile: mode_profile_name(requested_router_mode).to_string(),
        tools_profile: tools_profile_name(requested_router_mode).to_string(),
        requested_router_mode: format!("{:?}", requested_router_mode),
        routing_mode: format!("{:?}", effective_routing_mode),
        active_provider: ego_name.clone(),
        active_model: ego_model.clone(),
        governor_enabled: use_governor,
    };

    if use_governor {
        tracing::info!(agent = %id, "operational_chat: using Execution Governor path");

        let governor = governed_chat::build_governor(
            &config,
            ego_name.as_deref().unwrap_or("openai"),
            &{
                let keyring = ProviderKeyring::load(config.data_dir.clone())
                    .unwrap_or_else(|_| ProviderKeyring::new(config.data_dir.clone()));
                resolve_ego_credentials_with_preference(
                    &keyring,
                    config.active_provider_preference.as_deref(),
                )
                .1
                .unwrap_or_default()
            },
            state.skill_registry.clone(),
            state.skill_executor.clone(),
        );

        if let Some(governor) = governor {
            let user_request = orion_router::cognitive::UserRequest {
                message: message_for_model.clone(),
                available_tools: governed_chat::build_planner_tool_descriptors(
                    &state.skill_registry,
                ),
                known_constraints: governed_chat::load_persistent_constraints(&config.data_dir),
                agent_state_summary: format!(
                    "Agent: {}, Birth complete: true",
                    config.agent_name.as_deref().unwrap_or("unknown")
                ),
            };

            let session_ctx = orion_router::cognitive::SessionContext {
                agent_id: id.clone(),
                agent_name: config.agent_name.clone().unwrap_or_default(),
                system_prompt: system_prompt.clone(),
                conversation_history: history
                    .iter()
                    .map(|m| orion_capabilities::cognitive::Message::new(&m.role, &m.content))
                    .collect(),
            };

            let governed_result = governor.execute(user_request, &session_ctx).await;

            // Persist any newly discovered constraints
            // Persist any newly discovered constraints from the execution
            match &governed_result {
                orion_router::governor::GovernedResult::MaxIterationsReached { state, .. }
                | orion_router::governor::GovernedResult::Incomplete { state, .. }
                | orion_router::governor::GovernedResult::NeedsInput { state, .. }
                | orion_router::governor::GovernedResult::Aborted { state, .. }
                | orion_router::governor::GovernedResult::TimeBudgetExceeded { state } => {
                    governed_chat::save_discovered_constraints(
                        &config.data_dir,
                        &state.constraints_discovered,
                    );
                }
                orion_router::governor::GovernedResult::Success(_) => {}
            }

            let assistant_content = governed_chat::governed_result_to_content(&governed_result);
            let redacted_response = redact_api_keys(&assistant_content);

            // Persist conversation
            let _ = tokio::task::spawn_blocking({
                let chat_path = chat_path.clone();
                let redacted_user = redacted_user_message.clone();
                let redacted_assistant = redact_api_keys(&assistant_content);
                move || -> Result<(), String> {
                    let mut updated: Vec<BirthChatMessage> = if chat_path.exists() {
                        let content = std::fs::read_to_string(&chat_path)
                            .map_err(|e| format!("Read: {}", e))?;
                        serde_json::from_str(&content).unwrap_or_default()
                    } else {
                        Vec::new()
                    };
                    updated.push(BirthChatMessage {
                        role: "user".to_string(),
                        content: redacted_user,
                    });
                    updated.push(BirthChatMessage {
                        role: "assistant".to_string(),
                        content: redacted_assistant,
                    });
                    std::fs::write(&chat_path, serde_json::to_string_pretty(&updated).unwrap())
                        .map_err(|e| format!("Write: {}", e))?;
                    Ok(())
                }
            })
            .await;

            return Ok(Json(OperationalChatResponseBody {
                assistant_content: redacted_response,
                tool_executed: None,
                stored_providers: None,
                tool_log: None,
                attachment_notice: None,
                context_warning,
                auto_archived,
                routing_telemetry: Some(routing_telemetry.clone()),
                agentic_task_launched: None,
            }));
        }
    }

    tracing::info!(
        agent = %id,
        tool_def_count = tool_defs.len(),
        "operational_chat: sending chat turn"
    );

    // Pro-mode council path: for Pro tier with 2+ providers, run MoA DAG in Rust.
    // Council does not support structured tool-calling; falls back to tool-aware route on failure.
    let response = if tier == ThinkingModelTier::Pro {
        let keyring_for_pro = ProviderKeyring::load(config.data_dir.clone())
            .unwrap_or_else(|_| ProviderKeyring::new(config.data_dir.clone()));
        let council_providers = resolve_council_providers(
            &keyring_for_pro,
            config.active_provider_preference.as_deref(),
        );
        if council_providers.len() >= 2 {
            let provider_configs: Vec<orion_router::council::ProviderConfig> = council_providers
                .into_iter()
                .map(|(name, key)| {
                    let model = config.effective_tier_model(&name, ThinkingModelTier::Pro);
                    orion_router::council::ProviderConfig {
                        name,
                        api_key: key,
                        model,
                    }
                })
                .collect();
            tracing::info!(
                agent = %id,
                providers = ?provider_configs.iter().map(|p| format!("{}:{}", p.name, p.model)).collect::<Vec<_>>(),
                "operational_chat: pro council path"
            );

            // Council persist() calls store.insert_memory/add_edge. PostgresStore uses block_on(),
            // which panics when called from async. So use memory store only for Sqlite; for Postgres
            // pass None so council runs without persisting (Pro chat still works).
            let council_memory: Option<orion_memory::MemoryStore> = match config.memory_backend {
                orion_core::MemoryBackend::Postgres => None,
                orion_core::MemoryBackend::Sqlite => {
                    let config_for_store = config.clone();
                    tokio::task::spawn_blocking(move || {
                        orion_memory::MemoryStore::open_with_config(&config_for_store).ok()
                    })
                    .await
                    .ok()
                    .flatten()
                }
            };
            match orion_router::council::run_council(
                &messages,
                &provider_configs,
                council_memory.as_ref(),
            )
            .await
            {
                Ok(council_response) => council_response,
                Err(e) => {
                    tracing::warn!(agent = %id, error = %e, "operational_chat: council failed, falling back to tool-aware Ego");
                    router
                        .route_with_tools(messages.clone(), tool_defs.clone())
                        .await
                        .map_err(|e| ApiError::Internal(format!("Chat failed: {}", e)))?
                }
            }
        } else {
            router
                .route_with_tools(messages.clone(), tool_defs.clone())
                .await
                .map_err(|e| ApiError::Internal(format!("Chat failed: {}", e)))?
        }
    } else {
        router
            .route_with_tools(messages.clone(), tool_defs.clone())
            .await
            .map_err(|e| {
                tracing::error!(agent = %id, error = %e, "operational_chat: chat turn failed");
                ApiError::Internal(format!("Chat failed: {}", e))
            })?
    };

    // Unified tool extraction: structured tool_calls first, then legacy text blocks as fallback.
    let extraction = tool_extraction::extract_tool_calls(&response);
    let mut clean_content = extraction.clean_content;
    let mut tool_requests: Vec<BirthToolRequest> = extraction
        .tool_calls
        .iter()
        .map(BirthToolRequest::from)
        .collect();

    // Safety net: if the model describes immediate action but emits no tools,
    // do one corrective nudge turn to force action-oriented tool use.
    if tool_requests.is_empty() && contains_action_intent(&clean_content) {
        let mut nudge_messages = messages.clone();
        nudge_messages.push(orion_capabilities::cognitive::Message::new(
            "assistant",
            &clean_content,
        ));
        nudge_messages.push(orion_capabilities::cognitive::Message::new(
            "user",
            "You stated immediate action but emitted no tool calls. Execute now: emit the required tool call(s) this turn (or `launch_agentic_task` if the workflow is multi-step). Then report concrete results.",
        ));
        match router
            .route_with_tools(nudge_messages, tool_defs.clone())
            .await
        {
            Ok(nudged_response) => {
                let nudged = tool_extraction::extract_tool_calls(&nudged_response);
                clean_content = nudged.clean_content;
                tool_requests = nudged
                    .tool_calls
                    .iter()
                    .map(BirthToolRequest::from)
                    .collect();
                tracing::info!(
                    agent = %id,
                    tool_count = tool_requests.len(),
                    "operational_chat: applied action-intent nudge"
                );
            }
            Err(e) => {
                tracing::warn!(
                    agent = %id,
                    error = %e,
                    "operational_chat: action-intent nudge failed"
                );
            }
        }
    }

    tracing::info!(
        agent = %id,
        content_len = clean_content.len(),
        tool_count = tool_requests.len(),
        attachment_count = attachment_ids.len(),
        "operational_chat: chat turn complete"
    );

    let attachment_turn = has_attachments(&attachment_ids);
    let mut blocked_tool_count: usize = 0;
    let advisory_code = match router.superego_check(&message_for_model).await {
        orion_router::SuperegoResult::Advisory { code, .. } => code,
        _ => None,
    };
    let capability_envelope = orion_core::evaluate_user_message_for_capabilities(
        &message_for_model,
        advisory_code.as_deref(),
    );

    // Execute skill tool requests (async — must happen outside spawn_blocking).
    // Supports multiple tool calls per turn for autonomous multi-step actions.
    let mut skill_tool_results: Vec<OperationalToolResult> = Vec::new();
    let mut skill_tool_outputs: Vec<String> = Vec::new();
    let mut skill_tool_log: Vec<OperationalToolLogEntry> = Vec::new();
    let mut launched_agentic_task: Option<AgenticTaskLaunchedInfo> = None;
    for tr in &tool_requests {
        // Risk-based attachment policy: block high-risk tools on attachment turns,
        // allow read-only/low-risk tools so the agent can still act autonomously.
        if attachment_turn && should_block_tool_on_attachment_turn(&tr.name) {
            blocked_tool_count += 1;
            tracing::info!(
                agent = %id,
                tool = %tr.name,
                "operational_chat: blocked high-risk tool on attachment turn"
            );
            continue;
        }
        if tr.name == "store_secret"
            || tr.name == "store_provider_key"
            || tr.name == "store_vault_secret"
            || tr.name == "register_email_account"
            || tr.name == "launch_agentic_task"
        {
            continue; // handled in dedicated synthetic-tool blocks below
        }
        // Try to match against registered skill tools
        if let Some((skill_id, tool_desc)) =
            agentic::find_skill_for_tool(&state.skill_registry, &tr.name)
        {
            let skill_name = state
                .skill_registry
                .get_skill(&skill_id)
                .map(|(_, m, _)| m.name.clone())
                .unwrap_or_default();
            let mut tool_params = orion_skills::skill::ToolParams::new();
            if let serde_json::Value::Object(map) = &tr.arguments {
                for (k, v) in map {
                    tool_params = tool_params.with(k, v.clone());
                }
            } else {
                tracing::warn!(
                    tool = %tr.name,
                    args = %tr.arguments,
                    "operational_chat: tool arguments not a JSON object"
                );
            }
            let gate =
                orion_core::evaluate_tool_request(&tr.name, &tr.arguments, &capability_envelope);
            if !gate.allowed {
                let reason_code = gate.reason_code.unwrap_or("SAFETY_BLOCK");
                let reason_text = gate
                    .reason_text
                    .unwrap_or_else(|| "Blocked by capability safety policy".to_string());
                tracing::info!(
                    agent = %id,
                    tool = %tr.name,
                    reason_code = %reason_code,
                    reason = %reason_text,
                    "operational_chat: tool blocked by capability gate"
                );
                let output = format!(
                    "Blocked by safety policy ({}): {}",
                    reason_code, reason_text
                );
                skill_tool_outputs.push(format!("[{}] {}", tr.name, output));
                skill_tool_log.push(OperationalToolLogEntry {
                    tool_name: tr.name.clone(),
                    skill_name: Some(skill_name.clone()),
                    success: false,
                    output,
                });
                continue;
            }

            if tool_desc.requires_confirmation || gate.require_confirmation {
                let output = format!(
                    "Tool '{}' requires confirmation and was not executed in operational chat.",
                    tr.name
                );
                tracing::info!(
                    agent = %id,
                    tool = %tr.name,
                    "operational_chat: requires_confirmation tool blocked by server policy"
                );
                skill_tool_outputs.push(format!("[{}] {}", tr.name, output));
                skill_tool_log.push(OperationalToolLogEntry {
                    tool_name: tr.name.clone(),
                    skill_name: Some(skill_name.clone()),
                    success: false,
                    output,
                });
                continue;
            }

            tracing::info!(
                tool = %tr.name,
                skill = %skill_id,
                arg_keys = ?tool_params.values.keys().collect::<Vec<_>>(),
                "operational_chat: executing skill tool"
            );
            match state
                .skill_executor
                .execute(&skill_id, &tr.name, tool_params)
                .await
            {
                Ok(output) => {
                    tracing::info!(
                        tool = %tr.name,
                        skill = %skill_id,
                        "operational_chat: skill tool executed"
                    );
                    skill_tool_results.push(OperationalToolResult {
                        name: tr.name.clone(),
                        provider: skill_name.clone(),
                    });
                    if let Some(data) = &output.data {
                        let formatted_raw =
                            serde_json::to_string_pretty(data).unwrap_or_else(|_| data.to_string());
                        let formatted = redact_api_keys(&formatted_raw);
                        skill_tool_outputs.push(format!("[{}] {}", tr.name, formatted));
                        skill_tool_log.push(OperationalToolLogEntry {
                            tool_name: tr.name.clone(),
                            skill_name: Some(skill_name.clone()),
                            success: true,
                            output: formatted,
                        });
                    } else if let Some(err) = &output.error {
                        let err_text = redact_api_keys(&format!("Error: {}", err));
                        skill_tool_outputs.push(format!("[{}] {}", tr.name, err_text));
                        skill_tool_log.push(OperationalToolLogEntry {
                            tool_name: tr.name.clone(),
                            skill_name: Some(skill_name.clone()),
                            success: false,
                            output: err_text,
                        });
                    } else {
                        skill_tool_log.push(OperationalToolLogEntry {
                            tool_name: tr.name.clone(),
                            skill_name: Some(skill_name.clone()),
                            success: true,
                            output: "OK".to_string(),
                        });
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        tool = %tr.name,
                        skill = %skill_id,
                        error = %e,
                        "operational_chat: skill tool execution failed"
                    );
                    let err_text = redact_api_keys(&format!("Error: {}", e));
                    skill_tool_outputs.push(format!("[{}] {}", tr.name, err_text));
                    skill_tool_log.push(OperationalToolLogEntry {
                        tool_name: tr.name.clone(),
                        skill_name: Some(skill_name.clone()),
                        success: false,
                        output: err_text,
                    });
                }
            }
        }
    }

    // Execute synthetic configuration tools that need async APIs.
    for tr in &tool_requests {
        if tr.name == "register_email_account" {
            match serde_json::from_value::<routes::skills::RegisterEmailAccountRequest>(
                tr.arguments.clone(),
            ) {
                Ok(req) => {
                    match routes::skills::register_email_account_internal(
                        &id,
                        &state.skill_keychain,
                        &state.email_accounts,
                        req,
                    )
                    .await
                    {
                        Ok(outcome) => {
                            let probe_summary = outcome
                                .probes
                                .iter()
                                .map(|p| {
                                    format!(
                                        "{}:{} => {} ({})",
                                        p.host,
                                        p.port,
                                        if p.reachable { "ok" } else { "failed" },
                                        p.message
                                    )
                                })
                                .collect::<Vec<String>>()
                                .join("; ");
                            let output = format!(
                                "Registered account '{}' for {} (imap {}:{} / smtp {}:{}). Connectivity: {}",
                                outcome.account_id,
                                outcome.address,
                                outcome.imap_host,
                                outcome.imap_port,
                                outcome.smtp_host,
                                outcome.smtp_port,
                                probe_summary
                            );
                            skill_tool_outputs.push(format!("[{}] {}", tr.name, output));
                            skill_tool_log.push(OperationalToolLogEntry {
                                tool_name: tr.name.clone(),
                                skill_name: Some("Email".to_string()),
                                success: true,
                                output: output.clone(),
                            });
                            skill_tool_results.push(OperationalToolResult {
                                name: tr.name.clone(),
                                provider: outcome.account_id,
                            });
                        }
                        Err(e) => {
                            let output = format!("register_email_account failed: {}", e);
                            skill_tool_outputs.push(format!("[{}] {}", tr.name, output));
                            skill_tool_log.push(OperationalToolLogEntry {
                                tool_name: tr.name.clone(),
                                skill_name: Some("Email".to_string()),
                                success: false,
                                output,
                            });
                        }
                    }
                }
                Err(e) => {
                    let output = format!("register_email_account arguments invalid: {}", e);
                    skill_tool_outputs.push(format!("[{}] {}", tr.name, output));
                    skill_tool_log.push(OperationalToolLogEntry {
                        tool_name: tr.name.clone(),
                        skill_name: Some("Email".to_string()),
                        success: false,
                        output,
                    });
                }
            }
            continue;
        }

        if tr.name == "launch_agentic_task" {
            let goal = tr
                .arguments
                .get("goal")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim()
                .to_string();
            let context = tr
                .arguments
                .get("context")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim()
                .to_string();
            if goal.is_empty() {
                let output = "launch_agentic_task failed: goal is required.".to_string();
                skill_tool_outputs.push(format!("[{}] {}", tr.name, output));
                skill_tool_log.push(OperationalToolLogEntry {
                    tool_name: tr.name.clone(),
                    skill_name: None,
                    success: false,
                    output,
                });
                continue;
            }
            let merged_goal = if context.is_empty() {
                goal.clone()
            } else {
                format!("{}\n\nContext:\n{}", goal, context)
            };
            match launch_agentic_task_internal(
                &state,
                &id,
                AgenticRunRequest {
                    goal: merged_goal,
                    max_turns: 24,
                    auto_approve_safe_tools: false,
                    router_mode: requested_router_mode,
                },
                "chat_escalation".to_string(),
            )
            .await
            {
                Ok(task) => {
                    launched_agentic_task = Some(AgenticTaskLaunchedInfo {
                        task_id: task.task_id.clone(),
                        goal: goal.clone(),
                    });
                    let output = format!(
                        "Agentic task started (task_id: {}). Stream: {}",
                        task.task_id, task.stream_url
                    );
                    skill_tool_outputs.push(format!("[{}] {}", tr.name, output));
                    skill_tool_log.push(OperationalToolLogEntry {
                        tool_name: tr.name.clone(),
                        skill_name: None,
                        success: true,
                        output,
                    });
                }
                Err(e) => {
                    let output = format!("launch_agentic_task failed: {}", e);
                    skill_tool_outputs.push(format!("[{}] {}", tr.name, output));
                    skill_tool_log.push(OperationalToolLogEntry {
                        tool_name: tr.name.clone(),
                        skill_name: None,
                        success: false,
                        output,
                    });
                }
            }
        }
    }
    let skill_tool_result = skill_tool_results.into_iter().last();
    let skill_tool_output_text = if skill_tool_outputs.is_empty() {
        None
    } else {
        Some(skill_tool_outputs.join("\n\n"))
    };

    // Tool feedback loop: in Standard/Pro, feed tool output back for ONE follow-up LLM call.
    // Fast mode is intentionally single-pass for lower latency and simpler routing.
    let mut followup_content: Option<String> = None;
    if requested_router_mode != agentic::AgenticRouterMode::Auto {
        if let Some(ref output_text) = skill_tool_output_text {
            let feedback = format!("## Tool Results\n\n{}", output_text);
            messages.push(orion_capabilities::cognitive::Message::new(
                "assistant",
                &clean_content,
            ));
            messages.push(orion_capabilities::cognitive::Message::new(
                "user", &feedback,
            ));

            match router.route_with_tools(messages, tool_defs.clone()).await {
                Ok(followup_response) => {
                    let followup_extraction =
                        tool_extraction::extract_tool_calls(&followup_response);
                    followup_content = Some(followup_extraction.clean_content);
                }
                Err(e) => {
                    tracing::warn!("operational_chat: follow-up turn failed: {}", e);
                    // Fall back to original response — no followup_content set
                }
            }
        }
    } else {
        tracing::debug!("operational_chat: skipping follow-up tool synthesis in Fast mode");
    }

    // Blocking 2: execute credential tool requests, persist conversation with redacted content.
    let config_path_2 = config_path.clone();
    let raw_user_message = user_message.clone();
    let fast_tool_fallback = if requested_router_mode == agentic::AgenticRouterMode::Auto
        && followup_content.is_none()
        && looks_like_provisional_tool_message(&clean_content)
    {
        skill_tool_output_text
            .as_ref()
            .map(|output| format_fast_tool_reply(output))
    } else {
        None
    };
    let final_content = followup_content
        .clone()
        .or(fast_tool_fallback)
        .unwrap_or_else(|| clean_content.clone());
    let attachment_notice = if blocked_tool_count > 0 {
        Some(format!(
            "Attachment safety: {} high-risk tool(s) were blocked on this attachment turn. Read-only tools were allowed. Ask explicitly if you want me to execute write/shell actions.",
            blocked_tool_count
        ))
    } else {
        None
    };
    let mut final_content_for_storage = final_content;
    if let Some(note) = &attachment_notice {
        if !final_content_for_storage.trim().is_empty() {
            final_content_for_storage.push_str("\n\n");
        }
        final_content_for_storage.push_str(note);
    }
    let clean_content_clone = final_content_for_storage.clone();

    let (tool_result, final_providers) = tokio::task::spawn_blocking({
        let chat_path = chat_path.clone();
        let redacted_user = redacted_user_message.clone();
        let redacted_assistant = redact_api_keys(&final_content_for_storage);
        let tools = tool_requests.clone();
        let skill_output_text = skill_tool_output_text.clone();
        let allow_tool_execution = true; // credential tools are always allowed
        move || -> Result<(Option<OperationalToolResult>, Vec<String>), String> {
            let mut config =
                AppConfig::load(&config_path_2).map_err(|e| format!("Load config: {}", e))?;
            let mut keyring = ProviderKeyring::load(config.data_dir.clone())
                .unwrap_or_else(|_| ProviderKeyring::new(config.data_dir.clone()));
            let mut skill_kc = SkillKeychain::load(config.data_dir.clone())
                .unwrap_or_else(|_| SkillKeychain::new(config.data_dir.clone()));
            let mut tool_executed: Option<OperationalToolResult> = None;

            // Execute store_secret / store_provider_key / store_vault_secret tool calls from LLM
            if allow_tool_execution {
                for tr in &tools {
                    if tr.name == "store_secret" || tr.name == "store_provider_key" {
                        let provider = tr
                            .arguments
                            .get("provider")
                            .and_then(|v| v.as_str())
                            .unwrap_or("auto");
                        let key = tr
                            .arguments
                            .get("key")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        match execute_store_provider_key(
                            &mut keyring,
                            &mut config,
                            provider,
                            key,
                            Some(&mut skill_kc),
                        ) {
                            Ok(resolved) => {
                                tracing::info!(
                                    provider = %resolved,
                                    "operational_chat: stored secret via tool call"
                                );
                                tool_executed = Some(OperationalToolResult {
                                    name: tr.name.clone(),
                                    provider: resolved,
                                });
                            }
                            Err(e) => {
                                tracing::warn!("operational_chat: store_secret failed: {}", e);
                            }
                        }
                    } else if tr.name == "store_vault_secret" {
                        let key_name = tr
                            .arguments
                            .get("key")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        let value = tr
                            .arguments
                            .get("value")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        if !key_name.is_empty() && !value.is_empty() {
                            skill_kc.set_secret(key_name, value);
                            if let Err(e) = skill_kc.save() {
                                tracing::warn!(
                                    "operational_chat: store_vault_secret save failed: {}",
                                    e
                                );
                            } else {
                                tracing::info!(
                                    key = %key_name,
                                    "operational_chat: stored vault secret"
                                );
                                tool_executed = Some(OperationalToolResult {
                                    name: tr.name.clone(),
                                    provider: key_name.to_string(),
                                });
                            }
                        }
                    }
                }
            }

            // Auto-detect API keys in the raw user message if the LLM didn't emit tool calls
            if tool_executed.is_none() {
                let detected = extract_api_keys_from_text(&raw_user_message);
                for (provider, key_str) in detected {
                    match execute_store_provider_key(
                        &mut keyring,
                        &mut config,
                        provider,
                        &key_str,
                        Some(&mut skill_kc),
                    ) {
                        Ok(resolved) => {
                            tracing::info!(
                                provider = %resolved,
                                "operational_chat: auto-detected and stored API key"
                            );
                            tool_executed = Some(OperationalToolResult {
                                name: "store_secret".to_string(),
                                provider: resolved,
                            });
                        }
                        Err(e) => {
                            tracing::warn!(
                                provider = %provider,
                                "operational_chat: auto-detect store failed: {}",
                                e
                            );
                        }
                    }
                }
            }

            let providers: Vec<String> = provider_names_from_keyring(&keyring);

            // Persist conversation with redacted content (include skill output if any)
            let mut updated: Vec<BirthChatMessage> = if chat_path.exists() {
                let content =
                    std::fs::read_to_string(&chat_path).map_err(|e| format!("Read: {}", e))?;
                serde_json::from_str(&content).unwrap_or_default()
            } else {
                Vec::new()
            };
            updated.push(BirthChatMessage {
                role: "user".to_string(),
                content: redacted_user,
            });
            let mut assistant_msg = redacted_assistant;
            if let Some(ref output) = skill_output_text {
                assistant_msg.push_str(&format!("\n\n[Tool Result]\n{}", output));
            }
            updated.push(BirthChatMessage {
                role: "assistant".to_string(),
                content: assistant_msg,
            });
            std::fs::write(&chat_path, serde_json::to_string_pretty(&updated).unwrap())
                .map_err(|e| format!("Write operational_chat: {}", e))?;
            Ok((tool_executed, providers))
        }
    })
    .await
    .map_err(|e| ApiError::Internal(format!("Task join: {}", e)))?
    .map_err(ApiError::Internal)?;

    // Redact any credentials in the clean content sent back to the frontend.
    // Keep tool output out of chat bubbles and expose it via structured tool_log.
    let redacted_response = redact_api_keys(&clean_content_clone);

    // Prefer skill tool result over credential tool result
    let final_tool_result = skill_tool_result.or(tool_result);

    Ok(Json(OperationalChatResponseBody {
        assistant_content: redacted_response,
        tool_executed: final_tool_result,
        stored_providers: if final_providers.is_empty() {
            None
        } else {
            Some(final_providers)
        },
        tool_log: if skill_tool_log.is_empty() {
            None
        } else {
            Some(skill_tool_log)
        },
        attachment_notice,
        context_warning,
        auto_archived,
        routing_telemetry: Some(routing_telemetry),
        agentic_task_launched: launched_agentic_task,
    }))
}

/// POST /api/agents/{id}/chat/stream — SSE-streamed operational chat.
/// Emits status events during processing, then the final response as done.
async fn api_operational_chat_stream(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<OperationalChatRequest>,
) -> Result<Sse<impl futures_core::Stream<Item = Result<Event, std::convert::Infallible>>>, ApiError>
{
    // Validate agent exists
    let _ = agent_dir(&id).ok_or_else(|| ApiError::NotFound("Agent not found".to_string()))?;

    let (tx, rx) = tokio::sync::mpsc::channel::<ChatStreamEvent>(32);

    // Spawn the chat processing in a background task
    let state_clone = state.clone();
    let id_clone = id.clone();
    tokio::spawn(async move {
        let _ = tx
            .send(ChatStreamEvent::Status {
                phase: "routing".to_string(),
            })
            .await;

        // Delegate to the existing handler
        let result = api_operational_chat(State(state_clone), Path(id_clone), Json(body)).await;

        match result {
            Ok(Json(response)) => {
                if let Some(entries) = &response.tool_log {
                    for entry in entries {
                        let _ = tx
                            .send(ChatStreamEvent::ToolLog {
                                entry: serde_json::to_value(entry).unwrap_or_default(),
                            })
                            .await;
                    }
                }
                if let Some(launch) = &response.agentic_task_launched {
                    let _ = tx
                        .send(ChatStreamEvent::AgenticTaskLaunched {
                            task_id: launch.task_id.clone(),
                            goal: launch.goal.clone(),
                        })
                        .await;
                }
                let _ = tx
                    .send(ChatStreamEvent::Done {
                        response: serde_json::to_value(&response).unwrap_or_default(),
                    })
                    .await;
            }
            Err(err) => {
                let _ = tx
                    .send(ChatStreamEvent::Error {
                        message: err.to_string(),
                    })
                    .await;
            }
        }
    });

    let stream = async_stream::stream! {
        let mut rx = rx;
        while let Some(event) = rx.recv().await {
            let (event_name, data) = match &event {
                ChatStreamEvent::Status { phase } => {
                    ("status", serde_json::json!({"phase": phase}).to_string())
                }
                ChatStreamEvent::Token { text } => {
                    ("token", serde_json::json!({"text": text}).to_string())
                }
                ChatStreamEvent::ToolLog { entry } => {
                    ("tool_log", serde_json::to_string(entry).unwrap_or_default())
                }
                ChatStreamEvent::Done { response } => {
                    ("done", response.to_string())
                }
                ChatStreamEvent::AgenticTaskLaunched { task_id, goal } => (
                    "agentic_task_launched",
                    serde_json::json!({"task_id": task_id, "goal": goal}).to_string(),
                ),
                ChatStreamEvent::Error { message } => {
                    ("error", serde_json::json!({"message": message}).to_string())
                }
            };
            yield Ok(Event::default().event(event_name).data(data));
            if matches!(event, ChatStreamEvent::Done { .. } | ChatStreamEvent::Error { .. }) {
                break;
            }
        }
    };

    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}

#[derive(Debug, Clone)]
enum ChatStreamEvent {
    Status {
        phase: String,
    },
    #[allow(dead_code)]
    Token {
        text: String,
    },
    ToolLog {
        entry: serde_json::Value,
    },
    AgenticTaskLaunched {
        task_id: String,
        goal: String,
    },
    Done {
        response: serde_json::Value,
    },
    Error {
        message: String,
    },
}

/// Initialize the skill registry: instantiate all built-in skill plugins
/// and register them as Verified (first-party, shipped with the repo).
pub(crate) fn init_skill_registry(
    keychain: Arc<Mutex<SkillKeychain>>,
    email_accounts: Arc<tokio::sync::RwLock<Vec<orion_core::config::EmailAccountConfig>>>,
) -> Arc<SkillRegistry> {
    let registry = SkillRegistry::with_secrets(Arc::clone(&keychain));

    let mut registered = 0u32;
    let mut failed = 0u32;

    // Helper macro — uses the manifest's own ID as the registry key so
    // list_with_tiers() → get_skill(manifest.id) lookups are consistent.
    macro_rules! register_skill {
        ($skill:expr) => {{
            let skill_obj = $skill;
            let mid = skill_obj.manifest().id.clone();
            let id_str = mid.0.clone();
            match registry.register_with_tier(mid, Arc::new(skill_obj), TrustTier::Verified) {
                Ok(()) => {
                    tracing::info!(skill_id = %id_str, "Registered built-in skill (Verified)");
                    registered += 1;
                }
                Err(e) => {
                    tracing::warn!(skill_id = %id_str, error = %e, "Failed to register skill");
                    failed += 1;
                }
            }
        }};
    }

    // --- Simple skills (no secrets needed) ---

    // HTTP requests with SSRF protection
    register_skill!(skill_http::HttpSkill::new(
        skill_http::HttpSkill::default_manifest()
    ));

    // Shell command execution with safety blocklist
    register_skill!(skill_shell::ShellSkill::new(
        skill_shell::ShellSkill::default_manifest()
    ));

    // Cooperative install: direct install attempt with mentor script fallback
    register_skill!(skill_cooperative_install::CooperativeInstallSkill::new(
        skill_cooperative_install::CooperativeInstallSkill::default_manifest(),
    ));

    // Docker exec: dispatch commands to toolbox sidecar container
    register_skill!(skill_docker_exec::DockerExecSkill::new(
        skill_docker_exec::DockerExecSkill::default_manifest(),
    ));

    // Filesystem operations — sandbox to agent data dir
    let fs_roots = vec![
        data_root().unwrap_or_else(|| PathBuf::from(".")),
        std::env::temp_dir(),
    ];
    register_skill!(skill_filesystem::FilesystemSkill::new(
        skill_filesystem::FilesystemSkill::default_manifest(),
        fs_roots,
    ));

    // --- Keychain-backed skills (secrets loaded at execution time) ---

    // Web search via Tavily
    register_skill!(skill_web_search::WebSearchSkill::with_secrets(
        skill_web_search::WebSearchSkill::default_manifest(),
        Arc::clone(&keychain),
    ));

    // Web browsing (HTTP fetch + optional Tavily/Perplexity fallback)
    register_skill!(skill_web_browse::WebBrowseSkill::with_secrets(
        skill_web_browse::WebBrowseSkill::default_manifest(),
        Arc::clone(&keychain),
    ));

    // Perplexity AI search
    register_skill!(
        skill_perplexity_search::PerplexitySearchSkill::with_secrets(
            skill_perplexity_search::PerplexitySearchSkill::default_manifest(),
            Arc::clone(&keychain),
        )
    );

    // Generic Email (IMAP/SMTP — supports multiple accounts/providers via config)
    register_skill!(skill_email::EmailSkill::with_secrets(
        skill_email::EmailSkill::default_manifest(),
        Arc::clone(&keychain),
        Arc::clone(&email_accounts),
    ));

    tracing::info!(
        registered = registered,
        failed = failed,
        "Skill registry initialized"
    );

    Arc::new(registry)
}

pub(crate) async fn has_active_agentic_task(state: &AppState, agent_id: &str) -> bool {
    let tasks = state.agentic_tasks.lock().await;
    for task_arc in tasks.values() {
        let task = task_arc.lock().await;
        if task.agent_id == agent_id
            && matches!(
                task.status,
                AgenticTaskStatus::Running
                    | AgenticTaskStatus::WaitingForMentor
                    | AgenticTaskStatus::WaitingForConfirmation
            )
        {
            return true;
        }
    }
    false
}

pub(crate) async fn launch_agentic_task_internal(
    state: &AppState,
    agent_id: &str,
    body: AgenticRunRequest,
    run_source: String,
) -> Result<AgenticRunResponse, String> {
    let dir = agent_dir(agent_id).ok_or_else(|| "Agent not found".to_string())?;
    let config_path = agent_config_path(agent_id).ok_or_else(|| "Agent not found".to_string())?;
    let config = AppConfig::load(&config_path).map_err(|e| format!("Load config: {}", e))?;
    if !config.birth_complete {
        return Err("Agent birth must be complete before agentic mode".to_string());
    }

    let goal = body.goal.trim().to_string();
    if goal.is_empty() {
        return Err("goal is required".to_string());
    }

    let max_turns = body.max_turns.clamp(1, 50);

    if has_active_agentic_task(state, agent_id).await {
        return Err("Agent already has a running agentic task".to_string());
    }

    let task_id = Uuid::new_v4().to_string();
    let (event_tx, _) = tokio::sync::broadcast::channel::<AgenticEvent>(256);
    let (cancel_tx, cancel_rx) = tokio::sync::mpsc::channel::<()>(1);
    let started_at = chrono::Utc::now();

    let task = AgenticTask {
        id: task_id.clone(),
        agent_id: agent_id.to_string(),
        goal: goal.clone(),
        status: AgenticTaskStatus::Running,
        event_tx: event_tx.clone(),
        mentor_response_tx: None,
        confirmation_tx: None,
        steps: Vec::new(),
        turn: 0,
        tool_calls: 0,
        cancel_tx,
        started_at,
        completed_at: None,
    };

    let task_arc = Arc::new(TokioMutex::new(task));
    {
        let mut tasks = state.agentic_tasks.lock().await;
        tasks.insert(task_id.clone(), Arc::clone(&task_arc));
    }

    // Sync email accounts from agent config.
    routes::skills::sync_email_accounts(&config, &state.email_accounts, &state.skill_keychain)
        .await;

    // Load persisted agent-registered MCP servers into the shared registry.
    for mcp_def in &config.mcp_servers {
        if mcp_def.transport == "http" && !mcp_def.command_or_url.is_empty() {
            let sid = orion_skills::manifest::SkillId(mcp_def.id.clone());
            // Skip if already registered (e.g. from a previous agent load in the same process).
            if state.skill_registry.get_skill(&sid).is_ok() {
                continue;
            }
            let mut runtime = orion_skills::protocol::mcp::McpSkillRuntime::new(
                &mcp_def.id,
                &mcp_def.name,
                &mcp_def.command_or_url,
            );
            let skill_cfg = orion_skills::skill::SkillConfig {
                values: Default::default(),
                secrets: Default::default(),
                limits: orion_skills::ResourceLimits::default(),
                permissions: vec![],
                event_sender: None,
            };
            match runtime.initialize(skill_cfg).await {
                Ok(()) => {
                    if let Err(e) = state.skill_registry.register_with_tier(
                        sid,
                        Arc::new(runtime),
                        TrustTier::AgentBuilt,
                    ) {
                        tracing::warn!(
                            mcp_id = %mcp_def.id,
                            error = %e,
                            "Failed to register persisted MCP skill"
                        );
                    } else {
                        tracing::info!(
                            mcp_id = %mcp_def.id,
                            url = %mcp_def.command_or_url,
                            "Loaded persisted MCP skill (AgentBuilt)"
                        );
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        mcp_id = %mcp_def.id,
                        url = %mcp_def.command_or_url,
                        error = %e,
                        "Failed to initialize persisted MCP skill (server may be offline)"
                    );
                }
            }
        }
    }

    let skill_tool_entries = build_skill_tool_entries(&state.skill_registry);
    let stored_providers: Vec<String> = {
        let keyring = ProviderKeyring::load(config.data_dir.clone())
            .unwrap_or_else(|_| ProviderKeyring::new(config.data_dir.clone()));
        provider_names_from_keyring(&keyring)
    };

    let loop_config = AgenticLoopConfig {
        task_id: task_id.clone(),
        agent_id: agent_id.to_string(),
        goal,
        max_turns,
        auto_approve_safe_tools: body.auto_approve_safe_tools,
        router_mode: body.router_mode,
        agent_dir: dir,
        config,
        skill_registry: Arc::clone(&state.skill_registry),
        skill_executor: Arc::clone(&state.skill_executor),
        provider_keyring: Arc::clone(&state.provider_keyring),
        skill_keychain: Arc::clone(&state.skill_keychain),
        email_accounts: Arc::clone(&state.email_accounts),
        skill_tool_entries,
        stored_providers,
        event_tx,
        cancel_rx,
        task_handle: task_arc,
        started_at,
        run_source,
        superego_l2_mode: state.superego_l2_mode,
        use_council: body.router_mode == agentic::AgenticRouterMode::ThinkHarder,
    };

    tokio::spawn(agentic::run_agentic_loop(loop_config));
    let stream_url = format!("/api/agents/{}/agent/stream?task={}", agent_id, task_id);
    Ok(AgenticRunResponse {
        task_id,
        stream_url,
    })
}

/// Resolve connected providers for council execution (priority-ordered).
pub(crate) fn resolve_council_providers(
    keyring: &ProviderKeyring,
    preference: Option<&str>,
) -> Vec<(String, String)> {
    let mut result: Vec<(String, String)> = Vec::new();

    // If preference is set and has key, add it first.
    if let Some(pref) = preference {
        let normalized = AppConfig::normalize_provider_name(pref);
        if provider_supports_tier_models(&normalized) {
            if let Some(key) = keyring.get_key_str(&normalized) {
                result.push((normalized, key.to_string()));
            }
        }
    }

    // Add preferred providers first.
    let preferred = ["anthropic", "openai", "google", "xai", "perplexity"];
    for pref in &preferred {
        if result.iter().any(|(n, _)| n == *pref) {
            continue;
        }
        if let Some(key) = keyring.get_key_str(pref) {
            result.push((pref.to_string(), key.to_string()));
            if result.len() >= 3 {
                break;
            }
        }
    }

    // Fill remaining from keyring.
    if result.len() < 3 {
        for p in provider_names_from_keyring(keyring) {
            let normalized = AppConfig::normalize_provider_name(&p);
            if !provider_supports_tier_models(&normalized) {
                continue;
            }
            if result.iter().any(|(n, _)| *n == normalized) {
                continue;
            }
            if let Some(key) = keyring.get_key_str(&p) {
                result.push((normalized, key.to_string()));
                if result.len() >= 3 {
                    break;
                }
            }
        }
    }

    result
}

pub(crate) fn append_operational_notice(
    agent_dir: &std::path::Path,
    content: &str,
) -> Result<(), String> {
    let chat_path = agent_dir.join("operational_chat.json");
    let mut updated: Vec<BirthChatMessage> = if chat_path.exists() {
        let existing = std::fs::read_to_string(&chat_path).map_err(|e| format!("Read: {}", e))?;
        serde_json::from_str(&existing).unwrap_or_default()
    } else {
        Vec::new()
    };
    updated.push(BirthChatMessage {
        role: "assistant".to_string(),
        content: content.to_string(),
    });
    std::fs::write(&chat_path, serde_json::to_string_pretty(&updated).unwrap())
        .map_err(|e| format!("Write operational_chat: {}", e))
}

async fn run_id_check_for_job(
    config: &AppConfig,
    job: &OrchestrationJob,
) -> Result<String, String> {
    let router = orion_router::IdEgoRouter::with_provider_auto_detect(
        config.local_llm_base_url.clone(),
        None,
        None,
        None,
        RoutingMode::IdPrimary,
    )
    .await;
    let prompt = build_id_check_prompt(job);
    let response = router
        .id_only(vec![orion_capabilities::cognitive::Message::new(
            "user", &prompt,
        )])
        .await
        .map_err(|e| format!("Id check failed: {}", e))?;
    Ok(redact_api_keys(&response.content))
}

async fn run_orchestration_job_once(
    state: &AppState,
    agent_id: &str,
    agent_dir: &std::path::Path,
    job: &mut OrchestrationJob,
    trigger: &str,
) -> Result<JobRunNowResponse, String> {
    let started_at = chrono::Utc::now();
    let config_path = agent_config_path(agent_id).ok_or_else(|| "Agent not found".to_string())?;
    let config = AppConfig::load(&config_path).map_err(|e| format!("Load config: {}", e))?;

    let mut summary = String::new();
    let mut significance = SignificanceLevel::Low;
    let mut decision = OrchestrationDecision::SilentLog;
    let mut status = "ok".to_string();
    let mut task_id: Option<String> = None;

    match job.mode {
        orchestration::OrchestrationJobMode::AgenticRun => {
            significance = SignificanceLevel::Medium;
            decision = OrchestrationDecision::SpawnAgentic;
            if has_active_agentic_task(state, agent_id).await {
                status = "skipped_active_task".to_string();
                summary = "Agent already has a running task; skipped scheduled run.".to_string();
            } else {
                match launch_agentic_task_internal(
                    state,
                    agent_id,
                    AgenticRunRequest {
                        goal: job.goal_template.clone(),
                        max_turns: 12,
                        auto_approve_safe_tools: false,
                        router_mode: agentic::AgenticRouterMode::Auto,
                    },
                    format!("scheduled:{}", job.job_id),
                )
                .await
                {
                    Ok(resp) => {
                        task_id = Some(resp.task_id.clone());
                        summary = format!(
                            "Scheduled job launched agentic task from trigger '{}' ({})",
                            trigger, resp.task_id
                        );
                    }
                    Err(e) => {
                        status = "error".to_string();
                        summary = e;
                    }
                }
            }
        }
        orchestration::OrchestrationJobMode::IdCheck => {
            let id_output = match run_id_check_for_job(&config, job).await {
                Ok(output) => output,
                Err(e) => {
                    status = "error".to_string();
                    summary = e;
                    String::new()
                }
            };
            if status == "ok" {
                summary = id_output.clone();
                significance = assess_significance(&id_output, &job.significance_policy);
                decision = decide_action(job.mode, significance, &job.significance_policy);
                match decision {
                    OrchestrationDecision::SilentLog => {}
                    OrchestrationDecision::SpawnAgentic => {
                        if has_active_agentic_task(state, agent_id).await {
                            status = "skipped_active_task".to_string();
                        } else {
                            match launch_agentic_task_internal(
                                state,
                                agent_id,
                                AgenticRunRequest {
                                    goal: format!(
                                        "Investigate scheduled finding from '{}':\n\n{}",
                                        job.name, id_output
                                    ),
                                    max_turns: 10,
                                    auto_approve_safe_tools: false,
                                    router_mode: agentic::AgenticRouterMode::Auto,
                                },
                                format!("scheduled:{}", job.job_id),
                            )
                            .await
                            {
                                Ok(resp) => task_id = Some(resp.task_id),
                                Err(e) => {
                                    status = "error".to_string();
                                    summary =
                                        format!("{}\n\nEscalation launch failed: {}", summary, e);
                                }
                            }
                        }
                    }
                    OrchestrationDecision::FlagMentor => {
                        let attention =
                            make_mentor_attention_message(job, significance, &id_output);
                        let _ = append_operational_notice(agent_dir, &attention);
                    }
                }
            }
        }
    }

    let finished_at = chrono::Utc::now();
    update_job_after_execution(job, finished_at, significance, status.clone());
    let log_entry = OrchestrationJobLogEntry {
        entry_id: Uuid::new_v4().to_string(),
        job_id: job.job_id.clone(),
        job_name: job.name.clone(),
        mode: job.mode,
        started_at,
        completed_at: finished_at,
        significance,
        decision,
        status: status.clone(),
        summary: summary.clone(),
        task_id: task_id.clone(),
    };
    append_log_entry(agent_dir, log_entry, 200)?;

    Ok(JobRunNowResponse {
        job_id: job.job_id.clone(),
        status,
        decision,
        significance,
        task_id,
        summary,
    })
}

async fn run_orchestration_scheduler_loop(state: AppState) {
    let mut interval = tokio::time::interval(std::time::Duration::from_secs(
        state.orchestration_tick_seconds,
    ));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        interval.tick().await;
        let Some(root) = data_root() else {
            continue;
        };
        let identities_dir = root.join("identities");
        let Ok(entries) = std::fs::read_dir(&identities_dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let agent_dir = entry.path();
            if !agent_dir.is_dir() {
                continue;
            }
            let agent_id = entry.file_name().to_string_lossy().to_string();
            let mut jobs = match list_jobs(&agent_dir) {
                Ok(j) => j,
                Err(e) => {
                    tracing::warn!(agent = %agent_id, error = %e, "scheduler: failed to load jobs");
                    continue;
                }
            };
            if jobs.is_empty() {
                continue;
            }
            let now = chrono::Utc::now();
            let mut changed = false;
            for job in jobs.iter_mut().filter(|j| j.enabled) {
                let due = match is_job_due(job, now) {
                    Ok(d) => d,
                    Err(e) => {
                        tracing::warn!(
                            agent = %agent_id,
                            job = %job.job_id,
                            error = %e,
                            "scheduler: invalid cron expression"
                        );
                        continue;
                    }
                };
                if !due {
                    continue;
                }
                match run_orchestration_job_once(&state, &agent_id, &agent_dir, job, "scheduled")
                    .await
                {
                    Ok(resp) => {
                        tracing::info!(
                            agent = %agent_id,
                            job = %job.job_id,
                            decision = ?resp.decision,
                            significance = ?resp.significance,
                            status = %resp.status,
                            "scheduler: job executed"
                        );
                        changed = true;
                    }
                    Err(e) => {
                        tracing::warn!(
                            agent = %agent_id,
                            job = %job.job_id,
                            error = %e,
                            "scheduler: job execution failed"
                        );
                        changed = true;
                    }
                }
            }
            if changed {
                if let Err(e) = save_jobs(&agent_dir, &jobs) {
                    tracing::warn!(
                        agent = %agent_id,
                        error = %e,
                        "scheduler: failed to persist jobs"
                    );
                }
            }
        }
    }
}

/// GET /api/proxy/status — inspect proxy mode and recent logs.
async fn api_proxy_status() -> Result<Json<ProxyStatusResponse>, ApiError> {
    let allowlist_path = proxy_allowlist_path();
    let log_dir = proxy_log_dir();
    let internal_log_path = log_dir.join("internal").join("access.log");
    let external_log_path = log_dir.join("external").join("access.log");
    Ok(Json(ProxyStatusResponse {
        mode: std::env::var("PROXY_MODE").unwrap_or_else(|_| "allow_all".to_string()),
        allow_host_docker_internal: parse_bool_env("PROXY_ALLOW_HOST_DOCKER_INTERNAL", true),
        allowlist_path: allowlist_path.display().to_string(),
        internal_log_path: internal_log_path.display().to_string(),
        external_log_path: external_log_path.display().to_string(),
        internal_log_exists: internal_log_path.exists(),
        external_log_exists: external_log_path.exists(),
        internal_log_tail: read_log_tail(&internal_log_path, 40),
        external_log_tail: read_log_tail(&external_log_path, 40),
    }))
}

/// GET /api/proxy/allowlist — read the external proxy domain allowlist.
async fn api_proxy_allowlist_get() -> Result<Json<ProxyAllowlistResponse>, ApiError> {
    let path = proxy_allowlist_path();
    Ok(Json(ProxyAllowlistResponse {
        path: path.display().to_string(),
        content: read_file_or_empty(&path),
    }))
}

/// PUT /api/proxy/allowlist — update the external proxy domain allowlist.
async fn api_proxy_allowlist_put(
    Json(body): Json<ProxyAllowlistUpdateRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if !body.mentor_approved {
        return Err(ApiError::Forbidden(
            "mentor_approved=true is required for proxy allowlist updates".to_string(),
        ));
    }
    let path = proxy_allowlist_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| ApiError::Internal(format!("Create allowlist directory failed: {}", e)))?;
    }
    std::fs::write(&path, body.content.as_bytes())
        .map_err(|e| ApiError::Internal(format!("Write allowlist failed: {}", e)))?;

    // Best-effort live reload for local/docker dev setups.
    let reload_result = std::process::Command::new("sh")
        .arg("-lc")
        .arg("docker compose -f docker/docker-compose.yml exec -T proxy_external squid -k reconfigure")
        .output()
        .ok()
        .map(|out| {
            if out.status.success() {
                "proxy_external reconfigured".to_string()
            } else {
                let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
                if stderr.is_empty() {
                    "allowlist updated; proxy reload command failed".to_string()
                } else {
                    format!("allowlist updated; proxy reload command failed: {}", stderr)
                }
            }
        })
        .unwrap_or_else(|| "allowlist updated; proxy reload command unavailable".to_string());

    Ok(Json(serde_json::json!({
        "ok": true,
        "path": path.display().to_string(),
        "reload": reload_result
    })))
}

/// GET /api/proxy/logs?lines=N — read recent internal/external proxy logs.
async fn api_proxy_logs(
    Query(query): Query<ProxyLogsQuery>,
) -> Result<Json<ProxyLogsResponse>, ApiError> {
    let lines = query.lines.unwrap_or(80).clamp(1, 500);
    let log_dir = proxy_log_dir();
    let internal_log = log_dir.join("internal").join("access.log");
    let external_log = log_dir.join("external").join("access.log");
    Ok(Json(ProxyLogsResponse {
        lines,
        internal: read_log_tail(&internal_log, lines),
        external: read_log_tail(&external_log, lines),
    }))
}

/// GET /api/proxy/status — inspect proxy mode and recent logs.
async fn api_proxy_status() -> Result<Json<ProxyStatusResponse>, (axum::http::StatusCode, String)> {
    let allowlist_path = proxy_allowlist_path();
    let log_dir = proxy_log_dir();
    let internal_log_path = log_dir.join("internal").join("access.log");
    let external_log_path = log_dir.join("external").join("access.log");
    Ok(Json(ProxyStatusResponse {
        mode: std::env::var("PROXY_MODE").unwrap_or_else(|_| "allow_all".to_string()),
        allow_host_docker_internal: parse_bool_env("PROXY_ALLOW_HOST_DOCKER_INTERNAL", true),
        allowlist_path: allowlist_path.display().to_string(),
        internal_log_path: internal_log_path.display().to_string(),
        external_log_path: external_log_path.display().to_string(),
        internal_log_exists: internal_log_path.exists(),
        external_log_exists: external_log_path.exists(),
        internal_log_tail: read_log_tail(&internal_log_path, 40),
        external_log_tail: read_log_tail(&external_log_path, 40),
    }))
}

/// GET /api/proxy/allowlist — read the external proxy domain allowlist.
async fn api_proxy_allowlist_get(
) -> Result<Json<ProxyAllowlistResponse>, (axum::http::StatusCode, String)> {
    let path = proxy_allowlist_path();
    Ok(Json(ProxyAllowlistResponse {
        path: path.display().to_string(),
        content: read_file_or_empty(&path),
    }))
}

/// PUT /api/proxy/allowlist — update the external proxy domain allowlist.
async fn api_proxy_allowlist_put(
    Json(body): Json<ProxyAllowlistUpdateRequest>,
) -> Result<Json<serde_json::Value>, (axum::http::StatusCode, String)> {
    if !body.mentor_approved {
        return Err((
            axum::http::StatusCode::FORBIDDEN,
            "mentor_approved=true is required for proxy allowlist updates".to_string(),
        ));
    }
    let path = proxy_allowlist_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("Create allowlist directory failed: {}", e),
            )
        })?;
    }
    std::fs::write(&path, body.content.as_bytes()).map_err(|e| {
        (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("Write allowlist failed: {}", e),
        )
    })?;

    // Best-effort live reload for local/docker dev setups.
    let reload_result = std::process::Command::new("sh")
        .arg("-lc")
        .arg("docker compose -f docker/docker-compose.yml exec -T proxy_external squid -k reconfigure")
        .output()
        .ok()
        .map(|out| {
            if out.status.success() {
                "proxy_external reconfigured".to_string()
            } else {
                let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
                if stderr.is_empty() {
                    "allowlist updated; proxy reload command failed".to_string()
                } else {
                    format!("allowlist updated; proxy reload command failed: {}", stderr)
                }
            }
        })
        .unwrap_or_else(|| "allowlist updated; proxy reload command unavailable".to_string());

    Ok(Json(serde_json::json!({
        "ok": true,
        "path": path.display().to_string(),
        "reload": reload_result
    })))
}

/// GET /api/proxy/logs?lines=N — read recent internal/external proxy logs.
async fn api_proxy_logs(
    Query(query): Query<ProxyLogsQuery>,
) -> Result<Json<ProxyLogsResponse>, (axum::http::StatusCode, String)> {
    let lines = query.lines.unwrap_or(80).clamp(1, 500);
    let log_dir = proxy_log_dir();
    let internal_log = log_dir.join("internal").join("access.log");
    let external_log = log_dir.join("external").join("access.log");
    Ok(Json(ProxyLogsResponse {
        lines,
        internal: read_log_tail(&internal_log, lines),
        external: read_log_tail(&external_log, lines),
    }))
}

// ============================================================================
// Orchestration Jobs Endpoints
// ============================================================================

/// GET /api/agents/{id}/orchestration/jobs — list scheduled orchestration jobs.
async fn api_orchestration_jobs(
    Path(id): Path<String>,
) -> Result<Json<OrchestrationJobsResponse>, ApiError> {
    let dir = agent_dir(&id).ok_or_else(|| ApiError::NotFound("Agent not found".to_string()))?;
    let mut jobs = list_jobs(&dir).map_err(ApiError::Internal)?;
    jobs.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(Json(OrchestrationJobsResponse { jobs }))
}

/// POST /api/agents/{id}/orchestration/jobs — create a scheduled orchestration job.
async fn api_orchestration_job_create(
    Path(id): Path<String>,
    Json(body): Json<CreateOrchestrationJobRequest>,
) -> Result<Json<OrchestrationJob>, ApiError> {
    let dir = agent_dir(&id).ok_or_else(|| ApiError::NotFound("Agent not found".to_string()))?;
    let mut jobs = list_jobs(&dir).map_err(ApiError::Internal)?;
    let job = create_job(&mut jobs, body, chrono::Utc::now()).map_err(ApiError::BadRequest)?;
    save_jobs(&dir, &jobs).map_err(ApiError::Internal)?;
    Ok(Json(job))
}

/// POST /api/agents/{id}/orchestration/jobs/{job_id} — update an orchestration job.
async fn api_orchestration_job_update(
    Path((id, job_id)): Path<(String, String)>,
    Json(body): Json<UpdateOrchestrationJobRequest>,
) -> Result<Json<OrchestrationJob>, ApiError> {
    let dir = agent_dir(&id).ok_or_else(|| ApiError::NotFound("Agent not found".to_string()))?;
    let mut jobs = list_jobs(&dir).map_err(ApiError::Internal)?;
    let updated = update_job(&mut jobs, &job_id, body, chrono::Utc::now()).map_err(|e| {
        if e.contains("not found") {
            ApiError::NotFound(e)
        } else {
            ApiError::BadRequest(e)
        }
    })?;
    save_jobs(&dir, &jobs).map_err(ApiError::Internal)?;
    Ok(Json(updated))
}

/// POST /api/agents/{id}/orchestration/jobs/{job_id}/enable — enable/disable a job.
async fn api_orchestration_job_enable(
    Path((id, job_id)): Path<(String, String)>,
    Json(body): Json<SetOrchestrationJobEnabledRequest>,
) -> Result<Json<OrchestrationJob>, ApiError> {
    let dir = agent_dir(&id).ok_or_else(|| ApiError::NotFound("Agent not found".to_string()))?;
    let mut jobs = list_jobs(&dir).map_err(ApiError::Internal)?;
    let updated =
        set_job_enabled(&mut jobs, &job_id, body.enabled, chrono::Utc::now()).map_err(|e| {
            if e.contains("not found") {
                ApiError::NotFound(e)
            } else {
                ApiError::BadRequest(e)
            }
        })?;
    save_jobs(&dir, &jobs).map_err(ApiError::Internal)?;
    Ok(Json(updated))
}

/// POST /api/agents/{id}/orchestration/jobs/{job_id}/delete — delete an orchestration job.
async fn api_orchestration_job_delete(
    Path((id, job_id)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let dir = agent_dir(&id).ok_or_else(|| ApiError::NotFound("Agent not found".to_string()))?;
    let mut jobs = list_jobs(&dir).map_err(ApiError::Internal)?;
    if !delete_job(&mut jobs, &job_id) {
        return Err(ApiError::NotFound("Job not found".to_string()));
    }
    save_jobs(&dir, &jobs).map_err(ApiError::Internal)?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

/// POST /api/agents/{id}/orchestration/jobs/{job_id}/run — run one job immediately.
async fn api_orchestration_job_run_now(
    State(state): State<AppState>,
    Path((id, job_id)): Path<(String, String)>,
) -> Result<Json<JobRunNowResponse>, ApiError> {
    let dir = agent_dir(&id).ok_or_else(|| ApiError::NotFound("Agent not found".to_string()))?;
    let mut jobs = list_jobs(&dir).map_err(ApiError::Internal)?;
    let idx = jobs
        .iter()
        .position(|j| j.job_id == job_id)
        .ok_or_else(|| ApiError::NotFound("Job not found".to_string()))?;

    let mut job = jobs.remove(idx);
    let run_result = run_orchestration_job_once(&state, &id, &dir, &mut job, "manual")
        .await
        .map_err(ApiError::Internal)?;
    jobs.insert(idx, job);
    save_jobs(&dir, &jobs).map_err(ApiError::Internal)?;
    Ok(Json(run_result))
}

/// GET /api/agents/{id}/orchestration/logs — fetch recent orchestration logs.
async fn api_orchestration_logs(
    Path(id): Path<String>,
    Query(query): Query<JobLogsQuery>,
) -> Result<Json<OrchestrationLogsResponse>, ApiError> {
    let dir = agent_dir(&id).ok_or_else(|| ApiError::NotFound("Agent not found".to_string()))?;
    let limit = query.limit.or(Some(50));
    let logs = load_logs(&dir, limit).map_err(ApiError::Internal)?;
    Ok(Json(OrchestrationLogsResponse { logs }))
}

/// Periodic cleanup of terminal agentic tasks (completed/failed/cancelled) that have
/// been sitting in memory for longer than 10 minutes. Prevents unbounded growth of
/// the in-memory task map.
async fn agentic_task_cleanup_loop(
    tasks: Arc<TokioMutex<HashMap<String, Arc<TokioMutex<AgenticTask>>>>>,
) {
    let mut interval = tokio::time::interval(std::time::Duration::from_secs(120));
    loop {
        interval.tick().await;
        let mut map = tasks.lock().await;
        let mut to_remove = Vec::new();
        for (id, task_arc) in map.iter() {
            let task = task_arc.lock().await;
            if task.is_terminal()
                && task.completed_at_elapsed() > std::time::Duration::from_secs(600)
            {
                to_remove.push(id.clone());
            }
        }
        for id in &to_remove {
            map.remove(id);
        }
        if !to_remove.is_empty() {
            tracing::debug!("Cleaned up {} terminal agentic tasks", to_remove.len());
        }
    }
}

/// Bearer-token authentication middleware.
/// When ORION_API_TOKEN is set, all /api/* requests require a matching
/// Authorization: Bearer header.
/// /health and /ready are exempt.
async fn auth_middleware(
    req: axum::http::Request<axum::body::Body>,
    next: axum::middleware::Next,
) -> Result<axum::response::Response, axum::http::StatusCode> {
    static TOKEN: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    let expected = TOKEN.get_or_init(|| {
        let t = std::env::var("ORION_API_TOKEN")
            .ok()
            .filter(|s| !s.is_empty());
        if t.is_none() {
            let in_container = std::env::var("ORION_CONTAINER")
                .ok()
                .filter(|s| !s.is_empty())
                .is_some();
            if in_container {
                tracing::error!(
                    "ORION_API_TOKEN not set in container — API will reject all requests"
                );
            } else {
                tracing::warn!("ORION_API_TOKEN not set — API is unauthenticated (dev mode)");
            }
        }
        t
    });
    let Some(expected_token) = expected else {
        return Ok(next.run(req).await); // no token configured → pass through
    };
    let path = req.uri().path();
    if path == "/health" || path == "/ready" {
        return Ok(next.run(req).await); // exempt health endpoints
    }
    // Check Authorization: Bearer header
    let authorized = req
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "))
        .map(|t| t == expected_token.as_str())
        .unwrap_or(false);
    if authorized {
        Ok(next.run(req).await)
    } else {
        Err(axum::http::StatusCode::UNAUTHORIZED)
    }
}

#[tokio::main]
async fn main() -> std::io::Result<()> {
    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::new(
            std::env::var("RUST_LOG").unwrap_or_else(|_| {
                "orion_api=info,orion_router=info,orion_birth=info,orion_capabilities=warn,tower_http=info".into()
            }),
        ))
        .with(tracing_subscriber::fmt::layer())
        .init();

    // Enforce API token in container deployments — refuse to start unauthenticated.
    let in_container = std::env::var("ORION_CONTAINER")
        .ok()
        .filter(|s| !s.is_empty())
        .is_some();
    let has_token = std::env::var("ORION_API_TOKEN")
        .ok()
        .filter(|s| !s.is_empty())
        .is_some();
    if in_container && !has_token {
        tracing::error!("FATAL: ORION_API_TOKEN must be set when running in a container. Exiting.");
        std::process::exit(1);
    }

    let memory_backend_str = std::env::var("MEMORY_BACKEND")
        .ok()
        .unwrap_or_else(|| "sqlite".to_string());
    let memory_backend = MemoryBackend::from_str(&memory_backend_str).unwrap_or_default();

    let birth_keys = HashMap::new();
    let birth_keys = std::sync::Arc::new(Mutex::new(birth_keys));

    // Restore any persisted signing keys from disk (survives server restarts during birth)
    if let Some(root) = data_root() {
        let identities_dir = root.join("identities");
        if let Ok(entries) = std::fs::read_dir(&identities_dir) {
            for entry in entries.flatten() {
                let dir = entry.path();
                if dir.is_dir() {
                    if let Ok(Some(bytes)) = orion_core::load_signing_key(&dir) {
                        let id = entry.file_name().to_string_lossy().to_string();
                        tracing::info!("Restored persisted signing key for agent {}", id);
                        birth_keys.lock().unwrap().insert(id, bytes);
                    }
                }
            }
        }
    }

    // Run config key migration to provider keyring for active agent(s).
    if let Some(config_path) = resolve_config_path() {
        if let Some(agent_dir) = config_path.parent().map(|d| d.to_path_buf()) {
            if let Ok(mut config) = AppConfig::load(&config_path) {
                let mut keyring = ProviderKeyring::load(agent_dir.clone())
                    .unwrap_or_else(|_| ProviderKeyring::new(agent_dir));
                let key_migrated = config.migrate_keys_to_keyring(&mut keyring);
                if !key_migrated.is_empty() {
                    if let Err(e) = keyring.save() {
                        tracing::error!("Failed to save keyring after migration: {}", e);
                    }
                    if let Err(e) = config.save(&config_path) {
                        tracing::error!("Failed to save config after key migration: {}", e);
                    }
                    tracing::info!(
                        "Key migration complete: {} config keys migrated to keyring",
                        key_migrated.len()
                    );
                }
            }
        }
    }

    // Load the provider keyring and skill keychain for the current agent.
    let keyring_dir = resolve_config_path()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .or_else(data_root)
        .unwrap_or_else(|| PathBuf::from("."));
    let provider_keyring = Arc::new(RwLock::new(
        ProviderKeyring::load(keyring_dir.clone())
            .unwrap_or_else(|_| ProviderKeyring::new(keyring_dir.clone())),
    ));
    let skill_keychain = Arc::new(Mutex::new(
        SkillKeychain::load(keyring_dir.clone())
            .unwrap_or_else(|_| SkillKeychain::new(keyring_dir)),
    ));

    // Shared email accounts list — synced from agent config per request
    let email_accounts: Arc<tokio::sync::RwLock<Vec<orion_core::config::EmailAccountConfig>>> =
        Arc::new(tokio::sync::RwLock::new(Vec::new()));

    // Initialize skill registry and executor
    let skill_registry =
        init_skill_registry(Arc::clone(&skill_keychain), Arc::clone(&email_accounts));
    let skill_executor = Arc::new(SkillExecutor::new(Arc::clone(&skill_registry)));

    let state = AppState {
        memory_backend,
        local_llm_base_url: std::env::var("LOCAL_LLM_BASE_URL")
            .ok()
            .filter(|s| !s.is_empty()),
        birth_model: std::env::var("BIRTH_MODEL").ok().filter(|s| !s.is_empty()),
        birth_keys,
        skill_registry,
        skill_executor,
        agentic_tasks: Arc::new(TokioMutex::new(HashMap::new())),
        provider_keyring,
        skill_keychain,
        email_accounts,
        orchestration_tick_seconds: 30,
        skill_confirm_nonces: Arc::new(TokioMutex::new(HashMap::new())),
        pending_operational_actions: Arc::new(TokioMutex::new(HashMap::new())),
        superego_l2_mode: parse_superego_l2_mode(),
        genesis_registry: Arc::new(build_genesis_registry()),
        genesis_sessions: Arc::new(TokioMutex::new(HashMap::new())),
    };

    // Start orchestration scheduler for periodic jobs (UTC cron semantics).
    tokio::spawn(run_orchestration_scheduler_loop(state.clone()));

    // Periodic cleanup of terminal agentic tasks to prevent memory leaks.
    tokio::spawn(agentic_task_cleanup_loop(state.agentic_tasks.clone()));

    let cors = {
        let origins_env = std::env::var("CORS_ALLOW_ORIGINS").unwrap_or_default();
        let methods = vec![
            Method::GET,
            Method::POST,
            Method::PUT,
            Method::DELETE,
            Method::OPTIONS,
        ];
        let headers = vec![header::CONTENT_TYPE, header::AUTHORIZATION, header::ACCEPT];
        if origins_env.is_empty() {
            if std::env::var("ORION_CONTAINER").is_ok() {
                // Container: deny cross-origin (same-origin via nginx)
                CorsLayer::new()
                    .allow_methods(methods)
                    .allow_headers(headers)
            } else {
                // Host dev: allow all (Vite proxy is server-side, not CORS)
                CorsLayer::new()
                    .allow_origin(Any)
                    .allow_methods(methods)
                    .allow_headers(headers)
            }
        } else {
            let origins: Vec<HeaderValue> = origins_env
                .split(',')
                .filter_map(|s| s.trim().parse().ok())
                .collect();
            CorsLayer::new()
                .allow_origin(origins)
                .allow_methods(methods)
                .allow_headers(headers)
        }
    };

    let app = Router::new()
        .route("/health", get(health))
        .route("/ready", get(ready))
        .route("/api/status", get(api_status))
        .route("/api/proxy/status", get(api_proxy_status))
        .route(
            "/api/proxy/allowlist",
            get(api_proxy_allowlist_get).put(api_proxy_allowlist_put),
        )
        .route("/api/proxy/logs", get(api_proxy_logs))
        .route("/api/cognitive/registry", get(api_cognitive_registry))
        .route("/api/identities", get(api_identities))
        .route(
            "/api/mentor-name",
            get(api_get_mentor_name).put(api_set_mentor_name),
        )
        .route("/api/agents", post(api_create_agent))
        .route(
            "/api/agents/import",
            post(routes::identity::api_import_agent),
        )
        .route("/api/agents/{id}/load", post(api_load_agent))
        .route("/api/agents/{id}/birth/state", get(api_birth_state))
        .route(
            "/api/agents/{id}/birth/advance-darkness",
            post(api_birth_advance_darkness),
        )
        .route("/api/agents/{id}/birth/ignition", post(api_birth_ignition))
        .route(
            "/api/agents/{id}/connectivity/providers",
            get(api_connectivity_providers),
        )
        .route(
            "/api/agents/{id}/tier-models",
            get(api_agent_tier_models).put(api_agent_tier_models_update),
        )
        .route(
            "/api/agents/{id}/tier-models/refresh",
            post(api_agent_tier_models_refresh),
        )
        .route(
            "/api/agents/{id}/tier-models/validate",
            post(api_agent_tier_models_validate),
        )
        .route(
            "/api/agents/{id}/tier-models/reset",
            post(api_agent_tier_models_reset),
        )
        .route(
            "/api/agents/{id}/active-provider",
            axum::routing::put(api_agent_set_active_provider),
        )
        .route(
            "/api/agents/{id}/connectivity/keys",
            post(api_connectivity_store_key),
        )
        .route(
            "/api/agents/{id}/connectivity/keys/{provider}",
            axum::routing::delete(api_connectivity_remove_key),
        )
        .route(
            "/api/agents/{id}/connectivity/chat/history",
            get(api_connectivity_chat_history),
        )
        .route(
            "/api/agents/{id}/connectivity/chat",
            post(api_connectivity_chat),
        )
        .route("/api/genesis/paths", get(api_genesis_paths))
        .route("/api/agents/{id}/genesis/state", get(api_genesis_state))
        .route("/api/agents/{id}/genesis/start", post(api_genesis_start))
        .route("/api/agents/{id}/genesis/step", post(api_genesis_step))
        .route(
            "/api/agents/{id}/genesis/session/state",
            get(api_genesis_session_state),
        )
        .route(
            "/api/agents/{id}/birth/complete-emergence",
            post(api_birth_complete_emergence),
        )
        .route(
            "/api/agents/{id}/birth/chat/history",
            get(api_birth_chat_history),
        )
        .route("/api/agents/{id}/birth/chat", post(api_birth_chat))
        .route(
            "/api/agents/{id}/chat/history",
            get(api_operational_chat_history),
        )
        .route(
            "/api/agents/{id}/chat/attachments",
            post(api_operational_chat_upload_attachments).layer(DefaultBodyLimit::max(
                chat_attachments::MAX_TOTAL_UPLOAD_BYTES,
            )),
        )
        .route("/api/agents/{id}/chat", post(api_operational_chat))
        .route(
            "/api/agents/{id}/chat/stream",
            post(api_operational_chat_stream),
        )
        .route("/api/agents/{id}/chat/archive", post(api_archive_chat))
        .route(
            "/api/agents/{id}/chat/archives",
            get(api_list_chat_archives),
        )
        .route(
            "/api/agents/{id}/chat/archives/{archive_id}",
            get(api_get_chat_archive),
        )
        .route(
            "/api/agents/{id}/email/accounts",
            post(routes::skills::api_register_email_account),
        )
        .route(
            "/api/agents/{id}/skills",
            get(routes::skills::api_list_skills),
        )
        .route(
            "/api/agents/{id}/skills/missing-secrets",
            get(routes::skills::api_skills_missing_secrets),
        )
        .route(
            "/api/agents/{id}/skills/{skill_id}/execute",
            post(routes::skills::api_execute_skill),
        )
        .route(
            "/api/agents/{id}/agent/runs",
            get(routes::agent_runs::api_list_agentic_runs),
        )
        .route(
            "/api/agents/{id}/agent/run",
            post(routes::agent_runs::api_agentic_run),
        )
        .route(
            "/api/agents/{id}/agent/stream",
            get(routes::agent_runs::api_agentic_stream),
        )
        .route(
            "/api/agents/{id}/agent/respond",
            post(routes::agent_runs::api_agentic_respond),
        )
        .route(
            "/api/agents/{id}/agent/confirm",
            post(routes::agent_runs::api_agentic_confirm),
        )
        .route(
            "/api/agents/{id}/agent/cancel",
            post(routes::agent_runs::api_agentic_cancel),
        )
        .route(
            "/api/agents/{id}/agent/status",
            get(routes::agent_runs::api_agentic_status),
        )
        .route(
            "/api/agents/{id}/orchestration/jobs",
            get(api_orchestration_jobs).post(api_orchestration_job_create),
        )
        .route(
            "/api/agents/{id}/orchestration/jobs/{job_id}",
            post(api_orchestration_job_update),
        )
        .route(
            "/api/agents/{id}/orchestration/jobs/{job_id}/enable",
            post(api_orchestration_job_enable),
        )
        .route(
            "/api/agents/{id}/orchestration/jobs/{job_id}/delete",
            post(api_orchestration_job_delete),
        )
        .route(
            "/api/agents/{id}/orchestration/jobs/{job_id}/run",
            post(api_orchestration_job_run_now),
        )
        .route(
            "/api/agents/{id}/orchestration/logs",
            get(api_orchestration_logs),
        )
        .route(
            "/api/agents/{id}/identity",
            get(routes::identity::api_agent_identity),
        )
        .route(
            "/api/agents/{id}/constitution",
            get(routes::identity::api_agent_constitution),
        )
        .route(
            "/api/agents/{id}/verify",
            post(routes::identity::api_agent_verify),
        )
        .route(
            "/api/agents/{id}/export",
            post(routes::identity::api_export_agent),
        )
        .layer(axum::middleware::from_fn(auth_middleware))
        .layer(cors)
        .with_state(state);

    let bind_addr = std::env::var("ORION_BIND_ADDR").unwrap_or_else(|_| {
        if std::env::var("ORION_CONTAINER").is_ok() {
            "0.0.0.0:8080".to_string()
        } else {
            "127.0.0.1:8080".to_string()
        }
    });
    let addr: SocketAddr = bind_addr.parse().unwrap_or_else(|e| {
        tracing::error!("Invalid ORION_BIND_ADDR '{}': {}", bind_addr, e);
        SocketAddr::from(([127, 0, 0, 1], 8080))
    });
    tracing::info!("orion-api listening on {}", addr);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}
