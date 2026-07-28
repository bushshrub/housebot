//! Unit tests for `lib` (split out to keep the module under 400 lines).

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
fn record_tool_use_saturates_instead_of_overflowing() {
    let mut profile = UserProfile::default();
    profile
        .action_counts
        .insert("web research".to_string(), u64::MAX);
    profile.record_tool_use("web_search");
    assert_eq!(profile.action_counts["web research"], u64::MAX);
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
    assert_eq!(
        tool_to_tag("edit_feature_request"),
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
