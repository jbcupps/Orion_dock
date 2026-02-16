//! Proton Mail–style skill: implements Skill and EmailTransportCapability, wrapping orion-skills IMAP/SMTP.

mod transport;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use tokio::sync::RwLock;

use orion_core::secrets::SecretsVault;
use orion_skills::capability::email::{
    EmailProviderCapabilities, EmailTransportCapability, EmailTransportInfo, FetchOptions,
    OutgoingEmail,
};
use orion_skills::channel::{SkillEvent, TriggerDescriptor, TriggerFrequency, TriggerPriority};
use orion_skills::manifest::{
    CapabilityDescriptor, NetworkPermission, Permission, SkillId, SkillManifest,
};
use orion_skills::skill::{
    CostEstimate, ExecutionContext, HealthStatus, Skill, SkillConfig, SkillError, SkillHealth,
    SkillResult, ToolDescriptor, ToolOutput, ToolParams,
};
use orion_skills::transport::ImapClient;

use crate::transport::ProtonMailTransport;

/// Default skill ID for Proton Mail.
pub const PROTON_MAIL_SKILL_ID: &str = "com.orion.skills.proton-mail";

/// Proton Mail skill: Skill + EmailTransportCapability.
///
/// Supports two construction modes:
/// - `new(manifest)` — uninitialized, requires explicit `initialize()` call
/// - `with_secrets(manifest, vault)` — lazily connects on first tool execution
///   using "protonmail" (password) and "protonmail_user" (email) from the vault
pub struct ProtonMailSkill {
    manifest: SkillManifest,
    /// Lazily-initialized IMAP transport. Set by `initialize()` or on first tool call.
    transport: tokio::sync::OnceCell<Arc<RwLock<ProtonMailTransport>>>,
    /// Shared secrets vault for lazy credential lookup.
    vault: Option<Arc<Mutex<SecretsVault>>>,
    event_sender: Option<Arc<tokio::sync::broadcast::Sender<SkillEvent>>>,
}

impl ProtonMailSkill {
    /// Build manifest from embedded skill.toml or default in code.
    pub fn default_manifest() -> SkillManifest {
        SkillManifest::parse(include_str!("../skill.toml"))
            .unwrap_or_else(|_| Self::fallback_manifest())
    }

    fn fallback_manifest() -> SkillManifest {
        SkillManifest {
            id: SkillId(PROTON_MAIL_SKILL_ID.to_string()),
            name: "Proton Mail".to_string(),
            version: "0.1.0".to_string(),
            description: "Proton Mail–style email via IMAP/SMTP.".to_string(),
            license: None,
            category: "Communication".to_string(),
            keywords: vec![
                "email".into(),
                "proton".into(),
                "imap".into(),
                "smtp".into(),
            ],
            runtime: "Native".to_string(),
            min_orion_version: "0.1.0".to_string(),
            platforms: vec!["Windows".into(), "macOS".into(), "Linux".into()],
            capabilities: vec![CapabilityDescriptor {
                capability_type: "email_transport".to_string(),
                version: "1.0".to_string(),
                category: "protocol".to_string(),
            }],
            permissions: vec![Permission::Network(NetworkPermission::Domains(vec![
                "mail.proton.me".into(),
                "smtp.proton.me".into(),
            ]))],
            secrets: vec![
                orion_skills::SecretDescriptor {
                    name: "protonmail".to_string(),
                    description: "App password for IMAP".to_string(),
                    required: true,
                },
                orion_skills::SecretDescriptor {
                    name: "protonmail_user".to_string(),
                    description: "Email address for IMAP login".to_string(),
                    required: true,
                },
            ],
            config_defaults: HashMap::new(),
        }
    }

    pub fn new(manifest: SkillManifest) -> Self {
        Self {
            manifest,
            transport: tokio::sync::OnceCell::new(),
            vault: None,
            event_sender: None,
        }
    }

