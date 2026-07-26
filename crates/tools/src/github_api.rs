//! GitHub API tool — native access to issues, workflows, and repository metadata
//! without scraping the web UI.

use std::path::PathBuf;

use serde_json::{json, Value};

use housebot_config as config;
use housebot_github_issues::{GitHubIssueReporter, MergeOutcome, MERGE_METHODS};

/// Identity of the user on whose behalf a github_api call is made. Privileged
/// actions (merging pull requests) are refused unless `is_admin` is set.
pub struct ToolCaller<'a> {
    pub user_id: &'a str,
    pub username: &'a str,
    pub is_admin: bool,
}

/// Append-only audit trail of pull-request merge attempts, authorized or not.
#[derive(Clone)]
pub struct MergeAuditLog {
    path: PathBuf,
}

impl Default for MergeAuditLog {
    fn default() -> Self {
        Self::new(config::data_dir().join("pr_merge_audit.jsonl"))
    }
}

impl MergeAuditLog {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// Append one merge attempt. Failures are logged but never block the merge
    /// result from reaching the caller.
    pub async fn record(
        &self,
        caller: &ToolCaller<'_>,
        pull_number: u64,
        authorized: bool,
        result: &str,
        detail: &str,
    ) {
        let entry = json!({
            "timestamp": chrono::Utc::now().to_rfc3339(),
            "admin_id": caller.user_id,
            "admin_username": caller.username,
            "pull_request": pull_number,
            "authorized": authorized,
            "result": result,
            "detail": detail,
        });
        tracing::info!(
            target: "housebot::audit",
            admin_id = caller.user_id,
            admin_username = caller.username,
            pull_request = pull_number,
            authorized,
            result,
            "pull request merge attempt"
        );
        if let Err(error) = self.append(&entry).await {
            tracing::error!(%error, path = %self.path.display(), "failed to write merge audit record");
        }
    }

    async fn append(&self, entry: &Value) -> std::io::Result<()> {
        use tokio::io::AsyncWriteExt;

        if let Some(parent) = self.path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let mut line = serde_json::to_string(entry).map_err(std::io::Error::other)?;
        line.push('\n');
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .await?;
        file.write_all(line.as_bytes()).await
    }
}

