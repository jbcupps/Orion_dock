//! Birth chat runtime adapter: builds messages from orchestrator history,
//! pulls stage prompts, invokes IdEgoRouter, and parses text-based tool requests.

use crate::prompts::system_prompt_for_stage_with_context;
use crate::stages::{BirthOrchestrator, BirthStage};
use orion_capabilities::cognitive::Message;
use orion_core::{AppConfig, RoutingMode, SecretsVault, TrinityConfig};
use orion_router::IdEgoRouter;
use serde::{Deserialize, Serialize};

/// Parsed tool request from birth chat (text-based ```tool_request``` blocks).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BirthToolRequest {
    pub name: String,
    pub arguments: serde_json::Value,
}

/// Result of one birth chat turn: assistant text and any tool requests to execute.
#[derive(Debug, Clone)]
pub struct BirthChatResponse {
    /// Assistant reply text (tool blocks stripped for display if desired).
    pub assistant_content: String,
    /// Parsed tool requests from the reply.
    pub tool_requests: Vec<BirthToolRequest>,
}

/// Build chat messages for the current birth stage: system prompt + conversation history.
/// Pass `stored_providers` for Connectivity stage (e.g. from SecretsVault::list_providers).
/// If `next_user_message` is Some, it is appended as the latest user message (not yet in orchestrator history).
pub fn build_birth_messages(
    orchestrator: &BirthOrchestrator,
    stored_providers: &[String],
    next_user_message: Option<&str>,
) -> Vec<Message> {
    let stage = orchestrator.current_stage();
    let system = match system_prompt_for_stage_with_context(stage, stored_providers) {
        Some(s) => s,
        None => return vec![],
    };

    let mut messages = vec![Message::new("system", system)];

    for (role, content) in orchestrator.get_conversation() {
        messages.push(Message::new(role.as_str(), content.as_str()));
    }

    if let Some(user_msg) = next_user_message {
        if !user_msg.is_empty() {
            messages.push(Message::new("user", user_msg));
        }
    }

    messages
}

/// Build chat messages for Genesis stage from a conversation list (for API use without an orchestrator).
pub fn build_genesis_messages(conversation: &[(String, String)]) -> Vec<Message> {
    let system = match system_prompt_for_stage_with_context(BirthStage::Genesis, &[]) {
        Some(s) => s,
        None => return vec![],
    };
    let mut messages = vec![Message::new("system", system)];
    for (role, content) in conversation {
        messages.push(Message::new(role.as_str(), content.as_str()));
    }
    messages
}

/// Build an IdEgoRouter for birth: pinned local birth model and optional Ego when API key is present.
/// Uses `config.effective_birth_model()` for Id and EgoPrimary mode so cloud is tried first with local fallback once key is set.
pub async fn build_birth_router(config: &AppConfig) -> IdEgoRouter {
    let local_url = config.local_llm_base_url.clone();
    let id_model = Some(config.effective_birth_model());
    let (ego_name, ego_key) = ego_from_config(config);
    IdEgoRouter::with_provider_auto_detect_and_id_model(
        local_url,
        id_model,
        ego_name.as_deref(),
        ego_key,
        RoutingMode::EgoPrimary,
    )
    .await
}

fn ego_from_config(config: &AppConfig) -> (Option<String>, Option<String>) {
    if let Some(ref trinity) = config.trinity {
        if let Some(ref key) = trinity.ego_api_key {
            if !key.is_empty() {
                return (trinity.ego_provider.clone(), Some(key.clone()));
            }
        }
    }
    if let Some(ref key) = config.openai_api_key {
        if !key.is_empty() {
            return (Some("openai".to_string()), Some(key.clone()));
        }
    }
    (None, None)
}

/// Resolve provider name from key prefix when provider is "auto".
pub fn detect_provider_from_key(key: &str) -> Option<&'static str> {
    let key = key.trim();
    if key.starts_with("sk-ant-") {
        Some("anthropic")
    } else if key.starts_with("sk-") {
        Some("openai")
    } else if key.starts_with("pplx-") {
        Some("perplexity")
    } else if key.starts_with("xai-") {
        Some("xai")
    } else if key.starts_with("AIza") {
        Some("google")
    } else if key.starts_with("tvly-") {
        Some("tavily")
    } else {
        None
    }
}