    /// Create with vault access — enables lazy IMAP initialization on first tool call.
    /// Reads "protonmail" (password) and "protonmail_user" (email address) from vault.
    pub fn with_secrets(manifest: SkillManifest, vault: Arc<Mutex<SecretsVault>>) -> Self {
        Self {
            manifest,
            transport: tokio::sync::OnceCell::new(),
            vault: Some(vault),
            event_sender: None,
        }
    }

    /// Lazily establish the IMAP transport from vault credentials.
    async fn ensure_transport(&self) -> SkillResult<&Arc<RwLock<ProtonMailTransport>>> {
        self.transport
            .get_or_try_init(|| async {
                let vault = self.vault.as_ref().ok_or_else(|| {
                    SkillError::InitFailed(
                        "No vault configured — call initialize() or use with_secrets()".into(),
                    )
                })?;

                let (user, password, host, port) = {
                    let guard = vault.lock().map_err(|e| {
                        SkillError::InitFailed(format!("Vault lock: {}", e))
                    })?;
                    let password = guard
                        .get_secret("protonmail")
                        .map(String::from)
                        .ok_or_else(|| {
                            SkillError::InitFailed(
                                "No 'protonmail' secret in vault — ask your mentor for the IMAP app password".into(),
                            )
                        })?;
                    let user = guard
                        .get_secret("protonmail_user")
                        .map(String::from)
                        .ok_or_else(|| {
                            SkillError::InitFailed(
                                "No 'protonmail_user' secret in vault — store the email address (e.g. user@proton.me)".into(),
                            )
                        })?;
                    // Proton Mail Bridge runs on localhost:1143.
                    // In Docker, use host.docker.internal to reach the host.
                    // Override via vault keys "protonmail_imap_host" / "protonmail_imap_port".
                    let host = guard
                        .get_secret("protonmail_imap_host")
                        .map(String::from)
                        .or_else(|| std::env::var("PROTONMAIL_IMAP_HOST").ok())
                        .unwrap_or_else(|| "host.docker.internal".to_string());
                    let port: u16 = guard
                        .get_secret("protonmail_imap_port")
                        .and_then(|s| s.parse().ok())
                        .or_else(|| std::env::var("PROTONMAIL_IMAP_PORT").ok().and_then(|s| s.parse().ok()))
                        .unwrap_or(1143);
                    (user, password, host, port)
                };

                tracing::info!(user = %user, host = %host, port = port, "ProtonMail: connecting IMAP via Bridge");
                // Bridge uses self-signed certs, so accept invalid certs
                let imap = ImapClient::new(&host, port, &user, &password, orion_core::config::TlsMode::Implicit)
                    .with_insecure_tls();
                imap.test_connection()
                    .await
                    .map_err(|e| SkillError::InitFailed(format!("IMAP connection failed: {}", e)))?;

                tracing::info!("ProtonMail: IMAP connected successfully");
                let transport = ProtonMailTransport::new(Some(imap), None);
                Ok(Arc::new(RwLock::new(transport)))
            })
            .await
    }

