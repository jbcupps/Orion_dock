use anyhow::Context;
use orion_capabilities::cognitive::{
    AnthropicProvider, CompatibleProvider, CompletionRequest, CompletionResponse, LlmProvider,
    Message, OpenAiCompatibleProvider, OpenAiProvider,
};
use orion_memory::store::EdgeType;
use orion_memory::{Memory, MemoryStore};
use serde::Deserialize;
use std::sync::Arc;
use std::time::Duration;

/// Overall timeout for a full council execution (drafts + critiques + synthesis).
const COUNCIL_TIMEOUT: Duration = Duration::from_secs(90);

/// System prompt for draft nodes.
const DRAFT_SYSTEM: &str = "You are a helpful, thorough assistant. \
Provide a complete, independent answer to the user's query. \
Do not reference other drafts or reviewers.";

/// System prompt for critique nodes.
const CRITIQUE_SYSTEM: &str = "You are a critical reviewer. \
Evaluate the draft against the original query for correctness, completeness, \
and quality. If agent identity context is provided below, also assess whether \
the draft aligns with the agent's stated ethics, instincts, and personality. \
Return STRICT JSON: {\"score\": <number 0-10>, \"rationale\": \"<brief explanation>\"}. \
Only output the JSON object, nothing else.";

/// System prompt for synthesis node.
const SYNTHESIS_SYSTEM: &str = "You synthesize multiple expert drafts into a single best answer. \
Favor content from higher-scored drafts. Resolve conflicts by choosing the most accurate option. \
If agent identity context is provided below, maintain the agent's voice, ethical framework, \
and personality in the final response. Produce a polished, coherent final response.";

/// Artifact payload moving through council DAG nodes.
pub trait Artifact: Send + Sync {
    fn artifact_type(&self) -> &'static str;
    fn content(&self) -> &str;
}

#[derive(Debug, Clone)]
pub struct TextArtifact {
    pub content: String,
}

#[derive(Debug, Clone)]
pub struct CodeArtifact {
    pub content: String,
}

#[derive(Debug, Clone)]
pub struct PlanArtifact {
    pub content: String,
}

impl Artifact for TextArtifact {
    fn artifact_type(&self) -> &'static str {
        "text"
    }

    fn content(&self) -> &str {
        &self.content
    }
}

impl Artifact for CodeArtifact {
    fn artifact_type(&self) -> &'static str {
        "code"
    }

    fn content(&self) -> &str {
        &self.content
    }
}

impl Artifact for PlanArtifact {
    fn artifact_type(&self) -> &'static str {
        "plan"
    }

    fn content(&self) -> &str {
        &self.content
    }
}

#[derive(Debug, Clone)]
pub enum NodeKind {
    Query,
    Drafting,
    Critique,
    Synthesis,
}

#[derive(Clone)]
pub struct Node {
    pub id: String,
    pub name: String,
    pub model: String,
    pub kind: NodeKind,
    provider: Arc<dyn LlmProvider>,
}

impl Node {
    async fn run(&self, messages: Vec<Message>) -> anyhow::Result<CompletionResponse> {
        self.provider
            .complete(&CompletionRequest::simple(messages))
            .await
            .with_context(|| format!("node '{}' execution failed", self.name))
    }
}

#[derive(Debug, Clone)]
pub struct ProviderConfig {
    pub name: String,
    pub api_key: String,
    pub model: String,
}

#[derive(Debug, Clone, Deserialize)]
struct CritiqueScore {
    score: f32,
    #[allow(dead_code)]
    rationale: Option<String>,
}

#[derive(Clone)]
pub struct GraphExecutor {
    draft_nodes: Vec<Node>,
    critique_nodes: Vec<Node>,
    synthesis_node: Node,
}

