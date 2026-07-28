//! Unit tests for `lib` (split out to keep the module under 400 lines).

use super::*;
use housebot_llm::ThinkingMode;

#[test]
fn labs_pagination_is_off_by_default() {
    assert!(!UserConfig::default().labs_pagination_enabled);
}

#[test]
fn old_server_config_defaults_to_public_leaderboard() {
    let config: ServerConfig = serde_json::from_str(r#"{"allowed_channel_ids":[123]}"#).unwrap();
    assert_eq!(config.leaderboard_visibility, LeaderboardVisibility::Public);
    assert!(config.leaderboard_role_ids.is_empty());
    assert!(!config.respond_to_bot_pings);
    assert!(config.proactive_allowed);
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
fn old_user_config_defaults_progress_updates_to_enabled() {
    let config: UserConfig =
        serde_json::from_str(r#"{"personality":null,"followup_timeout_secs":300}"#).unwrap();
    assert!(config.progress_updates_enabled);
}

#[test]
fn disabled_progress_updates_persist_through_serde() {
    let config = UserConfig {
        progress_updates_enabled: false,
        ..UserConfig::default()
    };
    let json = serde_json::to_string(&config).unwrap();
    let restored: UserConfig = serde_json::from_str(&json).unwrap();
    assert!(!restored.progress_updates_enabled);
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

#[test]
fn deep_memory_is_on_by_default() {
    assert!(UserConfig::default().deep_memory_enabled);
}

#[test]
fn proactive_assistance_is_off_by_default() {
    assert!(!UserConfig::default().proactive_assistance_enabled);
}

#[test]
fn old_user_config_enables_memory_but_keeps_proactive_assistance_off() {
    let config: UserConfig =
        serde_json::from_str(r#"{"personality":null,"followup_timeout_secs":300}"#).unwrap();
    assert!(config.deep_memory_enabled);
    assert!(!config.proactive_assistance_enabled);
}

#[test]
fn privacy_fields_persist_through_serde() {
    let config = UserConfig {
        deep_memory_enabled: true,
        proactive_assistance_enabled: true,
        ..UserConfig::default()
    };
    let json = serde_json::to_string(&config).unwrap();
    let restored: UserConfig = serde_json::from_str(&json).unwrap();
    assert!(restored.deep_memory_enabled);
    assert!(restored.proactive_assistance_enabled);
}

#[test]
fn owner_is_always_a_configurer_and_always_responded_to() {
    let mut access = AccessControl::default();
    access.user_policies.insert(
        42,
        UserPolicy {
            max_output_tokens: None,
            respond: false,
        },
    );
    assert!(access.is_configurer(42, 42));
    assert!(access.should_respond(42, 42));
    assert!(!access.is_configurer(42, 7));
    assert!(!access.should_respond(42, 7));
}

#[test]
fn unset_owner_id_grants_no_access() {
    let access = AccessControl::default();
    assert!(!access.is_configurer(0, 0));
    assert!(access.should_respond(0, 0));
}

#[test]
fn configurers_bypass_their_own_respond_policy() {
    let mut access = AccessControl::default();
    access.configurer_ids.insert(9);
    access.user_policies.insert(
        9,
        UserPolicy {
            max_output_tokens: Some(512),
            respond: false,
        },
    );
    assert!(access.is_configurer(9, 1));
    assert!(access.should_respond(9, 1));
    assert_eq!(access.policy(9).max_output_tokens, Some(512));
}

#[test]
fn default_policy_responds_with_no_cap() {
    let access = AccessControl::default();
    let policy = access.policy(123);
    assert!(policy.respond);
    assert_eq!(policy.max_output_tokens, None);
}

#[test]
fn access_control_round_trips_through_serde() {
    let mut access = AccessControl::default();
    access.configurer_ids.insert(11);
    access.user_policies.insert(
        22,
        UserPolicy {
            max_output_tokens: Some(2048),
            respond: false,
        },
    );
    let json = serde_json::to_string(&access).unwrap();
    let restored: AccessControl = serde_json::from_str(&json).unwrap();
    assert!(restored.configurer_ids.contains(&11));
    let policy = restored.policy(22);
    assert_eq!(policy.max_output_tokens, Some(2048));
    assert!(!policy.respond);
}

#[test]
fn proactive_is_globally_enabled_by_default_and_for_old_configs() {
    assert!(AccessControl::default().proactive_enabled);
    let access: AccessControl = serde_json::from_str(r#"{"configurer_ids":[1]}"#).unwrap();
    assert!(access.proactive_enabled);
}

#[tokio::test]
async fn access_store_round_trips_on_files() {
    let tmp = tempfile::TempDir::new().unwrap();
    let store = AccessControlStore::new(tmp.path().join("bot_config"));
    assert!(store.load().await.configurer_ids.is_empty());
    let mut access = AccessControl::default();
    access.configurer_ids.insert(5);
    store.save(&access).await.unwrap();
    assert!(store.load().await.configurer_ids.contains(&5));
}

#[tokio::test]
async fn update_persists_mutations_and_reports_outcomes() {
    let tmp = tempfile::TempDir::new().unwrap();
    let store = AccessControlStore::new(tmp.path().join("bot_config"));
    let inserted = store
        .update(|access| access.configurer_ids.insert(7))
        .await
        .unwrap();
    assert!(inserted);
    let inserted = store
        .update(|access| access.configurer_ids.insert(7))
        .await
        .unwrap();
    assert!(!inserted);
    assert!(store.load().await.configurer_ids.contains(&7));
}
