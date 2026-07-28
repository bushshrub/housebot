//! Tool-ban proposals and voting.

use super::ban_messages::*;
use super::*;

impl HouseBot {
    pub(super) async fn handle_tool_ban_autocomplete(
        ctx: &Context,
        autocomplete: &serenity::all::CommandInteraction,
    ) {
        let Some(focused) = autocomplete.data.autocomplete() else {
            return;
        };
        if focused.name != "tool" {
            return;
        }
        let partial = focused.value;
        let lower = partial.to_ascii_lowercase();
        let mut names: Vec<&str> = crate::tools::all_tool_names()
            .iter()
            .filter(|name| name.contains(&lower))
            .copied()
            .take(25)
            .collect();
        names.sort_unstable();
        let mut resp = CreateAutocompleteResponse::new();
        for name in names {
            resp = resp.add_string_choice(name, name);
        }
        let _ = autocomplete
            .create_response(&ctx.http, CreateInteractionResponse::Autocomplete(resp))
            .await;
    }

    /// Handle `/tool_ban vote`: record the vote and update the public proposal message.
    pub(super) async fn handle_tool_ban_vote(
        &self,
        ctx: &Context,
        cmd: &serenity::all::CommandInteraction,
        author_id: u64,
        guild_id: Option<u64>,
    ) -> String {
        let Some(guild_id) = guild_id else {
            return "Tool-ban voting is only available inside a server.".into();
        };
        let Some(option) = cmd.data.options.first() else {
            return "Unexpected option structure.".into();
        };
        let CommandDataOptionValue::SubCommand(options) = &option.value else {
            return "Unexpected option structure.".into();
        };
        let proposal_str = options
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
        let (Some(proposal_str), Some(approve)) = (proposal_str, approve) else {
            return "Please specify a proposal ID and vote.".into();
        };

        let permissions = self.agent.tool_permissions();

        // Look up the proposal *before* voting so we have channel/message IDs
        // even if the vote finalizes and removes the proposal.
        let proposal_info = permissions
            .find_proposal_by_prefix(guild_id, proposal_str)
            .await
            .unwrap_or(None);

        match permissions
            .vote(guild_id, proposal_str, author_id, approve)
            .await
        {
            Ok(VoteResult::Pending {
                approvals,
                rejections,
                quorum,
            }) => {
                // Update the public message if we have channel/message IDs.
                if let Some(ref p) = proposal_info {
                    if p.channel_id != 0 && p.message_id != 0 {
                        let text = self
                            .redactor
                            .redact(&format_proposal_message(p, approvals, rejections, quorum));
                        let _ = ChannelId::new(p.channel_id)
                            .edit_message(&ctx.http, p.message_id, EditMessage::new().content(text))
                            .await;
                    }
                }
                format!(
                    "✅ Vote recorded. Current result: **{approvals} approve / {rejections} reject** (minimum {quorum} votes)."
                )
            }
            Ok(VoteResult::Approved(ref ban)) => {
                // Update the public message if we have channel/message IDs.
                if let Some(ref p) = proposal_info {
                    if p.channel_id != 0 && p.message_id != 0 {
                        let text = self.redactor.redact(&format_approved_message(ban));
                        let _ = ChannelId::new(p.channel_id)
                            .edit_message(&ctx.http, p.message_id, EditMessage::new().content(text))
                            .await;
                    }
                }
                format!(
                    "🚫 Vote passed. <@{}> is now blocked from using `{}` in this server.",
                    ban.user_id, ban.tool_name
                )
            }
            Ok(VoteResult::Rejected) => {
                // Update the public message if we have channel/message IDs.
                if let Some(ref p) = proposal_info {
                    if p.channel_id != 0 && p.message_id != 0 {
                        let text = self.redactor.redact(&format_rejected_message(p));
                        let _ = ChannelId::new(p.channel_id)
                            .edit_message(&ctx.http, p.message_id, EditMessage::new().content(text))
                            .await;
                    }
                }
                "✅ The proposal was rejected by majority vote.".into()
            }
            Ok(VoteResult::RestoreVoted(_)) => "⚠️ Unexpected result from ban vote.".into(),
            Err(error) => format!("⚠️ {error}"),
        }
    }

