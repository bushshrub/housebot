-- Destructive, one-time reset for the rearchitected bot.
--
-- Dropping the schema rather than named tables also removes anything created
-- outside the migration ledger, so the rebuilt bot starts on a genuinely empty
-- database. Recreating schema_migrations here is required because the ledger
-- lives in `public` and goes down with it; the runner records this migration
-- immediately afterwards, so the purge runs exactly once.
--
-- Nothing may be ordered between the ledger bootstrap and this migration: a
-- migration applied before the purge would be dropped along with its ledger row
-- and re-applied on the next run.

DROP SCHEMA public CASCADE;
CREATE SCHEMA public;
GRANT ALL ON SCHEMA public TO CURRENT_USER;

CREATE TABLE IF NOT EXISTS schema_migrations (
    version TEXT PRIMARY KEY,
    applied_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

INSERT INTO schema_migrations (version)
VALUES ('000_create_schema_migrations')
ON CONFLICT DO NOTHING;
