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
        "2026-07-17 12:00",
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
        "2026-07-17 12:00",
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

// ── stable-prefix / ordering tests ─────────────────────────────────────────

/// Returns the byte index of the first user/turn-specific marker.
fn dynamic_suffix_start(prompt: &str) -> usize {
    let markers = [
        "\n\n## User profile",
        "\n\n## Your memory about",
        "\n\n## Personality / tone",
        "\n\nCurrent date/time:",
    ];
    let mut earliest = prompt.len();
    for m in &markers {
        if let Some(pos) = prompt.find(m) {
            earliest = earliest.min(pos);
        }
    }
    earliest
}

#[test]
fn prompt_stable_prefix_unchanged_by_dynamic_content() {
    let skills = empty_skills();

    let cases: Vec<(&str, String)> = vec![
        (
            "baseline",
            build_system_prompt_with_profile(
                "Alice",
                "1",
                "Alice",
                "",
                "",
                "",
                &skills,
                None,
                true,
                "",
                "",
                "2026-07-17 12:00",
            ),
        ),
        (
            "different timestamp",
            build_system_prompt_with_profile(
                "Alice",
                "1",
                "Alice",
                "",
                "",
                "",
                &skills,
                None,
                true,
                "",
                "",
                "2026-07-18 08:30",
            ),
        ),
        (
            "different username+id",
            build_system_prompt_with_profile(
                "Bob",
                "999",
                "Bob",
                "",
                "",
                "",
                &skills,
                None,
                true,
                "",
                "",
                "2026-07-17 12:00",
            ),
        ),
        (
            "profile fields and avatar",
            build_system_prompt_with_profile(
                "Alice",
                "1",
                "Alice",
                "Ali",
                "https://ex/av.png",
                "",
                &skills,
                None,
                true,
                "tags",
                "actions",
                "2026-07-17 12:00",
            ),
        ),
        (
            "user memory",
            build_system_prompt_with_profile(
                "Alice",
                "1",
                "Alice",
                "",
                "",
                "Likes cats",
                &skills,
                None,
                true,
                "",
                "",
                "2026-07-17 12:00",
            ),
        ),
        (
            "personality",
            build_system_prompt_with_profile(
                "Alice",
                "1",
                "Alice",
                "",
                "",
                "",
                &skills,
                Some("Friendly"),
                true,
                "",
                "",
                "2026-07-17 12:00",
            ),
        ),
        (
            "usage tags and quick actions",
            build_system_prompt_with_profile(
                "Alice",
                "1",
                "Alice",
                "",
                "",
                "",
                &skills,
                None,
                true,
                "media",
                "search",
                "2026-07-17 12:00",
            ),
        ),
    ];

    let prefix_end = dynamic_suffix_start(&cases[0].1);
    let baseline_prefix = &cases[0].1[..prefix_end];
    for (label, prompt) in &cases {
        let end = dynamic_suffix_start(prompt);
        assert_eq!(
            &prompt[..end],
            baseline_prefix,
            "stable prefix differs for: {label}"
        );
    }
}

#[test]
fn prompt_static_base_present_regardless_of_deep_memory_or_skills() {
    let skills = empty_skills();
    let mut skill_map = BTreeMap::new();
    skill_map.insert(
        "greet".into(),
        Skill {
            name: "greet".into(),
            description: Some("Say hello".into()),
            prompt: "..".into(),
            created_by: None,
        },
    );

    let prompts: Vec<String> = vec![
        // deep_memory enabled, no skills
        build_system_prompt_with_profile(
            "Alice",
            "1",
            "Alice",
            "",
            "",
            "",
            &skills,
            None,
            true,
            "",
            "",
            "2026-07-17 12:00",
        ),
        // deep_memory disabled, no skills
        build_system_prompt_with_profile(
            "Alice",
            "1",
            "Alice",
            "",
            "",
            "",
            &skills,
            None,
            false,
            "",
            "",
            "2026-07-17 12:00",
        ),
        // deep_memory enabled, with skills
        build_system_prompt_with_profile(
            "Alice",
            "1",
            "Alice",
            "",
            "",
            "",
            &skill_map,
            None,
            true,
            "",
            "",
            "2026-07-17 12:00",
        ),
        // deep_memory disabled, with skills
        build_system_prompt_with_profile(
            "Alice",
            "1",
            "Alice",
            "",
            "",
            "",
            &skill_map,
            None,
            false,
            "",
            "",
            "2026-07-17 12:00",
        ),
    ];

    let static_base = crate::agent::prompt::STATIC_BASE;
    let static_len = static_base.len();
    for (i, p) in prompts.iter().enumerate() {
        assert_eq!(
            &p[..static_len],
            static_base,
            "STATIC_BASE differs for prompt {i}"
        );
    }

    // The text span from STATIC_BASE through the final stable guideline
    // must also be identical across all config combinations.
    let suffix_end = prompts[0]
        .find("summarizing what they asked.\n")
        .expect("final guideline in baseline")
        + "summarizing what they asked.\n".len();
    let baseline_stable = &prompts[0][..suffix_end];
    for (i, p) in prompts.iter().enumerate().skip(1) {
        assert_eq!(
            &p[..suffix_end],
            baseline_stable,
            "stable-guidelines prefix differs for prompt {i}"
        );
    }
}

