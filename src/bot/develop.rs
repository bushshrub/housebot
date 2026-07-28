//! Development-flow dispatch, owner approval, and component builders.

use super::*;

pub(crate) use super::develop_components::*;

impl HouseBot {
    pub(crate) async fn start_develop_flow(&self, ctx: &Context, msg: &Message, job_id: Uuid) {
        let title = self
            .pending_jobs
            .with_job(job_id, |j| j.specification.title.clone());
        let Some(title) = title else {
            let _ = reply_no_ping(ctx, msg, "Error: Development job not found.").await;
            return;
        };
        let content = format!(
            "**Feature development: {title}**\n\nChoose a coding agent to implement this feature:"
        );
        let components = develop_agent_components(&job_id.to_string());
        let builder = CreateMessage::new()
            .content(content)
            .components(components)
            .reference_message(msg)
            .allowed_mentions(CreateAllowedMentions::new());
        if let Ok(sent) = msg.channel_id.send_message(&ctx.http, builder).await {
            self.pending_jobs.with_job_mut(job_id, |j| {
                j.approval_message = Some(DiscordMessageRef {
                    channel_id: sent.channel_id.get(),
                    message_id: sent.id.get(),
                });
            });
        }
    }

    /// DM the configured owner about a non-owner approval request.
    pub(crate) async fn notify_owner_for_approval(
        &self,
        ctx: &Context,
        requester_msg: &Message,
        job_id: Uuid,
    ) {
        let owner_id = config::owner_id();
        if owner_id == 0 {
            tracing::warn!(target: "housebot::develop", "Cannot notify owner: OWNER_DISCORD_ID not set");
            return;
        }

        let job_info = self.pending_jobs.with_job(job_id, |j| {
            (
                j.specification.title.clone(),
                j.specification.objective.clone(),
                j.requester.username.clone(),
                j.requester.user_id,
                j.requester.channel_id,
                j.selection.agent,
                j.selection.model.clone(),
                j.selection.effort.clone(),
            )
        });
        let Some((title, objective, req_name, req_id, req_channel, agent, model, effort)) =
            job_info
        else {
            tracing::warn!(target: "housebot::develop", %job_id, "Job not found when notifying owner");
            return;
        };

        let agent_str = agent
            .map(|a| a.display_name().to_string())
            .unwrap_or_else(|| "default".into());
        let model_str = model.as_deref().unwrap_or("default");
        let effort_str = effort.as_deref().unwrap_or("default");

        let dm_content = format!(
            "**Feature-development request from <@{req_id}>** (`{req_name}`)\n\
             **Feature:** {title}\n\
             **Objective:**\n> {obj}\n\
             **Proposed configuration:**\n\
             Agent: {agent_str} | Model: `{model_str}` | Effort: `{effort_str}`\n\
             **Origin:** <#{req_channel}>",
            obj = objective.lines().collect::<Vec<_>>().join("\n> "),
        );

        let id_str = job_id.to_string();
        let components = develop_approval_components(&id_str);

        let send_dm = async {
            let owner_user = UserId::new(owner_id).to_user(&ctx.http).await?;
            let dm = owner_user.create_dm_channel(&ctx.http).await?;
            let builder = CreateMessage::new()
                .content(&dm_content)
                .components(components.clone());
            dm.send_message(&ctx.http, builder).await
        };

        match send_dm.await {
            Ok(sent) => {
                self.pending_jobs.with_job_mut(job_id, |j| {
                    j.approval_message = Some(DiscordMessageRef {
                        channel_id: sent.channel_id.get(),
                        message_id: sent.id.get(),
                    });
                });
                tracing::info!(
                    target: "housebot::develop",
                    %job_id,
                    requester_id = req_id,
                    "Owner DM sent for approval"
                );
            }
            Err(e) => {
                tracing::error!(
                    target: "housebot::develop",
                    %job_id,
                    error = %e,
                    "Failed to DM owner for approval"
                );
                // Try fallback channel.
                let fallback =
                    crate::config::env_parse::<u64>("DEVELOPMENT_APPROVAL_CHANNEL_ID", 0);
                if fallback != 0 {
                    let fb_channel = serenity::all::ChannelId::new(fallback);
                    let builder = CreateMessage::new()
                        .content(&dm_content)
                        .components(components);
                    if let Ok(sent) = fb_channel.send_message(&ctx.http, builder).await {
                        self.pending_jobs.with_job_mut(job_id, |j| {
                            j.approval_message = Some(DiscordMessageRef {
                                channel_id: sent.channel_id.get(),
                                message_id: sent.id.get(),
                            });
                        });
                        tracing::info!(
                            target: "housebot::develop",
                            %job_id,
                            "Approval card sent to fallback channel"
                        );
                        return;
                    }
                }
                // Both DM and fallback failed — cancel the job so it doesn't accumulate invisibly.
                self.pending_jobs.cancel(job_id);
                self.respond(
                    ctx,
                    requester_msg,
                    "I prepared the request, but I could not contact the owner for approval.",
                )
                .await;
            }
        }
    }

