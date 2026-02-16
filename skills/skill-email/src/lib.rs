//! Generic email skill: fetch, send, search, and manage mail via IMAP/SMTP.
//!
//! Composes transport capabilities (IMAP, SMTP, service discovery) from `orion-skills`
//! into agent-facing tools. Supports multiple accounts with provider auto-detection.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use orion_core::config::{
    provider_preset, EmailAccountConfig, EmailAccountStatus, EmailProvider, TlsMode,
};
use orion_core::email_auth;
use orion_core::secrets::SecretsVault;
use orion_skills::channel::{SkillEvent, TriggerDescriptor, TriggerFrequency, TriggerPriority};
use orion_skills::manifest::{
    CapabilityDescriptor, NetworkPermission, Permission, SkillId, SkillManifest,
};
use orion_skills::skill::{
    CostEstimate, ExecutionContext, HealthStatus, Skill, SkillConfig, SkillError, SkillHealth,
    SkillResult, ToolDescriptor, ToolOutput, ToolParams,
};
use orion_skills::transport::discovery;
use orion_skills::transport::{ImapClient, SearchCriteria, SmtpClient};
use tokio::sync::RwLock;

pub const EMAIL_SKILL_ID: &str = "com.orion.skills.email";

/// Generic email skill supporting multiple accounts and providers.
pub struct EmailSkill {
    manifest: SkillManifest,
    vault: Option<Arc<Mutex<SecretsVault>>>,
    accounts: Arc<RwLock<Vec<EmailAccountConfig>>>,
    event_sender: Option<Arc<tokio::sync::broadcast::Sender<SkillEvent>>>,
}

impl EmailSkill {
    pub fn default_manifest() -> SkillManifest {
        SkillManifest::parse(include_str!("../skill.toml"))
            .unwrap_or_else(|_| Self::fallback_manifest())
    }

    fn fallback_manifest() -> SkillManifest {
        SkillManifest {
            id: SkillId(EMAIL_SKILL_ID.to_string()),
            name: "Email".to_string(),
            version: "0.1.0".to_string(),
            description: "Generic email via IMAP/SMTP with provider auto-detection.".to_string(),
            license: None,
            category: "Communication".to_string(),
            keywords: vec![
                "email".into(),
                "imap".into(),
                "smtp".into(),
                "starttls".into(),
            ],
            runtime: "Native".to_string(),
            min_orion_version: "0.1.0".to_string(),
            platforms: vec!["Windows".into(), "macOS".into(), "Linux".into()],
            capabilities: vec![
                CapabilityDescriptor {
                    capability_type: "imap_transport".to_string(),
                    version: "1.0".to_string(),
                    category: "protocol".to_string(),
                },
                CapabilityDescriptor {
                    capability_type: "smtp_transport".to_string(),
                    version: "1.0".to_string(),
                    category: "protocol".to_string(),
                },
                CapabilityDescriptor {
                    capability_type: "service_discovery".to_string(),
                    version: "1.0".to_string(),
                    category: "protocol".to_string(),
                },
            ],
            permissions: vec![Permission::Network(NetworkPermission::Full)],
            secrets: vec![],
            config_defaults: HashMap::new(),
        }
    }

    pub fn new(manifest: SkillManifest) -> Self {
        Self {
            manifest,
            vault: None,
            accounts: Arc::new(RwLock::new(Vec::new())),
            event_sender: None,
        }
    }

    /// Create with vault and account list for credential lookup.
    pub fn with_secrets(
        manifest: SkillManifest,
        vault: Arc<Mutex<SecretsVault>>,
        accounts: Arc<RwLock<Vec<EmailAccountConfig>>>,
    ) -> Self {
        Self {
            manifest,
            vault: Some(vault),
            accounts,
            event_sender: None,
        }
    }

