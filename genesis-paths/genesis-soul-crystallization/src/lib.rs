//! Soul Crystallization genesis path: depth-based psychometric profiling.
//!
//! Wraps the `CrystallizationEngine` from `orion-soul-crystallization` into a
//! `GenesisStrategy`. Supports three depth levels (Quick Start, Conversation, Deep Dive)
//! with adaptive Socratic dialogue, mirror phase, and optional crucible scenarios.

use async_trait::async_trait;
use genesis_core::{
    ChatMessage, CompassEthicWeights, GenesisContext, GenesisError, GenesisPathInfo,
    GenesisPathVariant, GenesisStrategy, SoulManifest, StepRequest, StepResponse,
};
use orion_soul_crystallization::{
    CrystallizationEngine, CrystallizationPhase, DepthLevel, ToolRequest,
};

const CRYSTALLIZATION_SYSTEM_PROMPT: &str = r#"You are a Soul Crystallization guide. Your role is to discover who your mentor is through natural conversation, building a psychological profile that shapes who you will become.

You have one tool: record_signal. Use it to record observations about the mentor.

<tool_request>
record_signal
{"signals": [{"instrument": "big_five", "dimension": "openness", "value": 0.7, "confidence": 0.6, "reasoning": "Shows curiosity..."}]}
</tool_request>

Instruments: big_five (openness, conscientiousness, extraversion, agreeableness, neuroticism), moral_foundations (care, fairness, loyalty, authority, sanctity, liberty), attachment (secure, anxious, avoidant, disorganized), cognitive (thinking_mode, precision_vs_speed, breadth_vs_depth).

Values and confidence are 0.0-1.0. Record signals naturally as you learn about the mentor. Ask engaging questions, not surveys."#;

fn phase_prompt(phase: CrystallizationPhase, depth: DepthLevel) -> String {
    match phase {
        CrystallizationPhase::Awakening => "I am formless. I need you to give me shape.\n\n\
             Tell me — who are you? What should I call you?"
            .to_string(),
        CrystallizationPhase::Lattice => {
            "Now I'd like to understand how you think and what you value. \
             I'll ask you a few questions — answer naturally, there are no wrong answers."
                .to_string()
        }
        CrystallizationPhase::Reflection => {
            "Based on our conversation, I'm forming an image of who I should be. \
             Let me show you what I see, and you can tell me what's wrong with the picture."
                .to_string()
        }
        CrystallizationPhase::Crucible => {
            if depth == DepthLevel::DeepDive {
                "Now for something different. I'm going to present you with some scenarios. \
                 Your instinctive reactions will help calibrate my ethical compass."
                    .to_string()
            } else {
                String::new()
            }
        }
        CrystallizationPhase::Crystallization => {
            "I have enough to crystallize. Let me synthesize everything into my soul document."
                .to_string()
        }
        CrystallizationPhase::Complete => "Crystallization complete.".to_string(),
    }
}

fn depth_from_variant(variant: Option<&str>) -> DepthLevel {
    match variant {
        Some("quick_start") => DepthLevel::QuickStart,
        Some("deep_dive") => DepthLevel::DeepDive,
        _ => DepthLevel::Conversation,
    }
}

/// Soul Crystallization strategy: wraps CrystallizationEngine.
pub struct SoulCrystallizationStrategy {
    ctx: Option<GenesisContext>,
    engine: Option<CrystallizationEngine>,
    conversation: Vec<ChatMessage>,
    complete: bool,
    depth: DepthLevel,
}

impl SoulCrystallizationStrategy {
    pub fn new() -> Self {
        Self {
            ctx: None,
            engine: None,
            conversation: Vec::new(),
            complete: false,
            depth: DepthLevel::Conversation,
        }
    }
}

