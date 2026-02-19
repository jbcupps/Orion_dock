use axum::{
    extract::{Path, State},
    Json,
};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use uuid::Uuid;

use orion_core::{AppConfig, SkillKeychain};
use orion_skills::manifest::SkillId;

use crate::{agent_config_path, agent_dir, ApiError, AppState};

#[derive(Serialize)]
pub(crate) struct SkillToolInfo {
    pub(crate) name: String,
    pub(crate) description: String,
}

#[derive(Serialize)]
pub(crate) struct SkillInfo {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) description: String,
    pub(crate) trust_tier: String,
    pub(crate) tools: Vec<SkillToolInfo>,
}

#[derive(Deserialize)]
pub(crate) struct SkillExecuteRequest {
    pub(crate) tool: String,
    #[serde(default)]
    pub(crate) params: serde_json::Value,
    /// Set to true on the second call to confirm a requires_confirmation tool.
    #[serde(default)]
    pub(crate) confirm: bool,
    /// Nonce from the first call's confirmation_required response.
    #[serde(default)]
    pub(crate) nonce: Option<String>,
}

#[derive(Serialize)]
pub(crate) struct SkillExecuteResponse {
    pub(crate) success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) data: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) confirmation_required: Option<ConfirmationInfo>,
}

#[derive(Serialize)]
pub(crate) struct ConfirmationInfo {
    pub(crate) tool: String,
    pub(crate) skill_id: String,
    pub(crate) nonce: String,
    pub(crate) message: String,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub(crate) struct RegisterEmailAccountRequest {
    /// Optional account id; auto-generated if not provided.
    #[serde(default)]
    pub(crate) id: Option<String>,
    pub(crate) provider: orion_core::config::EmailProvider,
    #[serde(default = "default_email_auth_type")]
    pub(crate) auth_type: orion_core::config::EmailAuthType,
    pub(crate) address: String,
    #[serde(default)]
    pub(crate) username: Option<String>,
    #[serde(default)]
    pub(crate) imap_host: Option<String>,
    #[serde(default)]
    pub(crate) imap_port: Option<u16>,
    #[serde(default)]
    pub(crate) smtp_host: Option<String>,
    #[serde(default)]
    pub(crate) smtp_port: Option<u16>,
    /// Optional security hint: auto/starttls/implicit/none.
    #[serde(default)]
    pub(crate) security: Option<String>,
    /// Password/token for this account. Stored under email:{id}:password in vault.
    #[serde(default)]
    pub(crate) password: Option<String>,
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum EmailSecurityPreference {
    Auto,
    Starttls,
    Implicit,
    None,
}

#[derive(Debug, Clone, serde::Serialize)]
pub(crate) struct EmailEndpointProbe {
    pub(crate) host: String,
    pub(crate) port: u16,
    pub(crate) reachable: bool,
    pub(crate) message: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub(crate) struct RegisterEmailAccountOutcome {
    pub(crate) ok: bool,
    pub(crate) account_id: String,
    pub(crate) provider: String,
    pub(crate) auth_type: String,
    pub(crate) address: String,
    pub(crate) username: String,
    pub(crate) imap_host: String,
    pub(crate) imap_port: u16,
    pub(crate) smtp_host: String,
    pub(crate) smtp_port: u16,
    pub(crate) imap_tls: orion_core::config::TlsMode,
    pub(crate) smtp_tls: orion_core::config::TlsMode,
    pub(crate) remapped_to_container_ingress: bool,
    pub(crate) probes: Vec<EmailEndpointProbe>,
}

pub(crate) fn default_email_auth_type() -> orion_core::config::EmailAuthType {
    orion_core::config::EmailAuthType::AppPassword
}

pub(crate) fn parse_email_security_preference(raw: Option<&str>) -> EmailSecurityPreference {
    match raw.unwrap_or("auto").trim().to_ascii_lowercase().as_str() {
        "starttls" => EmailSecurityPreference::Starttls,
        "implicit" | "ssl" | "tls" => EmailSecurityPreference::Implicit,
        "none" | "plaintext" => EmailSecurityPreference::None,
        _ => EmailSecurityPreference::Auto,
    }
}

pub(crate) fn infer_tls_mode(
    security: EmailSecurityPreference,
    port: u16,
    fallback: orion_core::config::TlsMode,
) -> orion_core::config::TlsMode {
    match security {
        EmailSecurityPreference::Starttls => orion_core::config::TlsMode::Starttls,
        EmailSecurityPreference::Implicit => orion_core::config::TlsMode::Implicit,
        EmailSecurityPreference::None => orion_core::config::TlsMode::None,
        EmailSecurityPreference::Auto => match port {
            // Include Proton Bridge defaults and standard STARTTLS ports.
            143 | 587 | 1025 | 1143 => orion_core::config::TlsMode::Starttls,
            465 | 993 => orion_core::config::TlsMode::Implicit,
            _ => fallback,
        },
    }
}

pub(crate) fn is_loopback_host(host: &str) -> bool {
    let normalized = host.trim().to_ascii_lowercase();
    normalized == "127.0.0.1" || normalized == "localhost" || normalized == "host.docker.internal"
}

pub(crate) fn normalize_account_id(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for ch in raw.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
            out.push(ch.to_ascii_lowercase());
        } else {
            out.push('_');
        }
    }
    let compact = out.trim_matches('_');
    if compact.is_empty() {
        uuid::Uuid::new_v4().to_string()
    } else {
        compact.to_string()
    }
}

pub(crate) async fn probe_email_endpoint(host: &str, port: u16) -> EmailEndpointProbe {
    let address = format!("{}:{}", host, port);
    match tokio::time::timeout(
        std::time::Duration::from_secs(3),
        tokio::net::TcpStream::connect(&address),
    )
    .await
    {
        Ok(Ok(_)) => EmailEndpointProbe {
            host: host.to_string(),
            port,
            reachable: true,
            message: "Connected".to_string(),
        },
        Ok(Err(e)) => EmailEndpointProbe {
            host: host.to_string(),
            port,
            reachable: false,
            message: format!("Connect failed: {}", e),
        },
        Err(_) => EmailEndpointProbe {
            host: host.to_string(),
            port,
            reachable: false,
            message: "Connect timeout".to_string(),
        },
    }
}

pub(crate) async fn register_email_account_internal(
    agent_id: &str,
    skill_keychain: &Arc<Mutex<SkillKeychain>>,
    email_accounts: &Arc<tokio::sync::RwLock<Vec<orion_core::config::EmailAccountConfig>>>,
    body: RegisterEmailAccountRequest,
) -> Result<RegisterEmailAccountOutcome, String> {
    let dir = agent_dir(agent_id).ok_or_else(|| "Agent not found".to_string())?;
    let config_path =
        agent_config_path(agent_id).ok_or_else(|| "Agent config not found".to_string())?;
    let mut config = AppConfig::load(&config_path).map_err(|e| format!("Load config: {}", e))?;

    let provider = body.provider;
    let preset = orion_core::config::provider_preset(provider);
    let security_pref = parse_email_security_preference(body.security.as_deref());

    let mut imap_host = body
        .imap_host
        .clone()
        .or_else(|| preset.as_ref().map(|p| p.imap_host.to_string()))
        .ok_or_else(|| "imap_host is required for this provider".to_string())?;
    let mut smtp_host = body
        .smtp_host
        .clone()
        .or_else(|| preset.as_ref().map(|p| p.smtp_host.to_string()))
        .ok_or_else(|| "smtp_host is required for this provider".to_string())?;

    let mut remapped_to_container_ingress = false;
    let running_in_container = std::env::var("ORION_CONTAINER")
        .ok()
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);
    if provider == orion_core::config::EmailProvider::Proton && running_in_container {
        let ingress_host = std::env::var("ORION_EMAIL_BRIDGE_HOST")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| "protonbridge_ingress".to_string());
        if is_loopback_host(&imap_host) {
            imap_host = ingress_host.clone();
            remapped_to_container_ingress = true;
        }
        if is_loopback_host(&smtp_host) {
            smtp_host = ingress_host;
            remapped_to_container_ingress = true;
        }
    }

    let imap_port = body
        .imap_port
        .or_else(|| preset.as_ref().map(|p| p.imap_port))
        .unwrap_or(993);
    let smtp_port = body
        .smtp_port
        .or_else(|| preset.as_ref().map(|p| p.smtp_port))
        .unwrap_or(587);
    let imap_tls = infer_tls_mode(
        security_pref,
        imap_port,
        preset
            .as_ref()
            .map(|p| p.imap_tls)
            .unwrap_or(orion_core::config::TlsMode::Implicit),
    );
    let smtp_tls = infer_tls_mode(
        security_pref,
        smtp_port,
        preset
            .as_ref()
            .map(|p| p.smtp_tls)
            .unwrap_or(orion_core::config::TlsMode::Starttls),
    );

    let account_id = body.id.unwrap_or_else(|| {
        normalize_account_id(&format!(
            "{}_{}",
            format!("{:?}", provider).to_ascii_lowercase(),
            body.address
        ))
    });
    let username = body
        .username
        .clone()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| body.address.clone());

    let account = orion_core::config::EmailAccountConfig {
        id: account_id.clone(),
        provider,
        auth_type: body.auth_type,
        address: body.address.clone(),
        username: Some(username.clone()),
        imap_host: Some(imap_host.clone()),
        imap_port: Some(imap_port),
        imap_tls: Some(imap_tls),
        smtp_host: Some(smtp_host.clone()),
        smtp_port: Some(smtp_port),
        smtp_tls: Some(smtp_tls),
        scopes_granted: Vec::new(),
        status: orion_core::config::EmailAccountStatus::Active,
        last_verified_at: None,
    };

    config.email_accounts.retain(|a| a.id != account_id);
    config.email_accounts.push(account.clone());
    config
        .save(&config_path)
        .map_err(|e| format!("Save config: {}", e))?;

    if let Some(password) = &body.password {
        let mut kc =
            SkillKeychain::load(dir.clone()).unwrap_or_else(|_| SkillKeychain::new(dir.clone()));
        let email_key = format!("email:{}:password", account_id);
        kc.set_secret(&email_key, password);
        kc.save().map_err(|e| format!("Save keychain: {}", e))?;
        if let Ok(mut shared) = skill_keychain.lock() {
            shared.set_secret(&email_key, password);
        }
    }

    {
        let mut accounts = email_accounts.write().await;
        accounts.retain(|a| a.id != account_id);
        accounts.push(account);
    }

    let imap_probe = probe_email_endpoint(&imap_host, imap_port).await;
    let smtp_probe = probe_email_endpoint(&smtp_host, smtp_port).await;

    Ok(RegisterEmailAccountOutcome {
        ok: true,
        account_id,
        provider: format!("{:?}", provider).to_ascii_lowercase(),
        auth_type: format!("{:?}", body.auth_type).to_ascii_lowercase(),
        address: body.address,
        username,
        imap_host,
        imap_port,
        smtp_host,
        smtp_port,
        imap_tls,
        smtp_tls,
        remapped_to_container_ingress,
        probes: vec![imap_probe, smtp_probe],
    })
}

