//! GitHub API tool — native access to issues, workflows, and repository metadata
//! without scraping the web UI.

use serde_json::{json, Value};

use crate::github_issues::GitHubIssueReporter;

/// OpenAI-style tool definition.
pub fn definition() -> Value {
    json!({
        "name": "github_api",
        "description": "Query the GitHub API for information, issues, and workflow runs in the \
            configured repository (GITHUB_REPO). Used instead of fetch_webpage for this repo's \
            GitHub data because the API provides accurate, structured results. Use this for listing \
            issues, searching issues, checking workflow run status, getting repository metadata, and \
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
                        "get_repo",
                        "list_workflows",
                        "list_workflow_runs",
                        "get_workflow_run",
                        "get_workflow_run_jobs"
                    ]
                },
                "state": {
                    "type": "string",
                    "description": "Issue state filter (open, closed, all). Used with list_issues.",
                    "default": "open"
                },
                "labels": {
                    "type": "string",
                    "description": "Comma-separated label filter. Used with list_issues."
                },
                "query": {
                    "type": "string",
                    "description": "Search query for issues. Used with search_issues."
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
