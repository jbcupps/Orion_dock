//! Email transport capability trait and provider capability flags.
//!
//! Defines the **capability** interface that email skills implement.
//! Protocol-level transports (IMAP, SMTP) live in `transport/`.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::transport::{FolderInfo, SearchCriteria};
use crate::SkillResult;

/// Per-provider capability flags (e.g. OAuth API vs Bridge/IMAP limitations).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EmailProviderCapabilities {
    /// Can send mail (API or SMTP).
    #[serde(default = "bool_true")]
    pub supports_send: bool,
    /// Can search/filter (e.g. Gmail/Graph API; IMAP has limited search).
    #[serde(default)]
    pub supports_search: bool,
    /// Can manage labels/folders (Gmail labels; IMAP folders).
    #[serde(default)]
    pub supports_labels: bool,
    /// Requires local Bridge (e.g. Proton); not suitable for headless agents without Bridge.
    #[serde(default)]
    pub requires_bridge: bool,
    /// Send is token-based only (e.g. Proton SMTP token for custom domain).
    #[serde(default)]
    pub send_via_smtp_token_only: bool,
}

fn bool_true() -> bool {
    true
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EmailTransportInfo {
    pub id: String,
    pub name: String,
    /// Capability flags for this transport.
    #[serde(default)]
    pub capabilities: EmailProviderCapabilities,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmailAddress {
    pub email: String,
    pub name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Email {
    pub id: String,
    pub from: EmailAddress,
    pub to: Vec<EmailAddress>,
    pub subject: String,
    pub body_text: Option<String>,
    pub date: chrono::DateTime<chrono::Utc>,
    pub is_read: bool,
}

#[derive(Debug, Clone)]
pub struct FetchOptions {
    pub folder: Option<String>,
    pub limit: Option<u32>,
    pub unread_only: bool,
}

#[derive(Debug, Clone)]
pub struct OutgoingEmail {
    pub to: Vec<EmailAddress>,
    pub subject: String,
    pub body: String,
}

#[derive(Debug, Clone)]
pub struct SendResult {
    pub message_id: Option<String>,
}

#[async_trait]
pub trait EmailTransportCapability: Send + Sync {
    fn info(&self) -> EmailTransportInfo;
    /// Optional: return capability flags (defaults used if not overridden).
    fn provider_capabilities(&self) -> EmailProviderCapabilities {
        EmailProviderCapabilities::default()
    }
    async fn connect(&mut self) -> SkillResult<()>;
    async fn disconnect(&mut self) -> SkillResult<()>;
    async fn fetch_emails(&self, options: FetchOptions) -> SkillResult<Vec<Email>>;
    async fn send_email(&self, email: OutgoingEmail) -> SkillResult<SendResult>;
    async fn move_email(&self, email_id: &str, folder: &str) -> SkillResult<()>;
    async fn delete_email(&self, email_id: &str) -> SkillResult<()>;

    /// Search emails with structured criteria.
    async fn search_emails(
        &self,
        folder: &str,
        criteria: &SearchCriteria,
        limit: Option<u32>,
    ) -> SkillResult<Vec<Email>> {
        // Default: delegate to fetch_emails with the folder set.
        let _ = (folder, criteria, limit);
        Err(crate::SkillError::ToolFailed(
            "search_emails not implemented for this provider".to_string(),
        ))
    }

    /// List available mail folders.
    async fn list_folders(&self) -> SkillResult<Vec<FolderInfo>> {
        Err(crate::SkillError::ToolFailed(
            "list_folders not implemented for this provider".to_string(),
        ))
    }
}
