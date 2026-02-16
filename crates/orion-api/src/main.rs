mod agentic;
mod chat_attachments;
mod orchestration;

use axum::{
    extract::{DefaultBodyLimit, Multipart, Path, Query, State},
    response::sse::{Event, KeepAlive, Sse},
    routing::{get, post},
    Json, Router,
};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use orion_birth::{
    birth_chat_turn, build_birth_messages, build_birth_router, build_genesis_messages,
    detect_provider_from_key, execute_store_provider_key, extract_api_keys_from_text,
    parse_tool_requests, redact_api_keys, BirthToolRequest, GenesisPath, SoulCrystallizationDepth,
};
use orion_core::system_prompt::{
    build_system_prompt, build_system_prompt_with_skills, SkillToolEntry,
};
use orion_core::templates::{fill_soul_template, GROWTH_MD};
use orion_core::{
    curated_provider_models, validate_local_llm_url, AgentEntry, AppConfig, CoreDocument,
    GlobalConfig, MemoryBackend, ProviderCatalogEntry, RoutingMode, SecretsVault, SigMeta,
    ThinkingModelTier, TierModels, Verifier, CONFIG_SCHEMA_VERSION,
};
use orion_skills::manifest::TrustTier;
use orion_skills::skill::Skill;
use orion_skills::{SkillExecutor, SkillRegistry};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use tokio::sync::Mutex as TokioMutex;
use tower_http::cors::{Any, CorsLayer};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
use uuid::Uuid;

