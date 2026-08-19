use serde_json::{json, Value};

use housebot_skills::Skills;

use crate::create_skill::parse_strings;

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

pub fn read_file_definition() -> Value {
    json!({
        "name": "read_skill_file",
        "description": "Read one file from a skill's references/ directory. Skill instructions \
            list their reference files; load one only when the instructions call for it, so bulk \
            material stays out of your context until it is needed.",
        "input_schema": {
            "type": "object",
            "properties": {
                "skill": {"type": "string", "description": "The skill that owns the file."},
                "file": {"type": "string", "description": "File name within references/, exactly as listed."}
            },
            "required": ["skill", "file"]
        }
    })
}

pub fn run_script_definition() -> Value {
    json!({
        "name": "run_skill_script",
        "description": "Run one script from a skill's scripts/ directory inside the sandbox and \
            return its output. Supported types are .py, .sh, and .js. The script never opens \
            network access on its own, and has NO network unless the session already cloned a \
            repository — gather any data you need with web_search or fetch_webpage first and pass \
            it in through args. Run a script only when the skill's instructions call for it.",
        "input_schema": {
            "type": "object",
            "properties": {
                "skill": {"type": "string", "description": "The skill that owns the script."},
                "file": {"type": "string", "description": "Script name within scripts/, exactly as listed."},
                "args": {
                    "type": "array",
                    "description": "Command-line arguments passed to the script.",
                    "items": {"type": "string"}
                }
            },
            "required": ["skill", "file"]
        }
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
            let tools = if skill.enabled_tools.is_empty() {
                String::new()
            } else {
                format!("\nRecommended tools: {}", skill.enabled_tools.join(", "))
            };
            format!(
                "Skill: {}\nDescription: {}{}{}{}{}\n\nInstructions:\n{}",
                skill.name,
                skill.description.as_deref().unwrap_or("(none)"),
                author,
                editors,
                tools,
                skill.bundled_summary(),
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
    if let Some(i) = instructions {
        if i.trim().is_empty() {
            return "Error: Skill instructions cannot be empty.".into();
        }
    }
    let description = args.get("description").and_then(Value::as_str);
    let enabled_tools = match parse_strings(args.get("enabled_tools")) {
        Ok(t) => t,
        Err(e) => return format!("Error: {e}"),
    };
    if instructions.is_none() && description.is_none() && enabled_tools.is_none() {
        return "Error: provide at least one field to change.".into();
    }

    if let Some(instructions) = instructions {
        skill.instructions = instructions.to_string();
    }
    if let Some(description) = description {
        skill.description = Some(description.to_string());
    }
    if let Some(enabled_tools) = enabled_tools {
        skill.enabled_tools = enabled_tools;
    }
    if skills.save(skill).await.is_err() {
        return "Error: failed to save skill.".into();
    }
    format!("✅ Skill **{name}** updated.")
}

#[cfg(test)]
mod tests {
    use super::*;
    use housebot_skills::{Skill, Skills};
    use tempfile::TempDir;

    fn test_skills() -> (TempDir, Skills) {
        let tmp = TempDir::new().unwrap();
        let skills = Skills::new(tmp.path());
        (tmp, skills)
    }

    fn skill(name: &str, author: &str) -> Skill {
        Skill {
            name: name.to_string(),
            description: Some(format!("desc of {name}")),
            instructions: "do the thing".into(),
            created_by: Some(author.to_string()),
            ..Skill::default()
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
        assert_eq!(unchanged.instructions.trim(), "do the thing");
    }

    #[tokio::test]
    async fn edit_updates_only_provided_fields() {
        let (_t, skills) = test_skills();
        let mut base = skill("greet", "author1");
        base.enabled_tools = vec!["web_search".into()];
        skills.save(base).await.unwrap();

        let out = dispatch_edit_skill(
            &skills,
            "author1",
            &json!({"name": "greet", "instructions": "do the new thing"}),
        )
        .await;
        assert!(out.contains("updated"), "out: {out}");

        let updated = skills.get("greet").await.unwrap();
        assert_eq!(updated.instructions.trim(), "do the new thing");
        // Fields not passed to edit_skill are preserved.
        assert_eq!(updated.enabled_tools, vec!["web_search".to_string()]);
        assert_eq!(updated.description.as_deref(), Some("desc of greet"));
    }

    #[test]
    fn bundled_tool_definitions_are_well_formed() {
        let read = read_file_definition();
        assert_eq!(read["name"], "read_skill_file");
        assert_eq!(read["input_schema"]["required"], json!(["skill", "file"]));

        let run = run_script_definition();
        assert_eq!(run["name"], "run_skill_script");
        assert_eq!(run["input_schema"]["required"], json!(["skill", "file"]));
        // The network constraint must reach the model, not just the runtime,
        // and it is conditional: a clone earlier in the session leaves the
        // sandbox networked for everything that follows.
        let description = run["description"].as_str().unwrap();
        assert!(description.contains("NO network"));
        assert!(description.contains("unless the session already cloned"));
    }

    #[tokio::test]
    async fn edit_with_no_fields_rejected() {
        let (_t, skills) = test_skills();
        skills.save(skill("greet", "author1")).await.unwrap();
        let out = dispatch_edit_skill(&skills, "author1", &json!({"name": "greet"})).await;
        assert!(out.starts_with("Error:"), "out: {out}");
        assert_eq!(
            skills.get("greet").await.unwrap().instructions.trim(),
            "do the thing"
        );
    }
}
