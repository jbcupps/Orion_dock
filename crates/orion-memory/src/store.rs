#[cfg(feature = "postgres")]
use crate::postgres_store;
use crate::sqlite_store;
use chrono::Utc;
use orion_core::AppConfig;
use std::path::Path;
use thiserror::Error;
use uuid::Uuid;

/// Retention tier for a memory entry, from short-lived to permanent.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum MemoryWeight {
    Ephemeral,
    Distilled,
    Crystallized,
}

impl MemoryWeight {
    pub fn as_str(&self) -> &'static str {
        match self {
            MemoryWeight::Ephemeral => "ephemeral",
            MemoryWeight::Distilled => "distilled",
            MemoryWeight::Crystallized => "crystallized",
        }
    }
}

/// A single memory record stored by an agent (chat turn, birth record, etc.).
#[derive(Debug, Clone)]
pub struct Memory {
    pub id: String,
    pub content: String,
    pub weight: MemoryWeight,
    pub created_at: chrono::DateTime<Utc>,
}

impl Memory {
    /// Create a short-lived ephemeral memory (e.g. a single chat turn).
    pub fn ephemeral(content: String) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            content,
            weight: MemoryWeight::Ephemeral,
            created_at: Utc::now(),
        }
    }

    /// Create a medium-retention distilled memory (e.g. summarized conversation).
    pub fn distilled(content: String) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            content,
            weight: MemoryWeight::Distilled,
            created_at: Utc::now(),
        }
    }

    /// Create a permanent crystallized memory (e.g. birth record, core identity fact).
    pub fn crystallized(content: String) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            content,
            weight: MemoryWeight::Crystallized,
            created_at: Utc::now(),
        }
    }
}

/// Errors that can occur during memory store operations.
#[derive(Error, Debug)]
pub enum StoreError {
    #[error("SQLite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("Postgres error: {0}")]
    Postgres(String),
    #[error("Birth already recorded")]
    BirthAlreadyRecorded,
    #[error("Invalid data: {0}")]
    InvalidData(String),
}

pub type Result<T> = std::result::Result<T, StoreError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EdgeType {
    DerivedFrom,
    CritiquedBy,
    RefinedTo,
}

impl EdgeType {
    pub fn as_str(&self) -> &'static str {
        match self {
            EdgeType::DerivedFrom => "derived_from",
            EdgeType::CritiquedBy => "critiqued_by",
            EdgeType::RefinedTo => "refined_to",
        }
    }
}

/// Unified memory store: SQLite or PostgreSQL backend.
pub enum MemoryStore {
    Sqlite(sqlite_store::SqliteStore),
    #[cfg(feature = "postgres")]
    Postgres(postgres_store::PostgresStore),
}

