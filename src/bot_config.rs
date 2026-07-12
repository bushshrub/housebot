//! Per-server and per-user configuration, persisted as JSON under DATA_DIR.

use std::collections::HashSet;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use tokio::fs;

use crate::config::data_dir;
use crate::llm::ThinkingMode;

// ── server config ─────────────────────────────────────────────────────────────

/// Configuration scoped to a Discord guild (server).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ServerConfig {
    /// Channel IDs the bot is allowed to respond in. Empty means all channels.
    #[serde(default)]
    pub allowed_channel_ids: HashSet<u64>,
}

pub struct ServerConfigStore {
    dir: PathBuf,
}

impl Default for ServerConfigStore {
    fn default() -> Self {
        Self::new(data_dir().join("server_config"))
    }
}

impl ServerConfigStore {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    fn path(&self, guild_id: u64) -> PathBuf {
        self.dir.join(format!("{guild_id}.json"))
    }

    pub async fn load(&self, guild_id: u64) -> ServerConfig {
        let bytes = fs::read(self.path(guild_id)).await.unwrap_or_default();
        serde_json::from_slice(&bytes).unwrap_or_default()
    }

    pub async fn save(&self, guild_id: u64, cfg: &ServerConfig) -> anyhow::Result<()> {
        fs::create_dir_all(&self.dir).await?;
        let data = serde_json::to_vec_pretty(cfg)?;
        fs::write(self.path(guild_id), data).await?;
        Ok(())
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

// ── user config ───────────────────────────────────────────────────────────────

/// Configuration scoped to an individual Discord user.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserConfig {
    /// Optional personality/tone override injected into the system prompt.
    #[serde(default)]
    pub personality: Option<String>,
    /// Whether the bot should reply to follow-up messages without a ping/mention
    /// in guild channels. DMs enable follow-ups by default.
    #[serde(default)]
    pub followup_enabled: bool,
    /// How many seconds the bot will reply without a ping after the last interaction.
    #[serde(default = "default_followup_timeout")]
    pub followup_timeout_secs: u64,
    /// Whether LLM responses are rendered as paginated embeds.
    #[serde(default)]
    pub labs_pagination_enabled: bool,
    /// Reasoning budget used for this user's requests (set with `/effort`).
    #[serde(default)]
    pub thinking_mode: ThinkingMode,
}

fn default_followup_timeout() -> u64 {
    crate::config::env_parse("CONVERSATION_IDLE_TIMEOUT", 300)
}

impl Default for UserConfig {
    fn default() -> Self {
        Self {
            personality: None,
            followup_enabled: false,
            followup_timeout_secs: default_followup_timeout(),
            labs_pagination_enabled: false,
            thinking_mode: ThinkingMode::default(),
        }
    }
}

pub struct UserConfigStore {
    dir: PathBuf,
}

impl Default for UserConfigStore {
    fn default() -> Self {
        Self::new(data_dir().join("user_config"))
    }
}

impl UserConfigStore {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    fn path(&self, user_id: u64) -> PathBuf {
        self.dir.join(format!("{user_id}.json"))
    }

    pub async fn load(&self, user_id: u64) -> UserConfig {
        let bytes = fs::read(self.path(user_id)).await.unwrap_or_default();
        serde_json::from_slice(&bytes).unwrap_or_default()
    }

    pub async fn save(&self, user_id: u64, cfg: &UserConfig) -> anyhow::Result<()> {
        fs::create_dir_all(&self.dir).await?;
        let data = serde_json::to_vec_pretty(cfg)?;
        fs::write(self.path(user_id), data).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labs_pagination_is_off_by_default() {
        assert!(!UserConfig::default().labs_pagination_enabled);
    }

    #[test]
    fn followup_is_off_by_default() {
        assert!(!UserConfig::default().followup_enabled);
    }

    #[test]
    fn old_user_config_defaults_labs_pagination_to_off() {
        let config: UserConfig =
            serde_json::from_str(r#"{"personality":null,"followup_timeout_secs":300}"#).unwrap();
        assert!(!config.labs_pagination_enabled);
        assert!(!config.followup_enabled);
    }

    #[test]
    fn old_user_config_defaults_thinking_mode_to_medium() {
        let config: UserConfig =
            serde_json::from_str(r#"{"personality":null,"followup_timeout_secs":300}"#).unwrap();
        assert_eq!(config.thinking_mode, ThinkingMode::Medium);
    }

    #[test]
    fn thinking_mode_persists_through_serde() {
        let config = UserConfig {
            thinking_mode: ThinkingMode::XHigh,
            ..UserConfig::default()
        };
        let json = serde_json::to_string(&config).unwrap();
        let restored: UserConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.thinking_mode, ThinkingMode::XHigh);
    }
}
