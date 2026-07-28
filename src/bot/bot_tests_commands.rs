//! bot tests commands.

//! Unit tests for `bot` (split out to keep the module under 600 lines).

use super::bot_tests_support::*;
use super::*;
use crate::profile::{ProfileTag, UserProfile};
use serde_json::json;
use tempfile::TempDir;

#[test]
fn ordinary_command_responses_are_private() {
    assert!(command_response_is_ephemeral("token_leaderboard"));
    assert!(command_response_is_ephemeral("config"));
    assert!(command_response_is_ephemeral("data"));
}

#[test]
fn consolidated_slash_commands_replace_retired_top_level_commands() {
    let definitions = [
        session_command_definition(),
        storage_command_definition(),
        data_command_definition(),
    ];
    let values: Vec<serde_json::Value> = definitions
        .into_iter()
        .map(|definition| serde_json::to_value(definition).unwrap())
        .collect();
    let names: Vec<&str> = values
        .iter()
        .map(|definition| definition["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["session", "storage", "data"]);
    let option_names = |definition: &serde_json::Value| {
        definition["options"]
            .as_array()
            .unwrap()
            .iter()
            .map(|option| option["name"].as_str().unwrap().to_string())
            .collect::<Vec<_>>()
    };
    assert_eq!(option_names(&values[0]), ["status", "new", "compact"]);
    assert_eq!(option_names(&values[1]), ["memory", "notes"]);
    assert_eq!(option_names(&values[2]), ["profile", "history", "erase"]);
    assert!(RETIRED_SLASH_COMMANDS.contains(&"reset"));
    assert!(RETIRED_SLASH_COMMANDS.contains(&"erase_my_data"));
    assert!(!RETIRED_SLASH_COMMANDS.contains(&"session"));
}

#[test]
fn effort_command_includes_instant_and_optional_user_target() {
    let definition = serde_json::to_value(effort_command_definition()).unwrap();
    let options = definition["options"].as_array().unwrap();
    let level = options
        .iter()
        .find(|option| option["name"] == "level")
        .unwrap();
    let choices = level["choices"]
        .as_array()
        .unwrap()
        .iter()
        .map(|choice| choice["value"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        choices,
        ["instant", "low", "medium", "high", "xhigh", "max"]
    );
    let user = options
        .iter()
        .find(|option| option["name"] == "user")
        .unwrap();
    assert_eq!(user["type"], 6);
    assert_eq!(user["required"], false);
}

#[tokio::test]
async fn effort_target_requires_admin_and_saves_target_config() {
    let temp = TempDir::new().unwrap();
    let user_config = UserConfigStore::new(temp.path().join("user_config"));
    let options: Vec<serenity::all::CommandDataOption> = serde_json::from_value(json!([
        {"name": "level", "type": 3, "value": "instant"},
        {"name": "user", "type": 6, "value": "99"}
    ]))
    .unwrap();

    let denied = handle_effort_interaction(&user_config, &options, 42, false).await;
    assert!(denied.contains("Only server administrators and bot configurers"));
    assert_eq!(
        user_config.load(99).await.thinking_mode,
        ThinkingMode::Medium
    );

    let saved = handle_effort_interaction(&user_config, &options, 42, true).await;
    assert!(saved.contains("user `99`"));
    assert_eq!(
        user_config.load(99).await.thinking_mode,
        ThinkingMode::Instant
    );
    assert_eq!(
        user_config.load(42).await.thinking_mode,
        ThinkingMode::Medium
    );
}

#[tokio::test]
async fn progress_target_requires_admin_and_enables_final_only_mode() {
    let temp = TempDir::new().unwrap();
    let user_config = UserConfigStore::new(temp.path().join("user_config"));
    let options: Vec<serenity::all::CommandDataOption> = serde_json::from_value(json!([{
        "name": "progress",
        "type": 1,
        "options": [
            {"name": "enabled", "type": 5, "value": false},
            {"name": "user", "type": 6, "value": "99"}
        ]
    }]))
    .unwrap();

    let denied = handle_personalize_interaction(&user_config, &options, 42, false).await;
    assert!(denied.contains("Only server administrators and bot configurers"));
    assert!(user_config.load(99).await.progress_updates_enabled);

    let saved = handle_personalize_interaction(&user_config, &options, 42, true).await;
    assert!(saved.contains("only final responses"));
    assert!(!user_config.load(99).await.progress_updates_enabled);
    assert!(user_config.load(42).await.progress_updates_enabled);
}

#[tokio::test]
async fn storage_slash_notes_use_the_prefix_store_handler() {
    let (_temp, _skills, notes, memory, _history) = stores();
    let options: Vec<serenity::all::CommandDataOption> = serde_json::from_value(json!([{
        "name": "notes",
        "type": 2,
        "options": [{
            "name": "save",
            "type": 1,
            "options": [
                {"name": "name", "type": 3, "value": "shopping"},
                {"name": "content", "type": 3, "value": "milk and eggs"}
            ]
        }]
    }]))
    .unwrap();

    let reply = handle_storage_interaction(&memory, &notes, &options, 42).await;
    assert!(reply.contains("saved"));
    assert_eq!(
        notes.get(42, "shopping").await.as_deref(),
        Some("milk and eggs")
    );
}

#[test]
fn leaderboard_visibility_controls_access_and_response_scope() {
    let mut config = ServerConfig::default();
    assert_eq!(
        leaderboard_access(&config, true, &[], false),
        LeaderboardAccess::Public
    );

    config.leaderboard_visibility = LeaderboardVisibility::Private;
    assert_eq!(
        leaderboard_access(&config, true, &[], false),
        LeaderboardAccess::Private
    );

    config.leaderboard_visibility = LeaderboardVisibility::Restricted;
    config.leaderboard_role_ids.insert(42);
    assert_eq!(
        leaderboard_access(&config, true, &[], false),
        LeaderboardAccess::Denied
    );
    assert_eq!(
        leaderboard_access(&config, true, &[42], false),
        LeaderboardAccess::Private
    );
    assert_eq!(
        leaderboard_access(&config, true, &[], true),
        LeaderboardAccess::Private
    );
    assert_eq!(
        leaderboard_access(&config, false, &[], false),
        LeaderboardAccess::Private
    );
}

#[test]
fn lua_reply_is_fenced() {
    assert_eq!(format_lua_reply("hello"), "```\nhello\n```");
}

#[test]
fn lua_reply_escapes_nested_fences() {
    let reply = format_lua_reply("a ``` b");
    assert_eq!(reply.matches("```").count(), 2);
}

#[test]
fn lua_reply_fits_discord_limit() {
    let reply = format_lua_reply(&"x".repeat(5000));
    assert!(reply.chars().count() <= MAX_MESSAGE_LENGTH);
    assert!(reply.starts_with("```\n"));
    assert!(reply.ends_with("\n```"));
    assert!(reply.contains('…'));
}

#[test]
fn detects_raw_discord_user_mentions_from_connector_messages() {
    assert!(content_mentions_user("hello <@123456>", 123456));
    assert!(content_mentions_user("hello <@!123456>", 123456));
    assert!(!content_mentions_user("hello @123456", 123456));
    assert!(!content_mentions_user("hello <@123456", 123456));
    assert!(!content_mentions_user("hello <@1234567>", 123456));
}

#[test]
fn global_history_combines_profile_and_channel_context() {
    let profile = UserProfile {
        nickname: "Ali".to_string(),
        tags: vec![ProfileTag::WebResearch],
        ..Default::default()
    };
    let history = vec![
        json!({
            "role": "user",
            "content": "Find the release notes",
            "discord_context": {
                "channel_id": 42,
                "timestamp": "2026-07-14T20:15:00Z"
            }
        }),
        json!({"role": "assistant", "content": "Here they are"}),
    ];

    let rendered = render_history(&profile, &history);
    assert!(rendered.contains("History for Ali"));
    assert!(rendered.contains("all servers and channels"));
    assert!(rendered.contains("Profile interests: web research"));
    assert!(rendered.contains("[user in <#42> on 2026-07-14]"));
    assert!(
        rendered.find("Find the release notes").unwrap() < rendered.find("Here they are").unwrap()
    );
}

#[test]
fn global_history_empty_state_keeps_profile_identity() {
    let profile = UserProfile {
        display_name: "Alice".to_string(),
        ..Default::default()
    };
    let rendered = render_history(&profile, &[]);
    assert!(rendered.contains("History for Alice"));
    assert!(rendered.contains("No conversation history yet."));
}

#[test]
fn split_zero_limit_terminates() {
    // Regression: limit 0 previously looped forever on any non-empty input.
    let chunks = split_text("abc", 0);
    assert!(!chunks.is_empty());
    assert!(chunks.iter().all(|c| c.chars().count() <= 1));
}

#[test]
fn split_short_text_single_chunk() {
    assert_eq!(split_text("hello", 2000), vec!["hello"]);
}

#[test]
fn split_exact_limit_not_split() {
    let text = "a".repeat(2000);
    assert_eq!(split_text(&text, 2000), vec![text.clone()]);
}

#[test]
fn split_over_limit_on_newline() {
    let text = format!("{}\n{}", "a".repeat(1900), "b".repeat(200));
    let chunks = split_text(&text, 2000);
    assert_eq!(chunks.len(), 2);
    assert!(chunks.iter().all(|c| c.chars().count() <= 2000));
    assert_eq!(chunks.concat(), text.replacen('\n', "", 1));
}

#[test]
fn split_over_limit_no_newline() {
    let text = "x".repeat(2500);
    let chunks = split_text(&text, 2000);
    assert_eq!(chunks, vec!["x".repeat(2000), "x".repeat(500)]);
}

#[test]
fn split_multiple_chunks() {
    let text = vec!["a".repeat(1999); 3].join("\n");
    let chunks = split_text(&text, 2000);
    assert_eq!(chunks.len(), 3);
    assert!(chunks.iter().all(|c| c.chars().count() <= 2000));
}

#[test]
fn split_empty_string() {
    assert_eq!(split_text("", 2000), vec![""]);
}

#[test]
fn split_custom_limit() {
    let chunks = split_text("hello\nworld", 6);
    assert_eq!(chunks, vec!["hello", "world"]);
}

#[test]
fn hint_use_skill_with_name() {
    let h = tool_hint("use_skill", &json!({"name": "summarize"}));
    assert!(h.contains("summarize"));
}

#[test]
fn hint_use_skill_no_name() {
    assert_eq!(tool_hint("use_skill", &json!({})), "");
}

#[test]
fn hint_falls_back_to_query() {
    assert!(tool_hint("web_search", &json!({"query": "latest news"})).contains("latest news"));
}

#[test]
fn hint_falls_back_to_task() {
    assert!(tool_hint("some_tool", &json!({"task": "write a script"})).contains("write a script"));
}

#[test]
fn hint_long_value_truncated() {
    let h = tool_hint("some_tool", &json!({"task": "x".repeat(200)}));
    assert!(h.chars().count() <= 85);
}

#[test]
fn hint_unknown_tool_no_known_key() {
    assert_eq!(tool_hint("some_tool", &json!({"foo": "bar"})), "");
}

#[test]
fn status_includes_tool_name() {
    assert_eq!(tool_status("web_search"), "🔎 **Running `web_search`...**");
    assert_eq!(
        tool_status("jellyfin__search"),
        "🎬 **Running `jellyfin__search`...**"
    );
    assert_eq!(tool_status("run_lua"), "⚙️ **Running `run_lua`...**");
}

#[test]
fn status_has_a_generic_fallback() {
    assert_eq!(
        tool_status("new_external_tool"),
        "🔧 **Running `new_external_tool`...**"
    );
}

#[test]
fn status_fallback_caps_oversized_name() {
    let long = "x".repeat(200);
    let status = tool_status(&long);
    assert!(status.chars().count() < 200);
    assert!(status.contains('…'));
}

#[test]
fn status_caps_oversized_multibyte_name_without_panicking() {
    let long = "é".repeat(200);
    let status = tool_status(&long);
    assert!(status.chars().count() < 200);
    assert!(status.contains('…'));
}

#[test]
fn hint_multiline_flattened() {
    let h = tool_hint("some_tool", &json!({"task": "line1\nline2"}));
    assert!(!h.contains('\n'));
}
