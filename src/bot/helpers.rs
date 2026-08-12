//! Small pure helpers and the storage interaction handler.

use super::*;

pub(crate) fn truncate_memory_reply(header: &str, body: &str) -> String {
    const LIMIT: usize = MAX_MESSAGE_LENGTH;
    const ELLIPSIS: &str = "\n…(truncated)";
    let full = format!("{header}{body}");
    if full.chars().count() <= LIMIT {
        return full;
    }
    let keep = LIMIT.saturating_sub(ELLIPSIS.chars().count());
    format!("{}{ELLIPSIS}", full.chars().take(keep).collect::<String>())
}

pub(crate) fn nested_options(
    option: &serenity::all::CommandDataOption,
) -> Option<&[serenity::all::CommandDataOption]> {
    match &option.value {
        serenity::all::CommandDataOptionValue::SubCommand(options)
        | serenity::all::CommandDataOptionValue::SubCommandGroup(options) => Some(options),
        _ => None,
    }
}

pub(crate) fn string_option<'a>(
    options: &'a [serenity::all::CommandDataOption],
    name: &str,
) -> Option<&'a str> {
    options
        .iter()
        .find(|option| option.name == name)
        .and_then(|option| match &option.value {
            serenity::all::CommandDataOptionValue::String(value) => Some(value.as_str()),
            _ => None,
        })
}

pub(crate) fn bool_option(
    options: &[serenity::all::CommandDataOption],
    name: &str,
) -> Option<bool> {
    options
        .iter()
        .find(|option| option.name == name)
        .and_then(|option| match option.value {
            serenity::all::CommandDataOptionValue::Boolean(value) => Some(value),
            _ => None,
        })
}

/// Handle `/storage memory ...` through the same store-backed handlers used by
/// the prefix compatibility aliases.
pub(crate) async fn handle_storage_interaction(
    memory: &Memory,
    options: &[serenity::all::CommandDataOption],
    author_id: u64,
) -> String {
    let Some(group) = options.first() else {
        return "Use `/storage memory ...`.".into();
    };
    let Some(actions) = nested_options(group) else {
        return "Unexpected storage command structure.".into();
    };
    let Some(action) = actions.first() else {
        return "Choose a storage action.".into();
    };
    let action_options = nested_options(action).unwrap_or_default();

    match (group.name.as_str(), action.name.as_str()) {
        ("memory", "show" | "clear") => {
            let command = format!("!memory {}", action.name);
            memory_command(memory, &command, author_id).await
        }
        ("memory", "search") => {
            let Some(query) = string_option(action_options, "query") else {
                return "Please provide a search query.".into();
            };
            memory_command(memory, &format!("!memory search {query}"), author_id).await
        }
        _ => "Unknown storage action.".into(),
    }
}

pub(crate) async fn reply_no_ping(
    ctx: &Context,
    msg: &Message,
    content: &str,
) -> serenity::Result<Message> {
    let builder = CreateMessage::new()
        .content(content)
        .reference_message(msg)
        .allowed_mentions(CreateAllowedMentions::new());
    msg.channel_id.send_message(&ctx.http, builder).await
}

pub(crate) async fn reply_with_mentions(
    ctx: &Context,
    msg: &Message,
    content: &str,
    allowed_users: &[u64],
) -> serenity::Result<Message> {
    let mut mentions = CreateAllowedMentions::new();
    if !allowed_users.is_empty() {
        mentions = mentions.users(allowed_users.iter().map(|id| UserId::new(*id)));
    }
    let builder = CreateMessage::new()
        .content(content)
        .reference_message(msg)
        .allowed_mentions(mentions);
    msg.channel_id.send_message(&ctx.http, builder).await
}

pub(crate) fn help_response() -> String {
    crate::tools::features::features_text().to_string()
}

pub(crate) fn commit_hash_response(sha: Option<&str>) -> String {
    match sha.filter(|sha| !sha.is_empty()) {
        Some(sha) => format!("Running commit: `{sha}`"),
        None => "Running commit is unavailable for this build.".into(),
    }
}

/// Whether a slash command response should only be visible to its requester.
pub(crate) fn command_response_is_ephemeral(command_name: &str) -> bool {
    !matches!(command_name, "session" | "stats")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LeaderboardAccess {
    Public,
    Private,
    Denied,
}

pub(crate) fn leaderboard_access(
    config: &ServerConfig,
    in_guild: bool,
    member_roles: &[u64],
    is_admin: bool,
) -> LeaderboardAccess {
    if !in_guild {
        return LeaderboardAccess::Private;
    }
    match config.leaderboard_visibility {
        LeaderboardVisibility::Public => LeaderboardAccess::Public,
        LeaderboardVisibility::Private => LeaderboardAccess::Private,
        LeaderboardVisibility::Restricted
            if is_admin
                || member_roles
                    .iter()
                    .any(|role| config.leaderboard_role_ids.contains(role)) =>
        {
            LeaderboardAccess::Private
        }
        LeaderboardVisibility::Restricted => LeaderboardAccess::Denied,
    }
}

pub(crate) fn leaderboard_options(
    options: &[serenity::all::CommandDataOption],
) -> (LeaderboardPeriod, LeaderboardMetric) {
    let string_option = |name| {
        options
            .iter()
            .find(|option| option.name == name)
            .and_then(|option| match &option.value {
                CommandDataOptionValue::String(value) => Some(value.as_str()),
                _ => None,
            })
    };
    let period = match string_option("timeframe") {
        Some("daily") => LeaderboardPeriod::Daily,
        Some("weekly") => LeaderboardPeriod::Weekly,
        Some("monthly") => LeaderboardPeriod::Monthly,
        _ => LeaderboardPeriod::AllTime,
    };
    let metric = match string_option("metric") {
        Some("efficiency") => LeaderboardMetric::CacheEfficiency,
        _ => LeaderboardMetric::TotalTokens,
    };
    (period, metric)
}

/// Scan text for Discord mention patterns (`<@ID>`) and return unique user IDs,
/// excluding the bot's own ID.
pub(crate) fn extract_mentioned_users(text: &str, bot_id: u64) -> Vec<u64> {
    mentioned_user_ids(text)
        .filter(|id| *id != bot_id)
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect()
}

/// Check raw message content for a specific Discord user mention. This is a
/// fallback for connector-originated events that omit the structured mentions
/// array while preserving the canonical `<@ID>` token in message content.
pub(crate) fn content_mentions_user(text: &str, user_id: u64) -> bool {
    mentioned_user_ids(text).any(|id| id == user_id)
}

fn mentioned_user_ids(text: &str) -> impl Iterator<Item = u64> + '_ {
    text.split('<').filter_map(|part| {
        let remaining = if let Some(stripped) = part.strip_prefix("@!") {
            stripped
        } else {
            part.strip_prefix('@')?
        };
        let (id_str, _) = remaining.split_once('>')?;
        id_str.parse::<u64>().ok()
    })
}

pub(crate) const RETIRED_SLASH_COMMANDS: &[&str] = &[
    "new",
    "reset",
    "compact",
    "memory",
    "history",
    "profile",
    "erase_my_data",
];
