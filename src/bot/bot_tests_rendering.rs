//! bot tests rendering.

//! Unit tests for `bot` (split out to keep the module under 600 lines).

use super::bot_tests_support::*;
use super::*;
use serde_json::json;

#[test]
fn tool_summary_lists_tools_in_call_order() {
    let summary = append_tool_summary("answer", &["web_search".into(), "translate".into()]);
    assert!(summary.ends_with("🛠️ **Tools used:** `web_search`, `translate`"));
}

#[test]
fn tool_summary_shows_none_when_no_tools_were_called() {
    assert!(append_tool_summary("answer", &[]).ends_with("🛠️ **Tools used:** none"));
}

#[test]
fn code_short_block_not_extracted() {
    let text = "Here:\n```python\nprint('hi')\n```";
    let (modified, files) = extract_code_files(text);
    assert!(files.is_empty());
    assert!(modified.contains("```"));
}

#[test]
fn code_large_block_extracted() {
    let code = "x = 1\n".repeat(200);
    let text = format!("Here:\n```python\n{code}```");
    let (modified, files) = extract_code_files(&text);
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].0, "script_1.py");
    assert_eq!(files[0].1, code.as_bytes());
    assert!(!modified.contains("```"));
    assert!(modified.contains("script_1.py"));
}

#[test]
fn code_extension_from_language() {
    let code = "echo hi\n".repeat(150);
    let (_, files) = extract_code_files(&format!("```bash\n{code}```"));
    assert!(files[0].0.ends_with(".sh"));
}

#[test]
fn code_unknown_language_txt() {
    let code = "blah\n".repeat(200);
    let (_, files) = extract_code_files(&format!("```brainfuck\n{code}```"));
    assert!(files[0].0.ends_with(".txt"));
}

#[test]
fn code_unclosed_block_still_extracted() {
    let code = "x = 1\n".repeat(200);
    let (modified, files) = extract_code_files(&format!("```python\n{code}"));
    assert_eq!(files.len(), 1);
    assert!(modified.contains("script_1.py"));
}

#[test]
fn code_multiple_blocks_numbered() {
    let code = "x = 1\n".repeat(200);
    let (_, files) = extract_code_files(&format!("```python\n{code}```\n```bash\n{code}```"));
    assert_eq!(files.len(), 2);
    assert_eq!(files[0].0, "script_1.py");
    assert_eq!(files[1].0, "script_2.sh");
}

#[test]
fn code_mixed_small_and_large() {
    let small = "print('hi')\n";
    let large = "x = 1\n".repeat(200);
    let (modified, files) =
        extract_code_files(&format!("```python\n{small}```\n```python\n{large}```"));
    assert_eq!(files.len(), 1);
    assert!(modified.contains("script_1.py"));
    assert!(modified.contains("```python"));
}

#[test]
fn redact_known_secret() {
    let r = SecretRedactor::from_vars([(
        "MY_SECRET_TOKEN".into(),
        "super-secret-token-abc123xyz".into(),
    )]);
    let out = r.redact("The token is super-secret-token-abc123xyz");
    assert!(!out.contains("super-secret-token-abc123xyz"));
    assert!(out.contains("[REDACTED]"));
}

#[test]
fn redact_non_secret_env_not_redacted() {
    let r = SecretRedactor::from_vars([("MY_NAME".into(), "alice-longenough".into())]);
    assert_eq!(r.redact("hello alice-longenough"), "hello alice-longenough");
}

#[test]
fn redact_short_value_not_redacted() {
    let r = SecretRedactor::from_vars([("MY_TOKEN".into(), "abc".into())]);
    assert_eq!(r.redact("abc"), "abc");
}

#[test]
fn redact_multiple_secrets() {
    let r = SecretRedactor::from_vars([
        ("BOT_TOKEN".into(), "discord-token-xyz987".into()),
        ("JELLYFIN_API_KEY".into(), "jellyfin-api-key-456def".into()),
    ]);
    let out = r.redact("token=discord-token-xyz987 key=jellyfin-api-key-456def");
    assert!(!out.contains("discord-token-xyz987"));
    assert!(!out.contains("jellyfin-api-key-456def"));
    assert_eq!(out.matches("[REDACTED]").count(), 2);
}

#[test]
fn redact_text_without_secrets_unchanged() {
    let r = SecretRedactor::from_vars(std::iter::empty());
    assert_eq!(
        r.redact("hello world, no secrets here"),
        "hello world, no secrets here"
    );
}

#[test]
fn tracker_inactive_when_unknown() {
    let t = ConversationTracker::new(Duration::from_secs(300));
    assert!(!t.is_active(1, 2, Instant::now()));
}

