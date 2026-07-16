//! Agent tool for fetching versatile token usage metrics — global totals and
//! per-user breakdowns with period filtering.

use serde_json::{json, Value};

use crate::token_monitor::{LeaderboardMetric, LeaderboardPeriod, TokenMonitor};

const MAX_DISPLAY_USERS: usize = 15;

/// OpenAI-style tool definition (internal `input_schema` form).
pub fn definition() -> Value {
    json!({
        "name": "get_token_metrics",
        "description": "Fetch token usage metrics. Returns global bot-wide statistics \
            (total users, conversations, token breakdown) or per-user details \
            (token usage, conversation count, rank). Supports period filtering \
            (daily, weekly, monthly, all-time). More versatile than the \
            /token_leaderboard command — use this when you need structured \
            token-usage data for analysis.",
        "input_schema": {
            "type": "object",
            "properties": {
                "user_id": {
                    "type": "string",
                    "description": "Optional Discord user ID to fetch metrics for a specific user. \
                        If omitted, returns global bot-wide metrics for all users."
                },
                "period": {
                    "type": "string",
                    "enum": ["daily", "weekly", "monthly", "all_time"],
                    "description": "Time period for metrics. Default: all_time."
                },
                "metric": {
                    "type": "string",
                    "enum": ["total_tokens", "cache_efficiency"],
                    "description": "Sorting/display metric. Default: total_tokens."
                }
            }
        }
    })
}

/// Fetch and format token usage metrics.
pub async fn get_token_metrics(
    token_monitor: &TokenMonitor,
    user_id: Option<&str>,
    period_str: Option<&str>,
    metric_str: Option<&str>,
) -> String {
    let period = parse_period(period_str);
    let metric = parse_metric(metric_str);

    match user_id {
        Some(uid) if !uid.is_empty() => {
            format_user_metrics(token_monitor, uid, period, metric).await
        }
        _ => format_global_metrics(token_monitor, period, metric).await,
    }
}

// ── global metrics ────────────────────────────────────────────────────────────

async fn format_global_metrics(
    token_monitor: &TokenMonitor,
    period: LeaderboardPeriod,
    metric: LeaderboardMetric,
) -> String {
    let global = match token_monitor.get_global_stats(period).await {
        Ok(stats) => stats,
        Err(e) => return format!("Error: failed to fetch global token metrics: {e}"),
    };

    let leaderboard = match token_monitor
        .leaderboard_for(period, metric, MAX_DISPLAY_USERS, None)
        .await
    {
        Ok(lb) => lb,
        Err(e) => return format!("Error: failed to fetch token leaderboard: {e}"),
    };

    let total_tokens = global
        .total_input_tokens
        .saturating_add(global.total_output_tokens);
    let cache_pct = if global.total_input_tokens > 0 {
        (global.total_cached_tokens as f64 / global.total_input_tokens as f64) * 100.0
    } else {
        0.0
    };

    let icon = period_icon(period);
    let mut lines = vec![format!(
        "{icon} **Global Token Metrics — {}**",
        period.label()
    )];

    lines.push(String::new());
    lines.push("**Totals**".into());
    lines.push(format!("• Total users: **{}**", fmt(global.total_users)));
    lines.push(format!(
        "• Total conversations: **{}**",
        fmt(global.total_conversations)
    ));
    lines.push(format!(
        "• Input tokens: **{}**",
        fmt(global.total_input_tokens)
    ));
    lines.push(format!(
        "• Output tokens: **{}**",
        fmt(global.total_output_tokens)
    ));
    lines.push(format!(
        "• Cached tokens: **{}**",
        fmt(global.total_cached_tokens)
    ));
    lines.push(format!("• **Total tokens: {}**", fmt(total_tokens)));
    lines.push(format!("• Cache efficiency: **{cache_pct:.1}%**"));

    if leaderboard.users.is_empty() {
        lines.push(String::new());
        lines.push("No token usage has been recorded for this period.".into());
    } else {
        lines.push(String::new());
        lines.push(format!("**Top {} Users**", leaderboard.users.len()));
        for (i, entry) in leaderboard.users.iter().enumerate() {
            let val = match metric {
                LeaderboardMetric::TotalTokens => {
                    format!("{} tokens", fmt(entry.total_tokens()))
                }
                LeaderboardMetric::CacheEfficiency => {
                    format!(
                        "{:.1}% efficiency ({} tokens)",
                        entry.cache_efficiency(),
                        fmt(entry.total_tokens())
                    )
                }
            };
            lines.push(format!(
                "`{:>2}.` **{}** — {} — {} conv{}",
                i + 1,
                sanitize(&entry.label),
                val,
                entry.conversations,
                if entry.conversations == 1 { "" } else { "s" },
            ));
        }
    }

    lines.join("\n")
}

// ── per-user metrics ──────────────────────────────────────────────────────────