/// GET /api/agents/{id}/skills — list registered skills with trust tiers and tools.
pub(crate) async fn api_list_skills(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Vec<SkillInfo>>, ApiError> {
    // Verify agent exists
    let _config_path =
        agent_config_path(&id).ok_or_else(|| ApiError::NotFound("Agent not found".to_string()))?;

    let skills_with_tiers = state
        .skill_registry
        .list_with_tiers()
        .map_err(|e| ApiError::Internal(format!("Failed to list skills: {}", e)))?;

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
pub(crate) async fn api_execute_skill(
    State(state): State<AppState>,
    Path((id, skill_id)): Path<(String, String)>,
    Json(body): Json<SkillExecuteRequest>,
) -> Result<Json<SkillExecuteResponse>, ApiError> {
    // Verify agent exists and birth is complete
    let config_path =
        agent_config_path(&id).ok_or_else(|| ApiError::NotFound("Agent not found".to_string()))?;

    let config = AppConfig::load(&config_path)
        .map_err(|e| ApiError::Internal(format!("Load config: {}", e)))?;

    if !config.birth_complete {
        return Err(ApiError::BadRequest(
            "Agent birth must be complete before using skills".to_string(),
        ));
    }

    let sid = SkillId(skill_id.clone());
    let envelope = orion_core::evaluate_user_message_for_capabilities("", None);
    let gate = orion_core::evaluate_tool_request(&body.tool, &body.params, &envelope);
    if !gate.allowed {
        let code = gate.reason_code.unwrap_or("SAFETY_BLOCK");
        let text = gate
            .reason_text
            .unwrap_or_else(|| "Blocked by capability safety policy".to_string());
        return Ok(Json(SkillExecuteResponse {
            success: false,
            data: None,
            error: Some(format!("Blocked by safety policy ({}): {}", code, text)),
            confirmation_required: None,
        }));
    }

    // --- Confirmation enforcement ---
    if state.skill_executor.requires_confirmation(&sid, &body.tool) {
        if body.confirm {
            // Validate nonce
            let nonce = body.nonce.as_deref().unwrap_or("");
            let mut nonces = state.skill_confirm_nonces.lock().await;
            // Clean expired nonces (>5 min)
            let now = std::time::Instant::now();
            nonces.retain(|_, (_, _, created)| now.duration_since(*created).as_secs() < 300);
            match nonces.remove(nonce) {
                Some((ref ns, ref nt, created))
                    if *ns == skill_id
                        && *nt == body.tool
                        && now.duration_since(created).as_secs() < 300 =>
                {
                    // Valid nonce — fall through to execution
                }
                Some(_) => {
                    return Err(ApiError::BadRequest(
                        "Confirmation nonce does not match this tool/skill".to_string(),
                    ));
                }
                None => {
                    return Err(ApiError::BadRequest(
                        "Invalid or expired confirmation nonce".to_string(),
                    ));
                }
            }
        } else {
            // Issue nonce
            let nonce = Uuid::new_v4().to_string();
            let mut nonces = state.skill_confirm_nonces.lock().await;
            // Clean expired nonces
            let now = std::time::Instant::now();
            nonces.retain(|_, (_, _, created)| now.duration_since(*created).as_secs() < 300);
            nonces.insert(nonce.clone(), (skill_id.clone(), body.tool.clone(), now));
            return Ok(Json(SkillExecuteResponse {
                success: false,
                data: None,
                error: None,
                confirmation_required: Some(ConfirmationInfo {
                    tool: body.tool.clone(),
                    skill_id: skill_id.clone(),
                    nonce,
                    message: format!(
                        "Tool '{}' in skill '{}' requires confirmation before execution",
                        body.tool, skill_id
                    ),
                }),
            }));
        }
    }

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
            confirmation_required: None,
        })),
        Err(e) => Ok(Json(SkillExecuteResponse {
            success: false,
            data: None,
            error: Some(e.to_string()),
            confirmation_required: None,
        })),
    }
}