#[test]
fn tracker_active_within_window() {
    let mut t = ConversationTracker::new(Duration::from_secs(300));
    let now = Instant::now();
    t.mark_active(1, 2, now, Duration::from_secs(300));
    assert!(t.is_active(1, 2, now + Duration::from_secs(100)));
}

#[test]
fn tracker_pop_timed_out() {
    let mut t = ConversationTracker::new(Duration::from_secs(300));
    let now = Instant::now();
    t.mark_active(1, 2, now, Duration::from_secs(300));
    assert!(!t.is_active(1, 2, now + Duration::from_secs(400)));
    assert!(t.pop_timed_out(1, 2, now + Duration::from_secs(400)));
    // Now removed.
    assert!(!t.pop_timed_out(1, 2, now + Duration::from_secs(400)));
}

#[test]
fn commit_hash_response_reports_build_sha() {
    assert_eq!(
        commit_hash_response(Some("abcdef1234567890")),
        "Running commit: `abcdef1234567890`"
    );
    assert_eq!(
        commit_hash_response(None),
        "Running commit is unavailable for this build."
    );
}

#[test]
fn proactive_candidate_is_narrow() {
    assert!(is_proactive_candidate("How do I use reminders?"));
    assert!(is_proactive_candidate("Remind me tomorrow"));
    assert!(!is_proactive_candidate("hello everyone"));
}

#[tokio::test]
async fn skill_list_shows_saved_skill() {
    let (t, skills, _n, _m, _h) = stores();
    let user_config = UserConfigStore::new(t.path().join("user_config"));
    skills.save(test_skill("greeter", "7")).await.unwrap();
    let list = skill_command(&skills, &user_config, "!skill list", 7).await;
    assert!(list.contains("greeter"));
}

#[tokio::test]
async fn skill_add_and_edit_redirect_to_conversation() {
    let (t, skills, _n, _m, _h) = stores();
    let user_config = UserConfigStore::new(t.path().join("user_config"));
    let add = skill_command(&skills, &user_config, "!skill add greeter", 1).await;
    assert!(add.contains("create_skill"), "add: {add}");
    let edit = skill_command(&skills, &user_config, "!skill edit greeter", 1).await;
    assert!(edit.contains("edit_skill"), "edit: {edit}");
}

/// Regression test for the vulnerability this removal fixes: `!skill add`
/// used to silently overwrite any existing skill (including one owned by
/// someone else) with no ownership check at all. Now that `add` is a static
/// redirect, an attempted overwrite must leave the existing skill untouched.
#[tokio::test]
async fn skill_add_cannot_overwrite_an_existing_skill_owned_by_someone_else() {
    let (t, skills, _n, _m, _h) = stores();
    let user_config = UserConfigStore::new(t.path().join("user_config"));
    skills.save(test_skill("greeter", "7")).await.unwrap();
    let out = skill_command(&skills, &user_config, "!skill add greeter", 999).await;
    assert!(out.contains("create_skill"), "out: {out}");
    let unchanged = skills.get("greeter").await.unwrap();
    assert_eq!(unchanged.instructions, "You greet people");
    assert_eq!(unchanged.created_by.as_deref(), Some("7"));
    assert_eq!(unchanged.version, 1);
}

/// Regression test: the `/skill add` Discord slash subcommand was removed
/// along with `!skill add` (same overwrite vulnerability). Sending the old
/// subcommand shape must no longer be recognized or create anything.
#[tokio::test]
async fn skill_interaction_add_subcommand_no_longer_recognized() {
    let (t, skills, _n, _m, _h) = stores();
    let user_config = UserConfigStore::new(t.path().join("user_config"));
    let options: Vec<serenity::all::CommandDataOption> = serde_json::from_value(json!([{
        "name": "add",
        "type": 1,
        "options": [
            {"name": "name", "type": 3, "value": "greeter"},
            {"name": "prompt", "type": 3, "value": "Say hello"}
        ]
    }]))
    .unwrap();
    let out = handle_skill_interaction(&skills, &user_config, &options, 1).await;
    assert!(out.contains("Unknown subcommand"), "out: {out}");
    assert!(skills.get("greeter").await.is_none());
}