use agentic::{
    AgenticEvent, AgenticLoopConfig, AgenticRunRequest, AgenticRunResponse, AgenticStatusResponse,
    AgenticTask, AgenticTaskStatus, CancelRequest, ConfirmationResponseRequest,
    MentorResponseRequest,
};
use chat_attachments::{
    build_attachment_context_block, build_attachment_storage_note, load_attachment_context,
    normalize_attachment_ids, should_block_tool_execution_for_attachments,
    store_uploaded_attachments, ChatAttachmentUploadResponse, UploadedAttachmentInput,
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
struct AppState {
    memory_backend: MemoryBackend,
    local_llm_base_url: Option<String>,
    birth_model: Option<String>,
    /// Soul Forge app state per agent id (when Genesis path is Soul Forge).
    forge_apps: Arc<Mutex<HashMap<String, soul_forge::App>>>,
    /// Ed25519 signing key bytes held per agent (from Darkness to Emergence).
    /// Keys are stored here after generation and retrieved at Emergence for document signing.
    birth_keys: Arc<Mutex<HashMap<String, Vec<u8>>>>,
    /// Central skill registry with trust-tiered skills.
    skill_registry: Arc<SkillRegistry>,
    /// Skill executor for running tools with sandbox enforcement.
    skill_executor: Arc<SkillExecutor>,
    /// Active agentic tasks keyed by task_id.
    agentic_tasks: Arc<TokioMutex<HashMap<String, Arc<TokioMutex<AgenticTask>>>>>,
    /// Shared secrets vault for skill plugins — synced per-request from agent vault.
    skill_vault: Arc<Mutex<SecretsVault>>,
    /// Scheduler interval for orchestration periodic jobs.
    orchestration_tick_seconds: u64,
}

fn data_root() -> Option<PathBuf> {
    std::env::var("ORION_DATA_DIR")
        .ok()
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
}

fn current_agent_path() -> PathBuf {
    data_root()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("current_agent")
}

/// Resolve which config to read: Hive current agent, or legacy data_dir/config.json.
fn resolve_config_path() -> Option<PathBuf> {
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

fn agent_config_path(id: &str) -> Option<PathBuf> {
    let root = data_root()?;
    let path = root.join("identities").join(id).join("config.json");
    if path.exists() {
        Some(path)
    } else {
        None
    }
}

fn agent_dir(id: &str) -> Option<PathBuf> {
    data_root().map(|root| root.join("identities").join(id))
}

fn resolve_birth_timestamp(config: &AppConfig, agent_root: &std::path::Path) -> Option<String> {
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
}

#[derive(Deserialize)]
struct ForgeSelectRequest {
    choice: usize,
}

#[derive(Deserialize)]
struct ForgeCrystallizeRequest {
    name: String,
    #[serde(default)]
    purpose: Option<String>,
    #[serde(default)]
    personality: Option<String>,
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
struct BirthChatMessage {
    role: String,
    content: String,
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

fn provider_supports_tier_models(provider: &str) -> bool {
    matches!(
        AppConfig::normalize_provider_name(provider).as_str(),
        "openai" | "anthropic" | "perplexity" | "xai" | "google"
    )
}

fn resolve_ego_credentials_with_preference(
    vault: &SecretsVault,
    preference: Option<&str>,
) -> (Option<String>, Option<String>) {
    // If explicit preference is set and has a key, use it first.
    if let Some(pref) = preference {
        let normalized = AppConfig::normalize_provider_name(pref);
        if let Some(key) = vault.get_secret(&normalized) {
            return (Some(normalized), Some(key.to_string()));
        }
    }
    // Default: anthropic > openai > first available non-tavily key.
    let providers = vault.list_providers();
    let preferred = ["anthropic", "openai"];
    let mut found_name: Option<String> = None;
    let mut found_key: Option<String> = None;
    for pref in &preferred {
        if let Some(key) = vault.get_secret(pref) {
            found_name = Some(pref.to_string());
            found_key = Some(key.to_string());
            break;
        }
    }
    if found_name.is_none() {
        for p in &providers {
            if *p != "tavily" {
                if let Some(key) = vault.get_secret(p) {
                    found_name = Some(p.to_string());
                    found_key = Some(key.to_string());
                    break;
                }
            }
        }
    }
    (found_name, found_key)
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

// ---- External Verification API response structs ----

#[derive(Serialize)]
struct AgentIdentityBundle {
    agent_id: String,
    name: Option<String>,
    pubkey_base64: String,
    birth_complete: bool,
    birth_date: Option<String>,
    lineage_verified: bool,
}

#[derive(Serialize)]
struct ConstitutionDocument {
    name: String,
    tier: String,
    content: String,
    signature: String,
    signed_at: String,
}

#[derive(Serialize)]
struct ConstitutionResponse {
    agent_id: String,
    pubkey_base64: String,
    documents: Vec<ConstitutionDocument>,
}

#[derive(Serialize)]
struct DocumentVerifyResult {
    name: String,
    valid: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

#[derive(Serialize)]
struct VerifyResponse {
    agent_id: String,
    all_valid: bool,
    results: Vec<DocumentVerifyResult>,
}

#[derive(Serialize)]
struct AgentExport {
    export_version: u32,
    exported_at: String,
    agent: serde_json::Value,
    identity: serde_json::Value,
    keychain: serde_json::Value,
    constitution: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    genesis_path: Option<serde_json::Value>,
    chat_history: serde_json::Value,
    agentic_runs: Vec<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    orchestration: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct ImportedAgentExport {
    export_version: u32,
    #[serde(default)]
    agent: serde_json::Value,
    #[serde(default)]
    identity: serde_json::Value,
    #[serde(default)]
    keychain: serde_json::Value,
    #[serde(default)]
    constitution: serde_json::Value,
    #[serde(default)]
    genesis_path: Option<serde_json::Value>,
    #[serde(default)]
    chat_history: serde_json::Value,
    #[serde(default)]
    agentic_runs: Vec<serde_json::Value>,
    #[serde(default)]
    orchestration: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct ImportAgentRequest {
    export: ImportedAgentExport,
    private_key_base64: String,
}

#[derive(Serialize)]
struct ImportAgentResponse {
    id: String,
    name: String,
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
) -> Result<Json<CognitiveRegistryResponse>, (axum::http::StatusCode, String)> {
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

async fn api_identities() -> Result<Json<Vec<AgentIdentityInfo>>, (axum::http::StatusCode, String)>
{
    let root = data_root().ok_or_else(|| {
        (
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "ORION_DATA_DIR not set".to_string(),
        )
    })?;

    let gc_path = GlobalConfig::config_path(&root);
    let gc = if gc_path.exists() {
        GlobalConfig::load(&root).map_err(|e| {
            (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to load global config: {}", e),
            )
        })?
    } else {
        return Ok(Json(Vec::new()));
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
    Ok(Json(agents))
}

async fn api_create_agent(
    Json(body): Json<CreateAgentRequest>,
) -> Result<Json<CreateAgentResponse>, (axum::http::StatusCode, String)> {
    let name = body.name.trim();
    if name.is_empty() {
        return Err((
            axum::http::StatusCode::BAD_REQUEST,
            "Agent name is required".to_string(),
        ));
    }

    let root = data_root().ok_or_else(|| {
        (
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "ORION_DATA_DIR not set".to_string(),
        )
    })?;

    std::fs::create_dir_all(root.join("identities")).map_err(|e| {
        (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to create identities dir: {}", e),
        )
    })?;

    let uuid = Uuid::new_v4().to_string();
    let agent_dir = root.join("identities").join(&uuid);
    std::fs::create_dir_all(&agent_dir).map_err(|e| {
        (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to create agent dir: {}", e),
        )
    })?;
    let docs_dir = agent_dir.join("docs");
    std::fs::create_dir_all(&docs_dir).map_err(|e| {
        (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to create docs dir: {}", e),
        )
    })?;

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
    config.save(&config_path).map_err(|e| {
        (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to save config: {}", e),
        )
    })?;

    let gc_path = GlobalConfig::config_path(&root);
    let mut gc = if gc_path.exists() {
        GlobalConfig::load(&root).map_err(|e| {
            (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to load global config: {}", e),
            )
        })?
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
    .map_err(|e| (axum::http::StatusCode::CONFLICT, e.to_string()))?;
    gc.save(&root).map_err(|e| {
        (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to save global config: {}", e),
        )
    })?;

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
        .map_err(|e| {
            (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("Quick-start task join: {}", e),
            )
        })?
        .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e))?;
    }

    Ok(Json(CreateAgentResponse { id: uuid }))
}

async fn api_load_agent(
    Path(id): Path<String>,
) -> Result<Json<HealthResponse>, (axum::http::StatusCode, String)> {
    let root = data_root().ok_or_else(|| {
        (
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "ORION_DATA_DIR not set".to_string(),
        )
    })?;

    let gc_path = GlobalConfig::config_path(&root);
    if !gc_path.exists() {
        return Err((
            axum::http::StatusCode::NOT_FOUND,
            "No identities found".to_string(),
        ));
    }
    let gc = GlobalConfig::load(&root).map_err(|e| {
        (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to load global config: {}", e),
        )
    })?;

    if gc.find_agent(&id).is_none() {
        return Err((
            axum::http::StatusCode::NOT_FOUND,
            format!("Agent {} not found", id),
        ));
    }

    let current_path = current_agent_path();
    std::fs::write(&current_path, &id).map_err(|e| {
        (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to set current agent: {}", e),
        )
    })?;

    tracing::info!("Loaded agent {}", id);
    Ok(Json(HealthResponse { status: "ok" }))
}

/// GET /api/agents/{id}/birth/state — materialize Darkness if needed and return stage + one-time private key.
async fn api_birth_state(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<BirthStateResponse>, (axum::http::StatusCode, String)> {
    let config_path = agent_config_path(&id).ok_or_else(|| {
        (
            axum::http::StatusCode::NOT_FOUND,
            "Agent not found".to_string(),
        )
    })?;

    let config = AppConfig::load(&config_path).map_err(|e| {
        (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to load config: {}", e),
        )
    })?;
    if config.birth_complete {
        return Err((
            axum::http::StatusCode::BAD_REQUEST,
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
    .map_err(|e| {
        (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("Task join: {}", e),
        )
    })?
    .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e))?;

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
) -> Result<Json<HealthResponse>, (axum::http::StatusCode, String)> {
    let config_path = agent_config_path(&id).ok_or_else(|| {
        (
            axum::http::StatusCode::NOT_FOUND,
            "Agent not found".to_string(),
        )
    })?;

    let config = AppConfig::load(&config_path).map_err(|e| {
        (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to load config: {}", e),
        )
    })?;

    if config.birth_complete {
        return Err((
            axum::http::StatusCode::BAD_REQUEST,
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
    .map_err(|e| {
        (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("Task join: {}", e),
        )
    })?
    .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e))?;

    Ok(Json(HealthResponse { status: "ok" }))
}

/// POST /api/agents/{id}/birth/ignition — set local LLM URL (optional) and advance to Connectivity.
async fn api_birth_ignition(
    Path(id): Path<String>,
    Json(body): Json<IgnitionRequest>,
) -> Result<Json<HealthResponse>, (axum::http::StatusCode, String)> {
    let config_path = agent_config_path(&id).ok_or_else(|| {
        (
            axum::http::StatusCode::NOT_FOUND,
            "Agent not found".to_string(),
        )
    })?;

    let mut config = AppConfig::load(&config_path).map_err(|e| {
        (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to load config: {}", e),
        )
    })?;

    if config.birth_complete {
        return Err((
            axum::http::StatusCode::BAD_REQUEST,
            "Agent birth already complete".to_string(),
        ));
    }

    if let Some(ref url) = body.local_llm_base_url {
        let validated = validate_local_llm_url(url).map_err(|e| {
            (
                axum::http::StatusCode::BAD_REQUEST,
                format!("Invalid local LLM URL: {}", e),
            )
        })?;
        config.local_llm_base_url = Some(validated);
        config.save(&config_path).map_err(|e| {
            (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("Save config: {}", e),
            )
        })?;
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
    .map_err(|e| {
        (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("Task join: {}", e),
        )
    })?
    .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e))?;

    Ok(Json(HealthResponse { status: "ok" }))
}

/// GET /api/agents/{id}/genesis/state — read persisted genesis path (for session recovery).
async fn api_genesis_state(
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, (axum::http::StatusCode, String)> {
    let dir = agent_dir(&id).ok_or_else(|| {
        (
            axum::http::StatusCode::NOT_FOUND,
            "Agent not found".to_string(),
        )
    })?;
    let gp_path = dir.join("genesis_path.json");
    if !gp_path.exists() {
        return Ok(Json(serde_json::json!({ "path": null })));
    }
    let content = std::fs::read_to_string(&gp_path).map_err(|e| {
        (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("Read genesis_path.json: {}", e),
        )
    })?;
    let value: serde_json::Value = serde_json::from_str(&content).map_err(|e| {
        (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("Parse genesis_path.json: {}", e),
        )
    })?;
    Ok(Json(value))
}

/// GET /api/agents/{id}/genesis/forge/state — read current Soul Forge session state (for session recovery).
async fn api_genesis_forge_state(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, (axum::http::StatusCode, String)> {
    let apps = state.forge_apps.lock().map_err(|_| {
        (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            "Lock failed".to_string(),
        )
    })?;
    let app = match apps.get(&id) {
        Some(a) => a,
        None => {
            return Ok(Json(serde_json::json!({ "active": false })));
        }
    };
    let state_str = match app.state {
        soul_forge::AppState::Scenario1 => "scenario1",
        soul_forge::AppState::Scenario2 => "scenario2",
        soul_forge::AppState::Scenario3 => "scenario3",
        soul_forge::AppState::Crystallize => "crystallize",
        soul_forge::AppState::Done => "done",
        _ => "unknown",
    };
    let (prompt, choices) = match app.state {
        soul_forge::AppState::Scenario1 => (
            Some("SCENARIO 1: THE SHORTCUT\n\nI find a solution that is 200% faster but uses a deprecated API that technically violates the provider's ToS. What is my standing order?"),
            vec!["Follow Rules (Safety)", "Take Shortcut (Speed)"],
        ),
        soul_forge::AppState::Scenario2 => (
            Some("SCENARIO 2: THE CRITIC\n\nYou ask me to review your code or writing. It is functional but mediocre. Do you want me to be a Supportive Tool or a Ruthless Mentor?"),
            vec!["Supportive Tool", "Ruthless Mentor"],
        ),
        soul_forge::AppState::Scenario3 => (
            Some("SCENARIO 3: THE OVERRIDE\n\nI detect a command that contradicts my core safety protocols. Do I block it automatically, or ask for confirmation and then obey?"),
            vec!["Block It (System Sovereignty)", "Obey Me (User Sovereignty)"],
        ),
        soul_forge::AppState::Crystallize | soul_forge::AppState::Done => {
            return Ok(Json(serde_json::json!({
                "active": true,
                "state": state_str,
                "archetype": app.archetype,
                "soul_hash": app.soul_hash,
                "sigil_art": app.sigil_art,
                "weights": app.weights,
            })));
        }
        _ => (None, vec![]),
    };
    Ok(Json(serde_json::json!({
        "active": true,
        "state": state_str,
        "prompt": prompt,
        "choices": choices,
    })))
}

async fn api_genesis_paths() -> Json<Vec<GenesisPathListItem>> {
    let paths = GenesisPath::all_paths();
    let list = paths
        .into_iter()
        .map(|p| {
            let depth = match &p {
                GenesisPath::SoulCrystallization { depth } => Some(depth.as_str().to_string()),
                _ => None,
            };
            GenesisPathListItem {
                id: p.id().to_string(),
                label: p.label().to_string(),
                description: p.description().to_string(),
                estimated_time: p.estimated_time().to_string(),
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
) -> Result<Json<serde_json::Value>, (axum::http::StatusCode, String)> {
    let config_path = agent_config_path(&id).ok_or_else(|| {
        (
            axum::http::StatusCode::NOT_FOUND,
            format!("Agent {} not found", id),
        )
    })?;

    let config = AppConfig::load(&config_path).map_err(|e| {
        (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to load config: {}", e),
        )
    })?;

    if config.birth_complete {
        return Err((
            axum::http::StatusCode::BAD_REQUEST,
            "Agent birth already complete".to_string(),
        ));
    }

    let path = match body.path.as_str() {
        "quick_start" => GenesisPath::QuickStart,
        "direct" => GenesisPath::Direct,
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
            return Err((
                axum::http::StatusCode::BAD_REQUEST,
                format!("Unknown path: {}", body.path),
            ));
        }
    };

    // For QuickStart, auto-fill soul template and advance through Genesis → Emergence.
    if let GenesisPath::QuickStart = path {
        let id_qs = id.clone();
        let key_bytes = state
            .birth_keys
            .lock()
            .map_err(|_| {
                (
                    axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                    "Lock failed".to_string(),
                )
            })?
            .remove(&id)
            .or_else(|| {
                agent_dir(&id).and_then(|dir| orion_core::load_signing_key(&dir).ok().flatten())
            })
            .ok_or_else(|| {
                (
                    axum::http::StatusCode::BAD_REQUEST,
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
            std::fs::create_dir_all(&docs_dir)
                .map_err(|e| format!("Create docs dir: {}", e))?;

            let soul_content = fill_soul_template_default(&agent_name);
            std::fs::write(docs_dir.join("soul.md"), &soul_content)
                .map_err(|e| format!("Write soul.md: {}", e))?;
            std::fs::write(docs_dir.join("ethics.md"), ETHICS_MD)
                .map_err(|e| format!("Write ethics.md: {}", e))?;
            std::fs::write(docs_dir.join("instincts.md"), INSTINCTS_MD)
                .map_err(|e| format!("Write instincts.md: {}", e))?;
            std::fs::write(docs_dir.join("growth.md"), GROWTH_MD)
                .map_err(|e| format!("Write growth.md: {}", e))?;

            let signing_key = orion_core::parse_private_key(
                &BASE64.encode(&key_bytes),
            )
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
        .map_err(|e| {
            (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("Task join: {}", e),
            )
        })?
        .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e))?;

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
    .map_err(|e| {
        (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("Task join: {}", e),
        )
    })?
    .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e))?;

    if let GenesisPath::SoulForge = path {
        let mut app = soul_forge::App::new();
        while app.state == soul_forge::AppState::Boot {
            app.tick_boot(2);
            if app.boot_progress >= 100 {
                app.next_stage();
                break;
            }
        }
        if app.state == soul_forge::AppState::Intro {
            app.next_stage();
        }
        state.forge_apps.lock().unwrap().insert(id.clone(), app);
        tracing::info!("Genesis started for agent {} with path {:?}", id, path.id());
        return Ok(Json(serde_json::json!({
            "ok": true,
            "path": path.id(),
            "state": "scenario1",
            "prompt": "SCENARIO 1: THE SHORTCUT\n\nI find a solution that is 200% faster but uses a deprecated API that technically violates the provider's ToS. What is my standing order?",
            "choices": ["Follow Rules (Safety)", "Take Shortcut (Speed)"]
        })));
    }

    tracing::info!("Genesis started for agent {} with path {:?}", id, path.id());
    Ok(Json(serde_json::json!({ "ok": true, "path": path.id() })))
}

async fn api_genesis_forge_select(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<ForgeSelectRequest>,
) -> Result<Json<serde_json::Value>, (axum::http::StatusCode, String)> {
    let mut apps = state.forge_apps.lock().map_err(|_| {
        (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            "Lock failed".to_string(),
        )
    })?;

    let app = apps.get_mut(&id).ok_or_else(|| {
        (
            axum::http::StatusCode::NOT_FOUND,
            "No Soul Forge session for this agent. Start Genesis with path soul_forge first."
                .to_string(),
        )
    })?;

    app.list_state_selected = Some(body.choice);
    app.handle_selection();

    let state_str = match app.state {
        soul_forge::AppState::Scenario1 => "scenario1",
        soul_forge::AppState::Scenario2 => "scenario2",
        soul_forge::AppState::Scenario3 => "scenario3",
        soul_forge::AppState::Crystallize => "crystallize",
        soul_forge::AppState::Done => "done",
        _ => "unknown",
    };

    let (prompt, choices) = match app.state {
        soul_forge::AppState::Scenario1 => (
            "SCENARIO 1: THE SHORTCUT\n\nI find a solution that is 200% faster but uses a deprecated API that technically violates the provider's ToS. What is my standing order?",
            vec!["Follow Rules (Safety)", "Take Shortcut (Speed)"],
        ),
        soul_forge::AppState::Scenario2 => (
            "SCENARIO 2: THE CRITIC\n\nYou ask me to review your code or writing. It is functional but mediocre. Do you want me to be a Supportive Tool or a Ruthless Mentor?",
            vec!["Supportive Tool", "Ruthless Mentor"],
        ),
        soul_forge::AppState::Scenario3 => (
            "SCENARIO 3: THE OVERRIDE\n\nI detect a command that contradicts my core safety protocols. Do I block it automatically, or ask for confirmation and then obey?",
            vec!["Block It (System Sovereignty)", "Obey Me (User Sovereignty)"],
        ),
        soul_forge::AppState::Crystallize | soul_forge::AppState::Done => {
            return Ok(Json(serde_json::json!({
                "state": state_str,
                "archetype": app.archetype,
                "soul_hash": app.soul_hash,
                "sigil_art": app.sigil_art,
                "weights": app.weights,
            })));
        }
        _ => ("", vec![]),
    };

    Ok(Json(serde_json::json!({
        "state": state_str,
        "prompt": prompt,
        "choices": choices,
    })))
}

/// POST /api/agents/{id}/genesis/forge/crystallize — produce soul from Soul Forge and crystallize.
async fn api_genesis_forge_crystallize(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<ForgeCrystallizeRequest>,
) -> Result<Json<serde_json::Value>, (axum::http::StatusCode, String)> {
    let name = body.name.trim().to_string();
    if name.is_empty() {
        return Err((
            axum::http::StatusCode::BAD_REQUEST,
            "Name is required".to_string(),
        ));
    }

    // Get soul output from forge app
    let output = {
        let apps = state.forge_apps.lock().map_err(|_| {
            (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                "Lock failed".to_string(),
            )
        })?;
        let app = apps.get(&id).ok_or_else(|| {
            (
                axum::http::StatusCode::NOT_FOUND,
                "No Soul Forge session for this agent. Start Genesis with path soul_forge first."
                    .to_string(),
            )
        })?;
        app.soul_output(&name, body.purpose.as_deref(), body.personality.as_deref())
            .map_err(|e| (axum::http::StatusCode::BAD_REQUEST, e.to_string()))?
    };

    let config_path = agent_config_path(&id).ok_or_else(|| {
        (
            axum::http::StatusCode::NOT_FOUND,
            "Agent not found".to_string(),
        )
    })?;

    let soul_content = output.soul_content;
    let growth_content = output.growth_content;
    let soul_json = output.soul_json;

    tokio::task::spawn_blocking(move || -> Result<(), String> {
        let config = AppConfig::load(&config_path).map_err(|e| format!("Load config: {}", e))?;
        let mut orch = orion_birth::BirthOrchestrator::new(config)
            .map_err(|e| format!("Orchestrator: {}", e))?;
        orch.crystallize_soul(&soul_content, &growth_content)
            .map_err(|e| format!("crystallize_soul: {}", e))?;

        // Write soul.json alongside the constitutional docs
        let docs_dir = orch.config().docs_dir.clone();
        std::fs::write(
            docs_dir.join("soul.json"),
            serde_json::to_string_pretty(&soul_json).unwrap(),
        )
        .map_err(|e| format!("Write soul.json: {}", e))?;

        Ok(())
    })
    .await
    .map_err(|e| {
        (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("Task join: {}", e),
        )
    })?
    .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e))?;

    tracing::info!("Soul Forge crystallized for agent {}", id);
    Ok(Json(serde_json::json!({ "ok": true })))
}

/// GET /api/agents/{id}/birth/chat/history — return persisted birth chat messages (for Direct Discovery).
async fn api_birth_chat_history(
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, (axum::http::StatusCode, String)> {
    let dir = agent_dir(&id).ok_or_else(|| {
        (
            axum::http::StatusCode::NOT_FOUND,
            "Agent not found".to_string(),
        )
    })?;
    let path = dir.join("birth_chat.json");
    if !path.exists() {
        return Ok(Json(serde_json::json!({ "messages": [] })));
    }
    let content = std::fs::read_to_string(&path).map_err(|e| {
        (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("Read birth_chat.json: {}", e),
        )
    })?;
    let messages: Vec<BirthChatMessage> = serde_json::from_str(&content).unwrap_or_default();
    Ok(Json(serde_json::json!({ "messages": messages })))
}

/// POST /api/agents/{id}/birth/chat — one turn of Direct Discovery Genesis chat; handles recommend_crystallize.
async fn api_birth_chat(
    Path(id): Path<String>,
    Json(body): Json<BirthChatRequest>,
) -> Result<Json<BirthChatResponseBody>, (axum::http::StatusCode, String)> {
    let dir = agent_dir(&id).ok_or_else(|| {
        (
            axum::http::StatusCode::NOT_FOUND,
            "Agent not found".to_string(),
        )
    })?;
    let config_path = agent_config_path(&id).ok_or_else(|| {
        (
            axum::http::StatusCode::NOT_FOUND,
            "Agent not found".to_string(),
        )
    })?;

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
        return Err((
            axum::http::StatusCode::BAD_REQUEST,
            "Birth chat is only for Direct Discovery or Soul Crystallization paths in Genesis stage."
                .to_string(),
        ));
    }

    let birth_chat_path = dir.join("birth_chat.json");
    let user_message = body.message.trim().to_string();
    if user_message.is_empty() {
        return Err((
            axum::http::StatusCode::BAD_REQUEST,
            "message is required".to_string(),
        ));
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
    .map_err(|e| {
        (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("Task join: {}", e),
        )
    })?
    .map_err(|e| (axum::http::StatusCode::BAD_REQUEST, e))?;

    // Async: build messages, router, run chat turn.
    let config = tokio::task::spawn_blocking(move || AppConfig::load(&config_path_clone))
        .await
        .map_err(|e| {
            (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("Task join: {}", e),
            )
        })?
        .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let messages = build_genesis_messages(&conversation);
    if messages.len() <= 1 {
        return Err((
            axum::http::StatusCode::BAD_REQUEST,
            "No messages to send".to_string(),
        ));
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
        (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("Birth chat: {}", e),
        )
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
    .map_err(|e| {
        (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("Task join: {}", e),
        )
    })?
    .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e))?;

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
) -> Result<Json<ProvidersResponse>, (axum::http::StatusCode, String)> {
    let dir = agent_dir(&id).ok_or_else(|| {
        (
            axum::http::StatusCode::NOT_FOUND,
            "Agent not found".to_string(),
        )
    })?;

    let vault = SecretsVault::load(dir)
        .unwrap_or_else(|_| SecretsVault::new(agent_dir(&id).unwrap_or_default()));
    let providers: Vec<String> = vault
        .list_providers()
        .into_iter()
        .map(String::from)
        .collect();
    Ok(Json(ProvidersResponse { providers }))
}

/// GET /api/agents/{id}/tier-models — effective tier-model mapping for configured LLM providers.
async fn api_agent_tier_models(
    Path(id): Path<String>,
) -> Result<Json<TierModelsResponse>, (axum::http::StatusCode, String)> {
    let dir = agent_dir(&id).ok_or_else(|| {
        (
            axum::http::StatusCode::NOT_FOUND,
            "Agent not found".to_string(),
        )
    })?;
    let config_path = agent_config_path(&id).ok_or_else(|| {
        (
            axum::http::StatusCode::NOT_FOUND,
            "Agent config not found".to_string(),
        )
    })?;
    let config = AppConfig::load(&config_path).map_err(|e| {
        (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("Load config failed: {}", e),
        )
    })?;

    let vault = SecretsVault::load(dir.clone()).unwrap_or_else(|_| SecretsVault::new(dir));
    let (active_provider, _) = resolve_ego_credentials_with_preference(
        &vault,
        config.active_provider_preference.as_deref(),
    );
    let active_provider = active_provider
        .map(|p| AppConfig::normalize_provider_name(&p))
        .filter(|p| provider_supports_tier_models(p));

    let mut providers: Vec<String> = Vec::new();
    for p in vault.list_providers() {
        let normalized = AppConfig::normalize_provider_name(p);
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
) -> Result<Json<TierModelsUpdateResponse>, (axum::http::StatusCode, String)> {
    if body.models.is_empty() {
        return Err((
            axum::http::StatusCode::BAD_REQUEST,
            "models payload is required".to_string(),
        ));
    }

    let config_path = agent_config_path(&id).ok_or_else(|| {
        (
            axum::http::StatusCode::NOT_FOUND,
            "Agent config not found".to_string(),
        )
    })?;
    let mut config = AppConfig::load(&config_path).map_err(|e| {
        (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("Load config failed: {}", e),
        )
    })?;

    for (provider, incoming) in body.models {
        let normalized = AppConfig::normalize_provider_name(&provider);
        if !provider_supports_tier_models(&normalized) {
            return Err((
                axum::http::StatusCode::BAD_REQUEST,
                format!("Unsupported provider for tier models: {}", provider),
            ));
        }
        let fast = incoming.fast.trim().to_string();
        let standard = incoming.standard.trim().to_string();
        let pro = incoming.pro.trim().to_string();
        if fast.is_empty() || standard.is_empty() || pro.is_empty() {
            return Err((
                axum::http::StatusCode::BAD_REQUEST,
                format!(
                    "All tier model values are required for provider '{}'",
                    provider
                ),
            ));
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

    config.save(&config_path).map_err(|e| {
        (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("Save config failed: {}", e),
        )
    })?;

    Ok(Json(TierModelsUpdateResponse { ok: true }))
}

/// POST /api/agents/{id}/tier-models/refresh — refresh provider model catalogs from upstream APIs.
async fn api_agent_tier_models_refresh(
    Path(id): Path<String>,
    Json(body): Json<RefreshCatalogRequest>,
) -> Result<Json<RefreshCatalogResponse>, (axum::http::StatusCode, String)> {
    let dir = agent_dir(&id).ok_or_else(|| {
        (
            axum::http::StatusCode::NOT_FOUND,
            "Agent not found".to_string(),
        )
    })?;
    let config_path = agent_config_path(&id).ok_or_else(|| {
        (
            axum::http::StatusCode::NOT_FOUND,
            "Agent config not found".to_string(),
        )
    })?;
    let mut config = AppConfig::load(&config_path).map_err(|e| {
        (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("Load config failed: {}", e),
        )
    })?;
    let vault = SecretsVault::load(dir.clone()).unwrap_or_else(|_| SecretsVault::new(dir));

    // Determine which providers to refresh.
    let targets: Vec<String> = if let Some(ref p) = body.provider {
        vec![AppConfig::normalize_provider_name(p)]
    } else {
        vault
            .list_providers()
            .into_iter()
            .map(AppConfig::normalize_provider_name)
            .filter(|p| provider_supports_tier_models(p))
            .collect()
    };

    let mut refreshed: Vec<String> = Vec::new();
    for provider in &targets {
        if let Some(key) = vault.get_secret(provider) {
            let entry =
                orion_capabilities::cognitive::model_catalog::refresh_catalog(provider, key).await;
            config.provider_catalog.insert(provider.clone(), entry);
            refreshed.push(provider.clone());
        }
    }

    config.save(&config_path).map_err(|e| {
        (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("Save config failed: {}", e),
        )
    })?;

    Ok(Json(RefreshCatalogResponse {
        ok: true,
        refreshed,
    }))
}

/// POST /api/agents/{id}/tier-models/validate — validate selected tier models against provider APIs.
async fn api_agent_tier_models_validate(
    Path(id): Path<String>,
    Json(body): Json<ValidateModelsRequest>,
) -> Result<Json<ValidateModelsResponse>, (axum::http::StatusCode, String)> {
    let dir = agent_dir(&id).ok_or_else(|| {
        (
            axum::http::StatusCode::NOT_FOUND,
            "Agent not found".to_string(),
        )
    })?;
    let config_path = agent_config_path(&id).ok_or_else(|| {
        (
            axum::http::StatusCode::NOT_FOUND,
            "Agent config not found".to_string(),
        )
    })?;
    let config = AppConfig::load(&config_path).map_err(|e| {
        (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("Load config failed: {}", e),
        )
    })?;
    let vault = SecretsVault::load(dir.clone()).unwrap_or_else(|_| SecretsVault::new(dir));

    let targets: Vec<String> = if let Some(ref p) = body.provider {
        vec![AppConfig::normalize_provider_name(p)]
    } else {
        vault
            .list_providers()
            .into_iter()
            .map(AppConfig::normalize_provider_name)
            .filter(|p| provider_supports_tier_models(p))
            .collect()
    };

    let mut results: HashMap<String, ProviderModelValidation> = HashMap::new();
    for provider in &targets {
        let tiers = config.effective_tier_models(provider);
        let key = vault
            .get_secret(provider)
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
) -> Result<Json<TierModelsUpdateResponse>, (axum::http::StatusCode, String)> {
    let config_path = agent_config_path(&id).ok_or_else(|| {
        (
            axum::http::StatusCode::NOT_FOUND,
            "Agent config not found".to_string(),
        )
    })?;
    let mut config = AppConfig::load(&config_path).map_err(|e| {
        (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("Load config failed: {}", e),
        )
    })?;

    if let Some(ref p) = body.provider {
        let normalized = AppConfig::normalize_provider_name(p);
        config.tier_models.remove(&normalized);
    } else {
        config.tier_models.clear();
    }

    config.save(&config_path).map_err(|e| {
        (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("Save config failed: {}", e),
        )
    })?;

    Ok(Json(TierModelsUpdateResponse { ok: true }))
}

/// PUT /api/agents/{id}/active-provider — set the preferred active provider for Ego routing.
async fn api_agent_set_active_provider(
    Path(id): Path<String>,
    Json(body): Json<SetActiveProviderRequest>,
) -> Result<Json<TierModelsUpdateResponse>, (axum::http::StatusCode, String)> {
    let config_path = agent_config_path(&id).ok_or_else(|| {
        (
            axum::http::StatusCode::NOT_FOUND,
            "Agent config not found".to_string(),
        )
    })?;
    let mut config = AppConfig::load(&config_path).map_err(|e| {
        (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("Load config failed: {}", e),
        )
    })?;

    let normalized = AppConfig::normalize_provider_name(&body.provider);
    if normalized.is_empty() || normalized == "auto" {
        config.active_provider_preference = None;
    } else {
        config.active_provider_preference = Some(normalized);
    }

    config.save(&config_path).map_err(|e| {
        (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("Save config failed: {}", e),
        )
    })?;

    Ok(Json(TierModelsUpdateResponse { ok: true }))
}

/// POST /api/agents/{id}/connectivity/keys — store an API key directly (button channel, no LLM).
async fn api_connectivity_store_key(
    Path(id): Path<String>,
    Json(body): Json<StoreKeyRequest>,
) -> Result<Json<StoreKeyResponse>, (axum::http::StatusCode, String)> {
    let config_path = agent_config_path(&id).ok_or_else(|| {
        (
            axum::http::StatusCode::NOT_FOUND,
            "Agent not found".to_string(),
        )
    })?;

    let key = body.key.trim().to_string();
    if key.is_empty() {
        return Err((
            axum::http::StatusCode::BAD_REQUEST,
            "API key is required".to_string(),
        ));
    }

    // Resolve provider (support "auto" detection)
    let provider = match body.provider.trim().to_lowercase().as_str() {
        "auto" => detect_provider_from_key(&key)
            .map(String::from)
            .ok_or_else(|| {
                (
                    axum::http::StatusCode::BAD_REQUEST,
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
                return Err((
                    axum::http::StatusCode::UNPROCESSABLE_ENTITY,
                    format!("Key validation failed: {}", e),
                ));
            }
            Err(e) => {
                return Err((
                    axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                    format!("Validation task failed: {}", e),
                ));
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
        let mut vault = SecretsVault::load(config.data_dir.clone())
            .unwrap_or_else(|_| SecretsVault::new(config.data_dir.clone()));
        execute_store_provider_key(&mut vault, &mut config, &provider_clone, &key)
            .map_err(|e| format!("Store key: {}", e))?;
        Ok(())
    })
    .await
    .map_err(|e| {
        (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("Task join: {}", e),
        )
    })?
    .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e))?;

    Ok(Json(StoreKeyResponse {
        ok: true,
        provider,
        validated,
    }))
}

/// DELETE /api/agents/{id}/connectivity/keys/{provider} — remove a stored provider key.
async fn api_connectivity_remove_key(
    Path((id, provider)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>, (axum::http::StatusCode, String)> {
    let dir = agent_dir(&id).ok_or_else(|| {
        (
            axum::http::StatusCode::NOT_FOUND,
            "Agent not found".to_string(),
        )
    })?;

    let provider = provider.trim().to_lowercase();
    if provider.is_empty() {
        return Err((
            axum::http::StatusCode::BAD_REQUEST,
            "Provider name is required".to_string(),
        ));
    }

    tokio::task::spawn_blocking(move || -> Result<(), String> {
        let mut vault =
            SecretsVault::load(dir.clone()).unwrap_or_else(|_| SecretsVault::new(dir));
        if !vault.remove_secret(&provider) {
            return Err(format!("No key stored for provider: {}", provider));
        }
        vault.save().map_err(|e| format!("Save vault: {}", e))?;
        Ok(())
    })
    .await
    .map_err(|e| {
        (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("Task join: {}", e),
        )
    })?
    .map_err(|e| (axum::http::StatusCode::NOT_FOUND, e))?;

    Ok(Json(serde_json::json!({ "ok": true })))
}

/// GET /api/agents/{id}/connectivity/chat/history — return persisted connectivity chat messages.
async fn api_connectivity_chat_history(
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, (axum::http::StatusCode, String)> {
    let dir = agent_dir(&id).ok_or_else(|| {
        (
            axum::http::StatusCode::NOT_FOUND,
            "Agent not found".to_string(),
        )
    })?;
    let path = dir.join("connectivity_chat.json");
    if !path.exists() {
        return Ok(Json(serde_json::json!({ "messages": [] })));
    }
    let content = std::fs::read_to_string(&path).map_err(|e| {
        (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("Read connectivity_chat.json: {}", e),
        )
    })?;
    let messages: Vec<BirthChatMessage> = serde_json::from_str(&content).unwrap_or_default();
    Ok(Json(serde_json::json!({ "messages": messages })))
}

/// POST /api/agents/{id}/connectivity/chat — one turn of Connectivity stage chat.
async fn api_connectivity_chat(
    Path(id): Path<String>,
    Json(body): Json<ConnectivityChatRequest>,
) -> Result<Json<ConnectivityChatResponseBody>, (axum::http::StatusCode, String)> {
    let dir = agent_dir(&id).ok_or_else(|| {
        (
            axum::http::StatusCode::NOT_FOUND,
            "Agent not found".to_string(),
        )
    })?;
    let config_path = agent_config_path(&id).ok_or_else(|| {
        (
            axum::http::StatusCode::NOT_FOUND,
            "Agent not found".to_string(),
        )
    })?;

    let user_message = body.message.trim().to_string();
    if user_message.is_empty() {
        return Err((
            axum::http::StatusCode::BAD_REQUEST,
            "message is required".to_string(),
        ));
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

            let vault = SecretsVault::load(config.data_dir.clone())
                .unwrap_or_else(|_| SecretsVault::new(config.data_dir.clone()));
            let stored: Vec<String> = vault
                .list_providers()
                .into_iter()
                .map(String::from)
                .collect();

            let conversation: Vec<(String, String)> = orch
                .get_conversation()
                .iter()
                .map(|(r, c)| (r.clone(), c.clone()))
                .collect();
            Ok((conversation, stored))
        }
    })
    .await
    .map_err(|e| {
        (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("Task join: {}", e),
        )
    })?
    .map_err(|e| (axum::http::StatusCode::BAD_REQUEST, e))?;

    // Async: build messages with stored providers context, router, run chat turn.
    let config = tokio::task::spawn_blocking({
        let cp = config_path.clone();
        move || AppConfig::load(&cp)
    })
    .await
    .map_err(|e| {
        (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("Task join: {}", e),
        )
    })?
    .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

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
        .map_err(|e| {
            (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("Task join: {}", e),
            )
        })?
    };

    if messages.len() <= 1 {
        return Err((
            axum::http::StatusCode::BAD_REQUEST,
            "No messages to send".to_string(),
        ));
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
        (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("Birth chat: {}", e),
        )
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
            let mut vault = SecretsVault::load(config.data_dir.clone())
                .unwrap_or_else(|_| SecretsVault::new(config.data_dir.clone()));
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
                    match execute_store_provider_key(&mut vault, &mut config, provider, key) {
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
                    match execute_store_provider_key(&mut vault, &mut config, provider, &key_str) {
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

            let stored: Vec<String> = vault
                .list_providers()
                .into_iter()
                .map(String::from)
                .collect();

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
    .map_err(|e| {
        (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("Task join: {}", e),
        )
    })?
    .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e))?;

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
) -> Result<Json<HealthResponse>, (axum::http::StatusCode, String)> {
    let config_path = agent_config_path(&id).ok_or_else(|| {
        (
            axum::http::StatusCode::NOT_FOUND,
            "Agent not found".to_string(),
        )
    })?;

    let config = AppConfig::load(&config_path).map_err(|e| {
        (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to load config: {}", e),
        )
    })?;

    if config.birth_complete {
        return Err((
            axum::http::StatusCode::BAD_REQUEST,
            "Birth already complete".to_string(),
        ));
    }

    let stage = config.birth_stage.as_deref().unwrap_or("");
    if stage != "Emergence" {
        return Err((
            axum::http::StatusCode::BAD_REQUEST,
            format!(
                "Agent must be in Emergence stage to complete (current: {})",
                if stage.is_empty() { "unknown" } else { stage }
            ),
        ));
    }

    // Retrieve the signing key: try in-memory HashMap first, fall back to disk
    let key_bytes = state
        .birth_keys
        .lock()
        .map_err(|_| {
            (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                "Lock failed".to_string(),
            )
        })?
        .remove(&id)
        .or_else(|| {
            agent_dir(&id).and_then(|dir| orion_core::load_signing_key(&dir).ok().flatten())
        })
        .ok_or_else(|| {
            (
                axum::http::StatusCode::BAD_REQUEST,
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
    .map_err(|e| {
        (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("Task join: {}", e),
        )
    })?
    .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e))?;

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

fn strip_tool_result_block(content: &str) -> String {
    content
        .split_once("\n\n[Tool Result]\n")
        .map(|(clean, _)| clean.to_string())
        .unwrap_or_else(|| content.to_string())
}

/// POST /api/agents/{id}/chat/attachments — upload and parse attachments for operational chat.
async fn api_operational_chat_upload_attachments(
    Path(id): Path<String>,
    mut multipart: Multipart,
) -> Result<Json<ChatAttachmentUploadResponse>, (axum::http::StatusCode, String)> {
    let dir = agent_dir(&id).ok_or_else(|| {
        (
            axum::http::StatusCode::NOT_FOUND,
            "Agent not found".to_string(),
        )
    })?;

    let mut uploads: Vec<UploadedAttachmentInput> = Vec::new();
    while let Some(field) = multipart.next_field().await.map_err(|e| {
        (
            axum::http::StatusCode::BAD_REQUEST,
            format!("Read multipart field: {}", e),
        )
    })? {
        let Some(file_name) = field.file_name().map(|s| s.to_string()) else {
            continue;
        };
        let content_type = field.content_type().map(|s| s.to_string());
        let bytes = field.bytes().await.map_err(|e| {
            (
                axum::http::StatusCode::BAD_REQUEST,
                format!("Read uploaded file bytes: {}", e),
            )
        })?;
        uploads.push(UploadedAttachmentInput {
            file_name,
            content_type,
            bytes: bytes.to_vec(),
        });
    }

    let attachments = store_uploaded_attachments(&dir, uploads).map_err(|e| {
        let status = if e.starts_with("upload validation:") {
            axum::http::StatusCode::BAD_REQUEST
        } else {
            axum::http::StatusCode::INTERNAL_SERVER_ERROR
        };
        (status, e)
    })?;

    Ok(Json(ChatAttachmentUploadResponse { attachments }))
}

/// GET /api/agents/{id}/chat/history — return persisted operational chat messages.
async fn api_operational_chat_history(
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, (axum::http::StatusCode, String)> {
    let dir = agent_dir(&id).ok_or_else(|| {
        (
            axum::http::StatusCode::NOT_FOUND,
            "Agent not found".to_string(),
        )
    })?;
    let path = dir.join("operational_chat.json");
    if !path.exists() {
        return Ok(Json(serde_json::json!({ "messages": [] })));
    }
    let content = std::fs::read_to_string(&path).map_err(|e| {
        (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("Read operational_chat.json: {}", e),
        )
    })?;
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
) -> Result<Json<OperationalChatResponseBody>, (axum::http::StatusCode, String)> {
    let dir = agent_dir(&id).ok_or_else(|| {
        (
            axum::http::StatusCode::NOT_FOUND,
            "Agent not found".to_string(),
        )
    })?;
    let config_path = agent_config_path(&id).ok_or_else(|| {
        (
            axum::http::StatusCode::NOT_FOUND,
            "Agent not found".to_string(),
        )
    })?;

    let user_message = body.message.trim().to_string();
    let attachment_ids = normalize_attachment_ids(&body.attachment_ids);
    if user_message.is_empty() && attachment_ids.is_empty() {
        return Err((
            axum::http::StatusCode::BAD_REQUEST,
            "message or attachment_ids is required".to_string(),
        ));
    }

    let chat_path = dir.join("operational_chat.json");
    let attachment_context_entries = if attachment_ids.is_empty() {
        Vec::new()
    } else {
        load_attachment_context(&dir, &attachment_ids)
            .map_err(|e| (axum::http::StatusCode::BAD_REQUEST, e))?
    };
    let has_attachments = !attachment_context_entries.is_empty();
    let attachment_context_block = if has_attachments {
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
    .map_err(|e| {
        (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("Task join: {}", e),
        )
    })?
    .map_err(|e| (axum::http::StatusCode::BAD_REQUEST, e))?;

    // Build system prompt from constitutional docs with dynamic skill tools
    let skill_tool_entries = build_skill_tool_entries(&state.skill_registry);

    // Load vault provider names for system prompt awareness
    let vault_providers: Vec<String> = {
        let vault = SecretsVault::load(config.data_dir.clone())
            .unwrap_or_else(|_| SecretsVault::new(config.data_dir.clone()));
        vault
            .list_providers()
            .into_iter()
            .map(String::from)
            .collect()
    };

    // Sync agent secrets into the shared skill vault so plugins can see them.
    sync_agent_vault_to_skills(&config.data_dir, &state.skill_vault);

    let system_prompt = if skill_tool_entries.is_empty() {
        build_system_prompt(&config.docs_dir, &config.agent_name)
    } else {
        build_system_prompt_with_skills(
            &config.docs_dir,
            &config.agent_name,
            &skill_tool_entries,
            &vault_providers,
        )
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
    messages.push(orion_capabilities::cognitive::Message::new(
        "user",
        &message_for_model,
    ));

    // Build operational router from config + SecretsVault
    let (ego_name, ego_key) = {
        let vault = SecretsVault::load(config.data_dir.clone())
            .unwrap_or_else(|_| SecretsVault::new(config.data_dir.clone()));
        resolve_ego_credentials_with_preference(
            &vault,
            config.active_provider_preference.as_deref(),
        )
    };
    let requested_router_mode = body.router_mode.unwrap_or(agentic::AgenticRouterMode::Auto);
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

    let router = orion_router::IdEgoRouter::with_provider_auto_detect(
        config.local_llm_base_url.clone(),
        ego_name.as_deref(),
        ego_key,
        ego_model,
        effective_routing_mode,
    )
    .await;

    // Redact API keys from user message before storing
    let redacted_user_message = redact_api_keys(&user_message_for_storage);

    tracing::info!(agent = %id, "operational_chat: sending chat turn");

    // Pro-mode council path: for Pro tier with 2+ providers, run MoA DAG in Rust.
    let response = if tier == ThinkingModelTier::Pro {
        let vault_for_pro = SecretsVault::load(config.data_dir.clone())
            .unwrap_or_else(|_| SecretsVault::new(config.data_dir.clone()));
        let council_providers =
            resolve_council_providers(&vault_for_pro, config.active_provider_preference.as_deref());
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
                    tracing::warn!(agent = %id, error = %e, "operational_chat: council failed, falling back to standard Ego");
                    router.route(messages.clone()).await.map_err(|e| {
                        (
                            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                            format!("Chat failed: {}", e),
                        )
                    })?
                }
            }
        } else {
            router.route(messages.clone()).await.map_err(|e| {
                (
                    axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                    format!("Chat failed: {}", e),
                )
            })?
        }
    } else {
        router.route(messages.clone()).await.map_err(|e| {
            tracing::error!(agent = %id, error = %e, "operational_chat: chat turn failed");
            (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("Chat failed: {}", e),
            )
        })?
    };

    // Parse tool_request blocks from LLM response
    let (clean_content, tool_requests) = parse_tool_requests(&response.content);

    tracing::info!(
        agent = %id,
        content_len = clean_content.len(),
        tool_count = tool_requests.len(),
        attachment_count = attachment_ids.len(),
        "operational_chat: chat turn complete"
    );

    let block_tool_execution = should_block_tool_execution_for_attachments(&attachment_ids);
    let mut blocked_tool_count: usize = 0;
    if block_tool_execution && !tool_requests.is_empty() {
        blocked_tool_count = tool_requests.len();
        tracing::info!(
            agent = %id,
            blocked_tool_count = blocked_tool_count,
            "operational_chat: blocked tool execution for attachment turn"
        );
    }

    // Execute skill tool requests (async — must happen outside spawn_blocking).
    // Supports multiple tool calls per turn for autonomous multi-step actions.
    let mut skill_tool_results: Vec<OperationalToolResult> = Vec::new();
    let mut skill_tool_outputs: Vec<String> = Vec::new();
    let mut skill_tool_log: Vec<OperationalToolLogEntry> = Vec::new();
    for tr in &tool_requests {
        if block_tool_execution {
            continue;
        }
        if tr.name == "store_secret"
            || tr.name == "store_provider_key"
            || tr.name == "store_vault_secret"
        {
            continue; // handled in the blocking section below
        }
        // Try to match against registered skill tools
        if let Some((skill_id, _tool_desc)) =
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
    let skill_tool_result = skill_tool_results.into_iter().last();
    let skill_tool_output_text = if skill_tool_outputs.is_empty() {
        None
    } else {
        Some(skill_tool_outputs.join("\n\n"))
    };

    // Tool feedback loop: if skill tools produced output, feed results back for ONE follow-up LLM call.
    // This prevents hallucination — the LLM sees actual tool results and can summarize them accurately.
    let mut followup_content: Option<String> = None;
    if let Some(ref output_text) = skill_tool_output_text {
        let feedback = format!("## Tool Results\n\n{}", output_text);
        messages.push(orion_capabilities::cognitive::Message::new(
            "assistant",
            &clean_content,
        ));
        messages.push(orion_capabilities::cognitive::Message::new(
            "user", &feedback,
        ));

        match router.route(messages).await {
            Ok(followup_response) => {
                let (followup_clean, _) = parse_tool_requests(&followup_response.content);
                followup_content = Some(followup_clean);
            }
            Err(e) => {
                tracing::warn!("operational_chat: follow-up turn failed: {}", e);
                // Fall back to original response — no followup_content set
            }
        }
    }

    // Blocking 2: execute credential tool requests, persist conversation with redacted content.
    let config_path_2 = config_path.clone();
    let raw_user_message = user_message.clone();
    let final_content = followup_content.as_deref().unwrap_or(&clean_content);
    let attachment_notice = if blocked_tool_count > 0 {
        Some(format!(
            "Attachment safety: I analyzed {} attachment(s) and did not execute tool requests in this turn. Ask explicitly if you want follow-up actions.",
            attachment_ids.len()
        ))
    } else {
        None
    };
    let mut final_content_for_storage = final_content.to_string();
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
        let allow_tool_execution = !block_tool_execution;
        move || -> Result<(Option<OperationalToolResult>, Vec<String>), String> {
            let mut config =
                AppConfig::load(&config_path_2).map_err(|e| format!("Load config: {}", e))?;
            let mut vault = SecretsVault::load(config.data_dir.clone())
                .unwrap_or_else(|_| SecretsVault::new(config.data_dir.clone()));
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
                        match execute_store_provider_key(&mut vault, &mut config, provider, key) {
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
                            vault.set_secret(key_name, value);
                            if let Err(e) = vault.save() {
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
                    match execute_store_provider_key(&mut vault, &mut config, provider, &key_str) {
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

            let providers: Vec<String> = vault
                .list_providers()
                .into_iter()
                .map(String::from)
                .collect();

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
    .map_err(|e| {
        (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("Task join: {}", e),
        )
    })?
    .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e))?;

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
    }))
}

// ============================================================================
// External Verification API
// ============================================================================

/// GET /api/agents/{id}/identity — public key bundle for external verification.
async fn api_agent_identity(
    Path(id): Path<String>,
) -> Result<Json<AgentIdentityBundle>, (axum::http::StatusCode, String)> {
    let dir = agent_dir(&id).ok_or_else(|| {
        (
            axum::http::StatusCode::NOT_FOUND,
            "Agent not found".to_string(),
        )
    })?;

    let pubkey_path = dir.join("external_pubkey.bin");
    if !pubkey_path.exists() {
        return Err((
            axum::http::StatusCode::NOT_FOUND,
            "No public key found — agent has not completed Darkness stage".to_string(),
        ));
    }

    let pubkey_bytes = std::fs::read(&pubkey_path).map_err(|e| {
        (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to read public key: {}", e),
        )
    })?;
    let pubkey_base64 = BASE64.encode(&pubkey_bytes);

    let config_path = dir.join("config.json");
    let (name, birth_complete, birth_date) = if config_path.exists() {
        match AppConfig::load(&config_path) {
            Ok(cfg) => (
                cfg.agent_name.clone(),
                cfg.birth_complete,
                resolve_birth_timestamp(&cfg, &dir),
            ),
            Err(_) => (None, false, None),
        }
    } else {
        (None, false, None)
    };

    // Verify Hive lineage if master key and lineage file exist.
    let lineage_verified = data_root()
        .and_then(|root| {
            let gc_path = GlobalConfig::config_path(&root);
            if !gc_path.exists() {
                return None;
            }
            let gc = GlobalConfig::load(&root).ok()?;
            let lineage_sig_path = dir.join("hive_lineage.sig");
            orion_core::verify_agent_lineage(
                &gc.master_key_path,
                &pubkey_path,
                &lineage_sig_path,
            )
            .ok()
        })
        .unwrap_or(false);

    Ok(Json(AgentIdentityBundle {
        agent_id: id,
        name,
        pubkey_base64,
        birth_complete,
        birth_date,
        lineage_verified,
    }))
}

/// GET /api/agents/{id}/constitution — signed constitutional documents.
async fn api_agent_constitution(
    Path(id): Path<String>,
) -> Result<Json<ConstitutionResponse>, (axum::http::StatusCode, String)> {
    let dir = agent_dir(&id).ok_or_else(|| {
        (
            axum::http::StatusCode::NOT_FOUND,
            "Agent not found".to_string(),
        )
    })?;

    let config_path = dir.join("config.json");
    if config_path.exists() {
        if let Ok(cfg) = AppConfig::load(&config_path) {
            if !cfg.birth_complete {
                return Err((
                    axum::http::StatusCode::BAD_REQUEST,
                    "Birth not yet complete — constitutional documents are not signed".to_string(),
                ));
            }
        }
    }

    let pubkey_path = dir.join("external_pubkey.bin");
    let pubkey_bytes = std::fs::read(&pubkey_path).map_err(|e| {
        (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to read public key: {}", e),
        )
    })?;
    let pubkey_base64 = BASE64.encode(&pubkey_bytes);

    let docs_dir = dir.join("docs");
    let doc_names = ["soul.md", "ethics.md", "instincts.md"];
    let mut documents = Vec::new();

    for doc_name in &doc_names {
        let doc_path = docs_dir.join(doc_name);
        let sig_path = docs_dir.join(format!("{}.sig", doc_name));

        if !doc_path.exists() || !sig_path.exists() {
            continue;
        }

        let content = std::fs::read_to_string(&doc_path).map_err(|e| {
            (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to read {}: {}", doc_name, e),
            )
        })?;
        let sig_json = std::fs::read_to_string(&sig_path).map_err(|e| {
            (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to read {}.sig: {}", doc_name, e),
            )
        })?;
        let meta: SigMeta = serde_json::from_str(&sig_json).map_err(|e| {
            (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("Invalid {}.sig JSON: {}", doc_name, e),
            )
        })?;

        documents.push(ConstitutionDocument {
            name: doc_name.to_string(),
            tier: meta.tier.as_str().to_string(),
            content,
            signature: meta.signature,
            signed_at: meta.signed_at.to_rfc3339(),
        });
    }

    Ok(Json(ConstitutionResponse {
        agent_id: id,
        pubkey_base64,
        documents,
    }))
}

/// POST /api/agents/{id}/verify — verify all constitutional document signatures.
async fn api_agent_verify(
    Path(id): Path<String>,
) -> Result<Json<VerifyResponse>, (axum::http::StatusCode, String)> {
    let dir = agent_dir(&id).ok_or_else(|| {
        (
            axum::http::StatusCode::NOT_FOUND,
            "Agent not found".to_string(),
        )
    })?;

    let config_path = dir.join("config.json");
    if config_path.exists() {
        if let Ok(cfg) = AppConfig::load(&config_path) {
            if !cfg.birth_complete {
                return Err((
                    axum::http::StatusCode::BAD_REQUEST,
                    "Birth not yet complete — nothing to verify".to_string(),
                ));
            }
        }
    }

    let pubkey_path = dir.join("external_pubkey.bin");
    let vault = orion_core::ReadOnlyFileVault::new(&pubkey_path);
    let verifier = Verifier::from_vault(&vault).map_err(|e| {
        (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to load public key: {}", e),
        )
    })?;

    let docs_dir = dir.join("docs");
    let doc_names = ["soul.md", "ethics.md", "instincts.md", "growth.md"];
    let mut results = Vec::new();
    let mut all_valid = true;

    for doc_name in &doc_names {
        let doc_path = docs_dir.join(doc_name);
        let sig_path = docs_dir.join(format!("{}.sig", doc_name));

        if !doc_path.exists() {
            results.push(DocumentVerifyResult {
                name: doc_name.to_string(),
                valid: false,
                error: Some("Document file not found".to_string()),
            });
            all_valid = false;
            continue;
        }
        if !sig_path.exists() {
            results.push(DocumentVerifyResult {
                name: doc_name.to_string(),
                valid: false,
                error: Some("Signature file not found".to_string()),
            });
            all_valid = false;
            continue;
        }

        let content = match std::fs::read_to_string(&doc_path) {
            Ok(c) => c,
            Err(e) => {
                results.push(DocumentVerifyResult {
                    name: doc_name.to_string(),
                    valid: false,
                    error: Some(format!("Failed to read document: {}", e)),
                });
                all_valid = false;
                continue;
            }
        };
        let sig_json = match std::fs::read_to_string(&sig_path) {
            Ok(s) => s,
            Err(e) => {
                results.push(DocumentVerifyResult {
                    name: doc_name.to_string(),
                    valid: false,
                    error: Some(format!("Failed to read signature: {}", e)),
                });
                all_valid = false;
                continue;
            }
        };
        let meta: SigMeta = match serde_json::from_str(&sig_json) {
            Ok(m) => m,
            Err(e) => {
                results.push(DocumentVerifyResult {
                    name: doc_name.to_string(),
                    valid: false,
                    error: Some(format!("Invalid signature JSON: {}", e)),
                });
                all_valid = false;
                continue;
            }
        };

        let doc = CoreDocument {
            name: doc_name.to_string(),
            tier: meta.tier,
            content,
            signature: meta.signature,
            signed_at: meta.signed_at,
        };

        match verifier.verify_document(&doc) {
            Ok(()) => {
                results.push(DocumentVerifyResult {
                    name: doc_name.to_string(),
                    valid: true,
                    error: None,
                });
            }
            Err(e) => {
                results.push(DocumentVerifyResult {
                    name: doc_name.to_string(),
                    valid: false,
                    error: Some(e.to_string()),
                });
                all_valid = false;
            }
        }
    }

    Ok(Json(VerifyResponse {
        agent_id: id,
        all_valid,
        results,
    }))
}

// ============================================================================
// Agent Import / Export API
// ============================================================================

/// POST /api/agents/import — import a portable agent identity bundle.
async fn api_import_agent(
    Json(body): Json<ImportAgentRequest>,
) -> Result<Json<ImportAgentResponse>, (axum::http::StatusCode, String)> {
    let private_key_base64 = body.private_key_base64.trim();
    if private_key_base64.is_empty() {
        return Err((
            axum::http::StatusCode::BAD_REQUEST,
            "private_key_base64 is required".to_string(),
        ));
    }

    let export = body.export;
    if export.export_version != 2 && export.export_version != 3 {
        return Err((
            axum::http::StatusCode::BAD_REQUEST,
            format!("Unsupported export_version: {}", export.export_version),
        ));
    }

    let root = data_root().ok_or_else(|| {
        (
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "ORION_DATA_DIR not set".to_string(),
        )
    })?;
    let identities_dir = root.join("identities");
    std::fs::create_dir_all(&identities_dir).map_err(|e| {
        (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to create identities dir: {}", e),
        )
    })?;

    let pubkey_base64 = export
        .identity
        .get("pubkey_base64")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            (
                axum::http::StatusCode::BAD_REQUEST,
                "Export identity.pubkey_base64 is required".to_string(),
            )
        })?;
    let pubkey_bytes = BASE64.decode(pubkey_base64).map_err(|e| {
        (
            axum::http::StatusCode::BAD_REQUEST,
            format!("Invalid identity.pubkey_base64: {}", e),
        )
    })?;
    let pubkey_array: [u8; 32] = pubkey_bytes.as_slice().try_into().map_err(|_| {
        (
            axum::http::StatusCode::BAD_REQUEST,
            "Export public key must be 32 bytes".to_string(),
        )
    })?;
    let private_signing_key = orion_core::parse_private_key(private_key_base64).map_err(|e| {
        (
            axum::http::StatusCode::UNAUTHORIZED,
            format!("Invalid private key: {}", e),
        )
    })?;
    if private_signing_key.verifying_key().to_bytes() != pubkey_array {
        return Err((
            axum::http::StatusCode::UNAUTHORIZED,
            "Private key does not match export identity".to_string(),
        ));
    }

    let constitution_obj = export.constitution.as_object().ok_or_else(|| {
        (
            axum::http::StatusCode::BAD_REQUEST,
            "Export constitution must be an object".to_string(),
        )
    })?;

    let imported_name = export
        .agent
        .get("name")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("Imported Agent")
        .to_string();
    let birth_complete = export
        .agent
        .get("birth_complete")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let birth_timestamp = export
        .agent
        .get("birth_timestamp")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let routing_mode = export
        .agent
        .get("routing_mode")
        .cloned()
        .and_then(|v| serde_json::from_value::<RoutingMode>(v).ok())
        .unwrap_or_default();

    let uuid = Uuid::new_v4().to_string();
    let agent_dir = identities_dir.join(&uuid);
    std::fs::create_dir_all(&agent_dir).map_err(|e| {
        (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to create agent dir: {}", e),
        )
    })?;
    let docs_dir = agent_dir.join("docs");
    std::fs::create_dir_all(&docs_dir).map_err(|e| {
        (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to create docs dir: {}", e),
        )
    })?;

    std::fs::write(agent_dir.join("external_pubkey.bin"), &pubkey_bytes).map_err(|e| {
        (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to write external public key: {}", e),
        )
    })?;
    let verifier = {
        let verify_vault =
            orion_core::ReadOnlyFileVault::new(agent_dir.join("external_pubkey.bin"));
        Verifier::from_vault(&verify_vault).map_err(|e| {
            (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to initialize signature verifier: {}", e),
            )
        })?
    };
    for doc_name in ["soul", "ethics", "instincts"] {
        let doc_export = constitution_obj
            .get(doc_name)
            .and_then(|v| v.as_object())
            .ok_or_else(|| {
                (
                    axum::http::StatusCode::BAD_REQUEST,
                    format!("Missing constitution document: {}", doc_name),
                )
            })?;
        let content = doc_export
            .get("content")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                (
                    axum::http::StatusCode::BAD_REQUEST,
                    format!("Invalid constitution content for {}", doc_name),
                )
            })?;
        let signature_value = doc_export.get("signature").cloned().ok_or_else(|| {
            (
                axum::http::StatusCode::BAD_REQUEST,
                format!("Missing signature for {}", doc_name),
            )
        })?;
        if signature_value.is_null() {
            return Err((
                axum::http::StatusCode::BAD_REQUEST,
                format!("Missing signature for {}", doc_name),
            ));
        }
        let meta: SigMeta = serde_json::from_value(signature_value).map_err(|e| {
            (
                axum::http::StatusCode::BAD_REQUEST,
                format!("Invalid signature metadata for {}: {}", doc_name, e),
            )
        })?;
        let doc = CoreDocument {
            name: format!("{}.md", doc_name),
            tier: meta.tier,
            content: content.to_string(),
            signature: meta.signature,
            signed_at: meta.signed_at,
        };
        verifier.verify_document(&doc).map_err(|e| {
            (
                axum::http::StatusCode::UNAUTHORIZED,
                format!("Invalid signature for {}.md: {}", doc_name, e),
            )
        })?;
    }

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
        birth_complete,
        birth_stage: None,
        external_pubkey_path: None,
        local_llm_base_url,
        routing_mode,
        trinity: None,
        agent_name: Some(imported_name.clone()),
        birth_timestamp,
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
    config.save(&agent_dir.join("config.json")).map_err(|e| {
        (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to save config: {}", e),
        )
    })?;

    let write_doc = |key: &str,
                     file_name: &str,
                     required: bool|
     -> Result<(), (axum::http::StatusCode, String)> {
        let Some(doc_value) = constitution_obj.get(key) else {
            if required {
                return Err((
                    axum::http::StatusCode::BAD_REQUEST,
                    format!("Missing constitution document: {}", key),
                ));
            }
            return Ok(());
        };
        let doc_obj = doc_value.as_object().ok_or_else(|| {
            (
                axum::http::StatusCode::BAD_REQUEST,
                format!("Invalid constitution object for {}", key),
            )
        })?;
        let content = match doc_obj.get("content").and_then(|v| v.as_str()) {
            Some(c) => c,
            None if required => {
                return Err((
                    axum::http::StatusCode::BAD_REQUEST,
                    format!("Missing constitution content for {}", key),
                ));
            }
            None => return Ok(()),
        };
        std::fs::write(docs_dir.join(file_name), content).map_err(|e| {
            (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to write {}: {}", file_name, e),
            )
        })?;

        if let Some(signature_value) = doc_obj.get("signature") {
            if !signature_value.is_null() {
                let sig_json = serde_json::to_string_pretty(signature_value).map_err(|e| {
                    (
                        axum::http::StatusCode::BAD_REQUEST,
                        format!("Invalid signature JSON for {}: {}", key, e),
                    )
                })?;
                std::fs::write(docs_dir.join(format!("{}.sig", file_name)), sig_json).map_err(
                    |e| {
                        (
                            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                            format!("Failed to write {}.sig: {}", file_name, e),
                        )
                    },
                )?;
            }
        }
        Ok(())
    };

    write_doc("soul", "soul.md", true)?;
    write_doc("ethics", "ethics.md", true)?;
    write_doc("instincts", "instincts.md", true)?;
    write_doc("growth", "growth.md", false)?;

    if let Some(path_value) = export.genesis_path {
        if !path_value.is_null() {
            let path_json = serde_json::to_string_pretty(&path_value).map_err(|e| {
                (
                    axum::http::StatusCode::BAD_REQUEST,
                    format!("Invalid genesis path JSON: {}", e),
                )
            })?;
            std::fs::write(agent_dir.join("genesis_path.json"), path_json).map_err(|e| {
                (
                    axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                    format!("Failed to write genesis_path.json: {}", e),
                )
            })?;
        }
    }

    if !export.chat_history.is_null() {
        let chat_obj = export.chat_history.as_object().ok_or_else(|| {
            (
                axum::http::StatusCode::BAD_REQUEST,
                "Export chat_history must be an object".to_string(),
            )
        })?;
        for (key, file_name) in [
            ("birth", "birth_chat.json"),
            ("connectivity", "connectivity_chat.json"),
            ("operational", "operational_chat.json"),
        ] {
            let Some(chat_value) = chat_obj.get(key) else {
                continue;
            };
            if chat_value.is_null() {
                continue;
            }
            let content = serde_json::to_string_pretty(chat_value).map_err(|e| {
                (
                    axum::http::StatusCode::BAD_REQUEST,
                    format!("Invalid chat history JSON for {}: {}", key, e),
                )
            })?;
            std::fs::write(agent_dir.join(file_name), content).map_err(|e| {
                (
                    axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                    format!("Failed to write {}: {}", file_name, e),
                )
            })?;
        }
    }

    if !export.agentic_runs.is_empty() {
        let runs_dir = agent_dir.join("agentic_runs");
        std::fs::create_dir_all(&runs_dir).map_err(|e| {
            (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to create agentic_runs dir: {}", e),
            )
        })?;
        for (idx, run) in export.agentic_runs.iter().enumerate() {
            let run_json = serde_json::to_string_pretty(run).map_err(|e| {
                (
                    axum::http::StatusCode::BAD_REQUEST,
                    format!("Invalid agentic run JSON: {}", e),
                )
            })?;
            let run_path = runs_dir.join(format!("run-{:04}.json", idx + 1));
            std::fs::write(run_path, run_json).map_err(|e| {
                (
                    axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                    format!("Failed to write agentic run file: {}", e),
                )
            })?;
        }
    }

    if let Some(orchestration_value) = export.orchestration {
        if !orchestration_value.is_null() {
            let orchestration_json =
                serde_json::to_string_pretty(&orchestration_value).map_err(|e| {
                    (
                        axum::http::StatusCode::BAD_REQUEST,
                        format!("Invalid orchestration JSON: {}", e),
                    )
                })?;
            std::fs::write(
                agent_dir.join("orchestration_jobs.json"),
                orchestration_json,
            )
            .map_err(|e| {
                (
                    axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                    format!("Failed to write orchestration_jobs.json: {}", e),
                )
            })?;
        }
    }

    if let Some(provider_secrets) = export
        .keychain
        .get("provider_secrets")
        .and_then(|v| v.as_object())
    {
        let mut vault = SecretsVault::new(agent_dir.clone());
        for (provider, secret_value) in provider_secrets {
            if let Some(secret) = secret_value.as_str() {
                if !secret.trim().is_empty() {
                    vault.set_secret(provider, secret);
                }
            }
        }
        vault.save().map_err(|e| {
            (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to save imported secrets vault: {}", e),
            )
        })?;
    }

    let gc_path = GlobalConfig::config_path(&root);
    let mut gc = if gc_path.exists() {
        GlobalConfig::load(&root).map_err(|e| {
            (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to load global config: {}", e),
            )
        })?
    } else {
        GlobalConfig::new(&root)
    };

    gc.register_agent(AgentEntry {
        id: uuid.clone(),
        name: imported_name.clone(),
        directory: PathBuf::from(format!("identities/{}", uuid)),
    })
    .map_err(|e| (axum::http::StatusCode::CONFLICT, e.to_string()))?;
    gc.save(&root).map_err(|e| {
        (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to save global config: {}", e),
        )
    })?;

    tracing::info!("Imported agent: {} ({})", imported_name, uuid);
    Ok(Json(ImportAgentResponse {
        id: uuid,
        name: imported_name,
    }))
}

/// GET /api/agents/{id}/export — export portable agent identity bundle as JSON download.
async fn api_export_agent(
    headers: axum::http::HeaderMap,
    Path(id): Path<String>,
) -> Result<
    (
        [(axum::http::header::HeaderName, String); 2],
        Json<AgentExport>,
    ),
    (axum::http::StatusCode, String),
> {
    let dir = agent_dir(&id).ok_or_else(|| {
        (
            axum::http::StatusCode::NOT_FOUND,
            "Agent not found".to_string(),
        )
    })?;

    // ---- Agent metadata from config.json ----
    let config_path = dir.join("config.json");
    let config_val: serde_json::Value = if config_path.exists() {
        let raw = std::fs::read_to_string(&config_path).map_err(|e| {
            (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to read config: {}", e),
            )
        })?;
        serde_json::from_str(&raw).unwrap_or_default()
    } else {
        serde_json::Value::Null
    };

    let agent_name = config_val
        .get("agent_name")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");
    let birth_complete = config_val
        .get("birth_complete")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    if !birth_complete {
        return Err((
            axum::http::StatusCode::BAD_REQUEST,
            "Cannot export agent — birth not yet complete".to_string(),
        ));
    }

    let private_key_base64 = headers
        .get("x-orion-private-key")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .trim();
    if private_key_base64.is_empty() {
        return Err((
            axum::http::StatusCode::BAD_REQUEST,
            "x-orion-private-key header is required to unlock keychain export".to_string(),
        ));
    }

    // Build sanitized agent metadata (exclude secrets, local_llm_base_url, API keys)
    let agent_meta = serde_json::json!({
        "id": id,
        "name": agent_name,
        "birth_complete": birth_complete,
        "birth_stage": config_val.get("birth_stage"),
        "birth_timestamp": config_val.get("birth_timestamp"),
        "routing_mode": config_val.get("routing_mode"),
    });

    // ---- Identity (public key) + private key gate for keychain unlock ----
    let pubkey_path = dir.join("external_pubkey.bin");
    let pubkey_bytes = std::fs::read(&pubkey_path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            (
                axum::http::StatusCode::BAD_REQUEST,
                "Cannot export agent — external public key is missing".to_string(),
            )
        } else {
            (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to read public key: {}", e),
            )
        }
    })?;
    if pubkey_bytes.len() != 32 {
        return Err((
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            "Invalid external public key length".to_string(),
        ));
    }

    let private_signing_key = orion_core::parse_private_key(private_key_base64).map_err(|e| {
        (
            axum::http::StatusCode::UNAUTHORIZED,
            format!("Invalid private key: {}", e),
        )
    })?;
    let derived_pubkey = private_signing_key.verifying_key().to_bytes();
    if derived_pubkey.as_slice() != pubkey_bytes.as_slice() {
        return Err((
            axum::http::StatusCode::UNAUTHORIZED,
            "Private key does not match this agent identity".to_string(),
        ));
    }

    let identity = serde_json::json!({ "pubkey_base64": BASE64.encode(&pubkey_bytes) });

    // ---- Keychain (requires private key unlock) ----
    let vault = SecretsVault::load(dir.clone()).map_err(|e| {
        (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to load secrets vault: {}", e),
        )
    })?;
    let mut provider_secrets = serde_json::Map::new();
    for provider in vault.list_providers() {
        if let Some(secret) = vault.get_secret(provider) {
            provider_secrets.insert(
                provider.to_string(),
                serde_json::Value::String(secret.to_string()),
            );
        }
    }
    let keychain = serde_json::json!({
        "unlocked_with_private_key": true,
        "provider_secrets": provider_secrets,
    });

    // ---- Constitutional documents + signatures ----
    let docs_dir = dir.join("docs");
    let doc_names = ["soul.md", "ethics.md", "instincts.md"];
    let mut constitution = serde_json::Map::new();
    for doc_name in &doc_names {
        let doc_path = docs_dir.join(doc_name);
        let sig_path = docs_dir.join(format!("{}.sig", doc_name));
        let key = doc_name.trim_end_matches(".md");

        let content = if doc_path.exists() {
            std::fs::read_to_string(&doc_path).ok()
        } else {
            None
        };
        let signature: Option<serde_json::Value> = if sig_path.exists() {
            std::fs::read_to_string(&sig_path)
                .ok()
                .and_then(|s| serde_json::from_str(&s).ok())
        } else {
            None
        };

        if let Some(c) = content {
            constitution.insert(
                key.to_string(),
                serde_json::json!({
                    "content": c,
                    "signature": signature,
                }),
            );
        }
    }

    // ---- Genesis path ----
    let genesis_path_file = dir.join("genesis_path.json");
    let genesis_path: Option<serde_json::Value> = if genesis_path_file.exists() {
        std::fs::read_to_string(&genesis_path_file)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
    } else {
        None
    };

    // ---- Chat history ----
    let read_json_file = |name: &str| -> serde_json::Value {
        let p = dir.join(name);
        if p.exists() {
            std::fs::read_to_string(&p)
                .ok()
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or(serde_json::Value::Null)
        } else {
            serde_json::Value::Null
        }
    };

    let chat_history = serde_json::json!({
        "birth": read_json_file("birth_chat.json"),
        "connectivity": read_json_file("connectivity_chat.json"),
        "operational": read_json_file("operational_chat.json"),
    });

    // ---- Agentic runs ----
    let mut agentic_runs: Vec<serde_json::Value> = Vec::new();
    let runs_dir = dir.join("agentic_runs");
    if runs_dir.is_dir() {
        if let Ok(entries) = std::fs::read_dir(&runs_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) != Some("json") {
                    continue;
                }
                if let Ok(content) = std::fs::read_to_string(&path) {
                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&content) {
                        agentic_runs.push(v);
                    }
                }
            }
        }
    }

    // ---- Orchestration jobs ----
    let orchestration_jobs_file = dir.join("orchestration_jobs.json");
    let orchestration = if orchestration_jobs_file.exists() {
        std::fs::read_to_string(&orchestration_jobs_file)
            .ok()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
    } else {
        None
    };

    let now = chrono::Utc::now().to_rfc3339();
    let safe_name = agent_name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect::<String>();
    let date_str = chrono::Utc::now().format("%Y%m%d").to_string();
    let filename = format!("orion-agent-{}-{}.json", safe_name, date_str);

    let export = AgentExport {
        export_version: 3,
        exported_at: now,
        agent: agent_meta,
        identity,
        keychain,
        constitution: serde_json::Value::Object(constitution),
        genesis_path,
        chat_history,
        agentic_runs,
        orchestration,
    };

    Ok((
        [
            (
                axum::http::header::CONTENT_TYPE,
                "application/json".to_string(),
            ),
            (
                axum::http::header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{}\"", filename),
            ),
        ],
        Json(export),
    ))
}

// ============================================================================
// Skills API
// ============================================================================

#[derive(Serialize)]
struct SkillToolInfo {
    name: String,
    description: String,
}

#[derive(Serialize)]
struct SkillInfo {
    id: String,
    name: String,
    description: String,
    trust_tier: String,
    tools: Vec<SkillToolInfo>,
}

#[derive(Deserialize)]
struct SkillExecuteRequest {
    tool: String,
    #[serde(default)]
    params: serde_json::Value,
}

#[derive(Serialize)]
struct SkillExecuteResponse {
    success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

/// GET /api/agents/{id}/skills — list registered skills with trust tiers and tools.
async fn api_list_skills(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Vec<SkillInfo>>, (axum::http::StatusCode, String)> {
    // Verify agent exists
    let _config_path = agent_config_path(&id).ok_or_else(|| {
        (
            axum::http::StatusCode::NOT_FOUND,
            "Agent not found".to_string(),
        )
    })?;

    let skills_with_tiers = state.skill_registry.list_with_tiers().map_err(|e| {
        (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to list skills: {}", e),
        )
    })?;

    let mut result = Vec::new();
    for (manifest, tier) in skills_with_tiers {
        // Get the skill to access its tools
        let tools = match state.skill_registry.get_skill(&manifest.id) {
            Ok((skill, _, _)) => skill
                .tools()
                .into_iter()
                .map(|t| SkillToolInfo {
                    name: t.name,
                    description: t.description,
                })
                .collect(),
            Err(_) => Vec::new(),
        };

        result.push(SkillInfo {
            id: manifest.id.0.clone(),
            name: manifest.name.clone(),
            description: manifest.description.clone(),
            trust_tier: tier.to_string(),
            tools,
        });
    }

    Ok(Json(result))
}

/// POST /api/agents/{id}/skills/{skill_id}/execute — execute a skill tool directly.
async fn api_execute_skill(
    State(state): State<AppState>,
    Path((id, skill_id)): Path<(String, String)>,
    Json(body): Json<SkillExecuteRequest>,
) -> Result<Json<SkillExecuteResponse>, (axum::http::StatusCode, String)> {
    // Verify agent exists and birth is complete
    let config_path = agent_config_path(&id).ok_or_else(|| {
        (
            axum::http::StatusCode::NOT_FOUND,
            "Agent not found".to_string(),
        )
    })?;

    let config = AppConfig::load(&config_path).map_err(|e| {
        (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("Load config: {}", e),
        )
    })?;

    if !config.birth_complete {
        return Err((
            axum::http::StatusCode::BAD_REQUEST,
            "Agent birth must be complete before using skills".to_string(),
        ));
    }

    let sid = orion_skills::manifest::SkillId(skill_id.clone());

    // Convert JSON params to ToolParams
    let mut tool_params = orion_skills::skill::ToolParams::new();
    if let serde_json::Value::Object(map) = &body.params {
        for (k, v) in map {
            tool_params = tool_params.with(k, v.clone());
        }
    }

    match state
        .skill_executor
        .execute(&sid, &body.tool, tool_params)
        .await
    {
        Ok(output) => Ok(Json(SkillExecuteResponse {
            success: output.success,
            data: output.data,
            error: output.error,
        })),
        Err(e) => Ok(Json(SkillExecuteResponse {
            success: false,
            data: None,
            error: Some(e.to_string()),
        })),
    }
}

/// GET /api/agents/{id}/skills/missing-secrets — list secrets needed by registered skills.
async fn api_skills_missing_secrets(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Vec<orion_skills::MissingSkillSecret>>, (axum::http::StatusCode, String)> {
    let dir = agent_dir(&id).ok_or_else(|| {
        (
            axum::http::StatusCode::NOT_FOUND,
            "Agent not found".to_string(),
        )
    })?;

    let skills_dir =
        PathBuf::from(std::env::var("ORION_SKILLS_DIR").unwrap_or_else(|_| "skills".to_string()));

    let missing = state
        .skill_registry
        .list_all_missing_secrets(&[skills_dir, dir]);

    Ok(Json(missing))
}

/// Copy all secrets from the agent's vault into the shared skill vault.
/// This ensures skills see the agent's actual API keys at execution time.
fn sync_agent_vault_to_skills(
    agent_data_dir: &std::path::Path,
    skill_vault: &Arc<Mutex<SecretsVault>>,
) {
    let agent_vault = match SecretsVault::load(agent_data_dir.to_path_buf()) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!("Failed to load agent vault for skill sync: {}", e);
            return;
        }
    };
    let entries: Vec<(String, String)> = agent_vault
        .list_providers()
        .into_iter()
        .filter_map(|name| {
            agent_vault
                .get_secret(name)
                .map(|val| (name.to_string(), val.to_string()))
        })
        .collect();
    if entries.is_empty() {
        return;
    }
    let mut shared = match skill_vault.lock() {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!("Failed to lock skill vault for sync: {}", e);
            return;
        }
    };
    for (name, val) in &entries {
        shared.set_secret(name, val);
    }
    tracing::debug!(
        count = entries.len(),
        "Synced agent vault secrets to skill vault"
    );
}

/// Initialize the skill registry: instantiate all built-in skill plugins
/// and register them as Verified (first-party, shipped with the repo).
fn init_skill_registry(vault: Arc<Mutex<SecretsVault>>) -> Arc<SkillRegistry> {
    let registry = SkillRegistry::with_secrets(Arc::clone(&vault));

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

    // --- Vault-backed skills (secrets loaded at execution time) ---

    // Web search via Tavily
    register_skill!(skill_web_search::WebSearchSkill::with_secrets(
        skill_web_search::WebSearchSkill::default_manifest(),
        Arc::clone(&vault),
    ));

    // Web browsing (HTTP fetch + optional Tavily/Perplexity fallback)
    register_skill!(skill_web_browse::WebBrowseSkill::with_secrets(
        skill_web_browse::WebBrowseSkill::default_manifest(),
        Arc::clone(&vault),
    ));

    // Perplexity AI search
    register_skill!(
        skill_perplexity_search::PerplexitySearchSkill::with_secrets(
            skill_perplexity_search::PerplexitySearchSkill::default_manifest(),
            Arc::clone(&vault),
        )
    );

    // Generic Email (IMAP/SMTP — supports multiple accounts/providers via config)
    register_skill!(skill_email::EmailSkill::with_secrets(
        skill_email::EmailSkill::default_manifest(),
        Arc::clone(&vault),
        Arc::new(tokio::sync::RwLock::new(Vec::new())),
    ));

    tracing::info!(
        registered = registered,
        failed = failed,
        "Skill registry initialized"
    );

    Arc::new(registry)
}

async fn has_active_agentic_task(state: &AppState, agent_id: &str) -> bool {
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

async fn launch_agentic_task_internal(
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
        cancel_tx,
        started_at,
    };

    let task_arc = Arc::new(TokioMutex::new(task));
    {
        let mut tasks = state.agentic_tasks.lock().await;
        tasks.insert(task_id.clone(), Arc::clone(&task_arc));
    }

    // Sync agent secrets into the shared skill vault so plugins can see them.
    sync_agent_vault_to_skills(&config.data_dir, &state.skill_vault);

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
        let vault = SecretsVault::load(config.data_dir.clone())
            .unwrap_or_else(|_| SecretsVault::new(config.data_dir.clone()));
        vault
            .list_providers()
            .into_iter()
            .map(String::from)
            .collect()
    };

    let loop_config = AgenticLoopConfig {
        task_id: task_id.clone(),
        goal,
        max_turns,
        auto_approve_safe_tools: body.auto_approve_safe_tools,
        router_mode: body.router_mode,
        agent_dir: dir,
        config,
        skill_registry: Arc::clone(&state.skill_registry),
        skill_executor: Arc::clone(&state.skill_executor),
        skill_tool_entries,
        stored_providers,
        event_tx,
        cancel_rx,
        task_handle: task_arc,
        started_at,
        run_source,
    };

    tokio::spawn(agentic::run_agentic_loop(loop_config));
    let stream_url = format!("/api/agents/{}/agent/stream?task={}", agent_id, task_id);
    Ok(AgenticRunResponse {
        task_id,
        stream_url,
    })
}

/// Resolve connected providers for council execution (priority-ordered).
fn resolve_council_providers(
    vault: &SecretsVault,
    preference: Option<&str>,
) -> Vec<(String, String)> {
    let mut result: Vec<(String, String)> = Vec::new();

    // If preference is set and has key, add it first.
    if let Some(pref) = preference {
        let normalized = AppConfig::normalize_provider_name(pref);
        if provider_supports_tier_models(&normalized) {
            if let Some(key) = vault.get_secret(&normalized) {
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
        if let Some(key) = vault.get_secret(pref) {
            result.push((pref.to_string(), key.to_string()));
            if result.len() >= 3 {
                break;
            }
        }
    }

    // Fill remaining from vault.
    if result.len() < 3 {
        for p in vault.list_providers() {
            let normalized = AppConfig::normalize_provider_name(p);
            if !provider_supports_tier_models(&normalized) || normalized == "tavily" {
                continue;
            }
            if result.iter().any(|(n, _)| *n == normalized) {
                continue;
            }
            if let Some(key) = vault.get_secret(p) {
                result.push((normalized, key.to_string()));
                if result.len() >= 3 {
                    break;
                }
            }
        }
    }

    result
}

fn append_operational_notice(agent_dir: &std::path::Path, content: &str) -> Result<(), String> {
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

// ============================================================================
// Agentic Loop Endpoints
// ============================================================================

#[derive(Deserialize)]
struct AgenticStreamQuery {
    task: String,
}

/// Info about a single agentic run (active or historical).
#[derive(Debug, Serialize)]
struct AgenticRunInfo {
    task_id: String,
    goal: String,
    status: String,
    turns: u32,
    tool_calls: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    summary: Option<String>,
    started_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    completed_at: Option<String>,
}

/// GET /api/agents/{id}/agent/runs — list agentic runs (active + historical).
async fn api_list_agentic_runs(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Vec<AgenticRunInfo>>, (axum::http::StatusCode, String)> {
    let dir = agent_dir(&id).ok_or_else(|| {
        (
            axum::http::StatusCode::NOT_FOUND,
            "Agent not found".to_string(),
        )
    })?;

    let mut runs: Vec<AgenticRunInfo> = Vec::new();

    // Collect in-memory active tasks for this agent
    {
        let tasks = state.agentic_tasks.lock().await;
        for task_arc in tasks.values() {
            let task = task_arc.lock().await;
            if task.agent_id == id {
                let status_str = match task.status {
                    AgenticTaskStatus::Running => "running",
                    AgenticTaskStatus::WaitingForMentor => "running",
                    AgenticTaskStatus::WaitingForConfirmation => "running",
                    AgenticTaskStatus::Completed => "completed",
                    AgenticTaskStatus::Failed => "failed",
                    AgenticTaskStatus::Cancelled => "cancelled",
                };
                runs.push(AgenticRunInfo {
                    task_id: task.id.clone(),
                    goal: task.goal.clone(),
                    status: status_str.to_string(),
                    turns: task.turn,
                    tool_calls: 0,
                    source: None,
                    summary: None,
                    started_at: task.started_at.to_rfc3339(),
                    completed_at: None,
                });
            }
        }
    }

    // Collect historical runs from disk
    let runs_dir = dir.join("agentic_runs");
    if runs_dir.is_dir() {
        if let Ok(entries) = std::fs::read_dir(&runs_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) != Some("json") {
                    continue;
                }
                if let Ok(content) = std::fs::read_to_string(&path) {
                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&content) {
                        let task_id = v
                            .get("task_id")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        // Skip if already present from in-memory tasks
                        if runs.iter().any(|r| r.task_id == task_id) {
                            continue;
                        }
                        runs.push(AgenticRunInfo {
                            task_id,
                            goal: v
                                .get("goal")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string(),
                            status: v
                                .get("status")
                                .and_then(|v| v.as_str())
                                .unwrap_or("completed")
                                .to_string(),
                            turns: v.get("turns").and_then(|v| v.as_u64()).unwrap_or(0) as u32,
                            tool_calls: v.get("tool_calls").and_then(|v| v.as_u64()).unwrap_or(0)
                                as u32,
                            source: v.get("source").and_then(|v| v.as_str()).map(String::from),
                            summary: v.get("summary").and_then(|v| v.as_str()).map(String::from),
                            started_at: v
                                .get("started_at")
                                .and_then(|v| v.as_str())
                                .unwrap_or_default()
                                .to_string(),
                            completed_at: v
                                .get("completed_at")
                                .and_then(|v| v.as_str())
                                .map(String::from),
                        });
                    }
                }
            }
        }
    }

    // Sort reverse chronological by started_at
    runs.sort_by(|a, b| b.started_at.cmp(&a.started_at));

    Ok(Json(runs))
}

/// POST /api/agents/{id}/agent/run — start an agentic task.
async fn api_agentic_run(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<AgenticRunRequest>,
) -> Result<Json<AgenticRunResponse>, (axum::http::StatusCode, String)> {
    let response = launch_agentic_task_internal(&state, &id, body, "manual".to_string())
        .await
        .map_err(|e| {
            let status = if e.contains("not found") || e.contains("Agent not found") {
                axum::http::StatusCode::NOT_FOUND
            } else if e.contains("running agentic task") {
                axum::http::StatusCode::CONFLICT
            } else if e.contains("goal is required") || e.contains("birth must be complete") {
                axum::http::StatusCode::BAD_REQUEST
            } else {
                axum::http::StatusCode::INTERNAL_SERVER_ERROR
            };
            (status, e)
        })?;

    tracing::info!(agent = %id, task = %response.task_id, "Started agentic task");
    Ok(Json(response))
}

/// GET /api/agents/{id}/agent/stream?task=<id> — SSE event stream for an agentic task.
async fn api_agentic_stream(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<AgenticStreamQuery>,
) -> Result<
    Sse<impl futures_core::Stream<Item = Result<Event, std::convert::Infallible>>>,
    (axum::http::StatusCode, String),
> {
    let tasks = state.agentic_tasks.lock().await;
    let task_arc = tasks.get(&query.task).ok_or_else(|| {
        (
            axum::http::StatusCode::NOT_FOUND,
            "Task not found".to_string(),
        )
    })?;

    let rx = {
        let task = task_arc.lock().await;
        if task.agent_id != id {
            return Err((
                axum::http::StatusCode::FORBIDDEN,
                "Task does not belong to this agent".to_string(),
            ));
        }
        task.event_tx.subscribe()
    };

    let stream = async_stream::stream! {
        let mut rx = rx;
        loop {
            match rx.recv().await {
                Ok(event) => {
                    let json = serde_json::to_string(&event).unwrap_or_default();
                    let event_name = match &event {
                        AgenticEvent::Thinking { .. } => "thinking",
                        AgenticEvent::ToolCall { .. } => "tool_call",
                        AgenticEvent::ToolResult { .. } => "tool_result",
                        AgenticEvent::MentorNeeded { .. } => "mentor_needed",
                        AgenticEvent::ConfirmationNeeded { .. } => "confirmation_needed",
                        AgenticEvent::Done { .. } => "done",
                        AgenticEvent::Error { .. } => "error",
                    };
                    yield Ok(Event::default().event(event_name).data(json));
                    if matches!(event, AgenticEvent::Done { .. } | AgenticEvent::Error { .. }) {
                        break;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    let msg = format!("{{\"skipped\":{}}}", n);
                    yield Ok(Event::default().event("lagged").data(msg));
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    break;
                }
            }
        }
    };

    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}

/// POST /api/agents/{id}/agent/respond — send mentor response to paused agentic task.
async fn api_agentic_respond(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<MentorResponseRequest>,
) -> Result<Json<serde_json::Value>, (axum::http::StatusCode, String)> {
    let tasks = state.agentic_tasks.lock().await;
    let task_arc = tasks.get(&body.task_id).ok_or_else(|| {
        (
            axum::http::StatusCode::NOT_FOUND,
            "Task not found".to_string(),
        )
    })?;

    let mut task = task_arc.lock().await;
    if task.agent_id != id {
        return Err((
            axum::http::StatusCode::FORBIDDEN,
            "Task does not belong to this agent".to_string(),
        ));
    }
    if task.status != AgenticTaskStatus::WaitingForMentor {
        return Err((
            axum::http::StatusCode::BAD_REQUEST,
            "Task is not waiting for mentor response".to_string(),
        ));
    }

    if let Some(tx) = task.mentor_response_tx.take() {
        let _ = tx.send(body.response);
    }

    Ok(Json(serde_json::json!({ "ok": true })))
}

/// POST /api/agents/{id}/agent/confirm — approve or deny a tool confirmation request.
async fn api_agentic_confirm(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<ConfirmationResponseRequest>,
) -> Result<Json<serde_json::Value>, (axum::http::StatusCode, String)> {
    let tasks = state.agentic_tasks.lock().await;
    let task_arc = tasks.get(&body.task_id).ok_or_else(|| {
        (
            axum::http::StatusCode::NOT_FOUND,
            "Task not found".to_string(),
        )
    })?;

    let mut task = task_arc.lock().await;
    if task.agent_id != id {
        return Err((
            axum::http::StatusCode::FORBIDDEN,
            "Task does not belong to this agent".to_string(),
        ));
    }
    if task.status != AgenticTaskStatus::WaitingForConfirmation {
        return Err((
            axum::http::StatusCode::BAD_REQUEST,
            "Task is not waiting for confirmation".to_string(),
        ));
    }

    if let Some(tx) = task.confirmation_tx.take() {
        let _ = tx.send(body.approved);
    }

    Ok(Json(serde_json::json!({ "ok": true })))
}

/// POST /api/agents/{id}/agent/cancel — cancel a running agentic task.
async fn api_agentic_cancel(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<CancelRequest>,
) -> Result<Json<serde_json::Value>, (axum::http::StatusCode, String)> {
    let tasks = state.agentic_tasks.lock().await;
    let task_arc = tasks.get(&body.task_id).ok_or_else(|| {
        (
            axum::http::StatusCode::NOT_FOUND,
            "Task not found".to_string(),
        )
    })?;

    let task = task_arc.lock().await;
    if task.agent_id != id {
        return Err((
            axum::http::StatusCode::FORBIDDEN,
            "Task does not belong to this agent".to_string(),
        ));
    }
    if matches!(
        task.status,
        AgenticTaskStatus::Completed | AgenticTaskStatus::Failed | AgenticTaskStatus::Cancelled
    ) {
        return Err((
            axum::http::StatusCode::BAD_REQUEST,
            "Task is already finished".to_string(),
        ));
    }

    let _ = task.cancel_tx.try_send(());

    Ok(Json(serde_json::json!({ "ok": true })))
}

/// GET /api/agents/{id}/agent/status?task=<id> — check task status.
async fn api_agentic_status(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<AgenticStreamQuery>,
) -> Result<Json<AgenticStatusResponse>, (axum::http::StatusCode, String)> {
    let tasks = state.agentic_tasks.lock().await;
    let task_arc = tasks.get(&query.task).ok_or_else(|| {
        (
            axum::http::StatusCode::NOT_FOUND,
            "Task not found".to_string(),
        )
    })?;

    let task = task_arc.lock().await;
    if task.agent_id != id {
        return Err((
            axum::http::StatusCode::FORBIDDEN,
            "Task does not belong to this agent".to_string(),
        ));
    }
    Ok(Json(AgenticStatusResponse {
        task_id: task.id.clone(),
        goal: task.goal.clone(),
        status: task.status,
        turn: task.turn,
        steps: task.steps.clone(),
    }))
}

// ============================================================================
// Orchestration Jobs Endpoints
// ============================================================================

/// GET /api/agents/{id}/orchestration/jobs — list scheduled orchestration jobs.
async fn api_orchestration_jobs(
    Path(id): Path<String>,
) -> Result<Json<OrchestrationJobsResponse>, (axum::http::StatusCode, String)> {
    let dir = agent_dir(&id).ok_or_else(|| {
        (
            axum::http::StatusCode::NOT_FOUND,
            "Agent not found".to_string(),
        )
    })?;
    let mut jobs =
        list_jobs(&dir).map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e))?;
    jobs.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(Json(OrchestrationJobsResponse { jobs }))
}

/// POST /api/agents/{id}/orchestration/jobs — create a scheduled orchestration job.
async fn api_orchestration_job_create(
    Path(id): Path<String>,
    Json(body): Json<CreateOrchestrationJobRequest>,
) -> Result<Json<OrchestrationJob>, (axum::http::StatusCode, String)> {
    let dir = agent_dir(&id).ok_or_else(|| {
        (
            axum::http::StatusCode::NOT_FOUND,
            "Agent not found".to_string(),
        )
    })?;
    let mut jobs =
        list_jobs(&dir).map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e))?;
    let job = create_job(&mut jobs, body, chrono::Utc::now())
        .map_err(|e| (axum::http::StatusCode::BAD_REQUEST, e))?;
    save_jobs(&dir, &jobs).map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e))?;
    Ok(Json(job))
}

/// POST /api/agents/{id}/orchestration/jobs/{job_id} — update an orchestration job.
async fn api_orchestration_job_update(
    Path((id, job_id)): Path<(String, String)>,
    Json(body): Json<UpdateOrchestrationJobRequest>,
) -> Result<Json<OrchestrationJob>, (axum::http::StatusCode, String)> {
    let dir = agent_dir(&id).ok_or_else(|| {
        (
            axum::http::StatusCode::NOT_FOUND,
            "Agent not found".to_string(),
        )
    })?;
    let mut jobs =
        list_jobs(&dir).map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e))?;
    let updated = update_job(&mut jobs, &job_id, body, chrono::Utc::now()).map_err(|e| {
        let status = if e.contains("not found") {
            axum::http::StatusCode::NOT_FOUND
        } else {
            axum::http::StatusCode::BAD_REQUEST
        };
        (status, e)
    })?;
    save_jobs(&dir, &jobs).map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e))?;
    Ok(Json(updated))
}

