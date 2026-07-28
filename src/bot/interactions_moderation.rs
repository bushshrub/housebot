//! Interactions moderation.

//! Slash-command interaction handlers (effort, tool bans, status, data, privacy, skill, stats).

use super::*;

/// Handle guild-scoped `/tool_ban` proposals, votes, and status requests.
pub(crate) async fn handle_tool_ban_interaction(
    permissions: &ToolPermissions,
    options: &[serenity::all::CommandDataOption],
    author_id: u64,
    guild_id: Option<u64>,
) -> String {
    let Some(guild_id) = guild_id else {
        return "Tool-ban voting is only available inside a server.".into();
    };
    let Some(command) = options.first() else {
        return "Choose `propose`, `vote`, or `status`.".into();
    };
    match command.name.as_str() {
        "propose" => {
            let CommandDataOptionValue::SubCommand(options) = &command.value else {
                return "Unexpected option structure.".into();
            };
            let target = options
                .iter()
                .find(|option| option.name == "user")
                .and_then(|option| match option.value {
                    CommandDataOptionValue::User(user) => Some(user.get()),
                    _ => None,
                });
            let tool = options
                .iter()
                .find(|option| option.name == "tool")
                .and_then(|option| match &option.value {
                    CommandDataOptionValue::String(tool) => Some(tool.as_str()),
                    _ => None,
                });
            let (Some(target), Some(tool)) = (target, tool) else {
                return "Please specify both a user and tool name.".into();
            };
            match permissions.propose(guild_id, target, tool, author_id).await {
                Ok(proposal) => format!(
                    "🗳️ Proposed banning <@{}> from `{}`. Proposal `{}` is open for 24 hours.\nVote with `/tool_ban vote proposal:{} approve:true|false`. The proposal needs at least {} votes; your approval was recorded automatically.",
                    proposal.target_user_id,
                    proposal.tool_name,
                    &proposal.id[..8],
                    &proposal.id[..8],
                    permissions.min_votes()
                ),
                Err(error) => format!("⚠️ {error}"),
            }
        }
        "vote" => {
            let CommandDataOptionValue::SubCommand(options) = &command.value else {
                return "Unexpected option structure.".into();
            };
            let proposal = options
                .iter()
                .find(|option| option.name == "proposal")
                .and_then(|option| match &option.value {
                    CommandDataOptionValue::String(id) => Some(id.as_str()),
                    _ => None,
                });
            let approve = options
                .iter()
                .find(|option| option.name == "approve")
                .and_then(|option| match option.value {
                    CommandDataOptionValue::Boolean(approve) => Some(approve),
                    _ => None,
                });
            let (Some(proposal), Some(approve)) = (proposal, approve) else {
                return "Please specify a proposal ID and vote.".into();
            };
            match permissions.vote(guild_id, proposal, author_id, approve).await {
                Ok(VoteResult::Pending {
                    approvals,
                    rejections,
                    quorum,
                }) => format!(
                    "✅ Vote recorded. Current result: **{approvals} approve / {rejections} reject** (minimum {quorum} votes)."
                ),
                Ok(VoteResult::Approved(ban)) => format!(
                    "🚫 Vote passed. <@{}> is now blocked from using `{}` in this server.",
                    ban.user_id, ban.tool_name
                ),
                Ok(VoteResult::Rejected) => {
                    "✅ The proposal was rejected by majority vote.".into()
                }
                Ok(VoteResult::RestoreVoted(_)) => {
                    "⚠️ Unexpected result from ban vote.".into()
                }
                Err(error) => format!("⚠️ {error}"),
            }
        }
        "status" => {
            let status = match permissions.status(guild_id).await {
                Ok(status) => status,
                Err(error) => {
                    tracing::error!(%error, %guild_id, "failed to load tool permission status");
                    return "⚠️ Tool permission status is temporarily unavailable.".into();
                }
            };
            if status.bans.is_empty() && status.proposals.is_empty() {
                return "No active tool bans or open proposals in this server.".into();
            }
            let mut lines = vec!["**Tool permissions**".to_string()];
            if !status.bans.is_empty() {
                lines.push("**Active bans**".into());
                for ban in status.bans.iter().take(10) {
                    lines.push(format!("• <@{}> — `{}`", ban.user_id, ban.tool_name));
                }
            }
            if !status.proposals.is_empty() {
                lines.push("**Open proposals**".into());
                for proposal in status.proposals.iter().take(10) {
                    let (approvals, rejections) = proposal.vote_counts();
                    lines.push(format!(
                        "• `{}`: <@{}> / `{}` — {approvals} approve, {rejections} reject",
                        &proposal.id[..8],
                        proposal.target_user_id,
                        proposal.tool_name
                    ));
                }
            }
            lines.join("\n")
        }
        other => format!("Unknown tool-ban option `{other}`."),
    }
}

