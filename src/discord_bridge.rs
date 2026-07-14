//! Shared bridge between the Discord bot and the agent, used to fetch channel messages
//! and user profiles on demand.

use std::sync::Arc;

use serenity::all::{GetMessages, UserId};
use serenity::model::id::ChannelId;
use tokio::sync::RwLock;

pub struct ChatMessage {
    pub author_id: String,
    pub author_name: String,
    pub content: String,
    pub timestamp: String,
    pub is_bot: bool,
}

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
#[derive(Clone, Default)]
pub struct DiscordBridge {
    http: Arc<RwLock<Option<Arc<serenity::http::Http>>>>,
}

impl DiscordBridge {
    pub async fn set_http(&self, http: Arc<serenity::http::Http>) {
        *self.http.write().await = Some(http);
    }

    pub async fn fetch_messages(
        &self,
        channel_id: u64,
        count: u8,
    ) -> Result<Vec<ChatMessage>, String> {
        let guard = self.http.read().await;
        let Some(http) = guard.as_ref() else {
            return Err("Discord bridge not available.".to_string());
        };
        let limit = count.clamp(1, 50);
        let msgs = ChannelId::new(channel_id)
            .messages(http.as_ref(), GetMessages::new().limit(limit))
            .await
            .map_err(|e| format!("Failed to fetch messages: {e}"))?;
        let result = msgs
            .into_iter()
            .rev()
            .map(|m| ChatMessage {
                author_id: m.author.id.get().to_string(),
                author_name: m.author.name.clone(),
                content: m.content.clone(),
                timestamp: m.timestamp.to_string(),
                is_bot: m.author.bot,
            })
            .collect();
        Ok(result)
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