    /// Resolve an account by ID, or return the first active account.
    async fn resolve_account(
        &self,
        account_id: Option<&str>,
    ) -> SkillResult<EmailAccountConfig> {
        let accounts = self.accounts.read().await;
        if accounts.is_empty() {
            return Err(SkillError::ToolFailed(
                "No email accounts configured. Use discover_email_service to find settings, \
                 then configure an account via the API."
                    .to_string(),
            ));
        }

        if let Some(id) = account_id {
            accounts
                .iter()
                .find(|a| a.id == id)
                .cloned()
                .ok_or_else(|| {
                    SkillError::ToolFailed(format!(
                        "Account '{}' not found. Available: {}",
                        id,
                        accounts
                            .iter()
                            .map(|a| a.id.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ))
                })
        } else {
            accounts
                .iter()
                .find(|a| a.status == EmailAccountStatus::Active)
                .or_else(|| accounts.first())
                .cloned()
                .ok_or_else(|| SkillError::ToolFailed("No accounts available".to_string()))
        }
    }

    /// Get IMAP credentials from vault for an account.
    fn get_credentials(&self, account: &EmailAccountConfig) -> SkillResult<(String, String)> {
        let vault = self.vault.as_ref().ok_or_else(|| {
            SkillError::MissingSecret("No vault configured for credential lookup".to_string())
        })?;
        let guard = vault
            .lock()
            .map_err(|e| SkillError::ToolFailed(format!("Vault lock: {}", e)))?;

        let password = email_auth::get_password(&guard, &account.id)
            .ok_or_else(|| {
                SkillError::MissingSecret(format!(
                    "No password/token for account '{}'. Store via: email:{}:password",
                    account.id, account.id
                ))
            })?;
        Ok((account.address.clone(), password))
    }

    /// Build an IMAP client for an account using config + provider presets.
    fn make_imap_client(&self, account: &EmailAccountConfig) -> SkillResult<ImapClient> {
        let (user, password) = self.get_credentials(account)?;
        let preset = provider_preset(account.provider);

        let host = account
            .imap_host
            .clone()
            .or_else(|| preset.as_ref().map(|p| p.imap_host.to_string()))
            .ok_or_else(|| {
                SkillError::ToolFailed(format!(
                    "No IMAP host for account '{}'. Set imap_host in config or use a known provider.",
                    account.id
                ))
            })?;
        let port = account
            .imap_port
            .or_else(|| preset.as_ref().map(|p| p.imap_port))
            .unwrap_or(993);
        let tls_mode = preset
            .as_ref()
            .map(|p| p.imap_tls)
            .unwrap_or(TlsMode::Implicit);

        let client = ImapClient::new(&host, port, &user, &password, tls_mode);

        let needs_insecure = account.provider == EmailProvider::Proton
            || host == "127.0.0.1"
            || host == "localhost"
            || host == "host.docker.internal";

        if needs_insecure {
            Ok(client.with_insecure_tls())
        } else {
            Ok(client)
        }
    }

    /// Build an SMTP client for an account using config + provider presets.
    fn make_smtp_client(&self, account: &EmailAccountConfig) -> SkillResult<SmtpClient> {
        let (user, password) = self.get_credentials(account)?;
        let preset = provider_preset(account.provider);

        let host = account
            .smtp_host
            .clone()
            .or_else(|| preset.as_ref().map(|p| p.smtp_host.to_string()))
            .ok_or_else(|| {
                SkillError::ToolFailed(format!(
                    "No SMTP host for account '{}'. Set smtp_host in config or use a known provider.",
                    account.id
                ))
            })?;
        let port = account
            .smtp_port
            .or_else(|| preset.as_ref().map(|p| p.smtp_port))
            .unwrap_or(587);
        let tls_mode = preset
            .as_ref()
            .map(|p| p.smtp_tls)
            .unwrap_or(TlsMode::Starttls);

        Ok(SmtpClient::new(&host, port, &user, &password, tls_mode))
    }

    // --- Tool descriptors ---

    fn tool_discover_email_service() -> ToolDescriptor {
        ToolDescriptor {
            name: "discover_email_service".to_string(),
            description: "Auto-detect IMAP/SMTP server settings for an email domain using \
                          provider presets, Mozilla autoconfig, and port probing."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "domain": {
                        "type": "string",
                        "description": "Email domain to discover (e.g. gmail.com, example.org)"
                    }
                },
                "required": ["domain"]
            }),
            returns: serde_json::json!({
                "type": "object",
                "description": "Discovered IMAP/SMTP server configuration"
            }),
            cost_estimate: CostEstimate {
                latency_ms: 5000,
                network_bound: true,
                token_cost: None,
            },
            required_permissions: vec![Permission::Network(NetworkPermission::Full)],
            autonomous: true,
            requires_confirmation: false,
        }
    }

    fn tool_fetch_emails() -> ToolDescriptor {
        ToolDescriptor {
            name: "fetch_emails".to_string(),
            description: "Fetch emails from a configured account's IMAP folder.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "account_id": {
                        "type": "string",
                        "description": "Account ID (optional; uses first active if omitted)"
                    },
                    "folder": {
                        "type": "string",
                        "description": "IMAP folder name (default: INBOX)"
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Max emails to fetch (default: 50)"
                    },
                    "unread_only": {
                        "type": "boolean",
                        "description": "Only fetch unread emails (default: true)"
                    }
                }
            }),
            returns: serde_json::json!({ "type": "array", "items": { "type": "object" } }),
            cost_estimate: CostEstimate {
                latency_ms: 3000,
                network_bound: true,
                token_cost: None,
            },
            required_permissions: vec![Permission::Network(NetworkPermission::Full)],
            autonomous: true,
            requires_confirmation: false,
        }
    }

    fn tool_send_email() -> ToolDescriptor {
        ToolDescriptor {
            name: "send_email".to_string(),
            description: "Send an email via SMTP.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "account_id": {
                        "type": "string",
                        "description": "Account ID (optional; uses first active if omitted)"
                    },
                    "to": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Recipient email addresses"
                    },
                    "subject": {
                        "type": "string",
                        "description": "Email subject line"
                    },
                    "body": {
                        "type": "string",
                        "description": "Email body (plain text)"
                    }
                },
                "required": ["to", "subject", "body"]
            }),
            returns: serde_json::json!({ "type": "object" }),
            cost_estimate: CostEstimate {
                latency_ms: 3000,
                network_bound: true,
                token_cost: None,
            },
            required_permissions: vec![Permission::Network(NetworkPermission::Full)],
            autonomous: false,
            requires_confirmation: true,
        }
    }

    fn tool_search_emails() -> ToolDescriptor {
        ToolDescriptor {
            name: "search_emails".to_string(),
            description: "Search emails with criteria (from, subject, date range, keywords)."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "account_id": {
                        "type": "string",
                        "description": "Account ID (optional)"
                    },
                    "folder": {
                        "type": "string",
                        "description": "IMAP folder (default: INBOX)"
                    },
                    "from": {
                        "type": "string",
                        "description": "Filter by sender"
                    },
                    "subject": {
                        "type": "string",
                        "description": "Filter by subject"
                    },
                    "since": {
                        "type": "string",
                        "description": "Emails since date (dd-Mon-yyyy, e.g. 01-Jan-2025)"
                    },
                    "before": {
                        "type": "string",
                        "description": "Emails before date (dd-Mon-yyyy)"
                    },
                    "keyword": {
                        "type": "string",
                        "description": "Full-text search keyword"
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Max results (default: 50)"
                    }
                }
            }),
            returns: serde_json::json!({ "type": "array", "items": { "type": "object" } }),
            cost_estimate: CostEstimate {
                latency_ms: 5000,
                network_bound: true,
                token_cost: None,
            },
            required_permissions: vec![Permission::Network(NetworkPermission::Full)],
            autonomous: true,
            requires_confirmation: false,
        }
    }

    fn tool_list_folders() -> ToolDescriptor {
        ToolDescriptor {
            name: "list_folders".to_string(),
            description: "List available IMAP folders for an account.".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "account_id": {
                        "type": "string",
                        "description": "Account ID (optional)"
                    }
                }
            }),
            returns: serde_json::json!({ "type": "array", "items": { "type": "object" } }),
            cost_estimate: CostEstimate {
                latency_ms: 2000,
                network_bound: true,
                token_cost: None,
            },
            required_permissions: vec![Permission::Network(NetworkPermission::Full)],
            autonomous: true,
            requires_confirmation: false,
        }
    }
}

