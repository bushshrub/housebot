//! Persistent conversation archive and global token-usage leaderboard.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::Value;
use tokio::sync::Mutex;
use tokio_postgres::NoTls;

use crate::config;
use crate::llm::TokenUsage;

const DEFAULT_DATABASE_URL: &str = "postgres://housebot:housebot@postgres/housebot";
const DEFAULT_CONNECT_ATTEMPTS: usize = 10;
const DEFAULT_CONNECT_RETRY_SECS: u64 = 2;
const DEFAULT_CONNECT_TIMEOUT_SECS: u64 = 10;

mod leaderboard;
use leaderboard::{finish_leaderboard, from_i64, memory_leaderboard, message_role, to_i64};
pub use leaderboard::{
    LeaderboardEntry, LeaderboardMetric, LeaderboardPeriod, LeaderboardRank, TokenLeaderboard,
};

#[derive(Clone)]
enum Backend {
    Memory(Arc<Mutex<MemoryData>>),
    Postgres(Arc<tokio_postgres::Client>),
}

#[derive(Default)]
struct MemoryData {
    conversations: HashMap<String, MemoryConversation>,
    messages: Vec<MemoryMessage>,
    usage_events: Vec<MemoryUsageEvent>,
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

struct MemoryUsageEvent {
    conversation_id: String,
    user_id: String,
    display_name: String,
    input_tokens: u64,
    output_tokens: u64,
    cached_tokens: u64,
    created_at: SystemTime,
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
    ///
    /// Production callers must propagate failure instead of substituting the
    /// in-memory test backend, otherwise leaderboard totals disappear on restart.
    pub async fn from_env() -> anyhow::Result<Self> {
        let url = config::env_or("DATABASE_URL", DEFAULT_DATABASE_URL);
        let attempts =
            config::env_parse("DATABASE_CONNECT_MAX_ATTEMPTS", DEFAULT_CONNECT_ATTEMPTS).max(1);
        let retry_delay = std::time::Duration::from_secs(config::env_parse(
            "DATABASE_CONNECT_RETRY_SECS",
            DEFAULT_CONNECT_RETRY_SECS,
        ));
        let attempt_timeout = std::time::Duration::from_secs(
            config::env_parse(
                "DATABASE_CONNECT_TIMEOUT_SECS",
                DEFAULT_CONNECT_TIMEOUT_SECS,
            )
            .max(1),
        );
        let (client, connection) =
            connect_with_retry(&url, attempts, retry_delay, attempt_timeout).await?;
        tokio::spawn(async move {
            if let Err(error) = connection.await {
                tracing::error!(%error, "PostgreSQL token-monitor connection closed");
            }
        });
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
                let mut data = data.lock().await;
                if let Some(conversation) = data.conversations.get_mut(conversation_id) {
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
                    let event = MemoryUsageEvent {
                        conversation_id: conversation_id.into(),
                        user_id: conversation.user_id.clone(),
                        display_name: conversation.display_name.clone(),
                        input_tokens: usage.prompt_tokens,
                        output_tokens: usage.completion_tokens,
                        cached_tokens: usage.prompt_tokens_details.cached_tokens,
                        created_at: SystemTime::now(),
                    };
                    data.usage_events.push(event);
                }
            }
            Backend::Postgres(client) => {
                client
                    .execute(
                        "WITH updated AS (\
                            UPDATE conversations SET \
                                input_tokens = input_tokens + $2, \
                                output_tokens = output_tokens + $3, \
                                cached_tokens = cached_tokens + $4, \
                                request_count = request_count + 1 \
                            WHERE conversation_id = $1 \
                            RETURNING conversation_id, user_id, display_name\
                         ) \
                         INSERT INTO token_usage_events \
                            (conversation_id, user_id, display_name, input_tokens, output_tokens, cached_tokens) \
                         SELECT conversation_id, user_id, display_name, $2, $3, $4 \
                         FROM updated",
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
                data.usage_events
                    .retain(|event| retained.contains(&event.conversation_id));
            }
            Backend::Postgres(client) => {
                client
                    .execute("DELETE FROM conversations WHERE user_id = $1", &[&user_id])
                    .await?;
            }
        }
        Ok(())
    }

    /// Return the most recent conversation ID that has not yet been ended for
    /// `user_id`, or `None` if no active conversation exists.
    ///
    /// Called on startup to resume the previous conversation across bot
    /// restarts so token counts accumulate on the same row and the leaderboard
    /// shows correct persistent totals.
    pub async fn get_active_conversation_id(&self, user_id: &str) -> Option<String> {
        match &self.backend {
            Backend::Memory(_) => None,
            Backend::Postgres(client) => client
                .query_opt(
                    "SELECT conversation_id FROM conversations \
                     WHERE user_id = $1 AND ended_at IS NULL \
                     ORDER BY started_at DESC LIMIT 1",
                    &[&user_id],
                )
                .await
                .ok()?
                .map(|row| row.get(0)),
        }
    }

    pub async fn leaderboard(&self, limit: usize) -> anyhow::Result<TokenLeaderboard> {
        self.leaderboard_for(
            LeaderboardPeriod::AllTime,
            LeaderboardMetric::TotalTokens,
            limit,
            None,
        )
        .await
    }

