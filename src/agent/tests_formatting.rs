//! tests formatting.

//! Unit tests for `agent` (split out to keep the module under 600 lines).

use super::tests_support::*;
use super::*;
use std::collections::BTreeSet;

#[test]
fn prompt_regression_dynamic_markers_after_guidelines_maximal() {
    let mut skills = BTreeMap::new();
    skills.insert(
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
        "",
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
        "",
    );
    assert!(
        p.contains("summarizing what they asked.\n- When a user asks what was discussed"),
        "new history guideline must appear after the TL;DR bullet"
    );
    assert!(
        p.contains("## Session information\n- update_memory"),
        "memory tool must appear in the Session information section"
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
        "",
    );
    let last_stable_pos = p
        .find("summarizing what they asked.")
        .expect("final stable guideline present");
    let memory_tool_pos = p
        .find("- update_memory —")
        .expect("memory tool present with deep_memory enabled");
    let use_skill_pos = p.find("- use_skill —").expect("use_skill tool present");
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
        memory_tool_pos < use_skill_pos,
        "memory tools must precede skills section"
    );
    // Config content before memory_guidance
    assert!(
        use_skill_pos < memory_guidance_pos,
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
        use_skill_tool(),
        create_skill_tool(),
        crate::tools::manage_skills::list_definition(),
        crate::tools::manage_skills::info_definition(),
        crate::tools::manage_skills::delete_definition(),
        crate::tools::manage_skills::edit_definition(),
        crate::tools::manage_skills::enable_definition(),
        crate::tools::manage_skills::disable_definition(),
        crate::tools::feature_request::definition(),
        crate::tools::edit_feature_request::definition(),
        crate::tools::feature_development::definition(),
        crate::tools::github_api::definition(),
        crate::tools::remind::definition(),
        crate::tools::summarize_url::definition(),
        crate::tools::token_metrics::definition(),
        crate::tools::translate::definition(),
        crate::tools::features::definition(),
        get_messages_tool(),
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
