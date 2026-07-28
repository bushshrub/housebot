//! Github api dispatch.

use serde_json::Value;

use crate::github_api::*;
use housebot_github_issues::{GitHubIssueReporter, MergeOutcome, MERGE_METHODS};

/// Dispatch a github_api tool call.
pub async fn handle_github_api(
    reporter: &GitHubIssueReporter,
    action: &str,
    args: &Value,
    caller: &ToolCaller<'_>,
    audit: &MergeAuditLog,
) -> String {
    if action == "merge_pull_request" {
        return merge_pull_request(reporter, args, caller, audit).await;
    }
    if !reporter.is_configured() {
        return "Error: GitHub integration is not configured — the github_api tool requires GITHUB_APP_ID, \
            GITHUB_APP_PRIVATE_KEY, GITHUB_INSTALLATION_ID, and GITHUB_REPO to be set."
            .to_string();
    }

    match action {
        "list_issues" => {
            let state = args.get("state").and_then(Value::as_str).unwrap_or("open");
            let labels = args.get("labels").and_then(Value::as_str).unwrap_or("");
            reporter.list_issues(state, labels).await
        }
        "search_issues" => {
            let query = args.get("query").and_then(Value::as_str).unwrap_or("");
            if query.is_empty() {
                return "Error: query is required for search_issues.".to_string();
            }
            reporter.search_issues(query).await
        }
        "get_issue" => {
            let issue_number = args
                .get("issue_number")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            if issue_number == 0 {
                return "Error: issue_number is required for get_issue.".to_string();
            }
            reporter
                .get_issue_detail(issue_number)
                .await
                .unwrap_or_else(|| "Error: failed to fetch issue detail.".to_string())
        }
        "close_issue" => {
            let issue_number = args
                .get("issue_number")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            if issue_number == 0 {
                return "Error: issue_number is required for close_issue.".to_string();
            }
            if reporter.close_issue(issue_number).await {
                format!("Issue #{issue_number} closed successfully.")
            } else {
                format!("Error: failed to close issue #{issue_number}.")
            }
        }
        "add_labels" => {
            let issue_number = args
                .get("issue_number")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            let label_names = args
                .get("label_names")
                .and_then(Value::as_str)
                .unwrap_or("");
            if issue_number == 0 {
                return "Error: issue_number is required for add_labels.".to_string();
            }
            if label_names.is_empty() {
                return "Error: label_names is required for add_labels.".to_string();
            }
            let labels: Vec<&str> = label_names
                .split(',')
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .collect();
            if reporter.add_labels(issue_number, &labels).await {
                format!(
                    "Labels [{}] added to issue #{issue_number}.",
                    labels.join(", ")
                )
            } else {
                format!("Error: failed to add labels to issue #{issue_number}.")
            }
        }
        "remove_labels" => {
            let issue_number = args
                .get("issue_number")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            let label_names = args
                .get("label_names")
                .and_then(Value::as_str)
                .unwrap_or("");
            if issue_number == 0 {
                return "Error: issue_number is required for remove_labels.".to_string();
            }
            if label_names.is_empty() {
                return "Error: label_names is required for remove_labels.".to_string();
            }
            let labels: Vec<&str> = label_names
                .split(',')
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .collect();
            if reporter.remove_labels(issue_number, &labels).await {
                format!(
                    "Labels [{}] removed from issue #{issue_number}.",
                    labels.join(", ")
                )
            } else {
                format!("Error: failed to remove labels from issue #{issue_number}.")
            }
        }
        "prune_issues" => {
            let state = args.get("state").and_then(Value::as_str).unwrap_or("open");
            let labels = args.get("labels").and_then(Value::as_str).unwrap_or("");
            let action = args
                .get("action_value")
                .and_then(Value::as_str)
                .unwrap_or("");
            let action_type = if action == "close" {
                "close"
            } else if let Some(labels) = action.strip_prefix("label:") {
                if labels.is_empty() {
                    return "Error: prune_issues with 'label:' requires at least one label name."
                        .to_string();
                }
                "label"
            } else if let Some(labels) = action.strip_prefix("unlabel:") {
                if labels.is_empty() {
                    return "Error: prune_issues with 'unlabel:' requires at least one label name."
                        .to_string();
                }
                "unlabel"
            } else {
                return "Error: prune_issues requires action_value in format: 'close', 'label:name1,name2', or 'unlabel:name1,name2'.".to_string();
            };
            let action_value = if action_type == "close" {
                ""
            } else {
                action
                    .strip_prefix(&format!("{}:", action_type))
                    .unwrap_or("")
            };
            reporter
                .prune_issues(state, labels, action_type, action_value)
                .await
        }
        "get_repo" => reporter.get_repo().await,
        "list_workflows" => reporter.list_workflows().await,
        "list_workflow_runs" => {
            let workflow_name = args
                .get("workflow_name")
                .and_then(Value::as_str)
                .unwrap_or("");
            let branch = args.get("branch").and_then(Value::as_str).unwrap_or("");
            let status = args.get("status").and_then(Value::as_str).unwrap_or("");
            let event = args.get("event").and_then(Value::as_str).unwrap_or("");
            let created = args.get("created").and_then(Value::as_str).unwrap_or("");
            reporter
                .list_workflow_runs(workflow_name, branch, status, event, created)
                .await
        }
        "get_workflow_run" => {
            let run_id = args.get("run_id").and_then(Value::as_u64).unwrap_or(0);
            if run_id == 0 {
                return "Error: run_id is required for get_workflow_run.".to_string();
            }
            reporter.get_workflow_run(run_id).await
        }
        "get_workflow_run_jobs" => {
            let run_id = args.get("run_id").and_then(Value::as_u64).unwrap_or(0);
            if run_id == 0 {
                return "Error: run_id is required for get_workflow_run_jobs.".to_string();
            }
            reporter.get_workflow_run_jobs(run_id).await
        }
        _ => format!("Error: unknown github_api action — {action}"),
    }
}

