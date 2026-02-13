//! Web Browse skill: direct URL fetch, HTML parsing, optional Tavily/Perplexity fallback.
//!
//! Provides a unified `web_browse` tool with strategy control: HTTP-first (curl-style),
//! optional Chromium render, and optional search API fallback when direct fetch fails.

use async_trait::async_trait;
use orion_capabilities::sensory::web_fetch;
use orion_capabilities::sensory::web_search;
use orion_core::{check_search_query, SecretsVault};
use orion_skills::channel::TriggerDescriptor;
use orion_skills::manifest::{CapabilityDescriptor, NetworkPermission, Permission, SkillManifest};
use orion_skills::skill::{
    CostEstimate, ExecutionContext, HealthStatus, Skill, SkillConfig, SkillError, SkillHealth,
    SkillResult, ToolDescriptor, ToolOutput, ToolParams,
};
use serde::Serialize;
use std::any::Any;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[cfg(feature = "browser")]
mod browser;

/// Maximum response body size: 1MB.
const MAX_RESPONSE_BYTES: usize = 1_048_576;

/// Default request timeout in seconds.
const DEFAULT_TIMEOUT_SECS: u64 = 30;

/// Browsing strategy: how to obtain page content.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BrowseStrategy {
    /// Try HTTP fetch, then optional browser, then optional search API fallback.
    #[default]
    Auto,
    /// Only HTTP fetch (no browser, no search fallback).
    HttpOnly,
    /// Only use browser automation (Chromium). Not yet implemented.
    BrowserOnly,
    /// Only use search APIs (Tavily or Perplexity) with the given or derived query.
    SearchOnly,
}

impl std::str::FromStr for BrowseStrategy {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "auto" => Ok(BrowseStrategy::Auto),
            "http_only" | "http" => Ok(BrowseStrategy::HttpOnly),
            "browser_only" | "browser" => Ok(BrowseStrategy::BrowserOnly),
            "search_only" | "search" => Ok(BrowseStrategy::SearchOnly),
            _ => Err(()),
        }
    }
}

/// Error class for LLM retry guidance.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorClass {
    BlockedUrl,
    NeedsBrowserRender,
    BotProtection,
    Timeout,
    ParseFailed,
    SearchBlocked,
    MissingSearchKey,
    Other,
}

pub struct WebBrowseSkill {
    manifest: SkillManifest,
    vault: Arc<Mutex<SecretsVault>>,
    client: reqwest::Client,
}

impl WebBrowseSkill {
    pub fn default_manifest() -> SkillManifest {
        let toml_str = include_str!("../skill.toml");
        SkillManifest::parse(toml_str).expect("embedded skill.toml must be valid")
    }

