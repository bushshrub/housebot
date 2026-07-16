//! Agent tool for fetching token usage metrics and statistics.

use serde_json::{json, Value};

use crate::token_monitor::{
    LeaderboardEntry, LeaderboardMetric, LeaderboardPeriod, TokenMonitor,
};

/// OpenAI-style tool definition (internal `input_schema` form).
pub fn definition() -> Value {
    json!({
        "name": "token_metrics",
        "description": "Fetch token usage metrics and statistics for the bot. Can return global \
            totals across all users or drill into a specific user's usage. Use this whenever a \
            user asks about token usage, their token stats, the token leaderboard, or how many \
            tokens they or someone else has used.",
        "input_schema": {
            "type": "object",
            "properties": {
                "scope": {
                    "type": "string",
                    "enum": ["global", "user"],
                    "description": "Scope of the query. 'global' returns overall statistics \
                        across all users (totals + leaderboard). 'user' returns per-user \
                        breakdown, optionally filtered to a specific user_id."
                },
                "user_id": {
                    "type": "string",
                    "description": "Discord user ID to focus on. With scope='user', shows that \
                        user's personal stats and top conversations. With scope='global', finds \
                        their rank in the leaderboard."
                },
                "period": {
                    "type": "string",
                    "enum": ["daily", "weekly", "monthly", "all_time"],
                    "description": "Time window for aggregation. Default: all_time."
                },
                "metric": {
                    "type": "string",
                    "enum": ["total_tokens", "cache_efficiency"],
                    "description": "Metric to sort and rank by. Default: total_tokens."
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of results (1-25). Default: 10."
                }
            },
            "required": ["scope"]
        }
    })
}

fn parse_period(s: &str) -> LeaderboardPeriod {
    match s {
        "daily" => LeaderboardPeriod::Daily,
        "weekly" => LeaderboardPeriod::Weekly,
        "monthly" => LeaderboardPeriod::Monthly,
        _ => LeaderboardPeriod::AllTime,
    }
}

fn parse_metric(s: &str) -> LeaderboardMetric {
    match s {
        "cache_efficiency" => LeaderboardMetric::CacheEfficiency,
        _ => LeaderboardMetric::TotalTokens,
    }
}

/// Fetch token metrics and return a formatted text report.
pub async fn get_token_metrics(
    monitor: &TokenMonitor,
    scope: &str,
    user_id: Option<&str>,
    period_str: &str,
    metric_str: &str,
    limit: usize,
) -> String {
    let period = parse_period(period_str);
    let metric = parse_metric(metric_str);
    let limit = limit.clamp(1, 25);

    let leaderboard = match monitor.leaderboard_for(period, metric, limit, user_id).await {
        Ok(lb) => lb,
        Err(e) => return format!("Error: failed to query token metrics: {e}"),
    };

    let total_input: u64 = leaderboard.users.iter().map(|u| u.input_tokens).sum();
    let total_output: u64 = leaderboard.users.iter().map(|u| u.output_tokens).sum();
    let total_cached: u64 = leaderboard.users.iter().map(|u| u.cached_tokens).sum();
    let total_all = total_input + total_output;

    match scope {
        "user" => format_user_metrics(&leaderboard, user_id, total_input, total_output, total_cached, total_all),
        _ => format_global_metrics(&leaderboard, total_input, total_output, total_cached, total_all),
    }
}

fn format_global_metrics(
    leaderboard: &crate::token_monitor::TokenLeaderboard,
    total_input: u64,
    total_output: u64,
    total_cached: u64,
    total_all: u64,
) -> String {
    let mut lines = vec![format!(
        "Token Usage — {} ({})",
        leaderboard.period.label(),
        leaderboard.metric.label(),
    )];

    lines.push(format!(
        "Total: {} tokens ({} input + {} output) | {} cached",
        total_all, total_input, total_output, total_cached
    ));

    if leaderboard.users.is_empty() {
        lines.push("No token usage recorded for this period.".to_string());
        return lines.join("\n");
    }

    lines.push(String::new());
    lines.push("Top users:".to_string());
    for (i, entry) in leaderboard.users.iter().enumerate() {
        lines.push(format!(
            "{:>2}. {} — {} ({} conversations)",
            i + 1,
            entry.label,
            format_metric(entry, leaderboard.metric),
            entry.conversations,
        ));
    }

    if let Some(rank) = &leaderboard.requester_rank {
        lines.push(format!(
            "Your rank: #{position} — {metric}",
            position = rank.position,
            metric = format_metric(&rank.entry, leaderboard.metric),
        ));
    }

    if !leaderboard.conversations.is_empty() {
        lines.push(String::new());
        lines.push("Top conversations:".to_string());
        for (i, entry) in leaderboard.conversations.iter().take(5).enumerate() {
            let id = entry
                .conversation_id
                .as_deref()
                .unwrap_or("unknown")
                .chars()
                .take(8)
                .collect::<String>();
            lines.push(format!(
                "{:>2}. {} ({}) — {}",
                i + 1,
                entry.label,
                id,
                format_metric(entry, leaderboard.metric),
            ));
        }
    }

    lines.join("\n")
}

