//! Develop-flow handlers for effort selection and back navigation.

use super::*;

impl HouseBot {
    pub(crate) async fn develop_on_effort(
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
        let Some(effort_id) = selected else {
            return;
        };
        let (agent, model) = self
            .pending_jobs
            .with_job(job_id, |j| (j.selection.agent, j.selection.model.clone()))
            .unwrap_or_default();
        let (Some(agent), Some(model)) = (agent, model) else {
            return;
        };
        // Validate effort.
        if self
            .catalog
            .efforts_for(agent, &model)
            .and_then(|efs| efs.iter().find(|e| e.id == effort_id))
            .is_none()
        {
            let _ = component
                .create_response(
                    &ctx.http,
                    CreateInteractionResponse::Message(
                        CreateInteractionResponseMessage::new()
                            .content(format!(
                                "Effort `{effort_id}` is not valid for model `{model}`."
                            ))
                            .ephemeral(true),
                    ),
                )
                .await;
            return;
        }
        self.pending_jobs.with_job_mut(job_id, |j| {
            j.selection.effort = Some(effort_id.clone());
            j.stage = DispatchStage::Confirming;
        });
        let content = self
            .pending_jobs
            .with_job(job_id, |j| {
                format!(
                    "**Feature development: {}**\n\n\
                             **Agent:** {}\n\
                             **Model:** {}\n\
                             **Effort:** {}\n\n\
                             **Objective:**\n{}\n\n\
                             Confirm dispatch to create a GitHub issue and queue the coding job.",
                    j.specification.title,
                    agent.display_name(),
                    model,
                    effort_id,
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
            Some(DispatchStage::ChoosingModel) => {
                self.pending_jobs.with_job_mut(job_id, |j| {
                    j.selection.agent = None;
                    j.stage = DispatchStage::ChoosingAgent;
                });
                let title = self
                    .pending_jobs
                    .with_job(job_id, |j| j.specification.title.clone())
                    .unwrap_or_default();
                (
                            format!(
                                "**Feature development: {title}**\n\nChoose a coding agent to implement this feature:"
                            ),
                            develop_agent_components(id_str),
                        )
            }
            Some(DispatchStage::ChoosingEffort) => {
                let agent = self
                    .pending_jobs
                    .with_job(job_id, |j| j.selection.agent)
                    .flatten();
                self.pending_jobs.with_job_mut(job_id, |j| {
                    j.selection.model = None;
                    j.stage = DispatchStage::ChoosingModel;
                });
                let (title, agent_name) = self
                    .pending_jobs
                    .with_job(job_id, |j| {
                        (
                            j.specification.title.clone(),
                            j.selection.agent.map(|a| a.display_name().to_string()),
                        )
                    })
                    .unwrap_or_default();
                let agent = agent.unwrap_or(CodingAgent::Claude);
                (
                    format!(
                        "**Feature development: {title}**\n\nAgent: **{}**\nChoose a model:",
                        agent_name.unwrap_or_default()
                    ),
                    develop_model_components(id_str, agent, &self.catalog),
                )
            }
            Some(DispatchStage::Confirming) => {
                let agent_opt = self
                    .pending_jobs
                    .with_job(job_id, |j| j.selection.agent)
                    .flatten();
                let model_opt = self
                    .pending_jobs
                    .with_job(job_id, |j| j.selection.model.clone())
                    .flatten();
                self.pending_jobs.with_job_mut(job_id, |j| {
                    j.selection.effort = None;
                    j.stage = DispatchStage::ChoosingEffort;
                });
                let title = self
                    .pending_jobs
                    .with_job(job_id, |j| j.specification.title.clone())
                    .unwrap_or_default();
                let agent = agent_opt.unwrap_or(CodingAgent::Claude);
                let model = model_opt.unwrap_or_default();
                (
                            format!(
                                "**Feature development: {title}**\n\nAgent: **{}**\nModel: `{model}`\nChoose effort level:",
                                agent.display_name()
                            ),
                            develop_effort_components(id_str, agent, &model, &self.catalog),
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
