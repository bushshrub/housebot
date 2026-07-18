//! GitHub API tool — native access to issues, workflows, and repository metadata
//! without scraping the web UI.

use serde_json::{json, Value};

use crate::github_issues::GitHubIssueReporter;

/// OpenAI-style tool definition.
pub fn definition() -> Value {
    json!({
        "name": "github_api",
        "description": "Query and manage the GitHub API for issues, workflow runs, and repository metadata in the \
            configured repository (GITHUB_REPO). Used instead of fetch_webpage for this repo's \
            GitHub data because the API provides accurate, structured results. Use this for listing \
            issues, searching issues, viewing issue details, closing issues, managing labels, \
            pruning issues, checking workflow run status, getting repository metadata, and \
            viewing workflow job details.",
        "input_schema": {
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "description": "The GitHub API operation to perform.",
                    "enum": [
                        "list_issues",
                        "search_issues",
                        "get_issue",
                        "close_issue",
                        "add_labels",
                        "remove_labels",
                        "prune_issues",
                        "get_repo",
                        "list_workflows",
                        "list_workflow_runs",
                        "get_workflow_run",
                        "get_workflow_run_jobs"
                    ]
                },
                "state": {
                    "type": "string",
                    "description": "Issue state filter (open, closed, all). Used with list_issues and prune_issues.",
                    "default": "open"
                },
                "labels": {
                    "type": "string",
                    "description": "Comma-separated label filter. Used with list_issues and prune_issues."
                },
                "query": {
                    "type": "string",
                    "description": "Search query for issues. Used with search_issues."
                },
                "issue_number": {
                    "type": "integer",
                    "description": "Issue number. Used with get_issue, close_issue, add_labels, remove_labels."
                },
                "label_names": {
                    "type": "string",
                    "description": "Comma-separated label names to add or remove. Used with add_labels and remove_labels."
                },
                "action_value": {
                    "type": "string",
                    "description": "Value for the prune action (e.g. comma-separated labels for 'label'/'unlabel'). Used with prune_issues."
                },
                "workflow_name": {
                    "type": "string",
                    "description": "Workflow file name (e.g. ci.yml) or numeric ID. Used with list_workflow_runs."
                },
                "branch": {
                    "type": "string",
                    "description": "Filter by branch name. Used with list_workflow_runs."
                },
                "status": {
                    "type": "string",
                    "description": "Filter by run status (queued, in_progress, completed, etc.). Used with list_workflow_runs."
                },
                "event": {
                    "type": "string",
                    "description": "Filter by trigger event (push, pull_request, schedule, etc.). Used with list_workflow_runs."
                },
                "created": {
                    "type": "string",
                    "description": "Filter by created date (e.g. 2024-01-01, >=2024-01-01). Used with list_workflow_runs."
                },
                "run_id": {
                    "type": "integer",
                    "description": "Workflow run ID. Used with get_workflow_run and get_workflow_run_jobs."
                }
            },
            "required": ["action"]
        }
    })
}

/// Dispatch a github_api tool call.
pub async fn handle_github_api(
    reporter: &GitHubIssueReporter,
    action: &str,
    args: &Value,
) -> String {
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
            let action_type = if action.starts_with("close") {
                "close"
            } else if action.starts_with("label:") {
                "label"
            } else if action.starts_with("unlabel:") {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn definition_includes_new_actions() {
        let def = definition();
        let actions = def["input_schema"]["properties"]["action"]["enum"]
            .as_array()
            .expect("actions should be an array");
        let names: Vec<&str> = actions.iter().filter_map(|v| v.as_str()).collect();
        assert!(names.contains(&"get_issue"));
        assert!(names.contains(&"close_issue"));
        assert!(names.contains(&"add_labels"));
        assert!(names.contains(&"remove_labels"));
        assert!(names.contains(&"prune_issues"));
        assert!(names.contains(&"list_issues"));
        assert!(names.contains(&"search_issues"));
    }

    #[tokio::test]
    async fn handle_github_api_returns_not_configured() {
        let reporter =
            GitHubIssueReporter::new(String::new(), String::new(), String::new(), String::new());
        let result = handle_github_api(&reporter, "close_issue", &json!({"issue_number": 1})).await;
        assert!(result.contains("not configured"), "got: {result}");
    }

    #[tokio::test]
    async fn handle_github_api_validation_checks_precede_config_check() {
        // With a configured reporter (using direct token), we can test validation
        let reporter =
            GitHubIssueReporter::with_direct_token("test-token".into(), "owner/repo".into());

        let result = handle_github_api(&reporter, "get_issue", &json!({})).await;
        assert!(result.contains("issue_number is required"), "got: {result}");

        let result = handle_github_api(&reporter, "close_issue", &json!({})).await;
        assert!(result.contains("issue_number is required"), "got: {result}");

        let result = handle_github_api(
            &reporter,
            "add_labels",
            &json!({"issue_number": 1, "label_names": ""}),
        )
        .await;
        assert!(result.contains("label_names is required"), "got: {result}");

        let result = handle_github_api(
            &reporter,
            "prune_issues",
            &json!({"action_value": "invalid_action"}),
        )
        .await;
        assert!(
            result.contains("requires action_value in format"),
            "got: {result}"
        );

        let result = handle_github_api(&reporter, "nonexistent", &json!({})).await;
        assert!(
            result.contains("unknown github_api action"),
            "got: {result}"
        );
    }
}