/// Handle guild-scoped `/tool_restore` proposals, votes, and status requests.
pub(crate) async fn handle_tool_restore_interaction(
    permissions: &ToolPermissions,
    options: &[serenity::all::CommandDataOption],
    author_id: u64,
    guild_id: Option<u64>,
) -> String {
    let Some(guild_id) = guild_id else {
        return "Tool-restore voting is only available inside a server.".into();
    };
    let Some(command) = options.first() else {
        return "Choose `propose`, `vote`, or `status`.".into();
    };
    match command.name.as_str() {
        "propose" => {
            let CommandDataOptionValue::SubCommand(options) = &command.value else {
                return "Unexpected option structure.".into();
            };
            let target = options
                .iter()
                .find(|option| option.name == "user")
                .and_then(|option| match option.value {
                    CommandDataOptionValue::User(user) => Some(user.get()),
                    _ => None,
                });
            let tool = options
                .iter()
                .find(|option| option.name == "tool")
                .and_then(|option| match &option.value {
                    CommandDataOptionValue::String(tool) => Some(tool.as_str()),
                    _ => None,
                });
            let (Some(target), Some(tool)) = (target, tool) else {
                return "Please specify both a user and tool name.".into();
            };
            match permissions.propose_restore(guild_id, target, tool, author_id).await {
                Ok(proposal) => format!(
                    "🗳️ Proposed restoring `{}` access for <@{}>. Proposal `{}` is open for 24 hours.\nVote with `/tool_restore vote proposal:{} approve:true|false`. The proposal needs at least {} votes; your approval was recorded automatically.",
                    proposal.tool_name,
                    proposal.target_user_id,
                    &proposal.id[..8],
                    &proposal.id[..8],
                    permissions.min_votes()
                ),
                Err(error) => format!("⚠️ {error}"),
            }
        }
        "vote" => {
            let CommandDataOptionValue::SubCommand(options) = &command.value else {
                return "Unexpected option structure.".into();
            };
            let proposal = options
                .iter()
                .find(|option| option.name == "proposal")
                .and_then(|option| match &option.value {
                    CommandDataOptionValue::String(id) => Some(id.as_str()),
                    _ => None,
                });
            let approve = options
                .iter()
                .find(|option| option.name == "approve")
                .and_then(|option| match option.value {
                    CommandDataOptionValue::Boolean(approve) => Some(approve),
                    _ => None,
                });
            let (Some(proposal), Some(approve)) = (proposal, approve) else {
                return "Please specify a proposal ID and vote.".into();
            };
            match permissions.vote_restore(guild_id, proposal, author_id, approve).await {
                Ok(VoteResult::Pending {
                    approvals,
                    rejections,
                    quorum,
                }) => format!(
                    "✅ Vote recorded. Current result: **{approvals} approve / {rejections} reject** (minimum {quorum} votes)."
                ),
                Ok(VoteResult::RestoreVoted(ban)) => format!(
                    "✅ Vote passed. <@{}>'s access to `{}` has been restored.",
                    ban.user_id, ban.tool_name
                ),
                Ok(VoteResult::Rejected) => {
                    "✅ The proposal was rejected by majority vote.".into()
                }
                Ok(VoteResult::Approved(_)) => {
                    "⚠️ Unexpected result from restore vote.".into()
                }
                Err(error) => format!("⚠️ {error}"),
            }
        }
        "status" => {
            let status = match permissions.status(guild_id).await {
                Ok(status) => status,
                Err(error) => {
                    tracing::error!(%error, %guild_id, "failed to load tool permission status");
                    return "⚠️ Tool permission status is temporarily unavailable.".into();
                }
            };
            if status.bans.is_empty()
                && status.proposals.is_empty()
                && status.restore_proposals.is_empty()
            {
                return "No active tool bans or open proposals in this server.".into();
            }
            let mut lines = vec!["**Tool permissions**".to_string()];
            if !status.bans.is_empty() {
                lines.push("**Active bans**".into());
                for ban in status.bans.iter().take(10) {
                    lines.push(format!("• <@{}> — `{}`", ban.user_id, ban.tool_name));
                }
            }
            if !status.proposals.is_empty() {
                lines.push("**Open ban proposals**".into());
                for proposal in status.proposals.iter().take(10) {
                    let (approvals, rejections) = proposal.vote_counts();
                    lines.push(format!(
                        "• `{}`: <@{}> / `{}` — {approvals} approve, {rejections} reject",
                        &proposal.id[..8],
                        proposal.target_user_id,
                        proposal.tool_name
                    ));
                }
            }
            if !status.restore_proposals.is_empty() {
                lines.push("**Open restore proposals**".into());
                for p in status.restore_proposals.iter().take(10) {
                    let (approvals, rejections) = p.vote_counts();
                    lines.push(format!(
                        "• `{}`: <@{}> / `{}` — {approvals} approve, {rejections} reject",
                        &p.id[..8],
                        p.target_user_id,
                        p.tool_name
                    ));
                }
            }
            lines.join("\n")
        }
        other => format!("Unknown tool-restore option `{other}`."),
    }
}