    pub async fn leaderboard_for(
        &self,
        period: LeaderboardPeriod,
        metric: LeaderboardMetric,
        limit: usize,
        requester_id: Option<&str>,
    ) -> anyhow::Result<TokenLeaderboard> {
        let limit = limit.clamp(1, 25);
        match &self.backend {
            Backend::Memory(data) => {
                let data = data.lock().await;
                Ok(memory_leaderboard(
                    &data,
                    period,
                    metric,
                    limit,
                    requester_id,
                    SystemTime::now(),
                ))
            }
            Backend::Postgres(client) => {
                let filter = period.sql_filter();
                let (user_source, conversation_count) = match period {
                    LeaderboardPeriod::AllTime => ("conversations", "COUNT(*)"),
                    _ => ("token_usage_events", "COUNT(DISTINCT conversation_id)"),
                };
                let users = client
                    .query(
                        &format!(
                            "SELECT user_id, COALESCE(NULLIF(MAX(display_name), ''), user_id), \
                                    {conversation_count}, SUM(input_tokens)::BIGINT, SUM(output_tokens)::BIGINT, SUM(cached_tokens)::BIGINT \
                             FROM {user_source}{filter} GROUP BY user_id"
                        ),
                        &[],
                    )
                    .await?
                    .into_iter()
                    .map(|row| LeaderboardEntry {
                        user_id: Some(row.get(0)),
                        label: row.get(1),
                        conversation_id: None,
                        conversations: from_i64(row.get(2)),
                        input_tokens: from_i64(row.get(3)),
                        output_tokens: from_i64(row.get(4)),
                        cached_tokens: from_i64(row.get(5)),
                    })
                    .collect::<Vec<_>>();
                let (conversation_source, conversation_filter, order) = match (period, metric) {
                    (LeaderboardPeriod::AllTime, LeaderboardMetric::TotalTokens) => (
                        "SELECT display_name, conversation_id, input_tokens, output_tokens, cached_tokens FROM conversations",
                        "",
                        "input_tokens + output_tokens DESC",
                    ),
                    (LeaderboardPeriod::AllTime, LeaderboardMetric::CacheEfficiency) => (
                        "SELECT display_name, conversation_id, input_tokens, output_tokens, cached_tokens FROM conversations",
                        "",
                        "cached_tokens::double precision / GREATEST(input_tokens, 1) DESC, input_tokens + output_tokens DESC",
                    ),
                    (_, LeaderboardMetric::TotalTokens) => (
                        "SELECT MAX(display_name), conversation_id, SUM(input_tokens)::BIGINT, SUM(output_tokens)::BIGINT, SUM(cached_tokens)::BIGINT FROM token_usage_events",
                        filter,
                        "SUM(input_tokens + output_tokens) DESC",
                    ),
                    (_, LeaderboardMetric::CacheEfficiency) => (
                        "SELECT MAX(display_name), conversation_id, SUM(input_tokens)::BIGINT, SUM(output_tokens)::BIGINT, SUM(cached_tokens)::BIGINT FROM token_usage_events",
                        filter,
                        "SUM(cached_tokens)::double precision / GREATEST(SUM(input_tokens), 1) DESC, SUM(input_tokens + output_tokens) DESC",
                    ),
                };
                let group_by = if period == LeaderboardPeriod::AllTime {
                    ""
                } else {
                    " GROUP BY conversation_id"
                };
                let conversations = client
                    .query(
                        &format!(
                            "{conversation_source}{conversation_filter}{group_by} ORDER BY {order} LIMIT $1"
                        ),
                        &[&(limit as i64)],
                    )
                    .await?
                    .into_iter()
                    .map(|row| LeaderboardEntry {
                        user_id: None,
                        label: row.get(0),
                        conversation_id: Some(row.get(1)),
                        conversations: 1,
                        input_tokens: from_i64(row.get(2)),
                        output_tokens: from_i64(row.get(3)),
                        cached_tokens: from_i64(row.get(4)),
                    })
                    .collect();
                Ok(finish_leaderboard(
                    users,
                    conversations,
                    period,
                    metric,
                    limit,
                    requester_id,
                ))
            }
        }
    }
}

async fn connect_with_retry(
    url: &str,
    attempts: usize,
    retry_delay: Duration,
    attempt_timeout: Duration,
) -> anyhow::Result<(
    tokio_postgres::Client,
    tokio_postgres::Connection<tokio_postgres::Socket, tokio_postgres::tls::NoTlsStream>,
)> {
    let attempts = attempts.max(1);
    let mut last_error = None;
    for attempt in 1..=attempts {
        let result =
            tokio::time::timeout(attempt_timeout, tokio_postgres::connect(url, NoTls)).await;
        match result {
            Ok(Ok(connection)) => return Ok(connection),
            Ok(Err(error)) => last_error = Some(error.to_string()),
            Err(_) => {
                last_error = Some(format!(
                    "connection attempt timed out after {attempt_timeout:?}"
                ))
            }
        }
        tracing::warn!(
            attempt,
            attempts,
            error = %last_error.as_deref().expect("failed attempt records an error"),
            "PostgreSQL token monitor connection failed"
        );
        if attempt < attempts && !retry_delay.is_zero() {
            tokio::time::sleep(retry_delay).await;
        }
    }
    Err(anyhow::anyhow!(
        "could not connect persistent token monitor after {attempts} attempt(s): {}",
        last_error.expect("at least one connection attempt ran")
    ))
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
