//! Native tool JSON definitions and small argument helpers.

use super::*;

/// Wrap a tool in the OpenAI function-calling envelope.
pub fn to_openai_tool(name: &str, description: &str, parameters: Value) -> Value {
    json!({
        "type": "function",
        "function": {"name": name, "description": description, "parameters": parameters},
    })
}

/// Convert an internal tool definition into `(name, description, parameters)`.
pub fn flatten_tool(tool_def: &Value) -> (String, String, Value) {
    let name = tool_def
        .get("name")
        .and_then(|n| n.as_str())
        .unwrap_or("")
        .to_string();
    let description = tool_def
        .get("description")
        .and_then(|d| d.as_str())
        .unwrap_or("")
        .to_string();
    let parameters = tool_def
        .get("input_schema")
        .or_else(|| tool_def.get("parameters"))
        .cloned()
        .unwrap_or_else(|| json!({}));
    (name, description, parameters)
}

/// Extract a string argument from tool-call args, defaulting to empty.
pub(crate) fn str_arg<'a>(args: &'a Value, key: &str) -> &'a str {
    args.get(key).and_then(Value::as_str).unwrap_or("")
}

/// Extract an unsigned integer argument from tool-call args.
pub(crate) fn u64_arg(args: &Value, key: &str, default: u64) -> u64 {
    args.get(key).and_then(Value::as_u64).unwrap_or(default)
}

pub(crate) fn get_messages_tool() -> Value {
    json!({
        "name": "get_messages",
        "description": "Flexibly retrieve Discord channel messages. Modes: 'recent' (default) \
            returns everything posted in the last N minutes, in chronological order — use it for \
            recaps or vague/open-ended questions like 'what happened recently' or 'what did I \
            miss'. 'before' / 'after' / 'around' return messages positioned relative to a specific \
            message_id — use these when the user replies to a message in Discord and you need the \
            conversation near it (the replied-to message's ID is included in the \
            '[Message being replied to, id: ...]' context). 'search' finds messages by regex \
            pattern matched against message content, author username, AND author nickname/display \
            name — use it only when searching for a specific keyword, topic, or person (e.g. \
            '(?i)hexagone' to find messages by or mentioning 'hexagone'); supports full Rust regex \
            syntax, case-insensitive patterns ((?i)) are common.",
        "input_schema": {
            "type": "object",
            "properties": {
                "mode": {
                    "type": "string",
                    "enum": ["recent", "before", "after", "around", "search"],
                    "description": "Retrieval mode. Defaults to 'recent'."
                },
                "channel_id": {
                    "type": "string",
                    "description": "Discord channel ID. Omit to use the current channel."
                },
                "minutes": {
                    "type": "integer",
                    "description": "For mode=recent: how far back to look, in minutes (1–1440, default 30)."
                },
                "message_id": {
                    "type": "string",
                    "description": "For mode=before/after/around: the anchor Discord message ID, e.g. the ID of the message being replied to."
                },
                "limit": {
                    "type": "integer",
                    "description": "For mode=before/after/around/search: maximum number of messages to return (1–100, default 20)."
                },
                "pattern": {
                    "type": "string",
                    "description": "For mode=search: regex pattern matched against message content, author username, and author nickname/display name."
                }
            },
            "required": []
        }
    })
}

pub(crate) fn get_current_time_tool() -> Value {
    json!({
        "name": "get_current_time",
        "description": "Get the current date, time, and weekday. Call this whenever an answer \
            depends on the current date or time; you do not otherwise know it.",
        "input_schema": {
            "type": "object",
            "properties": {
                "timezone": {
                    "type": "string",
                    "description": "IANA time zone name, e.g. 'America/New_York' or 'Europe/Paris'. Defaults to UTC."
                }
            },
            "required": []
        }
    })
}

/// Current time in `timezone` (an IANA name, empty for UTC). The zone data is
/// compiled in, so this does not depend on tzdata files in any container.
pub(crate) fn current_time_text(timezone: &str, now: DateTime<Utc>) -> String {
    let zone = if timezone.trim().is_empty() {
        chrono_tz::UTC
    } else {
        match timezone.trim().parse::<chrono_tz::Tz>() {
            Ok(zone) => zone,
            Err(_) => {
                return format!(
                    "Error: unknown time zone '{timezone}'. Use an IANA name such as 'America/New_York'."
                )
            }
        }
    };
    let local = now.with_timezone(&zone);
    format!(
        "{} ({}, UTC{})",
        local.format("%A %Y-%m-%d %H:%M:%S"),
        zone.name(),
        local.format("%:z")
    )
}

pub(crate) fn configure_bot_tool() -> Value {
    json!({
        "name": "configure_bot",
        "description": "View or change the bot's configuration. Only available to authorized \
            configurers (the bot owner plus users granted access).\n\n\
            Actions:\n\
            - 'show' — list configurers and per-user policies.\n\
            - 'allow_configurer' — grant a user permission to configure the bot. \
              Requires user_id.\n\
            - 'revoke_configurer' — remove a user's configure permission. \
              Requires user_id.\n\
            - 'set_user_limit' — cap a user's maximum output tokens. \
              Requires user_id. Omit max_output_tokens to remove the cap.\n\
            - 'set_user_respond' — control whether the bot responds to a user. \
              Requires user_id and respond (boolean).\n\
            - 'set_user_limit_all' — cap max_output_tokens for every user who already \
              has a policy. Omit max_output_tokens to remove all caps.\n\
            - 'set_user_respond_all' — set the respond flag for every user who already \
              has a policy. Requires respond (boolean).",
        "input_schema": {
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": [
                        "show",
                        "allow_configurer",
                        "revoke_configurer",
                        "set_user_limit",
                        "set_user_respond",
                        "set_user_limit_all",
                        "set_user_respond_all"
                    ],
                    "description": "The configuration action to perform."
                },
                "user_id": {
                    "type": "string",
                    "description": "Discord user ID the action applies to (required for allow_configurer, revoke_configurer, set_user_limit, set_user_respond)."
                },
                "max_output_tokens": {
                    "type": "integer",
                    "description": "Maximum output tokens for set_user_limit / set_user_limit_all. Omit to remove the cap."
                },
                "respond": {
                    "type": "boolean",
                    "description": "Whether the bot responds to the user, for set_user_respond / set_user_respond_all."
                }
            },
            "required": ["action"]
        }
    })
}

pub(crate) fn search_rate_limited(content: &str) -> bool {
    let content = content.to_ascii_lowercase();
    content.contains("returned http 429")
        || content.contains("too many requests")
        || content.contains("rate limit")
        || content.contains("temporarily blocked")
}
