use serde_json::{json, Value};

pub fn definition() -> Value {
    json!({
        "name": "ping_users",
        "description": "Mention (ping) one or more Discord users in your response. You MUST call this tool BEFORE including @mentions in your text so the system knows which users to notify. You cannot ping the bot itself.",
        "input_schema": {
            "type": "object",
            "properties": {
                "user_ids": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Discord user IDs to ping (strings, not <@ mentions)."
                },
                "reason": {
                    "type": "string",
                    "description": "Optional human-readable reason shown to explain why these users are being pinged."
                }
            },
            "required": ["user_ids"]
        }
    })
}

pub fn execute(user_ids: &[String], bot_id: u64) -> Result<Vec<u64>, String> {
    if user_ids.is_empty() {
        return Err("Error: at least one user_id is required.".to_string());
    }
    let mut seen = std::collections::HashSet::new();
    let mut valid = Vec::new();
    for raw in user_ids {
        let id: u64 = raw.parse().map_err(|_| {
            format!("Error: invalid user_id '{raw}' — must be a numeric Discord ID.")
        })?;
        if id == bot_id {
            return Err("Error: you cannot ping the bot itself.".to_string());
        }
        if seen.insert(id) {
            valid.push(id);
        }
    }
    Ok(valid)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn definition_has_required_fields() {
        let d = definition();
        assert_eq!(d["name"], "ping_users");
        let props = &d["input_schema"]["properties"];
        assert!(props.get("user_ids").is_some());
        assert!(props.get("reason").is_some());
        assert_eq!(d["input_schema"]["required"], json!(["user_ids"]));
    }

    #[test]
    fn execute_returns_valid_ids() {
        let ids = vec!["123".into(), "456".into()];
        let result = execute(&ids, 999).unwrap();
        assert_eq!(result, vec![123, 456]);
    }

    #[test]
    fn execute_rejects_pinging_bot_itself() {
        let ids = vec!["42".into()];
        let err = execute(&ids, 42).unwrap_err();
        assert!(err.contains("cannot ping the bot itself"));
    }

    #[test]
    fn execute_rejects_empty_list() {
        let err = execute(&[], 999).unwrap_err();
        assert!(err.contains("at least one user_id"));
    }

    #[test]
    fn execute_rejects_non_numeric_id() {
        let ids = vec!["not-a-number".into()];
        let err = execute(&ids, 999).unwrap_err();
        assert!(err.contains("invalid user_id"));
    }

    #[test]
    fn execute_deduplicates_ids() {
        let ids = vec!["123".into(), "123".into(), "456".into()];
        let result = execute(&ids, 999).unwrap();
        assert_eq!(result, vec![123, 456]);
    }

    #[test]
    fn execute_accepts_single_id() {
        let ids = vec!["789".into()];
        let result = execute(&ids, 999).unwrap();
        assert_eq!(result, vec![789]);
    }

    #[test]
    fn execute_zero_bot_id_allows_all_users() {
        let ids = vec!["1".into(), "2".into()];
        let result = execute(&ids, 0).unwrap();
        assert_eq!(result, vec![1, 2]);
    }
}
