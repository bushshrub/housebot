//! Unit tests for `manage_skills` (split out to keep the module under 400 lines).

use super::*;
use housebot_skills::{Skill, Skills};
use tempfile::TempDir;

fn test_skills() -> (TempDir, Skills) {
    let tmp = TempDir::new().unwrap();
    let skills = Skills::new(tmp.path().join("skills.json"));
    (tmp, skills)
}

fn skill(name: &str, author: &str) -> Skill {
    Skill {
        name: name.to_string(),
        description: Some(format!("desc of {name}")),
        instructions: "do the thing".into(),
        triggers: Vec::new(),
        enabled_tools: Vec::new(),
        examples: Vec::new(),
        version: 1,
        version_history: Vec::new(),
        created_by: Some(author.to_string()),
        editors: Vec::new(),
        created_at: 0,
        updated_at: 0,
        prompt: None,
    }
}

#[tokio::test]
async fn list_contains_builtin_skill_creator_by_default() {
    let (_t, skills) = test_skills();
    assert!(
        dispatch_list_skills(&skills, &[housebot_skills::SKILL_CREATOR_NAME.to_string()])
            .await
            .contains("✓ skill_creator")
    );
}

#[tokio::test]
async fn list_populated_marks_enabled() {
    let (_t, skills) = test_skills();
    skills.save(skill("greet", "1")).await.unwrap();
    skills.save(skill("recap", "1")).await.unwrap();
    let out = dispatch_list_skills(&skills, &["greet".to_string()]).await;
    assert!(out.contains("greet"));
    assert!(out.contains("desc of greet"));
    // enabled skill marked, un-enabled skill not marked
    assert!(out.contains("✓ greet"));
    assert!(out.contains("• recap"));
}

#[tokio::test]
async fn info_missing() {
    let (_t, skills) = test_skills();
    let out = dispatch_skill_info(&skills, &json!({"name": "nope"})).await;
    assert!(out.contains("not found"));
}

#[tokio::test]
async fn info_found() {
    let (_t, skills) = test_skills();
    skills.save(skill("greet", "1")).await.unwrap();
    let out = dispatch_skill_info(&skills, &json!({"name": "greet"})).await;
    assert!(out.contains("Skill: greet"));
    assert!(out.contains("v1"));
    assert!(out.contains("do the thing"));
}

#[tokio::test]
async fn delete_requires_author() {
    let (_t, skills) = test_skills();
    skills.save(skill("greet", "author1")).await.unwrap();
    let denied = dispatch_delete_skill(&skills, "intruder", &json!({"name": "greet"})).await;
    assert!(denied.contains("⛔"));
    assert!(skills.get("greet").await.is_some());
    let ok = dispatch_delete_skill(&skills, "author1", &json!({"name": "greet"})).await;
    assert!(ok.contains("deleted"));
    assert!(skills.get("greet").await.is_none());
}

#[tokio::test]
async fn edit_missing_skill() {
    let (_t, skills) = test_skills();
    let out = dispatch_edit_skill(
        &skills,
        "author1",
        &json!({"name": "nope", "instructions": "x"}),
    )
    .await;
    assert!(out.contains("not found"), "out: {out}");
}

#[tokio::test]
async fn edit_requires_author_or_editor() {
    let (_t, skills) = test_skills();
    skills.save(skill("greet", "author1")).await.unwrap();
    let denied = dispatch_edit_skill(
        &skills,
        "intruder",
        &json!({"name": "greet", "instructions": "hacked"}),
    )
    .await;
    assert!(denied.contains("⛔"), "denied: {denied}");
    let unchanged = skills.get("greet").await.unwrap();
    assert_eq!(unchanged.instructions, "do the thing");
    assert_eq!(unchanged.version, 1);
}

#[tokio::test]
async fn edit_updates_only_provided_fields_and_bumps_version() {
    let (_t, skills) = test_skills();
    let mut base = skill("greet", "author1");
    base.triggers = vec![housebot_skills::SkillTrigger {
        trigger_type: "keyword".into(),
        value: "hi".into(),
    }];
    base.enabled_tools = vec!["web_search".into()];
    skills.save(base).await.unwrap();

    let out = dispatch_edit_skill(
        &skills,
        "author1",
        &json!({"name": "greet", "instructions": "do the new thing"}),
    )
    .await;
    assert!(out.contains("updated to version 2"), "out: {out}");

    let updated = skills.get("greet").await.unwrap();
    assert_eq!(updated.instructions, "do the new thing");
    assert_eq!(updated.version, 2);
    assert_eq!(updated.version_history.len(), 1);
    // Fields not passed to edit_skill are preserved.
    assert_eq!(updated.triggers.len(), 1);
    assert_eq!(updated.enabled_tools, vec!["web_search".to_string()]);
    assert_eq!(updated.description.as_deref(), Some("desc of greet"));
}

#[tokio::test]
async fn edit_with_no_fields_rejected() {
    let (_t, skills) = test_skills();
    skills.save(skill("greet", "author1")).await.unwrap();
    let out = dispatch_edit_skill(&skills, "author1", &json!({"name": "greet"})).await;
    assert!(out.starts_with("Error:"), "out: {out}");
    assert_eq!(skills.get("greet").await.unwrap().version, 1);
}