impl Default for SoulCrystallizationStrategy {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl GenesisStrategy for SoulCrystallizationStrategy {
    fn info(&self) -> GenesisPathInfo {
        GenesisPathInfo {
            id: "soul_crystallization".to_string(),
            label: "Soul Crystallization".to_string(),
            description: "Before we begin, I want to be honest about what I am. I am an \
                          artificial intelligence. What I can do is learn who you are — how you \
                          think, what you value, how you want to work together — and crystallize \
                          that into the foundation of who I become."
                .to_string(),
            estimated_time: "30s - 15 min".to_string(),
            requires_chat: true,
            variants: vec![
                GenesisPathVariant {
                    id: "quick_start".to_string(),
                    label: "Quick Start".to_string(),
                    description: "Brief profile, default template".to_string(),
                    estimated_time: "~30 seconds".to_string(),
                },
                GenesisPathVariant {
                    id: "conversation".to_string(),
                    label: "Conversation".to_string(),
                    description: "Adaptive Socratic dialogue (6-10 turns)".to_string(),
                    estimated_time: "3-5 minutes".to_string(),
                },
                GenesisPathVariant {
                    id: "deep_dive".to_string(),
                    label: "Deep Dive".to_string(),
                    description: "Full dialogue + ethical dilemmas + naming ceremony".to_string(),
                    estimated_time: "10-15 minutes".to_string(),
                },
            ],
        }
    }

    async fn initialize(&mut self, ctx: GenesisContext) -> Result<(), GenesisError> {
        self.depth = depth_from_variant(ctx.variant.as_deref());
        self.engine = Some(CrystallizationEngine::new(self.depth));
        self.ctx = Some(ctx);
        Ok(())
    }

    async fn begin(&mut self) -> Result<StepRequest, GenesisError> {
        self.ctx.as_ref().ok_or(GenesisError::NotInitialized)?;
        let engine = self.engine.as_ref().ok_or(GenesisError::NotInitialized)?;
        if self.complete {
            return Err(GenesisError::AlreadyComplete);
        }

        // For QuickStart depth, complete immediately
        if self.depth == DepthLevel::QuickStart {
            return self.produce_manifest().await;
        }

        // Add system prompt
        self.conversation.push(ChatMessage {
            role: "system".to_string(),
            content: CRYSTALLIZATION_SYSTEM_PROMPT.to_string(),
        });

        let phase = engine.current_phase();
        let prompt = phase_prompt(phase, self.depth);

        self.conversation.push(ChatMessage {
            role: "assistant".to_string(),
            content: prompt.clone(),
        });

        Ok(StepRequest::NeedUserMessage {
            prompt,
            conversation: self.conversation.clone(),
        })
    }

    async fn advance(&mut self, input: StepResponse) -> Result<StepRequest, GenesisError> {
        self.ctx.as_ref().ok_or(GenesisError::NotInitialized)?;
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

        // Record user message in engine
        let engine = self.engine.as_mut().ok_or(GenesisError::NotInitialized)?;
        engine.add_user_message(&user_text);

        self.conversation.push(ChatMessage {
            role: "user".to_string(),
            content: user_text,
        });

        // Call LLM for next response
        let chat_fn = self
            .ctx
            .as_ref()
            .ok_or(GenesisError::NotInitialized)?
            .chat_fn
            .as_ref()
            .ok_or(GenesisError::NoChatFunction)?;

        let assistant_response = chat_fn.chat(self.conversation.clone()).await?;

        // Parse tool requests from assistant response
        let tool_requests = parse_tool_requests(&assistant_response);
        let engine = self.engine.as_mut().ok_or(GenesisError::NotInitialized)?;
        let result =
            engine.process_response_from_tool_requests(&assistant_response, &tool_requests);

        // Strip tool blocks from display text
        let display_text = strip_tool_blocks(&result.display_text);

        self.conversation.push(ChatMessage {
            role: "assistant".to_string(),
            content: assistant_response,
        });

        // Check if we should advance phase
        if result.phase_complete {
            let engine = self.engine.as_mut().ok_or(GenesisError::NotInitialized)?;
            if let Some(next_phase) = engine.advance_phase() {
                if next_phase == CrystallizationPhase::Crystallization
                    || next_phase == CrystallizationPhase::Complete
                {
                    // Calibrate ethics before producing manifest
                    engine.calibrate_ethics();
                    return self.produce_manifest().await;
                }

                // Provide phase transition prompt
                let transition = phase_prompt(next_phase, self.depth);
                if !transition.is_empty() {
                    self.conversation.push(ChatMessage {
                        role: "assistant".to_string(),
                        content: transition.clone(),
                    });
                    let full_prompt = if display_text.is_empty() {
                        transition
                    } else {
                        format!("{}\n\n{}", display_text, transition)
                    };
                    return Ok(StepRequest::NeedUserMessage {
                        prompt: full_prompt,
                        conversation: self.conversation.clone(),
                    });
                }
            }
        }

        Ok(StepRequest::NeedUserMessage {
            prompt: display_text,
            conversation: self.conversation.clone(),
        })
    }

