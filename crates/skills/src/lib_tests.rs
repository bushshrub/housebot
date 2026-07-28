//! Store, model, and trigger tests for `skills`.

use super::*;

#[tokio::test]
async fn skill_creator_is_builtin_and_not_persisted() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("skills.json");
    let skills = Skills::new(&path);

    let creator = skills.get(SKILL_CREATOR_NAME).await.unwrap();
    assert_eq!(creator.version, 1);
    assert!(creator.created_by.is_none());
    assert!(creator.enabled_tools.contains(&"create_skill".to_string()));
    assert_eq!(
        skills.delete(SKILL_CREATOR_NAME).await.unwrap_err().kind(),
        std::io::ErrorKind::PermissionDenied
    );

    skills
        .save(Skill {
            name: "custom".to_string(),
            description: None,
            instructions: "Do one thing well.".to_string(),
            triggers: Vec::new(),
            enabled_tools: Vec::new(),
            examples: Vec::new(),
            version: 1,
            version_history: Vec::new(),
            created_by: Some("1".to_string()),
            editors: Vec::new(),
            created_at: 0,
            updated_at: 0,
            prompt: None,
        })
        .await
        .unwrap();

    let persisted = tokio::fs::read_to_string(path).await.unwrap();
    assert!(persisted.contains("\"custom\""));
    assert!(!persisted.contains(SKILL_CREATOR_NAME));
}

#[tokio::test]
async fn corrupt_store_is_not_overwritten() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("skills.json");
    tokio::fs::write(&path, "{not json").await.unwrap();
    let skills = Skills::new(&path);

    assert!(skills.load_all().await.contains_key(SKILL_CREATOR_NAME));
    let mut custom = builtin_skill_creator();
    custom.name = "custom".to_string();
    assert_eq!(
        skills.save(custom).await.unwrap_err().kind(),
        std::io::ErrorKind::InvalidData
    );
    assert_eq!(tokio::fs::read_to_string(path).await.unwrap(), "{not json");
}
use tempfile::TempDir;

fn store() -> (TempDir, Skills) {
    let tmp = TempDir::new().unwrap();
    let s = Skills::new(tmp.path().join("skills.json"));
    (tmp, s)
}

fn skill(name: &str, desc: Option<&str>, instructions: &str) -> Skill {
    Skill {
        name: name.to_string(),
        description: desc.map(String::from),
        instructions: instructions.to_string(),
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
    }
}

fn full_skill(name: &str) -> Skill {
    Skill {
        name: name.to_string(),
        description: Some("A full-featured skill".into()),
        instructions: "Do the thing".into(),
        triggers: vec![SkillTrigger {
            trigger_type: "keyword".into(),
            value: "standup".into(),
        }],
        enabled_tools: vec!["web_search".into(), "fetch_webpage".into()],
        examples: vec![SkillExample {
            input: "summarize my week".into(),
            output: "Here's your weekly summary...".into(),
        }],
        version: 2,
        version_history: vec![SkillArchive {
            version: 1,
            description: None,
            instructions: "Old instructions".into(),
            triggers: vec![],
            enabled_tools: vec![],
            examples: vec![],
            archived_at: 100,
        }],
        created_by: Some("author".into()),
        editors: vec!["editor1".into()],
        created_at: 50,
        updated_at: 200,
        prompt: None,
    }
}

#[tokio::test]
async fn load_all_contains_only_builtin_when_no_file() {
    let (_t, s) = store();
    let all = s.load_all().await;
    assert_eq!(all.len(), 1);
    assert!(all.contains_key(SKILL_CREATOR_NAME));
}

#[tokio::test]
async fn save_and_load_skill() {
    let (_t, s) = store();
    s.save(skill("greet", Some("Say hello"), "Hello!"))
        .await
        .unwrap();
    let all = s.load_all().await;
    assert_eq!(all.get("greet").unwrap().effective_instructions(), "Hello!");
}

#[tokio::test]
async fn get_existing_skill() {
    let (_t, s) = store();
    s.save(skill("greet", Some("Say hello"), "Hello!"))
        .await
        .unwrap();
    assert_eq!(s.get("greet").await.unwrap().name, "greet");
}

#[tokio::test]
async fn get_missing_returns_none() {
    let (_t, s) = store();
    assert!(s.get("nonexistent").await.is_none());
}

#[tokio::test]
async fn save_overwrites_existing() {
    let (_t, s) = store();
    s.save(skill("greet", Some("old"), "Hi")).await.unwrap();
    s.save(skill("greet", Some("new"), "Hey")).await.unwrap();
    assert_eq!(
        s.get("greet").await.unwrap().description.as_deref(),
        Some("new")
    );
}

#[tokio::test]
async fn delete_existing_skill() {
    let (_t, s) = store();
    s.save(skill("greet", Some("Say hello"), "Hello!"))
        .await
        .unwrap();
    assert!(s.delete("greet").await.unwrap());
    assert!(s.get("greet").await.is_none());
}

#[tokio::test]
async fn delete_missing_returns_false() {
    let (_t, s) = store();
    assert!(!s.delete("nonexistent").await.unwrap());
}

