use axum::{
    extract::{Path, State},
    routing::{get, post},
    Json, Router,
};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use orion_birth::{
    birth_chat_turn, build_birth_messages, build_birth_router, build_genesis_messages,
    detect_provider_from_key, execute_store_provider_key, extract_api_keys_from_text,
    parse_tool_requests, redact_api_keys, BirthToolRequest, GenesisPath, SoulCrystallizationDepth,
};
use orion_core::system_prompt::build_system_prompt;
use orion_core::templates::{fill_soul_template, GROWTH_MD};
use orion_core::{
    validate_local_llm_url, AgentEntry, AppConfig, CoreDocument, GlobalConfig, MemoryBackend,
    RoutingMode, SecretsVault, SigMeta, Verifier, CONFIG_SCHEMA_VERSION,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Mutex;
use tower_http::cors::{Any, CorsLayer};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
use uuid::Uuid;

#[derive(Clone)]
struct AppState {
    memory_backend: MemoryBackend,
    local_llm_base_url: Option<String>,
    birth_model: Option<String>,
    /// Soul Forge app state per agent id (when Genesis path is Soul Forge).
    forge_apps: std::sync::Arc<Mutex<HashMap<String, soul_forge::App>>>,
    /// Ed25519 signing key bytes held per agent (from Darkness to Emergence).
    /// Keys are stored here after generation and retrieved at Emergence for document signing.
    birth_keys: std::sync::Arc<Mutex<HashMap<String, Vec<u8>>>>,
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

async fn health() -> Json<HealthResponse> {
    Json(HealthResponse { status: "ok" })
}

async fn ready(State(_state): State<AppState>) -> Json<HealthResponse> {
    Json(HealthResponse { status: "ok" })
}

async fn api_status(State(state): State<AppState>) -> Json<StatusResponse> {
    let (birth_complete, birth_stage, agent_name) = read_birth_status();
    Json(StatusResponse {
        memory_backend: state.memory_backend.as_str().to_string(),
        local_llm_configured: state.local_llm_base_url.is_some(),
        birth_model: state.birth_model.clone(),
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
                Ok(cfg) => (cfg.birth_complete, cfg.birth_timestamp.clone()),
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
        GlobalConfig::new(&root)
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

/// GET /api/agents/:id/birth/state — materialize Darkness if needed and return stage + one-time private key.
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

    // Run orchestrator on a blocking thread (PostgresStore creates its own Tokio runtime).
    let cp = config_path.clone();
    let id_clone = id.clone();
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

/// POST /api/agents/:id/birth/advance-darkness — user has saved the private key; advance to Ignition.
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

/// POST /api/agents/:id/birth/ignition — set local LLM URL (optional) and advance to Connectivity.
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

/// GET /api/agents/:id/genesis/state — read persisted genesis path (for session recovery).
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

/// GET /api/agents/:id/genesis/forge/state — read current Soul Forge session state (for session recovery).
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

    // Run orchestrator on a blocking thread (PostgresStore creates its own Tokio runtime).
    let path_clone = path.clone();
    let id_clone = id.clone();
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

        persist_genesis_path(&id_clone, &path_clone)
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

/// POST /api/agents/:id/genesis/forge/crystallize — produce soul from Soul Forge and crystallize.
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

/// GET /api/agents/:id/birth/chat/history — return persisted birth chat messages (for Direct Discovery).
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

/// POST /api/agents/:id/birth/chat — one turn of Direct Discovery Genesis chat; handles recommend_crystallize.
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

    // Guard: only Direct Discovery in Genesis stage.
    let gp_path = dir.join("genesis_path.json");
    let path_is_direct = gp_path
        .exists()
        .then(|| {
            std::fs::read_to_string(&gp_path).ok().and_then(|s| {
                serde_json::from_str::<serde_json::Value>(&s)
                    .ok()
                    .map(|v| v.get("path").and_then(|p| p.as_str()) == Some("direct"))
            })
        })
        .flatten()
        .unwrap_or(false);
    if !path_is_direct {
        return Err((
            axum::http::StatusCode::BAD_REQUEST,
            "Birth chat is only for Direct Discovery path in Genesis stage.".to_string(),
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
    tracing::info!(
        agent = %id,
        local_llm = ?config.local_llm_base_url,
        birth_model = %config.effective_birth_model(),
        message_count = messages.len(),
        "genesis_chat: building router"
    );
    let router = build_birth_router(&config).await;
    tracing::info!(agent = %id, "genesis_chat: sending chat turn");
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

/// GET /api/agents/:id/connectivity/providers — list currently stored provider names.
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

/// POST /api/agents/:id/connectivity/keys — store an API key directly (button channel, no LLM).
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

/// GET /api/agents/:id/connectivity/chat/history — return persisted connectivity chat messages.
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

/// POST /api/agents/:id/connectivity/chat — one turn of Connectivity stage chat.
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

/// POST /api/agents/:id/birth/complete-emergence — sign docs, write birth memory, drop key. Only when birth_stage is Emergence.
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
}

#[derive(Serialize)]
struct OperationalChatResponseBody {
    assistant_content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_executed: Option<OperationalToolResult>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stored_providers: Option<Vec<String>>,
}

#[derive(Serialize)]
struct OperationalToolResult {
    name: String,
    provider: String,
}

/// GET /api/agents/:id/chat/history — return persisted operational chat messages.
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
    Ok(Json(serde_json::json!({ "messages": messages })))
}

/// POST /api/agents/:id/chat — one turn of operational (post-birth) conversation.
async fn api_operational_chat(
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
    if user_message.is_empty() {
        return Err((
            axum::http::StatusCode::BAD_REQUEST,
            "message is required".to_string(),
        ));
    }

    let chat_path = dir.join("operational_chat.json");

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

    // Build system prompt from constitutional docs
    let system_prompt = build_system_prompt(&config.docs_dir, &config.agent_name);

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
        &user_message,
    ));

    // Build operational router from config + SecretsVault
    let (ego_name, ego_key) = {
        let vault = SecretsVault::load(config.data_dir.clone())
            .unwrap_or_else(|_| SecretsVault::new(config.data_dir.clone()));
        // Prefer anthropic > openai > first available
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
            // Fall back to first available non-tavily provider
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
    };

    tracing::info!(
        agent = %id,
        local_llm = ?config.local_llm_base_url,
        ego_provider = ?ego_name,
        routing_mode = ?config.routing_mode,
        message_count = messages.len(),
        "operational_chat: building router"
    );

    let router = orion_router::IdEgoRouter::with_provider_auto_detect(
        config.local_llm_base_url.clone(),
        ego_name.as_deref(),
        ego_key,
        config.routing_mode,
    )
    .await;

    // Redact API keys from user message before storing
    let redacted_user_message = redact_api_keys(&user_message);

    tracing::info!(agent = %id, "operational_chat: sending chat turn");
    let response = router.route(messages).await.map_err(|e| {
        tracing::error!(agent = %id, error = %e, "operational_chat: chat turn failed");
        (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("Chat failed: {}", e),
        )
    })?;

    // Parse tool_request blocks from LLM response
    let (clean_content, tool_requests) = parse_tool_requests(&response.content);

    tracing::info!(
        agent = %id,
        content_len = clean_content.len(),
        tool_count = tool_requests.len(),
        "operational_chat: chat turn complete"
    );

    // Blocking 2: execute tool requests, persist conversation with redacted content.
    let config_path_2 = config_path.clone();
    let raw_user_message = user_message.clone();
    let clean_content_clone = clean_content.clone();

    let (tool_result, final_providers) = tokio::task::spawn_blocking({
        let chat_path = chat_path.clone();
        let redacted_user = redacted_user_message.clone();
        let redacted_assistant = redact_api_keys(&clean_content);
        let tools = tool_requests.clone();
        move || -> Result<(Option<OperationalToolResult>, Vec<String>), String> {
            let mut config =
                AppConfig::load(&config_path_2).map_err(|e| format!("Load config: {}", e))?;
            let mut vault = SecretsVault::load(config.data_dir.clone())
                .unwrap_or_else(|_| SecretsVault::new(config.data_dir.clone()));
            let mut tool_executed: Option<OperationalToolResult> = None;

            // Execute store_secret / store_provider_key tool calls from LLM
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

            // Persist conversation with redacted content
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
            updated.push(BirthChatMessage {
                role: "assistant".to_string(),
                content: redacted_assistant,
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

    // Redact any credentials in the clean content sent back to the frontend
    let redacted_response = redact_api_keys(&clean_content_clone);

    Ok(Json(OperationalChatResponseBody {
        assistant_content: redacted_response,
        tool_executed: tool_result,
        stored_providers: if final_providers.is_empty() {
            None
        } else {
            Some(final_providers)
        },
    }))
}

// ============================================================================
// External Verification API
// ============================================================================

/// GET /api/agents/:id/identity — public key bundle for external verification.
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
                cfg.birth_timestamp.clone(),
            ),
            Err(_) => (None, false, None),
        }
    } else {
        (None, false, None)
    };

    Ok(Json(AgentIdentityBundle {
        agent_id: id,
        name,
        pubkey_base64,
        birth_complete,
        birth_date,
    }))
}

/// GET /api/agents/:id/constitution — signed constitutional documents.
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

/// POST /api/agents/:id/verify — verify all constitutional document signatures.
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
    let doc_names = ["soul.md", "ethics.md", "instincts.md"];
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

#[tokio::main]
async fn main() -> std::io::Result<()> {
    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::new(
            std::env::var("RUST_LOG").unwrap_or_else(|_| "orion_api=info,tower_http=info".into()),
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

    let state = AppState {
        memory_backend,
        local_llm_base_url: std::env::var("LOCAL_LLM_BASE_URL")
            .ok()
            .filter(|s| !s.is_empty()),
        birth_model: std::env::var("BIRTH_MODEL").ok().filter(|s| !s.is_empty()),
        forge_apps: std::sync::Arc::new(Mutex::new(HashMap::new())),
        birth_keys,
    };

    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    let app = Router::new()
        .route("/health", get(health))
        .route("/ready", get(ready))
        .route("/api/status", get(api_status))
        .route("/api/identities", get(api_identities))
        .route("/api/agents", post(api_create_agent))
        .route("/api/agents/:id/load", post(api_load_agent))
        .route("/api/agents/:id/birth/state", get(api_birth_state))
        .route(
            "/api/agents/:id/birth/advance-darkness",
            post(api_birth_advance_darkness),
        )
        .route("/api/agents/:id/birth/ignition", post(api_birth_ignition))
        .route(
            "/api/agents/:id/connectivity/providers",
            get(api_connectivity_providers),
        )
        .route(
            "/api/agents/:id/connectivity/keys",
            post(api_connectivity_store_key),
        )
        .route(
            "/api/agents/:id/connectivity/chat/history",
            get(api_connectivity_chat_history),
        )
        .route(
            "/api/agents/:id/connectivity/chat",
            post(api_connectivity_chat),
        )
        .route("/api/genesis/paths", get(api_genesis_paths))
        .route("/api/agents/:id/genesis/state", get(api_genesis_state))
        .route("/api/agents/:id/genesis/start", post(api_genesis_start))
        .route(
            "/api/agents/:id/genesis/forge/state",
            get(api_genesis_forge_state),
        )
        .route(
            "/api/agents/:id/genesis/forge/select",
            post(api_genesis_forge_select),
        )
        .route(
            "/api/agents/:id/genesis/forge/crystallize",
            post(api_genesis_forge_crystallize),
        )
        .route(
            "/api/agents/:id/birth/complete-emergence",
            post(api_birth_complete_emergence),
        )
        .route(
            "/api/agents/:id/birth/chat/history",
            get(api_birth_chat_history),
        )
        .route("/api/agents/:id/birth/chat", post(api_birth_chat))
        .route(
            "/api/agents/:id/chat/history",
            get(api_operational_chat_history),
        )
        .route("/api/agents/:id/chat", post(api_operational_chat))
        .route("/api/agents/:id/identity", get(api_agent_identity))
        .route("/api/agents/:id/constitution", get(api_agent_constitution))
        .route("/api/agents/:id/verify", post(api_agent_verify))
        .layer(cors)
        .with_state(state);

    let addr = SocketAddr::from(([0, 0, 0, 0], 8080));
    tracing::info!("orion-api listening on {}", addr);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}
