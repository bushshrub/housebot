//! Leaderboard and aggregate statistics queries.
use crate::*;

impl TokenMonitor {
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
    pub async fn get_global_stats(
        &self,
        period: LeaderboardPeriod,
    ) -> anyhow::Result<GlobalTokenStats> {
        match &self.backend {
            Backend::Memory(data) => {
                let data = data.lock().await;
                let now = SystemTime::now();
                let cutoff = period.cutoff(now);

                if period == LeaderboardPeriod::AllTime {
                    let user_ids: std::collections::HashSet<_> = data
                        .conversations
                        .values()
                        .map(|c| c.user_id.clone())
                        .collect();
                    let total_conversations = data.conversations.len() as u64;

                    let mut total_input = 0u64;
                    let mut total_output = 0u64;
                    let mut total_cached = 0u64;
                    for event in &data.usage_events {
                        total_input = total_input.saturating_add(event.input_tokens);
                        total_output = total_output.saturating_add(event.output_tokens);
                        total_cached = total_cached.saturating_add(event.cached_tokens);
                    }

                    Ok(GlobalTokenStats {
                        total_users: user_ids.len() as u64,
                        total_conversations,
                        total_input_tokens: total_input,
                        total_output_tokens: total_output,
                        total_cached_tokens: total_cached,
                        period,
                    })
                } else {
                    let cutoff = cutoff.expect("non-all-time periods have a cutoff");
                    let user_ids: std::collections::HashSet<_> = data
                        .usage_events
                        .iter()
                        .filter(|e| e.created_at >= cutoff)
                        .map(|e| e.user_id.clone())
                        .collect();
                    let total_conversations = data
                        .usage_events
                        .iter()
                        .filter(|e| e.created_at >= cutoff)
                        .map(|e| &e.conversation_id[..])
                        .collect::<std::collections::HashSet<_>>()
                        .len() as u64;

                    let mut total_input = 0u64;
                    let mut total_output = 0u64;
                    let mut total_cached = 0u64;
                    for event in &data.usage_events {
                        if event.created_at < cutoff {
                            continue;
                        }
                        total_input = total_input.saturating_add(event.input_tokens);
                        total_output = total_output.saturating_add(event.output_tokens);
                        total_cached = total_cached.saturating_add(event.cached_tokens);
                    }

                    Ok(GlobalTokenStats {
                        total_users: user_ids.len() as u64,
                        total_conversations,
                        total_input_tokens: total_input,
                        total_output_tokens: total_output,
                        total_cached_tokens: total_cached,
                        period,
                    })
                }
            }
            Backend::Postgres(client) => {
                let filter = period.sql_filter();
                let (source, count_query) = match period {
                    LeaderboardPeriod::AllTime => ("conversations", "COUNT(*)"),
                    _ => ("token_usage_events", "COUNT(DISTINCT conversation_id)"),
                };
                let row = client
                    .query_one(
                        &format!(
                            "SELECT COUNT(DISTINCT user_id), {count_query}, \
                             COALESCE(SUM(input_tokens), 0)::BIGINT, \
                             COALESCE(SUM(output_tokens), 0)::BIGINT, \
                             COALESCE(SUM(cached_tokens), 0)::BIGINT \
                             FROM {source}{filter}"
                        ),
                        &[],
                    )
                    .await?;
                let total_users: i64 = row.get(0);
                Ok(GlobalTokenStats {
                    total_users: total_users.max(0) as u64,
                    total_conversations: from_i64(row.get(1)),
                    total_input_tokens: from_i64(row.get(2)),
                    total_output_tokens: from_i64(row.get(3)),
                    total_cached_tokens: from_i64(row.get(4)),
                    period,
                })
            }
        }
    }

    pub async fn get_user_stats(
        &self,
        user_id: &str,
        period: LeaderboardPeriod,
    ) -> anyhow::Result<Option<LeaderboardEntry>> {
        match &self.backend {
            Backend::Memory(data) => {
                let data = data.lock().await;
                let now = SystemTime::now();
                let cutoff = period.cutoff(now);

                let mut input = 0u64;
                let mut output = 0u64;
                let mut cached = 0u64;
                let mut conversations = std::collections::HashSet::new();

                for event in &data.usage_events {
                    if cutoff.is_some_and(|cutoff| event.created_at < cutoff) {
                        continue;
                    }
                    if event.user_id == user_id {
                        input = input.saturating_add(event.input_tokens);
                        output = output.saturating_add(event.output_tokens);
                        cached = cached.saturating_add(event.cached_tokens);
                        conversations.insert(&event.conversation_id[..]);
                    }
                }

                let display_name = data
                    .conversations
                    .values()
                    .find(|c| c.user_id == user_id)
                    .map(|c| c.display_name.clone())
                    .unwrap_or_else(|| user_id.to_string());

                if conversations.is_empty() {
                    Ok(None)
                } else {
                    Ok(Some(LeaderboardEntry {
                        user_id: Some(user_id.to_string()),
                        label: display_name,
                        conversation_id: None,
                        conversations: conversations.len() as u64,
                        input_tokens: input,
                        output_tokens: output,
                        cached_tokens: cached,
                    }))
                }
            }
            Backend::Postgres(client) => {
                let filter = period.sql_filter();
                let (source, count_query) = match period {
                    LeaderboardPeriod::AllTime => ("conversations", "COUNT(*)"),
                    _ => ("token_usage_events", "COUNT(DISTINCT conversation_id)"),
                };
                let user_filter = if period == LeaderboardPeriod::AllTime {
                    " WHERE user_id = $1".to_string()
                } else {
                    let base = filter.trim_start_matches(" WHERE ");
                    format!(" WHERE user_id = $1 AND {base}")
                };
                let row = client
                    .query_opt(
                        &format!(
                            "SELECT COALESCE(NULLIF(MAX(display_name), ''), $1), {count_query}, \
                             SUM(input_tokens)::BIGINT, SUM(output_tokens)::BIGINT, SUM(cached_tokens)::BIGINT \
                             FROM {source}{user_filter}"
                        ),
                        &[&user_id],
                    )
                    .await?;
                match row {
                    Some(r) => {
                        let conversations: i64 = r.get(1);
                        if conversations == 0 {
                            return Ok(None);
                        }
                        Ok(Some(LeaderboardEntry {
                            user_id: Some(user_id.to_string()),
                            label: r.get(0),
                            conversation_id: None,
                            conversations: conversations as u64,
                            input_tokens: from_i64(r.get(2)),
                            output_tokens: from_i64(r.get(3)),
                            cached_tokens: from_i64(r.get(4)),
                        }))
                    }
                    None => Ok(None),
                }
            }
        }
    }
}
