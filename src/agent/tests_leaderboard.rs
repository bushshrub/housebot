//! tests leaderboard.

//! Unit tests for `agent` (split out to keep the module under 600 lines).

use super::tests_support::*;
use super::*;

#[test]
fn system_prompt_mentions_run_lua() {
    let p = build_system_prompt("Alice", "123", "Alice", "", "", &empty_skills(), None, true);
    assert!(p.contains("run_lua"));
    assert!(p.contains("get_lua_docs"));
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
                "",
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
                "",
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
                "",
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
                "",
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
                "",
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
                "",
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
                "",
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
            instructions: "..".into(),
            triggers: Vec::new(),
            enabled_tools: Vec::new(),
            examples: Vec::new(),
            version: 1,
            version_history: Vec::new(),
            created_by: None,
            editors: Vec::new(),
            created_at: 0,
            updated_at: 0,
            prompt: None,
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
            "",
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
            "",
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
            "",
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
            "",
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
        "",
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