    fn tool_fetch_emails() -> ToolDescriptor {
        ToolDescriptor {
            name: "fetch_emails".to_string(),
            description: "Fetch unread emails from INBOX.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "limit": { "type": "integer", "description": "Max emails to fetch", "default": 50 },
                    "unread_only": { "type": "boolean", "default": true }
                }
            }),
            returns: serde_json::json!({ "type": "array", "items": { "type": "object" } }),
            cost_estimate: CostEstimate {
                latency_ms: 2000,
                network_bound: true,
                token_cost: None,
            },
            required_permissions: vec![Permission::Network(NetworkPermission::Domains(vec![
                "mail.proton.me".into(),
            ]))],
            autonomous: true,
            requires_confirmation: false,
        }
    }

    #[allow(dead_code)]
    fn tool_send_email() -> ToolDescriptor {
        ToolDescriptor {
            name: "send_email".to_string(),
            description: "Send an email (stub; not yet implemented).".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "to": { "type": "array", "items": { "type": "object" } },
                    "subject": { "type": "string" },
                    "body": { "type": "string" }
                },
                "required": ["to", "subject", "body"]
            }),
            returns: serde_json::json!({ "type": "object" }),
            cost_estimate: CostEstimate::default(),
            required_permissions: vec![Permission::Network(NetworkPermission::Domains(vec![
                "smtp.proton.me".into(),
            ]))],
            autonomous: false,
            requires_confirmation: true,
        }
    }

    #[allow(dead_code)]
    fn tool_classify_importance() -> ToolDescriptor {
        ToolDescriptor {
            name: "classify_importance".to_string(),
            description: "Classify email importance (stub; rule-based or future LLM).".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": { "email_id": { "type": "string" } }
            }),
            returns: serde_json::json!({ "type": "string", "enum": ["low", "normal", "high"] }),
            cost_estimate: CostEstimate::default(),
            required_permissions: vec![],
            autonomous: true,
            requires_confirmation: false,
        }
    }

    #[allow(dead_code)]
    fn tool_create_filter() -> ToolDescriptor {
        ToolDescriptor {
            name: "create_filter".to_string(),
            description: "Create a filter rule (stub; not yet implemented).".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string" },
                    "criteria": { "type": "object" }
                }
            }),
            returns: serde_json::json!({ "type": "object" }),
            cost_estimate: CostEstimate::default(),
            required_permissions: vec![],
            autonomous: false,
            requires_confirmation: true,
        }
    }
}

#[async_trait::async_trait]
impl Skill for ProtonMailSkill {
    fn manifest(&self) -> &SkillManifest {
        &self.manifest
    }

    async fn initialize(&mut self, config: SkillConfig) -> SkillResult<()> {
        self.event_sender = config.event_sender.clone();
        let host = config
            .values
            .get("imap_host")
            .and_then(|v| v.as_str())
            .map(String::from)
            .unwrap_or_else(|| "mail.proton.me".to_string());
        let port = config
            .values
            .get("imap_port")
            .and_then(|v| v.as_u64())
            .unwrap_or(993) as u16;
        let user = config
            .values
            .get("imap_user")
            .and_then(|v| v.as_str())
            .map(String::from)
            .or_else(|| config.secrets.get("imap_user").cloned())
            .unwrap_or_default();
        let password = config
            .secrets
            .get("imap_password")
            .cloned()
            .unwrap_or_default();

        if user.is_empty() || password.is_empty() {
            return Err(SkillError::InitFailed(
                "imap_user and imap_password (secret) required".to_string(),
            ));
        }

        let imap = ImapClient::new(&host, port, &user, &password, orion_core::config::TlsMode::Implicit);
        imap.test_connection()
            .await
            .map_err(|e| SkillError::InitFailed(e.to_string()))?;

        let transport = ProtonMailTransport::new(Some(imap), None);
        let _ = self.transport.set(Arc::new(RwLock::new(transport)));
        Ok(())
    }

    async fn shutdown(&mut self) -> SkillResult<()> {
        // OnceCell doesn't have a clear method, but dropping the skill handles cleanup
        Ok(())
    }

    fn health(&self) -> SkillHealth {
        let status = if self.transport.get().is_some() {
            HealthStatus::Healthy
        } else {
            HealthStatus::Unknown // No transport yet; will lazy-init on first call if vault is set
        };
        SkillHealth {
            status,
            message: None,
            last_check: chrono::Utc::now(),
            metrics: HashMap::new(),
        }
    }

    fn tools(&self) -> Vec<ToolDescriptor> {
        vec![Self::tool_fetch_emails()]
    }

