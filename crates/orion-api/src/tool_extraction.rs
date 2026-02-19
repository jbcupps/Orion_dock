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

    defs.push(ToolDefinition {
        name: "register_email_account".to_string(),
        description:
            "Register or update an email account configuration and store password in the vault."
                .to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "id": { "type": "string", "description": "Optional stable account id (e.g. proton)" },
                "provider": { "type": "string", "enum": ["gmail", "outlook", "proton", "fastmail", "imap_fallback"] },
                "auth_type": { "type": "string", "enum": ["app_password", "smtp_token", "o_auth2"], "description": "Defaults to app_password" },
                "address": { "type": "string", "description": "Mailbox address (display/from address)" },
                "username": { "type": "string", "description": "Optional login username. Defaults to address." },
                "password": { "type": "string", "description": "Optional password/token (stored as email:{id}:password)." },
                "imap_host": { "type": "string" },
                "imap_port": { "type": "integer" },
                "smtp_host": { "type": "string" },
                "smtp_port": { "type": "integer" },
                "security": { "type": "string", "enum": ["auto", "starttls", "implicit", "none"], "description": "Transport security preference for inferred TLS modes." }
            },
            "required": ["provider", "address"]
        }),
    });

    defs
}

/// Build the `manage_hive` tool definition (shared between operational and agentic modes).
fn manage_hive_tool_def() -> ToolDefinition {
    ToolDefinition {
        name: "manage_hive".to_string(),
        description:
            "List, create, check status of, or delegate tasks to sibling agents in the Hive."
                .to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "action": { "type": "string", "enum": ["list", "create", "status", "delegate"], "description": "Action to perform" },
                "agent_name": { "type": "string", "description": "Name for the new agent (create action)" },
                "agent_id": { "type": "string", "description": "Target agent UUID (status/delegate actions)" },
                "goal": { "type": "string", "description": "Goal to delegate (delegate action)" },
                "router_mode": { "type": "string", "enum": ["auto", "think_hard", "think_harder"], "description": "Router mode for delegated task (default: auto)" }
            },
            "required": ["action"]
        }),
    }
}

/// Build tool definitions for operational chat mode: skill tools + lightweight synthetic tools.
pub fn build_operational_tool_definitions(skill_tools: &[SkillToolEntry]) -> Vec<ToolDefinition> {
    let mut defs = build_tool_definitions(skill_tools);
    defs.push(ToolDefinition {
        name: "launch_agentic_task".to_string(),
        description: "Start an autonomous background task. Use this when work needs 3+ dependent steps, or when you must configure + verify (e.g. store credentials, register integration, then confirm connectivity), or when failure handling needs retries/diagnosis. The mentor can monitor progress in the Agent view."
            .to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "goal": { "type": "string", "description": "Primary autonomous task goal" },
                "context": { "type": "string", "description": "Optional context to include in the goal" }
            },
            "required": ["goal"]
        }),
    });
    defs.push(manage_hive_tool_def());
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

    defs.push(ToolDefinition {
        name: "manage_proxy".to_string(),
        description:
            "View or modify outbound proxy settings and logs. Mutations require mentor_approved=true."
                .to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "action": { "type": "string", "enum": ["status", "get_allowlist", "update_allowlist", "get_logs"] },
                "domains": { "type": "array", "items": { "type": "string" }, "description": "Domain list for update_allowlist (one domain per entry)." },
                "lines": { "type": "integer", "description": "Optional number of lines for get_logs (default 80)." },
                "mentor_approved": { "type": "boolean", "description": "Required true for update_allowlist." }
            },
            "required": ["action"]
        }),
    });

    defs.push(manage_hive_tool_def());

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
        assert!(names.contains(&"register_email_account"));
    }

    #[test]
    fn test_build_operational_tool_definitions_includes_launch_agentic_task() {
        let defs = build_operational_tool_definitions(&[]);
        let names: Vec<&str> = defs.iter().map(|d| d.name.as_str()).collect();
        assert!(names.contains(&"launch_agentic_task"));
    }

    #[test]
    fn test_build_agentic_tool_definitions_includes_synthetic() {
        let defs = build_agentic_tool_definitions(&[]);
        let names: Vec<&str> = defs.iter().map(|d| d.name.as_str()).collect();
        assert!(names.contains(&"task_complete"));
        assert!(names.contains(&"ask_mentor"));
        assert!(names.contains(&"register_mcp_skill"));
        assert!(names.contains(&"manage_proxy"));
        assert!(names.contains(&"store_provider_key"));
        assert!(names.contains(&"manage_hive"));
    }

    #[test]
    fn test_build_operational_tool_definitions_includes_manage_hive() {
        let defs = build_operational_tool_definitions(&[]);
        let names: Vec<&str> = defs.iter().map(|d| d.name.as_str()).collect();
        assert!(names.contains(&"launch_agentic_task"));
        assert!(names.contains(&"manage_hive"));
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
