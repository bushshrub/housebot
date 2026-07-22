//! Develop confirm/approve/configure/reject/cancel actions.

use super::*;

impl HouseBot {
    pub(crate) async fn develop_on_confirm(
        &self,
        ctx: &Context,
        component: &serenity::all::ComponentInteraction,
        job_id: Uuid,
        _id_str: &str,
    ) {
        // Atomic dispatch: only succeeds once.
        if !self.pending_jobs.try_start_dispatch(job_id) {
            let _ = component
                .create_response(
                    &ctx.http,
                    CreateInteractionResponse::Message(
                        CreateInteractionResponseMessage::new()
                            .content("This job is already being dispatched or has been dispatched.")
                            .ephemeral(true),
                    ),
                )
                .await;
            return;
        }

        // Acknowledge immediately so Discord doesn't timeout.
        let _ = component
            .create_response(
                &ctx.http,
                CreateInteractionResponse::UpdateMessage(
                    CreateInteractionResponseMessage::new()
                        .content("⏳ **Dispatching...** Triggering the GitHub workflow...")
                        .components(vec![]),
                ),
            )
            .await;

        // Gather all needed data — use original requester, not the approver.
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
        let Some(Some((spec, agent, model, effort, _requester_name, requester_user_id))) = job_data
        else {
            self.pending_jobs.mark_dispatch_failed(job_id);
            let _ = component
                .edit_response(
                    &ctx.http,
                    EditInteractionResponse::new().content(
                        "❌ Failed to dispatch: incomplete selection. Please start again.",
                    ),
                )
                .await;
            return;
        };

        let _selection = match self.catalog.validate_selection(agent, &model, &effort) {
            Ok(s) => s,
            Err(e) => {
                self.pending_jobs.mark_dispatch_failed(job_id);
                let _ = component
                    .edit_response(
                        &ctx.http,
                        EditInteractionResponse::new()
                            .content(format!("❌ Configuration error: {e}. Please start again.")),
                    )
                    .await;
                return;
            }
        };

        if agent_dispatch_disabled(agent) {
            self.pending_jobs.mark_dispatch_failed(job_id);
            let _ = component
                .edit_response(
                    &ctx.http,
                    EditInteractionResponse::new().content(format!("❌ {AGENT_DISABLED_MESSAGE}")),
                )
                .await;
            return;
        }

        // Get the reporter from the agent.
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
        inputs.insert(
            "requester_id".into(),
            serde_json::Value::String(requester_user_id.to_string()),
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
                "Development job dispatched"
            );
            let _ = component
                            .edit_response(
                                &ctx.http,
                                EditInteractionResponse::new().content(format!(
                                    "✅ **Dispatched!**\n\
                                     Existing issue #{num}\n\
                                     Agent: **{agent_name}** | Model: `{model}` | Effort: `{effort}`\n\
                                     The `{workflow}` workflow was triggered.",
                                    num = spec.issue_number,
                                    agent_name = agent.display_name(),
                                    workflow = dispatch_workflow_file(agent),
                                )),
                            )
                            .await;
        } else {
            self.pending_jobs.mark_dispatch_failed(job_id);
            let _ = component
                .edit_response(
                    &ctx.http,
                    EditInteractionResponse::new().content(
                        "❌ Failed to trigger the GitHub workflow. The job has been reset — click Dispatch to retry.",
                    ),
                )
                .await;
        }
    }

    pub(crate) async fn develop_on_approve(
        &self,
        ctx: &Context,
        component: &serenity::all::ComponentInteraction,
        job_id: Uuid,
        _id_str: &str,
    ) {
        // Owner approves a non-owner request with default selection.
        if !self.pending_jobs.try_approve_with_defaults(job_id) {
            let _ = component
                        .create_response(
                            &ctx.http,
                            CreateInteractionResponse::Message(
                                CreateInteractionResponseMessage::new()
                                    .content(
                                        "This request cannot be approved now (wrong stage, expired, or selection incomplete).",
                                    )
                                    .ephemeral(true),
                            ),
                        )
                        .await;
            return;
        }

        let _ = component
            .create_response(
                &ctx.http,
                CreateInteractionResponse::UpdateMessage(
                    CreateInteractionResponseMessage::new()
                        .content("⏳ **Approving...** Triggering the GitHub workflow...")
                        .components(vec![]),
                ),
            )
            .await;

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
                j.requester.channel_id,
            ))
        });
        let Some(Some((spec, agent, model, effort, _req_name, req_id, req_channel))) = job_data
        else {
            self.pending_jobs.mark_dispatch_failed(job_id);
            let _ = component
                .edit_response(
                    &ctx.http,
                    EditInteractionResponse::new().content("❌ Failed: incomplete selection."),
                )
                .await;
            return;
        };

        let _selection = match self.catalog.validate_selection(agent, &model, &effort) {
            Ok(s) => s,
            Err(e) => {
                self.pending_jobs.mark_dispatch_failed(job_id);
                let _ = component
                    .edit_response(
                        &ctx.http,
                        EditInteractionResponse::new()
                            .content(format!("❌ Configuration error: {e}")),
                    )
                    .await;
                return;
            }
        };

        if agent_dispatch_disabled(agent) {
            self.pending_jobs.mark_dispatch_failed(job_id);
            let _ = component
                .edit_response(
                    &ctx.http,
                    EditInteractionResponse::new().content(format!("❌ {AGENT_DISABLED_MESSAGE}")),
                )
                .await;
            return;
        }

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
        inputs.insert(
            "requester_id".into(),
            serde_json::Value::String(req_id.to_string()),
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
                "Non-owner development job approved and dispatched"
            );
            let success_msg = format!(
                "✅ **Dispatched!**\n\
                             Existing issue #{num}\n\
                             Agent: **{agent_name}** | Model: `{model}` | Effort: `{effort}`\n\
                             The `{workflow}` workflow was triggered.",
                num = spec.issue_number,
                agent_name = agent.display_name(),
                workflow = dispatch_workflow_file(agent),
            );
            let _ = component
                .edit_response(
                    &ctx.http,
                    EditInteractionResponse::new().content(&success_msg),
                )
                .await;
            // Notify original requester.
            let channel = serenity::all::ChannelId::new(req_channel);
            let _ = channel
                            .say(
                                &ctx.http,
                                format!(
                                    "✅ <@{req_id}> The bot owner approved your development request. \
                                     Development has started using {agent_name}, `{model}`, `{effort}`.\n\
                                     Existing issue: #{issue_number}\n\
                                     The `{workflow}` workflow was triggered.",
                                    agent_name = agent.display_name(),
                                    issue_number = spec.issue_number,
                                    workflow = dispatch_workflow_file(agent),
                                ),
                            )
                            .await;
        } else {
            self.pending_jobs.mark_dispatch_failed(job_id);
            let _ = component
                            .edit_response(
                                &ctx.http,
                                EditInteractionResponse::new().content(
                                    "❌ Failed to trigger the GitHub workflow. The job has been reset — click Start Work to retry.",
                                ),
                            )
                            .await;
        }
    }

    pub(crate) async fn develop_on_configure(
        &self,
        ctx: &Context,
        component: &serenity::all::ComponentInteraction,
        job_id: Uuid,
        id_str: &str,
    ) {
        // Owner wants to change agent/model/effort before approving.
        if !self.pending_jobs.try_begin_configuration(job_id) {
            let _ = component
                .create_response(
                    &ctx.http,
                    CreateInteractionResponse::Message(
                        CreateInteractionResponseMessage::new()
                            .content("Cannot begin configuration from the current state.")
                            .ephemeral(true),
                    ),
                )
                .await;
            return;
        }
        let title = self
            .pending_jobs
            .with_job(job_id, |j| j.specification.title.clone())
            .unwrap_or_default();
        let content = format!(
            "**Feature development: {title}**\n\nChoose a coding agent to implement this feature:"
        );
        let components = develop_agent_components(id_str);
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

    pub(crate) async fn develop_on_reject(
        &self,
        ctx: &Context,
        component: &serenity::all::ComponentInteraction,
        job_id: Uuid,
        _id_str: &str,
    ) {
        let req_channel = self
            .pending_jobs
            .with_job(job_id, |j| (j.requester.channel_id, j.requester.user_id));
        if !self.pending_jobs.try_reject(job_id) {
            let _ = component
                .create_response(
                    &ctx.http,
                    CreateInteractionResponse::Message(
                        CreateInteractionResponseMessage::new()
                            .content("This request is no longer active.")
                            .ephemeral(true),
                    ),
                )
                .await;
            return;
        }
        let _ = component
            .create_response(
                &ctx.http,
                CreateInteractionResponse::UpdateMessage(
                    CreateInteractionResponseMessage::new()
                        .content("❌ Request rejected.")
                        .components(vec![]),
                ),
            )
            .await;
        // Notify requester.
        if let Some((channel_id, requester_id)) = req_channel {
            let channel = serenity::all::ChannelId::new(channel_id);
            let _ = channel
                        .say(
                            &ctx.http,
                            format!(
                                "<@{requester_id}> Your automated development request was not approved by the bot owner."
                            ),
                        )
                        .await;
        }
    }

    pub(crate) async fn develop_on_cancel(
        &self,
        ctx: &Context,
        component: &serenity::all::ComponentInteraction,
        job_id: Uuid,
        _id_str: &str,
    ) {
        self.pending_jobs.cancel(job_id);
        let _ = component
            .create_response(
                &ctx.http,
                CreateInteractionResponse::UpdateMessage(
                    CreateInteractionResponseMessage::new()
                        .content("❌ Development job cancelled.")
                        .components(vec![]),
                ),
            )
            .await;
    }
}
