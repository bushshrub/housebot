//! Feature-request and GitHub tools.

use super::*;

impl Agent {
    pub(super) async fn dispatch_features(
        &self,
        name: &str,
        args: &Value,
        user_id: &str,
        username: &str,
        channel_id: u64,
        guild_id: u64,
    ) -> Option<ToolOutcome> {
        let outcome = match name {
            "github_api" => {
                // Merging is administrator-only; the tools layer re-checks this
                // flag as a defence-in-depth measure and audits every attempt.
                let is_admin = self
                    .access_control
                    .load()
                    .await
                    .is_configurer(user_id.parse::<u64>().unwrap_or(0), config::owner_id());
                let caller = tools::github_api::ToolCaller {
                    user_id,
                    username,
                    is_admin,
                };
                ToolOutcome::Text(
                    tools::github_api::handle_github_api(
                        &self.reporter,
                        str_arg(args, "action"),
                        args,
                        &caller,
                        &self.merge_audit,
                    )
                    .await,
                )
            }
            "create_feature_request" => ToolOutcome::Text(
                tools::feature_request::create_feature_request(
                    &self.reporter,
                    &self.rate_limiter,
                    str_arg(args, "title"),
                    str_arg(args, "description"),
                    str_arg(args, "type"),
                    username,
                    user_id,
                )
                .await,
            ),
            "edit_feature_request" => ToolOutcome::Text(
                tools::edit_feature_request::edit_feature_request(
                    &self.reporter,
                    &self.feature_edit_limiter,
                    u64_arg(args, "issue_number", 0),
                    args.get("title").and_then(Value::as_str),
                    args.get("description").and_then(Value::as_str),
                    user_id,
                )
                .await,
            ),
            "prepare_feature_development" => {
                use crate::coding_agent::pending::{
                    DevelopmentRequester, DiscordMessageRef, PartialAgentSelection,
                };
                use crate::tools::feature_development::{DispatchMode, FeatureDevelopmentOutcome};

                let requirements: Vec<String> = args
                    .get("requirements")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default();
                let acceptance_criteria: Vec<String> = args
                    .get("acceptance_criteria")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default();

                let owner_id = config::owner_id();
                let requester_user_id: u64 = user_id.parse().unwrap_or(0);
                let issue_number = u64_arg(args, "issue_number", 0);
                if issue_number == 0 {
                    return Some(ToolOutcome::Text(
                        "Error: an existing GitHub issue_number is required.".to_string(),
                    ));
                }
                let Some(issue) = self.reporter.fetch_issue(issue_number).await else {
                    return Some(ToolOutcome::Text(format!(
                    "Error: GitHub issue #{issue_number} could not be found in the configured repository."
                )));
                };
                if issue.pull_request.is_some() {
                    return Some(ToolOutcome::Text(format!(
                    "Error: #{issue_number} is a pull request; feature development requires an existing issue."
                )));
                }
                let is_configurer = self
                    .access_control
                    .load()
                    .await
                    .is_configurer(requester_user_id, owner_id);
                let dispatch_mode = if is_configurer {
                    DispatchMode::Interactive
                } else {
                    DispatchMode::RequireOwnerApproval
                };

                let requester = DevelopmentRequester {
                    user_id: requester_user_id,
                    username: username.to_string(),
                    channel_id,
                    guild_id: (guild_id != 0).then_some(guild_id),
                    source_message_id: 0,
                };
                let source_message = DiscordMessageRef {
                    channel_id,
                    message_id: 0,
                };

                // Pre-fill defaults so the owner can dispatch immediately without
                // going through the interactive picker. Read from env vars so the
                // operator can override them; fall back to the opencode free tier.
                let defaults = {
                    use crate::coding_agent::catalog::CodingAgent;
                    use std::str::FromStr;
                    let agent_str = config::env_or("DEVELOPMENT_DEFAULT_AGENT", "opencode");
                    let model = config::env_or(
                        "DEVELOPMENT_DEFAULT_MODEL",
                        "opencode/deepseek-v4-flash-free",
                    );
                    let effort = config::env_or("DEVELOPMENT_DEFAULT_EFFORT", "medium");
                    PartialAgentSelection {
                        agent: CodingAgent::from_str(&agent_str).ok(),
                        model: Some(model),
                        effort: Some(effort),
                    }
                };

                let outcome = tools::feature_development::prepare_feature_development(
                    &self.pending_jobs,
                    &self.non_owner_dev_limiter,
                    owner_id,
                    requester,
                    source_message,
                    issue_number,
                    str_arg(args, "title"),
                    str_arg(args, "objective"),
                    str_arg(args, "context"),
                    requirements,
                    acceptance_criteria,
                    dispatch_mode,
                    &defaults,
                );

                let text = outcome.tool_response();
                let action = match &outcome {
                    FeatureDevelopmentOutcome::OwnerConfigurationRequired { job_id } => {
                        Some(AgentControlAction::OwnerConfigurationRequired { job_id: *job_id })
                    }
                    FeatureDevelopmentOutcome::OwnerApprovalRequired { job_id } => {
                        Some(AgentControlAction::OwnerApprovalRequired { job_id: *job_id })
                    }
                    FeatureDevelopmentOutcome::Rejected { .. } => None,
                };
                if let Some(action) = action {
                    ToolOutcome::DevelopmentAction { text, action }
                } else {
                    ToolOutcome::Text(text)
                }
            }
            _ => return None,
        };
        Some(outcome)
    }
}
