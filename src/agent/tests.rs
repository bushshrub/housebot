//! Unit tests for `agent` (split out to keep the module under 600 lines).

use super::*;
use crate::token_monitor::LeaderboardRank;

#[test]
fn token_leaderboard_format_shows_period_metric_and_requester_rank() {
    let entry = LeaderboardEntry {
        user_id: Some("u1".into()),
        label: "Alice".into(),
        conversation_id: None,
        conversations: 2,
        input_tokens: 100,
        output_tokens: 25,
        cached_tokens: 50,
    };
    let leaderboard = TokenLeaderboard {
        users: vec![entry.clone()],
        conversations: Vec::new(),
        requester_rank: Some(LeaderboardRank { position: 1, entry }),
        period: LeaderboardPeriod::Weekly,
        metric: LeaderboardMetric::CacheEfficiency,
    };

    let output = format_token_leaderboard(&leaderboard);
    assert!(output.contains("Weekly token leaderboard"));
    assert!(output.contains("50.0% cache efficiency"));
    assert!(output.contains("Your rank:** #1"));
}

fn empty_skills() -> BTreeMap<String, Skill> {
    BTreeMap::new()
}

#[test]
fn system_prompt_includes_username_and_id() {
    let p = build_system_prompt("Alice", "123", "Alice", "", "", &empty_skills(), None, true);
    assert!(p.contains("Alice"));
    assert!(p.contains("123"));
}

#[test]
fn system_prompt_memory_section_present_when_nonempty() {
    let p = build_system_prompt(
        "Alice",
        "123",
        "Alice",
        "",
        "Likes cats",
        &empty_skills(),
        None,
        true,
    );
    assert!(p.contains("Likes cats"));
    assert!(p.contains("Your memory"));
}

#[test]
fn system_prompt_memory_absent_when_blank() {
    assert!(!build_system_prompt(
        "Alice",
        "123",
        "Alice",
        "",
        "   ",
        &empty_skills(),
        None,
        true
    )
    .contains("Your memory"));
}

#[test]
fn system_prompt_lists_skills() {
    let mut skills = BTreeMap::new();
    skills.insert(
        "greet".into(),
        Skill {
            name: "greet".into(),
            description: Some("Say hello".into()),
            prompt: "..".into(),
            created_by: None,
        },
    );
    let p = build_system_prompt("Alice", "123", "Alice", "", "", &skills, None, true);
    assert!(p.contains("greet"));
    assert!(p.contains("Say hello"));
}

#[test]
fn system_prompt_placeholder_without_skills() {
    assert!(
        build_system_prompt("Alice", "123", "Alice", "", "", &empty_skills(), None, true)
            .contains("No skills are defined yet")
    );
}

#[test]
fn system_prompt_has_tldr_and_500() {
    let p = build_system_prompt("Alice", "123", "Alice", "", "", &empty_skills(), None, true);
    assert!(p.contains("TL;DR"));
    assert!(p.contains("500"));
}

#[test]
fn system_prompt_explains_guarded_file_delivery() {
    let prompt = build_system_prompt("Alice", "123", "", "", "", &empty_skills(), None, true);
    assert!(prompt.contains("download_file"));
    assert!(prompt.contains("specific file"));
    assert!(prompt.contains("private-network URLs"));
}

#[test]
fn system_prompt_routes_complex_questions_to_deep_research() {
    let p = build_system_prompt("Alice", "123", "", "", "", &empty_skills(), None, true);
    assert!(p.contains("deep_research"));
    assert!(p.contains("multiple perspectives"));
    assert!(p.contains("source links"));
}

#[test]
fn system_prompt_excludes_code_execution() {
    let p = build_system_prompt("Alice", "123", "Alice", "", "", &empty_skills(), None, true);
    assert!(!p.contains("code execution"));
}

#[test]
fn system_prompt_lists_discord_user_tools_once_and_in_order() {
    let p = build_system_prompt("Alice", "123", "Alice", "", "", &empty_skills(), None, true);
    let find = p.find("- find_discord_users —").unwrap();
    let get = p.find("- get_discord_user —").unwrap();
    assert!(find < get);
    assert_eq!(p.matches("- get_discord_user —").count(), 1);
}

#[test]
fn system_prompt_includes_profile_section_with_nickname() {
    let p = build_system_prompt(
        "Alice",
        "123",
        "Alice",
        "Ali",
        "",
        &empty_skills(),
        None,
        true,
    );
    assert!(p.contains("User profile"));
    assert!(p.contains("Nickname: Ali"));
}

#[test]
fn system_prompt_skips_profile_section_when_identical() {
    let p = build_system_prompt("Alice", "123", "Alice", "", "", &empty_skills(), None, true);
    assert!(!p.contains("User profile"));
}