async fn format_user_metrics(
    token_monitor: &TokenMonitor,
    user_id: &str,
    period: LeaderboardPeriod,
    metric: LeaderboardMetric,
) -> String {
    let global = match token_monitor.get_global_stats(period).await {
        Ok(stats) => stats,
        Err(e) => return format!("Error: failed to fetch global token metrics: {e}"),
    };

    let user_entry = match token_monitor.get_user_stats(user_id, period).await {
        Ok(Some(entry)) => entry,
        Ok(None) => {
            return format!(
                "No token usage found for user `{user_id}` in the {} period.",
                period.label().to_lowercase()
            );
        }
        Err(e) => return format!("Error: failed to fetch user token metrics: {e}"),
    };

    let leaderboard = match token_monitor
        .leaderboard_for(period, metric, MAX_DISPLAY_USERS, Some(user_id))
        .await
    {
        Ok(lb) => lb,
        Err(e) => return format!("Error: failed to fetch token leaderboard: {e}"),
    };

    let total_user_tokens = user_entry.total_tokens();
    let global_total = global
        .total_input_tokens
        .saturating_add(global.total_output_tokens);
    let share_pct = if global_total > 0 {
        (total_user_tokens as f64 / global_total as f64) * 100.0
    } else {
        0.0
    };

    let rank_line = match &leaderboard.requester_rank {
        Some(rank) => format!(
            "\n**Rank:** #{} of {} users",
            rank.position,
            fmt(global.total_users),
        ),
        None => String::new(),
    };

    let icon = period_icon(period);
    format!(
        "{icon} **Token Metrics for {} — {}**\n\
         \n\
         **Usage**\n\
         • Conversations: **{}**\n\
         • Input tokens: **{}**\n\
         • Output tokens: **{}**\n\
         • Cached tokens: **{}**\n\
         • **Total tokens: {}**\n\
         • Cache efficiency: **{:.1}%**\n\
         • Share of total usage: **{share_pct:.1}%**{rank_line}",
        sanitize(&user_entry.label),
        period.label(),
        fmt(user_entry.conversations),
        fmt(user_entry.input_tokens),
        fmt(user_entry.output_tokens),
        fmt(user_entry.cached_tokens),
        fmt(total_user_tokens),
        user_entry.cache_efficiency(),
    )
}

// ── helpers ───────────────────────────────────────────────────────────────────

fn parse_period(s: Option<&str>) -> LeaderboardPeriod {
    match s {
        Some("daily") => LeaderboardPeriod::Daily,
        Some("weekly") => LeaderboardPeriod::Weekly,
        Some("monthly") => LeaderboardPeriod::Monthly,
        _ => LeaderboardPeriod::AllTime,
    }
}

fn parse_metric(s: Option<&str>) -> LeaderboardMetric {
    match s {
        Some("cache_efficiency") => LeaderboardMetric::CacheEfficiency,
        _ => LeaderboardMetric::TotalTokens,
    }
}

fn period_icon(period: LeaderboardPeriod) -> &'static str {
    match period {
        LeaderboardPeriod::Daily => "☀️",
        LeaderboardPeriod::Weekly => "📅",
        LeaderboardPeriod::Monthly => "🗓️",
        LeaderboardPeriod::AllTime => "📊",
    }
}

fn fmt(n: impl Into<u64>) -> String {
    let n: u64 = n.into();
    let s = n.to_string();
    let mut result = String::with_capacity(s.len() + s.len() / 3);
    for (i, c) in s.char_indices() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            result.push(',');
        }
        result.push(c);
    }
    result
}

fn sanitize(label: &str) -> String {
    label
        .replace('\\', "\\\\")
        .replace('*', "\\*")
        .replace('_', "\\_")
        .replace('`', "\\`")
        .replace('~', "\\~")
        .replace('|', "\\|")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::token_monitor::TokenMonitor;

    #[tokio::test]
    async fn global_metrics_no_data() {
        let tm = TokenMonitor::default();
        let result = get_token_metrics(&tm, None, Some("all_time"), Some("total_tokens")).await;
        assert!(!result.starts_with("Error:"), "result: {result}");
        assert!(result.contains("Global Token Metrics"));
        assert!(result.contains("0"));
    }

    #[tokio::test]
    async fn user_metrics_no_data() {
        let tm = TokenMonitor::default();
        let result =
            get_token_metrics(&tm, Some("999"), Some("all_time"), Some("total_tokens")).await;
        assert!(result.contains("No token usage found"));
    }

    #[tokio::test]
    async fn test_fmt() {
        assert_eq!(fmt(0u64), "0");
        assert_eq!(fmt(100u64), "100");
        assert_eq!(fmt(1000u64), "1,000");
        assert_eq!(fmt(1000000u64), "1,000,000");
    }

    #[test]
    fn definition_has_proper_shape() {
        let d = definition();
        assert_eq!(d["name"], "get_token_metrics");
        let props = &d["input_schema"]["properties"];
        assert!(props.get("user_id").is_some());
        assert!(props.get("period").is_some());
        assert!(props.get("metric").is_some());
    }

    #[test]
    fn parse_period_defaults_to_all_time() {
        assert_eq!(parse_period(None), LeaderboardPeriod::AllTime);
        assert_eq!(parse_period(Some("invalid")), LeaderboardPeriod::AllTime);
        assert_eq!(parse_period(Some("daily")), LeaderboardPeriod::Daily);
    }

    #[test]
    fn parse_metric_defaults_to_total_tokens() {
        assert_eq!(parse_metric(None), LeaderboardMetric::TotalTokens);
        assert_eq!(
            parse_metric(Some("cache_efficiency")),
            LeaderboardMetric::CacheEfficiency
        );
    }

    #[test]
    fn sanitize_escapes_discord_markdown() {
        assert_eq!(sanitize("hello_world"), "hello\\_world");
        assert_eq!(sanitize("normal"), "normal");
    }
}
