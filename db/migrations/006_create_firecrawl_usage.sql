-- `crates/tools` (firecrawl.rs) counts the Firecrawl credits the bot spends per
-- UTC calendar month, so the keyless tier's shared monthly allowance is not
-- used up by the bot alone.

CREATE TABLE IF NOT EXISTS firecrawl_usage (
    month DATE PRIMARY KEY,
    credits INTEGER NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
