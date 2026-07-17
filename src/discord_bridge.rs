//! Shared bridge between the Discord bot and the agent, used to fetch
//! public user profiles on demand and send pings.

use std::sync::Arc;

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
    bot_user_id: Arc<RwLock<u64>>,
}

impl Default for DiscordBridge {
    fn default() -> Self {
        Self {
            http: Arc::new(RwLock::new(None)),
            bot_user_id: Arc::new(RwLock::new(0)),
        }
    }
}

impl DiscordBridge {
    pub async fn set_http(&self, http: Arc<serenity::http::Http>) {
        *self.http.write().await = Some(http);
    }

    pub async fn set_bot_user_id(&self, id: u64) {
        *self.bot_user_id.write().await = id;
    }

    pub async fn bot_user_id(&self) -> u64 {
        *self.bot_user_id.read().await
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
            .content(content)
            .allowed_mentions(CreateAllowedMentions::new());
        ChannelId::new(channel_id)
            .send_message(http.as_ref(), builder)
            .await
            .map(|_| ())
            .map_err(|e| format!("Failed to send message: {e}"))
    }

    /// Send a message that pings a specific user. The message content is
    /// prepended with `<@{target_user_id}>` to trigger the Discord mention.
    pub async fn send_user_ping(
        &self,
        channel_id: u64,
        target_user_id: u64,
        message: Option<&str>,
    ) -> Result<(), String> {
        let guard = self.http.read().await;
        let Some(http) = guard.as_ref() else {
            return Err("Discord bridge not available.".to_string());
        };
        let content = match message {
            Some(msg) if !msg.is_empty() => {
                format!("<@{}> {}", target_user_id, msg)
            }
            _ => format!("<@{}>", target_user_id),
        };
        let builder = CreateMessage::new()
            .content(content)
            .allowed_mentions(
                CreateAllowedMentions::new()
                    .replied_user(false)
                    .users(vec![UserId::new(target_user_id)]),
            );
        ChannelId::new(channel_id)
            .send_message(http.as_ref(), builder)
            .await
            .map(|_| ())
            .map_err(|e| format!("Failed to send ping: {e}"))
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
