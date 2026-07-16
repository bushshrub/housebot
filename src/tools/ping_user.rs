//! Agent tool for pinging (@mentioning) a Discord user in the current channel.
//! The bot refuses to ping itself to prevent pointless self-notifications.

use serde_json::{json, Value};

use crate::discord_bridge::DiscordBridge;

/// OpenAI-style tool definition (internal `input_schema` form).
pub fn definition() -> Value {
    json!({
        "name": "ping_user",
        "description": "Send a message that mentions (@pings) a Discord user in the current \
            channel. Use this when you need to get a specific user's attention. The bot cannot \
            ping itself.",
        "input_schema": {
            "type": "object",
            "properties": {
                "user_id": {
                    "type": "string",
                    "description": "The Discord user ID (snowflake) of the user to ping."
                },
                "message": {
                    "type": "string",
                    "description": "Optional message to include alongside the ping."
                }
            },
            "required": ["user_id"]
        }
    })
}

/// Ping a Discord user by `target_user_id` in the given `channel_id`.
///
/// `bot_user_id` is the bot's own user ID, used to prevent self-pings. When the
/// bridge's HTTP client is unavailable, an appropriate error is returned.
pub async fn ping_user(
    discord: &DiscordBridge,
    channel_id: u64,
    target_user_id: &str,
    message: &str,
    bot_user_id: u64,
) -> String {
    let target_id: u64 = match target_user_id.parse() {
        Ok(id) => id,
        Err(_) => {
            return "Error: invalid user_id — must be a numeric Discord user ID.".to_string();
        }
    };

    if target_id == bot_user_id {
        return "Error: I cannot ping myself.".to_string();
    }

    let content = if message.is_empty() {
        format!("<@{}>", target_id)
    } else {
        format!("<@{}> — {}", target_id, message)
    };

    match discord
        .send_user_mention(channel_id, target_id, &content)
        .await
    {
        Ok(()) => format!("✅ Successfully pinged <@{}>.", target_id),
        Err(e) => format!("Error: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn noop_bridge() -> DiscordBridge {
        DiscordBridge::default()
    }

    #[test]
    fn definition_has_required_fields() {
        let d = definition();
        assert_eq!(d["name"], "ping_user");
        let props = &d["input_schema"]["properties"];
        assert!(props.get("user_id").is_some());
        assert!(props.get("message").is_some());
        assert_eq!(d["input_schema"]["required"], json!(["user_id"]));
    }

    #[tokio::test]
    async fn invalid_user_id_returns_error() {
        let result = ping_user(&noop_bridge(), 1, "not-a-number", "hello", 42).await;
        assert!(result.starts_with("Error:"));
        assert!(result.contains("invalid user_id"));
    }

    #[tokio::test]
    async fn self_ping_returns_error() {
        let result = ping_user(&noop_bridge(), 1, "42", "hello", 42).await;
        assert!(result.starts_with("Error:"));
        assert!(result.contains("cannot ping myself"));
    }

    #[tokio::test]
    async fn bridge_unavailable_returns_error() {
        let result = ping_user(&noop_bridge(), 1, "12345", "hello", 42).await;
        assert!(result.starts_with("Error:"));
        assert!(result.contains("Discord bridge not available"));
    }

    #[tokio::test]
    async fn empty_message_is_valid() {
        let result = ping_user(&noop_bridge(), 1, "12345", "", 42).await;
        // Bridge unavailable is expected, but no parsing or self-ping error
        assert!(result.starts_with("Error:"));
        assert!(result.contains("Discord bridge not available"));
    }
}