/// Extract API keys from free text (e.g. user chat messages).
/// Returns a list of (provider, full_key) pairs found in the text.
/// Uses the same prefix patterns as `detect_provider_from_key` and `redact_api_keys`.
/// Deduplicates: a key matched by `sk-ant-` (anthropic) won't also match `sk-` (openai).
pub fn extract_api_keys_from_text(text: &str) -> Vec<(&'static str, String)> {
    // Order matters: sk-ant- must come before sk- to avoid partial matches.
    let prefixes: &[(&str, usize, &str)] = &[
        ("sk-ant-", 7, "anthropic"),
        ("sk-", 3, "openai"),
        ("pplx-", 5, "perplexity"),
        ("xai-", 4, "xai"),
        ("AIza", 4, "google"),
        ("tvly-", 5, "tavily"),
    ];
    let mut results: Vec<(&'static str, String)> = Vec::new();
    for &(prefix, prefix_len, provider) in prefixes {
        let mut remaining = text;
        while let Some(pos) = remaining.find(prefix) {
            let after_prefix = &remaining[pos + prefix_len..];
            let key_chars: usize = after_prefix
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
                .count();
            if key_chars >= 10 {
                let full_key = &remaining[pos..pos + prefix_len + key_chars];
                // Skip if this key was already captured by a more specific prefix
                if !results.iter().any(|(_, k)| {
                    k == full_key || full_key.starts_with(k.as_str()) || k.starts_with(full_key)
                }) {
                    results.push((provider, full_key.to_string()));
                }
            }
            remaining = &remaining[pos + prefix_len + key_chars.max(1)..];
        }
    }
    results
}

/// Execute the store_provider_key tool: resolve provider (including "auto"), store in vault, update config, save config.
/// Returns the resolved provider name so the caller can rebuild the router.
pub fn execute_store_provider_key(
    vault: &mut SecretsVault,
    config: &mut AppConfig,
    provider: &str,
    key: &str,
) -> anyhow::Result<String> {
    let key = key.trim();
    if key.is_empty() {
        anyhow::bail!("API key cannot be empty");
    }
    let resolved = match provider.trim().to_lowercase().as_str() {
        "auto" => detect_provider_from_key(key)
            .map(String::from)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "Could not detect provider from key prefix; specify provider explicitly"
                )
            })?,
        p => p.to_string(),
    };
    vault.set_secret(&resolved, key);
    vault
        .save()
        .map_err(|e| anyhow::anyhow!("Failed to save vault: {}", e))?;

    // Update config so next build_birth_router() picks up Ego.
    if resolved == "openai" {
        config.openai_api_key = Some(key.to_string());
    }
    if config.trinity.is_none() {
        config.trinity = Some(TrinityConfig::default());
    }
    if let Some(ref mut trinity) = config.trinity {
        trinity.ego_provider = Some(resolved.clone());
        trinity.ego_api_key = Some(key.to_string());
    }
    config
        .save(&config.config_path())
        .map_err(|e| anyhow::anyhow!("Failed to save config: {}", e))?;
    Ok(resolved)
}

/// Run one birth chat turn: route messages through the router and return response plus parsed tool requests.
/// For birth we use local-first: when Ego is configured we use route() (Ego-primary with fallback); otherwise id_only().
pub async fn birth_chat_turn(
    router: &IdEgoRouter,
    messages: Vec<Message>,
) -> anyhow::Result<BirthChatResponse> {
    if messages.len() <= 1 {
        return Ok(BirthChatResponse {
            assistant_content: String::new(),
            tool_requests: vec![],
        });
    }

    let response = if router.has_ego() {
        router.route(messages).await?
    } else {
        router.id_only(messages).await?
    };

    let (assistant_content, tool_requests) = parse_tool_requests(&response.content);
    Ok(BirthChatResponse {
        assistant_content,
        tool_requests,
    })
}

/// Redact API keys from text to prevent them from being stored in conversation history.
/// Replaces key-like patterns with their prefix + `***`.
pub fn redact_api_keys(text: &str) -> String {
    let mut result = text.to_string();
    // Order matters: sk-ant- must come before sk- to avoid partial matches.
    let prefixes: &[(&str, usize)] = &[
        ("sk-ant-", 7),
        ("sk-", 3),
        ("pplx-", 5),
        ("xai-", 4),
        ("AIza", 4),
        ("tvly-", 5),
    ];
    for &(prefix, prefix_len) in prefixes {
        let mut output = String::with_capacity(result.len());
        let mut remaining = result.as_str();
        while let Some(pos) = remaining.find(prefix) {
            output.push_str(&remaining[..pos]);
            let after_prefix = &remaining[pos + prefix_len..];
            // Count how many key-like chars follow the prefix
            let key_chars = after_prefix
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
                .count();
            if key_chars >= 10 {
                output.push_str(prefix);
                output.push_str("***");
                remaining = &remaining[pos + prefix_len + key_chars..];
            } else {
                // Not a real key, keep original text
                output.push_str(&remaining[pos..pos + prefix_len]);
                remaining = &remaining[pos + prefix_len..];
            }
        }
        output.push_str(remaining);
        result = output;
    }
    result
}

/// Parse ```tool_request\n{...}``` blocks from assistant content.
/// Returns (content with blocks stripped, vec of parsed tool requests).
pub fn parse_tool_requests(content: &str) -> (String, Vec<BirthToolRequest>) {
    const MARKER: &str = "```tool_request";
    let mut out = String::with_capacity(content.len());
    let mut tool_requests = Vec::new();
    let mut rest = content;

    while let Some(open) = rest.find(MARKER) {
        out.push_str(&rest[..open]);
        rest = &rest[open + MARKER.len()..];
        let close = rest.find("```").unwrap_or(rest.len());
        let block = rest[..close].trim();
        rest = rest[close.min(rest.len())..]
            .strip_prefix("```")
            .unwrap_or(rest);

        if let Ok(parsed) = serde_json::from_str::<BirthToolRequest>(block) {
            tool_requests.push(parsed);
        }
    }
    out.push_str(rest);
    (out.trim().to_string(), tool_requests)
}

