use serde_json::{json, Value};

use housebot_skills::{Skill, Skills};

pub fn definition() -> Value {
    json!({
        "name": "manage_skill",
        "description": "Save or delete a custom skill. A skill is a SKILL.md file with a name, a \
            description, and instructions; every skill is copied into the sandbox at \
            skills/<name>/, where you read it with the read tool. \
            Actions:\n\
            - 'save' — create a skill, or update the given fields of an existing one. A new skill \
              needs description and instructions. Only the author or a delegated editor may \
              update a skill.\n\
            - 'delete' — delete a skill. Only the author or a delegated editor may delete it.\n\
            Gather requirements through conversation, then present the final draft and obtain \
            the user's explicit approval before you save.",
        "input_schema": {
            "type": "object",
            "properties": {
                "action": {"type": "string", "enum": ["save", "delete"]},
                "name": {
                    "type": "string",
                    "description": "Skill name (letters, digits, hyphens, underscores)."
                },
                "description": {
                    "type": "string",
                    "description": "One sentence that says what the skill does and when to use it."
                },
                "instructions": {
                    "type": "string",
                    "description": "The SKILL.md body: what to do and how."
                }
            },
            "required": ["action", "name"]
        }
    })
}

pub async fn dispatch(skills: &Skills, user_id: &str, args: &Value) -> String {
    let name = args
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_lowercase();
    if let Err(error) = housebot_skills::validate_name(&name) {
        return format!("Error: {error}.");
    }
    match args.get("action").and_then(Value::as_str).unwrap_or("") {
        "save" => save(skills, user_id, &name, args).await,
        "delete" => delete(skills, user_id, &name).await,
        other => format!("Error: unknown manage_skill action `{other}`."),
    }
}

async fn save(skills: &Skills, user_id: &str, name: &str, args: &Value) -> String {
    let description = args.get("description").and_then(Value::as_str);
    let instructions = args.get("instructions").and_then(Value::as_str);
    if instructions.is_some_and(|i| i.trim().is_empty()) {
        return "Error: Skill instructions cannot be empty.".into();
    }
    let (skill, verb) = match skills.get(name).await {
        Some(mut existing) => {
            if !existing.can_edit(user_id) {
                return format!("⛔ Only the author or a delegated editor can update **{name}**.");
            }
            if description.is_none() && instructions.is_none() {
                return "Error: provide description or instructions to change.".into();
            }
            if let Some(description) = description {
                existing.description = Some(description.to_string());
            }
            if let Some(instructions) = instructions {
                existing.instructions = instructions.to_string();
            }
            (existing, "updated")
        }
        None => {
            let (Some(description), Some(instructions)) = (description, instructions) else {
                return "Error: a new skill needs both description and instructions.".into();
            };
            let skill = Skill {
                name: name.to_string(),
                description: Some(description.to_string()),
                instructions: instructions.to_string(),
                created_by: Some(user_id.to_string()),
                ..Skill::default()
            };
            (skill, "created")
        }
    };
    match skills.save(skill).await {
        Ok(()) => format!("✅ Skill **{name}** {verb}."),
        Err(error) => format!("Error: failed to save skill: {error}."),
    }
}

async fn delete(skills: &Skills, user_id: &str, name: &str) -> String {
    match skills.get(name).await {
        None => format!("Error: Skill '{name}' not found."),
        Some(skill) if !skill.can_edit(user_id) => {
            format!("⛔ Only the author or a delegated editor can delete **{name}**.")
        }
        Some(_) => match skills.delete(name).await {
            Ok(true) => format!("✅ Skill **{name}** deleted."),
            _ => "Error: failed to delete skill.".into(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn test_skills() -> (TempDir, Skills) {
        let tmp = TempDir::new().unwrap();
        let skills = Skills::new(tmp.path());
        (tmp, skills)
    }

    fn save_args(name: &str, description: &str, instructions: &str) -> Value {
        json!({
            "action": "save",
            "name": name,
            "description": description,
            "instructions": instructions,
        })
    }

    #[tokio::test]
    async fn save_creates_a_skill_owned_by_the_caller() {
        let (_t, skills) = test_skills();
        let out = dispatch(
            &skills,
            "author1",
            &save_args("greet", "Say hi", "Say hello."),
        )
        .await;
        assert!(out.contains("created"), "out: {out}");
        let skill = skills.get("greet").await.unwrap();
        assert_eq!(skill.created_by.as_deref(), Some("author1"));
        assert_eq!(skill.description.as_deref(), Some("Say hi"));
    }

    #[tokio::test]
    async fn a_new_skill_needs_a_description_and_instructions() {
        let (_t, skills) = test_skills();
        let out = dispatch(
            &skills,
            "author1",
            &json!({"action": "save", "name": "greet", "instructions": "Say hello."}),
        )
        .await;
        assert!(out.starts_with("Error:"), "out: {out}");
        assert!(skills.get("greet").await.is_none());
    }

    #[tokio::test]
    async fn save_updates_only_the_given_fields() {
        let (_t, skills) = test_skills();
        dispatch(
            &skills,
            "author1",
            &save_args("greet", "Say hi", "Say hello."),
        )
        .await;
        let out = dispatch(
            &skills,
            "author1",
            &json!({"action": "save", "name": "greet", "instructions": "Wave."}),
        )
        .await;
        assert!(out.contains("updated"), "out: {out}");
        let skill = skills.get("greet").await.unwrap();
        assert_eq!(skill.instructions.trim(), "Wave.");
        assert_eq!(skill.description.as_deref(), Some("Say hi"));
    }

    #[tokio::test]
    async fn only_the_author_can_update_or_delete() {
        let (_t, skills) = test_skills();
        dispatch(
            &skills,
            "author1",
            &save_args("greet", "Say hi", "Say hello."),
        )
        .await;

        let denied = dispatch(&skills, "intruder", &save_args("greet", "x", "hacked")).await;
        assert!(denied.contains("⛔"), "denied: {denied}");
        let denied = dispatch(
            &skills,
            "intruder",
            &json!({"action": "delete", "name": "greet"}),
        )
        .await;
        assert!(denied.contains("⛔"), "denied: {denied}");
        assert_eq!(
            skills.get("greet").await.unwrap().instructions.trim(),
            "Say hello."
        );

        let ok = dispatch(
            &skills,
            "author1",
            &json!({"action": "delete", "name": "greet"}),
        )
        .await;
        assert!(ok.contains("deleted"), "ok: {ok}");
        assert!(skills.get("greet").await.is_none());
    }

    #[tokio::test]
    async fn names_that_are_not_a_safe_path_segment_are_refused() {
        let (_t, skills) = test_skills();
        let out = dispatch(&skills, "author1", &save_args("../etc", "x", "y")).await;
        assert!(out.starts_with("Error:"), "out: {out}");
    }
}