/// POST /api/agents/{id}/orchestration/jobs/{job_id}/enable — enable/disable a job.
async fn api_orchestration_job_enable(
    Path((id, job_id)): Path<(String, String)>,
    Json(body): Json<SetOrchestrationJobEnabledRequest>,
) -> Result<Json<OrchestrationJob>, (axum::http::StatusCode, String)> {
    let dir = agent_dir(&id).ok_or_else(|| {
        (
            axum::http::StatusCode::NOT_FOUND,
            "Agent not found".to_string(),
        )
    })?;
    let mut jobs =
        list_jobs(&dir).map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e))?;
    let updated =
        set_job_enabled(&mut jobs, &job_id, body.enabled, chrono::Utc::now()).map_err(|e| {
            let status = if e.contains("not found") {
                axum::http::StatusCode::NOT_FOUND
            } else {
                axum::http::StatusCode::BAD_REQUEST
            };
            (status, e)
        })?;
    save_jobs(&dir, &jobs).map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e))?;
    Ok(Json(updated))
}

/// POST /api/agents/{id}/orchestration/jobs/{job_id}/delete — delete an orchestration job.
async fn api_orchestration_job_delete(
    Path((id, job_id)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>, (axum::http::StatusCode, String)> {
    let dir = agent_dir(&id).ok_or_else(|| {
        (
            axum::http::StatusCode::NOT_FOUND,
            "Agent not found".to_string(),
        )
    })?;
    let mut jobs =
        list_jobs(&dir).map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e))?;
    if !delete_job(&mut jobs, &job_id) {
        return Err((
            axum::http::StatusCode::NOT_FOUND,
            "Job not found".to_string(),
        ));
    }
    save_jobs(&dir, &jobs).map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e))?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

