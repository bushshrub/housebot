//! GitHub Actions: workflow dispatch, run listing, and job inspection.

use serde_json::json;

use crate::{urlencoding, GitHubIssueReporter};

impl GitHubIssueReporter {
    /// Trigger a workflow_dispatch event on the configured repository.
    /// Returns `true` if the dispatch was successfully requested, `false` otherwise.
    pub async fn trigger_workflow_dispatch(
        &self,
        workflow_file_name: &str,
        ref_branch: &str,
        inputs: &serde_json::Map<String, serde_json::Value>,
    ) -> bool {
        if !self.is_configured() {
            return false;
        }
        match self
            .try_trigger_workflow_dispatch(workflow_file_name, ref_branch, inputs)
            .await
        {
            Ok(()) => true,
            Err(e) => {
                tracing::error!(workflow = %workflow_file_name, "Failed to trigger workflow_dispatch: {e}");
                false
            }
        }
    }

    async fn try_trigger_workflow_dispatch(
        &self,
        workflow_file_name: &str,
        ref_branch: &str,
        inputs: &serde_json::Map<String, serde_json::Value>,
    ) -> anyhow::Result<()> {
        let token = self.token().await?;
        let url = format!(
            "https://api.github.com/repos/{}/actions/workflows/{}/dispatches",
            self.repo,
            urlencoding(workflow_file_name)
        );
        let payload = json!({
            "ref": ref_branch,
            "inputs": inputs,
        });
        self.http
            .post(&url)
            .header("Authorization", format!("Bearer {token}"))
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .header("User-Agent", "house-chatbot")
            .json(&payload)
            .send()
            .await?
            .error_for_status()?;
        Ok(())
    }

    /// List all workflows in the repository.
    pub async fn list_workflows(&self) -> String {
        match self.authed_get("/actions/workflows?per_page=50").await {
            Ok(body) => match serde_json::from_str::<serde_json::Value>(&body) {
                Ok(val) => {
                    let workflows = val["workflows"]
                        .as_array()
                        .map(|arr| {
                            arr.iter()
                                .map(|w| {
                                    json!({
                                        "id": w["id"],
                                        "name": w["name"],
                                        "state": w["state"],
                                        "path": w["path"],
                                        "html_url": w["html_url"],
                                    })
                                })
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default();
                    serde_json::to_string_pretty(&json!({"workflows": workflows}))
                        .unwrap_or_default()
                }
                Err(e) => format!("Error: failed to parse workflows — {e}"),
            },
            Err(e) => format!("Error: {e}"),
        }
    }

    /// List workflow runs with optional filters.
    pub async fn list_workflow_runs(
        &self,
        workflow_name: &str,
        branch: &str,
        status: &str,
        event: &str,
        created: &str,
    ) -> String {
        let (base_path, mut params) = if workflow_name.is_empty() {
            ("/actions/runs".to_string(), vec!["per_page=20".to_string()])
        } else {
            let path = format!("/actions/workflows/{}/runs", urlencoding(workflow_name));
            (path, vec!["per_page=20".to_string()])
        };
        if !branch.is_empty() {
            params.push(format!("branch={}", urlencoding(branch)));
        }
        if !status.is_empty() {
            params.push(format!("status={}", urlencoding(status)));
        }
        if !event.is_empty() {
            params.push(format!("event={}", urlencoding(event)));
        }
        if !created.is_empty() {
            params.push(format!("created={}", urlencoding(created)));
        }
        let qs = params.join("&");
        let path = format!("{base_path}?{qs}");
        match self.authed_get(&path).await {
            Ok(body) => match serde_json::from_str::<serde_json::Value>(&body) {
                Ok(val) => {
                    let runs = val["workflow_runs"]
                        .as_array()
                        .map(|arr| {
                            arr.iter()
                                .map(|r| {
                                    json!({
                                        "id": r["id"],
                                        "name": r["name"],
                                        "workflow_id": r["workflow_id"],
                                        "head_branch": r["head_branch"],
                                        "head_sha": r["head_sha"],
                                        "status": r["status"],
                                        "conclusion": r["conclusion"],
                                        "event": r["event"],
                                        "display_title": r["display_title"],
                                        "html_url": r["html_url"],
                                        "created_at": r["created_at"],
                                        "updated_at": r["updated_at"],
                                        "run_started_at": r["run_started_at"],
                                    })
                                })
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default();
                    let total = &val["total_count"];
                    serde_json::to_string_pretty(
                        &json!({"total_count": total, "workflow_runs": runs}),
                    )
                    .unwrap_or_default()
                }
                Err(e) => format!("Error: failed to parse workflow runs — {e}"),
            },
            Err(e) => format!("Error: {e}"),
        }
    }

    /// Get details for a specific workflow run.
    pub async fn get_workflow_run(&self, run_id: u64) -> String {
        match self.authed_get(&format!("/actions/runs/{run_id}")).await {
            Ok(body) => match serde_json::from_str::<serde_json::Value>(&body) {
                Ok(val) => {
                    let run = json!({
                        "id": val["id"],
                        "name": val["name"],
                        "head_branch": val["head_branch"],
                        "head_sha": val["head_sha"],
                        "status": val["status"],
                        "conclusion": val["conclusion"],
                        "event": val["event"],
                        "display_title": val["display_title"],
                        "html_url": val["html_url"],
                        "created_at": val["created_at"],
                        "updated_at": val["updated_at"],
                        "run_started_at": val["run_started_at"],
                        "run_attempt": val["run_attempt"],
                        "actor": val["actor"]["login"],
                    });
                    serde_json::to_string_pretty(&run).unwrap_or_default()
                }
                Err(e) => format!("Error: failed to parse workflow run — {e}"),
            },
            Err(e) => format!("Error: {e}"),
        }
    }

    /// List jobs for a specific workflow run.
    pub async fn get_workflow_run_jobs(&self, run_id: u64) -> String {
        match self
            .authed_get(&format!("/actions/runs/{run_id}/jobs?per_page=50"))
            .await
        {
            Ok(body) => match serde_json::from_str::<serde_json::Value>(&body) {
                Ok(val) => {
                    let jobs = val["jobs"]
                        .as_array()
                        .map(|arr| {
                            arr.iter()
                                .map(|j| {
                                    json!({
                                        "id": j["id"],
                                        "name": j["name"],
                                        "status": j["status"],
                                        "conclusion": j["conclusion"],
                                        "started_at": j["started_at"],
                                        "completed_at": j["completed_at"],
                                        "runner_name": j["runner_name"],
                                        "steps": j["steps"].as_array().map(|steps| {
                                            steps.iter().map(|s| {
                                                json!({
                                                    "name": s["name"],
                                                    "status": s["status"],
                                                    "conclusion": s["conclusion"],
                                                    "number": s["number"],
                                                })
                                            }).collect::<Vec<_>>()
                                        }),
                                    })
                                })
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default();
                    let total = &val["total_count"];
                    serde_json::to_string_pretty(&json!({"total_count": total, "jobs": jobs}))
                        .unwrap_or_default()
                }
                Err(e) => format!("Error: failed to parse workflow jobs — {e}"),
            },
            Err(e) => format!("Error: {e}"),
        }
    }
}