/// Regression test: the `/skill` command definition must not re-offer an
/// `add` subcommand — creation/editing now happens only through the
/// create_skill / edit_skill LLM tools.
#[test]
fn skill_slash_command_definition_has_no_add_option() {
    let definition = serde_json::to_value(skill_command_definition()).unwrap();
    let option_names: Vec<String> = definition["options"]
        .as_array()
        .unwrap()
        .iter()
        .map(|option| option["name"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(option_names, ["list", "info", "delete"]);
}

#[tokio::test]
async fn skill_enable_then_disable() {
    let (t, skills, _n, _m, _h) = stores();
    let user_config = UserConfigStore::new(t.path().join("user_config"));
    skills.save(test_skill("greeter", "7")).await.unwrap();
    let enable = skill_command(&skills, &user_config, "!skill enable greeter", 7).await;
    assert!(enable.contains("enabled"));
    assert!(user_config
        .load(7)
        .await
        .enabled_skills
        .contains(&"greeter".to_string()));
    let list = skill_command(&skills, &user_config, "!skill list", 7).await;
    assert!(list.contains("✓ **greeter**"));
    let disable = skill_command(&skills, &user_config, "!skill disable greeter", 7).await;
    assert!(disable.contains("disabled"));
    assert!(user_config.load(7).await.enabled_skills.is_empty());
}

#[tokio::test]
async fn skill_enable_missing_rejected() {
    let (t, skills, _n, _m, _h) = stores();
    let user_config = UserConfigStore::new(t.path().join("user_config"));
    let out = skill_command(&skills, &user_config, "!skill enable nope", 7).await;
    assert!(out.contains("not found"));
}

#[tokio::test]
async fn skill_delete_missing() {
    let (t, skills, _n, _m, _h) = stores();
    let user_config = UserConfigStore::new(t.path().join("user_config"));
    assert!(
        skill_command(&skills, &user_config, "!skill delete nope", 1)
            .await
            .contains("not found")
    );
}

#[tokio::test]
async fn note_save_get_delete() {
    let (_t, _s, notes, _m, _h) = stores();
    assert!(
        note_command(&notes, "!note save shopping", "milk, eggs", 42)
            .await
            .contains("saved")
    );
    assert!(note_command(&notes, "!note get shopping", "", 42)
        .await
        .contains("milk, eggs"));
    assert!(note_command(&notes, "!note delete shopping", "", 42)
        .await
        .contains("deleted"));
    assert!(note_command(&notes, "!note get shopping", "", 42)
        .await
        .contains("not found"));
}

#[tokio::test]
async fn note_list_empty() {
    let (_t, _s, notes, _m, _h) = stores();
    assert!(note_command(&notes, "!note list", "", 1)
        .await
        .contains("no saved notes"));
}

#[tokio::test]
async fn stats_reports_counts() {
    let (_t, skills, notes, memory, history) = stores();
    memory.save(5.to_string(), "some memory").await.unwrap();
    notes.save(5, "a", "x").await.unwrap();
    let out = stats_command(&history, &memory, &notes, &skills, 5, "Alice").await;
    assert!(out.contains("Stats for Alice"));
    assert!(out.contains("Saved notes: 1"));
}

#[test]
fn dev_notify_footer_parses_valid_text() {
    let footer = "housebot-dev-notify requester_id=123456789 issue=42 status=success sig=ab12";
    assert_eq!(
        parse_dev_notify_footer(footer),
        Some((123456789, 42, "success".to_string(), "ab12".to_string()))
    );
}

#[test]
fn dev_notify_footer_rejects_unrelated_text() {
    assert_eq!(parse_dev_notify_footer("some other footer text"), None);
    assert_eq!(
        parse_dev_notify_footer("housebot-dev-notify issue=42"),
        None
    );
}

#[test]
fn dev_notify_footer_rejects_missing_requester_id() {
    // requester_id absent even though issue and status are present.
    assert_eq!(
        parse_dev_notify_footer("housebot-dev-notify issue=42 status=success sig=ab12"),
        None
    );
}

#[test]
fn dev_notify_footer_rejects_empty_status() {
    assert_eq!(
        parse_dev_notify_footer("housebot-dev-notify requester_id=1 issue=42 status= sig=ab12"),
        None
    );
}

#[test]
fn dev_notify_footer_rejects_zero_requester_id() {
    assert_eq!(
        parse_dev_notify_footer(
            "housebot-dev-notify requester_id=0 issue=42 status=success sig=ab12"
        ),
        None
    );
}

#[test]
fn dev_notify_footer_rejects_missing_sig() {
    assert_eq!(
        parse_dev_notify_footer("housebot-dev-notify requester_id=1 issue=42 status=success"),
        None
    );
}

#[test]
fn dev_notify_footer_allows_equals_in_value() {
    // split_once splits on the *first* '=', so values may safely contain '='.
    let footer = "housebot-dev-notify requester_id=1 issue=42 status=error=timeout sig=ab12";
    assert_eq!(
        parse_dev_notify_footer(footer),
        Some((1, 42, "error=timeout".to_string(), "ab12".to_string()))
    );
}