#[test]
fn prompt_regression_dynamic_markers_after_guidelines_minimal() {
    let p = build_system_prompt_with_profile(
        "Alice",
        "1",
        "Alice",
        "",
        "",
        "",
        &empty_skills(),
        None,
        false,
        "",
        "",
        "2026-07-17 12:00",
    );
    let guidelines_pos = p
        .find("## Guidelines")
        .expect("## Guidelines section present");
    // In minimal form, only these markers appear
    assert!(
        p.find("Current date/time:").unwrap() > guidelines_pos,
        "Current date/time: must appear after ## Guidelines"
    );
    assert!(
        p.find("Current user:").unwrap() > guidelines_pos,
        "Current user: must appear after ## Guidelines"
    );
}

#[test]
fn prompt_regression_dynamic_markers_after_guidelines_maximal() {
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
    let p = build_system_prompt_with_profile(
        "Alice",
        "1",
        "Alice",
        "Ali",
        "https://ex/av.png",
        "Likes cats",
        &skills,
        Some("Friendly"),
        true,
        "tags",
        "actions",
        "2026-07-17 12:00",
    );
    let guidelines_pos = p
        .find("## Guidelines")
        .expect("## Guidelines section present");
    let markers = [
        "Current date/time:",
        "Current user:",
        "## User profile",
        "## Your memory about",
        "## Personality / tone",
    ];
    for marker in &markers {
        let pos = p
            .find(marker)
            .unwrap_or_else(|| panic!("marker {marker:?} not found"));
        assert!(
            pos > guidelines_pos,
            "marker {marker:?} (pos {pos}) appears before ## Guidelines (pos {guidelines_pos})"
        );
    }
}

#[test]
fn prompt_memory_tools_separated_from_preceding_guidelines_bullet() {
    let p = build_system_prompt_with_profile(
        "Alice",
        "1",
        "Alice",
        "",
        "",
        "",
        &empty_skills(),
        None,
        true,
        "",
        "",
        "2026-07-17 12:00",
    );
    assert!(
        p.contains("summarizing what they asked.\n- update_memory"),
        "memory tool must follow the last stable guidelines bullet on a new line, not merged"
    );
}

#[test]
fn prompt_config_content_ordered_between_guidelines_and_dynamic() {
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
    let p = build_system_prompt_with_profile(
        "Alice",
        "1",
        "Alice",
        "Ali",
        "https://ex/av.png",
        "Likes cats",
        &skills,
        Some("Friendly"),
        true,
        "tags",
        "actions",
        "2026-07-17 12:00",
    );
    let last_stable_pos = p
        .find("summarizing what they asked.")
        .expect("final stable guideline present");
    let memory_tool_pos = p
        .find("- update_memory —")
        .expect("memory tool present with deep_memory enabled");
    let run_skill_pos = p.find("- run_skill —").expect("run_skill tool present");
    let memory_guidance_pos = p
        .find("Actively use memory:")
        .expect("memory guidance present");
    let profile_pos = p.find("## User profile").expect("profile section present");
    let memory_pos = p
        .find("## Your memory about")
        .expect("memory section present");
    let personality_pos = p
        .find("## Personality / tone")
        .expect("personality section present");
    let date_pos = p.find("Current date/time:").expect("date/time present");

    // All stable guidelines come before config content
    assert!(
        last_stable_pos < memory_tool_pos,
        "all stable guidelines must precede config content"
    );
    // Config suffix: memory tools before skills
    assert!(
        memory_tool_pos < run_skill_pos,
        "memory tools must precede skills section"
    );
    // Config content before memory_guidance
    assert!(
        run_skill_pos < memory_guidance_pos,
        "skills section must precede memory guidance"
    );
    // memory_guidance before dynamic suffix
    assert!(
        memory_guidance_pos < profile_pos,
        "memory guidance before profile section"
    );
    assert!(
        memory_guidance_pos < memory_pos,
        "memory guidance before memory section"
    );
    assert!(
        memory_guidance_pos < personality_pos,
        "memory guidance before personality section"
    );
    assert!(
        memory_guidance_pos < date_pos,
        "memory guidance before date/time"
    );
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
        crate::tools::searxng::deep_research_definition(),
        crate::tools::web_fetch::definition(),
        crate::tools::file_download::definition(),
        crate::tools::common_crawl::definition(),
        run_skill_tool(),
        crate::tools::feature_request::definition(),
        crate::tools::edit_feature_request::definition(),
        crate::tools::feature_development::definition(),
        crate::tools::github_api::definition(),
        crate::tools::remind::definition(),
        crate::tools::summarize_url::definition(),
        crate::tools::token_metrics::definition(),
        crate::tools::translate::definition(),
        crate::tools::features::definition(),
        search_messages_tool(),
        get_recent_messages_tool(),
        find_discord_users_tool(),
        get_discord_user_tool(),
        run_lua_tool(),
        get_lua_docs_tool(),
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
    let conditionals: BTreeSet<String> = [
        "update_memory",
        "search_memory",
        "sandbox_clone_repository",
        "sandbox_list_files",
        "sandbox_search_code",
        "sandbox_read_file",
        "sandbox_run",
    ]
    .into_iter()
    .map(String::from)
    .collect();

    for name in &defined {
        assert!(
            all_tool_names.contains(name),
            "tool `{name}` is defined but missing from all_tool_names()"
        );
    }

    for name in &all_tool_names {
        assert!(
            defined.contains(name) || conditionals.contains(name),
            "tool `{name}` is in all_tool_names() but has no matching definition"
        );
    }
}
