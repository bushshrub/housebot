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

/// Handle `/storage memory ...` and `/storage notes ...` through the same
/// store-backed handlers used by the prefix compatibility aliases.
pub(crate) async fn handle_storage_interaction(
    memory: &Memory,
    notes: &Notes,
    options: &[serenity::all::CommandDataOption],
    author_id: u64,
) -> String {
    let Some(group) = options.first() else {
        return "Use `/storage memory ...` or `/storage notes ...`.".into();
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
        ("notes", "list") => note_command(notes, "!note list", "", author_id).await,
        ("notes", "get" | "delete") => {
            let Some(name) = string_option(action_options, "name") else {
                return "Please provide a note name.".into();
            };
            note_command(
                notes,
                &format!("!note {} {name}", action.name),
                "",
                author_id,
            )
            .await
        }
        ("notes", "save") => {
            let Some(name) = string_option(action_options, "name") else {
                return "Please provide a note name.".into();
            };
            let Some(content) = string_option(action_options, "content") else {
                return "Please provide note content.".into();
            };
            note_command(notes, &format!("!note save {name}"), content, author_id).await
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

pub(crate) fn is_proactive_candidate(content: &str) -> bool {
    let normalized = content.trim().to_ascii_lowercase();
    if normalized.is_empty() {
        return false;
    }
    normalized.contains('?')
        || normalized.starts_with("how ")
        || normalized.starts_with("what ")
        || normalized.starts_with("where ")
        || normalized.starts_with("when ")
        || normalized.starts_with("why ")
        || normalized.starts_with("can you ")
        || normalized.starts_with("could you ")
        || normalized.starts_with("remind me ")
        || normalized.starts_with("how do i ")
        || normalized.starts_with("what can you do")
}

pub(crate) fn compact_done_message(deep_memory_enabled: bool) -> &'static str {
    if deep_memory_enabled {
        "✅ Conversation compacted into memory. A new session has started."
    } else {
        "✅ Conversation cleared without saving a memory summary. A new session has started."
    }
}

pub(crate) fn prefix_session_action(content: &str) -> Option<&'static str> {
    match content {
        "!session" | "!session status" => Some("status"),
        "!new" | "!reset" | "!session new" | "!session reset" => Some("new"),
        "!compact" | "!session compact" => Some("compact"),
        _ => None,
    }
}

pub(crate) fn command_suffix<'a>(first_line: &'a str, command: &str) -> Option<&'a str> {
    if first_line == command {
        Some("")
    } else {
        first_line
            .strip_prefix(command)
            .filter(|suffix| suffix.chars().next().is_some_and(char::is_whitespace))
    }
}

pub(crate) fn normalize_storage_prefix(first_line: &str) -> Option<(&'static str, String)> {
    if let Some(suffix) = command_suffix(first_line, "!storage notes") {
        Some(("notes", format!("!note{suffix}")))
    } else if let Some(suffix) = command_suffix(first_line, "!storage memory") {
        Some(("memory", format!("!memory{suffix}")))
    } else if command_suffix(first_line, "!note").is_some() {
        Some(("notes", first_line.to_string()))
    } else if command_suffix(first_line, "!memory").is_some() {
        Some(("memory", first_line.to_string()))
    } else {
        None
    }
}

pub(crate) fn commit_hash_response(sha: Option<&str>) -> String {
    match sha.filter(|sha| !sha.is_empty()) {
        Some(sha) => format!("Running commit: `{sha}`"),
        None => "Running commit is unavailable for this build.".into(),
    }
}

/// Whether a slash command response should only be visible to its requester.
pub(crate) fn command_response_is_ephemeral(_command_name: &str) -> bool {
    true
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

/// Wrap `/lua` output in a code fence sized to fit a single Discord message.
pub(crate) fn format_lua_reply(output: &str) -> String {
    let sanitized = output.replace("```", "`\u{200b}``");
    let budget = MAX_MESSAGE_LENGTH - "```\n\n```".chars().count();
    let body: String = if sanitized.chars().count() > budget {
        let mut truncated: String = sanitized.chars().take(budget - 1).collect();
        truncated.push('…');
        truncated
    } else {
        sanitized
    };
    format!("```\n{body}\n```")
}

pub(crate) async fn respond_ephemeral(
    ctx: &Context,
    cmd: &serenity::all::CommandInteraction,
    content: &str,
) {
    let response = CreateInteractionResponse::Message(
        CreateInteractionResponseMessage::new()
            .content(content)
            .ephemeral(true),
    );
    if let Err(e) = cmd.create_response(&ctx.http, response).await {
        tracing::warn!("Failed to send interaction response: {e}");
    }
}

/// Scan text for Discord mention patterns (`<@ID>`) and return unique user IDs,
/// excluding the bot's own ID.
pub(crate) fn extract_mentioned_users(text: &str, bot_id: u64) -> Vec<u64> {
    text.split('<')
        .filter_map(|part| {
            let remaining = if let Some(stripped) = part.strip_prefix("@!") {
                stripped
            } else {
                part.strip_prefix('@')?
            };
            let id_str = remaining.split('>').next()?;
            id_str.parse::<u64>().ok()
        })
        .filter(|id| *id != bot_id)
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect()
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
