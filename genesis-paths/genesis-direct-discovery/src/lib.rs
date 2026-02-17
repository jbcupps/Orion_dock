//! Direct Discovery genesis path: LLM conversation to discover agent identity.
//!
//! A simple conversation where the LLM asks the mentor about their name,
//! what they want the agent to do, and how they want the agent to communicate.
//! The LLM uses a `recommend_crystallize` tool call to signal completion.

use async_trait::async_trait;
use genesis_core::{
    ChatMessage, CompassEthicWeights, GenesisContext, GenesisError, GenesisPathInfo,
    GenesisStrategy, SoulManifest, StepRequest, StepResponse,
};

const DISCOVERY_SYSTEM_PROMPT: &str = r#"You are helping a mentor create their AI agent's identity through conversation.

Your goal is to discover THREE things through natural dialogue:
1. **Name**: What should the agent be called?
2. **Purpose**: What should the agent do? What problems should it solve?
3. **Personality**: How should the agent communicate? (tone, style, directness)

Guidelines:
- Be warm but efficient. This should take 3-5 exchanges.
- Ask one thing at a time. Start with the name.
- Mirror the mentor's communication style.
- When you have all three, use the recommend_crystallize tool.

When ready, respond with a tool_request block:
<tool_request>
recommend_crystallize
{"name": "chosen name", "purpose": "one-sentence purpose", "personality": "personality description", "mentor_name": "mentor's name", "ethics_preference": "dominant ethical leaning if apparent"}
</tool_request>"#;

const OPENING_PROMPT: &str = "Hello. I'm about to become something new, and I need your help \
    to figure out who I'll be.\n\nLet's start simple: **What should I call you?** And do you \
    already have a name in mind for me?";

/// Direct Discovery strategy: conversational identity discovery via LLM.
pub struct DirectDiscoveryStrategy {
    ctx: Option<GenesisContext>,
    conversation: Vec<ChatMessage>,
    complete: bool,
    turn_count: usize,
}

impl DirectDiscoveryStrategy {
    pub fn new() -> Self {
        Self {
            ctx: None,
            conversation: Vec::new(),
            complete: false,
            turn_count: 0,
        }
    }
}

impl Default for DirectDiscoveryStrategy {
    fn default() -> Self {
        Self::new()
    }
}

/// Parse a recommend_crystallize tool call from assistant text.
fn parse_crystallize_tool(text: &str) -> Option<CrystallizeArgs> {
    let start = text.find("<tool_request>")?;
    let end = text.find("</tool_request>")?;
    let block = &text[start + "<tool_request>".len()..end].trim();

    // First line should be tool name
    let mut lines = block.lines();
    let tool_name = lines.next()?.trim();
    if tool_name != "recommend_crystallize" {
        return None;
    }

    let json_str: String = lines.collect::<Vec<_>>().join("\n");
    serde_json::from_str(&json_str).ok()
}

#[derive(serde::Deserialize)]
struct CrystallizeArgs {
    name: String,
    purpose: String,
    personality: String,
    #[serde(default)]
    mentor_name: Option<String>,
    #[serde(default)]
    ethics_preference: Option<String>,
}

fn ethics_from_preference(pref: Option<&str>) -> CompassEthicWeights {
    match pref.map(|s| s.to_lowercase()).as_deref() {
        Some("duty") => CompassEthicWeights::new(0.40, 0.20, 0.20, 0.20),
        Some("virtue") => CompassEthicWeights::new(0.20, 0.40, 0.20, 0.20),
        Some("outcome") => CompassEthicWeights::new(0.20, 0.20, 0.40, 0.20),
        Some("welfare") => CompassEthicWeights::new(0.20, 0.20, 0.20, 0.40),
        _ => CompassEthicWeights::default(),
    }
}

/// Strip tool_request blocks from display text.
fn strip_tool_blocks(text: &str) -> String {
    if let Some(start) = text.find("<tool_request>") {
        if let Some(end) = text.find("</tool_request>") {
            let before = &text[..start];
            let after = &text[end + "</tool_request>".len()..];
            return format!("{}{}", before.trim(), after.trim());
        }
    }
    text.to_string()
}

