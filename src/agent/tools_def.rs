//! Native tool JSON definitions and small argument helpers.

use super::*;

pub(crate) fn use_skill_tool() -> Value {
    json!({
        "name": "use_skill",
        "description": "Load a named custom skill into your context — a packaged set of \
            instructions, recommended tools, and examples for handling a particular kind of \
            request. This returns the skill's full instructions; follow them yourself using your \
            normal tools. Call it when a skill listed in the session information looks relevant.",
        "input_schema": {
            "type": "object",
            "properties": {
                "name": {"type": "string", "description": "The skill name to load."}
            },
            "required": ["name"]
        }
    })
}

pub(crate) fn create_skill_tool() -> Value {
    tools::create_skill::definition()
}

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

pub(crate) fn get_discord_user_tool() -> Value {
    json!({
        "name": "get_discord_user",
        "description": "Fetch public profile information for a Discord user by their user ID. \
            Returns the username, display name, account creation date, and whether the account \
            is a bot.",
        "input_schema": {
            "type": "object",
            "properties": {
                "user_id": {
                    "type": "string",
                    "description": "The Discord user ID (snowflake) to look up."
                }
            },
            "required": ["user_id"]
        }
    })
}

pub(crate) fn find_discord_users_tool() -> Value {
    json!({
        "name": "find_discord_users",
        "description": "Fuzzy-find Discord users previously seen in the current channel by username, nickname, or user ID. Supports multi-word queries — each word is matched independently, so searching for \"rice farmer\" will match users whose username or nickname contains \"rice\" OR \"farmer\". The search is fully case-insensitive, ignores punctuation, and tolerates minor typos via Levenshtein distance (1 edit for 4-5 char words, 2 for 6-7, 3 for 8+). Use this before get_discord_user when a person is named but their numeric ID is unknown. Results are limited to the selected channel's message history.",
        "input_schema": {
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Case-insensitive fuzzy search — matches if any whitespace-separated word is a substring of username, nickname, or user ID. Punctuation is ignored and minor typos are tolerated."
                },
                "max_results": {
                    "type": "integer",
                    "description": "Maximum number of users to return (1–20, default 10)."
                },
                "channel_id": {
                    "type": "string",
                    "description": "Discord channel ID to search. Omit to use the current channel."
                }
            },
            "required": ["query"]
        }
    })
}

pub(crate) fn run_lua_tool() -> Value {
    json!({
        "name": "run_lua",
        "description": "Write and execute a sandboxed Lua 5.4 script for calculations, data \
            processing, algorithmic tasks, or generating directed-graph diagrams. `print(...)` \
            output and return values are captured and returned as the tool result. The `graph.*` \
            API (`graph.node`, `graph.edge`, `graph.title`) builds directed graphs that are \
            rendered as PNG images and automatically attached to the Discord response. \
            `discord.web_search` and `discord.jellyfin_search` are available as bridge functions. \
            Call `get_lua_docs` first if you need the full API reference for the sandbox.",
        "input_schema": {
            "type": "object",
            "properties": {
                "script": {
                    "type": "string",
                    "description": "Lua 5.4 source code to execute. May be wrapped in a ```lua … ``` fence."
                }
            },
            "required": ["script"]
        }
    })
}

pub(crate) fn get_lua_docs_tool() -> Value {
    json!({
        "name": "get_lua_docs",
        "description": "Return the full API reference for the bot's Lua scripting sandbox: \
            which standard libraries and built-in globals are available, the discord.* bridge API \
            (web_search, jellyfin_search), execution limits (timeout, memory, call caps), and \
            usage examples. Call this before writing a Lua script to understand the environment.",
        "input_schema": {
            "type": "object",
            "properties": {}
        }
    })
}

pub(crate) fn configure_bot_tool() -> Value {
    json!({
        "name": "configure_bot",
        "description": "View or change the bot's configuration. Only available to authorized \
            configurers (the bot owner plus users granted access). Actions: 'show' lists the \
            configurers and per-user policies; 'allow_configurer' / 'revoke_configurer' manage \
            who may configure the bot; 'set_user_limit' caps a user's maximum output tokens \
            (omit max_output_tokens to remove the cap); 'set_user_respond' controls whether the \
            bot responds to a user's messages at all.",
        "input_schema": {
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["show", "allow_configurer", "revoke_configurer", "set_user_limit", "set_user_respond"],
                    "description": "The configuration action to perform."
                },
                "user_id": {
                    "type": "string",
                    "description": "Discord user ID the action applies to (required for every action except 'show')."
                },
                "max_output_tokens": {
                    "type": "integer",
                    "description": "Maximum output tokens for set_user_limit. Omit to remove the cap."
                },
                "respond": {
                    "type": "boolean",
                    "description": "Whether the bot responds to the user, for set_user_respond."
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