impl GraphExecutor {
    pub fn from_provider_configs(configs: &[ProviderConfig]) -> anyhow::Result<Self> {
        anyhow::ensure!(
            configs.len() >= 2,
            "council requires at least two providers"
        );

        let draft_nodes: Vec<Node> = configs
            .iter()
            .enumerate()
            .map(|(idx, cfg)| {
                Ok(Node {
                    id: format!("draft-{}", idx),
                    name: format!("Drafting-{}", cfg.name),
                    model: cfg.model.clone(),
                    kind: NodeKind::Drafting,
                    provider: build_provider(cfg)?,
                })
            })
            .collect::<anyhow::Result<_>>()?;

        let critique_nodes: Vec<Node> = configs
            .iter()
            .enumerate()
            .map(|(idx, cfg)| {
                Ok(Node {
                    id: format!("critique-{}", idx),
                    name: format!("Reviewing-{}", cfg.name),
                    model: cfg.model.clone(),
                    kind: NodeKind::Critique,
                    provider: build_provider(cfg)?,
                })
            })
            .collect::<anyhow::Result<_>>()?;

        let synth_cfg = &configs[0];
        let synthesis_node = Node {
            id: "synthesis-0".to_string(),
            name: format!("Synthesis-{}", synth_cfg.name),
            model: synth_cfg.model.clone(),
            kind: NodeKind::Synthesis,
            provider: build_provider(synth_cfg)?,
        };

        Ok(Self {
            draft_nodes,
            critique_nodes,
            synthesis_node,
        })
    }

