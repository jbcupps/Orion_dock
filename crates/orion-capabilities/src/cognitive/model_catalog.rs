//! Provider model catalog and cognitive registry.
//!
//! Catalog APIs are used by tier-model management, while `CognitiveRegistry`
//! provides richer runtime model metadata for cognitive discovery endpoints.

use orion_core::config::{curated_provider_models, AppConfig, ProviderCatalogEntry};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::Duration;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelInteractionType {
    Chat,
    Instruct,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CognitiveModelEntry {
    pub id: String,
    pub interaction: ModelInteractionType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u32>,
    #[serde(default)]
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CognitiveProviderRegistry {
    pub source: String,
    #[serde(default)]
    pub warnings: Vec<String>,
    pub models: Vec<CognitiveModelEntry>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CognitiveRegistry {
    pub providers: HashMap<String, CognitiveProviderRegistry>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refreshed_at: Option<String>,
}

impl CognitiveRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn refresh(
        &mut self,
        openai_api_key: Option<&str>,
        local_openai_base_url: Option<&str>,
        ollama_base_url: Option<&str>,
    ) {
        if let Some(key) = openai_api_key.filter(|k| !k.trim().is_empty()) {
            let provider = refresh_openai_registry("https://api.openai.com", Some(key)).await;
            self.providers.insert("openai".to_string(), provider);
        }

        if let Some(base) = local_openai_base_url.filter(|b| !b.trim().is_empty()) {
            let provider = refresh_openai_registry(base, None).await;
            self.providers.insert("local".to_string(), provider);
        }

        if let Some(base) = ollama_base_url
            .or(local_openai_base_url)
            .filter(|b| !b.trim().is_empty())
        {
            let provider = refresh_ollama_registry(base).await;
            self.providers.insert("ollama".to_string(), provider);
        }

        self.refreshed_at = Some(chrono::Utc::now().to_rfc3339());
    }
}

/// Fetch available model IDs from a provider API. Returns (model_ids, source_type).
/// Falls back to curated catalog on error.
pub async fn fetch_provider_models(
    provider: &str,
    api_key: &str,
) -> (Vec<String>, String, Vec<String>) {
    let normalized = AppConfig::normalize_provider_name(provider);
    match normalized.as_str() {
        "openai" => fetch_openai_models(api_key).await,
        "anthropic" => fetch_anthropic_models(),
        "perplexity" => fetch_perplexity_models(),
        "xai" => fetch_xai_models(api_key).await,
        "google" => fetch_google_models(api_key).await,
        _ => (
            curated_provider_models(&normalized),
            "curated".into(),
            vec![],
        ),
    }
}

/// Validate that a specific model ID is available on a provider.
/// Returns Ok(()) if valid, Err with reason if not.
pub async fn validate_model(provider: &str, model: &str, api_key: &str) -> anyhow::Result<()> {
    let normalized = AppConfig::normalize_provider_name(provider);
    match normalized.as_str() {
        "anthropic" => validate_anthropic_model(model, api_key).await,
        "perplexity" => validate_perplexity_model(model),
        _ => {
            let (models, _, _) = fetch_provider_models(&normalized, api_key).await;
            if models.is_empty() {
                Ok(())
            } else if models.iter().any(|m| m == model) {
                Ok(())
            } else {
                Err(anyhow::anyhow!(
                    "Model '{}' not found in {} catalog ({} models available)",
                    model,
                    normalized,
                    models.len()
                ))
            }
        }
    }
}

/// Build a full catalog entry for a provider.
pub async fn refresh_catalog(provider: &str, api_key: &str) -> ProviderCatalogEntry {
    let (models, source, warnings) = fetch_provider_models(provider, api_key).await;
    ProviderCatalogEntry {
        available_models: models,
        source,
        last_refreshed: Some(chrono::Utc::now().to_rfc3339()),
        warnings,
        validated: false,
    }
}

#[derive(serde::Deserialize)]
struct OpenAiModelsResponse {
    data: Vec<OpenAiModelEntry>,
}

#[derive(serde::Deserialize)]
struct OpenAiModelEntry {
    id: String,
    #[serde(default)]
    context_window: Option<u32>,
    #[serde(flatten)]
    extra: std::collections::HashMap<String, serde_json::Value>,
}

#[derive(serde::Deserialize)]
struct OllamaTagsResponse {
    models: Vec<OllamaModelEntry>,
}

#[derive(serde::Deserialize)]
struct OllamaModelEntry {
    name: String,
    #[serde(default)]
    details: serde_json::Value,
}

async fn refresh_openai_registry(
    base_url: &str,
    api_key: Option<&str>,
) -> CognitiveProviderRegistry {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap_or_default();

    let mut req = client.get(format!("{}/v1/models", base_url.trim_end_matches('/')));
    if let Some(key) = api_key {
        req = req.header("Authorization", format!("Bearer {}", key));
    }

    match req.send().await {
        Ok(resp) if resp.status().is_success() => match resp.json::<OpenAiModelsResponse>().await {
            Ok(body) => {
                let mut models: Vec<CognitiveModelEntry> = body
                    .data
                    .into_iter()
                    .map(|m| {
                        let extra =
                            serde_json::to_value(&m.extra).unwrap_or(serde_json::Value::Null);
                        let context_window =
                            m.context_window.or_else(|| extract_context_window(&extra));
                        let interaction = classify_interaction(&m.id, &extra);
                        CognitiveModelEntry {
                            id: m.id.clone(),
                            tags: derive_tags(&interaction, context_window),
                            interaction,
                            context_window,
                        }
                    })
                    .collect();
                models.sort_by(|a, b| a.id.cmp(&b.id));
                models.dedup_by(|a, b| a.id == b.id);
                CognitiveProviderRegistry {
                    source: "api".to_string(),
                    warnings: vec![],
                    models,
                }
            }
            Err(_) => CognitiveProviderRegistry {
                source: "unknown".to_string(),
                warnings: vec!["failed to decode /v1/models response".to_string()],
                models: vec![],
            },
        },
        Ok(resp) => CognitiveProviderRegistry {
            source: "unknown".to_string(),
            warnings: vec![format!("/v1/models returned status {}", resp.status())],
            models: vec![],
        },
        Err(err) => CognitiveProviderRegistry {
            source: "unknown".to_string(),
            warnings: vec![format!("failed to query /v1/models: {}", err)],
            models: vec![],
        },
    }
}

async fn refresh_ollama_registry(base_url: &str) -> CognitiveProviderRegistry {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap_or_default();
    let url = format!("{}/api/tags", base_url.trim_end_matches('/'));

    match client.get(&url).send().await {
        Ok(resp) if resp.status().is_success() => match resp.json::<OllamaTagsResponse>().await {
            Ok(body) => {
                let mut models: Vec<CognitiveModelEntry> = body
                    .models
                    .into_iter()
                    .map(|m| {
                        let context_window = extract_context_window(&m.details);
                        let interaction = classify_interaction(&m.name, &m.details);
                        CognitiveModelEntry {
                            id: m.name,
                            tags: derive_tags(&interaction, context_window),
                            interaction,
                            context_window,
                        }
                    })
                    .collect();
                models.sort_by(|a, b| a.id.cmp(&b.id));
                models.dedup_by(|a, b| a.id == b.id);
                CognitiveProviderRegistry {
                    source: "api".to_string(),
                    warnings: vec![],
                    models,
                }
            }
            Err(_) => CognitiveProviderRegistry {
                source: "unknown".to_string(),
                warnings: vec!["failed to decode /api/tags response".to_string()],
                models: vec![],
            },
        },
        Ok(resp) => CognitiveProviderRegistry {
            source: "unknown".to_string(),
            warnings: vec![format!("/api/tags returned status {}", resp.status())],
            models: vec![],
        },
        Err(err) => CognitiveProviderRegistry {
            source: "unknown".to_string(),
            warnings: vec![format!("failed to query /api/tags: {}", err)],
            models: vec![],
        },
    }
}

fn classify_interaction(id: &str, metadata: &serde_json::Value) -> ModelInteractionType {
    let id_lc = id.to_ascii_lowercase();
    if id_lc.contains("instruct") {
        return ModelInteractionType::Instruct;
    }

    if metadata
        .get("type")
        .and_then(|v| v.as_str())
        .map(|t| t.eq_ignore_ascii_case("instruct"))
        .unwrap_or(false)
    {
        return ModelInteractionType::Instruct;
    }

    if id_lc.contains("chat")
        || id_lc.contains("gpt")
        || id_lc.contains("claude")
        || id_lc.contains("gemini")
        || id_lc.contains("llama")
        || id_lc.contains("qwen")
    {
        return ModelInteractionType::Chat;
    }

    ModelInteractionType::Unknown
}

fn extract_context_window(metadata: &serde_json::Value) -> Option<u32> {
    let candidates = [
        metadata.get("context_window"),
        metadata.get("max_context_length"),
        metadata.get("input_token_limit"),
        metadata.pointer("/details/context_length"),
        metadata.pointer("/details/num_ctx"),
    ];

    candidates.into_iter().flatten().find_map(|v| {
        v.as_u64()
            .map(|n| n as u32)
            .or_else(|| v.as_str()?.parse::<u32>().ok())
    })
}

fn derive_tags(interaction: &ModelInteractionType, context_window: Option<u32>) -> Vec<String> {
    let mut tags = Vec::new();
    match interaction {
        ModelInteractionType::Chat => tags.push("chat".to_string()),
        ModelInteractionType::Instruct => tags.push("instruct".to_string()),
        ModelInteractionType::Unknown => tags.push("unknown".to_string()),
    }

    if let Some(ctx) = context_window {
        if ctx >= 64_000 {
            tags.push("long-context".to_string());
        } else if ctx <= 8_192 {
            tags.push("short-context".to_string());
        }
    }

    tags
}

async fn fetch_openai_models(api_key: &str) -> (Vec<String>, String, Vec<String>) {
    let provider = refresh_openai_registry("https://api.openai.com", Some(api_key)).await;
    (
        provider.models.into_iter().map(|m| m.id).collect(),
        provider.source,
        provider.warnings,
    )
}

fn fetch_anthropic_models() -> (Vec<String>, String, Vec<String>) {
    (
        curated_provider_models("anthropic"),
        "curated".into(),
        vec![],
    )
}

async fn validate_anthropic_model(model: &str, api_key: &str) -> anyhow::Result<()> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()?;

    let body = serde_json::json!({
        "model": model,
        "max_tokens": 1,
        "messages": [{"role": "user", "content": "hi"}]
    });

    let resp = client
        .post("https://api.anthropic.com/v1/messages")
        .header("x-api-key", api_key)
        .header("anthropic-version", "2023-06-01")
        .header("content-type", "application/json")
        .body(body.to_string())
        .send()
        .await?;

    match resp.status().as_u16() {
        200 | 429 | 529 => Ok(()),
        404 => Err(anyhow::anyhow!("Model '{}' not found on Anthropic", model)),
        400 => {
            let text = resp.text().await.unwrap_or_default();
            if text.contains("not_found_error") || text.contains("unknown model") {
                Err(anyhow::anyhow!("Model '{}' not found on Anthropic", model))
            } else {
                Ok(())
            }
        }
        _ => Ok(()),
    }
}

fn fetch_perplexity_models() -> (Vec<String>, String, Vec<String>) {
    (
        curated_provider_models("perplexity"),
        "curated".into(),
        vec![],
    )
}

fn validate_perplexity_model(model: &str) -> anyhow::Result<()> {
    let known = curated_provider_models("perplexity");
    if known.iter().any(|m| m == model) {
        Ok(())
    } else {
        Err(anyhow::anyhow!(
            "Model '{}' is not a known Perplexity model. Known: {:?}",
            model,
            known
        ))
    }
}

async fn fetch_xai_models(api_key: &str) -> (Vec<String>, String, Vec<String>) {
    let provider = refresh_openai_registry("https://api.x.ai", Some(api_key)).await;
    (
        provider.models.into_iter().map(|m| m.id).collect(),
        provider.source,
        provider.warnings,
    )
}

#[derive(serde::Deserialize)]
struct GoogleModelsResponse {
    models: Vec<GoogleModelEntry>,
}

#[derive(serde::Deserialize)]
struct GoogleModelEntry {
    name: String,
}

async fn fetch_google_models(api_key: &str) -> (Vec<String>, String, Vec<String>) {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap_or_default();

    let url = format!(
        "https://generativelanguage.googleapis.com/v1/models?key={}",
        api_key
    );

    match client.get(&url).send().await {
        Ok(resp) if resp.status().is_success() => {
            if let Ok(body) = resp.json::<GoogleModelsResponse>().await {
                let mut ids: Vec<String> = body
                    .models
                    .into_iter()
                    .map(|m| {
                        m.name
                            .strip_prefix("models/")
                            .unwrap_or(&m.name)
                            .to_string()
                    })
                    .collect();
                ids.sort();
                ids.dedup();
                (ids, "api".into(), vec![])
            } else {
                (curated_provider_models("google"), "curated".into(), vec![])
            }
        }
        _ => (curated_provider_models("google"), "curated".into(), vec![]),
    }
}
