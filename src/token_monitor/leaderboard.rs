//! Leaderboard value types and pure aggregation helpers, split out of
//! `token_monitor` to keep that module under 600 lines.

use std::collections::HashMap;
use std::time::{Duration, SystemTime};

use serde_json::Value;

use super::MemoryData;

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

    pub(super) fn sql_filter(self) -> &'static str {
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

pub(super) fn memory_leaderboard(
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

pub(super) fn finish_leaderboard(
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

pub(super) fn message_role(message: &Value) -> &str {
    message
        .get("role")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
}

pub(super) fn to_i64(value: u64) -> i64 {
    value.min(i64::MAX as u64) as i64
}

pub(super) fn from_i64(value: i64) -> u64 {
    value.max(0) as u64
}
