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
        // requester's own interactive selection (model/confirm/
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
            "model" => self.develop_on_model(ctx, component, job_id, &id_str).await,
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
        if self.catalog.validate_selection(agent, &model_id).is_err() {
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
            j.stage = DispatchStage::Confirming;
        });
        let content = self
            .pending_jobs
            .with_job(job_id, |j| {
                format!(
                    "**Feature development: {}**\n\n\
                             **Agent:** {}\n\
                             **Model:** {}\n\n\
                             **Objective:**\n{}\n\n\
                             Confirm dispatch to create a GitHub issue and queue the coding job.",
                    j.specification.title,
                    agent.display_name(),
                    model_id,
                    j.specification.objective
                )
            })
            .unwrap_or_default();
        let components = develop_confirm_components(id_str);
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

    pub(crate) async fn develop_on_back(
        &self,
        ctx: &Context,
        component: &serenity::all::ComponentInteraction,
        job_id: Uuid,
        id_str: &str,
    ) {
        // Navigate back one stage.
        let stage = self.pending_jobs.with_job(job_id, |j| j.stage);
        let (content, components) = match stage {
            Some(DispatchStage::Confirming) => {
                let agent = self
                    .pending_jobs
                    .with_job(job_id, |j| j.selection.agent)
                    .flatten()
                    .unwrap_or(CodingAgent::OpenCode);
                self.pending_jobs.with_job_mut(job_id, |j| {
                    j.selection.model = None;
                    j.stage = DispatchStage::ChoosingModel;
                });
                let title = self
                    .pending_jobs
                    .with_job(job_id, |j| j.specification.title.clone())
                    .unwrap_or_default();
                (
                    format!(
                        "**Feature development: {title}**\n\nAgent: **{}**\nChoose a model:",
                        agent.display_name()
                    ),
                    develop_model_components(id_str, agent, &self.catalog),
                )
            }
            _ => return,
        };
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
