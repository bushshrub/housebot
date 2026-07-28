//! Guild-scoped configuration.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::backend::*;
use crate::user::default_respond;
use housebot_config::data_dir;

/// Configuration scoped to a Discord guild (server).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    /// Channel IDs the bot is allowed to respond in. Empty means all channels.
    #[serde(default)]
    pub allowed_channel_ids: HashSet<u64>,
    /// Who can view the server token leaderboard and whether the response is public.
    #[serde(default)]
    pub leaderboard_visibility: LeaderboardVisibility,
    /// Roles allowed to view the leaderboard when visibility is restricted.
    #[serde(default)]
    pub leaderboard_role_ids: HashSet<u64>,
    /// Whether to respond to @-mentions from other bots in this server.
    /// The bot always ignores its own pings regardless.
    #[serde(default)]
    pub respond_to_bot_pings: bool,
    /// Whether proactive assistance is allowed in this server at all.
    /// Users still opt in individually via `/personalize proactive`.
    #[serde(default = "default_respond")]
    pub proactive_allowed: bool,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            allowed_channel_ids: HashSet::new(),
            leaderboard_visibility: LeaderboardVisibility::default(),
            leaderboard_role_ids: HashSet::new(),
            respond_to_bot_pings: false,
            proactive_allowed: true,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LeaderboardVisibility {
    #[default]
    Public,
    Private,
    Restricted,
}

impl LeaderboardVisibility {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Public => "public",
            Self::Private => "private",
            Self::Restricted => "restricted",
        }
    }
}

#[derive(Clone)]
pub struct ServerConfigStore {
    backend: Backend,
}

impl Default for ServerConfigStore {
    fn default() -> Self {
        Self::new(data_dir().join("server_config"))
    }
}

impl ServerConfigStore {
    pub fn new(dir: PathBuf) -> Self {
        Self {
            backend: Backend::Files(dir),
        }
    }

    /// Database-backed store; imports any legacy JSON files once.
    pub async fn postgres(client: Arc<tokio_postgres::Client>) -> Self {
        import_legacy_files(&client, &data_dir().join("server_config"), "server:").await;
        Self {
            backend: Backend::Postgres(client),
        }
    }

    pub async fn load(&self, guild_id: u64) -> ServerConfig {
        match self
            .backend
            .load(&guild_id.to_string(), &format!("server:{guild_id}"))
            .await
        {
            Ok(Some(bytes)) => serde_json::from_slice(&bytes).unwrap_or_default(),
            Ok(None) => ServerConfig::default(),
            Err(_) => ServerConfig::default(),
        }
    }

    pub async fn save(&self, guild_id: u64, cfg: &ServerConfig) -> anyhow::Result<()> {
        let data = serde_json::to_string_pretty(cfg)?;
        self.backend
            .save(&guild_id.to_string(), &format!("server:{guild_id}"), data)
            .await
    }

    /// Returns true if the channel is allowed (or if no restrictions are set).
    pub async fn is_channel_allowed(&self, guild_id: Option<u64>, channel_id: u64) -> bool {
        let Some(gid) = guild_id else {
            return true; // DMs are always allowed
        };
        let cfg = self.load(gid).await;
        cfg.allowed_channel_ids.is_empty() || cfg.allowed_channel_ids.contains(&channel_id)
    }

    /// Follow-ups require an explicitly configured server channel.
    pub async fn is_followup_channel_allowed(
        &self,
        guild_id: Option<u64>,
        channel_id: u64,
    ) -> bool {
        let Some(gid) = guild_id else {
            return false;
        };
        self.load(gid)
            .await
            .allowed_channel_ids
            .contains(&channel_id)
    }
}