#[async_trait::async_trait]
impl Skill for EmailSkill {
    fn manifest(&self) -> &SkillManifest {
        &self.manifest
    }

    async fn initialize(&mut self, config: SkillConfig) -> SkillResult<()> {
        self.event_sender = config.event_sender.clone();
        Ok(())
    }

    async fn shutdown(&mut self) -> SkillResult<()> {
        Ok(())
    }

    fn health(&self) -> SkillHealth {
        SkillHealth {
            status: HealthStatus::Healthy,
            message: None,
            last_check: chrono::Utc::now(),
            metrics: HashMap::new(),
        }
    }

    fn tools(&self) -> Vec<ToolDescriptor> {
        vec![
            Self::tool_discover_email_service(),
            Self::tool_fetch_emails(),
            Self::tool_send_email(),
            Self::tool_search_emails(),
            Self::tool_list_folders(),
        ]
    }

    async fn execute_tool(
        &self,
        tool_name: &str,
        params: ToolParams,
        _context: &ExecutionContext,
    ) -> SkillResult<ToolOutput> {
        match tool_name {
            "discover_email_service" => {
                let raw = params
                    .get::<String>("domain")
                    .or_else(|| params.get::<String>("email"))
                    .or_else(|| params.get::<String>("address"))
                    .or_else(|| params.get::<String>("query"))
                    .ok_or_else(|| {
                        SkillError::ToolFailed(
                            "'domain' parameter required (e.g. \"gmail.com\" or \"user@gmail.com\")"
                                .to_string(),
                        )
                    })?;
                let domain = if let Some(at_pos) = raw.find('@') {
                    raw[at_pos + 1..].to_string()
                } else {
                    raw
                };
                let config = discovery::discover_email_config(&domain)
                    .await
                    .map_err(|e| SkillError::ToolFailed(e.to_string()))?;
                Ok(ToolOutput::success(config))
            }

            "fetch_emails" => {
                let account_id = params.get::<String>("account_id");
                let account = self.resolve_account(account_id.as_deref()).await?;
                let folder = params
                    .get::<String>("folder")
                    .unwrap_or_else(|| "INBOX".to_string());
                let limit = params.get::<u32>("limit").unwrap_or(50);
                let unread_only = params.get::<bool>("unread_only").unwrap_or(true);

                let imap = self.make_imap_client(&account)?;
                let criteria = SearchCriteria {
                    unseen_only: unread_only,
                    ..Default::default()
                };
                let emails = imap
                    .fetch_folder(&folder, &criteria, Some(limit))
                    .await
                    .map_err(|e| SkillError::ToolFailed(e.to_string()))?;

                let count = emails.len();
                let result = ToolOutput::success(&emails);

                if let Some(ref sender) = self.event_sender {
                    let _ = sender.send(SkillEvent {
                        skill_id: self.manifest.id.clone(),
                        trigger: "email_received".to_string(),
                        payload: serde_json::json!({
                            "count": count,
                            "account": account.id,
                            "folder": folder,
                        }),
                        timestamp: chrono::Utc::now(),
                        priority: TriggerPriority::Normal,
                    });
                }

                Ok(result)
            }

            "send_email" => {
                let account_id = params.get::<String>("account_id");
                let account = self.resolve_account(account_id.as_deref()).await?;
                let to: Vec<String> = params.get("to").ok_or_else(|| {
                    SkillError::ToolFailed("'to' parameter required".to_string())
                })?;
                let subject = params.get::<String>("subject").ok_or_else(|| {
                    SkillError::ToolFailed("'subject' parameter required".to_string())
                })?;
                let body = params.get::<String>("body").ok_or_else(|| {
                    SkillError::ToolFailed("'body' parameter required".to_string())
                })?;

                let smtp = self.make_smtp_client(&account)?;
                let email = orion_skills::transport::smtp::SmtpEmail {
                    from: account.address.clone(),
                    to,
                    subject,
                    body,
                };
                let result = smtp
                    .send(&email)
                    .await
                    .map_err(|e| SkillError::ToolFailed(e.to_string()))?;

                Ok(ToolOutput::success(result))
            }

            "search_emails" => {
                let account_id = params.get::<String>("account_id");
                let account = self.resolve_account(account_id.as_deref()).await?;
                let folder = params
                    .get::<String>("folder")
                    .unwrap_or_else(|| "INBOX".to_string());
                let limit = params.get::<u32>("limit").unwrap_or(50);

                let criteria = SearchCriteria {
                    from: params.get("from"),
                    subject: params.get("subject"),
                    since: params.get("since"),
                    before: params.get("before"),
                    keyword: params.get("keyword"),
                    unseen_only: false,
                };

                let imap = self.make_imap_client(&account)?;
                let emails = imap
                    .search(&folder, &criteria, Some(limit))
                    .await
                    .map_err(|e| SkillError::ToolFailed(e.to_string()))?;

                Ok(ToolOutput::success(&emails))
            }

            "list_folders" => {
                let account_id = params.get::<String>("account_id");
                let account = self.resolve_account(account_id.as_deref()).await?;

                let imap = self.make_imap_client(&account)?;
                let folders = imap
                    .list_folders()
                    .await
                    .map_err(|e| SkillError::ToolFailed(e.to_string()))?;

                Ok(ToolOutput::success(&folders))
            }

            _ => Err(SkillError::ToolFailed(format!(
                "Unknown tool: {}",
                tool_name
            ))),
        }
    }

    fn capabilities(&self) -> Vec<CapabilityDescriptor> {
        vec![
            CapabilityDescriptor {
                capability_type: "imap_transport".to_string(),
                version: "1.0".to_string(),
                category: "protocol".to_string(),
            },
            CapabilityDescriptor {
                capability_type: "smtp_transport".to_string(),
                version: "1.0".to_string(),
                category: "protocol".to_string(),
            },
            CapabilityDescriptor {
                capability_type: "service_discovery".to_string(),
                version: "1.0".to_string(),
                category: "protocol".to_string(),
            },
        ]
    }

    fn get_capability(&self, _cap_type: &str) -> Option<&dyn std::any::Any> {
        None
    }

    fn triggers(&self) -> Vec<TriggerDescriptor> {
        vec![
            TriggerDescriptor {
                name: "email_received".to_string(),
                description: "Fired when emails are fetched.".to_string(),
                payload_schema: serde_json::json!({
                    "count": "integer",
                    "account": "string",
                    "folder": "string"
                }),
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
