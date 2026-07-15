//! Persistent conversation archive and global token-usage leaderboard.

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::Value;
use tokio::sync::Mutex;
use tokio_postgres::NoTls;

use crate::config;
use crate::llm::TokenUsage;

const DEFAULT_DATABASE_URL: &str = "postgres://housebot:housebot@postgres/housebot";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeaderboardEntry {
    pub label: String,
    pub conversation_id: Option<String>,
    pub conversations: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cached_tokens: u64,
}

impl LeaderboardEntry {
    pub fn total_tokens(&self) -> u64 {
        self.input_tokens.saturating_add(self.output_tokens)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TokenLeaderboard {
    pub users: Vec<LeaderboardEntry>,
    pub conversations: Vec<LeaderboardEntry>,
}

#[derive(Clone)]
enum Backend {
    Memory(Arc<Mutex<MemoryData>>),
    Postgres(Arc<tokio_postgres::Client>),
}

#[derive(Default)]
struct MemoryData {
    conversations: HashMap<String, MemoryConversation>,
    messages: Vec<MemoryMessage>,
}

#[derive(Default)]
struct MemoryConversation {
    user_id: String,
    display_name: String,
    input_tokens: u64,
    output_tokens: u64,
    cached_tokens: u64,
    requests: u64,
    ended: bool,
}

struct MemoryMessage {
    conversation_id: String,
    #[allow(dead_code)]
    role: String,
    #[allow(dead_code)]
    content: String,
}

/// Shared handle for conversation and token persistence.
#[derive(Clone)]
pub struct TokenMonitor {
    backend: Backend,
}

impl Default for TokenMonitor {
    fn default() -> Self {
        Self {
            backend: Backend::Memory(Arc::new(Mutex::new(MemoryData::default()))),
        }
    }
}

impl TokenMonitor {
    /// Connect to PostgreSQL and create the conversation archive schema.
    pub async fn from_env() -> anyhow::Result<Self> {
        let url = config::env_or("DATABASE_URL", DEFAULT_DATABASE_URL);
        let (client, connection) = tokio_postgres::connect(&url, NoTls).await?;
        tokio::spawn(async move {
            if let Err(error) = connection.await {
                tracing::error!(%error, "PostgreSQL token-monitor connection closed");
            }
        });
        client
            .batch_execute(
                "CREATE TABLE IF NOT EXISTS conversations (\
                    conversation_id TEXT PRIMARY KEY,\
                    user_id TEXT NOT NULL,\
                    display_name TEXT NOT NULL,\
                    channel_id TEXT NOT NULL,\
                    input_tokens BIGINT NOT NULL DEFAULT 0,\
                    output_tokens BIGINT NOT NULL DEFAULT 0,\
                    cached_tokens BIGINT NOT NULL DEFAULT 0,\
                    request_count BIGINT NOT NULL DEFAULT 0,\
                    started_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),\
                    ended_at TIMESTAMPTZ\
                );\
                CREATE INDEX IF NOT EXISTS conversations_user_id_idx\
                    ON conversations (user_id);\
                CREATE INDEX IF NOT EXISTS conversations_tokens_idx\
                    ON conversations ((input_tokens + output_tokens) DESC);\
                CREATE TABLE IF NOT EXISTS conversation_messages (\
                    id BIGSERIAL PRIMARY KEY,\
                    conversation_id TEXT NOT NULL REFERENCES conversations(conversation_id) ON DELETE CASCADE,\
                    turn_id TEXT NOT NULL,\
                    message_index INTEGER NOT NULL,\
                    role TEXT NOT NULL,\
                    content TEXT NOT NULL,\
                    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()\
                );\
                CREATE INDEX IF NOT EXISTS conversation_messages_conversation_idx\
                    ON conversation_messages (conversation_id, id);",
            )
            .await?;
        Ok(Self {
            backend: Backend::Postgres(Arc::new(client)),
        })
    }

    pub async fn start_conversation(
        &self,
        conversation_id: &str,
        user_id: &str,
        display_name: &str,
        channel_id: u64,
    ) -> anyhow::Result<()> {
        match &self.backend {
            Backend::Memory(data) => {
                data.lock()
                    .await
                    .conversations
                    .entry(conversation_id.into())
                    .or_insert_with(|| MemoryConversation {
                        user_id: user_id.into(),
                        display_name: display_name.into(),
                        ..Default::default()
                    });
            }
            Backend::Postgres(client) => {
                client
                    .execute(
                        "INSERT INTO conversations (conversation_id, user_id, display_name, channel_id) \
                         VALUES ($1, $2, $3, $4) ON CONFLICT (conversation_id) DO NOTHING",
                        &[&conversation_id, &user_id, &display_name, &channel_id.to_string()],
                    )
                    .await?;
            }
        }
        Ok(())
    }

    pub async fn record_usage(
        &self,
        conversation_id: &str,
        usage: TokenUsage,
    ) -> anyhow::Result<()> {
        match &self.backend {
            Backend::Memory(data) => {
                if let Some(conversation) = data.lock().await.conversations.get_mut(conversation_id)
                {
                    conversation.input_tokens = conversation
                        .input_tokens
                        .saturating_add(usage.prompt_tokens);
                    conversation.output_tokens = conversation
                        .output_tokens
                        .saturating_add(usage.completion_tokens);
                    conversation.cached_tokens = conversation
                        .cached_tokens
                        .saturating_add(usage.prompt_tokens_details.cached_tokens);
                    conversation.requests = conversation.requests.saturating_add(1);
                }
            }
            Backend::Postgres(client) => {
                client
                    .execute(
                        "UPDATE conversations SET \
                            input_tokens = input_tokens + $2, \
                            output_tokens = output_tokens + $3, \
                            cached_tokens = cached_tokens + $4, \
                            request_count = request_count + 1 \
                         WHERE conversation_id = $1",
                        &[
                            &conversation_id,
                            &to_i64(usage.prompt_tokens),
                            &to_i64(usage.completion_tokens),
                            &to_i64(usage.prompt_tokens_details.cached_tokens),
                        ],
                    )
                    .await?;
            }
        }
        Ok(())
    }

    pub async fn record_turn(
        &self,
        conversation_id: &str,
        user_message: &Value,
        assistant_messages: &[Value],
    ) -> anyhow::Result<()> {
        let turn_id = uuid::Uuid::new_v4().to_string();
        let messages = std::iter::once(user_message).chain(assistant_messages.iter());
        match &self.backend {
            Backend::Memory(data) => {
                let mut data = data.lock().await;
                for message in messages {
                    data.messages.push(MemoryMessage {
                        conversation_id: conversation_id.into(),
                        role: message_role(message).into(),
                        content: serde_json::to_string(message)?,
                    });
                }
            }
            Backend::Postgres(client) => {
                for (index, message) in messages.enumerate() {
                    client
                        .execute(
                            "INSERT INTO conversation_messages \
                             (conversation_id, turn_id, message_index, role, content) \
                             VALUES ($1, $2, $3, $4, $5)",
                            &[
                                &conversation_id,
                                &turn_id,
                                &(index as i32),
                                &message_role(message),
                                &serde_json::to_string(message)?,
                            ],
                        )
                        .await?;
                }
            }
        }
        Ok(())
    }

    pub async fn finish_conversation(&self, conversation_id: &str) -> anyhow::Result<()> {
        match &self.backend {
            Backend::Memory(data) => {
                if let Some(conversation) = data.lock().await.conversations.get_mut(conversation_id)
                {
                    conversation.ended = true;
                }
            }
            Backend::Postgres(client) => {
                client
                    .execute(
                        "UPDATE conversations SET ended_at = COALESCE(ended_at, NOW()) \
                         WHERE conversation_id = $1",
                        &[&conversation_id],
                    )
                    .await?;
            }
        }
        Ok(())
    }

    pub async fn clear_user(&self, user_id: &str) -> anyhow::Result<()> {
        match &self.backend {
            Backend::Memory(data) => {
                let mut data = data.lock().await;
                data.conversations
                    .retain(|_, conversation| conversation.user_id != user_id);
                let retained: std::collections::HashSet<_> =
                    data.conversations.keys().cloned().collect();
                data.messages
                    .retain(|message| retained.contains(&message.conversation_id));
            }
            Backend::Postgres(client) => {
                client
                    .execute("DELETE FROM conversations WHERE user_id = $1", &[&user_id])
                    .await?;
            }
        }
        Ok(())
    }

    pub async fn leaderboard(&self, limit: usize) -> anyhow::Result<TokenLeaderboard> {
        let limit = limit.clamp(1, 25);
        match &self.backend {
            Backend::Memory(data) => {
                let data = data.lock().await;
                Ok(memory_leaderboard(&data, limit))
            }
            Backend::Postgres(client) => {
                let users = client
                    .query(
                        "SELECT COALESCE(NULLIF(MAX(display_name), ''), user_id), COUNT(*), \
                                SUM(input_tokens), SUM(output_tokens), SUM(cached_tokens) \
                         FROM conversations GROUP BY user_id \
                         ORDER BY SUM(input_tokens + output_tokens) DESC LIMIT $1",
                        &[&(limit as i64)],
                    )
                    .await?
                    .into_iter()
                    .map(|row| LeaderboardEntry {
                        label: row.get(0),
                        conversation_id: None,
                        conversations: from_i64(row.get(1)),
                        input_tokens: from_i64(row.get(2)),
                        output_tokens: from_i64(row.get(3)),
                        cached_tokens: from_i64(row.get(4)),
                    })
                    .collect();
                let conversations = client
                    .query(
                        "SELECT display_name, conversation_id, input_tokens, output_tokens, cached_tokens \
                         FROM conversations ORDER BY input_tokens + output_tokens DESC LIMIT $1",
                        &[&(limit as i64)],
                    )
                    .await?
                    .into_iter()
                    .map(|row| LeaderboardEntry {
                        label: row.get(0),
                        conversation_id: Some(row.get(1)),
                        conversations: 1,
                        input_tokens: from_i64(row.get(2)),
                        output_tokens: from_i64(row.get(3)),
                        cached_tokens: from_i64(row.get(4)),
                    })
                    .collect();
                Ok(TokenLeaderboard {
                    users,
                    conversations,
                })
            }
        }
    }
}

fn memory_leaderboard(data: &MemoryData, limit: usize) -> TokenLeaderboard {
    let mut by_user: HashMap<&str, LeaderboardEntry> = HashMap::new();
    let mut conversations = Vec::new();
    for (id, conversation) in &data.conversations {
        let user = by_user
            .entry(&conversation.user_id)
            .or_insert_with(|| LeaderboardEntry {
                label: conversation.display_name.clone(),
                conversation_id: None,
                conversations: 0,
                input_tokens: 0,
                output_tokens: 0,
                cached_tokens: 0,
            });
        user.conversations += 1;
        user.input_tokens = user.input_tokens.saturating_add(conversation.input_tokens);
        user.output_tokens = user
            .output_tokens
            .saturating_add(conversation.output_tokens);
        user.cached_tokens = user
            .cached_tokens
            .saturating_add(conversation.cached_tokens);
        conversations.push(LeaderboardEntry {
            label: conversation.display_name.clone(),
            conversation_id: Some(id.clone()),
            conversations: 1,
            input_tokens: conversation.input_tokens,
            output_tokens: conversation.output_tokens,
            cached_tokens: conversation.cached_tokens,
        });
    }
    let mut users: Vec<_> = by_user.into_values().collect();
    users.sort_by_key(|entry| std::cmp::Reverse(entry.total_tokens()));
    conversations.sort_by_key(|entry| std::cmp::Reverse(entry.total_tokens()));
    users.truncate(limit);
    conversations.truncate(limit);
    TokenLeaderboard {
        users,
        conversations,
    }
}

fn message_role(message: &Value) -> &str {
    message
        .get("role")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
}

fn to_i64(value: u64) -> i64 {
    value.min(i64::MAX as u64) as i64
}

fn from_i64(value: i64) -> u64 {
    value.max(0) as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::PromptTokenDetails;
    use serde_json::json;

    fn usage(input: u64, output: u64, cached: u64) -> TokenUsage {
        TokenUsage {
            prompt_tokens: input,
            completion_tokens: output,
            prompt_tokens_details: PromptTokenDetails {
                cached_tokens: cached,
            },
        }
    }

    #[tokio::test]
    async fn aggregates_users_and_conversations() {
        let monitor = TokenMonitor::default();
        monitor
            .start_conversation("c1", "u1", "Alice", 10)
            .await
            .unwrap();
        monitor
            .record_usage("c1", usage(100, 20, 10))
            .await
            .unwrap();
        monitor
            .start_conversation("c2", "u1", "Alice", 20)
            .await
            .unwrap();
        monitor.record_usage("c2", usage(50, 5, 0)).await.unwrap();
        monitor
            .start_conversation("c3", "u2", "Bob", 10)
            .await
            .unwrap();
        monitor.record_usage("c3", usage(10, 5, 0)).await.unwrap();

        let board = monitor.leaderboard(10).await.unwrap();
        assert_eq!(board.users[0].label, "Alice");
        assert_eq!(board.users[0].conversations, 2);
        assert_eq!(board.users[0].total_tokens(), 175);
        assert_eq!(
            board.conversations[0].conversation_id.as_deref(),
            Some("c1")
        );
    }

    #[tokio::test]
    async fn archives_every_message_and_erases_user_data() {
        let monitor = TokenMonitor::default();
        monitor
            .start_conversation("c1", "u1", "Alice", 10)
            .await
            .unwrap();
        monitor
            .record_turn(
                "c1",
                &json!({"role":"user","content":"hello"}),
                &[json!({"role":"assistant","content":"hi"})],
            )
            .await
            .unwrap();
        if let Backend::Memory(data) = &monitor.backend {
            assert_eq!(data.lock().await.messages.len(), 2);
        }
        monitor.clear_user("u1").await.unwrap();
        assert!(monitor.leaderboard(10).await.unwrap().users.is_empty());
    }
}
