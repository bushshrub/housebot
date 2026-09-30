//! Unit tests for `agent` (split out to keep the module under 600 lines).

use super::*;
use crate::token_monitor::LeaderboardRank;
use std::collections::BTreeSet;

#[test]
fn emoji_selection_accepts_only_an_emoji_or_none() {
    assert_eq!(parse_emoji_selection("👍"), Some("👍".into()));
    assert_eq!(parse_emoji_selection("👍🏽"), Some("👍🏽".into()));
    assert_eq!(parse_emoji_selection("🇨🇦"), Some("🇨🇦".into()));
    assert_eq!(parse_emoji_selection("❤️"), Some("❤️".into()));
    assert_eq!(parse_emoji_selection("👍👍"), None);
    assert_eq!(parse_emoji_selection("NONE"), None);
    assert_eq!(parse_emoji_selection("👍 sounds good"), None);
    assert_eq!(parse_emoji_selection("sure"), None);
}

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
    let p = build_system_prompt("Alice", "123", "Alice", "", "", None, true);
    assert!(p.contains("Alice"));
    assert!(p.contains("123"));
}

#[test]
fn system_prompt_memory_section_present_when_nonempty() {
    let p = build_system_prompt("Alice", "123", "Alice", "", "Likes cats", None, true);
    assert!(p.contains("Likes cats"));
    assert!(p.contains("Your memory"));
}

#[test]
fn system_prompt_memory_absent_when_blank() {
    assert!(
        !build_system_prompt("Alice", "123", "Alice", "", "   ", None, true)
            .contains("Your memory")
    );
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

    let p = build_system_prompt("Alice", "123", "Alice", "", "", None, true);
    assert!(!p.contains("Say hello"));
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
