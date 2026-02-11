-- pgvector and graph tables for semantic and graph-based memory retrieval.
-- Requires the vector extension (e.g. in Docker: postgres image with pgvector).

CREATE EXTENSION IF NOT EXISTS vector;

ALTER TABLE memories ADD COLUMN IF NOT EXISTS metadata JSONB;
CREATE INDEX IF NOT EXISTS idx_memories_metadata ON memories USING GIN (metadata);

CREATE TABLE IF NOT EXISTS memory_embeddings (
    memory_id TEXT NOT NULL REFERENCES memories(id) ON DELETE CASCADE,
    embedding vector(384) NOT NULL,
    model TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (memory_id)
);

CREATE INDEX IF NOT EXISTS idx_memory_embeddings_embedding ON memory_embeddings
    USING hnsw (embedding vector_cosine_ops) WITH (m = 16, ef_construction = 64);

CREATE TABLE IF NOT EXISTS memory_edges (
    from_memory_id TEXT NOT NULL REFERENCES memories(id) ON DELETE CASCADE,
    to_memory_id TEXT NOT NULL REFERENCES memories(id) ON DELETE CASCADE,
    edge_type TEXT NOT NULL,
    weight REAL NOT NULL DEFAULT 1.0,
    metadata JSONB,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (from_memory_id, to_memory_id, edge_type)
);

CREATE INDEX IF NOT EXISTS idx_memory_edges_from ON memory_edges(from_memory_id, edge_type);
CREATE INDEX IF NOT EXISTS idx_memory_edges_to ON memory_edges(to_memory_id, edge_type);