    /// Watch the configured dev-notify channel (`/config dev_notify_channel`) for
    /// the completion webhook posted by `claude-dispatch.yml`/`opencode-dispatch.yml`,
    /// and DM the requester encoded in the embed footer.
    ///
    /// Returns `true` if the message was in the configured channel (and so should
    /// not fall through to normal message handling), regardless of whether a DM
    /// was actually sent.
    pub(crate) async fn handle_dev_notify_webhook(&self, ctx: &Context, msg: &Message) -> bool {
        let notify_channel = self.access.load().await.dev_notify_channel_id;
        if notify_channel != Some(msg.channel_id.get()) {
            return false;
        }
        let Some((requester_id, issue_number, status, sig)) = msg
            .embeds
            .first()
            .and_then(|e| e.footer.as_ref())
            .and_then(|f| parse_dev_notify_footer(&f.text))
        else {
            return true;
        };
        let signing_key = config::env_or("DEV_NOTIFY_SIGNING_KEY", "");
        if signing_key.is_empty() {
            tracing::warn!(
                target: "housebot::develop",
                "DEV_NOTIFY_SIGNING_KEY is not configured — ignoring unverifiable dev-notify webhook"
            );
            return true;
        }
        if !crate::coding_agent::dev_notify::verify(
            signing_key.as_bytes(),
            requester_id,
            issue_number,
            &status,
            &sig,
        ) {
            tracing::warn!(
                target: "housebot::develop",
                channel_id = msg.channel_id.get(),
                "Dev-notify webhook signature verification failed — ignoring"
            );
            return true;
        }
        let emoji = if status == "success" { "✅" } else { "❌" };
        let content = self.redactor.redact(&format!(
            "{emoji} Feature development for issue #{issue_number} finished (`{status}`)."
        ));
        let Ok(user) = UserId::new(requester_id).to_user(&ctx.http).await else {
            return true;
        };
        let Ok(dm) = user.create_dm_channel(&ctx.http).await else {
            return true;
        };
        let _ = dm.say(&ctx.http, content).await;
        true
    }

    /// Handle a Discord component interaction for the develop flow.
    pub(crate) async fn handle_pagination_component(
        &self,
        ctx: &Context,
        component: &serenity::all::ComponentInteraction,
    ) {
        let Some(rest) = component.data.custom_id.strip_prefix(PAGINATION_PREFIX) else {
            return;
        };
        let Some((token, page)) = rest.rsplit_once(':') else {
            return;
        };
        let Ok(page) = page.parse::<usize>() else {
            return;
        };
        let response = self
            .paginated
            .lock()
            .await
            .get(token)
            .map(|response| (response.owner_id, response.pages.clone()));
        let Some((owner_id, pages)) = response else {
            let _ = component
                .create_response(
                    &ctx.http,
                    CreateInteractionResponse::Message(
                        CreateInteractionResponseMessage::new()
                            .content("This paginated response has expired.")
                            .ephemeral(true),
                    ),
                )
                .await;
            return;
        };
        if owner_id != component.user.id.get() || page >= pages.len() {
            let _ = component
                .create_response(
                    &ctx.http,
                    CreateInteractionResponse::Message(
                        CreateInteractionResponseMessage::new()
                            .content("Only the response author can use these buttons.")
                            .ephemeral(true),
                    ),
                )
                .await;
            return;
        }
        let response = CreateInteractionResponse::UpdateMessage(
            CreateInteractionResponseMessage::new()
                .embed(pagination_embed(&pages, page))
                .components(pagination_components(token, page, pages.len())),
        );
        let _ = component.create_response(&ctx.http, response).await;
    }
}

/// Parse the `housebot-dev-notify requester_id=<id> issue=<n> status=<s> sig=<hex>`
/// footer text posted by the dispatch workflows' completion-notify step. The
/// `sig` field still needs verifying against `DEV_NOTIFY_SIGNING_KEY` — this only
/// extracts the fields.
pub(crate) fn parse_dev_notify_footer(text: &str) -> Option<(u64, u64, String, String)> {
    let rest = text.strip_prefix("housebot-dev-notify ")?;
    let mut requester_id = None;
    let mut issue = None;
    let mut status = None;
    let mut sig = None;
    for kv in rest.split_whitespace() {
        let (key, value) = kv.split_once('=')?;
        match key {
            "requester_id" => requester_id = value.parse::<u64>().ok().filter(|id| *id != 0),
            "issue" => issue = value.parse::<u64>().ok(),
            "status" => status = Some(value).filter(|s| !s.is_empty()),
            "sig" => sig = Some(value).filter(|s| !s.is_empty()),
            _ => {}
        }
    }
    Some((requester_id?, issue?, status?.to_string(), sig?.to_string()))
}
