CREATE TABLE IF NOT EXISTS user_memories (
    user_id TEXT PRIMARY KEY,
    content TEXT NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