    pub fn with_secrets(manifest: SkillManifest, vault: Arc<Mutex<SecretsVault>>) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(DEFAULT_TIMEOUT_SECS))
            .connect_timeout(Duration::from_secs(10))
            .redirect(reqwest::redirect::Policy::limited(5))
            .user_agent(web_fetch::DEFAULT_USER_AGENT)
            .build()
            .expect("Failed to create HTTP client");

        Self {
            manifest,
            vault,
            client,
        }
    }

    /// Fetch URL via HTTP and return raw body and content-type.
    async fn http_fetch(&self, url_str: &str) -> Result<(String, String), (String, ErrorClass)> {
        let url = web_fetch::validate_url(url_str).map_err(|e| (e, ErrorClass::BlockedUrl))?;

        let response = self
            .client
            .get(url.as_str())
            .header("User-Agent", web_fetch::DEFAULT_USER_AGENT)
            .send()
            .await
            .map_err(|e| {
                let msg = e.to_string();
                let class = if e.is_timeout() {
                    ErrorClass::Timeout
                } else {
                    ErrorClass::Other
                };
                (msg, class)
            })?;

        let status = response.status();
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();

        if status == 403 || status == 429 || status.as_u16() == 503 {
            return Err((
                format!(
                    "HTTP {} — may need browser render or hit rate limit",
                    status
                ),
                ErrorClass::BotProtection,
            ));
        }

        if !status.is_success() {
            return Err((
                format!(
                    "HTTP {} {}",
                    status,
                    status.canonical_reason().unwrap_or("")
                ),
                ErrorClass::Other,
            ));
        }

        let bytes = response
            .bytes()
            .await
            .map_err(|e| (e.to_string(), ErrorClass::Other))?;

        let truncated = bytes.len() > MAX_RESPONSE_BYTES;
        let body_bytes = if truncated {
            &bytes[..MAX_RESPONSE_BYTES]
        } else {
            &bytes[..]
        };

        let body = String::from_utf8_lossy(body_bytes).to_string();
        if truncated {
            tracing::warn!("Response truncated at {} bytes", MAX_RESPONSE_BYTES);
        }

        Ok((body, content_type))
    }

    /// Build formatted string for LLM from parsed page.
    fn format_parsed(parsed: &web_fetch::ParsedPage, url: &str) -> String {
        let mut out = String::new();
        if let Some(ref t) = parsed.title {
            out.push_str(&format!("Title: {}\n\n", t));
        }
        if let Some(ref d) = parsed.meta_description {
            out.push_str(&format!("Description: {}\n\n", d));
        }
        if !parsed.headings.is_empty() {
            out.push_str("Headings: ");
            out.push_str(&parsed.headings.join(" | "));
            out.push_str("\n\n");
        }
        out.push_str(&parsed.body_text);
        if !parsed.links.is_empty() {
            out.push_str("\n\nLinks (first 20): ");
            for (i, l) in parsed.links.iter().take(20).enumerate() {
                if i > 0 {
                    out.push_str("; ");
                }
                out.push_str(&format!("{} [{}]", l.text, l.href));
            }
        }
        out.push_str(&format!("\n\nSource: {}", url));
        out
    }

    /// Run search fallback (Tavily or Perplexity). Caller must pass a query (e.g. URL or user query).
    async fn search_fallback(&self, query: &str, max_results: u32) -> SkillResult<ToolOutput> {
        let verdict = check_search_query(query);
        if !verdict.allowed {
            return Ok(ToolOutput::success(serde_json::json!({
                "formatted": format!("Search blocked: {}", verdict.reason.unwrap_or_default()),
                "strategy_used": "search_fallback",
                "error_class": "search_blocked",
                "answer": null,
                "result_count": 0
            })));
        }

        let (tavily_key, perplexity_key) = {
            let vault = self
                .vault
                .lock()
                .map_err(|e| SkillError::ToolFailed(format!("Vault lock: {}", e)))?;
            (
                vault.get_secret("tavily").map(str::to_string),
                vault.get_secret("perplexity").map(str::to_string),
            )
        };

        if let Some(api_key) = tavily_key {
            let response = web_search::tavily_search(&api_key, query, max_results)
                .await
                .map_err(SkillError::ToolFailed)?;
            let formatted = web_search::format_search_results(&response);
            return Ok(ToolOutput::success(serde_json::json!({
                "formatted": formatted,
                "strategy_used": "tavily",
                "answer": response.answer,
                "result_count": response.results.len(),
                "title": null,
                "body_text": null,
                "links": null
            })));
        }

        if let Some(api_key) = perplexity_key {
            let client = reqwest::Client::new();
            #[derive(serde::Serialize)]
            struct PplxReq {
                model: String,
                messages: Vec<serde_json::Value>,
            }
            let req = PplxReq {
                model: "sonar".to_string(),
                messages: vec![serde_json::json!({
                    "role": "user",
                    "content": query
                })],
            };
            let resp = client
                .post("https://api.perplexity.ai/chat/completions")
                .header("Authorization", format!("Bearer {}", api_key))
                .json(&req)
                .send()
                .await
                .map_err(|e| SkillError::ToolFailed(e.to_string()))?;

            if !resp.status().is_success() {
                let status = resp.status();
                let body = resp.text().await.unwrap_or_default();
                return Ok(ToolOutput::success(serde_json::json!({
                    "formatted": format!("Perplexity API error ({}): {}", status, body),
                    "strategy_used": "perplexity",
                    "error_class": "other"
                })));
            }

            #[derive(serde::Deserialize)]
            struct PplxChoice {
                message: PplxMessage,
            }
            #[derive(serde::Deserialize)]
            struct PplxMessage {
                content: String,
            }
            #[derive(serde::Deserialize)]
            struct PplxResp {
                choices: Vec<PplxChoice>,
            }

            let pplx: PplxResp = resp
                .json()
                .await
                .map_err(|e| SkillError::ToolFailed(e.to_string()))?;
            let answer = pplx
                .choices
                .first()
                .map(|c| c.message.content.clone())
                .unwrap_or_default();

            return Ok(ToolOutput::success(serde_json::json!({
                "formatted": answer,
                "strategy_used": "perplexity",
                "answer": answer,
                "result_count": 0,
                "title": null,
                "body_text": null,
                "links": null
            })));
        }

        Ok(ToolOutput::success(serde_json::json!({
            "formatted": "No search API key configured (tavily or perplexity). Add a key in settings for search fallback.",
            "strategy_used": "search_fallback",
            "error_class": "missing_search_key"
        })))
    }

    /// Execute web_browse: try HTTP (or browser/search per strategy), parse, return structured output.
    async fn web_browse(
        &self,
        url: Option<String>,
        query: Option<String>,
        strategy: BrowseStrategy,
        max_results: u32,
    ) -> SkillResult<ToolOutput> {
        if strategy == BrowseStrategy::BrowserOnly {
            #[allow(unused_variables)]
            let url_str = url
                .as_deref()
                .ok_or_else(|| SkillError::ToolFailed("browser_only requires 'url'".to_string()))?;
            #[cfg(feature = "browser")]
            {
                if web_fetch::validate_url(url_str).is_err() {
                    return Ok(ToolOutput::success(serde_json::json!({
                        "formatted": "Invalid or blocked URL for browser.",
                        "strategy_used": "browser_only",
                        "error_class": "blocked_url"
                    })));
                }
                match browser::fetch_page_html(url_str).await {
                    Ok(html) => match web_fetch::parse_html(&html) {
                        Ok(parsed) => {
                            let formatted = Self::format_parsed(&parsed, url_str);
                            let links_json: Vec<serde_json::Value> = parsed
                                .links
                                .iter()
                                .map(|l| serde_json::json!({ "href": l.href, "text": l.text }))
                                .collect();
                            return Ok(ToolOutput::success(serde_json::json!({
                                "formatted": formatted,
                                "strategy_used": "browser",
                                "title": parsed.title,
                                "meta_description": parsed.meta_description,
                                "body_text": parsed.body_text,
                                "headings": parsed.headings,
                                "links": links_json,
                                "url": url_str
                            })));
                        }
                        Err(_) => {
                            return Ok(ToolOutput::success(serde_json::json!({
                                "formatted": html.get(..4000).unwrap_or(&html),
                                "strategy_used": "browser",
                                "body_text": html.get(..20000).unwrap_or(&html).to_string(),
                                "links": []
                            })));
                        }
                    },
                    Err(e) => {
                        return Ok(ToolOutput::success(serde_json::json!({
                            "formatted": e,
                            "strategy_used": "browser_only",
                            "error_class": "needs_browser_render"
                        })));
                    }
                }
            }
            #[cfg(not(feature = "browser"))]
            return Ok(ToolOutput::success(serde_json::json!({
                "formatted": "Browser backend not compiled. Build with default features or use strategy 'auto' / 'http_only' / 'search_only'.",
                "strategy_used": "browser_only",
                "error_class": "needs_browser_render"
            })));
        }

        if strategy == BrowseStrategy::SearchOnly {
            let q = query.or(url).ok_or_else(|| {
                SkillError::ToolFailed("search_only requires 'url' or 'query'".to_string())
            })?;
            return self.search_fallback(&q, max_results).await;
        }

        let url_str = url.ok_or_else(|| {
            SkillError::ToolFailed(
                "Missing required parameter: url (or use strategy search_only with query)"
                    .to_string(),
            )
        })?;

        // HTTP path (auto or http_only)
        match self.http_fetch(&url_str).await {
            Ok((body, content_type)) => {
                let is_html = content_type.to_lowercase().contains("text/html");
                if is_html {
                    match web_fetch::parse_html(&body) {
                        Ok(parsed) => {
                            let formatted = Self::format_parsed(&parsed, &url_str);
                            let links_json: Vec<serde_json::Value> = parsed
                                .links
                                .iter()
                                .map(|l| serde_json::json!({ "href": l.href, "text": l.text }))
                                .collect();
                            Ok(ToolOutput::success(serde_json::json!({
                                "formatted": formatted,
                                "strategy_used": "http",
                                "title": parsed.title,
                                "meta_description": parsed.meta_description,
                                "body_text": parsed.body_text,
                                "headings": parsed.headings,
                                "links": links_json,
                                "url": url_str
                            })))
                        }
                        Err(_) => Ok(ToolOutput::success(serde_json::json!({
                            "formatted": body.get(..4000).unwrap_or(&body),
                            "strategy_used": "http",
                            "error_class": "parse_failed",
                            "title": null,
                            "body_text": body.get(..20000).unwrap_or(&body).to_string(),
                            "links": []
                        }))),
                    }
                } else {
                    let preview = body.chars().take(4000).collect::<String>();
                    Ok(ToolOutput::success(serde_json::json!({
                        "formatted": format!("Content-Type: {}\n\n{}", content_type, preview),
                        "strategy_used": "http",
                        "title": null,
                        "body_text": body.get(..20000).unwrap_or(&body).to_string(),
                        "links": [],
                        "url": url_str
                    })))
                }
            }
            Err((msg, error_class)) => {
                if strategy == BrowseStrategy::HttpOnly {
                    return Ok(ToolOutput::success(serde_json::json!({
                        "formatted": msg,
                        "strategy_used": "http_only",
                        "error_class": format!("{:?}", error_class).to_lowercase()
                    })));
                }
                // Auto: try Chromium then search fallback
                #[cfg(feature = "browser")]
                match browser::fetch_page_html(&url_str).await {
                    Ok(html) => {
                        if let Ok(parsed) = web_fetch::parse_html(&html) {
                            let formatted = Self::format_parsed(&parsed, &url_str);
                            let links_json: Vec<serde_json::Value> = parsed
                                .links
                                .iter()
                                .map(|l| serde_json::json!({ "href": l.href, "text": l.text }))
                                .collect();
                            return Ok(ToolOutput::success(serde_json::json!({
                                "formatted": formatted,
                                "strategy_used": "browser",
                                "title": parsed.title,
                                "meta_description": parsed.meta_description,
                                "body_text": parsed.body_text,
                                "headings": parsed.headings,
                                "links": links_json,
                                "url": url_str
                            })));
                        }
                    }
                    Err(e) => {
                        tracing::warn!(url = %url_str, error = %e, "Chromium fallback failed, trying search");
                    }
                }
                let fallback_query = query.as_deref().unwrap_or(&url_str);
                return self.search_fallback(fallback_query, max_results).await;
            }
        }
    }
}

