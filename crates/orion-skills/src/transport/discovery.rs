//! Email service discovery: autoconfig, provider presets, and port probing.
//!
//! Discovers IMAP/SMTP server settings for a given email domain using:
//! 1. Provider presets (well-known configs for major providers)
//! 2. Mozilla autoconfig / Thunderbird ISPDB
//! 3. TCP port probing with TLS detection

use orion_core::config::{
    provider_preset, EmailAuthType, EmailProvider, EmailServerPreset, TlsMode,
};
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Discovered email server configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveredConfig {
    pub provider: Option<String>,
    pub imap_host: Option<String>,
    pub imap_port: Option<u16>,
    pub imap_tls: TlsMode,
    pub smtp_host: Option<String>,
    pub smtp_port: Option<u16>,
    pub smtp_tls: TlsMode,
    pub auth_type: Option<String>,
    pub source: DiscoverySource,
}

/// How the configuration was discovered.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiscoverySource {
    ProviderPreset,
    Autoconfig,
    PortProbe,
    NotFound,
}

/// Well-known domain → provider mappings.
fn domain_to_provider(domain: &str) -> Option<EmailProvider> {
    let d = domain.to_lowercase();
    if d == "gmail.com" || d == "googlemail.com" || d.ends_with(".google.com") {
        Some(EmailProvider::Gmail)
    } else if d == "outlook.com"
        || d == "hotmail.com"
        || d == "live.com"
        || d.ends_with(".outlook.com")
    {
        Some(EmailProvider::Outlook)
    } else if d == "proton.me" || d == "protonmail.com" || d == "pm.me" {
        Some(EmailProvider::Proton)
    } else if d == "fastmail.com" || d == "fastmail.fm" || d.ends_with(".fastmail.com") {
        Some(EmailProvider::Fastmail)
    } else {
        Option::None
    }
}

fn preset_to_discovered(preset: &EmailServerPreset, provider_name: &str) -> DiscoveredConfig {
    let auth_str = match preset.auth {
        EmailAuthType::OAuth2 => "oauth2",
        EmailAuthType::SmtpToken => "smtp_token",
        EmailAuthType::AppPassword => "app_password",
    };
    DiscoveredConfig {
        provider: Some(provider_name.to_string()),
        imap_host: Some(preset.imap_host.to_string()),
        imap_port: Some(preset.imap_port),
        imap_tls: preset.imap_tls,
        smtp_host: Some(preset.smtp_host.to_string()),
        smtp_port: Some(preset.smtp_port),
        smtp_tls: preset.smtp_tls,
        auth_type: Some(auth_str.to_string()),
        source: DiscoverySource::ProviderPreset,
    }
}

fn running_in_container() -> bool {
    std::env::var("ORION_CONTAINER")
        .ok()
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

fn proton_bridge_host_for_runtime() -> String {
    std::env::var("ORION_EMAIL_BRIDGE_HOST")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "protonbridge_ingress".to_string())
}

/// Attempt Mozilla autoconfig for a domain.
///
/// Tries the domain-hosted autoconfig URL, then the Thunderbird ISPDB.
async fn try_autoconfig(domain: &str) -> Option<DiscoveredConfig> {
    let urls = [
        format!("https://autoconfig.{}/mail/config-v1.1.xml", domain),
        format!("https://autoconfig.thunderbird.net/v1.1/{}", domain),
    ];

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(8))
        .build()
        .ok()?;

    for url in &urls {
        if let Ok(resp) = client.get(url).send().await {
            if resp.status().is_success() {
                if let Ok(body) = resp.text().await {
                    if let Some(config) = parse_autoconfig_xml(&body) {
                        return Some(config);
                    }
                }
            }
        }
    }
    Option::None
}

