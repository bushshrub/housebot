//! Reaction handling (cancel, retry, and vote reactions).

use super::ban_messages::*;
use super::*;

impl HouseBot {
    pub(super) async fn on_reaction_add(&self, ctx: Context, reaction: serenity::all::Reaction) {
        let user_id = match reaction.user_id {
            Some(id) => id.get(),
            None => return,
        };
        let bot_id = ctx.cache.current_user().id.get();
        if user_id == bot_id {
            return;
        }

        // ── Cancel reaction: ❌ on a progress message ────────────────────
        if let serenity::all::ReactionType::Unicode(e) = &reaction.emoji {
            if e == "❌" {
                let progress = self
                    .progress_messages
                    .lock()
                    .await
                    .get(&reaction.message_id.get())
                    .cloned();
                if let Some((owner_id, cancel_token)) = progress {
                    if owner_id == user_id {
                        cancel_token.cancel();
                        let _ = reaction
                            .channel_id
                            .edit_message(
                                &ctx.http,
                                reaction.message_id,
                                EditMessage::new().content("❌ **Cancelled**"),
                            )
                            .await;
                        let _ = reaction
                            .channel_id
                            .delete_reaction(
                                &ctx.http,
                                reaction.message_id,
                                Some(UserId::new(user_id)),
                                '❌',
                            )
                            .await;
                        return;
                    }
                }
            }
        }

        // ── Emoji echo: when a user reacts to a bot reply, copy the reaction
        //    back to the user's original message.
        //
        //    We do this *before* the tool-ban check so that the message-fetch
        //    is shared: the tool-ban path returns early on non-proposal
        //    messages, which is *after* our echo has already fired.
        if let Ok(message) = reaction
            .channel_id
            .message(&ctx.http, reaction.message_id)
            .await
        {
            if message.author.id.get() == bot_id {
                if let Some(ref referenced) = message.referenced_message {
                    let _ = referenced.react(&ctx.http, reaction.emoji.clone()).await;
                }
            }
        }

        // ── Tool-ban voting ──────────────────────────────────────────────
        let Some(guild_id) = reaction.guild_id.map(|g| g.get()) else {
            return;
        };
        let message_id = reaction.message_id.get();
        let approve = match &reaction.emoji {
            serenity::all::ReactionType::Unicode(e) if e == "\u{2705}" => true,
            serenity::all::ReactionType::Unicode(e) if e == "\u{274C}" => false,
            _ => return,
        };

        let permissions = self.agent.tool_permissions();

        // Check for ban proposals first.
        let found = match permissions.find_by_message(message_id).await {
            Ok(found) => found,
            Err(error) => {
                tracing::error!(%error, %message_id, "Failed to load proposals for reaction vote");
                return;
            }
        };
        if let Some((_id, proposal)) = found {
            if proposal.guild_id != guild_id {
                return;
            }
            match permissions
                .vote(guild_id, &proposal.id, user_id, approve)
                .await
            {
                Ok(VoteResult::Pending {
                    approvals,
                    rejections,
                    quorum,
                }) => {
                    let text = self.redactor.redact(&format_proposal_message(
                        &proposal, approvals, rejections, quorum,
                    ));
                    let _ = ChannelId::new(proposal.channel_id)
                        .edit_message(&ctx.http, message_id, EditMessage::new().content(text))
                        .await;
                }
                Ok(VoteResult::Approved(ref ban)) => {
                    let text = self.redactor.redact(&format_approved_message(ban));
                    let _ = ChannelId::new(proposal.channel_id)
                        .edit_message(&ctx.http, message_id, EditMessage::new().content(text))
                        .await;
                }
                Ok(VoteResult::Rejected) => {
                    let text = self.redactor.redact(&format_rejected_message(&proposal));
                    let _ = ChannelId::new(proposal.channel_id)
                        .edit_message(&ctx.http, message_id, EditMessage::new().content(text))
                        .await;
                }
                Ok(VoteResult::RestoreVoted(_)) => {}
                Err(error) => {
                    tracing::debug!(%error, %user_id, %message_id, "Ban reaction vote failed");
                }
            }
            return;
        }

        // Check for restore proposals.
        let found_restore = match permissions.find_restore_by_message(message_id).await {
            Ok(found) => found,
            Err(error) => {
                tracing::error!(%error, %message_id, "Failed to load restore proposals for reaction vote");
                return;
            }
        };
        let Some((_id, restore)) = found_restore else {
            return;
        };
        if restore.guild_id != guild_id {
            return;
        }
        match permissions
            .vote_restore(guild_id, &restore.id, user_id, approve)
            .await
        {
            Ok(VoteResult::Pending {
                approvals,
                rejections,
                quorum,
            }) => {
                let text = self.redactor.redact(&format_restore_proposal_message(
                    &restore, approvals, rejections, quorum,
                ));
                let _ = ChannelId::new(restore.channel_id)
                    .edit_message(&ctx.http, message_id, EditMessage::new().content(text))
                    .await;
            }
            Ok(VoteResult::RestoreVoted(ref ban)) => {
                let text = self.redactor.redact(&format_restore_approved_message(ban));
                let _ = ChannelId::new(restore.channel_id)
                    .edit_message(&ctx.http, message_id, EditMessage::new().content(text))
                    .await;
            }
            Ok(VoteResult::Rejected) => {
                let text = self
                    .redactor
                    .redact(&format_restore_rejected_message(&restore));
                let _ = ChannelId::new(restore.channel_id)
                    .edit_message(&ctx.http, message_id, EditMessage::new().content(text))
                    .await;
            }
            Ok(_) => {}
            Err(error) => {
                tracing::debug!(%error, %user_id, %message_id, "Restore reaction vote failed");
            }
        }
    }
}
