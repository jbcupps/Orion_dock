-- Orion memory: memories and birth tables (PostgreSQL).
-- Vector and graph tables added in later migrations.

CREATE TABLE IF NOT EXISTS memories (
    id TEXT PRIMARY KEY,
    content TEXT NOT NULL,
    weight TEXT NOT NULL CHECK (weight IN ('ephemeral', 'distilled', 'crystallized')),
    created_at TIMESTAMPTZ NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_memories_created_at ON memories(created_at DESC);

CREATE TABLE IF NOT EXISTS birth (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    content TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL
);
