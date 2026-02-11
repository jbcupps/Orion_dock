use axum::{
    extract::{Path, State},
    routing::{get, post},
    Json, Router,
};
use orion_birth::{GenesisPath, SoulCrystallizationDepth};
use orion_core::{
    validate_local_llm_url, AgentEntry, AppConfig, GlobalConfig, MemoryBackend, RoutingMode,
    CONFIG_SCHEMA_VERSION,
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

/// Read only birth_complete and birth_stage from config.json (no migrations).
fn read_birth_status() -> (bool, Option<String>) {
    let path = match resolve_config_path() {
        Some(p) => p,
        None => return (false, None),
    };
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return (false, None),
    };
    let value: serde_json::Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(_) => return (false, None),
    };
    let birth_complete = value
        .get("birth_complete")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let birth_stage = value
        .get("birth_stage")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    (birth_complete, birth_stage)
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

async fn api_status(State(state): State<AppState>) -> Json<StatusResponse> {
    let (birth_complete, birth_stage) = read_birth_status();
    Json(StatusResponse {
        memory_backend: state.memory_backend.as_str().to_string(),
        local_llm_configured: state.local_llm_base_url.is_some(),
        birth_model: state.birth_model.clone(),
        birth_complete,
        birth_stage,
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
    let result = tokio::task::spawn_blocking(move || -> Result<BirthStateResponse, String> {
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

        Ok(BirthStateResponse {
            stage: stage_name,
            private_key_base64,
        })
    })
    .await
    .map_err(|e| {
        (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            format!("Task join: {}", e),
        )
    })?
    .map_err(|e| (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e))?;

    Ok(Json(result))
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

    let state = AppState {
        memory_backend,
        local_llm_base_url: std::env::var("LOCAL_LLM_BASE_URL")
            .ok()
            .filter(|s| !s.is_empty()),
        birth_model: std::env::var("BIRTH_MODEL").ok().filter(|s| !s.is_empty()),
        forge_apps: std::sync::Arc::new(Mutex::new(HashMap::new())),
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
        .route("/api/genesis/paths", get(api_genesis_paths))
        .route("/api/agents/:id/genesis/start", post(api_genesis_start))
        .route(
            "/api/agents/:id/genesis/forge/select",
            post(api_genesis_forge_select),
        )
        .layer(cors)
        .with_state(state);

    let addr = SocketAddr::from(([0, 0, 0, 0], 8080));
    tracing::info!("orion-api listening on {}", addr);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}