/// OpenAI-style tool definition.
pub fn definition() -> Value {
    json!({
        "name": "github_api",
        "description": "Query and manage the GitHub API for issues, workflow runs, and repository metadata in the \
            configured repository (GITHUB_REPO). Used instead of fetch_webpage for this repo's \
            GitHub data because the API provides accurate, structured results. Use this for listing \
            issues, searching issues, viewing issue details, closing issues, managing labels, \
            pruning issues, checking workflow run status, getting repository metadata, and \
            viewing workflow job details. The merge_pull_request action merges a pull request and \
            is restricted to bot administrators; every attempt is recorded in an audit log.",
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
                        "get_workflow_run_jobs",
                        "merge_pull_request"
                    ]
                },
                "pull_request_number": {
                    "type": "integer",
                    "description": "Pull request number to merge. Used with merge_pull_request."
                },
                "merge_method": {
                    "type": "string",
                    "description": "How to merge the pull request (merge, squash, rebase). Used with merge_pull_request.",
                    "enum": ["merge", "squash", "rebase"],
                    "default": "merge"
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

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn admin() -> ToolCaller<'static> {
        ToolCaller {
            user_id: "196556976866459648",
            username: "derp_z",
            is_admin: true,
        }
    }

    fn member() -> ToolCaller<'static> {
        ToolCaller {
            user_id: "42",
            username: "someone",
            is_admin: false,
        }
    }

    fn audit_log() -> (TempDir, MergeAuditLog) {
        let temp = TempDir::new().unwrap();
        let log = MergeAuditLog::new(temp.path().join("audit").join("pr_merge_audit.jsonl"));
        (temp, log)
    }

    async fn audit_entries(temp: &TempDir) -> Vec<Value> {
        let path = temp.path().join("audit").join("pr_merge_audit.jsonl");
        let raw = tokio::fs::read_to_string(path).await.unwrap_or_default();
        raw.lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| serde_json::from_str(line).expect("audit lines are JSON"))
            .collect()
    }

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
        let (_temp, audit) = audit_log();
        let result = handle_github_api(
            &reporter,
            "close_issue",
            &json!({"issue_number": 1}),
            &admin(),
            &audit,
        )
        .await;
        assert!(result.contains("not configured"), "got: {result}");
    }

    #[tokio::test]
    async fn handle_github_api_validation_checks_precede_config_check() {
        // With a configured reporter (using direct token), we can test validation
        let reporter =
            GitHubIssueReporter::with_direct_token("test-token".into(), "owner/repo".into());

        let (_temp, audit) = audit_log();
        let result = handle_github_api(&reporter, "get_issue", &json!({}), &admin(), &audit).await;
        assert!(result.contains("issue_number is required"), "got: {result}");

        let result =
            handle_github_api(&reporter, "close_issue", &json!({}), &admin(), &audit).await;
        assert!(result.contains("issue_number is required"), "got: {result}");

        let result = handle_github_api(
            &reporter,
            "add_labels",
            &json!({"issue_number": 1, "label_names": ""}),
            &admin(),
            &audit,
        )
        .await;
        assert!(result.contains("label_names is required"), "got: {result}");

        let result = handle_github_api(
            &reporter,
            "prune_issues",
            &json!({"action_value": "invalid_action"}),
            &admin(),
            &audit,
        )
        .await;
        assert!(
            result.contains("requires action_value in format"),
            "got: {result}"
        );

        let result =
            handle_github_api(&reporter, "nonexistent", &json!({}), &admin(), &audit).await;
        assert!(
            result.contains("unknown github_api action"),
            "got: {result}"
        );
    }

    #[test]
    fn definition_exposes_merge_pull_request() {
        let def = definition();
        let properties = &def["input_schema"]["properties"];
        let actions: Vec<&str> = properties["action"]["enum"]
            .as_array()
            .expect("actions should be an array")
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        assert!(actions.contains(&"merge_pull_request"));
        assert_eq!(properties["pull_request_number"]["type"], "integer");
        let methods: Vec<&str> = properties["merge_method"]["enum"]
            .as_array()
            .expect("merge methods should be an array")
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        assert_eq!(methods, vec!["merge", "squash", "rebase"]);
    }

    #[tokio::test]
    async fn merge_is_refused_for_non_administrators_and_audited() {
        let reporter =
            GitHubIssueReporter::with_direct_token("test-token".into(), "owner/repo".into());
        let (temp, audit) = audit_log();

        let result = handle_github_api(
            &reporter,
            "merge_pull_request",
            &json!({"pull_request_number": 7}),
            &member(),
            &audit,
        )
        .await;

        assert!(result.contains("permission denied"), "got: {result}");
        let entries = audit_entries(&temp).await;
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0]["admin_id"], "42");
        assert_eq!(entries[0]["admin_username"], "someone");
        assert_eq!(entries[0]["pull_request"], 7);
        assert_eq!(entries[0]["authorized"], false);
        assert_eq!(entries[0]["result"], "denied");
        assert!(entries[0]["timestamp"]
            .as_str()
            .is_some_and(|ts| ts.contains('T')));
    }

    #[tokio::test]
    async fn merge_validates_arguments_for_administrators() {
        let reporter =
            GitHubIssueReporter::with_direct_token("test-token".into(), "owner/repo".into());
        let (temp, audit) = audit_log();

        let result = handle_github_api(
            &reporter,
            "merge_pull_request",
            &json!({}),
            &admin(),
            &audit,
        )
        .await;
        assert!(
            result.contains("pull_request_number is required"),
            "got: {result}"
        );

        let result = handle_github_api(
            &reporter,
            "merge_pull_request",
            &json!({"pull_request_number": 7, "merge_method": "fast-forward"}),
            &admin(),
            &audit,
        )
        .await;
        assert!(
            result.contains("merge_method must be one of"),
            "got: {result}"
        );

        // Rejected arguments never reach GitHub, so nothing is audited as an attempt.
        assert!(audit_entries(&temp).await.is_empty());
    }

    #[tokio::test]
    async fn merge_reports_unconfigured_github_and_audits_the_attempt() {
        let reporter =
            GitHubIssueReporter::new(String::new(), String::new(), String::new(), String::new());
        let (temp, audit) = audit_log();

        let result = handle_github_api(
            &reporter,
            "merge_pull_request",
            &json!({"pull_request_number": 12}),
            &admin(),
            &audit,
        )
        .await;

        assert!(result.starts_with("Error:"), "got: {result}");
        assert!(result.contains("not configured"), "got: {result}");
        let entries = audit_entries(&temp).await;
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0]["admin_id"], "196556976866459648");
        assert_eq!(entries[0]["pull_request"], 12);
        assert_eq!(entries[0]["authorized"], true);
        assert_eq!(entries[0]["result"], "error");
    }

    #[tokio::test]
    async fn audit_log_appends_successive_entries() {
        let (temp, audit) = audit_log();
        audit.record(&admin(), 1, true, "success", "merged").await;
        audit
            .record(&member(), 2, false, "denied", "not admin")
            .await;
        let entries = audit_entries(&temp).await;
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0]["pull_request"], 1);
        assert_eq!(entries[1]["pull_request"], 2);
    }
}
