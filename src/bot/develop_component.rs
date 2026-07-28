//! Develop component dispatcher and selection actions.

use super::*;

impl HouseBot {
    pub(crate) async fn handle_develop_component(
        &self,
        ctx: &Context,
        component: &serenity::all::ComponentInteraction,
    ) {
        // custom_id format: develop:<job-id>:<action>
        let rest = component
            .data
            .custom_id
            .strip_prefix(DEVELOP_PREFIX)
            .unwrap_or("");
        let Some((id_str, action)) = rest.split_once(':') else {
            return;
        };
        let Ok(job_id) = id_str.parse::<Uuid>() else {
            return;
        };

        let ids = self
            .pending_jobs
            .with_job(job_id, |j| (j.owner_id, j.requester.user_id));
        let Some((owner_id, requester_id)) = ids else {
            let _ = component
                .create_response(
                    &ctx.http,
                    CreateInteractionResponse::Message(
                        CreateInteractionResponseMessage::new()
                            .content(
                                "This development job has expired. Please ask the bot to prepare a new one.",
                            )
                            .ephemeral(true),
                    ),
                )
                .await;
            return;
        };

        // Approval decisions on someone else's request (approve/reject/configure,
        // shown only on AwaitingOwnerApproval cards) are owner-only. The
        // requester's own interactive selection (agent/model/effort/confirm/
        // back/cancel) may be driven by either the owner or the requester.
        let caller = component.user.id.get();
        let owner_only_action = matches!(action, "approve" | "reject" | "configure");
        let authorized = caller == owner_id || (!owner_only_action && caller == requester_id);
        if !authorized {
            let message = if owner_only_action {
                "Only the bot owner can approve, reject, or reconfigure this request."
            } else {
                "Only the bot owner or the requester can use these controls."
            };
            let _ = component
                .create_response(
                    &ctx.http,
                    CreateInteractionResponse::Message(
                        CreateInteractionResponseMessage::new()
                            .content(message)
                            .ephemeral(true),
                    ),
                )
                .await;
            return;
        }

        // Check expiry.
        let expired = self
            .pending_jobs
            .with_job(job_id, |j| j.is_expired())
            .unwrap_or(true);
        if expired {
            let _ = component
                .create_response(
                    &ctx.http,
                    CreateInteractionResponse::Message(
                        CreateInteractionResponseMessage::new()
                            .content(
                                "This development job has expired (15-minute timeout). Please ask the bot to prepare a new one.",
                            )
                            .ephemeral(true),
                    ),
                )
                .await;
            return;
        }

        let id_str = job_id.to_string();
        match action {
            "agent" => self.develop_on_agent(ctx, component, job_id, &id_str).await,
            "model" => self.develop_on_model(ctx, component, job_id, &id_str).await,
            "effort" => {
                self.develop_on_effort(ctx, component, job_id, &id_str)
                    .await
            }
            "confirm" => {
                self.develop_on_confirm(ctx, component, job_id, &id_str)
                    .await
            }
            "approve" => {
                self.develop_on_approve(ctx, component, job_id, &id_str)
                    .await
            }
            "configure" => {
                self.develop_on_configure(ctx, component, job_id, &id_str)
                    .await
            }
            "reject" => {
                self.develop_on_reject(ctx, component, job_id, &id_str)
                    .await
            }
            "back" => self.develop_on_back(ctx, component, job_id, &id_str).await,
            "cancel" => {
                self.develop_on_cancel(ctx, component, job_id, &id_str)
                    .await
            }
            _ => {}
        }
    }

    pub(crate) async fn develop_on_agent(
        &self,
        ctx: &Context,
        component: &serenity::all::ComponentInteraction,
        job_id: Uuid,
        id_str: &str,
    ) {
        // Value from the select menu.
        let selected = match &component.data.kind {
            ComponentInteractionDataKind::StringSelect { values } => values.first().cloned(),
            _ => None,
        };
        let Some(agent_id) = selected else {
            return;
        };
        let Ok(agent) = agent_id.parse::<CodingAgent>() else {
            let _ = component
                .create_response(
                    &ctx.http,
                    CreateInteractionResponse::Message(
                        CreateInteractionResponseMessage::new()
                            .content(format!("Unknown agent: {agent_id}"))
                            .ephemeral(true),
                    ),
                )
                .await;
            return;
        };
        if agent_dispatch_disabled(agent) {
            let _ = component
                .create_response(
                    &ctx.http,
                    CreateInteractionResponse::Message(
                        CreateInteractionResponseMessage::new()
                            .content(AGENT_DISABLED_MESSAGE)
                            .ephemeral(true),
                    ),
                )
                .await;
            return;
        }
        self.pending_jobs.with_job_mut(job_id, |j| {
            j.selection.agent = Some(agent);
            j.selection.model = None;
            j.selection.effort = None;
            j.stage = DispatchStage::ChoosingModel;
        });
        let (title, models_text) = self
            .pending_jobs
            .with_job(job_id, |j| {
                (
                    j.specification.title.clone(),
                    format!(
                        "**Feature development: {}**\n\n\
                                 Agent: **{}**\nChoose a model:",
                        j.specification.title,
                        agent.display_name()
                    ),
                )
            })
            .unwrap_or_default();
        let _ = title;
        let components = develop_model_components(id_str, agent, &self.catalog);
        let _ = component
            .create_response(
                &ctx.http,
                CreateInteractionResponse::UpdateMessage(
                    CreateInteractionResponseMessage::new()
                        .content(models_text)
                        .components(components),
                ),
            )
            .await;
    }

    pub(crate) async fn develop_on_model(
        &self,
        ctx: &Context,
        component: &serenity::all::ComponentInteraction,
        job_id: Uuid,
        id_str: &str,
    ) {
        let selected = match &component.data.kind {
            ComponentInteractionDataKind::StringSelect { values } => values.first().cloned(),
            _ => None,
        };
        let Some(model_id) = selected else {
            return;
        };
        let agent = self
            .pending_jobs
            .with_job(job_id, |j| j.selection.agent)
            .flatten();
        let Some(agent) = agent else {
            return;
        };
        // Validate model against catalog.
        if self.catalog.efforts_for(agent, &model_id).is_none() {
            let _ = component
                .create_response(
                    &ctx.http,
                    CreateInteractionResponse::Message(
                        CreateInteractionResponseMessage::new()
                            .content(format!("Model `{model_id}` is not valid for {agent}."))
                            .ephemeral(true),
                    ),
                )
                .await;
            return;
        }
        self.pending_jobs.with_job_mut(job_id, |j| {
            j.selection.model = Some(model_id.clone());
            j.selection.effort = None;
            j.stage = DispatchStage::ChoosingEffort;
        });
        let content = self
            .pending_jobs
            .with_job(job_id, |j| {
                format!(
                    "**Feature development: {}**\n\n\
                             Agent: **{}**\nModel: **{}**\nChoose effort level:",
                    j.specification.title,
                    agent.display_name(),
                    model_id
                )
            })
            .unwrap_or_default();
        let components = develop_effort_components(id_str, agent, &model_id, &self.catalog);
        let _ = component
            .create_response(
                &ctx.http,
                CreateInteractionResponse::UpdateMessage(
                    CreateInteractionResponseMessage::new()
                        .content(content)
                        .components(components),
                ),
            )
            .await;
    }
}