#[cfg(test)]
mod tests {
    use super::*;
    use orion_core::AppConfig;
    use std::path::Path;

    fn test_config(base: &Path) -> AppConfig {
        let data_dir = base.join("data");
        std::fs::create_dir_all(&data_dir).unwrap();
        let mut config = AppConfig::default_paths();
        config.data_dir = data_dir.clone();
        config.models_dir = data_dir.join("models");
        config.docs_dir = data_dir.join("docs");
        config.db_path = data_dir.join("test.db");
        config
    }

    #[tokio::test]
    async fn test_build_birth_router_no_key_no_ego() {
        let tmp = std::env::temp_dir().join("orion_birth_router_no_key");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let config = test_config(&tmp);
        let router = build_birth_router(&config).await;
        assert!(!router.has_ego());
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[tokio::test]
    async fn test_build_birth_router_with_key_has_ego() {
        let tmp = std::env::temp_dir().join("orion_birth_router_with_key");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let mut config = test_config(&tmp);
        config.openai_api_key = Some("sk-test-fake-key".to_string());
        let router = build_birth_router(&config).await;
        assert!(router.has_ego());
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_detect_provider_from_key() {
        assert_eq!(detect_provider_from_key("sk-ant-xxx"), Some("anthropic"));
        assert_eq!(detect_provider_from_key("sk-abc"), Some("openai"));
        assert_eq!(detect_provider_from_key("pplx-xxx"), Some("perplexity"));
        assert_eq!(detect_provider_from_key("xai-xxx"), Some("xai"));
        assert_eq!(detect_provider_from_key("AIzaxxx"), Some("google"));
        assert_eq!(detect_provider_from_key("tvly-xxx"), Some("tavily"));
        assert_eq!(detect_provider_from_key("unknown"), None);
    }

    #[test]
    fn test_execute_store_provider_key() {
        let tmp = std::env::temp_dir().join("orion_birth_store_key");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let mut config = test_config(&tmp);
        let mut vault = SecretsVault::new(config.data_dir.clone());
        let resolved =
            execute_store_provider_key(&mut vault, &mut config, "openai", "sk-test-key").unwrap();
        assert_eq!(resolved, "openai");
        assert_eq!(config.openai_api_key.as_deref(), Some("sk-test-key"));
        assert_eq!(vault.get_secret("openai"), Some("sk-test-key"));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_parse_tool_requests_empty() {
        let (content, tools) = parse_tool_requests("Just text.");
        assert_eq!(content, "Just text.");
        assert!(tools.is_empty());
    }

    #[test]
    fn test_parse_tool_requests_one() {
        let s = r#"Here you go. ```tool_request
{"name": "store_provider_key", "arguments": {"provider": "openai", "key": "sk-x"}}
``` Done."#;
        let (content, tools) = parse_tool_requests(s);
        assert!(content.contains("Here you go.") && content.contains("Done."));
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "store_provider_key");
        assert_eq!(tools[0].arguments["provider"], "openai");
    }

    #[test]
    fn test_parse_tool_requests_invalid_json_ignored() {
        let s = r#"Text ```tool_request
{ invalid }
``` More."#;
        let (_, tools) = parse_tool_requests(s);
        assert!(tools.is_empty());
    }

    #[test]
    fn test_redact_api_keys_openai() {
        let text = "Here is my key: sk-abcdefghij1234567890";
        let redacted = redact_api_keys(text);
        assert_eq!(redacted, "Here is my key: sk-***");
    }

    #[test]
    fn test_redact_api_keys_anthropic() {
        let text = "My anthropic key is sk-ant-abcdefghij1234567890";
        let redacted = redact_api_keys(text);
        assert_eq!(redacted, "My anthropic key is sk-ant-***");
    }

    #[test]
    fn test_redact_api_keys_multiple() {
        let text = "openai: sk-AAAAAAAAAA1234567890 and anthropic: sk-ant-BBBBBBBBBB1234567890";
        let redacted = redact_api_keys(text);
        assert_eq!(redacted, "openai: sk-*** and anthropic: sk-ant-***");
    }

    #[test]
    fn test_redact_api_keys_short_prefix_not_redacted() {
        let text = "sk-short is not a key";
        let redacted = redact_api_keys(text);
        assert_eq!(redacted, "sk-short is not a key");
    }

    #[test]
    fn test_redact_api_keys_no_keys() {
        let text = "No keys here, just regular text.";
        let redacted = redact_api_keys(text);
        assert_eq!(redacted, text);
    }

    #[test]
    fn test_redact_api_keys_all_providers() {
        assert!(redact_api_keys("pplx-abcdefghij1234567890").contains("pplx-***"));
        assert!(redact_api_keys("xai-abcdefghij1234567890").contains("xai-***"));
        assert!(redact_api_keys("AIzaabcdefghij1234567890").contains("AIza***"));
        assert!(redact_api_keys("tvly-abcdefghij1234567890").contains("tvly-***"));
    }
}
