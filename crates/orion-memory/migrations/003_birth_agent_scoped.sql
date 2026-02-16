-- Scope birth table by agent_id so multiple agents can share one Postgres database.
-- Previous schema had: id INTEGER PRIMARY KEY CHECK (id = 1) — a singleton row.
-- New schema uses agent_id TEXT as the primary key.

-- Step 1: Add the agent_id column (nullable temporarily for existing rows).
ALTER TABLE birth ADD COLUMN IF NOT EXISTS agent_id TEXT;

-- Step 2: Backfill existing rows with a default agent_id.
UPDATE birth SET agent_id = 'legacy-default' WHERE agent_id IS NULL;

-- Step 3: Drop the old primary key and CHECK constraint, set new primary key.
-- PostgreSQL doesn't support DROP CONSTRAINT IF EXISTS directly, so we use a DO block.
DO $$
BEGIN
    -- Drop the old primary key constraint (named birth_pkey by default).
    IF EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'birth_pkey') THEN
        ALTER TABLE birth DROP CONSTRAINT birth_pkey;
    END IF;
    -- Drop the CHECK constraint on id column (named birth_id_check by default).
    IF EXISTS (SELECT 1 FROM pg_constraint WHERE conname = 'birth_id_check') THEN
        ALTER TABLE birth DROP CONSTRAINT birth_id_check;
    END IF;
END $$;

-- Step 4: Make agent_id NOT NULL and set as new primary key.
ALTER TABLE birth ALTER COLUMN agent_id SET NOT NULL;
ALTER TABLE birth ADD PRIMARY KEY (agent_id);

-- Step 5: Drop the old id column (no longer needed).
ALTER TABLE birth DROP COLUMN IF EXISTS id;
