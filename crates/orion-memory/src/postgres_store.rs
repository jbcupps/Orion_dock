//! PostgreSQL-backed memory store implementation. Requires feature "postgres".

use crate::store::{EdgeType, Memory, MemoryWeight, Result, StoreError};
use chrono::{DateTime, Utc};
use pgvector::Vector;
use sqlx::postgres::PgRow;
use sqlx::Row;
use sqlx::{PgPool, Pool, Postgres};

/// PostgreSQL-backed memory store. Runs migrations on connection.
pub struct PostgresStore {
    pool: PgPool,
    rt: tokio::runtime::Runtime,
}

impl PostgresStore {
    /// Connect to Postgres at `database_url` and run migrations.
    pub async fn connect_async(database_url: &str) -> Result<Self> {
        let pool = PgPool::connect(database_url)
            .await
            .map_err(|e| StoreError::Postgres(e.to_string()))?;
        let rt = tokio::runtime::Runtime::new().map_err(|e| StoreError::Postgres(e.to_string()))?;
        Self::run_migrations(&pool)
            .await
            .map_err(|e| StoreError::Postgres(e.to_string()))?;
        Ok(Self { pool, rt })
    }

    /// Connect synchronously (creates a temporary runtime for migration).
    pub fn connect(database_url: &str) -> Result<Self> {
        let rt = tokio::runtime::Runtime::new().map_err(|e| StoreError::Postgres(e.to_string()))?;
        let pool = rt
            .block_on(PgPool::connect(database_url))
            .map_err(|e| StoreError::Postgres(e.to_string()))?;
        rt.block_on(Self::run_migrations(&pool))
            .map_err(|e| StoreError::Postgres(e.to_string()))?;
        Ok(Self { pool, rt })
    }

    async fn run_migrations(pool: &Pool<Postgres>) -> std::result::Result<(), String> {
        let migrator = sqlx::migrate::Migrator::new(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations"),
        )
        .await
        .map_err(|e| e.to_string())?;
        migrator.run(pool).await.map_err(|e| e.to_string())?;
        Ok(())
    }

    fn block_on<F, T>(&self, f: F) -> T
    where
        F: std::future::Future<Output = T>,
    {
        self.rt.block_on(f)
    }

    pub fn has_birth(&self, agent_id: &str) -> Result<bool> {
        let pool = &self.pool;
        let agent_id = agent_id.to_string();
        let count: (i64,) = self
            .block_on(
                sqlx::query_as("SELECT COUNT(*)::bigint FROM birth WHERE agent_id = $1")
                    .bind(&agent_id)
                    .fetch_one(pool),
            )
            .map_err(|e| StoreError::Postgres(e.to_string()))?;
        Ok(count.0 > 0)
    }

    pub fn record_birth(&self, agent_id: &str, memory: &Memory) -> Result<()> {
        if self.has_birth(agent_id)? {
            return Err(StoreError::BirthAlreadyRecorded);
        }
        let pool = &self.pool;
        let agent_id = agent_id.to_string();
        let content = memory.content.clone();
        let created_at = memory.created_at;
        self.block_on(
            sqlx::query(
                "INSERT INTO birth (agent_id, content, created_at) VALUES ($1, $2, $3)",
            )
            .bind(&agent_id)
            .bind(&content)
            .bind(created_at)
            .execute(pool),
        )
        .map_err(|e| StoreError::Postgres(e.to_string()))?;
        Ok(())
    }

    pub fn insert_memory(&self, memory: &Memory) -> Result<()> {
        let pool = &self.pool;
        let id = memory.id.clone();
        let content = memory.content.clone();
        let weight = memory.weight.as_str();
        let created_at = memory.created_at;
        self.block_on(
            sqlx::query(
                "INSERT INTO memories (id, content, weight, created_at) VALUES ($1, $2, $3, $4)",
            )
            .bind(&id)
            .bind(&content)
            .bind(weight)
            .bind(created_at)
            .execute(pool),
        )
        .map_err(|e| StoreError::Postgres(e.to_string()))?;
        Ok(())
    }

    pub fn count_memories(&self) -> Result<u64> {
        let pool = &self.pool;
        let row: (i64,) = self
            .block_on(sqlx::query_as("SELECT COUNT(*)::bigint FROM memories").fetch_one(pool))
            .map_err(|e| StoreError::Postgres(e.to_string()))?;
        Ok(row.0 as u64)
    }

    pub fn vacuum(&self) -> Result<()> {
        let pool = &self.pool;
        self.block_on(sqlx::query("VACUUM FULL memories").execute(pool))
            .map_err(|e| StoreError::Postgres(e.to_string()))?;
        Ok(())
    }

    pub fn clear_memories(&self) -> Result<u64> {
        let pool = &self.pool;
        let result = self
            .block_on(sqlx::query("DELETE FROM memories").execute(pool))
            .map_err(|e| StoreError::Postgres(e.to_string()))?;
        Ok(result.rows_affected() as u64)
    }