#[async_trait]
impl Skill for WebBrowseSkill {
    fn manifest(&self) -> &SkillManifest {
        &self.manifest
    }

    async fn initialize(&mut self, _config: SkillConfig) -> SkillResult<()> {
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
        vec![ToolDescriptor {
            name: "web_browse".to_string(),
            description: "Fetch and parse a web page by URL, or run a web search. Use this for \
                specific URLs (docs, articles, a known page). Prefer direct browse (default strategy \
                'auto') for a given URL; use 'search_only' with a query when you need to discover \
                pages or when the user asks to 'search for' something. When direct HTTP fetch fails \
                (403, timeout, or empty/JS-heavy page), the tool may fall back to Tavily or Perplexity \
                if API keys are configured. Use strategy 'http_only' to avoid search fallback; \
                'browser_only' is reserved for future JS-heavy pages."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "url": {
                        "type": "string",
                        "description": "The URL to fetch (required unless strategy is search_only and query is provided)"
                    },
                    "query": {
                        "type": "string",
                        "description": "Optional search query; used when strategy is search_only or as fallback query when direct fetch fails"
                    },
                    "strategy": {
                        "type": "string",
                        "description": "How to get content: 'auto' (HTTP then optional search fallback), 'http_only', 'browser_only', 'search_only'",
                        "enum": ["auto", "http_only", "browser_only", "search_only"],
                        "default": "auto"
                    },
                    "max_results": {
                        "type": "integer",
                        "description": "Max results for search fallback (Tavily); default 5",
                        "default": 5
                    }
                },
                "required": []
            }),
            returns: serde_json::json!({
                "type": "object",
                "properties": {
                    "formatted": { "type": "string" },
                    "strategy_used": { "type": "string" },
                    "title": { "type": ["string", "null"] },
                    "body_text": { "type": "string" },
                    "links": { "type": "array" },
                    "error_class": { "type": "string", "description": "If set: blocked_url, needs_browser_render, bot_protection, timeout, parse_failed, search_blocked, missing_search_key" }
                }
            }),
            cost_estimate: CostEstimate {
                latency_ms: 5000,
                network_bound: true,
                token_cost: None,
            },
            required_permissions: vec![Permission::Network(NetworkPermission::Full)],
            autonomous: true,
            requires_confirmation: false,
        }]
    }

    async fn execute_tool(
        &self,
        tool_name: &str,
        params: ToolParams,
        _context: &ExecutionContext,
    ) -> SkillResult<ToolOutput> {
        match tool_name {
            "web_browse" => {
                let url: Option<String> = params.get("url");
                let query: Option<String> = params.get("query");
                let strategy_str: Option<String> = params.get("strategy");
                let strategy = strategy_str
                    .as_deref()
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(BrowseStrategy::Auto);
                let max_results: u32 = params.get("max_results").unwrap_or(5);
                self.web_browse(url, query, strategy, max_results).await
            }
            other => Err(SkillError::ToolFailed(format!("Unknown tool: {}", other))),
        }
    }

    fn capabilities(&self) -> Vec<CapabilityDescriptor> {
        vec![]
    }

    fn get_capability(&self, _cap_type: &str) -> Option<&dyn Any> {
        None
    }

    fn triggers(&self) -> Vec<TriggerDescriptor> {
        vec![]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_manifest_parses() {
        let manifest = WebBrowseSkill::default_manifest();
        assert_eq!(manifest.id.0, "com.orion.skills.web-browse");
        assert_eq!(manifest.name, "Web Browse");
    }

    #[test]
    fn test_ssrf_blocks_localhost() {
        assert!(web_fetch::validate_url("http://localhost:8080/").is_err());
        assert!(web_fetch::validate_url("http://127.0.0.1/").is_err());
    }

    #[test]
    fn test_ssrf_allows_public() {
        assert!(web_fetch::validate_url("https://example.com/").is_ok());
    }

    #[test]
    fn test_parse_html() {
        let html = r#"<!DOCTYPE html><html><head><title>Test Page</title>
<meta name="description" content="A test"></head>
<body><h1>Hello</h1><p>World</p><a href="https://a.com">A</a></body></html>"#;
        let parsed = web_fetch::parse_html(html).unwrap();
        assert_eq!(parsed.title.as_deref(), Some("Test Page"));
        assert!(parsed.body_text.contains("Hello"));
        assert!(parsed.body_text.contains("World"));
        assert_eq!(parsed.links.len(), 1);
        assert_eq!(parsed.links[0].href, "https://a.com");
    }

    #[test]
    fn test_strategy_from_str() {
        assert_eq!(
            "auto".parse::<BrowseStrategy>().unwrap(),
            BrowseStrategy::Auto
        );
        assert_eq!(
            "http_only".parse::<BrowseStrategy>().unwrap(),
            BrowseStrategy::HttpOnly
        );
        assert_eq!(
            "search_only".parse::<BrowseStrategy>().unwrap(),
            BrowseStrategy::SearchOnly
        );
    }

    #[tokio::test]
    async fn test_web_browse_search_only_no_keys_returns_missing_key_message() {
        let tmp = std::env::temp_dir().join("orion_wb_search_no_keys");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let vault = Arc::new(Mutex::new(SecretsVault::new(tmp.clone())));
        let skill = WebBrowseSkill::with_secrets(WebBrowseSkill::default_manifest(), vault);
        let params = ToolParams::new()
            .with("query", "test query")
            .with("strategy", "search_only");
        let ctx = ExecutionContext {
            request_id: "test".to_string(),
            user_id: None,
        };
        let out = skill
            .execute_tool("web_browse", params, &ctx)
            .await
            .unwrap();
        assert!(out.success);
        let data = out.data.unwrap();
        assert_eq!(data["strategy_used"], "search_fallback");
        assert_eq!(data["error_class"], "missing_search_key");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[tokio::test]
    async fn test_web_browse_browser_only_no_url_fails() {
        let tmp = std::env::temp_dir().join("orion_wb_browser_no_url");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let vault = Arc::new(Mutex::new(SecretsVault::new(tmp.clone())));
        let skill = WebBrowseSkill::with_secrets(WebBrowseSkill::default_manifest(), vault);
        let params = ToolParams::new().with("strategy", "browser_only");
        let ctx = ExecutionContext {
            request_id: "test".to_string(),
            user_id: None,
        };
        let result = skill.execute_tool("web_browse", params, &ctx).await;
        assert!(result.is_err());
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[tokio::test]
    async fn test_web_browse_http_only_blocked_url_returns_error_class() {
        let tmp = std::env::temp_dir().join("orion_wb_http_blocked");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let vault = Arc::new(Mutex::new(SecretsVault::new(tmp.clone())));
        let skill = WebBrowseSkill::with_secrets(WebBrowseSkill::default_manifest(), vault);
        let params = ToolParams::new()
            .with("url", "http://127.0.0.1/")
            .with("strategy", "http_only");
        let ctx = ExecutionContext {
            request_id: "test".to_string(),
            user_id: None,
        };
        let out = skill
            .execute_tool("web_browse", params, &ctx)
            .await
            .unwrap();
        assert!(out.success);
        let data = out.data.unwrap();
        assert_eq!(data["strategy_used"], "http_only");
        assert!(data["error_class"].as_str().unwrap().contains("blocked"));
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