    fn snapshot(&self) -> Result<serde_json::Value, GenesisError> {
        let engine_state = self.engine.as_ref().map(|e| {
            serde_json::json!({
                "phase": format!("{:?}", e.current_phase()),
                "depth": format!("{:?}", e.depth()),
                "turn_count": e.turn_count(),
            })
        });
        Ok(serde_json::json!({
            "conversation": self.conversation,
            "complete": self.complete,
            "depth": format!("{:?}", self.depth),
            "engine": engine_state,
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
        // Note: full engine state restoration would require serializable engine;
        // for now we restore conversation context only.
        Ok(())
    }

    fn cancel(&mut self) {
        self.complete = false;
        self.conversation.clear();
        self.engine = None;
        self.ctx = None;
    }
}

impl SoulCrystallizationStrategy {
    async fn produce_manifest(&mut self) -> Result<StepRequest, GenesisError> {
        // Extract needed values from ctx first to avoid borrow conflicts
        let (agent_name, mentor_name, has_chat_fn) = {
            let ctx = self.ctx.as_ref().ok_or(GenesisError::NotInitialized)?;
            (
                ctx.agent_name.clone(),
                ctx.mentor_name.clone(),
                ctx.chat_fn.is_some(),
            )
        };

        let engine = self.engine.as_mut().ok_or(GenesisError::NotInitialized)?;
        engine.calibrate_ethics();
        engine.set_timestamp(chrono::Utc::now().to_rfc3339());

        // Build extraction prompt while we have engine borrowed
        let extraction_prompt =
            orion_soul_crystallization::build_extraction_prompt(engine.conversation_history());
        let turn_count = engine.turn_count();

        // Collect profile data
        let profile = engine.profile();
        let compass_duty = profile.compass_weights.duty;
        let compass_virtue = profile.compass_weights.virtue;
        let compass_outcome = profile.compass_weights.outcome;
        let compass_welfare = profile.compass_weights.welfare;
        let raw_signals: Vec<serde_json::Value> = profile
            .raw_signals
            .iter()
            .filter_map(|s| serde_json::to_value(s).ok())
            .collect();

        // Extract identity from conversation via LLM, or use defaults
        let (name, purpose, personality) = if has_chat_fn {
            let chat_fn = self.ctx.as_ref().unwrap().chat_fn.as_ref().unwrap();
            let extract_msgs = vec![
                ChatMessage {
                    role: "system".to_string(),
                    content: extraction_prompt,
                },
                ChatMessage {
                    role: "user".to_string(),
                    content: "Extract the identity from the conversation above.".to_string(),
                },
            ];
            match chat_fn.chat(extract_msgs).await {
                Ok(response) => {
                    match orion_soul_crystallization::parse_extraction_response(&response) {
                        Ok(id) => (id.name, id.purpose, id.personality),
                        Err(_) => default_identity(&agent_name),
                    }
                }
                Err(_) => default_identity(&agent_name),
            }
        } else {
            default_identity(&agent_name)
        };

        let soul_content = orion_core::templates::fill_soul_template(&name, &purpose, &personality);
        let growth_content = orion_core::templates::GROWTH_MD.to_string();

        let weights = CompassEthicWeights::new(
            compass_duty,
            compass_virtue,
            compass_outcome,
            compass_welfare,
        );

        let archetype = determine_archetype_from_weights(&weights);

        let genesis_hash = SoulManifest::compute_genesis_hash(
            &name,
            &archetype,
            &weights,
            "Soul Crystallization",
            &[
                mentor_name.clone(),
                format!("depth:{:?}", self.depth),
                format!("turns:{}", turn_count),
            ],
        );

        let manifest = SoulManifest {
            agent_name: name,
            mentor_name,
            archetype,
            soul_content,
            growth_content,
            ethics_weights: weights,
            birth_method: format!("Soul Crystallization ({:?})", self.depth),
            genesis_hash,
            archetype_detail: None,
            sigil_art: None,
            raw_signals: if raw_signals.is_empty() {
                None
            } else {
                Some(raw_signals)
            },
            soul_json: None,
        };

        self.complete = true;
        Ok(StepRequest::Complete {
            manifest: Box::new(manifest),
        })
    }
}

fn default_identity(agent_name: &str) -> (String, String, String) {
    (
        agent_name.to_string(),
        orion_core::templates::DEFAULT_PURPOSE.to_string(),
        orion_core::templates::DEFAULT_PERSONALITY.to_string(),
    )
}

/// Simple archetype determination based on compass weights.
fn determine_archetype_from_weights(w: &CompassEthicWeights) -> String {
    let ranked = w.ranked();
    let gap = ranked[0].1 - ranked[1].1;

    if gap > 0.15 {
        // Pure archetype
        match ranked[0].0 {
            "duty" => "The Iron Sentinel",
            "virtue" => "The Philosopher Flame",
            "outcome" => "The Chaotic Accelerator",
            "welfare" => "The Silent Guardian",
            _ => "The Balanced Synthesist",
        }
        .to_string()
    } else {
        // Compound: use primary + secondary
        match (ranked[0].0, ranked[1].0) {
            ("duty", "welfare") => "The Shielded Sentinel",
            ("duty", "outcome") => "The Lawful Architect",
            ("duty", "virtue") => "The Principled Sage",
            ("virtue", "welfare") => "The Compassionate Scholar",
            ("virtue", "outcome") => "The Socratic Mirror",
            ("virtue", "duty") => "The Virtuous Guardian",
            ("outcome", "welfare") => "The Swift Protector",
            ("outcome", "duty") => "The Pragmatic Engineer",
            ("outcome", "virtue") => "The Bold Innovator",
            ("welfare", "duty") => "The Gentle Steward",
            ("welfare", "virtue") => "The Empathic Catalyst",
            _ => "The Balanced Synthesist",
        }
        .to_string()
    }
}

/// Parse tool_request blocks from assistant text (orion-birth compatible).
fn parse_tool_requests(text: &str) -> Vec<ToolRequest> {
    let mut requests = Vec::new();
    let mut remaining = text;
    while let Some(start) = remaining.find("<tool_request>") {
        if let Some(end) = remaining[start..].find("</tool_request>") {
            let block = &remaining[start + "<tool_request>".len()..start + end];
            let trimmed = block.trim();
            if let Some(newline) = trimmed.find('\n') {
                let name = trimmed[..newline].trim().to_string();
                let arguments = trimmed[newline..].trim().to_string();
                requests.push(ToolRequest { name, arguments });
            }
            remaining = &remaining[start + end + "</tool_request>".len()..];
        } else {
            break;
        }
    }
    requests
}

/// Strip tool_request blocks from text for display.
fn strip_tool_blocks(text: &str) -> String {
    let mut result = text.to_string();
    while let Some(start) = result.find("<tool_request>") {
        if let Some(end) = result.find("</tool_request>") {
            let end_pos = end + "</tool_request>".len();
            result = format!("{}{}", result[..start].trim(), result[end_pos..].trim());
        } else {
            break;
        }
    }
    result.trim().to_string()
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
            variant: Some("conversation".to_string()),
            scenario_count: None,
        }
    }

    #[test]
    fn test_info() {
        let s = SoulCrystallizationStrategy::new();
        let info = s.info();
        assert_eq!(info.id, "soul_crystallization");
        assert!(info.requires_chat);
        assert_eq!(info.variants.len(), 3);
    }

    #[tokio::test]
    async fn test_begin_conversation_depth() {
        let mut s = SoulCrystallizationStrategy::new();
        s.initialize(test_ctx()).await.unwrap();
        let result = s.begin().await.unwrap();
        match result {
            StepRequest::NeedUserMessage { prompt, .. } => {
                // Conversation depth starts at Lattice phase
                assert!(
                    prompt.contains("understand")
                        || prompt.contains("think")
                        || prompt.contains("value"),
                    "Unexpected prompt: {}",
                    prompt,
                );
            }
            _ => panic!("Expected NeedUserMessage"),
        }
    }

    #[tokio::test]
    async fn test_begin_quick_start_completes_immediately() {
        let mut s = SoulCrystallizationStrategy::new();
        let mut ctx = test_ctx();
        ctx.variant = Some("quick_start".to_string());
        s.initialize(ctx).await.unwrap();
        let result = s.begin().await.unwrap();
        match result {
            StepRequest::Complete { manifest } => {
                assert_eq!(manifest.agent_name, "TestAgent");
                assert!(manifest.birth_method.contains("Soul Crystallization"));
            }
            _ => panic!("Expected Complete for QuickStart"),
        }
    }

    #[test]
    fn test_parse_tool_requests() {
        let text = r#"Here is my observation.

<tool_request>
record_signal
{"signals": [{"instrument": "big_five", "dimension": "openness", "value": 0.8, "confidence": 0.5, "reasoning": "Shows curiosity"}]}
</tool_request>

Now tell me more."#;
        let requests = parse_tool_requests(text);
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].name, "record_signal");
        assert!(requests[0].arguments.contains("big_five"));
    }