    pub async fn execute(
        &self,
        messages: &[Message],
        memory: Option<&MemoryStore>,
    ) -> anyhow::Result<CompletionResponse> {
        // Separate system context, user query, and conversation history.
        let (system_context, query_text, history) = extract_context(messages);
        let query_id = persist(memory, "query", &query_text);

        // ── Phase 1: Parallel drafting (graceful degradation) ────────────
        let mut draft_tasks = Vec::new();
        for (idx, node) in self.draft_nodes.iter().enumerate() {
            let node = node.clone();
            let system = system_context.clone();
            let query = query_text.clone();
            let hist = history.clone();
            draft_tasks.push(tokio::spawn(async move {
                let mut msgs = Vec::new();
                if !system.is_empty() {
                    msgs.push(Message::new(
                        "system",
                        format!("{}\n\n{}", DRAFT_SYSTEM, system),
                    ));
                } else {
                    msgs.push(Message::new("system", DRAFT_SYSTEM));
                }
                // Include conversation history for multi-turn continuity.
                msgs.extend(hist);
                msgs.push(Message::new("user", query));
                let response = node.run(msgs).await?;
                Ok::<(usize, Node, TextArtifact), anyhow::Error>((
                    idx,
                    node,
                    TextArtifact {
                        content: response.content,
                    },
                ))
            }));
        }

        let mut drafts: Vec<(usize, Node, TextArtifact, Option<String>)> = Vec::new();
        let mut draft_failures: Vec<String> = Vec::new();
        for task in draft_tasks {
            match task.await {
                Ok(Ok((idx, node, artifact))) => {
                    let memory_id = persist(memory, &node.name, artifact.content());
                    if let (Some(from), Some(to), Some(store)) =
                        (query_id.as_deref(), memory_id.as_deref(), memory)
                    {
                        let _ = store.add_edge(
                            from,
                            to,
                            EdgeType::DerivedFrom,
                            1.0,
                            serde_json::json!({"node": node.id}),
                        );
                    }
                    drafts.push((idx, node, artifact, memory_id));
                }
                Ok(Err(e)) => {
                    tracing::warn!(error = %e, "council: draft node failed, skipping");
                    draft_failures.push(format!("{}", e));
                }
                Err(e) => {
                    tracing::warn!(error = %e, "council: draft task panicked, skipping");
                    draft_failures.push(format!("task panic: {}", e));
                }
            }
        }

        if drafts.is_empty() {
            anyhow::bail!(
                "council: all draft nodes failed ({})",
                draft_failures.join("; ")
            );
        }

        if !draft_failures.is_empty() {
            tracing::info!(
                succeeded = drafts.len(),
                failed = draft_failures.len(),
                "council: continuing with partial drafts"
            );
        }

        // ── Phase 2: Parallel critique (graceful degradation) ────────────
        let mut critique_tasks = Vec::new();
        for (idx, _draft_node, artifact, draft_mem_id) in &drafts {
            let reviewer_idx = (idx + 1) % self.critique_nodes.len();
            let reviewer = self.critique_nodes[reviewer_idx].clone();
            let draft_content = artifact.content.clone();
            let draft_mem_id = draft_mem_id.clone();
            let query = query_text.clone();
            let system = system_context.clone();
            let draft_idx = *idx;
            critique_tasks.push(tokio::spawn(async move {
                let critique_system = if !system.is_empty() {
                    format!("{}\n\n{}", CRITIQUE_SYSTEM, system)
                } else {
                    CRITIQUE_SYSTEM.to_string()
                };
                let msgs = vec![
                    Message::new("system", critique_system),
                    Message::new(
                        "user",
                        format!(
                            "Original query:\n{}\n\n---\n\nDraft to review:\n{}",
                            query, draft_content
                        ),
                    ),
                ];
                let critique = reviewer.run(msgs).await?;
                Ok::<(usize, Node, String, Option<String>), anyhow::Error>((
                    draft_idx,
                    reviewer,
                    critique.content,
                    draft_mem_id,
                ))
            }));
        }

        // Map draft_idx → (score, critique_text, reviewer_name)
        let mut critique_map: std::collections::HashMap<usize, (f32, String, String)> =
            std::collections::HashMap::new();
        for task in critique_tasks {
            match task.await {
                Ok(Ok((draft_idx, reviewer, critique_raw, draft_mem_id))) => {
                    let parsed = parse_score(&critique_raw).unwrap_or(5.0);
                    let critique_mem_id = persist(memory, &reviewer.name, &critique_raw);
                    if let (Some(from), Some(to), Some(store)) =
                        (draft_mem_id.as_deref(), critique_mem_id.as_deref(), memory)
                    {
                        let _ = store.add_edge(
                            from,
                            to,
                            EdgeType::CritiquedBy,
                            parsed,
                            serde_json::json!({"draft_index": draft_idx}),
                        );
                    }
                    critique_map.insert(draft_idx, (parsed, critique_raw, reviewer.name));
                }
                Ok(Err(e)) => {
                    tracing::warn!(error = %e, "council: critique node failed, using default score");
                }
                Err(e) => {
                    tracing::warn!(error = %e, "council: critique task panicked, using default score");
                }
            }
        }

        // ── Phase 3: Rank and synthesize ─────────────────────────────────
        let mut ranked: Vec<(f32, String, String)> = drafts
            .iter()
            .map(|(idx, node, artifact, _)| {
                let (score, critique, _reviewer) = critique_map.get(idx).cloned().unwrap_or((
                    5.0,
                    "(no critique available)".to_string(),
                    "none".to_string(),
                ));
                (
                    score,
                    artifact.content.clone(),
                    format!("{}\n{}", node.name, critique),
                )
            })
            .collect();
        ranked.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));

        let mut synthesis_input =
            String::from("Synthesize a single best answer from these ranked drafts. Favor high-score content and resolve conflicts.\n\n");
        for (rank, (score, draft, context)) in ranked.iter().enumerate() {
            synthesis_input.push_str(&format!(
                "Draft #{rank} (score {score:.1}):\n{draft}\n\nCritique context:\n{context}\n\n",
            ));
        }

        let synthesis_system = if !system_context.is_empty() {
            format!("{}\n\n{}", SYNTHESIS_SYSTEM, system_context)
        } else {
            SYNTHESIS_SYSTEM.to_string()
        };
        let synthesis_msgs = vec![
            Message::new("system", synthesis_system),
            Message::new("user", synthesis_input),
        ];

        let synthesis = self.synthesis_node.run(synthesis_msgs).await?;

        let synthesis_id = persist(memory, &self.synthesis_node.name, &synthesis.content);
        if let (Some(final_id), Some(store)) = (synthesis_id.as_deref(), memory) {
            for (_, _, _, draft_id) in &drafts {
                if let Some(draft_id) = draft_id.as_deref() {
                    let _ = store.add_edge(
                        draft_id,
                        final_id,
                        EdgeType::RefinedTo,
                        1.0,
                        serde_json::json!({"stage":"synthesis"}),
                    );
                }
            }
        }

        Ok(synthesis)
    }
}

