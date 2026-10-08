//! Unit tests for `agent` (split out to keep the module under 600 lines).

use super::*;
use crate::token_monitor::LeaderboardRank;
use std::collections::BTreeSet;

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

#[test]
fn system_prompt_includes_username_and_id() {
    let p = build_system_prompt("Alice", "123", None);
    assert!(p.contains("Alice"));
    assert!(p.contains("123"));
}

#[test]
fn system_prompt_names_the_hardcoded_model_identity() {
    let p = build_system_prompt("Alice", "123", None);
    assert!(
        p.contains("This iteration is Claude Sonnet 6."),
        "identity line must name Claude Sonnet 6: {p}"
    );
}

#[test]
fn session_context_carries_memory_and_profile() {
    let m = build_session_context_message("alice", "Alice", "Ali", "", "Likes cats");
    let content = m["content"].as_str().unwrap();
    assert!(content.contains("Likes cats"), "{content}");
    assert!(content.contains("Your memory"), "{content}");
    assert!(content.contains("Nickname: Ali"), "{content}");
    assert!(is_session_context(&m));
    assert!(!is_session_context(
        &json!({"role": "user", "content": "hi"})
    ));
}

#[test]
fn session_context_omits_blank_memory() {
    let m = build_session_context_message("alice", "alice", "", "", "   ");
    assert!(!m["content"].as_str().unwrap().contains("Your memory"));
}

#[test]
fn system_prompt_holds_no_per_session_data() {
    // Memory and profile change between sessions; in the system prompt they
    // would miss the prompt cache for the whole conversation.
    let p = build_system_prompt("alice", "7", None);
    assert!(!p.contains("Your memory"), "{p}");
    assert!(!p.contains("User profile"), "{p}");
}

#[test]
fn skill_descriptions_arrive_as_a_user_message_not_the_system_prompt() {
    let mut skills = BTreeMap::new();
    skills.insert(
        "greet".into(),
        Skill {
            name: "greet".into(),
            description: Some("Say hello".into()),
            instructions: "..".into(),
            ..Skill::default()
        },
    );
    let message = build_skills_message(&skills);
    assert_eq!(message["role"], "user");
    let content = message["content"].as_str().unwrap();
    assert!(content.contains("**greet**: Say hello"), "{content}");
    assert!(content.contains("skills/<name>/SKILL.md"), "{content}");

    let p = build_system_prompt("Alice", "123", None);
    assert!(!p.contains("Say hello"));
}

#[test]
fn system_prompt_does_not_contain_the_time() {
    // A timestamp in the prompt would break the prompt cache on every turn.
    let p = build_system_prompt("Alice", "123", None);
    assert!(!p.contains("Current date/time"), "{p}");
    assert!(p.contains("get_current_time"), "{p}");
}

#[test]
fn current_time_text_converts_to_the_given_zone() {
    let now = DateTime::parse_from_rfc3339("2026-01-15T12:00:00Z")
        .unwrap()
        .with_timezone(&Utc);
    assert_eq!(
        current_time_text("", now),
        "Thursday 2026-01-15 12:00:00 (UTC, UTC+00:00)"
    );
    assert_eq!(
        current_time_text("America/New_York", now),
        "Thursday 2026-01-15 07:00:00 (America/New_York, UTC-05:00)"
    );
    assert!(current_time_text("Mars/Olympus", now).starts_with("Error: unknown time zone"));
}

/// Verify that `all_tool_names()` stays in sync with the actual tool
/// definitions registered in `Agent::build_tools`.  Any name present in one
/// but not the other represents either a missing autocomplete entry or a
/// tool that was added/removed without updating the list.
#[test]
fn all_tool_names_matches_built_in_definitions() {
    // Collect names from the definition functions (mirrors build_tools
    // excluding conditionally-included sandbox and memory tools).
    let defined: BTreeSet<String> = [
        crate::tools::searxng::definition(),
        crate::tools::web_fetch::definition(),
        crate::tools::manage_skills::definition(),
        crate::tools::feature_request::definition(),
        crate::tools::edit_feature_request::definition(),
        crate::tools::feature_development::definition(),
        crate::tools::github_api::definition(),
        crate::tools::remind::definition(),
        crate::tools::features::definition(),
        get_messages_tool(),
        get_current_time_tool(),
    ]
    .into_iter()
    .map(|def| {
        def.get("name")
            .and_then(|n| n.as_str())
            .expect("tool definition must have a name")
            .to_string()
    })
    .collect();

    let all_tool_names: BTreeSet<String> = crate::tools::all_tool_names()
        .iter()
        .copied()
        .map(String::from)
        .collect();

    // These are conditionally included in build_tools so they appear in
    // all_tool_names but not in the unconditional list above.
    let conditionals: BTreeSet<String> =
        ["update_memory", "search_memory", "read", "write", "shell"]
            .into_iter()
            .map(String::from)
            .collect();

    // `housebot` is a special sentinel name representing a full bot-interaction
    // ban — it is not a real tool with a definition.
    let sentinels: BTreeSet<String> = ["housebot"].into_iter().map(String::from).collect();

    for name in &defined {
        assert!(
            all_tool_names.contains(name),
            "tool `{name}` is defined but missing from all_tool_names()"
        );
    }

    for name in &all_tool_names {
        assert!(
            defined.contains(name) || conditionals.contains(name) || sentinels.contains(name),
            "tool `{name}` is in all_tool_names() but has no matching definition"
        );
    }
}
