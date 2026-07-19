use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use tokio::sync::Mutex;
use tokio_postgres::{Client, NoTls};

const DEFAULT_DATABASE_URL: &str = "postgres://housebot:housebot@postgres/housebot";
const RECONNECT_BASE_SECS: u64 = 2;
const RECONNECT_MAX_SECS: u64 = 60;

#[derive(Clone)]
pub(crate) struct DeploymentPermissions {
    client: Arc<Mutex<Option<Client>>>,
    database_url: String,
}

impl DeploymentPermissions {
    pub(crate) async fn connect() -> Self {
        if let Err(error) = housebot_database::migrate_from_env().await {
            tracing::warn!(%error, "Could not run database migrations for deployment permissions; deferring to owner-only authorization");
        }
        let url = housebot_config::env_or("DATABASE_URL", DEFAULT_DATABASE_URL);
        let client = match Self::new_client(&url).await {
            Ok(client) => {
                tracing::info!("Connected to deployment permissions database");
                Some(client)
            }
            Err(error) => {
                tracing::warn!(%error, "Could not connect to deployment permissions database; falling back to owner-only authorization");
                None
            }
        };
        let permissions = Self {
            client: Arc::new(Mutex::new(client)),
            database_url: url,
        };
        permissions.clone().spawn_reconnector();
        permissions
    }

    async fn new_client(url: &str) -> anyhow::Result<Client> {
        let (client, connection) = tokio_postgres::connect(url, NoTls)
            .await
            .context("connect deployment permissions database")?;
        tokio::spawn(connection);
        Ok(client)
    }

    fn spawn_reconnector(self) {
        tokio::spawn(async move {
            let mut delay = RECONNECT_BASE_SECS;
            loop {
                tokio::time::sleep(Duration::from_secs(delay)).await;
                let mut guard = self.client.lock().await;
                if guard.is_some() {
                    // Health check on the existing connection
                    match guard.as_ref().unwrap().query_one("SELECT 1", &[]).await {
                        Ok(_) => {
                            delay = RECONNECT_BASE_SECS;
                            continue;
                        }
                        Err(_) => {
                            tracing::warn!("Deployment permissions database connection lost; attempting reconnection");
                            *guard = None;
                        }
                    }
                }
                drop(guard);
                match Self::new_client(&self.database_url).await {
                    Ok(client) => {
                        tracing::info!("Reconnected to deployment permissions database");
                        *self.client.lock().await = Some(client);
                        delay = RECONNECT_BASE_SECS;
                    }
                    Err(error) => {
                        tracing::warn!(%error, "Could not reconnect to deployment permissions database; retrying in {delay}s");
                        delay = (delay * 2).min(RECONNECT_MAX_SECS);
                    }
                }
            }
        });
    }

    pub(crate) async fn contains(&self, user_id: u64) -> anyhow::Result<bool> {
        let client = self.client.lock().await;
        match client.as_ref() {
            Some(client) => {
                let user_id = discord_id(user_id)?;
                Ok(client
                    .query_opt(
                        "SELECT 1 FROM deployment_permissions WHERE user_id = $1",
                        &[&user_id],
                    )
                    .await
                    .context("check deployment permission")?
                    .is_some())
            }
            None => Ok(false),
        }
    }

    pub(crate) async fn allow(&self, user_id: u64, owner_id: u64) -> anyhow::Result<()> {
        let client = self.client.lock().await;
        let client = client.as_ref().ok_or_else(|| {
            anyhow::anyhow!("Database is not available; cannot grant deployment access")
        })?;
        let user_id = discord_id(user_id)?;
        let owner_id = discord_id(owner_id)?;
        client
            .execute(
                "INSERT INTO deployment_permissions (user_id, granted_by) VALUES ($1, $2) \
                 ON CONFLICT (user_id) DO UPDATE SET granted_by = EXCLUDED.granted_by, granted_at = NOW()",
                &[&user_id, &owner_id],
            )
            .await
            .context("grant deployment permission")?;
        Ok(())
    }

    pub(crate) async fn revoke(&self, user_id: u64) -> anyhow::Result<()> {
        let client = self.client.lock().await;
        let client = client.as_ref().ok_or_else(|| {
            anyhow::anyhow!("Database is not available; cannot revoke deployment access")
        })?;
        let user_id = discord_id(user_id)?;
        client
            .execute(
                "DELETE FROM deployment_permissions WHERE user_id = $1",
                &[&user_id],
            )
            .await
            .context("revoke deployment permission")?;
        Ok(())
    }

    pub(crate) async fn list(&self) -> anyhow::Result<Vec<u64>> {
        let client = self.client.lock().await;
        let client = client.as_ref().ok_or_else(|| {
            anyhow::anyhow!("Database is not available; cannot list deployment access")
        })?;
        client
            .query(
                "SELECT user_id FROM deployment_permissions ORDER BY user_id",
                &[],
            )
            .await
            .context("list deployment permissions")?
            .into_iter()
            .map(|row| {
                let id: i64 = row.get(0);
                u64::try_from(id).context("deployment permission contains a negative user ID")
            })
            .collect()
    }
}

fn discord_id(id: u64) -> anyhow::Result<i64> {
    i64::try_from(id).context("Discord user ID is too large for the database")
}