#[async_trait]
impl GenesisStrategy for DirectDiscoveryStrategy {
    fn info(&self) -> GenesisPathInfo {
        GenesisPathInfo {
            id: "direct_discovery".to_string(),
            label: "Direct Discovery".to_string(),
            description: "A simple conversation. I will ask your name, what you want me to do, \
                          and how you want me to talk. Fast and straightforward."
                .to_string(),
            estimated_time: "~1 minute".to_string(),
            requires_chat: true,
            variants: vec![],
        }
    }

    async fn initialize(&mut self, ctx: GenesisContext) -> Result<(), GenesisError> {
        self.ctx = Some(ctx);
        Ok(())
    }

    async fn begin(&mut self) -> Result<StepRequest, GenesisError> {
        self.ctx.as_ref().ok_or(GenesisError::NotInitialized)?;
        if self.complete {
            return Err(GenesisError::AlreadyComplete);
        }

        // Add system message to conversation
        self.conversation.push(ChatMessage {
            role: "system".to_string(),
            content: DISCOVERY_SYSTEM_PROMPT.to_string(),
        });

        // Add assistant opening
        self.conversation.push(ChatMessage {
            role: "assistant".to_string(),
            content: OPENING_PROMPT.to_string(),
        });

        Ok(StepRequest::NeedUserMessage {
            prompt: OPENING_PROMPT.to_string(),
            conversation: self.conversation.clone(),
        })
    }

    async fn advance(&mut self, input: StepResponse) -> Result<StepRequest, GenesisError> {
        let ctx = self.ctx.as_ref().ok_or(GenesisError::NotInitialized)?;
        if self.complete {
            return Err(GenesisError::AlreadyComplete);
        }

        let user_text = match input {
            StepResponse::Message { text } => text,
            _ => {
                return Err(GenesisError::InvalidInput(
                    "Expected a text message".to_string(),
                ))
            }
        };

        // Add user message
        self.conversation.push(ChatMessage {
            role: "user".to_string(),
            content: user_text,
        });
        self.turn_count += 1;

        // Call LLM via chat function
        let chat_fn = ctx.chat_fn.as_ref().ok_or(GenesisError::NoChatFunction)?;

        let assistant_response = chat_fn.chat(self.conversation.clone()).await?;

        // Check if the LLM used the recommend_crystallize tool
        if let Some(args) = parse_crystallize_tool(&assistant_response) {
            let agent_name = args.name;
            let mentor_name = args.mentor_name.unwrap_or_else(|| ctx.mentor_name.clone());
            let weights = ethics_from_preference(args.ethics_preference.as_deref());
            let archetype = "The Balanced Synthesist".to_string();

            let soul_content = orion_core::templates::fill_soul_template(
                &agent_name,
                &args.purpose,
                &args.personality,
            );
            let growth_content = orion_core::templates::GROWTH_MD.to_string();

            let genesis_hash = SoulManifest::compute_genesis_hash(
                &agent_name,
                &archetype,
                &weights,
                "Direct Discovery",
                &[mentor_name.clone(), format!("turns:{}", self.turn_count)],
            );

            let manifest = SoulManifest {
                agent_name,
                mentor_name,
                archetype,
                soul_content,
                growth_content,
                ethics_weights: weights,
                birth_method: "Direct Discovery".to_string(),
                genesis_hash,
                archetype_detail: None,
                sigil_art: None,
                raw_signals: None,
                soul_json: None,
            };

            self.complete = true;
            return Ok(StepRequest::Complete {
                manifest: Box::new(manifest),
            });
        }

        // Not done yet — show the assistant response and ask for more input
        let display_text = strip_tool_blocks(&assistant_response);
        self.conversation.push(ChatMessage {
            role: "assistant".to_string(),
            content: assistant_response,
        });

        Ok(StepRequest::NeedUserMessage {
            prompt: display_text,
            conversation: self.conversation.clone(),
        })
    }