/// Merge a pull request on behalf of an administrator, auditing every attempt.
async fn merge_pull_request(
    reporter: &GitHubIssueReporter,
    args: &Value,
    caller: &ToolCaller<'_>,
    audit: &MergeAuditLog,
) -> String {
    let pull_number = args
        .get("pull_request_number")
        .and_then(Value::as_u64)
        .unwrap_or(0);

    if !caller.is_admin {
        audit
            .record(
                caller,
                pull_number,
                false,
                "denied",
                "caller is not a bot administrator",
            )
            .await;
        return "Error: permission denied — only bot administrators can merge pull requests."
            .to_string();
    }
    if pull_number == 0 {
        return "Error: pull_request_number is required for merge_pull_request.".to_string();
    }
    let merge_method = args
        .get("merge_method")
        .and_then(Value::as_str)
        .unwrap_or("merge");
    if !MERGE_METHODS.contains(&merge_method) {
        return format!(
            "Error: merge_method must be one of {}.",
            MERGE_METHODS.join(", ")
        );
    }

    let outcome = reporter.merge_pull_request(pull_number, merge_method).await;
    let response = match &outcome {
        MergeOutcome::Merged { sha, message } => format!(
            "Success: pull request #{pull_number} merged with `{merge_method}` (commit `{sha}`) — {message}"
        ),
        MergeOutcome::Conflict(detail) => format!(
            "Conflict: pull request #{pull_number} could not be merged — {detail}. Resolve the conflict and try again."
        ),
        MergeOutcome::Blocked(detail) => format!(
            "Error: GitHub refused to merge pull request #{pull_number} — {detail}. It may be a draft, or required checks or reviews are outstanding."
        ),
        MergeOutcome::NotPermitted(detail) => format!(
            "Error: the bot lacks permission to merge in this repository — {detail}."
        ),
        MergeOutcome::NotFound(detail) => format!(
            "Error: pull request #{pull_number} was not found in the configured repository — {detail}."
        ),
        MergeOutcome::Error(detail) => {
            format!("Error: merging pull request #{pull_number} failed — {detail}")
        }
    };
    audit
        .record(caller, pull_number, true, outcome.status(), &response)
        .await;
    response
}
