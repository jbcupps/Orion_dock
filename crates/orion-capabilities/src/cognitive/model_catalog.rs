//! Provider model catalog — fetch available models and validate model IDs.
//!
//! Strategy per provider (from official docs):
//! - **OpenAI**: `GET /v1/models` returns authoritative runtime list.
//! - **Anthropic**: No public model-list API; use curated catalog + validation via minimal completion.
//! - **Perplexity**: Documented enum (sonar, sonar-pro, sonar-deep-research, sonar-reasoning-pro).
//! - **xAI**: OpenAI-compatible `GET /v1/models`; curated fallback.
//! - **Google Gemini**: `GET /v1/models?key=...` returns model list with lifecycle metadata.

use orion_core::config::{curated_provider_models, AppConfig, ProviderCatalogEntry};
use std::time::Duration;

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
            // For OpenAI, xAI, Google: check against fetched model list
            let (models, _, _) = fetch_provider_models(&normalized, api_key).await;
            if models.is_empty() {
                // Could not fetch; accept optimistically
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

// ── OpenAI ───────────────────────────────────────────────────────────

#[derive(serde::Deserialize)]
struct OpenAiModelsResponse {
    data: Vec<OpenAiModelEntry>,
}

#[derive(serde::Deserialize)]
struct OpenAiModelEntry {
    id: String,
}

async fn fetch_openai_models(api_key: &str) -> (Vec<String>, String, Vec<String>) {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap_or_default();

    match client
        .get("https://api.openai.com/v1/models")
        .header("Authorization", format!("Bearer {}", api_key))
        .send()
        .await
    {
        Ok(resp) if resp.status().is_success() => {
            if let Ok(body) = resp.json::<OpenAiModelsResponse>().await {
                let mut ids: Vec<String> = body
                    .data
                    .into_iter()
                    .map(|m| m.id)
                    .filter(|id| {
                        // Filter to chat-relevant models
                        id.starts_with("gpt-")
                            || id.starts_with("o1")
                            || id.starts_with("o3")
                            || id.starts_with("chatgpt-")
                    })
                    .collect();
                ids.sort();
                ids.dedup();
                (ids, "api".into(), vec![])
            } else {
                (curated_provider_models("openai"), "curated".into(), vec![])
            }
        }
        _ => (curated_provider_models("openai"), "curated".into(), vec![]),
    }
}

// ── Anthropic ────────────────────────────────────────────────────────

fn fetch_anthropic_models() -> (Vec<String>, String, Vec<String>) {
    // Anthropic has no public model list API; use curated catalog.
    (
        curated_provider_models("anthropic"),
        "curated".into(),
        vec![],
    )
}

async fn validate_anthropic_model(model: &str, api_key: &str) -> anyhow::Result<()> {
    // Anthropic model validation: send a minimal completion and check for model-not-found errors.
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
        200 | 429 | 529 => Ok(()), // Success or rate-limited (model exists)
        404 => Err(anyhow::anyhow!("Model '{}' not found on Anthropic", model)),
        400 => {
            let text = resp.text().await.unwrap_or_default();
            if text.contains("not_found_error") || text.contains("unknown model") {
                Err(anyhow::anyhow!("Model '{}' not found on Anthropic", model))
            } else {
                Ok(()) // Other 400 errors likely not model-related
            }
        }
        _ => Ok(()), // Assume valid for other statuses
    }
}

// ── Perplexity ───────────────────────────────────────────────────────

fn fetch_perplexity_models() -> (Vec<String>, String, Vec<String>) {
    // Perplexity documents a fixed model enum.
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

// ── xAI ──────────────────────────────────────────────────────────────

async fn fetch_xai_models(api_key: &str) -> (Vec<String>, String, Vec<String>) {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap_or_default();

    match client
        .get("https://api.x.ai/v1/models")
        .header("Authorization", format!("Bearer {}", api_key))
        .send()
        .await
    {
        Ok(resp) if resp.status().is_success() => {
            if let Ok(body) = resp.json::<OpenAiModelsResponse>().await {
                let mut ids: Vec<String> = body.data.into_iter().map(|m| m.id).collect();
                ids.sort();
                ids.dedup();
                (ids, "api".into(), vec![])
            } else {
                (curated_provider_models("xai"), "curated".into(), vec![])
            }
        }
        _ => (curated_provider_models("xai"), "curated".into(), vec![]),
    }
}

// ── Google Gemini ────────────────────────────────────────────────────

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
                        // Google returns "models/gemini-2.5-pro"; strip prefix.
                        m.name
                            .strip_prefix("models/")
                            .unwrap_or(&m.name)
                            .to_string()
                    })
                    .filter(|id| id.starts_with("gemini-"))
                    .collect();
                ids.sort();
                ids.dedup();

                let mut warnings = vec![];
                // Lifecycle warning for soon-to-retire models
                if ids.iter().any(|id| id.starts_with("gemini-2.0-flash")) {
                    warnings.push(
                        "gemini-2.0-flash models are scheduled for retirement (March 2026)"
                            .to_string(),
                    );
                }
                (ids, "api".into(), warnings)
            } else {
                (curated_provider_models("google"), "curated".into(), vec![])
            }
        }
        _ => (curated_provider_models("google"), "curated".into(), vec![]),
    }
}