impl MemoryStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let store = sqlite_store::SqliteStore::open(path)?;
        Ok(MemoryStore::Sqlite(store))
    }

    pub fn open_in_memory() -> Result<Self> {
        let store = sqlite_store::SqliteStore::open_in_memory()?;
        Ok(MemoryStore::Sqlite(store))
    }

    /// Open store from app config. Uses Postgres when memory_backend is postgres and DATABASE_URL is set.
    pub fn open_with_config(config: &AppConfig) -> Result<Self> {
        use orion_core::MemoryBackend;
        match config.memory_backend {
            MemoryBackend::Postgres => {
                #[cfg(feature = "postgres")]
                {
                    let url = config.database_url.as_deref().ok_or_else(|| {
                        StoreError::InvalidData(
                            "DATABASE_URL required for postgres backend".to_string(),
                        )
                    })?;
                    let store = postgres_store::PostgresStore::connect(url)?;
                    Ok(MemoryStore::Postgres(store))
                }
                #[cfg(not(feature = "postgres"))]
                {
                    Err(StoreError::InvalidData(
                        "postgres backend requires orion-memory with postgres feature".to_string(),
                    ))
                }
            }
            MemoryBackend::Sqlite => {
                let store = sqlite_store::SqliteStore::open_with_config(config)?;
                Ok(MemoryStore::Sqlite(store))
            }
        }
    }

    pub fn has_birth(&self) -> Result<bool> {
        match self {
            MemoryStore::Sqlite(s) => s.has_birth(),
            #[cfg(feature = "postgres")]
            MemoryStore::Postgres(s) => s.has_birth(),
        }
    }

    pub fn record_birth(&self, memory: &Memory) -> Result<()> {
        match self {
            MemoryStore::Sqlite(s) => s.record_birth(memory),
            #[cfg(feature = "postgres")]
            MemoryStore::Postgres(s) => s.record_birth(memory),
        }
    }

    pub fn insert_memory(&self, memory: &Memory) -> Result<()> {
        match self {
            MemoryStore::Sqlite(s) => s.insert_memory(memory),
            #[cfg(feature = "postgres")]
            MemoryStore::Postgres(s) => s.insert_memory(memory),
        }
    }

    pub fn count_memories(&self) -> Result<u64> {
        match self {
            MemoryStore::Sqlite(s) => s.count_memories(),
            #[cfg(feature = "postgres")]
            MemoryStore::Postgres(s) => s.count_memories(),
        }
    }

    pub fn vacuum(&self) -> Result<()> {
        match self {
            MemoryStore::Sqlite(s) => s.vacuum(),
            #[cfg(feature = "postgres")]
            MemoryStore::Postgres(s) => s.vacuum(),
        }
    }

    pub fn clear_memories(&self) -> Result<u64> {
        match self {
            MemoryStore::Sqlite(s) => s.clear_memories(),
            #[cfg(feature = "postgres")]
            MemoryStore::Postgres(s) => s.clear_memories(),
        }
    }

    pub fn recent_memories(&self, limit: usize) -> Result<Vec<Memory>> {
        match self {
            MemoryStore::Sqlite(s) => s.recent_memories(limit),
            #[cfg(feature = "postgres")]
            MemoryStore::Postgres(s) => s.recent_memories(limit),
        }
    }

    /// Semantic search by embedding (cosine similarity). Requires postgres backend with embeddings.
    pub fn search_by_vector(&self, embedding: &[f32], limit: usize) -> Result<Vec<Memory>> {
        match self {
            MemoryStore::Sqlite(_) => Err(StoreError::InvalidData(format!(
                "vector search requires postgres backend (embedding len={}, limit={})",
                embedding.len(),
                limit
            ))),
            #[cfg(feature = "postgres")]
            MemoryStore::Postgres(s) => s.search_by_vector(embedding, limit),
        }
    }

    /// Graph search: N-hop traversal from a memory. Requires postgres backend.
    pub fn search_by_graph(
        &self,
        from_memory_id: &str,
        edge_type: &str,
        max_hops: u32,
        limit: usize,
    ) -> Result<Vec<Memory>> {
        match self {
            MemoryStore::Sqlite(_) => Err(StoreError::InvalidData(format!(
                "graph search requires postgres backend (from={}, edge_type={}, max_hops={}, limit={})",
                from_memory_id, edge_type, max_hops, limit
            ))),
            #[cfg(feature = "postgres")]
            MemoryStore::Postgres(s) => {
                s.search_by_graph(from_memory_id, edge_type, max_hops, limit)
            }
        }
    }

    /// Store an embedding for a memory. Requires postgres backend.
    pub fn upsert_embedding(&self, memory_id: &str, embedding: &[f32], model: &str) -> Result<()> {
        match self {
            MemoryStore::Sqlite(_) => Err(StoreError::InvalidData(format!(
                "embeddings require postgres backend (memory_id={}, embedding_dim={}, model={})",
                memory_id,
                embedding.len(),
                model
            ))),
            #[cfg(feature = "postgres")]
            MemoryStore::Postgres(s) => s.upsert_embedding(memory_id, embedding, model),
        }
    }

    /// Add a directed edge between memories. Requires postgres backend.
    pub fn add_edge(
        &self,
        from_memory_id: &str,
        to_memory_id: &str,
        edge_type: EdgeType,
        weight: f32,
        _metadata: serde_json::Value,
    ) -> Result<()> {
        match self {
            MemoryStore::Sqlite(_) => Err(StoreError::InvalidData(format!(
                "graph edges require postgres backend ({} -> {}, type={}, weight={})",
                from_memory_id,
                to_memory_id,
                edge_type.as_str(),
                weight
            ))),
            #[cfg(feature = "postgres")]
            MemoryStore::Postgres(s) => {
                s.add_edge(from_memory_id, to_memory_id, edge_type, weight, _metadata)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_birth_record() {
        let store = MemoryStore::open_in_memory().unwrap();
        assert!(!store.has_birth().unwrap());
        store
            .record_birth(&Memory::crystallized("I was born".into()))
            .unwrap();
        assert!(store.has_birth().unwrap());
    }

    #[test]
    fn test_double_birth_rejected() {
        let store = MemoryStore::open_in_memory().unwrap();
        store
            .record_birth(&Memory::crystallized("I was born".into()))
            .unwrap();
        let result = store.record_birth(&Memory::crystallized("Born again".into()));
        assert!(result.is_err());
        match result.unwrap_err() {
            StoreError::BirthAlreadyRecorded => {}
            e => panic!("Expected BirthAlreadyRecorded, got: {:?}", e),
        }
    }

    #[test]
    fn test_insert_and_retrieve_ephemeral_memory() {
        let store = MemoryStore::open_in_memory().unwrap();

        let mem = Memory::ephemeral("user: Hello | assistant: Hi there".into());
        store.insert_memory(&mem).unwrap();

        let recent = store.recent_memories(10).unwrap();
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].content, "user: Hello | assistant: Hi there");
        assert_eq!(recent[0].weight, MemoryWeight::Ephemeral);
    }

    #[test]
    fn test_insert_multiple_weights() {
        let store = MemoryStore::open_in_memory().unwrap();

        store
            .insert_memory(&Memory::ephemeral("ephemeral msg".into()))
            .unwrap();
        store
            .insert_memory(&Memory::distilled("distilled msg".into()))
            .unwrap();
        store
            .insert_memory(&Memory::crystallized("crystallized msg".into()))
            .unwrap();

        let recent = store.recent_memories(10).unwrap();
        assert_eq!(recent.len(), 3);

        // Verify all weight tiers are stored and retrieved correctly
        let weights: Vec<&MemoryWeight> = recent.iter().map(|m| &m.weight).collect();
        assert!(weights.contains(&&MemoryWeight::Ephemeral));
        assert!(weights.contains(&&MemoryWeight::Distilled));
        assert!(weights.contains(&&MemoryWeight::Crystallized));
    }

    #[test]
    fn test_recent_memories_ordering() {
        let store = MemoryStore::open_in_memory().unwrap();

        // Insert in order — recent_memories should return most recent first
        store
            .insert_memory(&Memory::ephemeral("first".into()))
            .unwrap();
        // Small delay to ensure different timestamps
        std::thread::sleep(std::time::Duration::from_millis(10));
        store
            .insert_memory(&Memory::ephemeral("second".into()))
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(10));
        store
            .insert_memory(&Memory::ephemeral("third".into()))
            .unwrap();

        let recent = store.recent_memories(10).unwrap();
        assert_eq!(recent.len(), 3);
        assert_eq!(recent[0].content, "third");
        assert_eq!(recent[1].content, "second");
        assert_eq!(recent[2].content, "first");
    }

    #[test]
    fn test_recent_memories_limit() {
        let store = MemoryStore::open_in_memory().unwrap();

        for i in 0..10 {
            store
                .insert_memory(&Memory::ephemeral(format!("msg {}", i)))
                .unwrap();
        }

        let recent = store.recent_memories(3).unwrap();
        assert_eq!(recent.len(), 3);
    }

    #[test]
    fn test_recent_memories_empty_store() {
        let store = MemoryStore::open_in_memory().unwrap();
        let recent = store.recent_memories(10).unwrap();
        assert!(recent.is_empty());
    }

    #[test]
    fn test_memory_ids_are_unique() {
        let store = MemoryStore::open_in_memory().unwrap();

        let m1 = Memory::ephemeral("msg1".into());
        let m2 = Memory::ephemeral("msg2".into());
        assert_ne!(m1.id, m2.id);

        store.insert_memory(&m1).unwrap();
        store.insert_memory(&m2).unwrap();

        let recent = store.recent_memories(10).unwrap();
        assert_eq!(recent.len(), 2);
    }

    #[test]
    fn test_file_backed_store() {
        let tmp = std::env::temp_dir().join("orion_memory_file_test");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        let db_path = tmp.join("test.db");

        // Open, insert, close
        {
            let store = MemoryStore::open(&db_path).unwrap();
            store
                .insert_memory(&Memory::ephemeral("persisted msg".into()))
                .unwrap();
            store
                .record_birth(&Memory::crystallized("born".into()))
                .unwrap();
        }

        // Reopen and verify persistence
        {
            let store = MemoryStore::open(&db_path).unwrap();
            assert!(store.has_birth().unwrap());
            let recent = store.recent_memories(10).unwrap();
            assert_eq!(recent.len(), 1);
            assert_eq!(recent[0].content, "persisted msg");
        }

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_count_memories() {
        let store = MemoryStore::open_in_memory().unwrap();
        assert_eq!(store.count_memories().unwrap(), 0);

        store
            .insert_memory(&Memory::ephemeral("msg1".into()))
            .unwrap();
        assert_eq!(store.count_memories().unwrap(), 1);

        store
            .insert_memory(&Memory::ephemeral("msg2".into()))
            .unwrap();
        store
            .insert_memory(&Memory::distilled("msg3".into()))
            .unwrap();
        assert_eq!(store.count_memories().unwrap(), 3);
    }

    #[test]
    fn test_clear_memories() {
        let store = MemoryStore::open_in_memory().unwrap();

        // Add some memories and a birth record
        store
            .insert_memory(&Memory::ephemeral("msg1".into()))
            .unwrap();
        store
            .insert_memory(&Memory::ephemeral("msg2".into()))
            .unwrap();
        store
            .record_birth(&Memory::crystallized("born".into()))
            .unwrap();

        assert_eq!(store.count_memories().unwrap(), 2);
        assert!(store.has_birth().unwrap());

        // Clear memories
        let deleted = store.clear_memories().unwrap();
        assert_eq!(deleted, 2);

        // Memories gone, but birth still there
        assert_eq!(store.count_memories().unwrap(), 0);
        assert!(store.has_birth().unwrap());
    }

    #[test]
    fn test_vacuum() {
        let store = MemoryStore::open_in_memory().unwrap();

        // Insert and delete some data
        for i in 0..10 {
            store
                .insert_memory(&Memory::ephemeral(format!("msg {}", i)))
                .unwrap();
        }
        store.clear_memories().unwrap();

        // VACUUM should succeed
        assert!(store.vacuum().is_ok());
    }
}
