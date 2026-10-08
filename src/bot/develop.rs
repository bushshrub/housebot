//! Development-flow dispatch, owner approval, and component builders.

use super::*;

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
            "**Feature development: {title}**\n\nAgent: **{}**\nChoose a model:",
            CodingAgent::OpenCode.display_name()
        );
        let components =
            develop_model_components(&job_id.to_string(), CodingAgent::OpenCode, &self.catalog);
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
            )
        });
        let Some((title, objective, req_name, req_id, req_channel, agent, model)) = job_info else {
            tracing::warn!(target: "housebot::develop", %job_id, "Job not found when notifying owner");
            return;
        };

        let agent_str = agent
            .map(|a| a.display_name().to_string())
            .unwrap_or_else(|| "default".into());
        let model_str = model.as_deref().unwrap_or("default");

        let dm_content = format!(
            "**Feature-development request from <@{req_id}>** (`{req_name}`)\n\
             **Feature:** {title}\n\
             **Objective:**\n> {obj}\n\
             **Proposed configuration:**\n\
             Agent: {agent_str} | Model: `{model_str}`\n\
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
}

// ── develop flow component builders ──────────────────────────────────────────

pub(crate) fn develop_approval_components(job_id: &str) -> Vec<CreateActionRow> {
    vec![CreateActionRow::Buttons(vec![
        CreateButton::new(format!("{DEVELOP_PREFIX}{job_id}:approve"))
            .label("Start work")
            .style(ButtonStyle::Success),
        CreateButton::new(format!("{DEVELOP_PREFIX}{job_id}:configure"))
            .label("Change configuration")
            .style(ButtonStyle::Secondary),
        CreateButton::new(format!("{DEVELOP_PREFIX}{job_id}:reject"))
            .label("Reject")
            .style(ButtonStyle::Danger),
    ])]
}

pub(crate) fn develop_model_components(
    job_id: &str,
    agent: CodingAgent,
    catalog: &AgentCatalog,
) -> Vec<CreateActionRow> {
    let models = catalog.models_for(agent);
    let options: Vec<CreateSelectMenuOption> = models
        .iter()
        .map(|m| {
            let mut opt = CreateSelectMenuOption::new(&m.display_name, &m.id);
            if let Some(desc) = &m.description {
                opt = opt.description(desc.chars().take(100).collect::<String>());
            }
            opt
        })
        .collect();
    vec![
        CreateActionRow::SelectMenu(
            CreateSelectMenu::new(
                format!("{DEVELOP_PREFIX}{job_id}:model"),
                CreateSelectMenuKind::String { options },
            )
            .placeholder("Select model"),
        ),
        CreateActionRow::Buttons(vec![CreateButton::new(format!(
            "{DEVELOP_PREFIX}{job_id}:cancel"
        ))
        .label("Cancel")
        .style(ButtonStyle::Danger)]),
    ]
}

pub(crate) fn develop_confirm_components(job_id: &str) -> Vec<CreateActionRow> {
    vec![CreateActionRow::Buttons(vec![
        CreateButton::new(format!("{DEVELOP_PREFIX}{job_id}:confirm"))
            .label("Dispatch")
            .style(ButtonStyle::Success),
        CreateButton::new(format!("{DEVELOP_PREFIX}{job_id}:back"))
            .label("← Change Model")
            .style(ButtonStyle::Secondary),
        CreateButton::new(format!("{DEVELOP_PREFIX}{job_id}:cancel"))
            .label("Cancel")
            .style(ButtonStyle::Danger),
    ])]
}
