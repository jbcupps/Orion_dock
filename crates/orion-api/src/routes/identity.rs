use axum::{
    extract::Path,
    Json,
};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::str::FromStr;
use uuid::Uuid;

use orion_core::{
    AgentEntry, AppConfig, CoreDocument, GlobalConfig, MemoryBackend, ProviderKeyring, RoutingMode,
    SigMeta, Verifier, CONFIG_SCHEMA_VERSION,
};

use crate::{
    agent_dir, data_root, provider_names_from_keyring, resolve_birth_timestamp,
    ApiError,
};

#[derive(Serialize)]
pub(crate) struct AgentIdentityBundle {
    pub(crate) agent_id: String,
    pub(crate) name: Option<String>,
    pub(crate) pubkey_base64: String,
    pub(crate) birth_complete: bool,
    pub(crate) birth_date: Option<String>,
    pub(crate) lineage_verified: bool,
}

#[derive(Serialize)]
pub(crate) struct ConstitutionDocument {
    pub(crate) name: String,
    pub(crate) tier: String,
    pub(crate) content: String,
    pub(crate) signature: String,
    pub(crate) signed_at: String,
}

#[derive(Serialize)]
pub(crate) struct ConstitutionResponse {
    pub(crate) agent_id: String,
    pub(crate) pubkey_base64: String,
    pub(crate) documents: Vec<ConstitutionDocument>,
}

#[derive(Serialize)]
pub(crate) struct DocumentVerifyResult {
    pub(crate) name: String,
    pub(crate) valid: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) error: Option<String>,
}

#[derive(Serialize)]
pub(crate) struct VerifyResponse {
    pub(crate) agent_id: String,
    pub(crate) all_valid: bool,
    pub(crate) results: Vec<DocumentVerifyResult>,
}

#[derive(Deserialize)]
pub(crate) struct ExportRequest {
    pub(crate) private_key: String,
}

#[derive(Serialize)]
pub(crate) struct AgentExport {
    pub(crate) export_version: u32,
    pub(crate) exported_at: String,
    pub(crate) agent: serde_json::Value,
    pub(crate) identity: serde_json::Value,
    pub(crate) keychain: serde_json::Value,
    pub(crate) constitution: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) genesis_path: Option<serde_json::Value>,
    pub(crate) chat_history: serde_json::Value,
    pub(crate) agentic_runs: Vec<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) orchestration: Option<serde_json::Value>,
}

#[derive(Deserialize)]
pub(crate) struct ImportedAgentExport {
    pub(crate) export_version: u32,
    #[serde(default)]
    pub(crate) agent: serde_json::Value,
    #[serde(default)]
    pub(crate) identity: serde_json::Value,
    #[serde(default)]
    pub(crate) keychain: serde_json::Value,
    #[serde(default)]
    pub(crate) constitution: serde_json::Value,
    #[serde(default)]
    pub(crate) genesis_path: Option<serde_json::Value>,
    #[serde(default)]
    pub(crate) chat_history: serde_json::Value,
    #[serde(default)]
    pub(crate) agentic_runs: Vec<serde_json::Value>,
    #[serde(default)]
    pub(crate) orchestration: Option<serde_json::Value>,
}

#[derive(Deserialize)]
pub(crate) struct ImportAgentRequest {
    pub(crate) export: ImportedAgentExport,
    pub(crate) private_key_base64: String,
}

#[derive(Serialize)]
pub(crate) struct ImportAgentResponse {
    pub(crate) id: String,
    pub(crate) name: String,
}

