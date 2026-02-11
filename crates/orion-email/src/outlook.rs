//! Microsoft Graph mail adapter (OAuth2 + Graph API).

use crate::pkce::PkcePair;
use anyhow::Context;
use chrono::{DateTime, Duration, Utc};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

const AUTH_URL: &str = "https://login.microsoftonline.com/common/oauth2/v2.0/authorize";
const TOKEN_URL: &str = "https://login.microsoftonline.com/common/oauth2/v2.0/token";
const GRAPH_BASE: &str = "https://graph.microsoft.com/v1.0";

/// Delegated scopes for read + send. offline_access for refresh token.
pub const SCOPES: &[&str] = &["offline_access", "User.Read", "Mail.Read", "Mail.Send"];

#[derive(Debug, Clone)]
pub struct OutlookAdapter {
    client_id: String,
    client: Client,
}

impl OutlookAdapter {
    pub fn new(client_id: String) -> Self {
        Self {
            client_id,
            client: Client::new(),
        }
    }

    /// Build authorize URL. redirect_uri must match app registration (e.g. http://localhost:PORT/).
    pub fn authorize_url(&self, redirect_uri: &str, state: &str, pkce: &PkcePair) -> String {
        let scope = SCOPES.join(" ");
        let params = [
            ("client_id", self.client_id.as_str()),
            ("redirect_uri", redirect_uri),
            ("response_type", "code"),
            ("scope", scope.as_str()),
            ("state", state),
            ("code_challenge", pkce.code_challenge.as_str()),
            ("code_challenge_method", "S256"),
            ("response_mode", "query"),
        ];
        let query = params
            .iter()
            .map(|(k, v)| format!("{}={}", k, urlencoding::encode(v)))
            .collect::<Vec<_>>()
            .join("&");
        format!("{}?{}", AUTH_URL, query)
    }

    /// Exchange code for tokens. client_secret optional for public clients.
    pub async fn exchange_code(
        &self,
        code: &str,
        redirect_uri: &str,
        code_verifier: &str,
        client_secret: Option<&str>,
    ) -> anyhow::Result<GraphTokenResponse> {
        let mut form = HashMap::new();
        form.insert("client_id", self.client_id.as_str());
        form.insert("code", code);
        form.insert("redirect_uri", redirect_uri);
        form.insert("grant_type", "authorization_code");
        form.insert("code_verifier", code_verifier);
        if let Some(secret) = client_secret {
            form.insert("client_secret", secret);
        }

        let res = self
            .client
            .post(TOKEN_URL)
            .form(&form)
            .send()
            .await
            .context("Graph token exchange")?;
        let status = res.status();
        let body = res.text().await?;
        if !status.is_success() {
            anyhow::bail!("Graph token exchange failed {}: {}", status, body);
        }
        let token: GraphTokenResponse =
            serde_json::from_str(&body).context("parse Graph token response")?;
        Ok(token)
    }

    /// Refresh access token.
    pub async fn refresh_token(
        &self,
        refresh_token: &str,
        client_secret: Option<&str>,
    ) -> anyhow::Result<GraphTokenResponse> {
        let mut form = HashMap::new();
        form.insert("client_id", self.client_id.as_str());
        form.insert("refresh_token", refresh_token);
        form.insert("grant_type", "refresh_token");
        if let Some(secret) = client_secret {
            form.insert("client_secret", secret);
        }

        let res = self
            .client
            .post(TOKEN_URL)
            .form(&form)
            .send()
            .await
            .context("Graph refresh")?;
        let status = res.status();
        let body = res.text().await?;
        if !status.is_success() {
            anyhow::bail!("Graph refresh failed {}: {}", status, body);
        }
        let token: GraphTokenResponse =
            serde_json::from_str(&body).context("parse Graph refresh response")?;
        Ok(token)
    }

