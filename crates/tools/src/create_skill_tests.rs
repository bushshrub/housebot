//! Unit tests for `create_skill` (split out to keep the module under 400 lines).

use super::*;
use housebot_skills::Skills;
use serde_json::json;
use tempfile::TempDir;

fn test_skills() -> (TempDir, Skills) {
    let tmp = TempDir::new().unwrap();
    let skills = Skills::new(tmp.path().join("skills.json"));
    (tmp, skills)
}

#[tokio::test]
async fn create_new_skill() {
    let (_t, skills) = test_skills();
    let result = dispatch_create_skill(
        &skills,
        "user123",
        &json!({
            "name": "summarizer",
            "instructions": "Summarize the user's input concisely.",
            "description": "A summarization skill",
            "triggers": [{"trigger_type": "keyword", "value": "summarize"}],
            "enabled_tools": ["web_search"],
            "examples": [{"input": "summarize this article", "output": "Here is the summary..."}]
        }),
    )
    .await;
    assert!(result.contains("created"), "result: {result}");
    let skill = skills.get("summarizer").await.unwrap();
    assert_eq!(skill.version, 1);
    assert_eq!(skill.triggers.len(), 1);
    assert_eq!(skill.enabled_tools.len(), 1);
    assert_eq!(skill.examples.len(), 1);
}

#[tokio::test]
async fn update_existing_skill_archives_old_version() {
    let (_t, skills) = test_skills();
    dispatch_create_skill(
        &skills,
        "user123",
        &json!({
            "name": "greeter",
            "instructions": "Say hello",
        }),
    )
    .await;

    let result = dispatch_create_skill(
        &skills,
        "user123",
        &json!({
            "name": "greeter",
            "instructions": "Say hello warmly",
            "version": 1,
        }),
    )
    .await;
    assert!(result.contains("updated"), "result: {result}");

    let skill = skills.get("greeter").await.unwrap();
    assert_eq!(skill.version, 2);
    assert_eq!(skill.version_history.len(), 1);
    assert_eq!(skill.version_history[0].version, 1);
    assert_eq!(skill.instructions, "Say hello warmly");
}

#[tokio::test]
async fn non_author_cannot_update() {
    let (_t, skills) = test_skills();
    dispatch_create_skill(
        &skills,
        "author1",
        &json!({
            "name": "locked",
            "instructions": "Private skill",
        }),
    )
    .await;

    let result = dispatch_create_skill(
        &skills,
        "intruder",
        &json!({
            "name": "locked",
            "instructions": "Hacked instructions",
            "version": 1,
        }),
    )
    .await;
    assert!(result.contains("⛔"));
}

#[tokio::test]
async fn update_rejects_wrong_version() {
    let (_t, skills) = test_skills();
    dispatch_create_skill(
        &skills,
        "user1",
        &json!({
            "name": "s",
            "instructions": "v1 instructions",
        }),
    )
    .await;

    let result = dispatch_create_skill(
        &skills,
        "user1",
        &json!({
            "name": "s",
            "instructions": "v2 instructions",
            "version": 999,
        }),
    )
    .await;
    assert!(result.contains("exists at version 1 but version 999 was supplied"));
}

#[tokio::test]
async fn omit_arrays_preserves_existing_on_update() {
    let (_t, skills) = test_skills();
    dispatch_create_skill(
        &skills,
        "user1",
        &json!({
            "name": "s",
            "instructions": "original",
            "triggers": [{"trigger_type": "keyword", "value": "hello"}],
            "enabled_tools": ["web_search"],
            "examples": [{"input": "hi", "output": "hello back"}],
        }),
    )
    .await;

    // Update instructions only — omit array fields
    let result = dispatch_create_skill(
        &skills,
        "user1",
        &json!({
            "name": "s",
            "instructions": "updated",
            "version": 1,
        }),
    )
    .await;
    assert!(result.contains("updated"), "result: {result}");

    let skill = skills.get("s").await.unwrap();
    assert_eq!(skill.instructions, "updated");
    // Arrays should be preserved since they were omitted
    assert_eq!(skill.triggers.len(), 1, "triggers should be preserved");
    assert_eq!(
        skill.enabled_tools.len(),
        1,
        "enabled_tools should be preserved"
    );
    assert_eq!(skill.examples.len(), 1, "examples should be preserved");
}

#[test]
fn definition_has_required_fields() {
    let d = definition();
    assert_eq!(d["name"], "create_skill");
    assert_eq!(
        d["input_schema"]["required"],
        json!(["name", "instructions"])
    );
}

#[test]
fn parse_triggers_from_value() {
    let v = json!([
        {"trigger_type": "keyword", "value": "hello"},
        {"trigger_type": "intent", "value": "greeting"},
    ]);
    let triggers = parse_triggers(Some(&v)).unwrap().unwrap();
    assert_eq!(triggers.len(), 2);
    assert_eq!(triggers[0].trigger_type, "keyword");
    assert_eq!(triggers[1].value, "greeting");
}

#[test]
fn parse_triggers_none_when_absent() {
    assert!(parse_triggers(None).unwrap().is_none());
}

#[test]
fn parse_triggers_rejects_non_array() {
    assert!(parse_triggers(Some(&json!("not_an_array"))).is_err());
}

#[test]
fn parse_triggers_rejects_malformed_element() {
    let v = json!([{"trigger_type": "keyword"}]); // missing "value"
    assert!(parse_triggers(Some(&v)).is_err());
}

#[test]
fn parse_examples_from_value() {
    let v = json!([
        {"input": "hi", "output": "hello back"},
    ]);
    let examples = parse_examples(Some(&v)).unwrap().unwrap();
    assert_eq!(examples.len(), 1);
    assert_eq!(examples[0].input, "hi");
}

#[test]
fn parse_examples_none_when_absent() {
    assert!(parse_examples(None).unwrap().is_none());
}

#[test]
fn parse_examples_rejects_non_array() {
    assert!(parse_examples(Some(&json!(42))).is_err());
}

#[test]
fn parse_strings_from_value() {
    let v = json!(["web_search", "fetch_webpage"]);
    let tools = parse_strings(Some(&v)).unwrap().unwrap();
    assert_eq!(tools, vec!["web_search", "fetch_webpage"]);
}

#[test]
fn parse_strings_none_when_absent() {
    assert!(parse_strings(None).unwrap().is_none());
}

#[test]
fn parse_strings_rejects_non_array() {
    assert!(parse_strings(Some(&json!("bad"))).is_err());
}

#[test]
fn parse_strings_rejects_non_string_element() {
    assert!(parse_strings(Some(&json!([42]))).is_err());
}

#[tokio::test]
async fn invalid_name_rejected() {
    let (_t, skills) = test_skills();
    let result = dispatch_create_skill(
        &skills,
        "user123",
        &json!({
            "name": "Bad Name!",
            "instructions": "some instructions",
        }),
    )
    .await;
    assert!(result.starts_with("Error:"));
    assert!(result.contains("lowercase letters"));
}

#[tokio::test]
async fn empty_instructions_rejected() {
    let (_t, skills) = test_skills();
    let result = dispatch_create_skill(
        &skills,
        "user123",
        &json!({
            "name": "empty",
            "instructions": "",
        }),
    )
    .await;
    assert!(result.starts_with("Error:"));
    assert!(result.contains("empty"));
}