    /// Handle `/tool_ban propose`: send a visible channel message and add emoji
    /// voting reactions, then respond to the interaction ephemerally.
    pub(super) async fn handle_tool_ban_propose(
        &self,
        ctx: &Context,
        cmd: &serenity::all::CommandInteraction,
        author_id: u64,
        guild_id: Option<u64>,
    ) {
        let Some(guild_id) = guild_id else {
            respond_ephemeral(
                ctx,
                cmd,
                "Tool-ban voting is only available inside a server.",
            )
            .await;
            return;
        };
        let Some(option) = cmd.data.options.first() else {
            respond_ephemeral(ctx, cmd, "Unexpected option structure.").await;
            return;
        };
        let CommandDataOptionValue::SubCommand(options) = &option.value else {
            respond_ephemeral(ctx, cmd, "Unexpected option structure.").await;
            return;
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
            respond_ephemeral(ctx, cmd, "Please specify both a user and tool name.").await;
            return;
        };

        // Defer the interaction so we have time to post the channel message.
        let defer = CreateInteractionResponse::Defer(
            CreateInteractionResponseMessage::new().ephemeral(true),
        );
        if let Err(e) = cmd.create_response(&ctx.http, defer).await {
            tracing::warn!("Failed to defer /tool_ban propose response: {e}");
            return;
        }

        let permissions = self.agent.tool_permissions();
        let proposal = match permissions.propose(guild_id, target, tool, author_id).await {
            Ok(p) => p,
            Err(error) => {
                let _ = cmd
                    .edit_response(
                        &ctx.http,
                        EditInteractionResponse::new().content(format!("⚠️ {error}")),
                    )
                    .await;
                return;
            }
        };

        let (approvals, _) = proposal.vote_counts();
        let text = self.redactor.redact(&format!(
            "🗳️ **Ban proposal** by <@{}>\n\
             Target: <@{}>\n\
             Tool: `{}`\n\
             Votes: **{approvals} approve** / **0 reject** (minimum {} votes)\n\
             React with ✅ to approve, ❌ to reject (or use `/tool_ban vote`)",
            proposal.proposed_by,
            proposal.target_user_id,
            proposal.tool_name,
            permissions.min_votes(),
        ));
        let msg = match cmd
            .channel_id
            .send_message(&ctx.http, CreateMessage::new().content(text))
            .await
        {
            Ok(msg) => msg,
            Err(error) => {
                tracing::warn!(%error, "Failed to send proposal channel message");
                // Roll back the proposal so we don't orphan it.
                if let Err(e) = permissions.remove_proposal(guild_id, &proposal.id).await {
                    tracing::error!(%e, "Failed to roll back proposal after message send failure");
                }
                let _ = cmd
                    .edit_response(
                        &ctx.http,
                        EditInteractionResponse::new()
                            .content("⚠️ Failed to post proposal to channel."),
                    )
                    .await;
                return;
            }
        };

        // Store the message info in the proposal.
        if let Err(error) = permissions
            .set_proposal_message(guild_id, &proposal.id, cmd.channel_id.get(), msg.id.get())
            .await
        {
            tracing::error!(%error, "Failed to store proposal message IDs — deleting posted message");
            // Remove the orphaned message since we can't track it.
            let _ = msg.delete(&ctx.http).await;
            if let Err(e) = permissions.remove_proposal(guild_id, &proposal.id).await {
                tracing::error!(%e, "Failed to roll back proposal after message mapping failure");
            }
            let _ = cmd
                .edit_response(
                    &ctx.http,
                    EditInteractionResponse::new()
                        .content("⚠️ Failed to save proposal metadata. Please try again."),
                )
                .await;
            return;
        }

        // Add voting reactions.
        let _ = msg
            .react(
                &ctx.http,
                serenity::all::ReactionType::Unicode("\u{2705}".to_string()),
            )
            .await;
        let _ = msg
            .react(
                &ctx.http,
                serenity::all::ReactionType::Unicode("\u{274C}".to_string()),
            )
            .await;

        // Edit the deferred response with a confirmation.
        let confirmation = self.redactor.redact(&format!(
            "✅ Proposal created! Everyone in the server can see it and vote with reactions. \
             Proposal ID: `{}`. Vote also with `/tool_ban vote proposal:{} approve:true|false`.",
            &proposal.id[..8],
            &proposal.id[..8],
        ));
        let _ = cmd
            .edit_response(
                &ctx.http,
                EditInteractionResponse::new().content(confirmation),
            )
            .await;
    }
}
