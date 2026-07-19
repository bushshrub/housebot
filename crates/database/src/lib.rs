//! Ordered, append-only PostgreSQL migrations.

use std::time::Duration;

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
    (
        "004_create_deployment_permissions",
        include_str!("../../../db/migrations/004_create_deployment_permissions.sql"),
    ),
];
const MIGRATION_LOCK_ID: i64 = 1_593_778_914;
const DEFAULT_DATABASE_URL: &str = "postgres://housebot:housebot@postgres/housebot";
const DEFAULT_CONNECT_ATTEMPTS: usize = 10;
const DEFAULT_CONNECT_RETRY_SECS: u64 = 2;
const DEFAULT_CONNECT_TIMEOUT_SECS: u64 = 10;

/// Apply every unapplied migration from the deployment migration command.
pub async fn migrate_from_env() -> anyhow::Result<()> {
    let url = config::env_or("DATABASE_URL", DEFAULT_DATABASE_URL);
    let attempts =
        config::env_parse("DATABASE_CONNECT_MAX_ATTEMPTS", DEFAULT_CONNECT_ATTEMPTS).max(1);
    let retry_delay = Duration::from_secs(config::env_parse(
        "DATABASE_CONNECT_RETRY_SECS",
        DEFAULT_CONNECT_RETRY_SECS,
    ));
    let attempt_timeout = Duration::from_secs(
        config::env_parse(
            "DATABASE_CONNECT_TIMEOUT_SECS",
            DEFAULT_CONNECT_TIMEOUT_SECS,
        )
        .max(1),
    );
    let (client, connection) =
        connect_with_retry(&url, attempts, retry_delay, attempt_timeout).await?;
    tokio::spawn(async move {
        if let Err(error) = connection.await {
            tracing::error!(%error, "PostgreSQL migration connection closed");
        }
    });
    migrate(&client).await
}

async fn connect_with_retry(
    url: &str,
    attempts: usize,
    retry_delay: Duration,
    attempt_timeout: Duration,
) -> anyhow::Result<(
    Client,
    tokio_postgres::Connection<tokio_postgres::Socket, tokio_postgres::tls::NoTlsStream>,
)> {
    let mut last_error = None;
    for attempt in 1..=attempts {
        let result =
            tokio::time::timeout(attempt_timeout, tokio_postgres::connect(url, NoTls)).await;
        match result {
            Ok(Ok(connection)) => return Ok(connection),
            Ok(Err(error)) => last_error = Some(error.to_string()),
            Err(_) => {
                last_error = Some(format!(
                    "connection attempt timed out after {attempt_timeout:?}"
                ))
            }
        }
        tracing::warn!(
            attempt,
            attempts,
            error = %last_error.as_deref().expect("failed attempt records an error"),
            "PostgreSQL migration connection failed"
        );
        if attempt < attempts && !retry_delay.is_zero() {
            tokio::time::sleep(retry_delay).await;
        }
    }
    Err(anyhow::anyhow!(
        "could not connect for database migrations after {attempts} attempt(s): {}",
        last_error.expect("at least one connection attempt ran")
    ))
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
    use super::{connect_with_retry, MIGRATIONS};
    use std::time::Duration;

    #[tokio::test]
    async fn connection_failure_is_returned_instead_of_hanging_forever() {
        let result = connect_with_retry(
            "not-a-postgres-url",
            2,
            Duration::ZERO,
            Duration::from_secs(1),
        )
        .await;
        let Err(error) = result else {
            panic!("invalid database URL unexpectedly connected");
        };
        assert!(error.to_string().contains("after 2 attempt(s)"));
    }

    #[tokio::test]
    async fn stalled_connection_attempt_is_bounded_by_timeout() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (_stream, _) = listener.accept().await.unwrap();
            std::future::pending::<()>().await;
        });
        let url = format!("postgres://housebot:housebot@{address}/housebot");

        let result = connect_with_retry(&url, 1, Duration::ZERO, Duration::from_millis(100)).await;
        server.abort();

        let Err(error) = result else {
            panic!("stalled PostgreSQL handshake unexpectedly connected");
        };
        assert!(error.to_string().contains("timed out"));
    }

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

    #[test]
    fn deployment_permissions_are_created_by_an_ordered_migration() {
        let (_, sql) = MIGRATIONS
            .iter()
            .find(|(version, _)| *version == "004_create_deployment_permissions")
            .expect("deployment permissions migration must exist");
        assert!(sql.contains("CREATE TABLE IF NOT EXISTS deployment_permissions"));
        assert!(sql.contains("user_id BIGINT PRIMARY KEY"));
    }
}