    /// List messages (me/messages). top = page size.
    pub async fn list_messages(
        &self,
        access_token: &str,
        top: Option<u32>,
        filter: Option<&str>,
    ) -> anyhow::Result<Vec<GraphMessage>> {
        let url = format!("{}/me/messages", GRAPH_BASE);
        let mut req = self.client.get(&url).bearer_auth(access_token);
        if let Some(n) = top {
            req = req.query(&[("$top", n)]);
        }
        if let Some(f) = filter {
            req = req.query(&[("$filter", f)]);
        }
        let res = req.send().await.context("Graph list messages")?;
        let status = res.status();
        let body = res.text().await?;
        if !status.is_success() {
            anyhow::bail!("Graph list messages failed {}: {}", status, body);
        }
        let list: GraphMessageList = serde_json::from_str(&body)?;
        Ok(list.value.unwrap_or_default())
    }

    /// Get single message.
    pub async fn get_message(
        &self,
        access_token: &str,
        message_id: &str,
    ) -> anyhow::Result<GraphMessage> {
        let url = format!("{}/me/messages/{}", GRAPH_BASE, message_id);
        let res = self
            .client
            .get(&url)
            .bearer_auth(access_token)
            .send()
            .await
            .context("Graph get message")?;
        let status = res.status();
        let body = res.text().await?;
        if !status.is_success() {
            anyhow::bail!("Graph get message failed {}: {}", status, body);
        }
        let msg: GraphMessage = serde_json::from_str(&body)?;
        Ok(msg)
    }

    /// Send mail (JSON body). See Graph API user: sendMail.
    pub async fn send_mail(
        &self,
        access_token: &str,
        message: &GraphSendMailRequest,
    ) -> anyhow::Result<()> {
        let url = format!("{}/me/sendMail", GRAPH_BASE);
        let res = self
            .client
            .post(&url)
            .bearer_auth(access_token)
            .json(message)
            .send()
            .await
            .context("Graph sendMail")?;
        let status = res.status();
        if !status.is_success() {
            let body = res.text().await.unwrap_or_default();
            anyhow::bail!("Graph sendMail failed {}: {}", status, body);
        }
        Ok(())
    }
}

#[derive(Debug, Deserialize)]
pub struct GraphTokenResponse {
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: Option<String>,
    pub expires_in: Option<u64>,
    #[serde(default)]
    pub scope: Option<String>,
}

impl GraphTokenResponse {
    pub fn expires_at(&self) -> DateTime<Utc> {
        let secs = self.expires_in.unwrap_or(3600);
        Utc::now() + Duration::seconds(secs as i64)
    }
}

#[derive(Debug, Deserialize)]
struct GraphMessageList {
    value: Option<Vec<GraphMessage>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GraphMessage {
    pub id: String,
    pub subject: Option<String>,
    pub body: Option<GraphBody>,
    pub from: Option<GraphRecipient>,
    #[serde(rename = "toRecipients")]
    pub to_recipients: Option<Vec<GraphRecipient>>,
    #[serde(rename = "receivedDateTime")]
    pub received_date_time: Option<String>,
    #[serde(rename = "isRead")]
    pub is_read: Option<bool>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GraphBody {
    #[serde(rename = "contentType")]
    pub content_type: Option<String>,
    pub content: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GraphRecipient {
    #[serde(rename = "emailAddress")]
    pub email_address: Option<GraphEmailAddress>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphEmailAddress {
    pub address: Option<String>,
    pub name: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct GraphSendMailRequest {
    pub message: GraphSendMessage,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub save_to_sent_items: Option<bool>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct GraphSendMessage {
    pub subject: String,
    pub body: GraphSendBody,
    #[serde(rename = "toRecipients")]
    pub to_recipients: Vec<GraphSendRecipient>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct GraphSendBody {
    #[serde(rename = "contentType")]
    pub content_type: String,
    pub content: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct GraphSendRecipient {
    #[serde(rename = "emailAddress")]
    pub email_address: GraphEmailAddress,
}
