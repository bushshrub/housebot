//! Ordered, append-only PostgreSQL migrations.

use anyhow::Context;
use tokio_postgres::Client;

const MIGRATIONS: &[(&str, &str)] = &[
    (
        "001_create_user_memories",
        include_str!("../db/migrations/001_create_user_memories.sql"),
    ),
    (
        "002_create_token_monitor",
        include_str!("../db/migrations/002_create_token_monitor.sql"),
    ),
];
const MIGRATION_LOCK_ID: i64 = 1_593_778_914;

/// Apply every unapplied migration in order without altering existing data.
pub(crate) async fn migrate(client: &Client) -> anyhow::Result<()> {
    client
        .batch_execute(
            "CREATE TABLE IF NOT EXISTS schema_migrations (\
                version TEXT PRIMARY KEY,\
                applied_at TIMESTAMPTZ NOT NULL DEFAULT NOW()\
            )",
        )
        .await
        .context("create database migration ledger")?;
    client
        .query_one("SELECT pg_advisory_lock($1)", &[&MIGRATION_LOCK_ID])
        .await
        .context("acquire database migration lock")?;

    let result = apply_migrations(client).await;
    let unlock_result = client
        .query_one("SELECT pg_advisory_unlock($1)", &[&MIGRATION_LOCK_ID])
        .await;
    result?;
    unlock_result.context("release database migration lock")?;
    Ok(())
}

async fn apply_migrations(client: &Client) -> anyhow::Result<()> {
    for (version, sql) in MIGRATIONS {
        if client
            .query_opt(
                "SELECT 1 FROM schema_migrations WHERE version = $1",
                &[version],
            )
            .await
            .with_context(|| format!("check database migration {version}"))?
            .is_some()
        {
            continue;
        }
        client
            .batch_execute(sql)
            .await
            .with_context(|| format!("apply database migration {version}"))?;
        client
            .execute(
                "INSERT INTO schema_migrations (version) VALUES ($1)",
                &[version],
            )
            .await
            .with_context(|| format!("record database migration {version}"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::MIGRATIONS;

    #[test]
    fn migrations_are_ordered_and_token_indexes_are_valid() {
        assert!(MIGRATIONS.windows(2).all(|pair| pair[0].0 < pair[1].0));
        let (_, token_monitor) = MIGRATIONS[1];
        for index in token_monitor
            .lines()
            .filter(|line| line.starts_with("CREATE INDEX"))
        {
            assert!(
                index.contains(" ON "),
                "index statement must contain ON: {index}"
            );
        }
    }
}
