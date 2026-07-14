//! Per-user profile store, persisted as JSON under `data/profiles/<user_id>.json`.
//!
//! Tracks Discord display information, learned profile tags, and tool-usage
//! counters that drive quick-action suggestions.

use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::config;
use crate::memory::ensure_dir;

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
        "web_search" | "fetch_webpage" | "common_crawl__search" | "summarize_url" => {
            Some(ProfileTag::WebResearch)
        }
        "update_memory" => Some(ProfileTag::Coding),
        "create_feature_request" | "prepare_feature_development" => Some(ProfileTag::Coding),
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
            *count += 1;
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
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn store() -> (TempDir, ProfileStore) {
        let tmp = TempDir::new().unwrap();
        let s = ProfileStore::new(tmp.path().join("profiles"));
        (tmp, s)
    }

    #[tokio::test]
    async fn load_returns_default_for_unknown_user() {
        let (_t, s) = store();
        let p = s.load("unknown").await;
        assert_eq!(p.username, "");
        assert!(p.tags.is_empty());
    }

    #[tokio::test]
    async fn save_and_load_roundtrip() {
        let (_t, s) = store();
        let profile = UserProfile {
            username: "alice".into(),
            display_name: "Alice".into(),
            nickname: "Ali".into(),
            avatar_url: "https://example.com/avatar.png".into(),
            ..Default::default()
        };
        s.save("123", &profile).await.unwrap();

        let loaded = s.load("123").await;
        assert_eq!(loaded.username, "alice");
        assert_eq!(loaded.display_name, "Alice");
        assert_eq!(loaded.nickname, "Ali");
        assert_eq!(loaded.avatar_url, "https://example.com/avatar.png");
    }

    #[tokio::test]
    async fn clear_removes_profile() {
        let (_t, s) = store();
        let profile = UserProfile {
            username: "bob".into(),
            ..UserProfile::default()
        };
        s.save("456", &profile).await.unwrap();
        assert_eq!(s.load("456").await.username, "bob");
        s.clear("456").await.unwrap();
        assert_eq!(s.load("456").await.username, "");
    }

    #[tokio::test]
    async fn clear_noop_for_unknown_user() {
        let (_t, s) = store();
        s.clear("never_existed").await.unwrap();
    }

    #[test]
    fn best_name_prefers_nickname() {
        let p = UserProfile {
            nickname: "Nick".into(),
            display_name: "Display".into(),
            username: "user".into(),
            ..UserProfile::default()
        };
        assert_eq!(p.best_name(), "Nick");
    }

    #[test]
    fn best_name_falls_back_to_display_name() {
        let p = UserProfile {
            display_name: "Display".into(),
            username: "user".into(),
            ..UserProfile::default()
        };
        assert_eq!(p.best_name(), "Display");
    }

    #[test]
    fn best_name_falls_back_to_username() {
        let p = UserProfile {
            username: "user".into(),
            ..UserProfile::default()
        };
        assert_eq!(p.best_name(), "user");
    }

    #[test]
    fn best_name_defaults_to_user() {
        let p = UserProfile::default();
        assert_eq!(p.best_name(), "User");
    }

    #[test]
    fn record_tool_use_updates_counts_and_tags() {
        let mut p = UserProfile::default();
        p.record_tool_use("web_search");
        assert!(p.tags.contains(&ProfileTag::WebResearch));
        assert_eq!(p.action_counts.get("web research"), Some(&1));

        p.record_tool_use("web_search");
        assert_eq!(p.action_counts.get("web research"), Some(&2));
        assert_eq!(
            p.tags
                .iter()
                .filter(|t| **t == ProfileTag::WebResearch)
                .count(),
            1
        );
    }

    #[test]
    fn record_tool_use_jellyfin() {
        let mut p = UserProfile::default();
        p.record_tool_use("jellyfin__get_movies");
        assert!(p.tags.contains(&ProfileTag::Media));
        assert_eq!(p.action_counts.get("media"), Some(&1));
    }

    #[test]
    fn record_tool_use_unknown_tool_is_noop() {
        let mut p = UserProfile::default();
        p.record_tool_use("unknown_tool");
        assert!(p.tags.is_empty());
        assert!(p.action_counts.is_empty());
    }

    #[test]
    fn quick_actions_sorted_by_count() {
        let mut p = UserProfile::default();
        p.action_counts.insert("web research".into(), 5);
        p.action_counts.insert("media".into(), 10);
        p.action_counts.insert("reminders".into(), 3);
        let actions = p.quick_actions();
        assert_eq!(actions[0], ("media", 10));
        assert_eq!(actions[1], ("web research", 5));
        assert_eq!(actions[2], ("reminders", 3));
    }

    #[test]
    fn clear_learned_removes_tags_and_counts() {
        let mut p = UserProfile {
            username: "alice".into(),
            display_name: "Alice".into(),
            ..UserProfile::default()
        };
        p.record_tool_use("web_search");
        assert!(!p.tags.is_empty());
        p.clear_learned();
        assert!(p.tags.is_empty());
        assert!(p.action_counts.is_empty());
        assert_eq!(p.username, "alice");
    }

    #[test]
    fn tool_to_tag_mapping() {
        assert_eq!(tool_to_tag("web_search"), Some(ProfileTag::WebResearch));
        assert_eq!(tool_to_tag("fetch_webpage"), Some(ProfileTag::WebResearch));
        assert_eq!(tool_to_tag("summarize_url"), Some(ProfileTag::WebResearch));
        assert_eq!(tool_to_tag("set_reminder"), Some(ProfileTag::Reminders));
        assert_eq!(tool_to_tag("translate"), Some(ProfileTag::Translation));
        assert_eq!(tool_to_tag("jellyfin__get_movies"), Some(ProfileTag::Media));
        assert_eq!(
            tool_to_tag("create_feature_request"),
            Some(ProfileTag::Coding)
        );
        assert_eq!(tool_to_tag("random_tool"), None);
    }

    #[test]
    fn profile_tag_as_str() {
        assert_eq!(ProfileTag::Coding.as_str(), "coding");
        assert_eq!(ProfileTag::Media.as_str(), "media");
        assert_eq!(ProfileTag::WebResearch.as_str(), "web research");
        assert_eq!(ProfileTag::Reminders.as_str(), "reminders");
        assert_eq!(ProfileTag::Translation.as_str(), "translation");
    }

    #[tokio::test]
    async fn profile_persists_tags_through_serde() {
        let (_t, s) = store();
        let profile = UserProfile {
            tags: vec![ProfileTag::Media, ProfileTag::WebResearch],
            ..Default::default()
        };
        s.save("789", &profile).await.unwrap();
        let loaded = s.load("789").await;
        assert_eq!(
            loaded.tags,
            vec![ProfileTag::Media, ProfileTag::WebResearch]
        );
    }

    #[tokio::test]
    async fn old_profile_file_defaults_missing_fields() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("profiles");
        tokio::fs::create_dir_all(&path).await.unwrap();
        tokio::fs::write(path.join("100.json"), r#"{"username":"old_user"}"#)
            .await
            .unwrap();
        let s = ProfileStore::new(path);
        let p = s.load("100").await;
        assert_eq!(p.username, "old_user");
        assert!(p.tags.is_empty());
        assert!(p.action_counts.is_empty());
    }
}
