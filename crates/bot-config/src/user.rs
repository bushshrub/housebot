//! User-scoped configuration and per-user policy.

use std::path::PathBuf;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::backend::*;
use housebot_config::data_dir;
use housebot_llm::ThinkingMode;

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
    /// Whether intermediate reasoning, queue, and tool progress is shown in Discord.
    #[serde(default = "default_progress_updates_enabled")]
    pub progress_updates_enabled: bool,
    /// Whether the bot may use `update_memory` and auto-save conversation summaries.
    /// When disabled, short-term conversation history still works normally.
    #[serde(default = "default_deep_memory_enabled")]
    pub deep_memory_enabled: bool,
    /// Whether the bot may respond proactively to messages it wasn't mentioned in.
    /// Only narrow cases are handled (obvious reminder requests, help questions).
    #[serde(default)]
    pub proactive_assistance_enabled: bool,
    /// Names of global marketplace skills this user has enabled. Only enabled
    /// skills are listed in the user's prompt and executable via `use_skill`.
    #[serde(default)]
    pub enabled_skills: Vec<String>,
}

pub(crate) fn default_followup_timeout() -> u64 {
    housebot_config::env_parse("CONVERSATION_IDLE_TIMEOUT", 300)
}

pub(crate) fn default_deep_memory_enabled() -> bool {
    true
}

pub(crate) fn default_progress_updates_enabled() -> bool {
    true
}

impl Default for UserConfig {
    fn default() -> Self {
        Self {
            personality: None,
            followup_enabled: false,
            followup_timeout_secs: default_followup_timeout(),
            labs_pagination_enabled: false,
            thinking_mode: ThinkingMode::default(),
            progress_updates_enabled: true,
            deep_memory_enabled: true,
            proactive_assistance_enabled: false,
            enabled_skills: Vec::new(),
        }
    }
}

#[derive(Clone)]
pub struct UserConfigStore {
    backend: Backend,
}

impl Default for UserConfigStore {
    fn default() -> Self {
        Self::new(data_dir().join("user_config"))
    }
}

impl UserConfigStore {
    pub fn new(dir: PathBuf) -> Self {
        Self {
            backend: Backend::Files(dir),
        }
    }

    /// Database-backed store; imports any legacy JSON files once.
    pub async fn postgres(client: Arc<tokio_postgres::Client>) -> Self {
        import_legacy_files(&client, &data_dir().join("user_config"), "user:").await;
        Self {
            backend: Backend::Postgres(client),
        }
    }

    pub async fn load(&self, user_id: u64) -> UserConfig {
        let bytes = self
            .backend
            .load(&user_id.to_string(), &format!("user:{user_id}"))
            .await
            .ok()
            .flatten()
            .unwrap_or_default();
        serde_json::from_slice(&bytes).unwrap_or_default()
    }

    pub async fn save(&self, user_id: u64, cfg: &UserConfig) -> anyhow::Result<()> {
        let data = serde_json::to_string_pretty(cfg)?;
        self.backend
            .save(&user_id.to_string(), &format!("user:{user_id}"), data)
            .await
    }

    pub async fn clear(&self, user_id: u64) -> std::io::Result<()> {
        self.backend
            .delete(&user_id.to_string(), &format!("user:{user_id}"))
            .await
    }
}

// ── access control ────────────────────────────────────────────────────────────

/// Per-user policy set by the bot's configurers.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct UserPolicy {
    /// Cap on `max_tokens` for this user's completions. `None` means no cap.
    #[serde(default)]
    pub max_output_tokens: Option<u32>,
    /// Whether the bot responds to this user's messages at all.
    #[serde(default = "default_respond")]
    pub respond: bool,
}

pub(crate) fn default_respond() -> bool {
    true
}

impl Default for UserPolicy {
    fn default() -> Self {
        Self {
            max_output_tokens: None,
            respond: true,
        }
    }
}