/// GET /api/agents/{id}/identity — public key bundle for external verification.
pub(crate) async fn api_agent_identity(
    Path(id): Path<String>,
) -> Result<Json<AgentIdentityBundle>, ApiError> {
    let dir = agent_dir(&id).ok_or_else(|| {
        ApiError::NotFound("Agent not found".to_string())
    })?;

    let pubkey_path = dir.join("external_pubkey.bin");
    if !pubkey_path.exists() {
        return Err(ApiError::NotFound(
            "No public key found — agent has not completed Darkness stage".to_string(),
        ));
    }

    let pubkey_bytes = std::fs::read(&pubkey_path).map_err(|e| {
        ApiError::Internal(format!("Failed to read public key: {}", e))
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
            orion_core::verify_agent_lineage(&gc.master_key_path, &pubkey_path, &lineage_sig_path)
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
pub(crate) async fn api_agent_constitution(
    Path(id): Path<String>,
) -> Result<Json<ConstitutionResponse>, ApiError> {
    let dir = agent_dir(&id).ok_or_else(|| {
        ApiError::NotFound("Agent not found".to_string())
    })?;

    let config_path = dir.join("config.json");
    if config_path.exists() {
        if let Ok(cfg) = AppConfig::load(&config_path) {
            if !cfg.birth_complete {
                return Err(ApiError::BadRequest(
                    "Birth not yet complete — constitutional documents are not signed".to_string(),
                ));
            }
        }
    }

    let pubkey_path = dir.join("external_pubkey.bin");
    let pubkey_bytes = std::fs::read(&pubkey_path).map_err(|e| {
        ApiError::Internal(format!("Failed to read public key: {}", e))
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
            ApiError::Internal(format!("Failed to read {}: {}", doc_name, e))
        })?;
        let sig_json = std::fs::read_to_string(&sig_path).map_err(|e| {
            ApiError::Internal(format!("Failed to read {}.sig: {}", doc_name, e))
        })?;
        let meta: SigMeta = serde_json::from_str(&sig_json).map_err(|e| {
            ApiError::Internal(format!("Invalid {}.sig JSON: {}", doc_name, e))
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
pub(crate) async fn api_agent_verify(
    Path(id): Path<String>,
) -> Result<Json<VerifyResponse>, ApiError> {
    let dir = agent_dir(&id).ok_or_else(|| {
        ApiError::NotFound("Agent not found".to_string())
    })?;

    let config_path = dir.join("config.json");
    if config_path.exists() {
        if let Ok(cfg) = AppConfig::load(&config_path) {
            if !cfg.birth_complete {
                return Err(ApiError::BadRequest(
                    "Birth not yet complete — nothing to verify".to_string(),
                ));
            }
        }
    }

    let pubkey_path = dir.join("external_pubkey.bin");
    let vault = orion_core::ReadOnlyFileVault::new(&pubkey_path);
    let verifier = Verifier::from_vault(&vault).map_err(|e| {
        ApiError::Internal(format!("Failed to load public key: {}", e))
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

/// POST /api/agents/import — import a portable agent identity bundle.
pub(crate) async fn api_import_agent(
    Json(body): Json<ImportAgentRequest>,
) -> Result<Json<ImportAgentResponse>, ApiError> {
    let private_key_base64 = body.private_key_base64.trim();
    if private_key_base64.is_empty() {
        return Err(ApiError::BadRequest(
            "private_key_base64 is required".to_string(),
        ));
    }

    let export = body.export;
    if export.export_version != 2 && export.export_version != 3 {
        return Err(ApiError::BadRequest(
            format!("Unsupported export_version: {}", export.export_version),
        ));
    }

    let root = data_root().ok_or_else(|| {
        ApiError::ServiceUnavailable("ORION_DATA_DIR not set".to_string())
    })?;
    let identities_dir = root.join("identities");
    std::fs::create_dir_all(&identities_dir).map_err(|e| {
        ApiError::Internal(format!("Failed to create identities dir: {}", e))
    })?;

    let pubkey_base64 = export
        .identity
        .get("pubkey_base64")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            ApiError::BadRequest("Export identity.pubkey_base64 is required".to_string())
        })?;
    let pubkey_bytes = BASE64.decode(pubkey_base64).map_err(|e| {
        ApiError::BadRequest(format!("Invalid identity.pubkey_base64: {}", e))
    })?;
    let pubkey_array: [u8; 32] = pubkey_bytes.as_slice().try_into().map_err(|_| {
        ApiError::BadRequest("Export public key must be 32 bytes".to_string())
    })?;
    let private_signing_key = orion_core::parse_private_key(private_key_base64).map_err(|e| {
        ApiError::Unauthorized(format!("Invalid private key: {}", e))
    })?;
    if private_signing_key.verifying_key().to_bytes() != pubkey_array {
        return Err(ApiError::Unauthorized(
            "Private key does not match export identity".to_string(),
        ));
    }

    let constitution_obj = export.constitution.as_object().ok_or_else(|| {
        ApiError::BadRequest("Export constitution must be an object".to_string())
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
        ApiError::Internal(format!("Failed to create agent dir: {}", e))
    })?;
    let docs_dir = agent_dir.join("docs");
    std::fs::create_dir_all(&docs_dir).map_err(|e| {
        ApiError::Internal(format!("Failed to create docs dir: {}", e))
    })?;

    std::fs::write(agent_dir.join("external_pubkey.bin"), &pubkey_bytes).map_err(|e| {
        ApiError::Internal(format!("Failed to write external public key: {}", e))
    })?;
    let verifier = {
        let verify_vault =
            orion_core::ReadOnlyFileVault::new(agent_dir.join("external_pubkey.bin"));
        Verifier::from_vault(&verify_vault).map_err(|e| {
            ApiError::Internal(format!("Failed to initialize signature verifier: {}", e))
        })?
    };
    for doc_name in ["soul", "ethics", "instincts"] {
        let doc_export = constitution_obj
            .get(doc_name)
            .and_then(|v| v.as_object())
            .ok_or_else(|| {
                ApiError::BadRequest(format!("Missing constitution document: {}", doc_name))
            })?;
        let content = doc_export
            .get("content")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                ApiError::BadRequest(format!("Invalid constitution content for {}", doc_name))
            })?;
        let signature_value = doc_export.get("signature").cloned().ok_or_else(|| {
            ApiError::BadRequest(format!("Missing signature for {}", doc_name))
        })?;
        if signature_value.is_null() {
            return Err(ApiError::BadRequest(
                format!("Missing signature for {}", doc_name),
            ));
        }
        let meta: SigMeta = serde_json::from_value(signature_value).map_err(|e| {
            ApiError::BadRequest(format!("Invalid signature metadata for {}: {}", doc_name, e))
        })?;
        let doc = CoreDocument {
            name: format!("{}.md", doc_name),
            tier: meta.tier,
            content: content.to_string(),
            signature: meta.signature,
            signed_at: meta.signed_at,
        };
        verifier.verify_document(&doc).map_err(|e| {
            ApiError::Unauthorized(format!("Invalid signature for {}.md: {}", doc_name, e))
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
        ApiError::Internal(format!("Failed to save config: {}", e))
    })?;

    let write_doc = |key: &str,
                     file_name: &str,
                     required: bool|
     -> Result<(), ApiError> {
        let Some(doc_value) = constitution_obj.get(key) else {
            if required {
                return Err(ApiError::BadRequest(
                    format!("Missing constitution document: {}", key),
                ));
            }
            return Ok(());
        };
        let doc_obj = doc_value.as_object().ok_or_else(|| {
            ApiError::BadRequest(format!("Invalid constitution object for {}", key))
        })?;
        let content = match doc_obj.get("content").and_then(|v| v.as_str()) {
            Some(c) => c,
            None if required => {
                return Err(ApiError::BadRequest(
                    format!("Missing constitution content for {}", key),
                ));
            }
            None => return Ok(()),
        };
        std::fs::write(docs_dir.join(file_name), content).map_err(|e| {
            ApiError::Internal(format!("Failed to write {}: {}", file_name, e))
        })?;

        if let Some(signature_value) = doc_obj.get("signature") {
            if !signature_value.is_null() {
                let sig_json = serde_json::to_string_pretty(signature_value).map_err(|e| {
                    ApiError::BadRequest(format!("Invalid signature JSON for {}: {}", key, e))
                })?;
                std::fs::write(docs_dir.join(format!("{}.sig", file_name)), sig_json).map_err(
                    |e| {
                        ApiError::Internal(
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
                ApiError::BadRequest(format!("Invalid genesis path JSON: {}", e))
            })?;
            std::fs::write(agent_dir.join("genesis_path.json"), path_json).map_err(|e| {
                ApiError::Internal(format!("Failed to write genesis_path.json: {}", e))
            })?;
        }
    }

    if !export.chat_history.is_null() {
        let chat_obj = export.chat_history.as_object().ok_or_else(|| {
            ApiError::BadRequest("Export chat_history must be an object".to_string())
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
                ApiError::BadRequest(format!("Invalid chat history JSON for {}: {}", key, e))
            })?;
            std::fs::write(agent_dir.join(file_name), content).map_err(|e| {
                ApiError::Internal(format!("Failed to write {}: {}", file_name, e))
            })?;
        }
    }

    if !export.agentic_runs.is_empty() {
        let runs_dir = agent_dir.join("agentic_runs");
        std::fs::create_dir_all(&runs_dir).map_err(|e| {
            ApiError::Internal(format!("Failed to create agentic_runs dir: {}", e))
        })?;
        for (idx, run) in export.agentic_runs.iter().enumerate() {
            let run_json = serde_json::to_string_pretty(run).map_err(|e| {
                ApiError::BadRequest(format!("Invalid agentic run JSON: {}", e))
            })?;
            let run_path = runs_dir.join(format!("run-{:04}.json", idx + 1));
            std::fs::write(run_path, run_json).map_err(|e| {
                ApiError::Internal(format!("Failed to write agentic run file: {}", e))
            })?;
        }
    }

    if let Some(orchestration_value) = export.orchestration {
        if !orchestration_value.is_null() {
            let orchestration_json =
                serde_json::to_string_pretty(&orchestration_value).map_err(|e| {
                    ApiError::BadRequest(format!("Invalid orchestration JSON: {}", e))
                })?;
            std::fs::write(
                agent_dir.join("orchestration_jobs.json"),
                orchestration_json,
            )
            .map_err(|e| {
                ApiError::Internal(
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
        let mut keyring = ProviderKeyring::new(agent_dir.clone());
        for (provider, secret_value) in provider_secrets {
            if let Some(secret) = secret_value.as_str() {
                if !secret.trim().is_empty() {
                    keyring.set_key_str(provider, secret);
                }
            }
        }
        keyring.save().map_err(|e| {
            ApiError::Internal(format!("Failed to save imported provider keyring: {}", e))
        })?;
    }

    let gc_path = GlobalConfig::config_path(&root);
    let mut gc = if gc_path.exists() {
        GlobalConfig::load(&root).map_err(|e| {
            ApiError::Internal(format!("Failed to load global config: {}", e))
        })?
    } else {
        GlobalConfig::new(&root)
    };

    gc.register_agent(AgentEntry {
        id: uuid.clone(),
        name: imported_name.clone(),
        directory: PathBuf::from(format!("identities/{}", uuid)),
    })
    .map_err(|e| ApiError::Conflict(e.to_string()))?;
    gc.save(&root).map_err(|e| {
        ApiError::Internal(format!("Failed to save global config: {}", e))
    })?;

    tracing::info!("Imported agent: {} ({})", imported_name, uuid);
    Ok(Json(ImportAgentResponse {
        id: uuid,
        name: imported_name,
    }))
}

/// POST /api/agents/{id}/export — export portable agent identity bundle as JSON download.
pub(crate) async fn api_export_agent(
    Path(id): Path<String>,
    Json(body): Json<ExportRequest>,
) -> Result<
    (
        [(axum::http::header::HeaderName, String); 2],
        Json<AgentExport>,
    ),
    ApiError,
> {
    let dir = agent_dir(&id).ok_or_else(|| {
        ApiError::NotFound("Agent not found".to_string())
    })?;

    // ---- Agent metadata from config.json ----
    let config_path = dir.join("config.json");
    let config_val: serde_json::Value = if config_path.exists() {
        let raw = std::fs::read_to_string(&config_path).map_err(|e| {
            ApiError::Internal(format!("Failed to read config: {}", e))
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
        return Err(ApiError::BadRequest(
            "Cannot export agent — birth not yet complete".to_string(),
        ));
    }

    let private_key_base64 = body.private_key.trim();
    if private_key_base64.is_empty() {
        return Err(ApiError::BadRequest(
            "private_key field is required to unlock keychain export".to_string(),
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
            ApiError::BadRequest(
                "Cannot export agent — external public key is missing".to_string(),
            )
        } else {
            ApiError::Internal(format!("Failed to read public key: {}", e))
        }
    })?;
    if pubkey_bytes.len() != 32 {
        return Err(ApiError::Internal(
            "Invalid external public key length".to_string(),
        ));
    }

    let private_signing_key = orion_core::parse_private_key(private_key_base64).map_err(|e| {
        ApiError::Unauthorized(format!("Invalid private key: {}", e))
    })?;
    let derived_pubkey = private_signing_key.verifying_key().to_bytes();
    if derived_pubkey.as_slice() != pubkey_bytes.as_slice() {
        return Err(ApiError::Unauthorized(
            "Private key does not match this agent identity".to_string(),
        ));
    }

    let identity = serde_json::json!({ "pubkey_base64": BASE64.encode(&pubkey_bytes) });

    // ---- Keychain (requires private key unlock) ----
    let keyring = ProviderKeyring::load(dir.clone()).map_err(|e| {
        ApiError::Internal(format!("Failed to load provider keyring: {}", e))
    })?;
    let mut provider_secrets = serde_json::Map::new();
    for provider in provider_names_from_keyring(&keyring) {
        if let Some(secret) = keyring.get_key_str(&provider) {
            provider_secrets.insert(
                provider.clone(),
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