pub async fn run_council(
    messages: &[Message],
    providers: &[ProviderConfig],
    memory: Option<&MemoryStore>,
) -> anyhow::Result<CompletionResponse> {
    let executor = GraphExecutor::from_provider_configs(providers)?;
    match tokio::time::timeout(COUNCIL_TIMEOUT, executor.execute(messages, memory)).await {
        Ok(result) => result,
        Err(_) => anyhow::bail!(
            "council: execution timed out after {}s",
            COUNCIL_TIMEOUT.as_secs()
        ),
    }
}

fn parse_score(raw: &str) -> Option<f32> {
    // Try strict JSON first.
    if let Ok(parsed) = serde_json::from_str::<CritiqueScore>(raw) {
        return Some(parsed.score.clamp(0.0, 10.0));
    }
    // Try extracting JSON from markdown code fences.
    if let Some(json_start) = raw.find('{') {
        if let Some(json_end) = raw.rfind('}') {
            if json_start < json_end {
                if let Ok(parsed) =
                    serde_json::from_str::<CritiqueScore>(&raw[json_start..=json_end])
                {
                    return Some(parsed.score.clamp(0.0, 10.0));
                }
            }
        }
    }
    // Last resort: find a standalone number.
    let digit = raw
        .split(|c: char| !c.is_ascii_digit() && c != '.')
        .find_map(|token| token.parse::<f32>().ok())?;
    Some(digit.clamp(0.0, 10.0))
}

/// Extract system context, user query, and conversation history from the
/// message list.
///
/// Returns (system_context, query_text, history) where:
/// - system_context: concatenation of all system role messages
/// - query_text: the last user message (the actual query), or a fallback
///   concatenation of non-system messages if no user message exists.
/// - history: all non-system messages preceding the last user message,
///   preserving multi-turn conversational context.
fn extract_context(messages: &[Message]) -> (String, String, Vec<Message>) {
    let mut system_parts: Vec<&str> = Vec::new();
    let mut non_system: Vec<&Message> = Vec::new();

    for msg in messages {
        if msg.role == "system" {
            system_parts.push(&msg.content);
        } else {
            non_system.push(msg);
        }
    }

    let system = system_parts.join("\n\n");

    // Split: everything before the last user message is history; the last
    // user message becomes the query.
    if let Some(last_user_pos) = non_system.iter().rposition(|m| m.role == "user") {
        let query = non_system[last_user_pos].content.clone();
        let history: Vec<Message> = non_system
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != last_user_pos)
            .map(|(_, m)| Message::new(&m.role, &m.content))
            .collect();
        (system, query, history)
    } else {
        // Fallback: concatenate all non-system messages as query, no history.
        let fallback = non_system
            .iter()
            .map(|m| format!("{}: {}", m.role, m.content))
            .collect::<Vec<_>>()
            .join("\n");
        (system, fallback, Vec::new())
    }
}

fn persist(memory: Option<&MemoryStore>, node: &str, payload: &str) -> Option<String> {
    let store = memory?;
    let mem = Memory::ephemeral(format!("[council-node:{}]\n{}", node, payload));
    let id = mem.id.clone();
    if store.insert_memory(&mem).is_ok() {
        Some(id)
    } else {
        None
    }
}