/// Parse Mozilla autoconfig XML (simple extraction, no full XML parser).
fn parse_autoconfig_xml(xml: &str) -> Option<DiscoveredConfig> {
    let imap_host = extract_server_field(xml, "incomingServer", "imap", "hostname");
    let imap_port = extract_server_field(xml, "incomingServer", "imap", "port")
        .and_then(|p| p.parse::<u16>().ok());
    let imap_socket = extract_server_field(xml, "incomingServer", "imap", "socketType");

    let smtp_host = extract_server_field(xml, "outgoingServer", "smtp", "hostname");
    let smtp_port = extract_server_field(xml, "outgoingServer", "smtp", "port")
        .and_then(|p| p.parse::<u16>().ok());
    let smtp_socket = extract_server_field(xml, "outgoingServer", "smtp", "socketType");

    if imap_host.is_none() && smtp_host.is_none() {
        return Option::None;
    }

    Some(DiscoveredConfig {
        provider: Option::None,
        imap_host,
        imap_port,
        imap_tls: socket_type_to_tls(&imap_socket.unwrap_or_default()),
        smtp_host,
        smtp_port,
        smtp_tls: socket_type_to_tls(&smtp_socket.unwrap_or_default()),
        auth_type: Option::None,
        source: DiscoverySource::Autoconfig,
    })
}

/// Extract a field from an autoconfig server block.
fn extract_server_field(
    xml: &str,
    server_tag: &str,
    server_type: &str,
    field: &str,
) -> Option<String> {
    let type_attr = format!("type=\"{}\"", server_type);
    let open_tag = format!("<{}", server_tag);
    let close_tag = format!("</{}>", server_tag);

    let mut search_from = 0;
    while let Some(start) = xml[search_from..].find(&open_tag) {
        let abs_start = search_from + start;
        let tag_region = &xml[abs_start..];

        if let Some(end) = tag_region.find(&close_tag) {
            let block = &tag_region[..end];
            if block.contains(&type_attr) {
                let field_open = format!("<{}>", field);
                let field_close = format!("</{}>", field);
                if let Some(fs) = block.find(&field_open) {
                    let value_start = fs + field_open.len();
                    if let Some(fe) = block[value_start..].find(&field_close) {
                        return Some(block[value_start..value_start + fe].trim().to_string());
                    }
                }
            }
            search_from = abs_start + end + close_tag.len();
        } else {
            break;
        }
    }
    Option::None
}

fn socket_type_to_tls(socket_type: &str) -> TlsMode {
    match socket_type.to_uppercase().as_str() {
        "SSL" | "TLS" => TlsMode::Implicit,
        "STARTTLS" => TlsMode::Starttls,
        _ => TlsMode::Implicit,
    }
}

/// Probe common IMAP/SMTP ports on a host to detect available services.
async fn probe_ports(domain: &str) -> Option<DiscoveredConfig> {
    let imap_host = format!("imap.{}", domain);
    let smtp_host = format!("smtp.{}", domain);
    let mail_host = format!("mail.{}", domain);

    let mut imap_result: Option<(String, u16, TlsMode)> = Option::None;
    let mut smtp_result: Option<(String, u16, TlsMode)> = Option::None;

    let imap_targets = [
        (imap_host.as_str(), 993, TlsMode::Implicit),
        (imap_host.as_str(), 143, TlsMode::Starttls),
        (mail_host.as_str(), 993, TlsMode::Implicit),
        (mail_host.as_str(), 143, TlsMode::Starttls),
    ];

    for (host, port, tls) in &imap_targets {
        if tcp_probe(host, *port).await {
            imap_result = Some((host.to_string(), *port, *tls));
            break;
        }
    }

    let smtp_targets = [
        (smtp_host.as_str(), 465, TlsMode::Implicit),
        (smtp_host.as_str(), 587, TlsMode::Starttls),
        (mail_host.as_str(), 465, TlsMode::Implicit),
        (mail_host.as_str(), 587, TlsMode::Starttls),
    ];

    for (host, port, tls) in &smtp_targets {
        if tcp_probe(host, *port).await {
            smtp_result = Some((host.to_string(), *port, *tls));
            break;
        }
    }

    if imap_result.is_none() && smtp_result.is_none() {
        return Option::None;
    }

    Some(DiscoveredConfig {
        provider: Option::None,
        imap_host: imap_result.as_ref().map(|(h, _, _)| h.clone()),
        imap_port: imap_result.as_ref().map(|(_, p, _)| *p),
        imap_tls: imap_result
            .as_ref()
            .map(|(_, _, t)| *t)
            .unwrap_or(TlsMode::Implicit),
        smtp_host: smtp_result.as_ref().map(|(h, _, _)| h.clone()),
        smtp_port: smtp_result.as_ref().map(|(_, p, _)| *p),
        smtp_tls: smtp_result
            .as_ref()
            .map(|(_, _, t)| *t)
            .unwrap_or(TlsMode::Starttls),
        auth_type: Option::None,
        source: DiscoverySource::PortProbe,
    })
}