#[tokio::test]
async fn multiple_skills_coexist() {
    let (_t, s) = store();
    s.save(skill("a", None, "A instructions")).await.unwrap();
    s.save(skill("b", None, "B instructions")).await.unwrap();
    let all = s.load_all().await;
    assert!(all.contains_key("a"));
    assert!(all.contains_key("b"));
}

#[test]
fn skill_without_description_uses_name() {
    let sk = skill("a", None, "A instructions");
    assert_eq!(sk.description_or_name(), "a");
}

#[test]
fn effective_instructions_falls_back_to_legacy_prompt() {
    let mut sk = skill("x", None, "");
    sk.prompt = Some("legacy prompt".into());
    assert_eq!(sk.effective_instructions(), "legacy prompt");
}

#[test]
fn effective_instructions_prefers_instructions_over_prompt() {
    let mut sk = skill("x", None, "new instructions");
    sk.prompt = Some("old prompt".into());
    assert_eq!(sk.effective_instructions(), "new instructions");
}

#[test]
fn migrate_from_prompt_moves_to_instructions() {
    let mut sk = Skill {
        name: "x".into(),
        description: None,
        instructions: String::new(),
        triggers: Vec::new(),
        enabled_tools: Vec::new(),
        examples: Vec::new(),
        version: 1,
        version_history: Vec::new(),
        created_by: None,
        editors: Vec::new(),
        created_at: 0,
        updated_at: 0,
        prompt: Some("legacy".into()),
    };
    sk.migrate_from_prompt();
    assert_eq!(sk.instructions, "legacy");
    assert!(sk.prompt.is_none());
}

#[test]
fn bump_version_archives_and_increments() {
    let mut sk = skill("x", None, "v1 instructions");
    sk.description = Some("v1 desc".into());
    sk.triggers = vec![SkillTrigger {
        trigger_type: "keyword".into(),
        value: "test".into(),
    }];
    sk.enabled_tools = vec!["search".into()];
    assert_eq!(sk.version, 1);
    sk.bump_version();
    assert_eq!(sk.version, 2);
    assert_eq!(sk.version_history.len(), 1);
    assert_eq!(sk.version_history[0].version, 1);
    assert_eq!(sk.version_history[0].instructions, "v1 instructions");
    assert_eq!(
        sk.version_history[0].description.as_deref(),
        Some("v1 desc")
    );
    assert_eq!(sk.version_history[0].triggers.len(), 1);
    assert_eq!(sk.version_history[0].enabled_tools, vec!["search"]);
}

#[test]
fn full_skill_round_trip() {
    let sk = full_skill("test_skill");
    assert_eq!(sk.name, "test_skill");
    assert_eq!(sk.triggers.len(), 1);
    assert_eq!(sk.enabled_tools.len(), 2);
    assert_eq!(sk.examples.len(), 1);
    assert_eq!(sk.version, 2);
    assert!(sk.has_triggers());
}

#[test]
fn has_triggers_false_when_empty() {
    let sk = skill("x", None, "instructions");
    assert!(!sk.has_triggers());
}

fn skill_with_triggers(triggers: Vec<(&str, &str)>) -> Skill {
    let mut sk = skill("x", None, "instructions");
    sk.triggers = triggers
        .into_iter()
        .map(|(trigger_type, value)| SkillTrigger {
            trigger_type: trigger_type.into(),
            value: value.into(),
        })
        .collect();
    sk
}

#[test]
fn matches_message_keyword_case_insensitive() {
    let sk = skill_with_triggers(vec![("keyword", "Standup")]);
    assert!(sk.matches_message("time for the daily standup"));
    assert!(sk.matches_message("STANDUP now"));
}

#[test]
fn matches_message_keyword_miss() {
    let sk = skill_with_triggers(vec![("keyword", "standup")]);
    assert!(!sk.matches_message("what's for lunch"));
}

#[test]
fn matches_message_always_fires() {
    let sk = skill_with_triggers(vec![("always", "")]);
    assert!(sk.matches_message("anything at all"));
}

#[test]
fn matches_message_intent_is_advisory() {
    let sk = skill_with_triggers(vec![("intent", "user wants a summary")]);
    assert!(!sk.matches_message("user wants a summary"));
}

#[test]
fn matches_message_no_triggers() {
    let sk = skill("x", None, "instructions");
    assert!(!sk.matches_message("standup"));
}

#[tokio::test]
async fn legacy_prompt_is_migrated_on_load() {
    let (_t, s) = store();
    // Write old-format JSON with `prompt` field
    let old_json = r#"{"greet":{"name":"greet","description":"old","prompt":"Hello!"}}"#;
    tokio::fs::write(&s.path, old_json).await.unwrap();
    let all = s.load_all().await;
    let skill = all.get("greet").unwrap();
    assert_eq!(skill.effective_instructions(), "Hello!");
    assert_eq!(skill.instructions, "Hello!");
    // prompt should be None after migration
    assert!(skill.prompt.is_none());
    // migrated legacy skills should be at version 1
    assert_eq!(skill.version, 1);
}

#[tokio::test]
async fn new_skills_dont_write_prompt_field() {
    let (_t, s) = store();
    s.save(skill("new_skill", None, "new instructions"))
        .await
        .unwrap();
    let raw = tokio::fs::read_to_string(&s.path).await.unwrap();
    assert!(!raw.contains("\"prompt\""));
    assert!(raw.contains("\"instructions\""));
}