fn format_user_metrics(
    leaderboard: &crate::token_monitor::TokenLeaderboard,
    user_id: Option<&str>,
    total_input: u64,
    total_output: u64,
    total_cached: u64,
    total_all: u64,
) -> String {
    let user_id = user_id.unwrap_or("");
    let mut lines = vec![format!(
        "Token Usage — {} ({})",
        leaderboard.period.label(),
        leaderboard.metric.label(),
    )];

    lines.push(format!(
        "Server total: {} tokens ({} input + {} output) | {} cached",
        total_all, total_input, total_output, total_cached
    ));

    let user_entry = leaderboard
        .users
        .iter()
        .find(|e| e.user_id.as_deref() == Some(user_id))
        .or_else(|| {
            leaderboard
                .requester_rank
                .as_ref()
                .map(|r| &r.entry)
        });

    match user_entry {
        Some(entry) => {
            let label = if !user_id.is_empty() {
                format!("User: {}", entry.label)
            } else {
                "Your stats".to_string()
            };
            lines.push(String::new());
            lines.push(label);
            let rank = leaderboard
                .requester_rank
                .as_ref()
                .filter(|r| r.entry.user_id.as_deref() == Some(user_id))
                .map(|r| format!("#{} ", r.position))
                .unwrap_or_default();
            lines.push(format!(
                "{}— {} ({} conversations)",
                rank,
                format_metric(entry, leaderboard.metric),
                entry.conversations,
            ));
            lines.push(format!(
                "  Input: {} | Output: {} | Cached: {}",
                entry.input_tokens, entry.output_tokens, entry.cached_tokens,
            ));

            let user_convs: Vec<&LeaderboardEntry> = leaderboard
                .conversations
                .iter()
                .filter(|c| c.label == entry.label)
                .take(5)
                .collect();
            if !user_convs.is_empty() {
                lines.push("  Conversations:".to_string());
                for conv in user_convs {
                    let id = conv
                        .conversation_id
                        .as_deref()
                        .unwrap_or("unknown")
                        .chars()
                        .take(8)
                        .collect::<String>();
                    lines.push(format!(
                        "    {} ({}) — {}",
                        conv.label,
                        id,
                        format_metric(conv, leaderboard.metric),
                    ));
                }
            }
        }
        None => {
            if !user_id.is_empty() {
                lines.push(format!("User {} not found in this period.", user_id));
            } else {
                lines.push("No token usage recorded for this period.".to_string());
            }
        }
    }

    if !leaderboard.users.is_empty() && user_entry.is_none() {
        lines.push(String::new());
        lines.push("Top users:".to_string());
        for (i, entry) in leaderboard.users.iter().take(5).enumerate() {
            lines.push(format!(
                "{:>2}. {} — {}",
                i + 1,
                entry.label,
                format_metric(entry, leaderboard.metric),
            ));
        }
    }

    lines.join("\n")
}

fn format_metric(entry: &LeaderboardEntry, metric: LeaderboardMetric) -> String {
    match metric {
        LeaderboardMetric::TotalTokens => format!("{} tokens", entry.total_tokens()),
        LeaderboardMetric::CacheEfficiency => format!(
            "{:.1}% cached ({} tokens)",
            entry.cache_efficiency(),
            entry.total_tokens(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn definition_has_required_fields() {
        let d = definition();
        assert_eq!(d["name"], "token_metrics");
        let props = &d["input_schema"]["properties"];
        assert!(props.get("scope").is_some());
        assert!(props.get("period").is_some());
        assert!(props.get("metric").is_some());
        assert!(props.get("limit").is_some());
        assert_eq!(d["input_schema"]["required"], json!(["scope"]));
    }

    #[test]
    fn parse_period_defaults() {
        assert_eq!(parse_period("daily"), LeaderboardPeriod::Daily);
        assert_eq!(parse_period("weekly"), LeaderboardPeriod::Weekly);
        assert_eq!(parse_period("monthly"), LeaderboardPeriod::Monthly);
        assert_eq!(parse_period("all_time"), LeaderboardPeriod::AllTime);
        assert_eq!(parse_period("unknown"), LeaderboardPeriod::AllTime);
    }

    #[test]
    fn parse_metric_defaults() {
        assert_eq!(parse_metric("total_tokens"), LeaderboardMetric::TotalTokens);
        assert_eq!(
            parse_metric("cache_efficiency"),
            LeaderboardMetric::CacheEfficiency
        );
        assert_eq!(parse_metric("unknown"), LeaderboardMetric::TotalTokens);
    }
}
