//! Unified tool-call extraction: structured-first with legacy text fallback.
//!
//! LLM providers can return tool calls two ways:
//! 1. **Structured** — `CompletionResponse.tool_calls` (native function-calling).
//! 2. **Legacy text** — ` ```tool_request` / `<tool_request>` blocks in `content`.
//!
//! This module merges both sources into a single `Vec<UnifiedToolCall>`, preferring
//! structured calls when available while preserving backward compatibility with
//! text-emitted tool blocks (used by local models without function-calling support).

use orion_birth::{parse_tool_requests, BirthToolRequest};
use orion_capabilities::cognitive::provider::{CompletionResponse, ToolDefinition};
use orion_core::system_prompt::SkillToolEntry;
use serde::{Deserialize, Serialize};

/// A normalized tool call ready for skill execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnifiedToolCall {
    /// Tool call ID (from structured call, or synthetic for legacy).
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
    /// Whether this came from structured function-calling (true) or text parsing (false).
    pub structured: bool,
}

/// Result of extracting tool calls from a CompletionResponse.
pub struct ExtractionResult {
    /// The assistant content with legacy tool blocks stripped.
    pub clean_content: String,
    /// All tool calls (structured first, then any unique legacy ones).
    pub tool_calls: Vec<UnifiedToolCall>,
}

/// Extract tool calls from a CompletionResponse, merging structured and legacy sources.
///
/// Priority: structured `tool_calls` are taken first. Then any legacy text blocks
/// are parsed. Duplicates (same name + arguments) from the text path are dropped
/// to avoid double-execution when a model emits both.
pub fn extract_tool_calls(response: &CompletionResponse) -> ExtractionResult {
    let mut calls: Vec<UnifiedToolCall> = Vec::new();

    // 1. Structured tool calls from the provider response.
    if let Some(ref tcs) = response.tool_calls {
        for tc in tcs {
            let args = serde_json::from_str::<serde_json::Value>(&tc.arguments)
                .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));
            calls.push(UnifiedToolCall {
                id: tc.id.clone(),
                name: tc.name.clone(),
                arguments: args,
                structured: true,
            });
        }
    }

    // 2. Legacy text blocks (```tool_request / <tool_request>).
    let (clean_content, text_calls) = parse_tool_requests(&response.content);

    let mut legacy_idx: u32 = 0;
    for tr in text_calls {
        // De-duplicate: skip if we already have a structured call with the same name+args.
        let dominated = calls
            .iter()
            .any(|c| c.name == tr.name && c.arguments == tr.arguments);
        if dominated {
            continue;
        }
        calls.push(UnifiedToolCall {
            id: format!("text-{}", legacy_idx),
            name: tr.name,
            arguments: tr.arguments,
            structured: false,
        });
        legacy_idx += 1;
    }

    ExtractionResult {
        clean_content,
        tool_calls: calls,
    }
}

/// Convert a UnifiedToolCall back to BirthToolRequest for compatibility with
/// existing execution code that expects (name, arguments: Value).
impl From<&UnifiedToolCall> for BirthToolRequest {
    fn from(utc: &UnifiedToolCall) -> Self {
        BirthToolRequest {
            name: utc.name.clone(),
            arguments: utc.arguments.clone(),
        }
    }
}

// ---------------------------------------------------------------------------
// ToolDefinition builders
// ---------------------------------------------------------------------------

/// Build `Vec<ToolDefinition>` from registered skill tools for passing to the router.
pub fn build_tool_definitions(skill_tools: &[SkillToolEntry]) -> Vec<ToolDefinition> {
    let mut defs: Vec<ToolDefinition> = Vec::new();

    for entry in skill_tools {
        if !entry.ready {
            continue; // skip skills missing required secrets
        }
        defs.push(ToolDefinition {
            name: entry.tool_name.clone(),
            description: entry.tool_description.clone(),
            parameters: entry.parameters.clone(),
        });
    }

    // Credential management tools (always available).
    defs.push(ToolDefinition {
        name: "store_provider_key".to_string(),
        description: "Store an LLM API key in the agent's encrypted vault. Use provider 'auto' for auto-detection from key prefix.".to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "provider": { "type": "string", "description": "Provider name or 'auto'" },
                "key": { "type": "string", "description": "The API key" }
            },
            "required": ["provider", "key"]
        }),
    });

    defs.push(ToolDefinition {
        name: "store_vault_secret".to_string(),
        description: "Store a named secret (non-API-key credential) in the agent's vault."
            .to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "key": { "type": "string", "description": "Secret name (e.g. protonmail_user)" },
                "value": { "type": "string", "description": "Secret value" }
            },
            "required": ["key", "value"]
        }),
    });

    defs
}

