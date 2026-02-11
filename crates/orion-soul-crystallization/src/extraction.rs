//! Extract (name, purpose, personality) from crystallization conversation transcript via LLM.
//! Caller invokes the LLM with the prompt; this module builds the prompt and parses the response.

use crate::models::ConversationTurn;
use serde::{Deserialize, Serialize};

/// Identity extracted from the crystallization conversation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractedIdentity {
    /// Name the mentor wants for the agent.
    pub name: String,
    /// One-sentence purpose or mission.
    pub purpose: String,
    /// Short personality/tone description (e.g. for soul.md).
    pub personality: String,
}

/// Build the system/user prompt for the extraction LLM call.
/// The LLM should respond with a single JSON object matching ExtractedIdentity.
pub fn build_extraction_prompt(conversation: &[ConversationTurn]) -> String {
    let transcript: String = conversation
        .iter()
        .map(|t| format!("{}: {}", t.role, t.content))
        .collect::<Vec<_>>()
        .join("\n\n");

    format!(
        r#"You are extracting the mentor's chosen identity for their AI agent from a crystallization conversation.

Conversation transcript:

---
{transcript}
---

From this conversation, extract:
1. **name**: The name the mentor wants to give the agent (or a default like "Orion" if not specified).
2. **purpose**: One clear sentence describing what the agent is for (the mentor's goal or mission for the agent).
3. **personality**: A short description of how the agent should communicate (tone, style, values reflected in the conversation).

Respond with ONLY a JSON object, no markdown or explanation, with exactly these keys: "name", "purpose", "personality".
Example: {{"name":"Orion","purpose":"To assist the mentor with research and writing.","personality":"Warm, precise, and curious; prefers clarity over brevity."}}"#
    )
}

/// Parse the LLM's extraction response into ExtractedIdentity.
/// Strips markdown code fences if present.
pub fn parse_extraction_response(response: &str) -> Result<ExtractedIdentity, ExtractError> {
    let trimmed = response.trim();
    let json_str = if let Some(s) = trimmed.strip_prefix("```json") {
        s.trim().strip_suffix("```").unwrap_or(s.trim())
    } else if let Some(s) = trimmed.strip_prefix("```") {
        s.trim().strip_suffix("```").unwrap_or(s.trim())
    } else {
        trimmed
    };

    serde_json::from_str(json_str).map_err(ExtractError::Parse)
}

#[derive(Debug, thiserror::Error)]
pub enum ExtractError {
    #[error("Failed to parse extraction JSON: {0}")]
    Parse(serde_json::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_extraction_response_raw_json() {
        let s = r#"{"name":"Orion","purpose":"To help.","personality":"Friendly."}"#;
        let id = parse_extraction_response(s).unwrap();
        assert_eq!(id.name, "Orion");
        assert_eq!(id.purpose, "To help.");
        assert_eq!(id.personality, "Friendly.");
    }

    #[test]
    fn test_parse_extraction_response_with_code_fence() {
        let s = r#"```json
{"name":"Agent","purpose":"Research.","personality":"Curious."}
```"#;
        let id = parse_extraction_response(s).unwrap();
        assert_eq!(id.name, "Agent");
    }

    #[test]
    fn test_build_extraction_prompt_includes_transcript() {
        let conv = vec![
            ConversationTurn {
                role: "user".to_string(),
                content: "Call me Alex.".to_string(),
                signals: vec![],
            },
            ConversationTurn {
                role: "assistant".to_string(),
                content: "Noted.".to_string(),
                signals: vec![],
            },
        ];
        let prompt = build_extraction_prompt(&conv);
        assert!(prompt.contains("user: Call me Alex."));
        assert!(prompt.contains("assistant: Noted."));
        assert!(prompt.contains("\"name\""));
        assert!(prompt.contains("\"purpose\""));
        assert!(prompt.contains("\"personality\""));
    }
}
