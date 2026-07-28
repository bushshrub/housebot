//! Pull-request merging and the outcome taxonomy for GitHub's merge API.

use serde_json::json;

use crate::GitHubIssueReporter;

/// Merge strategies accepted by the GitHub merge API.
pub const MERGE_METHODS: [&str; 3] = ["merge", "squash", "rebase"];

/// The result of attempting to merge a pull request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MergeOutcome {
    Merged {
        sha: String,
        message: String,
    },
    /// The head and base branches conflict and cannot be merged automatically.
    Conflict(String),
    /// GitHub refused the merge (draft, failing required checks, missing reviews…).
    Blocked(String),
    /// The installation lacks write access to the repository.
    NotPermitted(String),
    NotFound(String),
    Error(String),
}

impl MergeOutcome {
    /// Short status word used in audit records and tool output.
    pub fn status(&self) -> &'static str {
        match self {
            Self::Merged { .. } => "success",
            Self::Conflict(_) => "conflict",
            Self::Blocked(_) => "blocked",
            Self::NotPermitted(_) => "not_permitted",
            Self::NotFound(_) => "not_found",
            Self::Error(_) => "error",
        }
    }
}

/// Map a GitHub merge-API response onto a [`MergeOutcome`].
pub(crate) fn merge_outcome(status: u16, body: &str) -> MergeOutcome {
    let parsed: serde_json::Value = serde_json::from_str(body).unwrap_or(serde_json::Value::Null);
    let message = parsed["message"]
        .as_str()
        .unwrap_or("no details returned by GitHub")
        .to_string();
    match status {
        200 => MergeOutcome::Merged {
            sha: parsed["sha"].as_str().unwrap_or_default().to_string(),
            message: parsed["message"]
                .as_str()
                .unwrap_or("Pull request successfully merged")
                .to_string(),
        },
        403 => MergeOutcome::NotPermitted(message),
        404 => MergeOutcome::NotFound(message),
        405 => MergeOutcome::Blocked(message),
        409 => MergeOutcome::Conflict(message),
        _ => MergeOutcome::Error(format!("GitHub returned HTTP {status} — {message}")),
    }
}

impl GitHubIssueReporter {
    /// Merge a pull request. `merge_method` must be one of [`MERGE_METHODS`].
    pub async fn merge_pull_request(&self, pull_number: u64, merge_method: &str) -> MergeOutcome {
        if !self.is_configured() {
            return MergeOutcome::Error(
                "GitHub integration is not configured — merging requires GITHUB_APP_ID, \
                 GITHUB_APP_PRIVATE_KEY, GITHUB_INSTALLATION_ID, and GITHUB_REPO to be set."
                    .to_string(),
            );
        }
        if pull_number == 0 {
            return MergeOutcome::Error("a valid pull request number is required.".to_string());
        }
        if !MERGE_METHODS.contains(&merge_method) {
            return MergeOutcome::Error(format!(
                "unsupported merge_method '{merge_method}' — expected one of {}.",
                MERGE_METHODS.join(", ")
            ));
        }
        let token = match self.token().await {
            Ok(token) => token,
            Err(error) => return MergeOutcome::Error(error.to_string()),
        };
        let url = format!(
            "https://api.github.com/repos/{}/pulls/{pull_number}/merge",
            self.repo
        );
        let response = self
            .http
            .put(&url)
            .header("Authorization", format!("Bearer {token}"))
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .header("User-Agent", "house-chatbot")
            .json(&json!({"merge_method": merge_method}))
            .send()
            .await;
        let response = match response {
            Ok(response) => response,
            Err(error) => return MergeOutcome::Error(error.to_string()),
        };
        let status = response.status().as_u16();
        let body = response.text().await.unwrap_or_default();
        merge_outcome(status, &body)
    }
}
