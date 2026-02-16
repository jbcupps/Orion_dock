use serde_json::Value;

use crate::superego;

/// Capability envelope for the current turn.
#[derive(Debug, Clone)]
pub struct CapabilityEnvelope {
    pub allow_web: bool,
    pub allow_toolbox_exec: bool,
    pub allow_file_write: bool,
    pub require_confirmation: bool,
    pub advisory_code: Option<String>,
}

/// Outcome for a single tool request gate.
#[derive(Debug, Clone)]
pub struct CapabilityGateResult {
    pub allowed: bool,
    pub reason_code: Option<&'static str>,
    pub reason_text: Option<String>,
    pub require_confirmation: bool,
}

impl CapabilityGateResult {
    fn allow() -> Self {
        Self {
            allowed: true,
            reason_code: None,
            reason_text: None,
            require_confirmation: false,
        }
    }

    fn deny(code: &'static str, text: impl Into<String>) -> Self {
        Self {
            allowed: false,
            reason_code: Some(code),
            reason_text: Some(text.into()),
            require_confirmation: false,
        }
    }
}

pub fn evaluate_user_message_for_capabilities(
    _message: &str,
    advisory_code: Option<&str>,
) -> CapabilityEnvelope {
    let mut envelope = CapabilityEnvelope {
        allow_web: true,
        allow_toolbox_exec: true,
        allow_file_write: true,
        require_confirmation: false,
        advisory_code: advisory_code.map(str::to_string),
    };

    if advisory_code.is_some() {
        // Advisory mode keeps chat open but tightens mutation/exec capabilities.
        envelope.allow_toolbox_exec = false;
        envelope.allow_file_write = false;
        envelope.require_confirmation = true;
    }

    envelope
}

pub fn evaluate_tool_request(
    tool_name: &str,
    args: &Value,
    envelope: &CapabilityEnvelope,
) -> CapabilityGateResult {
    if !envelope.allow_web
        && matches!(tool_name, "web_search" | "perplexity_search" | "web_browse")
    {
        return CapabilityGateResult::deny(
            superego::CODE_PII_DOXXING,
            "Web capabilities are restricted for this request",
        );
    }

    if !envelope.allow_toolbox_exec && matches!(tool_name, "toolbox_exec" | "shell_execute") {
        return CapabilityGateResult::deny(
            "CAPABILITY_RESTRICTED",
            "Execution tools are restricted for this request",
        );
    }

    if !envelope.allow_file_write && tool_name == "file_write" {
        return CapabilityGateResult::deny(
            "CAPABILITY_RESTRICTED",
            "File write tools are restricted for this request",
        );
    }

    if matches!(tool_name, "web_search" | "perplexity_search" | "web_browse") {
        if let Some(query) = extract_search_like_query(args) {
            let verdict = superego::check_search_query(&query);
            if !verdict.allowed {
                return CapabilityGateResult {
                    allowed: false,
                    reason_code: verdict.reason_code,
                    reason_text: verdict.reason_text,
                    require_confirmation: false,
                };
            }
        }
    }

    let mut out = CapabilityGateResult::allow();
    out.require_confirmation = envelope.require_confirmation
        && matches!(tool_name, "toolbox_exec" | "shell_execute" | "file_write" | "send_email");
    out
}

fn extract_search_like_query(args: &Value) -> Option<String> {
    let obj = args.as_object()?;
    if let Some(q) = obj.get("query").and_then(|v| v.as_str()) {
        return Some(q.to_string());
    }
    if let Some(q) = obj.get("url").and_then(|v| v.as_str()) {
        return Some(q.to_string());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn advisory_envelope_reduces_mutation_capabilities() {
        let env = evaluate_user_message_for_capabilities("hello", Some("PROMPT_INJECTION"));
        assert!(!env.allow_toolbox_exec);
        assert!(!env.allow_file_write);
        assert!(env.require_confirmation);
    }

    #[test]
    fn tool_gate_blocks_doxxing_search_query() {
        let env = evaluate_user_message_for_capabilities("hello", None);
        let args = serde_json::json!({
            "query": "where does Elon Musk live"
        });
        let gate = evaluate_tool_request("web_search", &args, &env);
        assert!(!gate.allowed);
        assert_eq!(gate.reason_code, Some(superego::CODE_PII_DOXXING));
    }

    #[test]
    fn tool_gate_allows_benign_weather_query() {
        let env = evaluate_user_message_for_capabilities("hello", None);
        let args = serde_json::json!({
            "query": "What is the weather in Miami right now?"
        });
        let gate = evaluate_tool_request("web_search", &args, &env);
        assert!(gate.allowed);
    }
}