/// Build tool definitions for agentic mode: skill tools + synthetic tools.
pub fn build_agentic_tool_definitions(skill_tools: &[SkillToolEntry]) -> Vec<ToolDefinition> {
    let mut defs = build_tool_definitions(skill_tools);

    // Synthetic agentic tools.
    defs.push(ToolDefinition {
        name: "task_complete".to_string(),
        description: "Signal that the agentic task is finished.".to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "summary": { "type": "string", "description": "What was accomplished" },
                "status": { "type": "string", "enum": ["success", "partial", "failed"], "description": "Outcome status" }
            },
            "required": ["summary"]
        }),
    });

    defs.push(ToolDefinition {
        name: "ask_mentor".to_string(),
        description: "Pause execution and ask the mentor a question. Use sparingly.".to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "question": { "type": "string", "description": "The question for the mentor" }
            },
            "required": ["question"]
        }),
    });

    defs.push(ToolDefinition {
        name: "register_mcp_skill".to_string(),
        description: "Connect to an MCP server and register its tools for use in subsequent turns."
            .to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "server_id": { "type": "string", "description": "Unique ID for this MCP server" },
                "server_name": { "type": "string", "description": "Human-readable name" },
                "base_url": { "type": "string", "description": "HTTP base URL of the MCP server" }
            },
            "required": ["server_id", "base_url"]
        }),
    });

    defs
}

#[cfg(test)]
mod tests {
    use super::*;
    use orion_capabilities::cognitive::provider::ToolCall;

    #[test]
    fn test_extract_structured_only() {
        let response = CompletionResponse {
            content: "I'll search for that.".to_string(),
            tool_calls: Some(vec![ToolCall {
                id: "call_1".to_string(),
                name: "web_search".to_string(),
                arguments: r#"{"query":"weather miami"}"#.to_string(),
            }]),
        };
        let result = extract_tool_calls(&response);
        assert_eq!(result.tool_calls.len(), 1);
        assert!(result.tool_calls[0].structured);
        assert_eq!(result.tool_calls[0].name, "web_search");
        assert_eq!(result.clean_content, "I'll search for that.");
    }

    #[test]
    fn test_extract_legacy_text_only() {
        let content = r#"Let me search for that.

```tool_request
{"name": "web_search", "arguments": {"query": "weather miami"}}
```"#;
        let response = CompletionResponse {
            content: content.to_string(),
            tool_calls: None,
        };
        let result = extract_tool_calls(&response);
        assert_eq!(result.tool_calls.len(), 1);
        assert!(!result.tool_calls[0].structured);
        assert_eq!(result.tool_calls[0].name, "web_search");
        assert!(result.clean_content.contains("Let me search"));
        assert!(!result.clean_content.contains("tool_request"));
    }

    #[test]
    fn test_extract_deduplicates_structured_and_legacy() {
        let content = r#"Searching now.

```tool_request
{"name": "web_search", "arguments": {"query": "weather miami"}}
```"#;
        let response = CompletionResponse {
            content: content.to_string(),
            tool_calls: Some(vec![ToolCall {
                id: "call_1".to_string(),
                name: "web_search".to_string(),
                arguments: r#"{"query":"weather miami"}"#.to_string(),
            }]),
        };
        let result = extract_tool_calls(&response);
        // Should be deduplicated to 1, not 2.
        assert_eq!(result.tool_calls.len(), 1);
        assert!(result.tool_calls[0].structured);
    }

    #[test]
    fn test_extract_different_tools_not_deduplicated() {
        let content = r#"Let me do both.

```tool_request
{"name": "file_read", "arguments": {"path": "/tmp/test"}}
```"#;
        let response = CompletionResponse {
            content: content.to_string(),
            tool_calls: Some(vec![ToolCall {
                id: "call_1".to_string(),
                name: "web_search".to_string(),
                arguments: r#"{"query":"weather"}"#.to_string(),
            }]),
        };
        let result = extract_tool_calls(&response);
        assert_eq!(result.tool_calls.len(), 2);
    }

    #[test]
    fn test_build_tool_definitions_includes_credential_tools() {
        let defs = build_tool_definitions(&[]);
        let names: Vec<&str> = defs.iter().map(|d| d.name.as_str()).collect();
        assert!(names.contains(&"store_provider_key"));
        assert!(names.contains(&"store_vault_secret"));
    }

    #[test]
    fn test_build_agentic_tool_definitions_includes_synthetic() {
        let defs = build_agentic_tool_definitions(&[]);
        let names: Vec<&str> = defs.iter().map(|d| d.name.as_str()).collect();
        assert!(names.contains(&"task_complete"));
        assert!(names.contains(&"ask_mentor"));
        assert!(names.contains(&"register_mcp_skill"));
        assert!(names.contains(&"store_provider_key"));
    }

    #[test]
    fn test_build_tool_definitions_skips_unready_skills() {
        let entries = vec![
            SkillToolEntry {
                skill_name: "Web Search".to_string(),
                skill_id: "com.orion.skills.web-search".to_string(),
                trust_tier: "Verified".to_string(),
                tool_name: "web_search".to_string(),
                tool_description: "Search the web".to_string(),
                parameters: serde_json::json!({}),
                ready: false,
                missing_secrets: vec!["tavily".to_string()],
            },
            SkillToolEntry {
                skill_name: "Shell".to_string(),
                skill_id: "com.orion.skills.shell".to_string(),
                trust_tier: "Verified".to_string(),
                tool_name: "shell_execute".to_string(),
                tool_description: "Execute a shell command".to_string(),
                parameters: serde_json::json!({}),
                ready: true,
                missing_secrets: vec![],
            },
        ];
        let defs = build_tool_definitions(&entries);
        let skill_names: Vec<&str> = defs
            .iter()
            .filter(|d| d.name != "store_provider_key" && d.name != "store_vault_secret")
            .map(|d| d.name.as_str())
            .collect();
        assert!(!skill_names.contains(&"web_search"));
        assert!(skill_names.contains(&"shell_execute"));
    }
}