/// TCP connect probe with a short timeout.
async fn tcp_probe(host: &str, port: u16) -> bool {
    let addr = format!("{}:{}", host, port);
    tokio::time::timeout(
        Duration::from_secs(3),
        tokio::net::TcpStream::connect(&addr),
    )
    .await
    .map(|r| r.is_ok())
    .unwrap_or(false)
}

/// Discover email server configuration for a domain.
///
/// Tries strategies in order: provider presets, Mozilla autoconfig, port probing.
pub async fn discover_email_config(domain: &str) -> anyhow::Result<DiscoveredConfig> {
    // 1. Check provider presets
    if let Some(provider) = domain_to_provider(domain) {
        if let Some(preset) = provider_preset(provider) {
            let name = format!("{:?}", provider);
            let mut discovered = preset_to_discovered(&preset, &name);
            if provider == EmailProvider::Proton && running_in_container() {
                let ingress = proton_bridge_host_for_runtime();
                discovered.imap_host = Some(ingress.clone());
                discovered.smtp_host = Some(ingress);
            }
            return Ok(discovered);
        }
    }

    // 2. Try Mozilla autoconfig
    if let Some(config) = try_autoconfig(domain).await {
        return Ok(config);
    }

    // 3. Port probing
    if let Some(config) = probe_ports(domain).await {
        return Ok(config);
    }

    Ok(DiscoveredConfig {
        provider: Option::None,
        imap_host: Option::None,
        imap_port: Option::None,
        imap_tls: TlsMode::Implicit,
        smtp_host: Option::None,
        smtp_port: Option::None,
        smtp_tls: TlsMode::Starttls,
        auth_type: Option::None,
        source: DiscoverySource::NotFound,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_domain_to_provider() {
        assert_eq!(domain_to_provider("gmail.com"), Some(EmailProvider::Gmail));
        assert_eq!(
            domain_to_provider("outlook.com"),
            Some(EmailProvider::Outlook)
        );
        assert_eq!(domain_to_provider("proton.me"), Some(EmailProvider::Proton));
        assert_eq!(
            domain_to_provider("fastmail.com"),
            Some(EmailProvider::Fastmail)
        );
        assert_eq!(domain_to_provider("example.com"), Option::None);
    }

    #[test]
    fn test_parse_autoconfig_xml() {
        let xml = r#"
        <clientConfig version="1.1">
          <emailProvider id="example.com">
            <incomingServer type="imap">
              <hostname>imap.example.com</hostname>
              <port>993</port>
              <socketType>SSL</socketType>
            </incomingServer>
            <outgoingServer type="smtp">
              <hostname>smtp.example.com</hostname>
              <port>587</port>
              <socketType>STARTTLS</socketType>
            </outgoingServer>
          </emailProvider>
        </clientConfig>
        "#;
        let config = parse_autoconfig_xml(xml).unwrap();
        assert_eq!(config.imap_host.as_deref(), Some("imap.example.com"));
        assert_eq!(config.imap_port, Some(993));
        assert_eq!(config.imap_tls, TlsMode::Implicit);
        assert_eq!(config.smtp_host.as_deref(), Some("smtp.example.com"));
        assert_eq!(config.smtp_port, Some(587));
        assert_eq!(config.smtp_tls, TlsMode::Starttls);
    }

    #[test]
    fn test_socket_type_mapping() {
        assert_eq!(socket_type_to_tls("SSL"), TlsMode::Implicit);
        assert_eq!(socket_type_to_tls("TLS"), TlsMode::Implicit);
        assert_eq!(socket_type_to_tls("STARTTLS"), TlsMode::Starttls);
    }
}
