use serde_json::{json, Value};

use housebot_skills::Skills;

use crate::create_skill::{parse_examples, parse_strings, parse_triggers};

fn truncate_chars(value: &str, limit: usize) -> String {
    value.chars().take(limit).collect()
}

pub fn list_definition() -> Value {
    json!({
        "name": "list_skills",
        "description": "Browse the global skill marketplace: every skill with its description and \
            author, marked with whether the current user has enabled it. Use this to find a skill \
            to enable (enable_skill) before loading it with use_skill.",
        "input_schema": {"type": "object", "properties": {}}
    })
}

pub fn info_definition() -> Value {
    json!({
        "name": "skill_info",
        "description": "Show the full details of one custom skill: description, author, version, \
            triggers, recommended tools, example count, and an instruction preview.",
        "input_schema": {
            "type": "object",
            "properties": {
                "name": {"type": "string", "description": "The skill name to inspect."}
            },
            "required": ["name"]
        }
    })
}

pub fn delete_definition() -> Value {
    json!({
        "name": "delete_skill",
        "description": "Delete a custom skill by name. Only the skill's author or a delegated \
            editor may delete it.",
        "input_schema": {
            "type": "object",
            "properties": {
                "name": {"type": "string", "description": "The skill name to delete."}
            },
            "required": ["name"]
        }
    })
}

pub fn edit_definition() -> Value {
    json!({
        "name": "edit_skill",
        "description": "Update one or more fields of an existing custom skill in place — only the \
            fields you provide are changed, everything else is preserved. Automatically archives \
            the previous version. Only the skill's author or a delegated editor may edit it. Use \
            create_skill instead to make a brand-new skill.",
        "input_schema": {
            "type": "object",
            "properties": {
                "name": {"type": "string", "description": "The existing skill name to edit."},
                "instructions": {
                    "type": "string",
                    "description": "New core behavioral instructions. Omit to leave unchanged."
                },
                "description": {
                    "type": "string",
                    "description": "New human-readable description. Omit to leave unchanged."
                },
                "triggers": {
                    "type": "array",
                    "description": "Replaces the skill's trigger conditions entirely. Omit to \
                        leave the existing triggers unchanged.",
                    "items": {
                        "type": "object",
                        "properties": {
                            "trigger_type": {
                                "type": "string",
                                "enum": ["keyword", "intent", "always", "context"]
                            },
                            "value": {"type": "string"}
                        },
                        "required": ["trigger_type", "value"]
                    }
                },
                "enabled_tools": {
                    "type": "array",
                    "description": "Replaces the skill's recommended tools entirely. Omit to leave \
                        the existing list unchanged.",
                    "items": {"type": "string"}
                },
                "examples": {
                    "type": "array",
                    "description": "Replaces the skill's few-shot examples entirely. Omit to leave \
                        the existing examples unchanged.",
                    "items": {
                        "type": "object",
                        "properties": {
                            "input": {"type": "string"},
                            "output": {"type": "string"}
                        },
                        "required": ["input", "output"]
                    }
                }
            },
            "required": ["name"]
        }
    })
}

pub fn enable_definition() -> Value {
    json!({
        "name": "enable_skill",
        "description": "Enable a marketplace skill for the current user so it is listed and can be \
            loaded with use_skill. Each user chooses which skills to load; nothing is available \
            until enabled.",
        "input_schema": {
            "type": "object",
            "properties": {
                "name": {"type": "string", "description": "The marketplace skill name to enable."}
            },
            "required": ["name"]
        }
    })
}

pub fn disable_definition() -> Value {
    json!({
        "name": "disable_skill",
        "description": "Disable a previously enabled skill for the current user so it no longer \
            loads. Does not delete the skill from the marketplace.",
        "input_schema": {
            "type": "object",
            "properties": {
                "name": {"type": "string", "description": "The skill name to disable."}
            },
            "required": ["name"]
        }
    })
}

pub async fn dispatch_list_skills(skills: &Skills, enabled: &[String]) -> String {
    let all = skills.load_all().await;
    if all.is_empty() {
        return "No skills exist in the marketplace yet.".into();
    }
    let mut lines = vec!["Marketplace skills (✓ = enabled for you):".to_string()];
    for skill in all.values() {
        let mark = if enabled.iter().any(|n| n == &skill.name) {
            "✓"
        } else {
            "•"
        };
        let author = skill
            .created_by
            .as_deref()
            .map(|id| format!(" (by <@{id}>)"))
            .unwrap_or_default();
        lines.push(format!(
            "{} {} — {}{}",
            mark,
            skill.name,
            truncate_chars(skill.description_or_name(), 80),
            author,
        ));
    }
    lines.join("\n")
}

