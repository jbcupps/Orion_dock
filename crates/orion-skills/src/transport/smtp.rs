//! Generic SMTP client supporting implicit TLS, STARTTLS, and plaintext connections.
//! Built on the `lettre` crate for robust SMTP protocol handling.

use super::TlsMode;
use lettre::message::header::ContentType;
use lettre::message::Mailbox;
use lettre::transport::smtp::authentication::Credentials;
use lettre::transport::smtp::client::Tls;
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};
use serde::Serialize;

/// Outgoing email for SMTP send.
pub struct SmtpEmail {
    pub from: String,
    pub to: Vec<String>,
    pub subject: String,
    pub body: String,
}

/// Result of sending an email via SMTP.
#[derive(Debug, Clone, Serialize)]
pub struct SmtpSendResult {
    pub server_response: Option<String>,
    pub accepted: bool,
}

/// Generic SMTP client with configurable TLS mode.
pub struct SmtpClient {
    host: String,
    port: u16,
    user: String,
    password: String,
    tls_mode: TlsMode,
}

impl SmtpClient {
    /// Create a new SMTP client with the specified TLS mode.
    pub fn new(host: &str, port: u16, user: &str, password: &str, tls_mode: TlsMode) -> Self {
        Self {
            host: host.to_string(),
            port,
            user: user.to_string(),
            password: password.to_string(),
            tls_mode,
        }
    }

    fn build_transport(&self) -> anyhow::Result<AsyncSmtpTransport<Tokio1Executor>> {
        let creds = Credentials::new(self.user.clone(), self.password.clone());

        let transport = match self.tls_mode {
            TlsMode::Implicit => {
                let tls_params =
                    lettre::transport::smtp::client::TlsParameters::new(self.host.clone())
                        .map_err(|e| anyhow::anyhow!("TLS parameters error: {}", e))?;
                AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(&self.host)
                    .port(self.port)
                    .tls(Tls::Wrapper(tls_params))
                    .credentials(creds)
                    .build()
            }
            TlsMode::Starttls => AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&self.host)
                .map_err(|e| anyhow::anyhow!("STARTTLS relay error: {}", e))?
                .port(self.port)
                .credentials(creds)
                .build(),
            TlsMode::None => AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(&self.host)
                .port(self.port)
                .credentials(creds)
                .build(),
        };

        Ok(transport)
    }

    /// Test SMTP connectivity and authentication.
    pub async fn test_connection(&self) -> anyhow::Result<()> {
        let transport = self.build_transport()?;
        transport
            .test_connection()
            .await
            .map_err(|e| anyhow::anyhow!("SMTP connection test failed: {}", e))?;
        Ok(())
    }

    /// Send an email.
    pub async fn send(&self, email: &SmtpEmail) -> anyhow::Result<SmtpSendResult> {
        let from: Mailbox = email
            .from
            .parse()
            .map_err(|e| anyhow::anyhow!("Invalid from address '{}': {}", email.from, e))?;

        let mut builder = Message::builder()
            .from(from)
            .subject(&email.subject);

        for to_addr in &email.to {
            let to_mailbox: Mailbox = to_addr
                .parse()
                .map_err(|e| anyhow::anyhow!("Invalid to address '{}': {}", to_addr, e))?;
            builder = builder.to(to_mailbox);
        }

        let message = builder
            .header(ContentType::TEXT_PLAIN)
            .body(email.body.clone())
            .map_err(|e| anyhow::anyhow!("Failed to build message: {}", e))?;

        let transport = self.build_transport()?;
        let response = transport.send(message).await?;

        Ok(SmtpSendResult {
            server_response: response.first_line().map(|s| s.to_string()),
            accepted: response.is_positive(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_smtp_connection() {
        if std::env::var("ORION_SMTP_TEST").is_err() {
            return;
        }
        let host = std::env::var("ORION_SMTP_HOST").unwrap_or_else(|_| "localhost".into());
        let port: u16 = std::env::var("ORION_SMTP_PORT")
            .ok()
            .and_then(|p| p.parse().ok())
            .unwrap_or(587);
        let user = std::env::var("ORION_SMTP_USER").unwrap_or_default();
        let pass = std::env::var("ORION_SMTP_PASS").unwrap_or_default();
        if user.is_empty() || pass.is_empty() {
            return;
        }
        let client = SmtpClient::new(&host, port, &user, &pass, TlsMode::Starttls);
        assert!(client.test_connection().await.is_ok());
    }
}