/// GET /api/agents/{id}/skills/missing-secrets — list secrets needed by registered skills.
pub(crate) async fn api_skills_missing_secrets(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Vec<orion_skills::MissingSkillSecret>>, ApiError> {
    let dir = agent_dir(&id).ok_or_else(|| ApiError::NotFound("Agent not found".to_string()))?;

    let skills_dir =
        PathBuf::from(std::env::var("ORION_SKILLS_DIR").unwrap_or_else(|_| "skills".to_string()));

    let missing = state
        .skill_registry
        .list_all_missing_secrets(&[skills_dir, dir]);

    Ok(Json(missing))
}

/// POST /api/agents/{id}/email/accounts — register an email account for the email skill.
pub(crate) async fn api_register_email_account(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<RegisterEmailAccountRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let outcome =
        register_email_account_internal(&id, &state.skill_keychain, &state.email_accounts, body)
            .await
            .map_err(|e| ApiError::BadRequest(e))?;
    tracing::info!(
        agent = %id,
        account_id = %outcome.account_id,
        address = %outcome.address,
        remapped = outcome.remapped_to_container_ingress,
        "Registered email account"
    );
    Ok(Json(serde_json::to_value(outcome).unwrap_or_default()))
}

/// Sync email account configurations from the agent's AppConfig into the shared
/// email accounts list used by the email skill. Also ensures the keychain password
/// is stored under the `email:{id}:password` key pattern that the skill expects.
pub(crate) async fn sync_email_accounts(
    config: &AppConfig,
    email_accounts: &tokio::sync::RwLock<Vec<orion_core::config::EmailAccountConfig>>,
    skill_keychain: &Arc<Mutex<SkillKeychain>>,
) {
    let mut accounts = email_accounts.write().await;
    if config.email_accounts.is_empty() {
        return;
    }

    *accounts = config.email_accounts.clone();

    // Also migrate keychain secrets: if the agent has protonmail_bridge_password but
    // no email:{id}:password key, create the mapping so the email skill can find it.
    if let Ok(mut kc) = skill_keychain.lock() {
        for acct in &config.email_accounts {
            let email_key = format!("email:{}:password", acct.id);
            if kc.get_secret(&email_key).is_none() {
                // Try common key patterns for the password
                let candidate_keys = [
                    "protonmail_bridge_password".to_string(),
                    format!("{}_bridge_password", acct.id),
                    format!("{}_password", acct.id),
                ];
                for candidate in &candidate_keys {
                    if let Some(pw) = kc.get_secret(candidate).map(|s| s.to_string()) {
                        kc.set_secret(&email_key, &pw);
                        tracing::info!(
                            account = %acct.id,
                            "Mapped keychain secret {} -> {}",
                            candidate,
                            email_key
                        );
                        break;
                    }
                }
            }
        }
    }

    tracing::debug!(
        count = accounts.len(),
        "Synced email accounts from agent config"
    );
}
