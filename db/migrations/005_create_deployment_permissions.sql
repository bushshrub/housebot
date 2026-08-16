-- Recreated after 001_purge_all_data. `crates/deployment-bot` grants deploy
-- access one Discord user at a time; the owner is authorized without a row, so
-- this table holds only the delegated grants. IDs are BIGINT rather than the
-- TEXT the chatbot stores, because `permissions.rs` converts to i64 and refuses
-- anything that does not fit.

CREATE TABLE IF NOT EXISTS deployment_permissions (
    user_id BIGINT PRIMARY KEY,
    granted_by BIGINT NOT NULL,
    granted_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
