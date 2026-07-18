use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use housebot_skills::{Skill, SkillExample, SkillTrigger, Skills};

/// OpenAI-style tool definition (internal `input_schema` form).
pub fn definition() -> Value {
    json!({
        "name": "create_skill",
        "description": "Create or update a custom skill — a packaged unit of capability with \
            trigger conditions, instructions, authorized tools, and few-shot examples. \
            Gather requirements from the user through conversation, then present the final \
            draft to the user and obtain their explicit approval before calling this tool. \
            When updating an existing skill, provide the correct version number to trigger \
            automatic version archiving.",
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
                    "description": "Optional human-readable description of what this skill does."
                },
                "triggers": {
                    "type": "array",
                    "description": "Optional conditions that determine when the skill activates. \
                        Each trigger has a type ('keyword', 'intent', 'always', 'context') and a value.",
                    "items": {
                        "type": "object",
                        "properties": {
                            "trigger_type": {
                                "type": "string",
                                "enum": ["keyword", "intent", "always", "context"],
                                "description": "'keyword' — activate when specific terms are mentioned; \
                                    'intent' — activate when user intent matches description; \
                                    'always' — always available as a fallback; \
                                    'context' — activate based on conversation context."
                            },
                            "value": {
                                "type": "string",
                                "description": "The keyword phrase, intent description, or context that triggers this skill."
                            }
                        },
                        "required": ["trigger_type", "value"]
                    }
                },
                "enabled_tools": {
                    "type": "array",
                    "description": "Tool names this skill is authorized to call during execution (e.g. \
                        'web_search', 'fetch_webpage'). Leave empty to restrict the skill to text-only responses.",
                    "items": {"type": "string"}
                },
                "examples": {
                    "type": "array",
                    "description": "Optional few-shot input/output examples for consistent behavior.",
                    "items": {
                        "type": "object",
                        "properties": {
                            "input": {"type": "string", "description": "Example user input."},
                            "output": {"type": "string", "description": "Expected skill output."}
                        },
                        "required": ["input", "output"]
                    }
                },
                "version": {
                    "type": "integer",
                    "description": "Current version number. Omit (or set to 0) for new skills. \
                        When updating, provide the existing version to archive it automatically."
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

#[allow(clippy::too_many_arguments)]
pub(crate) async fn create_skill(
    skills: &Skills,
    author_id: &str,
    name: &str,
    instructions: &str,
    description: Option<&str>,
    triggers: Option<Vec<SkillTrigger>>,
    enabled_tools: Option<Vec<String>>,
    examples: Option<Vec<SkillExample>>,
    version: u64,
) -> String {
    if !valid_name(name) {
        return "Error: Skill name must be lowercase letters, numbers, and underscores only."
            .into();
    }
    if instructions.trim().is_empty() {
        return "Error: Skill instructions cannot be empty.".into();
    }

    let now = now_secs();

    match skills.get(name).await {
        Some(mut existing) => {
            // Update path: require version to match the existing record exactly
            if version != existing.version as u64 {
                return format!(
                    "Error: Skill '{name}' exists at version {} but version {} was supplied. \
                     Provide the exact current version to update.",
                    existing.version, version
                );
            }
            if !existing.can_edit(author_id) {
                return format!("⛔ Only the author or a delegated editor can update **{name}**.");
            }
            existing.bump_version();
            let new_version = existing.version;
            existing.instructions = instructions.to_string();
            if let Some(desc) = description {
                existing.description = Some(desc.to_string());
            }
            if let Some(ref t) = triggers {
                existing.triggers = t.clone();
            }
            if let Some(ref t) = enabled_tools {
                existing.enabled_tools = t.clone();
            }
            if let Some(ref e) = examples {
                existing.examples = e.clone();
            }
            if skills.save(existing).await.is_err() {
                return "Error: failed to save skill.".into();
            }
            format!("✅ Skill **{name}** updated to version {new_version}.")
        }
        None => {
            // Create path: require version 0
            if version != 0 {
                return format!(
                    "Error: Skill '{name}' does not exist — use version 0 to create a new skill."
                );
            }
            let skill = Skill {
                name: name.to_string(),
                description: description.map(String::from),
                instructions: instructions.to_string(),
                triggers: triggers.unwrap_or_default(),
                enabled_tools: enabled_tools.unwrap_or_default(),
                examples: examples.unwrap_or_default(),
                version: 1,
                version_history: Vec::new(),
                created_by: Some(author_id.to_string()),
                editors: Vec::new(),
                created_at: now,
                updated_at: now,
                prompt: None,
            };
            if skills.save(skill).await.is_err() {
                return "Error: failed to save skill.".into();
            }
            format!("✅ Skill **{name}** (v1) created successfully.")
        }
    }
}

fn parse_triggers(val: Option<&Value>) -> Option<Vec<SkillTrigger>> {
    val.map(|v| {
        v.as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|item| {
                        let trigger_type = item.get("trigger_type")?.as_str()?.to_string();
                        let value = item.get("value")?.as_str()?.to_string();
                        Some(SkillTrigger {
                            trigger_type,
                            value,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default()
    })
}

fn parse_examples(val: Option<&Value>) -> Option<Vec<SkillExample>> {
    val.map(|v| {
        v.as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|item| {
                        let input = item.get("input")?.as_str()?.to_string();
                        let output = item.get("output")?.as_str()?.to_string();
                        Some(SkillExample { input, output })
                    })
                    .collect()
            })
            .unwrap_or_default()
    })
}

fn parse_strings(val: Option<&Value>) -> Option<Vec<String>> {
    val.map(|v| {
        v.as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|item| item.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default()
    })
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
    let triggers = parse_triggers(args.get("triggers"));
    let enabled_tools = parse_strings(args.get("enabled_tools"));
    let examples = parse_examples(args.get("examples"));
    let version = args.get("version").and_then(Value::as_u64).unwrap_or(0);

    create_skill(
        skills,
        author_id,
        &name,
        instructions,
        description,
        triggers,
        enabled_tools,
        examples,
        version,
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
        let triggers = parse_triggers(Some(&v)).unwrap();
        assert_eq!(triggers.len(), 2);
        assert_eq!(triggers[0].trigger_type, "keyword");
        assert_eq!(triggers[1].value, "greeting");
    }

    #[test]
    fn parse_triggers_none_when_absent() {
        assert!(parse_triggers(None).is_none());
    }

    #[test]
    fn parse_examples_from_value() {
        let v = json!([
            {"input": "hi", "output": "hello back"},
        ]);
        let examples = parse_examples(Some(&v)).unwrap();
        assert_eq!(examples.len(), 1);
        assert_eq!(examples[0].input, "hi");
    }

    #[test]
    fn parse_examples_none_when_absent() {
        assert!(parse_examples(None).is_none());
    }

    #[test]
    fn parse_strings_from_value() {
        let v = json!(["web_search", "fetch_webpage"]);
        let tools = parse_strings(Some(&v)).unwrap();
        assert_eq!(tools, vec!["web_search", "fetch_webpage"]);
    }

    #[test]
    fn parse_strings_none_when_absent() {
        assert!(parse_strings(None).is_none());
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
