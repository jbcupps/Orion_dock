//! Gmail API adapter (OAuth2 + Gmail REST).

use crate::pkce::PkcePair;
use anyhow::Context;
use chrono::{DateTime, Duration, Utc};
use reqwest::Client;
use serde::Deserialize;
use std::collections::HashMap;
const AUTH_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const GMAIL_BASE: &str = "https://gmail.googleapis.com/gmail/v1/users/me";

/// Recommended scopes for read + send (least privilege).
pub const SCOPES: &[&str] = &[
    "https://www.googleapis.com/auth/gmail.readonly",
    "https://www.googleapis.com/auth/gmail.send",
];

#[derive(Debug, Clone)]
pub struct GmailAdapter {
    client_id: String,
    client: Client,
}

impl GmailAdapter {
    pub fn new(client_id: String) -> Self {
        Self {
            client_id,
            client: Client::new(),
        }
    }

    /// Build authorize URL (user opens in browser). redirect_uri must match console (e.g. http://localhost:PORT/).
    pub fn authorize_url(
        &self,
        redirect_uri: &str,
        state: &str,
        pkce: &PkcePair,
        access_type: &str,
    ) -> String {
        let scope = SCOPES.join(" ");
        let u = url::Url::parse_with_params(
            AUTH_URL,
            &[
                ("client_id", self.client_id.as_str()),
                ("redirect_uri", redirect_uri),
                ("response_type", "code"),
                ("scope", &scope),
                ("state", state),
                ("code_challenge", &pkce.code_challenge),
                ("code_challenge_method", "S256"),
                ("access_type", access_type),
                ("prompt", "consent"),
            ],
        )
        .unwrap();
        u.to_string()
    }

    /// Exchange authorization code for tokens. client_secret optional for public/installed apps.
    pub async fn exchange_code(
        &self,
        code: &str,
        redirect_uri: &str,
        code_verifier: &str,
        client_secret: Option<&str>,
    ) -> anyhow::Result<OAuth2TokenResponse> {
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
            .context("Gmail token exchange request")?;
        let status = res.status();
        let body = res.text().await?;
        if !status.is_success() {
            anyhow::bail!("Gmail token exchange failed {}: {}", status, body);
        }
        let token: OAuth2TokenResponse =
            serde_json::from_str(&body).context("parse Gmail token response")?;
        Ok(token)
    }

    /// Refresh access token.
    pub async fn refresh_token(
        &self,
        refresh_token: &str,
        client_secret: Option<&str>,
    ) -> anyhow::Result<OAuth2TokenResponse> {
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
            .context("Gmail refresh request")?;
        let status = res.status();
        let body = res.text().await?;
        if !status.is_success() {
            anyhow::bail!("Gmail refresh failed {}: {}", status, body);
        }
        let token: OAuth2TokenResponse =
            serde_json::from_str(&body).context("parse Gmail refresh response")?;
        Ok(token)
    }

    /// List message IDs (INBOX, optional max).
    pub async fn list_messages(
        &self,
        access_token: &str,
        max_results: Option<u32>,
        q: Option<&str>,
    ) -> anyhow::Result<Vec<GmailMessageId>> {
        let url = format!("{}/messages", GMAIL_BASE);
        let mut req = self.client.get(&url).bearer_auth(access_token);
        if let Some(n) = max_results {
            req = req.query(&[("maxResults", n)]);
        }
        if let Some(q) = q {
            req = req.query(&[("q", q)]);
        }
        let res = req.send().await.context("Gmail list messages")?;
        let status = res.status();
        let body = res.text().await?;
        if !status.is_success() {
            anyhow::bail!("Gmail list messages failed {}: {}", status, body);
        }
        let list: GmailListResponse = serde_json::from_str(&body)?;
        Ok(list.messages.unwrap_or_default())
    }

    /// Get single message (full).
    pub async fn get_message(
        &self,
        access_token: &str,
        message_id: &str,
    ) -> anyhow::Result<GmailMessage> {
        let url = format!("{}/messages/{}?format=full", GMAIL_BASE, message_id);
        let res = self
            .client
            .get(&url)
            .bearer_auth(access_token)
            .send()
            .await
            .context("Gmail get message")?;
        let status = res.status();
        let body = res.text().await?;
        if !status.is_success() {
            anyhow::bail!("Gmail get message failed {}: {}", status, body);
        }
        let msg: GmailMessage = serde_json::from_str(&body)?;
        Ok(msg)
    }

    /// Send message (raw RFC 2822 base64url).
    pub async fn send_message(
        &self,
        access_token: &str,
        raw_base64url: &str,
    ) -> anyhow::Result<String> {
        let url = format!("{}/messages/send", GMAIL_BASE);
        let body = serde_json::json!({ "raw": raw_base64url });
        let res = self
            .client
            .post(&url)
            .bearer_auth(access_token)
            .json(&body)
            .send()
            .await
            .context("Gmail send message")?;
        let status = res.status();
        let body = res.text().await?;
        if !status.is_success() {
            anyhow::bail!("Gmail send failed {}: {}", status, body);
        }
        let sent: serde_json::Value = serde_json::from_str(&body)?;
        let id = sent
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        Ok(id)
    }
}

#[derive(Debug, Deserialize)]
pub struct OAuth2TokenResponse {
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: Option<String>,
    pub expires_in: Option<u64>,
    #[serde(default)]
    pub scope: Option<String>,
}

impl OAuth2TokenResponse {
    pub fn expires_at(&self) -> DateTime<Utc> {
        let secs = self.expires_in.unwrap_or(3600);
        Utc::now() + Duration::seconds(secs as i64)
    }
}

#[derive(Debug, Deserialize)]
struct GmailListResponse {
    messages: Option<Vec<GmailMessageId>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GmailMessageId {
    pub id: String,
    #[serde(rename = "threadId")]
    pub thread_id: String,
}

#[derive(Debug, Deserialize)]
pub struct GmailMessage {
    pub id: String,
    #[serde(rename = "threadId")]
    pub thread_id: Option<String>,
    pub label_ids: Option<Vec<String>>,
    pub snippet: Option<String>,
    pub payload: Option<GmailPayload>,
    pub internal_date: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct GmailPayload {
    pub headers: Option<Vec<GmailHeader>>,
    pub body: Option<GmailBody>,
    pub mime_type: Option<String>,
    pub parts: Option<Vec<GmailPayload>>,
}

#[derive(Debug, Deserialize)]
pub struct GmailHeader {
    pub name: String,
    pub value: String,
}

#[derive(Debug, Deserialize)]
pub struct GmailBody {
    pub data: Option<String>,
    pub size: Option<u32>,
}