fn build_provider(cfg: &ProviderConfig) -> anyhow::Result<Arc<dyn LlmProvider>> {
    let provider = cfg.name.trim().to_lowercase();
    let key = cfg.api_key.clone();
    let model = cfg.model.clone();
    let built: Arc<dyn LlmProvider> = match provider.as_str() {
        "anthropic" => Arc::new(AnthropicProvider::with_model(key, model)),
        "perplexity" | "pplx" => Arc::new(OpenAiCompatibleProvider::with_config(
            CompatibleProvider::Perplexity,
            CompatibleProvider::Perplexity.base_url().to_string(),
            key,
            model,
        )),
        "xai" | "grok" => Arc::new(OpenAiCompatibleProvider::with_config(
            CompatibleProvider::Xai,
            CompatibleProvider::Xai.base_url().to_string(),
            key,
            model,
        )),
        "google" | "gemini" => Arc::new(OpenAiCompatibleProvider::with_config(
            CompatibleProvider::Google,
            CompatibleProvider::Google.base_url().to_string(),
            key,
            model,
        )),
        "openai" => Arc::new(OpenAiProvider::with_model(key, model)),
        other => anyhow::bail!("unsupported council provider: {}", other),
    };
    Ok(built)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_score_strict_json() {
        let raw = r#"{"score": 8.5, "rationale": "good"}"#;
        assert_eq!(parse_score(raw), Some(8.5));
    }

    #[test]
    fn test_parse_score_in_markdown_fence() {
        let raw = "Here is my review:\n```json\n{\"score\": 7, \"rationale\": \"decent\"}\n```";
        assert_eq!(parse_score(raw), Some(7.0));
    }

    #[test]
    fn test_parse_score_bare_number() {
        assert_eq!(parse_score("I give this a 9 out of 10"), Some(9.0));
    }

    #[test]
    fn test_parse_score_clamped() {
        let raw = r#"{"score": 15, "rationale": "over"}"#;
        assert_eq!(parse_score(raw), Some(10.0));
    }

    #[test]
    fn test_parse_score_none() {
        assert_eq!(parse_score("no numbers here"), None);
    }

    #[test]
    fn test_extract_context_basic() {
        let msgs = vec![
            Message::new("system", "You are helpful"),
            Message::new("user", "Hello"),
            Message::new("assistant", "Hi there"),
            Message::new("user", "What is Rust?"),
        ];
        let (system, query, history) = extract_context(&msgs);
        assert_eq!(system, "You are helpful");
        assert_eq!(query, "What is Rust?");
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].role, "user");
        assert_eq!(history[0].content, "Hello");
        assert_eq!(history[1].role, "assistant");
        assert_eq!(history[1].content, "Hi there");
    }

    #[test]
    fn test_extract_context_multiple_system() {
        let msgs = vec![
            Message::new("system", "First"),
            Message::new("system", "Second"),
            Message::new("user", "Query"),
        ];
        let (system, query, history) = extract_context(&msgs);
        assert_eq!(system, "First\n\nSecond");
        assert_eq!(query, "Query");
        assert!(history.is_empty());
    }

    #[test]
    fn test_extract_context_no_user() {
        let msgs = vec![
            Message::new("system", "Sys"),
            Message::new("assistant", "Answer"),
        ];
        let (system, query, history) = extract_context(&msgs);
        assert_eq!(system, "Sys");
        assert_eq!(query, "assistant: Answer");
        assert!(history.is_empty());
    }

    #[test]
    fn test_extract_context_preserves_full_history() {
        let msgs = vec![
            Message::new("system", "You are Orion. soul.md: ..."),
            Message::new("system", "ethics.md: ..."),
            Message::new("user", "Hello, who are you?"),
            Message::new("assistant", "I am Orion."),
            Message::new("user", "Tell me about Rust generics"),
            Message::new("assistant", "Generics allow type parameters..."),
            Message::new("user", "Can you give an example?"),
        ];
        let (system, query, history) = extract_context(&msgs);
        assert_eq!(system, "You are Orion. soul.md: ...\n\nethics.md: ...");
        assert_eq!(query, "Can you give an example?");
        assert_eq!(history.len(), 4);
        assert_eq!(history[0].role, "user");
        assert_eq!(history[0].content, "Hello, who are you?");
        assert_eq!(history[1].role, "assistant");
        assert_eq!(history[1].content, "I am Orion.");
        assert_eq!(history[2].role, "user");
        assert_eq!(history[2].content, "Tell me about Rust generics");
        assert_eq!(history[3].role, "assistant");
        assert_eq!(history[3].content, "Generics allow type parameters...");
    }
}