pub async fn dispatch_skill_info(skills: &Skills, args: &Value) -> String {
    let name = args
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_lowercase();
    match skills.get(&name).await {
        None => format!("Skill '{name}' not found."),
        Some(skill) => {
            let instructions = skill.effective_instructions();
            let mut preview = truncate_chars(instructions, 500);
            if instructions.chars().count() > 500 {
                preview.push('…');
            }
            let author = skill
                .created_by
                .as_deref()
                .map(|id| format!("\nAuthor: <@{id}>"))
                .unwrap_or_default();
            let editors = if skill.editors.is_empty() {
                String::new()
            } else {
                let list: Vec<String> = skill.editors.iter().map(|id| format!("<@{id}>")).collect();
                format!("\nEditors: {}", list.join(", "))
            };
            let triggers = if skill.has_triggers() {
                let list: Vec<String> = skill
                    .triggers
                    .iter()
                    .map(|t| format!("{}: {}", t.trigger_type, t.value))
                    .collect();
                format!("\nTriggers: {}", list.join("; "))
            } else {
                String::new()
            };
            let tools = if skill.enabled_tools.is_empty() {
                String::new()
            } else {
                format!("\nRecommended tools: {}", skill.enabled_tools.join(", "))
            };
            format!(
                "Skill: {}\nDescription: {}{}{}\nVersion: v{}{}{}\nExamples: {}\n\nInstructions:\n{}",
                skill.name,
                skill.description.as_deref().unwrap_or("(none)"),
                author,
                editors,
                skill.version,
                triggers,
                tools,
                skill.examples.len(),
                preview,
            )
        }
    }
}

pub async fn dispatch_delete_skill(skills: &Skills, author_id: &str, args: &Value) -> String {
    let name = args
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_lowercase();
    match skills.get(&name).await {
        None => format!("Skill '{name}' not found."),
        Some(skill) => {
            if !skill.can_edit(author_id) {
                return format!("⛔ Only the author or a delegated editor can delete **{name}**.");
            }
            match skills.delete(&name).await {
                Ok(true) => format!("✅ Skill **{name}** deleted."),
                _ => "Error: failed to delete skill.".into(),
            }
        }
    }
}

pub async fn dispatch_edit_skill(skills: &Skills, author_id: &str, args: &Value) -> String {
    let name = args
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_lowercase();
    let mut skill = match skills.get(&name).await {
        None => {
            return format!("Error: Skill '{name}' not found. Use create_skill to make a new one.")
        }
        Some(skill) => skill,
    };
    if !skill.can_edit(author_id) {
        return format!("⛔ Only the author or a delegated editor can edit **{name}**.");
    }
    let instructions = args.get("instructions").and_then(Value::as_str);
    let description = args.get("description").and_then(Value::as_str);
    let triggers = match parse_triggers(args.get("triggers")) {
        Ok(t) => t,
        Err(e) => return format!("Error: {e}"),
    };
    let enabled_tools = match parse_strings(args.get("enabled_tools")) {
        Ok(t) => t,
        Err(e) => return format!("Error: {e}"),
    };
    let examples = match parse_examples(args.get("examples")) {
        Ok(e) => e,
        Err(e) => return format!("Error: {e}"),
    };
    if instructions.is_none()
        && description.is_none()
        && triggers.is_none()
        && enabled_tools.is_none()
        && examples.is_none()
    {
        return "Error: provide at least one field to change.".into();
    }

    skill.bump_version();
    let new_version = skill.version;
    if let Some(instructions) = instructions {
        skill.instructions = instructions.to_string();
    }
    if let Some(description) = description {
        skill.description = Some(description.to_string());
    }
    if let Some(triggers) = triggers {
        skill.triggers = triggers;
    }
    if let Some(enabled_tools) = enabled_tools {
        skill.enabled_tools = enabled_tools;
    }
    if let Some(examples) = examples {
        skill.examples = examples;
    }
    if skills.save(skill).await.is_err() {
        return "Error: failed to save skill.".into();
    }
    format!("✅ Skill **{name}** updated to version {new_version}.")
}

#[cfg(test)]
mod tests {
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
    async fn list_empty() {
        let (_t, skills) = test_skills();
        assert!(dispatch_list_skills(&skills, &[])
            .await
            .contains("No skills"));
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
}
