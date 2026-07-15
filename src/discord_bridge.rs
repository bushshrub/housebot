//! Shared bridge between the Discord bot and the agent, used to fetch
//! public user profiles on demand.

use std::sync::Arc;

use serenity::all::{ChannelId, UserId};
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
#[derive(Clone, Default)]
pub struct DiscordBridge {
    http: Arc<RwLock<Option<Arc<serenity::http::Http>>>>,
}

impl DiscordBridge {
    pub async fn set_http(&self, http: Arc<serenity::http::Http>) {
        *self.http.write().await = Some(http);
    }

    pub async fn send_message(&self, channel_id: u64, content: &str) -> Result<(), String> {
        let guard = self.http.read().await;
        let Some(http) = guard.as_ref() else {
            return Err("Discord bridge not available.".to_string());
        };
        ChannelId::new(channel_id)
            .say(http.as_ref(), content)
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
