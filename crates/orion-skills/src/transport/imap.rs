//! Generic IMAP client supporting implicit TLS, STARTTLS, and plaintext connections.

use super::TlsMode;
use async_imap::Session;
use async_native_tls::TlsConnector;
use futures_util::StreamExt;
use serde::Serialize;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_util::compat::TokioAsyncReadCompatExt;

/// Summary of an email fetched via IMAP headers.
#[derive(Debug, Clone, Serialize)]
pub struct EmailSummary {
    pub id: String,
    pub from: String,
    pub subject: String,
    pub date: Option<String>,
}

/// IMAP folder metadata.
#[derive(Debug, Clone, Serialize)]
pub struct FolderInfo {
    pub name: String,
    pub delimiter: Option<String>,
    pub flags: Vec<String>,
}

/// IMAP SEARCH criteria builder.
#[derive(Debug, Clone, Default)]
pub struct SearchCriteria {
    /// Match sender address or name.
    pub from: Option<String>,
    /// Match subject line.
    pub subject: Option<String>,
    /// Messages since date (IMAP date format: dd-Mon-yyyy).
    pub since: Option<String>,
    /// Messages before date (IMAP date format: dd-Mon-yyyy).
    pub before: Option<String>,
    /// Only unseen messages.
    pub unseen_only: bool,
    /// Full-text keyword search (IMAP TEXT criterion).
    pub keyword: Option<String>,
}

impl SearchCriteria {
    /// Convert to an IMAP SEARCH query string.
    pub fn to_imap_query(&self) -> String {
        let mut parts = Vec::new();
        if self.unseen_only {
            parts.push("UNSEEN".to_string());
        }
        if let Some(ref from) = self.from {
            parts.push(format!("FROM \"{}\"", from));
        }
        if let Some(ref subject) = self.subject {
            parts.push(format!("SUBJECT \"{}\"", subject));
        }
        if let Some(ref since) = self.since {
            parts.push(format!("SINCE {}", since));
        }
        if let Some(ref before) = self.before {
            parts.push(format!("BEFORE {}", before));
        }
        if let Some(ref keyword) = self.keyword {
            parts.push(format!("TEXT \"{}\"", keyword));
        }
        if parts.is_empty() {
            "ALL".to_string()
        } else {
            parts.join(" ")
        }
    }
}

type TlsStream = async_native_tls::TlsStream<tokio_util::compat::Compat<TcpStream>>;

/// Generic IMAP client with configurable TLS mode.
pub struct ImapClient {
    host: String,
    port: u16,
    user: String,
    password: String,
    tls_mode: TlsMode,
    /// Accept invalid/self-signed TLS certificates (e.g. local mail bridges).
    accept_invalid_certs: bool,
}

impl ImapClient {
    /// Create a new IMAP client with the specified TLS mode.
    pub fn new(host: &str, port: u16, user: &str, password: &str, tls_mode: TlsMode) -> Self {
        Self {
            host: host.to_string(),
            port,
            user: user.to_string(),
            password: password.to_string(),
            tls_mode,
            accept_invalid_certs: false,
        }
    }

    /// Builder: accept self-signed or invalid TLS certificates.
    pub fn with_insecure_tls(mut self) -> Self {
        self.accept_invalid_certs = true;
        self
    }

    fn make_tls_connector(&self) -> TlsConnector {
        if self.accept_invalid_certs {
            TlsConnector::new()
                .danger_accept_invalid_certs(true)
                .danger_accept_invalid_hostnames(true)
        } else {
            TlsConnector::new()
        }
    }

    /// Connect with implicit TLS (port 993 typical).
    async fn connect_implicit_tls(&self) -> anyhow::Result<Session<TlsStream>> {
        let addr = format!("{}:{}", self.host, self.port);
        let stream = TcpStream::connect(&addr).await?;
        let stream = stream.compat();
        let tls = self
            .make_tls_connector()
            .connect(&self.host, stream)
            .await?;
        let mut client = async_imap::Client::new(tls);
        let _ = client.read_response().await;
        let session = client
            .login(&self.user, &self.password)
            .await
            .map_err(|(e, _)| e)?;
        Ok(session)
    }

    /// Connect with STARTTLS upgrade (port 143 typical).
    async fn connect_starttls(&self) -> anyhow::Result<Session<TlsStream>> {
        let addr = format!("{}:{}", self.host, self.port);
        let mut tcp = TcpStream::connect(&addr).await?;

        let mut buf = vec![0u8; 4096];
        let n = tcp.read(&mut buf).await?;
        let greeting = String::from_utf8_lossy(&buf[..n]);
        if !greeting.contains("OK") {
            anyhow::bail!("IMAP server greeting not OK: {}", greeting.trim());
        }

        tcp.write_all(b"a001 STARTTLS\r\n").await?;
        tcp.flush().await?;

        let n = tcp.read(&mut buf).await?;
        let response = String::from_utf8_lossy(&buf[..n]);
        if !response.contains("OK") {
            anyhow::bail!("STARTTLS rejected by server: {}", response.trim());
        }

        let compat = tcp.compat();
        let tls = self
            .make_tls_connector()
            .connect(&self.host, compat)
            .await?;

        let client = async_imap::Client::new(tls);
        let session = client
            .login(&self.user, &self.password)
            .await
            .map_err(|(e, _)| e)?;
        Ok(session)
    }

