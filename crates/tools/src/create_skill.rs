use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use housebot_skills::{Skill, Skills};

/// OpenAI-style tool definition (internal `input_schema` form).
pub fn definition() -> Value {
    json!({
        "name": "create_skill",
        "description": "Create or update a custom skill — a packaged set of instructions stored \
            as a directory on disk. The instructions are loaded into your context on demand via \
            `use_skill`; you then follow them using your normal tools. Keep the instructions \
            focused: bulk material belongs in the skill's references/ directory, which is read \
            only when needed. Gather requirements from the user through conversation, then \
            present the final draft and obtain their explicit approval before calling this tool.",
        "input_schema": {
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Unique skill name (lowercase letters, numbers, underscores only)."
                },
                "instructions": {
                    "type": "string",
                    "description": "The core behavior instructions for this skill — what it should do and how it should behave."
                },
                "description": {
                    "type": "string",
                    "description": "Optional human-readable description of what this skill does, shown in list_skills."
                },
                "enabled_tools": {
                    "type": "array",
                    "description": "Tool names this skill is expected to use (e.g. 'web_search', \
                        'fetch_webpage'), surfaced as recommendations when the skill is loaded. \
                        Advisory only — it does not restrict which tools you may call. Leave \
                        empty for a text-only skill.",
                    "items": {"type": "string"}
                },
                "update": {
                    "type": "boolean",
                    "description": "Set true to overwrite an existing skill. Creating over an \
                        existing name fails without it, so an accidental clobber is not silent."
                }
            },
            "required": ["name", "instructions"]
        }
    })
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

pub(crate) async fn create_skill(
    skills: &Skills,
    author_id: &str,
    name: &str,
    instructions: &str,
    description: Option<&str>,
    enabled_tools: Option<Vec<String>>,
    update: bool,
) -> String {
    if !valid_name(name) {
        return "Error: Skill name must be lowercase letters, numbers, and underscores only."
            .into();
    }
    if instructions.trim().is_empty() {
        return "Error: Skill instructions cannot be empty.".into();
    }

    match skills.get(name).await {
        Some(mut existing) => {
            if !update {
                return format!(
                    "Error: Skill '{name}' already exists. Pass update=true to overwrite it."
                );
            }
            if !existing.can_edit(author_id) {
                return format!("⛔ Only the author or a delegated editor can update **{name}**.");
            }
            existing.instructions = instructions.to_string();
            if let Some(desc) = description {
                existing.description = Some(desc.to_string());
            }
            if let Some(ref t) = enabled_tools {
                existing.enabled_tools = t.clone();
            }
            if skills.save(existing).await.is_err() {
                return "Error: failed to save skill.".into();
            }
            format!("✅ Skill **{name}** updated.")
        }
        None => {
            let now = now_secs();
            let skill = Skill {
                name: name.to_string(),
                description: description.map(String::from),
                instructions: instructions.to_string(),
                enabled_tools: enabled_tools.unwrap_or_default(),
                created_by: Some(author_id.to_string()),
                created_at: now,
                updated_at: now,
                ..Skill::default()
            };
            if skills.save(skill).await.is_err() {
                return "Error: failed to save skill.".into();
            }
            format!("✅ Skill **{name}** created successfully.")
        }
    }
}

pub(crate) fn parse_strings(val: Option<&Value>) -> Result<Option<Vec<String>>, String> {
    match val {
        None => Ok(None),
        Some(v) => {
            let arr = v
                .as_array()
                .ok_or_else(|| "Expected an array of strings".to_string())?;
            let strings: Result<Vec<_>, String> = arr
                .iter()
                .map(|item| {
                    item.as_str()
                        .ok_or_else(|| "Each element must be a string".to_string())
                        .map(String::from)
                })
                .collect();
            Ok(Some(strings?))
        }
    }
}

/// Parse `create_skill` tool-call arguments and dispatch to the implementation.
pub async fn dispatch_create_skill(skills: &Skills, author_id: &str, args: &Value) -> String {
    let name = args
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_lowercase();
    let instructions = args
        .get("instructions")
        .and_then(Value::as_str)
        .unwrap_or("");
    let description = args.get("description").and_then(Value::as_str);
    let enabled_tools = match parse_strings(args.get("enabled_tools")) {
        Ok(t) => t,
        Err(e) => return format!("Error: {e}"),
    };
    let update = args.get("update").and_then(Value::as_bool).unwrap_or(false);

    create_skill(
        skills,
        author_id,
        &name,
        instructions,
        description,
        enabled_tools,
        update,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use housebot_skills::Skills;
    use serde_json::json;
    use tempfile::TempDir;

    fn test_skills() -> (TempDir, Skills) {
        let tmp = TempDir::new().unwrap();
        let skills = Skills::new(tmp.path());
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
                "enabled_tools": ["web_search"]
            }),
        )
        .await;
        assert!(result.contains("created"), "result: {result}");
        let skill = skills.get("summarizer").await.unwrap();
        assert_eq!(skill.description.as_deref(), Some("A summarization skill"));
        assert_eq!(skill.enabled_tools, vec!["web_search"]);
        assert_eq!(skill.created_by.as_deref(), Some("user123"));
    }

    #[tokio::test]
    async fn update_requires_the_update_flag() {
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

        let clobber = dispatch_create_skill(
            &skills,
            "user123",
            &json!({
                "name": "greeter",
                "instructions": "Say hello warmly",
            }),
        )
        .await;
        assert!(clobber.starts_with("Error:"), "result: {clobber}");
        assert_eq!(
            skills.get("greeter").await.unwrap().instructions.trim(),
            "Say hello"
        );

        let result = dispatch_create_skill(
            &skills,
            "user123",
            &json!({
                "name": "greeter",
                "instructions": "Say hello warmly",
                "update": true,
            }),
        )
        .await;
        assert!(result.contains("updated"), "result: {result}");
        assert_eq!(
            skills.get("greeter").await.unwrap().instructions.trim(),
            "Say hello warmly"
        );
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
                "update": true,
            }),
        )
        .await;
        assert!(result.contains("⛔"));
    }

    #[tokio::test]
    async fn creating_over_an_existing_skill_is_refused() {
        let (_t, skills) = test_skills();
        dispatch_create_skill(
            &skills,
            "user1",
            &json!({
                "name": "s",
                "instructions": "original instructions",
            }),
        )
        .await;

        let result = dispatch_create_skill(
            &skills,
            "user1",
            &json!({
                "name": "s",
                "instructions": "replacement instructions",
            }),
        )
        .await;
        assert!(result.contains("already exists"), "result: {result}");
        assert_eq!(
            skills.get("s").await.unwrap().instructions.trim(),
            "original instructions"
        );
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
                "enabled_tools": ["web_search"],
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
                "update": true,
            }),
        )
        .await;
        assert!(result.contains("updated"), "result: {result}");

        let skill = skills.get("s").await.unwrap();
        assert_eq!(skill.instructions.trim(), "updated");
        // Omitted fields keep their previous values.
        assert_eq!(
            skill.enabled_tools.len(),
            1,
            "enabled_tools should be preserved"
        );
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
}
