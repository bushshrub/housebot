//! Shared bridge between the Discord bot and the agent, used to fetch
//! public user profiles on demand.

use std::sync::Arc;

use housebot_bot_response::SecretRedactor;
use serenity::all::{ChannelId, CreateAllowedMentions, CreateMessage, UserId};
use tokio::sync::RwLock;

pub struct UserInfo {
    pub id: String,
    pub username: String,
    pub display_name: String,
    pub bot: bool,
    pub created_at: String,
    pub avatar_url: Option<String>,
}

/// Holds the Discord HTTP client so the agent can query Discord APIs.
///
/// The HTTP handle is injected after the bot connects (see `set_http`), so
/// tool calls that arrive before `ready` fires return an error rather than
/// panicking.
#[derive(Clone)]
pub struct DiscordBridge {
    http: Arc<RwLock<Option<Arc<serenity::http::Http>>>>,
    redactor: Arc<SecretRedactor>,
}

impl Default for DiscordBridge {
    fn default() -> Self {
        Self::with_redactor(SecretRedactor::from_env())
    }
}

impl DiscordBridge {
    pub fn with_redactor(redactor: SecretRedactor) -> Self {
        Self {
            http: Arc::new(RwLock::new(None)),
            redactor: Arc::new(redactor),
        }
    }

    pub async fn set_http(&self, http: Arc<serenity::http::Http>) {
        *self.http.write().await = Some(http);
    }

    /// Content as it will leave the bridge: known secret values scrubbed.
    fn outbound_content(&self, content: &str) -> String {
        self.redactor.redact(content)
    }

    /// Send a message on behalf of a Lua script. Mentions are suppressed so a
    /// Scripting-role member without Discord's own mention permissions cannot
    /// use the bridge to ping `@everyone`, roles, or arbitrary users.
    pub async fn send_message(&self, channel_id: u64, content: &str) -> Result<(), String> {
        let guard = self.http.read().await;
        let Some(http) = guard.as_ref() else {
            return Err("Discord bridge not available.".to_string());
        };
        let builder = CreateMessage::new()
            .content(self.outbound_content(content))
            .allowed_mentions(CreateAllowedMentions::new());
        ChannelId::new(channel_id)
            .send_message(http.as_ref(), builder)
            .await
            .map(|_| ())
            .map_err(|e| format!("Failed to send message: {e}"))
    }

    pub async fn fetch_user(&self, user_id: u64) -> Result<UserInfo, String> {
        let guard = self.http.read().await;
        let Some(http) = guard.as_ref() else {
            return Err("Discord bridge not available.".to_string());
        };
        let user = UserId::new(user_id)
            .to_user(http.as_ref())
            .await
            .map_err(|e| format!("Failed to fetch user {user_id}: {e}"))?;
        Ok(UserInfo {
            id: user.id.get().to_string(),
            username: user.name.clone(),
            display_name: user.display_name().to_string(),
            bot: user.bot,
            created_at: user.created_at().to_string(),
            avatar_url: user.avatar_url(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outbound_content_is_redacted() {
        let redactor = SecretRedactor::from_vars([(
            "DISCORD_TOKEN".to_string(),
            "super-secret-token".to_string(),
        )]);
        let bridge = DiscordBridge::with_redactor(redactor);
        let out = bridge.outbound_content("leak: super-secret-token!");
        assert_eq!(out, "leak: [REDACTED]!");
    }
}