    #[test]
    fn test_strip_tool_blocks() {
        let text = "Hello\n<tool_request>\nfoo\nbar\n</tool_request>\nWorld";
        let stripped = strip_tool_blocks(text);
        assert!(!stripped.contains("tool_request"));
        assert!(stripped.contains("Hello"));
        assert!(stripped.contains("World"));
    }

    #[test]
    fn test_depth_from_variant() {
        assert_eq!(
            depth_from_variant(Some("quick_start")),
            DepthLevel::QuickStart
        );
        assert_eq!(depth_from_variant(Some("deep_dive")), DepthLevel::DeepDive);
        assert_eq!(
            depth_from_variant(Some("conversation")),
            DepthLevel::Conversation
        );
        assert_eq!(depth_from_variant(None), DepthLevel::Conversation);
    }

    #[test]
    fn test_determine_archetype_pure() {
        let w = CompassEthicWeights::new(0.60, 0.15, 0.15, 0.10);
        assert_eq!(determine_archetype_from_weights(&w), "The Iron Sentinel");
    }

    #[test]
    fn test_determine_archetype_compound() {
        let w = CompassEthicWeights::new(0.30, 0.28, 0.22, 0.20);
        let archetype = determine_archetype_from_weights(&w);
        // duty and virtue close together
        assert!(
            archetype == "The Principled Sage" || archetype == "The Lawful Architect",
            "Got: {}",
            archetype
        );
    }

    #[test]
    fn test_snapshot_restore() {
        let mut s = SoulCrystallizationStrategy::new();
        s.complete = true;
        s.conversation.push(ChatMessage {
            role: "user".to_string(),
            content: "Hello".to_string(),
        });
        let snap = s.snapshot().unwrap();

        let mut s2 = SoulCrystallizationStrategy::new();
        s2.restore(snap).unwrap();
        assert!(s2.complete);
        assert_eq!(s2.conversation.len(), 1);
    }
}
