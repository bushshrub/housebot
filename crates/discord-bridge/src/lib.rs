//! Shared bridge between the Discord bot and the agent, used to fetch
//! public user profiles on demand.

use std::sync::Arc;

use housebot_bot_response::SecretRedactor;
use serenity::all::{
    ChannelId, CreateAllowedMentions, CreateMessage, GetMessages, Message, MessageId, Timestamp,
    UserId,
};
use tokio::sync::RwLock;

pub struct UserInfo {
    pub id: String,
    pub username: String,
    pub display_name: String,
    pub bot: bool,
    pub created_at: String,
    pub avatar_url: Option<String>,
}

/// Where to anchor a `fetch_messages` call within a channel's history.
pub enum MessageAnchor {
    Latest,
    Before(u64),
    After(u64),
    Around(u64),
}

pub struct FetchedMessage {
    pub id: String,
    pub ts: String,
    pub author: String,
    pub content: String,
}

impl From<&Message> for FetchedMessage {
    fn from(msg: &Message) -> Self {
        let author = msg
            .member
            .as_ref()
            .and_then(|m| m.nick.as_deref())
            .or(msg.author.global_name.as_deref())
            .filter(|n| *n != msg.author.name)
            .unwrap_or(&msg.author.name);
        FetchedMessage {
            id: msg.id.get().to_string(),
            ts: msg.timestamp.to_string(),
            author: author.to_string(),
            content: msg.content.clone(),
        }
    }
}

/// Backward-paging safety cap for `fetch_messages_recent`: at most this many
/// pages of up to 100 messages are fetched before giving up on reaching the
/// time cutoff, to bound cost in a very active channel.
const RECENT_MAX_PAGES: u32 = 5;

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

    async fn fetch_page(
        &self,
        channel_id: u64,
        builder: GetMessages,
    ) -> Result<Vec<Message>, String> {
        let guard = self.http.read().await;
        let Some(http) = guard.as_ref() else {
            return Err("Discord bridge not available.".to_string());
        };
        ChannelId::new(channel_id)
            .messages(http.as_ref(), builder)
            .await
            .map_err(|e| format!("Failed to fetch messages in channel {channel_id}: {e}"))
    }

    /// Fetch messages positioned relative to `anchor` (or the most recent
    /// messages, for `MessageAnchor::Latest`), oldest first.
    pub async fn fetch_messages(
        &self,
        channel_id: u64,
        anchor: MessageAnchor,
        limit: u8,
    ) -> Result<Vec<FetchedMessage>, String> {
        let builder = match anchor {
            MessageAnchor::Latest => GetMessages::new(),
            MessageAnchor::Before(id) => GetMessages::new().before(MessageId::new(id)),
            MessageAnchor::After(id) => GetMessages::new().after(MessageId::new(id)),
            MessageAnchor::Around(id) => GetMessages::new().around(MessageId::new(id)),
        }
        .limit(limit);
        let mut messages = self.fetch_page(channel_id, builder).await?;
        messages.sort_by_key(|m| m.timestamp.unix_timestamp());
        Ok(messages.iter().map(FetchedMessage::from).collect())
    }

    /// Fetch messages posted in `channel_id` within the last `minutes`
    /// minutes, oldest first, by paging backward from the most recent
    /// message up to `RECENT_MAX_PAGES` pages of 100.
    pub async fn fetch_messages_recent(
        &self,
        channel_id: u64,
        minutes: u32,
    ) -> Result<Vec<FetchedMessage>, String> {
        let cutoff = Timestamp::now().unix_timestamp() - i64::from(minutes) * 60;
        let mut collected: Vec<Message> = Vec::new();
        let mut before: Option<MessageId> = None;
        for _ in 0..RECENT_MAX_PAGES {
            let mut builder = GetMessages::new().limit(100);
            if let Some(id) = before {
                builder = builder.before(id);
            }
            let page = self.fetch_page(channel_id, builder).await?;
            let Some(oldest) = page.iter().min_by_key(|m| m.timestamp.unix_timestamp()) else {
                break;
            };
            let reached_cutoff = oldest.timestamp.unix_timestamp() < cutoff;
            before = Some(oldest.id);
            collected.extend(
                page.into_iter()
                    .filter(|m| m.timestamp.unix_timestamp() >= cutoff),
            );
            if reached_cutoff {
                break;
            }
        }
        collected.sort_by_key(|m| m.timestamp.unix_timestamp());
        Ok(collected.iter().map(FetchedMessage::from).collect())
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

    fn message(author: serde_json::Value, member: serde_json::Value) -> Message {
        serde_json::from_value(serde_json::json!({
            "id": "42",
            "channel_id": "1",
            "author": author,
            "member": member,
            "content": "hello",
            "timestamp": "2026-01-01T00:00:00+00:00",
            "tts": false,
            "mention_everyone": false,
            "mentions": [],
            "mention_roles": [],
            "attachments": [],
            "embeds": [],
            "pinned": false,
            "type": 0
        }))
        .unwrap()
    }

    fn author(username: &str, global_name: Option<&str>) -> serde_json::Value {
        serde_json::json!({
            "id": "1",
            "username": username,
            "discriminator": "0000",
            "avatar": null,
            "global_name": global_name,
        })
    }

    #[test]
    fn fetched_message_prefers_server_nick() {
        let msg = message(
            author("realname", Some("global")),
            serde_json::json!({"nick": "Nicky", "roles": [], "joined_at": null}),
        );
        assert_eq!(FetchedMessage::from(&msg).author, "Nicky");
    }

    #[test]
    fn fetched_message_falls_back_to_global_name() {
        let msg = message(
            author("realname", Some("Global Name")),
            serde_json::json!({"nick": null, "roles": [], "joined_at": null}),
        );
        assert_eq!(FetchedMessage::from(&msg).author, "Global Name");
    }

    #[test]
    fn fetched_message_falls_back_to_username_when_global_name_matches() {
        let msg = message(
            author("realname", Some("realname")),
            serde_json::json!({"nick": null, "roles": [], "joined_at": null}),
        );
        assert_eq!(FetchedMessage::from(&msg).author, "realname");
    }

    #[test]
    fn fetched_message_falls_back_to_username_with_no_member_or_global_name() {
        let msg = message(author("realname", None), serde_json::Value::Null);
        assert_eq!(FetchedMessage::from(&msg).author, "realname");
    }

    #[test]
    fn fetched_message_carries_id_and_content() {
        let msg = message(author("realname", None), serde_json::Value::Null);
        let fetched = FetchedMessage::from(&msg);
        assert_eq!(fetched.id, "42");
        assert_eq!(fetched.content, "hello");
    }
}
