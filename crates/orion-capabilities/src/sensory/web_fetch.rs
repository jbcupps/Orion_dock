//! Web fetch capability: SSRF-safe URL validation, HTTP fetch, HTML parsing.
//!
//! Shared primitives for direct URL fetching and structured page extraction.
//! Used by the web-browse skill and any other consumer that needs curl-style fetch + parse.

use scraper::{Html, Selector};
use serde::Serialize;
use std::net::IpAddr;
use url::Url;

/// Default user-agent for fetch requests (browser-like).
pub const DEFAULT_USER_AGENT: &str =
    "Mozilla/5.0 (Windows NT 10.0; rv:109.0) Gecko/20100101 Firefox/115.0 Orion/1.0";

/// Blocked hostnames (cloud metadata, internal services).
const BLOCKED_HOSTS: &[&str] = &[
    "metadata.google.internal",
    "metadata.google.com",
    "169.254.169.254",
];

/// Result of a raw HTTP fetch (before parsing).
#[derive(Debug, Clone, Serialize)]
pub struct FetchResponse {
    pub status: u16,
    pub content_type: String,
    pub body: String,
    pub truncated: bool,
}

/// Parsed page content for structured output.
#[derive(Debug, Clone, Serialize)]
pub struct ParsedPage {
    pub title: Option<String>,
    pub meta_description: Option<String>,
    pub body_text: String,
    pub headings: Vec<String>,
    pub links: Vec<LinkInfo>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LinkInfo {
    pub href: String,
    pub text: String,
}

/// Validate URL for SSRF. Blocks private IPs, loopback, link-local, cloud metadata.
pub fn validate_url(url_str: &str) -> Result<Url, String> {
    let url = Url::parse(url_str).map_err(|e| format!("Invalid URL: {}", e))?;

    let scheme = url.scheme();
    if scheme != "http" && scheme != "https" {
        return Err(format!("Only http/https allowed, got: {}", scheme));
    }

    let host = url.host_str().ok_or("URL must have a host")?;
    let host_lower = host.to_lowercase();
    for blocked in BLOCKED_HOSTS {
        if host_lower == *blocked {
            return Err(format!("Host '{}' is blocked (SSRF protection)", host));
        }
    }

    match url.host() {
        Some(url::Host::Ipv4(ip)) => {
            let addr = IpAddr::V4(ip);
            if is_private_ip(&addr) {
                return Err(format!(
                    "Private/internal IP '{}' is blocked (SSRF protection)",
                    ip
                ));
            }
        }
        Some(url::Host::Ipv6(ip)) => {
            let addr = IpAddr::V6(ip);
            if is_private_ip(&addr) {
                return Err(format!(
                    "Private/internal IP '{}' is blocked (SSRF protection)",
                    ip
                ));
            }
        }
        Some(url::Host::Domain(d)) => {
            let d_lower = d.to_lowercase();
            if d_lower == "localhost"
                || d_lower == "0.0.0.0"
                || d_lower.ends_with(".local")
                || d_lower.ends_with(".internal")
            {
                return Err(format!(
                    "Local/internal domain '{}' is blocked (SSRF protection)",
                    d
                ));
            }
        }
        None => return Err("URL must have a host".to_string()),
    }

    Ok(url)
}

/// Fetch URL via HTTP GET. Caller must have built the client (e.g. with timeout, redirect policy).
pub async fn fetch_url(
    client: &reqwest::Client,
    url: &Url,
    user_agent: &str,
    max_body_bytes: usize,
) -> Result<FetchResponse, String> {
    let response = client
        .get(url.as_str())
        .header("User-Agent", user_agent)
        .send()
        .await
        .map_err(|e| e.to_string())?;

    let status = response.status().as_u16();
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();

    let bytes = response.bytes().await.map_err(|e| e.to_string())?;
    let truncated = bytes.len() > max_body_bytes;
    let body_bytes = if truncated {
        &bytes[..max_body_bytes]
    } else {
        &bytes[..]
    };
    let body = String::from_utf8_lossy(body_bytes).to_string();

    Ok(FetchResponse {
        status,
        content_type,
        body,
        truncated,
    })
}

/// Parse HTML into structured title, body text, headings, links.
pub fn parse_html(html: &str) -> Result<ParsedPage, String> {
    let document = Html::parse_document(html);

    let title = document
        .select(&Selector::parse("title").map_err(|e| e.to_string())?)
        .next()
        .map(|el| el.text().collect::<String>().trim().to_string());

    let meta_desc = document
        .select(&Selector::parse("meta[name=description]").map_err(|e| e.to_string())?)
        .next()
        .and_then(|el| el.value().attr("content").map(String::from));

    let mut body_text = String::new();
    let body_sel = Selector::parse("body").map_err(|e| e.to_string())?;
    if let Some(body) = document.select(&body_sel).next() {
        body_text = body.text().collect::<String>();
        body_text = body_text.split_whitespace().collect::<Vec<_>>().join(" ");
        if body_text.len() > 50_000 {
            body_text.truncate(50_000);
            body_text.push_str("… [truncated]");
        }
    }

    let mut headings = Vec::new();
    for tag in ["h1", "h2", "h3", "h4", "h5", "h6"] {
        if let Ok(sel) = Selector::parse(tag) {
            for el in document.select(&sel) {
                let t = el.text().collect::<String>().trim().to_string();
                if !t.is_empty() {
                    headings.push(t);
                }
            }
        }
    }

    let mut links = Vec::new();
    if let Ok(sel) = Selector::parse("a[href]") {
        for el in document.select(&sel) {
            if let Some(href) = el.value().attr("href") {
                let text = el.text().collect::<String>().trim().to_string();
                if href.starts_with("http://") || href.starts_with("https://") {
                    links.push(LinkInfo {
                        href: href.to_string(),
                        text: if text.is_empty() {
                            href.to_string()
                        } else {
                            text
                        },
                    });
                }
            }
        }
    }
    if links.len() > 500 {
        links.truncate(500);
    }

    Ok(ParsedPage {
        title,
        meta_description: meta_desc,
        body_text: body_text.trim().to_string(),
        headings,
        links,
    })
}

fn is_private_ip(addr: &IpAddr) -> bool {
    match addr {
        IpAddr::V4(ip) => {
            let o = ip.octets();
            o[0] == 127
                || o[0] == 10
                || (o[0] == 172 && (16..=31).contains(&o[1]))
                || (o[0] == 192 && o[1] == 168)
                || (o[0] == 169 && o[1] == 254)
                || o[0] == 0
        }
        IpAddr::V6(ip) => {
            ip.is_loopback()
                || ip.is_unspecified()
                || (ip.segments()[0] & 0xfe00) == 0xfc00
                || (ip.segments()[0] & 0xffc0) == 0xfe80
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_url_blocks_localhost() {
        assert!(validate_url("http://localhost:8080/").is_err());
        assert!(validate_url("http://127.0.0.1/").is_err());
    }

    #[test]
    fn test_validate_url_allows_public() {
        assert!(validate_url("https://example.com/").is_ok());
    }

    #[test]
    fn test_parse_html() {
        let html = r#"<!DOCTYPE html><html><head><title>Test</title></head><body><h1>Hi</h1><a href="https://a.com">A</a></body></html>"#;
        let parsed = parse_html(html).unwrap();
        assert_eq!(parsed.title.as_deref(), Some("Test"));
        assert!(parsed.body_text.contains("Hi"));
        assert_eq!(parsed.links.len(), 1);
        assert_eq!(parsed.links[0].href, "https://a.com");
    }
}
