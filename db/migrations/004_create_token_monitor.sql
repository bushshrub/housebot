-- Recreated after 001_purge_all_data. `crates/token-monitor` keeps running
-- per-conversation totals for all-time queries and one immutable event per
-- recorded usage for the windowed ones; the archive of message bodies that
-- used to sit beside them is deliberately gone.

CREATE TABLE IF NOT EXISTS conversations (
    conversation_id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL,
    display_name TEXT NOT NULL DEFAULT '',
    channel_id TEXT NOT NULL,
    input_tokens BIGINT NOT NULL DEFAULT 0,
    output_tokens BIGINT NOT NULL DEFAULT 0,
    cached_tokens BIGINT NOT NULL DEFAULT 0,
    request_count BIGINT NOT NULL DEFAULT 0,
    started_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    ended_at TIMESTAMPTZ
);

-- Serves the startup lookup that resumes a user's unfinished conversation.
CREATE INDEX IF NOT EXISTS conversations_active_by_user_idx
    ON conversations (user_id, started_at DESC)
    WHERE ended_at IS NULL;

CREATE TABLE IF NOT EXISTS token_usage_events (
    id BIGSERIAL PRIMARY KEY,
    conversation_id TEXT NOT NULL
        REFERENCES conversations (conversation_id) ON DELETE CASCADE,
    user_id TEXT NOT NULL,
    display_name TEXT NOT NULL DEFAULT '',
    input_tokens BIGINT NOT NULL,
    output_tokens BIGINT NOT NULL,
    cached_tokens BIGINT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS token_usage_events_created_at_idx
    ON token_usage_events (created_at);
CREATE INDEX IF NOT EXISTS token_usage_events_user_idx
    ON token_usage_events (user_id, created_at);
