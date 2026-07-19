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

    /// Immediately dispatch an owner-direct job without interactive confirmation.
    pub(crate) async fn dispatch_owner_job_immediately(
        &self,
        ctx: &Context,
        msg: &Message,
        job_id: Uuid,
    ) {
        // Atomically transition Confirming → Dispatching.
        if !self.pending_jobs.try_start_dispatch(job_id) {
            let _ = reply_no_ping(
                ctx,
                msg,
                "❌ Failed to dispatch: job is not in a dispatchable state.",
            )
            .await;
            return;
        }

        let job_data = self.pending_jobs.with_job(job_id, |j| {
            let agent = j.selection.agent?;
            let model = j.selection.model.clone()?;
            let effort = j.selection.effort.clone()?;
            Some((
                j.specification.clone(),
                agent,
                model,
                effort,
                j.requester.username.clone(),
                j.requester.user_id,
            ))
        });
        let Some(Some((spec, agent, model, effort, _req_name, _req_id))) = job_data else {
            self.pending_jobs.mark_dispatch_failed(job_id);
            let _ = reply_no_ping(
                ctx,
                msg,
                "❌ Failed to dispatch: incomplete agent/model/effort selection. \
                 Please set DEVELOPMENT_DEFAULT_AGENT, DEVELOPMENT_DEFAULT_MODEL, \
                 and DEVELOPMENT_DEFAULT_EFFORT, or use the interactive flow.",
            )
            .await;
            return;
        };

        let _selection = match self.catalog.validate_selection(agent, &model, &effort) {
            Ok(s) => s,
            Err(e) => {
                self.pending_jobs.mark_dispatch_failed(job_id);
                let _ = reply_no_ping(ctx, msg, &format!("❌ Configuration error: {e}")).await;
                return;
            }
        };

        let reporter = self.agent.reporter();
        let mut inputs = serde_json::Map::new();
        // The workflow_dispatch API rejects non-string input values with 422,
        // even for inputs declared `type: number` in the workflow.
        inputs.insert(
            "issue_number".into(),
            serde_json::Value::String(spec.issue_number.to_string()),
        );
        inputs.insert(
            "prompt".into(),
            serde_json::Value::String(build_dispatch_prompt(spec.issue_number)),
        );
        if reporter
            .trigger_workflow_dispatch(dispatch_workflow_file(agent), "master", &inputs)
            .await
        {
            self.pending_jobs.mark_dispatched(job_id);
            tracing::info!(
                target: "housebot::develop",
                issue_number = spec.issue_number,
                agent = agent.id_str(),
                "Owner-immediate development job dispatched"
            );
            let _ = reply_no_ping(
                ctx,
                msg,
                &format!(
                    "✅ **Dispatched!**\n\
                         Existing issue #{num}\n\
                         Agent: **{agent_name}** | Model: `{model}` | Effort: `{effort}`\n\
                         The `{workflow}` workflow was triggered.",
                    num = spec.issue_number,
                    agent_name = agent.display_name(),
                    workflow = dispatch_workflow_file(agent),
                ),
            )
            .await;
        } else {
            self.pending_jobs.mark_dispatch_failed(job_id);
            let _ = reply_no_ping(
                ctx,
                msg,
                "❌ Failed to trigger the GitHub workflow. Check bot logs for details.",
            )
            .await;
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

pub(crate) fn develop_agent_components(job_id: &str) -> Vec<CreateActionRow> {
    // Discord cannot grey out a single select option, so the disabled state is
    // conveyed via the label/description and enforced in `develop_on_agent`.
    let options = vec![
        CreateSelectMenuOption::new("Claude Code", "claude"),
        CreateSelectMenuOption::new("OpenCode (NVIDIA)", "opencode"),
        CreateSelectMenuOption::new("🚫 Codex (disabled)", "codex")
            .description("Temporarily disabled — cannot be selected"),
    ];
    vec![
        CreateActionRow::SelectMenu(
            CreateSelectMenu::new(
                format!("{DEVELOP_PREFIX}{job_id}:agent"),
                CreateSelectMenuKind::String { options },
            )
            .placeholder("Select coding agent"),
        ),
        CreateActionRow::Buttons(vec![CreateButton::new(format!(
            "{DEVELOP_PREFIX}{job_id}:cancel"
        ))
        .label("Cancel")
        .style(ButtonStyle::Danger)]),
    ]
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
        CreateActionRow::Buttons(vec![
            CreateButton::new(format!("{DEVELOP_PREFIX}{job_id}:back"))
                .label("← Back")
                .style(ButtonStyle::Secondary),
            CreateButton::new(format!("{DEVELOP_PREFIX}{job_id}:cancel"))
                .label("Cancel")
                .style(ButtonStyle::Danger),
        ]),
    ]
}

pub(crate) fn develop_effort_components(
    job_id: &str,
    agent: CodingAgent,
    model: &str,
    catalog: &AgentCatalog,
) -> Vec<CreateActionRow> {
    let efforts = catalog.efforts_for(agent, model).unwrap_or(&[]);
    let options: Vec<CreateSelectMenuOption> = efforts
        .iter()
        .map(|e| {
            let mut opt = CreateSelectMenuOption::new(&e.display_name, &e.id);
            if let Some(desc) = &e.description {
                opt = opt.description(desc.chars().take(100).collect::<String>());
            }
            opt
        })
        .collect();
    vec![
        CreateActionRow::SelectMenu(
            CreateSelectMenu::new(
                format!("{DEVELOP_PREFIX}{job_id}:effort"),
                CreateSelectMenuKind::String { options },
            )
            .placeholder("Select effort level"),
        ),
        CreateActionRow::Buttons(vec![
            CreateButton::new(format!("{DEVELOP_PREFIX}{job_id}:back"))
                .label("← Back")
                .style(ButtonStyle::Secondary),
            CreateButton::new(format!("{DEVELOP_PREFIX}{job_id}:cancel"))
                .label("Cancel")
                .style(ButtonStyle::Danger),
        ]),
    ]
}

pub(crate) fn develop_confirm_components(job_id: &str) -> Vec<CreateActionRow> {
    vec![CreateActionRow::Buttons(vec![
        CreateButton::new(format!("{DEVELOP_PREFIX}{job_id}:confirm"))
            .label("Dispatch")
            .style(ButtonStyle::Success),
        CreateButton::new(format!("{DEVELOP_PREFIX}{job_id}:back"))
            .label("← Change Effort")
            .style(ButtonStyle::Secondary),
        CreateButton::new(format!("{DEVELOP_PREFIX}{job_id}:cancel"))
            .label("Cancel")
            .style(ButtonStyle::Danger),
    ])]
}
