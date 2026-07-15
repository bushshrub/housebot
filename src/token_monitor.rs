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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeaderboardEntry {
    pub user_id: Option<String>,
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

    pub fn cache_efficiency(&self) -> f64 {
        if self.input_tokens == 0 {
            0.0
        } else {
            self.cached_tokens as f64 / self.input_tokens as f64 * 100.0
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum LeaderboardPeriod {
    Daily,
    Weekly,
    Monthly,
    #[default]
    AllTime,
}

impl LeaderboardPeriod {
    fn cutoff(self, now: SystemTime) -> Option<SystemTime> {
        let days = match self {
            Self::Daily => 1,
            Self::Weekly => 7,
            Self::Monthly => 30,
            Self::AllTime => return None,
        };
        now.checked_sub(Duration::from_secs(days * 24 * 60 * 60))
    }

    fn sql_filter(self) -> &'static str {
        match self {
            Self::Daily => " WHERE created_at >= NOW() - INTERVAL '1 day'",
            Self::Weekly => " WHERE created_at >= NOW() - INTERVAL '7 days'",
            Self::Monthly => " WHERE created_at >= NOW() - INTERVAL '30 days'",
            Self::AllTime => "",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Daily => "Daily",
            Self::Weekly => "Weekly",
            Self::Monthly => "Monthly",
            Self::AllTime => "All time",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum LeaderboardMetric {
    #[default]
    TotalTokens,
    CacheEfficiency,
}

impl LeaderboardMetric {
    pub fn label(self) -> &'static str {
        match self {
            Self::TotalTokens => "Total tokens",
            Self::CacheEfficiency => "Cache efficiency",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeaderboardRank {
    pub position: usize,
    pub entry: LeaderboardEntry,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TokenLeaderboard {
    pub users: Vec<LeaderboardEntry>,
    pub conversations: Vec<LeaderboardEntry>,
    pub requester_rank: Option<LeaderboardRank>,
    pub period: LeaderboardPeriod,
    pub metric: LeaderboardMetric,
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
                    ON conversation_messages (conversation_id, id);\
                CREATE TABLE IF NOT EXISTS token_usage_events (\
                    id BIGSERIAL PRIMARY KEY,\
                    conversation_id TEXT NOT NULL REFERENCES conversations(conversation_id) ON DELETE CASCADE,\
                    user_id TEXT NOT NULL,\
                    display_name TEXT NOT NULL,\
                    input_tokens BIGINT NOT NULL DEFAULT 0,\
                    output_tokens BIGINT NOT NULL DEFAULT 0,\
                    cached_tokens BIGINT NOT NULL DEFAULT 0,\
                    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()\
                );\
                CREATE INDEX IF NOT EXISTS token_usage_events_created_at_idx\
                    ON token_usage_events (created_at);\
                CREATE INDEX IF NOT EXISTS token_usage_events_user_id_idx\
                    ON token_usage_events (user_id, created_at);",
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
                                    {conversation_count}, SUM(input_tokens), SUM(output_tokens), SUM(cached_tokens) \
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
                        "SELECT MAX(display_name), conversation_id, SUM(input_tokens), SUM(output_tokens), SUM(cached_tokens) FROM token_usage_events",
                        filter,
                        "SUM(input_tokens + output_tokens) DESC",
                    ),
                    (_, LeaderboardMetric::CacheEfficiency) => (
                        "SELECT MAX(display_name), conversation_id, SUM(input_tokens), SUM(output_tokens), SUM(cached_tokens) FROM token_usage_events",
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

fn memory_leaderboard(
    data: &MemoryData,
    period: LeaderboardPeriod,
    metric: LeaderboardMetric,
    limit: usize,
    requester_id: Option<&str>,
    now: SystemTime,
) -> TokenLeaderboard {
    let mut conversation_totals: HashMap<String, LeaderboardEntry> = HashMap::new();
    if period == LeaderboardPeriod::AllTime {
        for (id, conversation) in &data.conversations {
            conversation_totals.insert(
                id.clone(),
                LeaderboardEntry {
                    user_id: Some(conversation.user_id.clone()),
                    label: conversation.display_name.clone(),
                    conversation_id: Some(id.clone()),
                    conversations: 1,
                    input_tokens: conversation.input_tokens,
                    output_tokens: conversation.output_tokens,
                    cached_tokens: conversation.cached_tokens,
                },
            );
        }
    } else {
        let cutoff = period.cutoff(now);
        for event in &data.usage_events {
            if cutoff.is_some_and(|cutoff| event.created_at < cutoff) {
                continue;
            }
            let conversation = conversation_totals
                .entry(event.conversation_id.clone())
                .or_insert_with(|| LeaderboardEntry {
                    user_id: Some(event.user_id.clone()),
                    label: event.display_name.clone(),
                    conversation_id: Some(event.conversation_id.clone()),
                    conversations: 1,
                    input_tokens: 0,
                    output_tokens: 0,
                    cached_tokens: 0,
                });
            conversation.input_tokens =
                conversation.input_tokens.saturating_add(event.input_tokens);
            conversation.output_tokens = conversation
                .output_tokens
                .saturating_add(event.output_tokens);
            conversation.cached_tokens = conversation
                .cached_tokens
                .saturating_add(event.cached_tokens);
        }
    }

    let mut by_user: HashMap<String, LeaderboardEntry> = HashMap::new();
    for conversation in conversation_totals.values() {
        let user_id = conversation.user_id.as_deref().unwrap_or_default();
        let user = by_user
            .entry(user_id.to_string())
            .or_insert_with(|| LeaderboardEntry {
                user_id: Some(user_id.to_string()),
                label: conversation.label.clone(),
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
    }
    let mut conversations = conversation_totals.into_values().collect::<Vec<_>>();
    for conversation in &mut conversations {
        conversation.user_id = None;
    }
    sort_entries(&mut conversations, metric);
    conversations.truncate(limit);
    finish_leaderboard(
        by_user.into_values().collect(),
        conversations,
        period,
        metric,
        limit,
        requester_id,
    )
}

fn finish_leaderboard(
    mut users: Vec<LeaderboardEntry>,
    conversations: Vec<LeaderboardEntry>,
    period: LeaderboardPeriod,
    metric: LeaderboardMetric,
    limit: usize,
    requester_id: Option<&str>,
) -> TokenLeaderboard {
    sort_entries(&mut users, metric);
    let requester_rank = requester_id.and_then(|requester_id| {
        users
            .iter()
            .position(|entry| entry.user_id.as_deref() == Some(requester_id))
            .map(|index| LeaderboardRank {
                position: index + 1,
                entry: users[index].clone(),
            })
    });
    users.truncate(limit);
    TokenLeaderboard {
        users,
        conversations,
        requester_rank,
        period,
        metric,
    }
}

fn sort_entries(entries: &mut [LeaderboardEntry], metric: LeaderboardMetric) {
    entries.sort_by(|left, right| match metric {
        LeaderboardMetric::TotalTokens => right.total_tokens().cmp(&left.total_tokens()),
        LeaderboardMetric::CacheEfficiency => right
            .cache_efficiency()
            .total_cmp(&left.cache_efficiency())
            .then_with(|| right.total_tokens().cmp(&left.total_tokens())),
    });
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

    #[tokio::test]
    async fn timeframe_excludes_old_conversations() {
        let monitor = TokenMonitor::default();
        monitor
            .start_conversation("old", "u1", "Alice", 10)
            .await
            .unwrap();
        monitor
            .record_usage("old", usage(500, 100, 0))
            .await
            .unwrap();
        monitor
            .start_conversation("recent", "u2", "Bob", 10)
            .await
            .unwrap();
        monitor
            .record_usage("recent", usage(50, 10, 0))
            .await
            .unwrap();

        if let Backend::Memory(data) = &monitor.backend {
            data.lock()
                .await
                .usage_events
                .iter_mut()
                .find(|event| event.conversation_id == "old")
                .unwrap()
                .created_at = SystemTime::now() - Duration::from_secs(2 * 24 * 60 * 60);
        }

        let board = monitor
            .leaderboard_for(
                LeaderboardPeriod::Daily,
                LeaderboardMetric::TotalTokens,
                10,
                None,
            )
            .await
            .unwrap();
        assert_eq!(board.users.len(), 1);
        assert_eq!(board.users[0].label, "Bob");
        assert_eq!(board.period, LeaderboardPeriod::Daily);
    }

    #[tokio::test]
    async fn efficiency_metric_and_requester_rank_are_reported() {
        let monitor = TokenMonitor::default();
        for (id, user, name, token_usage) in [
            ("c1", "u1", "Efficient", usage(100, 1, 90)),
            ("c2", "u2", "Heavy", usage(1_000, 1_000, 5)),
            ("c3", "u3", "Requester", usage(100, 1, 10)),
        ] {
            monitor
                .start_conversation(id, user, name, 10)
                .await
                .unwrap();
            monitor.record_usage(id, token_usage).await.unwrap();
        }

        let board = monitor
            .leaderboard_for(
                LeaderboardPeriod::AllTime,
                LeaderboardMetric::CacheEfficiency,
                1,
                Some("u3"),
            )
            .await
            .unwrap();
        assert_eq!(board.users[0].label, "Efficient");
        assert_eq!(board.requester_rank.as_ref().unwrap().position, 2);
        assert_eq!(
            board.requester_rank.as_ref().unwrap().entry.label,
            "Requester"
        );
    }

    #[tokio::test]
    async fn get_active_conversation_id_returns_none_for_memory_backend() {
        // The in-memory backend has no recovery mechanism; None tells callers
        // to start a fresh conversation (which still accumulates correctly in
        // the leaderboard across the session).
        let monitor = TokenMonitor::default();
        monitor
            .start_conversation("conv1", "u1", "Alice", 10)
            .await
            .unwrap();
        assert_eq!(monitor.get_active_conversation_id("u1").await, None);
        assert_eq!(monitor.get_active_conversation_id("unknown").await, None);
    }

    #[tokio::test]
    async fn leaderboard_accumulates_across_multiple_conversations() {
        // Verify that even when a new conversation is created (simulating a
        // restart with the in-memory backend), the leaderboard sums tokens
        // from all conversations for the same user.
        let monitor = TokenMonitor::default();
        monitor
            .start_conversation("c1", "u1", "Alice", 10)
            .await
            .unwrap();
        monitor.record_usage("c1", usage(100, 40, 0)).await.unwrap();
        monitor.finish_conversation("c1").await.unwrap();
        // Simulate restart: new conversation created for the same user.
        monitor
            .start_conversation("c2", "u1", "Alice", 10)
            .await
            .unwrap();
        monitor.record_usage("c2", usage(60, 20, 0)).await.unwrap();

        let board = monitor.leaderboard(10).await.unwrap();
        assert_eq!(board.users.len(), 1);
        assert_eq!(board.users[0].label, "Alice");
        assert_eq!(board.users[0].conversations, 2);
        assert_eq!(
            board.users[0].total_tokens(),
            220,
            "tokens must sum across conversations"
        );
    }
}
