//! Agent tool for pinging/mentioning Discord users.

use serde_json::{json, Value};

use crate::discord_bridge::DiscordBridge;

/// OpenAI-style tool definition (internal `input_schema` form).
pub fn definition() -> Value {
    json!({
        "name": "ping_user",
        "description": "Ping/mention a Discord user in the current channel by their user ID. \
            Use find_discord_users or get_discord_user first to look up a user's ID if needed. \
            The bot cannot ping itself.",
        "input_schema": {
            "type": "object",
            "properties": {
                "user_id": {
                    "type": "string",
                    "description": "The Discord user ID (snowflake) of the user to ping."
                },
                "message": {
                    "type": "string",
                    "description": "Optional message to include with the ping."
                }
            },
            "required": ["user_id"]
        }
    })
}

/// Send a ping to a user in the given channel.
pub async fn send_ping(
    discord: &DiscordBridge,
    channel_id: u64,
    user_id: &str,
    message: &str,
) -> String {
    let target_id: u64 = match user_id.parse() {
        Ok(id) => id,
        Err(_) => return "Error: invalid user_id — must be a numeric Discord ID.".to_string(),
    };

    let bot_id = discord.bot_user_id().await;
    if target_id == bot_id {
        return "Error: I cannot ping myself.".to_string();
    }

    let msg = if message.is_empty() {
        None
    } else {
        Some(message)
    };

    match discord.send_user_ping(channel_id, target_id, msg).await {
        Ok(()) => {
            if let Some(m) = msg {
                format!("✅ Pinged <@{target_id}> with message: {m}")
            } else {
                format!("✅ Pinged <@{target_id}>")
            }
        }
        Err(e) => format!("Error: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn definition_has_required_fields() {
        let d = definition();
        assert_eq!(d["name"], "ping_user");
        let props = &d["input_schema"]["properties"];
        assert!(props.get("user_id").is_some());
        assert!(props.get("message").is_some());
        assert_eq!(d["input_schema"]["required"], json!(["user_id"]));
    }

    #[test]
    fn definition_user_id_is_string() {
        let d = definition();
        assert_eq!(
            d["input_schema"]["properties"]["user_id"]["type"],
            "string"
        );
    }

    #[test]
    fn definition_message_is_optional() {
        let d = definition();
        assert_eq!(
            d["input_schema"]["properties"]["message"]["type"],
            "string"
        );
    }
}
