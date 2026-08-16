-- Recreated after 001_purge_all_data. `crates/bot-config` stores one JSON
-- document per key (`server:<id>`, `user:<id>`, `access_control`,
-- `scheduler_limits`), so the value is opaque text to the database.

CREATE TABLE IF NOT EXISTS bot_config (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