/// POST /api/agents/{id}/orchestration/jobs/{job_id}/run — run one job immediately.
async fn api_orchestration_job_run_now(
    State(state): State<AppState>,
    Path((id, job_id)): Path<(String, String)>,
) -> Result<Json<JobRunNowResponse>, (axum::http::StatusCode, String)> {
    let dir = agent_dir(&id).ok_or_else(|| {
        (
            axum::http::StatusCode::NOT_FOUND,
            "Agent not found".to_string(),
        )
    })?;
    let mut jobs =
        list_jobs(&dir).map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e))?;
    let idx = jobs
        .iter()
        .position(|j| j.job_id == job_id)
        .ok_or_else(|| {
            (
                axum::http::StatusCode::NOT_FOUND,
                "Job not found".to_string(),
            )
        })?;

    let mut job = jobs.remove(idx);
    let run_result = run_orchestration_job_once(&state, &id, &dir, &mut job, "manual")
        .await
        .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e))?;
    jobs.insert(idx, job);
    save_jobs(&dir, &jobs).map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e))?;
    Ok(Json(run_result))
}

/// GET /api/agents/{id}/orchestration/logs — fetch recent orchestration logs.
async fn api_orchestration_logs(
    Path(id): Path<String>,
    Query(query): Query<JobLogsQuery>,
) -> Result<Json<OrchestrationLogsResponse>, (axum::http::StatusCode, String)> {
    let dir = agent_dir(&id).ok_or_else(|| {
        (
            axum::http::StatusCode::NOT_FOUND,
            "Agent not found".to_string(),
        )
    })?;
    let limit = query.limit.or(Some(50));
    let logs =
        load_logs(&dir, limit).map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e))?;
    Ok(Json(OrchestrationLogsResponse { logs }))
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

    // Load the secrets vault for the current agent (or a fresh one if none exists).
    // This is shared with skill plugins that need API keys at execution time.
    let skill_vault = {
        let vault_dir = resolve_config_path()
            .and_then(|p| p.parent().map(|d| d.to_path_buf()))
            .or_else(data_root)
            .unwrap_or_else(|| PathBuf::from("."));
        Arc::new(Mutex::new(
            SecretsVault::load(vault_dir.clone()).unwrap_or_else(|_| SecretsVault::new(vault_dir)),
        ))
    };

    // Initialize skill registry and executor
    let skill_registry = init_skill_registry(Arc::clone(&skill_vault));
    let skill_executor = Arc::new(SkillExecutor::new(Arc::clone(&skill_registry)));

    let state = AppState {
        memory_backend,
        local_llm_base_url: std::env::var("LOCAL_LLM_BASE_URL")
            .ok()
            .filter(|s| !s.is_empty()),
        birth_model: std::env::var("BIRTH_MODEL").ok().filter(|s| !s.is_empty()),
        forge_apps: Arc::new(Mutex::new(HashMap::new())),
        birth_keys,
        skill_registry,
        skill_executor,
        agentic_tasks: Arc::new(TokioMutex::new(HashMap::new())),
        skill_vault,
        orchestration_tick_seconds: 30,
    };

    // Start orchestration scheduler for periodic jobs (UTC cron semantics).
    tokio::spawn(run_orchestration_scheduler_loop(state.clone()));

    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    let app = Router::new()
        .route("/health", get(health))
        .route("/ready", get(ready))
        .route("/api/status", get(api_status))
        .route("/api/cognitive/registry", get(api_cognitive_registry))
        .route("/api/identities", get(api_identities))
        .route("/api/agents", post(api_create_agent))
        .route("/api/agents/import", post(api_import_agent))
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
        .route(
            "/api/agents/{id}/genesis/forge/state",
            get(api_genesis_forge_state),
        )
        .route(
            "/api/agents/{id}/genesis/forge/select",
            post(api_genesis_forge_select),
        )
        .route(
            "/api/agents/{id}/genesis/forge/crystallize",
            post(api_genesis_forge_crystallize),
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
        .route("/api/agents/{id}/skills", get(api_list_skills))
        .route(
            "/api/agents/{id}/skills/missing-secrets",
            get(api_skills_missing_secrets),
        )
        .route(
            "/api/agents/{id}/skills/{skill_id}/execute",
            post(api_execute_skill),
        )
        .route("/api/agents/{id}/agent/runs", get(api_list_agentic_runs))
        .route("/api/agents/{id}/agent/run", post(api_agentic_run))
        .route("/api/agents/{id}/agent/stream", get(api_agentic_stream))
        .route("/api/agents/{id}/agent/respond", post(api_agentic_respond))
        .route("/api/agents/{id}/agent/confirm", post(api_agentic_confirm))
        .route("/api/agents/{id}/agent/cancel", post(api_agentic_cancel))
        .route("/api/agents/{id}/agent/status", get(api_agentic_status))
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
        .route("/api/agents/{id}/identity", get(api_agent_identity))
        .route("/api/agents/{id}/constitution", get(api_agent_constitution))
        .route("/api/agents/{id}/verify", post(api_agent_verify))
        .route("/api/agents/{id}/export", get(api_export_agent))
        .layer(cors)
        .with_state(state);

    let addr = SocketAddr::from(([0, 0, 0, 0], 8080));
    tracing::info!("orion-api listening on {}", addr);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}
