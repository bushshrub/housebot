//! Tool-restore (unban) proposals and voting.

use super::ban_messages::*;
use super::*;

impl HouseBot {
    pub(super) async fn handle_tool_restore_vote(
        &self,
        ctx: &Context,
        cmd: &serenity::all::CommandInteraction,
        author_id: u64,
        guild_id: Option<u64>,
    ) -> String {
        let Some(guild_id) = guild_id else {
            return "Tool-restore voting is only available inside a server.".into();
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

        let proposal_info = permissions
            .find_restore_proposal_by_prefix(guild_id, proposal_str)
            .await
            .unwrap_or(None);

        match permissions
            .vote_restore(guild_id, proposal_str, author_id, approve)
            .await
        {
            Ok(VoteResult::Pending {
                approvals,
                rejections,
                quorum,
            }) => {
                if let Some(ref p) = proposal_info {
                    if p.channel_id != 0 && p.message_id != 0 {
                        let text = self.redactor.redact(&format_restore_proposal_message(
                            p, approvals, rejections, quorum,
                        ));
                        let _ = ChannelId::new(p.channel_id)
                            .edit_message(&ctx.http, p.message_id, EditMessage::new().content(text))
                            .await;
                    }
                }
                format!(
                    "✅ Vote recorded. Current result: **{approvals} approve / {rejections} reject** (minimum {quorum} votes)."
                )
            }
            Ok(VoteResult::RestoreVoted(ref ban)) => {
                if let Some(ref p) = proposal_info {
                    if p.channel_id != 0 && p.message_id != 0 {
                        let text = self.redactor.redact(&format_restore_approved_message(ban));
                        let _ = ChannelId::new(p.channel_id)
                            .edit_message(&ctx.http, p.message_id, EditMessage::new().content(text))
                            .await;
                    }
                }
                format!(
                    "✅ Vote passed. <@{}>'s access to `{}` has been restored.",
                    ban.user_id, ban.tool_name
                )
            }
            Ok(VoteResult::Rejected) => {
                if let Some(ref p) = proposal_info {
                    if p.channel_id != 0 && p.message_id != 0 {
                        let text = self.redactor.redact(&format_restore_rejected_message(p));
                        let _ = ChannelId::new(p.channel_id)
                            .edit_message(&ctx.http, p.message_id, EditMessage::new().content(text))
                            .await;
                    }
                }
                "✅ The proposal was rejected by majority vote.".into()
            }
            Ok(VoteResult::Approved(_)) => "⚠️ Unexpected result from restore vote.".into(),
            Err(error) => format!("⚠️ {error}"),
        }
    }

    /// Handle `/tool_restore propose`: send a visible channel message with emoji
    /// voting reactions, then respond to the interaction ephemerally.
    pub(super) async fn handle_tool_restore_propose(
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
                "Tool-restore voting is only available inside a server.",
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

        let defer = CreateInteractionResponse::Defer(
            CreateInteractionResponseMessage::new().ephemeral(true),
        );
        if let Err(e) = cmd.create_response(&ctx.http, defer).await {
            tracing::warn!("Failed to defer /tool_restore propose response: {e}");
            return;
        }

        let permissions = self.agent.tool_permissions();
        let proposal = match permissions
            .propose_restore(guild_id, target, tool, author_id)
            .await
        {
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
            "🔓 **Restore proposal** by <@{}>\n\
             Target: <@{}>\n\
             Tool: `{}`\n\
             Votes: **{approvals} approve** / **0 reject** (minimum {} votes)\n\
             React with ✅ to approve restore, ❌ to reject (or use `/tool_restore vote`)",
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
                tracing::warn!(%error, "Failed to send restore proposal channel message");
                if let Err(e) = permissions
                    .remove_restore_proposal(guild_id, &proposal.id)
                    .await
                {
                    tracing::error!(%e, "Failed to roll back restore proposal after message send failure");
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

        if let Err(error) = permissions
            .set_restore_proposal_message(
                guild_id,
                &proposal.id,
                cmd.channel_id.get(),
                msg.id.get(),
            )
            .await
        {
            tracing::error!(%error, "Failed to store restore proposal message IDs — deleting posted message");
            let _ = msg.delete(&ctx.http).await;
            if let Err(e) = permissions
                .remove_restore_proposal(guild_id, &proposal.id)
                .await
            {
                tracing::error!(%e, "Failed to roll back restore proposal after message mapping failure");
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

        let short_id = proposal.id.get(..8).unwrap_or(&proposal.id);
        let confirmation = self.redactor.redact(&format!(
            "✅ Restore proposal created! Everyone in the server can see it and vote with reactions. \
             Proposal ID: `{}`. Vote also with `/tool_restore vote proposal:{} approve:true|false`.",
            short_id, short_id,
        ));
        let _ = cmd
            .edit_response(
                &ctx.http,
                EditInteractionResponse::new().content(confirmation),
            )
            .await;
    }
}
