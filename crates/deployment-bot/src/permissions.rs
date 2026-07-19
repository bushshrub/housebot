use std::sync::Arc;

use anyhow::Context;
use tokio_postgres::{Client, NoTls};

const DEFAULT_DATABASE_URL: &str = "postgres://housebot:housebot@postgres/housebot";

#[derive(Clone)]
pub(crate) struct DeploymentPermissions {
    client: Arc<Client>,
}

impl DeploymentPermissions {
    pub(crate) async fn connect() -> anyhow::Result<Self> {
        housebot_database::migrate_from_env().await?;
        let url = housebot_config::env_or("DATABASE_URL", DEFAULT_DATABASE_URL);
        let (client, connection) = tokio_postgres::connect(&url, NoTls)
            .await
            .context("connect deployment permissions database")?;
        tokio::spawn(async move {
            if let Err(error) = connection.await {
                tracing::error!(%error, "Deployment permissions database connection closed");
            }
        });
        Ok(Self {
            client: Arc::new(client),
        })
    }

    pub(crate) async fn contains(&self, user_id: u64) -> anyhow::Result<bool> {
        let user_id = discord_id(user_id)?;
        Ok(self
            .client
            .query_opt(
                "SELECT 1 FROM deployment_permissions WHERE user_id = $1",
                &[&user_id],
            )
            .await
            .context("check deployment permission")?
            .is_some())
    }

    pub(crate) async fn allow(&self, user_id: u64, owner_id: u64) -> anyhow::Result<()> {
        let user_id = discord_id(user_id)?;
        let owner_id = discord_id(owner_id)?;
        self.client
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
        let user_id = discord_id(user_id)?;
        self.client
            .execute(
                "DELETE FROM deployment_permissions WHERE user_id = $1",
                &[&user_id],
            )
            .await
            .context("revoke deployment permission")?;
        Ok(())
    }

    pub(crate) async fn list(&self) -> anyhow::Result<Vec<u64>> {
        self.client
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