    async fn connect(&self) -> anyhow::Result<Session<TlsStream>> {
        match self.tls_mode {
            TlsMode::Implicit => self.connect_implicit_tls().await,
            TlsMode::Starttls => self.connect_starttls().await,
            TlsMode::None => {
                anyhow::bail!(
                    "Plaintext IMAP is not supported for security. Use Implicit or Starttls."
                );
            }
        }
    }

    /// Test connection (validates credentials and server reachability).
    pub async fn test_connection(&self) -> anyhow::Result<()> {
        let mut session = self.connect().await?;
        session.logout().await?;
        Ok(())
    }

    /// Fetch email summaries from a folder with search criteria.
    pub async fn fetch_folder(
        &self,
        folder: &str,
        criteria: &SearchCriteria,
        limit: Option<u32>,
    ) -> anyhow::Result<Vec<EmailSummary>> {
        let mut session = self.connect().await?;
        session.select(folder).await?;
        let query = criteria.to_imap_query();
        let results = session.search(&query).await?;
        let mut summaries = Vec::new();
        let take = limit.unwrap_or(u32::MAX) as usize;
        for seq in results.iter().take(take) {
            let seq_str = format!("{}", seq);
            let mut stream = session.fetch(&seq_str, "RFC822.HEADER").await?;
            while let Some(msg) = stream.next().await {
                let msg = msg?;
                let header = msg.header().unwrap_or_default();
                let (from, subject, date) =
                    if let Some(parsed) = mail_parser::MessageParser::default().parse(header) {
                        let from = parsed
                            .return_address()
                            .map(|s| s.to_string())
                            .unwrap_or_default();
                        let subject = parsed.subject().unwrap_or("").to_string();
                        let date = parsed.date().map(|d| d.to_string());
                        (from, subject, date)
                    } else {
                        (String::new(), String::new(), None)
                    };
                summaries.push(EmailSummary {
                    id: seq_str.clone(),
                    from,
                    subject,
                    date,
                });
            }
        }
        session.logout().await?;
        Ok(summaries)
    }

    /// Fetch unread emails from INBOX (convenience wrapper).
    pub async fn fetch_unread(&self, limit: Option<u32>) -> anyhow::Result<Vec<EmailSummary>> {
        self.fetch_folder(
            "INBOX",
            &SearchCriteria {
                unseen_only: true,
                ..Default::default()
            },
            limit,
        )
        .await
    }

    /// Search emails in a folder with criteria.
    pub async fn search(
        &self,
        folder: &str,
        criteria: &SearchCriteria,
        limit: Option<u32>,
    ) -> anyhow::Result<Vec<EmailSummary>> {
        self.fetch_folder(folder, criteria, limit).await
    }

    /// List available IMAP folders.
    pub async fn list_folders(&self) -> anyhow::Result<Vec<FolderInfo>> {
        let mut session = self.connect().await?;
        let mut list_stream = session.list(None, Some("*")).await?;
        let mut folders = Vec::new();
        while let Some(item) = list_stream.next().await {
            let item = item?;
            folders.push(FolderInfo {
                name: item.name().to_string(),
                delimiter: item.delimiter().map(|s| s.to_string()),
                flags: item
                    .attributes()
                    .iter()
                    .map(|a| format!("{:?}", a))
                    .collect(),
            });
        }
        drop(list_stream);
        session.logout().await?;
        Ok(folders)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_imap_connection() {
        if std::env::var("ORION_IMAP_TEST").is_err() {
            return;
        }
        let host = std::env::var("ORION_IMAP_HOST").unwrap_or_else(|_| "localhost".into());
        let port: u16 = std::env::var("ORION_IMAP_PORT")
            .ok()
            .and_then(|p| p.parse().ok())
            .unwrap_or(993);
        let user = std::env::var("ORION_IMAP_USER").unwrap_or_default();
        let pass = std::env::var("ORION_IMAP_PASS").unwrap_or_default();
        let tls_mode = match std::env::var("ORION_IMAP_TLS").unwrap_or_default().as_str() {
            "starttls" => TlsMode::Starttls,
            "none" => TlsMode::None,
            _ => TlsMode::Implicit,
        };
        if user.is_empty() || pass.is_empty() {
            return;
        }
        let client = ImapClient::new(&host, port, &user, &pass, tls_mode);
        assert!(client.test_connection().await.is_ok());
    }

    #[test]
    fn test_search_criteria_query() {
        let c = SearchCriteria {
            unseen_only: true,
            from: Some("alice@example.com".to_string()),
            subject: Some("hello".to_string()),
            ..Default::default()
        };
        let q = c.to_imap_query();
        assert!(q.contains("UNSEEN"));
        assert!(q.contains("FROM"));
        assert!(q.contains("SUBJECT"));
    }

    #[test]
    fn test_search_criteria_all() {
        let c = SearchCriteria::default();
        assert_eq!(c.to_imap_query(), "ALL");
    }
}
