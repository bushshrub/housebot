//! GitHub API tool — native access to issues, workflows, and repository metadata
//! without scraping the web UI.

use std::path::PathBuf;

use serde_json::{json, Value};

use housebot_config as config;

pub use crate::github_api_dispatch::*;

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

#[cfg(test)]
#[path = "github_api_tests.rs"]
mod tests;
