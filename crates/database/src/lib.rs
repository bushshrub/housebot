//! Ordered, append-only PostgreSQL migrations.

use anyhow::Context;
use tokio_postgres::{Client, NoTls};

use housebot_config as config;

const MIGRATIONS: &[(&str, &str)] = &[
    (
        "000_create_schema_migrations",
        include_str!("../../../db/migrations/000_create_schema_migrations.sql"),
    ),
    (
        "001_create_user_memories",
        include_str!("../../../db/migrations/001_create_user_memories.sql"),
    ),
    (
        "002_create_token_monitor",
        include_str!("../../../db/migrations/002_create_token_monitor.sql"),
    ),
    (
        "003_create_bot_config",
        include_str!("../../../db/migrations/003_create_bot_config.sql"),
    ),
];
const MIGRATION_LOCK_ID: i64 = 1_593_778_914;
const DEFAULT_DATABASE_URL: &str = "postgres://housebot:housebot@postgres/housebot";

/// Apply every unapplied migration from the deployment migration command.
pub async fn migrate_from_env() -> anyhow::Result<()> {
    let url = config::env_or("DATABASE_URL", DEFAULT_DATABASE_URL);
    let (client, connection) = tokio_postgres::connect(&url, NoTls)
        .await
        .context("connect for database migrations")?;
    tokio::spawn(async move {
        if let Err(error) = connection.await {
            tracing::error!(%error, "PostgreSQL migration connection closed");
        }
    });
    migrate(&client).await
}

async fn migrate(client: &Client) -> anyhow::Result<()> {
    let (ledger_version, ledger_sql) = MIGRATIONS
        .first()
        .expect("database migration ledger bootstrap must exist");
    client
        .query_one("SELECT pg_advisory_lock($1)", &[&MIGRATION_LOCK_ID])
        .await
        .context("acquire database migration lock")?;

    let result = bootstrap_and_apply_migrations(client, ledger_version, ledger_sql).await;
    let unlock_result = client
        .query_one("SELECT pg_advisory_unlock($1)", &[&MIGRATION_LOCK_ID])
        .await;
    result?;
    unlock_result.context("release database migration lock")?;
    Ok(())
}

async fn bootstrap_and_apply_migrations(
    client: &Client,
    ledger_version: &str,
    ledger_sql: &str,
) -> anyhow::Result<()> {
    client
        .batch_execute(ledger_sql)
        .await
        .with_context(|| format!("apply database migration {ledger_version}"))?;
    client
        .execute(
            "INSERT INTO schema_migrations (version) VALUES ($1) ON CONFLICT DO NOTHING",
            &[&ledger_version],
        )
        .await
        .with_context(|| format!("record database migration {ledger_version}"))?;
    apply_migrations(client).await
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
        let (_, token_monitor) = MIGRATIONS
            .iter()
            .find(|(version, _)| *version == "002_create_token_monitor")
            .expect("token-monitor migration must exist");
        let indexes = token_monitor
            .lines()
            .map(str::trim_start)
            .filter(|line| line.starts_with("CREATE INDEX"))
            .collect::<Vec<_>>();
        assert_eq!(
            indexes.len(),
            5,
            "all token-monitor indexes must be checked"
        );
        for index in indexes {
            assert!(
                index.contains(" ON "),
                "index statement must contain ON: {index}"
            );
        }
    }
}
