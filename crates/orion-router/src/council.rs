use anyhow::Context;
use orion_capabilities::cognitive::{
    AnthropicProvider, CompatibleProvider, CompletionRequest, CompletionResponse, LlmProvider,
    Message, OpenAiCompatibleProvider, OpenAiProvider,
};
use orion_memory::store::EdgeType;
use orion_memory::{Memory, MemoryStore};
use serde::Deserialize;
use std::sync::Arc;

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
        let query_text = flatten_messages(messages);
        let query_id = persist(memory, "query", &query_text);

        let mut tasks = Vec::new();
        for (idx, node) in self.draft_nodes.iter().enumerate() {
            let node = node.clone();
            let query = query_text.clone();
            tasks.push(tokio::spawn(async move {
                let prompt = Message::new(
                    "user",
                    format!(
                        "Create an independent draft solution. Do not mention other drafts.\n\nQuery:\n{}",
                        query
                    ),
                );
                let response = node.run(vec![prompt]).await?;
                Ok::<(usize, Node, TextArtifact), anyhow::Error>((
                    idx,
                    node,
                    TextArtifact {
                        content: response.content,
                    },
                ))
            }));
        }

        let mut drafts = Vec::new();
        for task in tasks {
            let (idx, node, artifact) = task.await??;
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

        let mut critique_tasks = Vec::new();
        for (idx, _draft_node, artifact, draft_mem_id) in &drafts {
            let reviewer = self.critique_nodes[(idx + 1) % self.critique_nodes.len()].clone();
            let draft_content = artifact.content.clone();
            let draft_mem_id = draft_mem_id.clone();
            critique_tasks.push(tokio::spawn(async move {
                let critique_prompt = Message::new(
                    "user",
                    format!(
                        "Review the draft and score it from 0-10. Return STRICT JSON: {{\"score\": number, \"rationale\": string}}.\n\nDraft:\n{}",
                        draft_content
                    ),
                );
                let critique = reviewer.run(vec![critique_prompt]).await?;
                Ok::<(Node, String, Option<String>), anyhow::Error>((
                    reviewer,
                    critique.content,
                    draft_mem_id,
                ))
            }));
        }

        let mut critiques: Vec<(f32, String, String)> = Vec::new();
        for (draft_idx, task) in critique_tasks.into_iter().enumerate() {
            let (reviewer, critique_raw, draft_mem_id) = task.await??;
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
            critiques.push((
                parsed,
                critique_raw,
                self.draft_nodes[draft_idx].name.clone(),
            ));
        }

        let mut ranked: Vec<(f32, String, String)> = drafts
            .iter()
            .enumerate()
            .map(|(idx, (_i, node, artifact, _))| {
                let (score, critique, _) = &critiques[idx];
                (
                    *score,
                    artifact.content.clone(),
                    format!("{}\n{}", node.name, critique),
                )
            })
            .collect();
        ranked.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));

        let mut synthesis_input = String::from("Synthesize a single best answer from these drafts. Favor high-score content and resolve conflicts.\n\n");
        for (rank, (score, draft, context)) in ranked.iter().enumerate() {
            synthesis_input.push_str(&format!(
                "Draft #{rank} (score {score:.1}):\n{draft}\n\nCritique context:\n{context}\n\n",
            ));
        }

        let synthesis = self
            .synthesis_node
            .run(vec![Message::new("user", synthesis_input)])
            .await?;

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
    executor.execute(messages, memory).await
}

fn parse_score(raw: &str) -> Option<f32> {
    if let Ok(parsed) = serde_json::from_str::<CritiqueScore>(raw) {
        return Some(parsed.score.clamp(0.0, 10.0));
    }
    let digit = raw
        .split(|c: char| !c.is_ascii_digit() && c != '.')
        .find_map(|token| token.parse::<f32>().ok())?;
    Some(digit.clamp(0.0, 10.0))
}

fn flatten_messages(messages: &[Message]) -> String {
    messages
        .iter()
        .map(|m| format!("{}: {}", m.role, m.content))
        .collect::<Vec<_>>()
        .join("\n")
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
