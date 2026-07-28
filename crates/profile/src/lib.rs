//! Per-user profile store, persisted as JSON under `data/profiles/<user_id>.json`.
//!
//! Tracks Discord display information, learned profile tags, and tool-usage
//! counters that drive quick-action suggestions.

use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use housebot_config as config;
use housebot_memory::ensure_dir;

/// Profile tags that describe the user's bot-usage patterns.
/// These are ordinary, non-sensitive categories derived from tool usage.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfileTag {
    Coding,
    Media,
    WebResearch,
    Reminders,
    Translation,
}

impl ProfileTag {
    pub fn as_str(&self) -> &'static str {
        match self {
            ProfileTag::Coding => "coding",
            ProfileTag::Media => "media",
            ProfileTag::WebResearch => "web research",
            ProfileTag::Reminders => "reminders",
            ProfileTag::Translation => "translation",
        }
    }
}

/// Map a tool name to the profile tag it contributes to.
pub fn tool_to_tag(tool_name: &str) -> Option<ProfileTag> {
    match tool_name {
        "web_search"
        | "deep_research"
        | "fetch_webpage"
        | "download_file"
        | "common_crawl__search"
        | "summarize_url" => Some(ProfileTag::WebResearch),
        "update_memory" => Some(ProfileTag::Coding),
        "create_feature_request" | "edit_feature_request" | "prepare_feature_development" => {
            Some(ProfileTag::Coding)
        }
        "set_reminder" => Some(ProfileTag::Reminders),
        "translate" => Some(ProfileTag::Translation),
        name if name.starts_with("jellyfin__") => Some(ProfileTag::Media),
        _ => None,
    }
}

/// Per-user profile stored on disk.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UserProfile {
    /// Discord username (global).
    #[serde(default)]
    pub username: String,
    /// Discord display name.
    #[serde(default)]
    pub display_name: String,
    /// Guild-specific nickname (empty if none or unknown).
    #[serde(default)]
    pub nickname: String,
    /// Avatar URL from Discord.
    #[serde(default)]
    pub avatar_url: String,
    /// Guild ID where the nickname was observed (0 if DM or unknown).
    #[serde(default)]
    pub guild_id: u64,
    /// Learned profile tags derived from tool usage.
    #[serde(default)]
    pub tags: Vec<ProfileTag>,
    /// Per-tag usage counter. Keys are the serialized tag name.
    #[serde(default)]
    pub action_counts: HashMap<String, u64>,
}

impl UserProfile {
    /// Return the best name to address the user by.
    pub fn best_name(&self) -> &str {
        if !self.nickname.is_empty() {
            &self.nickname
        } else if !self.display_name.is_empty() {
            &self.display_name
        } else if !self.username.is_empty() {
            &self.username
        } else {
            "User"
        }
    }

    /// Record that a tool was used, updating action counts and tags.
    pub fn record_tool_use(&mut self, tool_name: &str) {
        if let Some(tag) = tool_to_tag(tool_name) {
            let key = tag.as_str().to_string();
            let count = self.action_counts.entry(key).or_insert(0);
            *count = count.saturating_add(1);
            // Add the tag if it's not already present and we've seen it at least once.
            if !self.tags.contains(&tag) {
                self.tags.push(tag);
            }
        }
    }

    /// Return the top quick actions sorted by usage count (descending).
    pub fn quick_actions(&self) -> Vec<(&str, u64)> {
        let mut actions: Vec<(&str, u64)> = self
            .action_counts
            .iter()
            .map(|(k, &v)| (k.as_str(), v))
            .collect();
        actions.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));
        actions
    }

    /// Clear learned data (tags and counters) while keeping Discord identity.
    pub fn clear_learned(&mut self) {
        self.tags.clear();
        self.action_counts.clear();
    }
}

/// Handle to the per-user profile store.
#[derive(Clone)]
pub struct ProfileStore {
    dir: PathBuf,
}

impl Default for ProfileStore {
    fn default() -> Self {
        Self::new(config::data_dir().join("profiles"))
    }
}

impl ProfileStore {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    fn path(&self, user_id: impl std::fmt::Display) -> PathBuf {
        self.dir.join(format!("{user_id}.json"))
    }

    /// Load a user's profile, returning a default one when none exists.
    pub async fn load(&self, user_id: impl std::fmt::Display) -> UserProfile {
        let bytes = match tokio::fs::read(self.path(user_id)).await {
            Ok(b) => b,
            Err(_) => return UserProfile::default(),
        };
        serde_json::from_slice(&bytes).unwrap_or_default()
    }

    /// Save a user's profile.
    pub async fn save(
        &self,
        user_id: impl std::fmt::Display,
        profile: &UserProfile,
    ) -> std::io::Result<()> {
        ensure_dir(&self.dir).await?;
        let data = serde_json::to_vec_pretty(profile).unwrap_or_else(|_| b"{}".to_vec());
        tokio::fs::write(self.path(user_id), data).await
    }

    /// Delete a user's profile (no-op when it does not exist).
    pub async fn clear(&self, user_id: impl std::fmt::Display) -> std::io::Result<()> {
        match tokio::fs::remove_file(self.path(user_id)).await {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        }
    }
}

#[cfg(test)]
#[path = "lib_tests.rs"]
mod tests;
