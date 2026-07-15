//! Rendering the token leaderboard for Discord.

use super::*;

pub(crate) fn format_token_leaderboard(leaderboard: &TokenLeaderboard) -> String {
    if leaderboard.users.is_empty() {
        return format!(
            "🏆 **{} token leaderboard**\nNo token usage has been recorded for this timeframe.",
            leaderboard.period.label()
        );
    }
    let icon = match leaderboard.period {
        LeaderboardPeriod::Daily => "☀️",
        LeaderboardPeriod::Weekly => "📅",
        LeaderboardPeriod::Monthly => "🗓️",
        LeaderboardPeriod::AllTime => "🏆",
    };
    let mut lines = vec![format!(
        "{icon} **{} token leaderboard**\n*Ranked by {}*",
        leaderboard.period.label(),
        leaderboard.metric.label().to_lowercase()
    )];
    for (index, entry) in leaderboard.users.iter().enumerate() {
        lines.push(format!(
            "`{:>2}.` **{}** — {} · {} conversation{}",
            index + 1,
            format_leaderboard_label(&entry.label),
            format_leaderboard_metric(entry, leaderboard.metric),
            entry.conversations,
            if entry.conversations == 1 { "" } else { "s" }
        ));
    }

    if let Some(rank) = &leaderboard.requester_rank {
        lines.push(format!(
            "\n👤 **Your rank:** #{} — {}",
            rank.position,
            format_leaderboard_metric(&rank.entry, leaderboard.metric)
        ));
    }

    lines.push("\n**Top conversations**".to_string());
    for (index, entry) in leaderboard.conversations.iter().take(5).enumerate() {
        let id = entry
            .conversation_id
            .as_deref()
            .unwrap_or("unknown")
            .chars()
            .take(8)
            .collect::<String>();
        lines.push(format!(
            "`{:>2}.` **{}** (`{id}`) — {}",
            index + 1,
            format_leaderboard_label(&entry.label),
            format_leaderboard_metric(entry, leaderboard.metric)
        ));
    }
    lines.join("\n")
}

pub(crate) fn format_leaderboard_label(label: &str) -> String {
    label
        .replace('\\', "\\\\")
        .replace('*', "\\*")
        .replace('_', "\\_")
        .replace('`', "\\`")
        .replace('~', "\\~")
        .replace('|', "\\|")
}

pub(crate) fn format_leaderboard_metric(
    entry: &LeaderboardEntry,
    metric: LeaderboardMetric,
) -> String {
    match metric {
        LeaderboardMetric::TotalTokens => format!("{} tokens", entry.total_tokens()),
        LeaderboardMetric::CacheEfficiency => format!(
            "{:.1}% cache efficiency ({} tokens)",
            entry.cache_efficiency(),
            entry.total_tokens()
        ),
    }
}
