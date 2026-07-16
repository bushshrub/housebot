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
                        .content("⏳ **Dispatching...** Creating GitHub issue...")
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
        let Some(Some((spec, agent, model, effort, requester_name, requester_user_id))) = job_data
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

        let selection = match self.catalog.validate_selection(agent, &model, &effort) {
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

        let approver_name = component.user.name.clone();
        let approver_id = component.user.id.get();

        let body = match build_issue_body(
            &spec,
            &selection,
            &requester_name,
            requester_user_id,
            &approver_name,
            approver_id,
        ) {
            Ok(b) => b,
            Err(e) => {
                self.pending_jobs.mark_dispatch_failed(job_id);
                let _ = component
                    .edit_response(
                        &ctx.http,
                        EditInteractionResponse::new()
                            .content(format!("❌ Failed to build issue body: {e}")),
                    )
                    .await;
                return;
            }
        };

        let title = format!("[agent:{}] {}", agent.id_str(), spec.title);
        let labels = dispatch_labels(agent);
        let label_refs: Vec<&str> = labels.iter().map(String::as_str).collect();

        // Get the reporter from the agent.
        let reporter = self.agent.reporter();
        match reporter.create_issue_full(&title, &body, &label_refs).await {
            Some(issue) => {
                self.pending_jobs.mark_dispatched(job_id);
                tracing::info!(
                    target: "housebot::develop",
                    issue_number = issue.number,
                    agent = agent.id_str(),
                    "Development job dispatched"
                );
                let triggered = reporter
                    .post_issue_comment(issue.number, DISPATCH_TRIGGER_COMMENT)
                    .await;
                let status = if triggered {
                    "The opencode workflow will pick this up and open a pull request."
                } else {
                    "⚠️ Failed to post the `/oc` trigger comment — comment `/oc` on the issue manually to start the agent."
                };
                let _ = component
                            .edit_response(
                                &ctx.http,
                                EditInteractionResponse::new().content(format!(
                                    "✅ **Dispatched!**\n\
                                     Issue #{num} created: {url}\n\
                                     Agent: **{agent_name}** | Model: `{model}` | Effort: `{effort}`\n\
                                     {status}",
                                    num = issue.number,
                                    url = issue.html_url,
                                    agent_name = agent.display_name(),
                                )),
                            )
                            .await;
            }
            None => {
                self.pending_jobs.mark_dispatch_failed(job_id);
                let _ = component
                            .edit_response(
                                &ctx.http,
                                EditInteractionResponse::new().content(
                                    "❌ Failed to create GitHub issue. Check bot logs for details.\n\
                                     The job has been reset to the confirmation stage — click Dispatch to retry.",
                                ),
                            )
                            .await;
            }
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
                        .content("⏳ **Approving...** Creating GitHub issue...")
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
        let Some(Some((spec, agent, model, effort, req_name, req_id, req_channel))) = job_data
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

        let selection = match self.catalog.validate_selection(agent, &model, &effort) {
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

        let approver_name = component.user.name.clone();
        let approver_id = component.user.id.get();
        let body = match build_issue_body(
            &spec,
            &selection,
            &req_name,
            req_id,
            &approver_name,
            approver_id,
        ) {
            Ok(b) => b,
            Err(e) => {
                self.pending_jobs.mark_dispatch_failed(job_id);
                let _ = component
                    .edit_response(
                        &ctx.http,
                        EditInteractionResponse::new()
                            .content(format!("❌ Failed to build issue body: {e}")),
                    )
                    .await;
                return;
            }
        };

        let title = format!("[agent:{}] {}", agent.id_str(), spec.title);
        let labels = dispatch_labels(agent);
        let label_refs: Vec<&str> = labels.iter().map(String::as_str).collect();
        let reporter = self.agent.reporter();
        match reporter.create_issue_full(&title, &body, &label_refs).await {
            Some(issue) => {
                self.pending_jobs.mark_dispatched(job_id);
                tracing::info!(
                    target: "housebot::develop",
                    issue_number = issue.number,
                    agent = agent.id_str(),
                    "Non-owner development job approved and dispatched"
                );
                let triggered = reporter
                    .post_issue_comment(issue.number, DISPATCH_TRIGGER_COMMENT)
                    .await;
                let status = if triggered {
                    "The opencode workflow will pick this up and open a pull request."
                } else {
                    "⚠️ Failed to post the `/oc` trigger comment — comment `/oc` on the issue manually to start the agent."
                };
                let success_msg = format!(
                    "✅ **Dispatched!**\n\
                             Issue #{num} created: {url}\n\
                             Agent: **{agent_name}** | Model: `{model}` | Effort: `{effort}`\n\
                             {status}",
                    num = issue.number,
                    url = issue.html_url,
                    agent_name = agent.display_name(),
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
                                     Issue: {url}",
                                    agent_name = agent.display_name(),
                                    url = issue.html_url,
                                ),
                            )
                            .await;
            }
            None => {
                self.pending_jobs.mark_dispatch_failed(job_id);
                let _ = component
                            .edit_response(
                                &ctx.http,
                                EditInteractionResponse::new().content(
                                    "❌ Failed to create GitHub issue. The job has been reset — click Start Work to retry.",
                                ),
                            )
                            .await;
            }
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
