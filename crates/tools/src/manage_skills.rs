use serde_json::{json, Value};

use housebot_skills::Skills;

fn truncate_chars(value: &str, limit: usize) -> String {
    value.chars().take(limit).collect()
}

pub fn list_definition() -> Value {
    json!({
        "name": "list_skills",
        "description": "List every custom skill with its description and author. Use this to see \
            what skills exist before loading one with use_skill or before creating a new one.",
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

pub async fn dispatch_list_skills(skills: &Skills) -> String {
    let all = skills.load_all().await;
    if all.is_empty() {
        return "No skills defined yet.".into();
    }
    let mut lines = vec!["Skills:".to_string()];
    for skill in all.values() {
        let author = skill
            .created_by
            .as_deref()
            .map(|id| format!(" (by <@{id}>)"))
            .unwrap_or_default();
        lines.push(format!(
            "• {} — {}{}",
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
        assert!(dispatch_list_skills(&skills).await.contains("No skills"));
    }

    #[tokio::test]
    async fn list_populated() {
        let (_t, skills) = test_skills();
        skills.save(skill("greet", "1")).await.unwrap();
        let out = dispatch_list_skills(&skills).await;
        assert!(out.contains("greet"));
        assert!(out.contains("desc of greet"));
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
}