    async fn execute_tool(
        &self,
        tool_name: &str,
        params: ToolParams,
        _context: &ExecutionContext,
    ) -> SkillResult<ToolOutput> {
        // Lazy-init: connect IMAP on first tool call using vault credentials
        let transport = self.ensure_transport().await?;

        match tool_name {
            "fetch_emails" => {
                let limit = params.get::<u64>("limit").unwrap_or(50);
                let unread_only = params.get::<bool>("unread_only").unwrap_or(true);
                let options = FetchOptions {
                    folder: None,
                    limit: Some(limit as u32),
                    unread_only,
                };
                let guard = transport.write().await;
                let emails = guard.fetch_emails(options).await?;
                let count = emails.len();
                let first_id = emails.first().map(|e| e.id.clone());
                let out = ToolOutput::success(
                    emails
                        .into_iter()
                        .map(|e| serde_json::json!({ "id": e.id, "from": e.from.email, "subject": e.subject, "date": e.date.to_rfc3339() }))
                        .collect::<Vec<_>>(),
                );
                if let Some(ref sender) = self.event_sender {
                    let _ = sender.send(SkillEvent {
                        skill_id: self.manifest.id.clone(),
                        trigger: "email_received".to_string(),
                        payload: serde_json::json!({ "count": count, "first_id": first_id }),
                        timestamp: chrono::Utc::now(),
                        priority: TriggerPriority::Normal,
                    });
                }
                Ok(out)
            }
            "send_email" => Ok(ToolOutput::error("send_email not yet implemented")),
            "classify_importance" => {
                let _email_id = params.get::<String>("email_id");
                Ok(ToolOutput::success(serde_json::json!("normal")))
            }
            "create_filter" => Ok(ToolOutput::error("create_filter not yet implemented")),
            _ => Err(SkillError::ToolFailed(format!(
                "Unknown tool: {}",
                tool_name
            ))),
        }
    }

    fn capabilities(&self) -> Vec<CapabilityDescriptor> {
        vec![CapabilityDescriptor {
            capability_type: "email_transport".to_string(),
            version: "1.0".to_string(),
            category: "protocol".to_string(),
        }]
    }

    fn get_capability(&self, cap_type: &str) -> Option<&dyn std::any::Any> {
        if cap_type == "email_transport" {
            Some(self)
        } else {
            None
        }
    }

    fn triggers(&self) -> Vec<TriggerDescriptor> {
        vec![
            TriggerDescriptor {
                name: "email_received".to_string(),
                description: "Fired when new email is received.".to_string(),
                payload_schema: serde_json::json!({ "email_id": "string" }),
                frequency: TriggerFrequency::Occasional,
                priority: TriggerPriority::Normal,
            },
            TriggerDescriptor {
                name: "important_email".to_string(),
                description: "Fired when an important email is detected.".to_string(),
                payload_schema: serde_json::json!({ "email_id": "string" }),
                frequency: TriggerFrequency::Rare,
                priority: TriggerPriority::High,
            },
        ]
    }
}

#[async_trait::async_trait]
impl EmailTransportCapability for ProtonMailSkill {
    fn info(&self) -> EmailTransportInfo {
        EmailTransportInfo {
            id: PROTON_MAIL_SKILL_ID.to_string(),
            name: self.manifest.name.clone(),
            capabilities: EmailProviderCapabilities {
                supports_send: true,
                supports_search: false,
                supports_labels: true,
                requires_bridge: true,
                send_via_smtp_token_only: false,
            },
        }
    }

    async fn connect(&mut self) -> SkillResult<()> {
        let transport = self.ensure_transport().await?;
        transport.read().await.test_connection().await
    }

    async fn disconnect(&mut self) -> SkillResult<()> {
        Ok(())
    }

    async fn fetch_emails(
        &self,
        options: FetchOptions,
    ) -> SkillResult<Vec<orion_skills::capability::email::Email>> {
        let transport = self.ensure_transport().await?;
        let guard = transport.read().await;
        guard.fetch_emails(options).await
    }

    async fn send_email(
        &self,
        email: OutgoingEmail,
    ) -> SkillResult<orion_skills::capability::email::SendResult> {
        let transport = self.ensure_transport().await?;
        let guard = transport.read().await;
        guard.send_email(email).await
    }

    async fn move_email(&self, _email_id: &str, _folder: &str) -> SkillResult<()> {
        Err(SkillError::ToolFailed(
            "move_email not yet implemented".to_string(),
        ))
    }

    async fn delete_email(&self, _email_id: &str) -> SkillResult<()> {
        Err(SkillError::ToolFailed(
            "delete_email not yet implemented".to_string(),
        ))
    }
}
