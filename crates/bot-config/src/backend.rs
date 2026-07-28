//! Storage backend selection: PostgreSQL when configured, JSON files otherwise.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::fs;

pub(crate) const DEFAULT_DATABASE_URL: &str = "postgres://housebot:housebot@postgres/housebot";

// ── storage backend ───────────────────────────────────────────────────────────

#[derive(Clone)]
pub(crate) enum Backend {
    Files(PathBuf),
    Postgres(Arc<tokio_postgres::Client>),
}

pub(crate) fn json_path(dir: &std::path::Path, name: &str) -> PathBuf {
    dir.join(format!("{name}.json"))
}

impl Backend {
    /// `Ok(None)` means the key genuinely has no stored value; storage
    /// failures are propagated so callers can avoid resetting to defaults.
    pub(crate) async fn load(&self, name: &str, key: &str) -> anyhow::Result<Option<Vec<u8>>> {
        match self {
            Backend::Files(dir) => match fs::read(json_path(dir, name)).await {
                Ok(bytes) => Ok(Some(bytes)),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(error) => Err(error.into()),
            },
            Backend::Postgres(client) => match client
                .query_opt("SELECT value FROM bot_config WHERE key = $1", &[&key])
                .await
            {
                Ok(Some(row)) => Ok(Some(row.get::<_, String>(0).into_bytes())),
                Ok(None) => Ok(None),
                Err(error) => {
                    tracing::error!(%error, key, "failed to load bot config");
                    Err(error.into())
                }
            },
        }
    }

    pub(crate) async fn save(&self, name: &str, key: &str, value: String) -> anyhow::Result<()> {
        match self {
            Backend::Files(dir) => {
                fs::create_dir_all(dir).await?;
                fs::write(json_path(dir, name), value).await?;
            }
            Backend::Postgres(client) => {
                client
                    .execute(
                        "INSERT INTO bot_config (key, value) VALUES ($1, $2) \
                         ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value, updated_at = NOW()",
                        &[&key, &value],
                    )
                    .await?;
            }
        }
        Ok(())
    }

    pub(crate) async fn delete(&self, name: &str, key: &str) -> std::io::Result<()> {
        match self {
            Backend::Files(dir) => match fs::remove_file(json_path(dir, name)).await {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(error),
            },
            Backend::Postgres(client) => {
                match client
                    .execute("DELETE FROM bot_config WHERE key = $1", &[&key])
                    .await
                {
                    Ok(_) => Ok(()),
                    Err(error) => {
                        tracing::error!(%error, key, "failed to delete bot config");
                        Err(std::io::Error::other(error))
                    }
                }
            }
        }
    }
}

/// Connect to the deployment's PostgreSQL bot-config storage.
pub async fn postgres_client_from_env() -> anyhow::Result<Arc<tokio_postgres::Client>> {
    let url = housebot_config::env_or("DATABASE_URL", DEFAULT_DATABASE_URL);
    let (client, connection) = tokio_postgres::connect(&url, tokio_postgres::NoTls).await?;
    tokio::spawn(async move {
        if let Err(error) = connection.await {
            tracing::error!(%error, "PostgreSQL bot-config connection closed");
        }
    });
    Ok(Arc::new(client))
}

/// One-time, non-destructive import from the former JSON-file backend.
pub(crate) async fn import_legacy_files(
    client: &tokio_postgres::Client,
    dir: &Path,
    key_prefix: &str,
) {
    let mut entries = match fs::read_dir(dir).await {
        Ok(entries) => entries,
        Err(_) => return,
    };
    while let Ok(Some(entry)) = entries.next_entry().await {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        let Ok(content) = fs::read_to_string(&path).await else {
            continue;
        };
        let key = format!("{key_prefix}{stem}");
        if let Err(error) = client
            .execute(
                "INSERT INTO bot_config (key, value) VALUES ($1, $2) ON CONFLICT DO NOTHING",
                &[&key, &content],
            )
            .await
        {
            tracing::error!(%error, key, "failed to import legacy bot config file");
        }
    }
}