#[test]
fn system_prompt_includes_usage_profile() {
    let p = build_system_prompt_with_profile(
        "Alice",
        "123",
        "Alice",
        "",
        "",
        "",
        &empty_skills(),
        None,
        true,
        "media, reminders",
        "media (4), reminders (2)",
    );
    assert!(p.contains("Relevant usage tags: media, reminders"));
    assert!(p.contains("Frequently used actions: media (4), reminders (2)"));
    assert!(p.contains("naturally address them by their nickname or display name"));
    assert!(p.contains("suggest at most one relevant quick action"));
    assert!(p.contains("Never infer sensitive traits"));
}

#[test]
fn system_prompt_includes_profile_avatar_with_safety_guidance() {
    let p = build_system_prompt_with_profile(
        "Alice",
        "123",
        "Alice",
        "",
        "https://cdn.discordapp.com/avatars/123/avatar.png",
        "",
        &empty_skills(),
        None,
        true,
        "",
        "",
    );
    assert!(p.contains("Avatar URL: https://cdn.discordapp.com/avatars/123/avatar.png"));
    assert!(p.contains("Never infer sensitive traits, identity, or intent from a user's avatar."));
}

#[test]
fn system_prompt_respects_deep_memory_disabled() {
    let p = build_system_prompt(
        "Alice",
        "123",
        "Alice",
        "",
        "",
        &empty_skills(),
        None,
        false,
    );
    assert!(p.contains("Deep memory is disabled"));
    assert!(p.contains("Do NOT call update_memory"));
}

#[test]
fn system_prompt_allows_deep_memory_when_enabled() {
    let p = build_system_prompt("Alice", "123", "Alice", "", "", &empty_skills(), None, true);
    assert!(p.contains("Actively use memory"));
}

#[test]
fn flatten_tool_extracts_fields() {
    let tool =
        json!({"name": "my_tool", "description": "does stuff", "input_schema": {"type": "object"}});
    let (n, d, p) = flatten_tool(&tool);
    assert_eq!(n, "my_tool");
    assert_eq!(d, "does stuff");
    assert_eq!(p, json!({"type": "object"}));
}

#[test]
fn flatten_tool_falls_back_to_parameters() {
    let tool = json!({"name": "t", "parameters": {"type": "object"}});
    assert_eq!(flatten_tool(&tool).2, json!({"type": "object"}));
}

#[test]
fn to_openai_tool_wraps_in_envelope() {
    let t = to_openai_tool("my_tool", "does stuff", json!({"type": "object"}));
    assert_eq!(t["type"], "function");
    assert_eq!(t["function"]["name"], "my_tool");
    assert_eq!(t["function"]["parameters"], json!({"type": "object"}));
}

#[test]
fn search_rate_limit_errors_are_detected() {
    assert!(search_rate_limited(
        "Error: SearXNG returned HTTP 429 Too Many Requests"
    ));
    assert!(search_rate_limited("SearXNG rate limit reached"));
    assert!(!search_rate_limited(
        "Error: search request failed: timeout"
    ));
}

#[test]
fn build_user_message_plain_text() {
    let m = build_user_message("hi", &[]);
    assert_eq!(m["content"], "hi");
}

#[test]
fn build_user_message_with_image() {
    let imgs = vec![MediaData {
        media_type: "image/png".into(),
        data: "abc".into(),
    }];
    let m = build_user_message("look", &imgs);
    assert_eq!(m["content"][0]["type"], "image_url");
    assert!(m["content"][0]["image_url"]["url"]
        .as_str()
        .unwrap()
        .contains("data:image/png;base64,abc"));
    assert_eq!(m["content"][1]["text"], "look");
}

#[test]
fn build_user_message_with_audio_and_video() {
    let media = vec![
        MediaData {
            media_type: "audio/mpeg".into(),
            data: "audio-bytes".into(),
        },
        MediaData {
            media_type: "video/mp4".into(),
            data: "video-bytes".into(),
        },
    ];
    let message = build_user_message("analyze", &media);
    assert_eq!(message["content"][0]["type"], "input_audio");
    assert_eq!(message["content"][0]["input_audio"]["data"], "audio-bytes");
    assert_eq!(message["content"][1]["type"], "input_video");
    assert_eq!(message["content"][1]["input_video"]["data"], "video-bytes");
}
#[test]
fn system_prompt_mentions_run_lua() {
    let p = build_system_prompt("Alice", "123", "Alice", "", "", &empty_skills(), None, true);
    assert!(p.contains("run_lua"));
    assert!(p.contains("get_lua_docs"));
}
