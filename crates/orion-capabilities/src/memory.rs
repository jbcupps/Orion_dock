//! Specialized memory capability traits and store-backed implementations.

use async_trait::async_trait;

#[derive(Debug, Clone, Copy)]
pub enum MemoryType {
    KeyValue,
    Vector,
    Graph,
    Document,
    Hybrid,
}

#[derive(Debug, Clone)]
pub struct MemorySystemInfo {
    pub id: String,
    pub memory_type: MemoryType,
}

#[derive(Debug, Clone)]
pub struct MemoryEntry;

/// Typed query for memory search: recency, vector similarity, or graph traversal.
#[derive(Debug, Clone)]
pub enum MemoryQuery {
    /// Recent memories by created_at (limit).
    Recency { limit: usize },
    /// Semantic search by embedding (cosine similarity).
    Vector { embedding: Vec<f32>, limit: usize },
    /// Graph N-hop from a memory.
    Graph {
        from_memory_id: String,
        edge_type: String,
        max_hops: u32,
        limit: usize,
    },
}

/// A single memory search result with optional similarity score.
#[derive(Debug, Clone)]
pub struct MemorySearchResult {
    pub id: String,
    pub content: String,
    pub score: Option<f32>,
}

#[async_trait]
pub trait SpecializedMemoryCapability: Send + Sync {
    fn info(&self) -> MemorySystemInfo;
    async fn store(&self, _entry: MemoryEntry) -> anyhow::Result<String> {
        Err(anyhow::anyhow!("stub: not implemented"))
    }
    async fn retrieve(&self, _id: &str) -> anyhow::Result<Option<MemoryEntry>> {
        Err(anyhow::anyhow!("stub: not implemented"))
    }
    async fn search(&self, _query: MemoryQuery) -> anyhow::Result<Vec<MemorySearchResult>> {
        Err(anyhow::anyhow!("stub: not implemented"))
    }
}

#[cfg(feature = "memory")]
mod store_backed {
    use super::{MemoryQuery, MemorySearchResult, MemorySystemInfo, MemoryType};
    use async_trait::async_trait;
    use orion_memory::MemoryStore;
    use std::sync::Arc;
    use tokio::task::spawn_blocking;

    /// Vector or graph capability backed by a MemoryStore (Postgres required for vector/graph).
    pub struct StoreBackedMemoryCapability {
        pub store: Arc<MemoryStore>,
        pub memory_type: MemoryType,
        pub id: String,
    }

    #[async_trait]
    impl super::SpecializedMemoryCapability for StoreBackedMemoryCapability {
        fn info(&self) -> MemorySystemInfo {
            MemorySystemInfo {
                id: self.id.clone(),
                memory_type: self.memory_type,
            }
        }

        async fn search(
            &self,
            query: super::MemoryQuery,
        ) -> anyhow::Result<Vec<super::MemorySearchResult>> {
            let store = Arc::clone(&self.store);
            let results = match query {
                MemoryQuery::Recency { limit } => {
                    let store = Arc::clone(&store);
                    spawn_blocking(move || store.recent_memories(limit))
                        .await
                        .map_err(|e| anyhow::anyhow!("{}", e))?
                        .map_err(|e| anyhow::anyhow!("{}", e))?
                }
                MemoryQuery::Vector { embedding, limit } => {
                    let store = Arc::clone(&store);
                    let embedding = embedding.clone();
                    spawn_blocking(move || store.search_by_vector(&embedding, limit))
                        .await
                        .map_err(|e| anyhow::anyhow!("{}", e))?
                        .map_err(|e| anyhow::anyhow!("{}", e))?
                }
                MemoryQuery::Graph {
                    from_memory_id,
                    edge_type,
                    max_hops,
                    limit,
                } => {
                    let store = Arc::clone(&store);
                    spawn_blocking(move || {
                        store.search_by_graph(&from_memory_id, &edge_type, max_hops, limit)
                    })
                    .await
                    .map_err(|e| anyhow::anyhow!("{}", e))?
                    .map_err(|e| anyhow::anyhow!("{}", e))?
                }
            };
            Ok(results
                .into_iter()
                .map(|m| MemorySearchResult {
                    id: m.id,
                    content: m.content,
                    score: None,
                })
                .collect())
        }
    }

    impl StoreBackedMemoryCapability {
        pub fn vector(store: Arc<MemoryStore>, id: impl Into<String>) -> Self {
            Self {
                store,
                memory_type: MemoryType::Vector,
                id: id.into(),
            }
        }

        pub fn graph(store: Arc<MemoryStore>, id: impl Into<String>) -> Self {
            Self {
                store,
                memory_type: MemoryType::Graph,
                id: id.into(),
            }
        }
    }
}

#[cfg(feature = "memory")]
pub use store_backed::StoreBackedMemoryCapability;