    pub fn recent_memories(&self, limit: usize) -> Result<Vec<Memory>> {
        let pool = &self.pool;
        let limit = limit as i64;
        let rows = self
            .block_on(
                sqlx::query(
                    "SELECT id, content, weight, created_at FROM memories ORDER BY created_at DESC LIMIT $1",
                )
                .bind(limit)
                .fetch_all(pool),
            )
            .map_err(|e| StoreError::Postgres(e.to_string()))?;
        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            let created_at: DateTime<Utc> = row.get("created_at");
            let weight_str: String = row.get("weight");
            let weight = match weight_str.as_str() {
                "ephemeral" => MemoryWeight::Ephemeral,
                "distilled" => MemoryWeight::Distilled,
                "crystallized" => MemoryWeight::Crystallized,
                w => return Err(StoreError::InvalidData(format!("unknown weight: {}", w))),
            };
            out.push(Memory {
                id: row.get("id"),
                content: row.get("content"),
                weight,
                created_at,
            });
        }
        Ok(out)
    }

    /// Store an embedding for a memory. Overwrites if memory_id already has an embedding.
    pub fn upsert_embedding(&self, memory_id: &str, embedding: &[f32], model: &str) -> Result<()> {
        let pool = &self.pool;
        let v = Vector::from(embedding.to_vec());
        let memory_id = memory_id.to_string();
        let model = model.to_string();
        self.block_on(
            sqlx::query(
                "INSERT INTO memory_embeddings (memory_id, embedding, model) VALUES ($1, $2, $3) \
                 ON CONFLICT (memory_id) DO UPDATE SET embedding = $2, model = $3, created_at = now()",
            )
            .bind(&memory_id)
            .bind(v)
            .bind(&model)
            .execute(pool),
        )
        .map_err(|e| StoreError::Postgres(e.to_string()))?;
        Ok(())
    }

    /// Semantic search by cosine similarity. Returns memories that have embeddings, ordered by similarity.
    pub fn search_by_vector(&self, embedding: &[f32], limit: usize) -> Result<Vec<Memory>> {
        let pool = &self.pool;
        let v = Vector::from(embedding.to_vec());
        let limit = limit as i64;
        let rows = self
            .block_on(
                sqlx::query(
                    "SELECT m.id, m.content, m.weight, m.created_at \
                     FROM memories m \
                     INNER JOIN memory_embeddings e ON m.id = e.memory_id \
                     ORDER BY e.embedding <=> $1 \
                     LIMIT $2",
                )
                .bind(v)
                .bind(limit)
                .fetch_all(pool),
            )
            .map_err(|e| StoreError::Postgres(e.to_string()))?;
        rows_to_memories(&rows)
    }

    /// Add a directed edge between memories.
    pub fn add_edge(
        &self,
        from_memory_id: &str,
        to_memory_id: &str,
        edge_type: EdgeType,
        weight: f32,
        metadata: serde_json::Value,
    ) -> Result<()> {
        let pool = &self.pool;
        self.block_on(
            sqlx::query(
                "INSERT INTO memory_edges (from_memory_id, to_memory_id, edge_type, weight, metadata) \
                 VALUES ($1, $2, $3, $4, $5) \
                 ON CONFLICT (from_memory_id, to_memory_id, edge_type) DO UPDATE SET weight = $4, metadata = $5",
            )
            .bind(from_memory_id)
            .bind(to_memory_id)
            .bind(edge_type.as_str())
            .bind(weight)
            .bind(sqlx::types::Json(metadata))
            .execute(pool),
        )
        .map_err(|e| StoreError::Postgres(e.to_string()))?;
        Ok(())
    }

    /// Graph search: N-hop traversal from a memory. Returns connected memories ordered by hop distance.
    pub fn search_by_graph(
        &self,
        from_memory_id: &str,
        edge_type: &str,
        max_hops: u32,
        limit: usize,
    ) -> Result<Vec<Memory>> {
        let pool = &self.pool;
        let limit = limit as i64;
        let max_hops = max_hops as i32;
        let rows = self
            .block_on(
                sqlx::query(
                    "WITH RECURSIVE graph_walk(depth, memory_id) AS ( \
                     SELECT 1, to_memory_id FROM memory_edges \
                     WHERE from_memory_id = $1 AND edge_type = $2 \
                     UNION \
                     SELECT g.depth + 1, e.to_memory_id FROM graph_walk g \
                     JOIN memory_edges e ON e.from_memory_id = g.memory_id AND e.edge_type = $2 \
                     WHERE g.depth < $3 \
                     ) \
                     SELECT DISTINCT ON (m.id) m.id, m.content, m.weight, m.created_at \
                     FROM memories m \
                     JOIN graph_walk g ON m.id = g.memory_id \
                     ORDER BY m.id, g.depth \
                     LIMIT $4",
                )
                .bind(from_memory_id)
                .bind(edge_type)
                .bind(max_hops)
                .bind(limit)
                .fetch_all(pool),
            )
            .map_err(|e| StoreError::Postgres(e.to_string()))?;
        rows_to_memories(&rows)
    }
}

fn rows_to_memories(rows: &[PgRow]) -> Result<Vec<Memory>> {
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let created_at: DateTime<Utc> = row.get("created_at");
        let weight_str: String = row.get("weight");
        let weight = match weight_str.as_str() {
            "ephemeral" => MemoryWeight::Ephemeral,
            "distilled" => MemoryWeight::Distilled,
            "crystallized" => MemoryWeight::Crystallized,
            w => return Err(StoreError::InvalidData(format!("unknown weight: {}", w))),
        };
        out.push(Memory {
            id: row.get("id"),
            content: row.get("content"),
            weight,
            created_at,
        });
    }
    Ok(out)
}
