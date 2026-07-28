use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use housebot_skills::{Skill, SkillExample, SkillTrigger, Skills};

/// OpenAI-style tool definition (internal `input_schema` form).
pub fn definition() -> Value {
    json!({
        "name": "create_skill",
        "description": "Create or update a custom skill — a packaged set of instructions with \
            trigger conditions, recommended tools, and few-shot examples. The skill is loaded \
            into your context on demand via `use_skill`; you then follow its instructions using \
            your normal tools. Gather requirements from the user through conversation, then \
            present the final draft to the user and obtain their explicit approval before calling \
            this tool. When updating an existing skill, provide the correct version number to \
            trigger automatic version archiving.",
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
                    "description": "Tool names this skill is expected to use (e.g. 'web_search', \
                        'fetch_webpage'), surfaced as recommendations when the skill is loaded. \
                        Leave empty for a text-only skill.",
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

pub(crate) fn parse_triggers(val: Option<&Value>) -> Result<Option<Vec<SkillTrigger>>, String> {
    match val {
        None => Ok(None),
        Some(v) => {
            let arr = v
                .as_array()
                .ok_or_else(|| "'triggers' must be an array".to_string())?;
            let triggers: Result<Vec<_>, String> = arr
                .iter()
                .map(|item| {
                    let trigger_type = item
                        .get("trigger_type")
                        .and_then(Value::as_str)
                        .ok_or_else(|| {
                            "Each trigger must have a string field 'trigger_type'".to_string()
                        })?
                        .to_string();
                    let value = item
                        .get("value")
                        .and_then(Value::as_str)
                        .ok_or_else(|| "Each trigger must have a string field 'value'".to_string())?
                        .to_string();
                    Result::<_, String>::Ok(SkillTrigger {
                        trigger_type,
                        value,
                    })
                })
                .collect();
            Ok(Some(triggers?))
        }
    }
}

pub(crate) fn parse_examples(val: Option<&Value>) -> Result<Option<Vec<SkillExample>>, String> {
    match val {
        None => Ok(None),
        Some(v) => {
            let arr = v
                .as_array()
                .ok_or_else(|| "'examples' must be an array".to_string())?;
            let examples: Result<Vec<_>, String> = arr
                .iter()
                .map(|item| {
                    let input = item
                        .get("input")
                        .and_then(Value::as_str)
                        .ok_or_else(|| "Each example must have a string field 'input'".to_string())?
                        .to_string();
                    let output = item
                        .get("output")
                        .and_then(Value::as_str)
                        .ok_or_else(|| {
                            "Each example must have a string field 'output'".to_string()
                        })?
                        .to_string();
                    Result::<_, String>::Ok(SkillExample { input, output })
                })
                .collect();
            Ok(Some(examples?))
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
#[path = "create_skill_tests.rs"]
mod tests;