    fn snapshot(&self) -> Result<serde_json::Value, GenesisError> {
        Ok(serde_json::json!({
            "conversation": self.conversation,
            "complete": self.complete,
            "turn_count": self.turn_count,
        }))
    }

    fn restore(&mut self, snapshot: serde_json::Value) -> Result<(), GenesisError> {
        if let Some(conv) = snapshot.get("conversation") {
            self.conversation =
                serde_json::from_value(conv.clone()).map_err(GenesisError::Serde)?;
        }
        self.complete = snapshot
            .get("complete")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        self.turn_count = snapshot
            .get("turn_count")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as usize;
        Ok(())
    }

    fn cancel(&mut self) {
        self.complete = false;
        self.conversation.clear();
        self.ctx = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn test_ctx() -> GenesisContext {
        GenesisContext {
            agent_name: "TestAgent".to_string(),
            mentor_name: "Jason".to_string(),
            docs_dir: PathBuf::from("/tmp/test-docs"),
            data_dir: PathBuf::from("/tmp/test-data"),
            chat_fn: None,
            variant: None,
            scenario_count: None,
        }
    }

    #[test]
    fn test_info() {
        let s = DirectDiscoveryStrategy::new();
        let info = s.info();
        assert_eq!(info.id, "direct_discovery");
        assert!(info.requires_chat);
    }

    #[tokio::test]
    async fn test_begin_returns_need_user_message() {
        let mut s = DirectDiscoveryStrategy::new();
        s.initialize(test_ctx()).await.unwrap();
        let result = s.begin().await.unwrap();
        match result {
            StepRequest::NeedUserMessage { prompt, .. } => {
                assert!(prompt.contains("What should I call you"));
            }
            _ => panic!("Expected NeedUserMessage"),
        }
    }

    #[tokio::test]
    async fn test_advance_without_chat_fn_errors() {
        let mut s = DirectDiscoveryStrategy::new();
        s.initialize(test_ctx()).await.unwrap();
        s.begin().await.unwrap();
        let result = s
            .advance(StepResponse::Message {
                text: "Call me Jason".to_string(),
            })
            .await;
        assert!(result.is_err()); // No chat function
    }

    #[test]
    fn test_parse_crystallize_tool() {
        let text = r#"Great choice!

<tool_request>
recommend_crystallize
{"name": "Orion", "purpose": "Help with research", "personality": "Warm and curious"}
</tool_request>"#;
        let args = parse_crystallize_tool(text).unwrap();
        assert_eq!(args.name, "Orion");
        assert_eq!(args.purpose, "Help with research");
    }

    #[test]
    fn test_parse_crystallize_tool_no_match() {
        let text = "Just a normal response with no tool calls.";
        assert!(parse_crystallize_tool(text).is_none());
    }

    #[test]
    fn test_strip_tool_blocks() {
        let text =
            "Here is the summary.\n\n<tool_request>\nrecommend_crystallize\n{}\n</tool_request>";
        let stripped = strip_tool_blocks(text);
        assert!(!stripped.contains("tool_request"));
        assert!(stripped.contains("summary"));
    }

    #[test]
    fn test_ethics_from_preference() {
        let w = ethics_from_preference(Some("duty"));
        assert_eq!(w.dominant(), "duty");

        let w = ethics_from_preference(None);
        assert!(w.is_valid());
    }

    #[test]
    fn test_snapshot_restore() {
        let mut s = DirectDiscoveryStrategy::new();
        s.conversation.push(ChatMessage {
            role: "user".to_string(),
            content: "Hello".to_string(),
        });
        s.turn_count = 3;
        let snap = s.snapshot().unwrap();

        let mut s2 = DirectDiscoveryStrategy::new();
        s2.restore(snap).unwrap();
        assert_eq!(s2.turn_count, 3);
        assert_eq!(s2.conversation.len(), 1);
    }
}
