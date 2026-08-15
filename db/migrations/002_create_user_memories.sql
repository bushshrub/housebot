-- Recreated after 001_purge_all_data. `crates/memory` stores one markdown
-- document per user and rewrites it wholesale, so the user ID is the key and
-- the content is opaque text.

CREATE TABLE IF NOT EXISTS user_memories (
    user_id TEXT PRIMARY KEY,
    content TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
