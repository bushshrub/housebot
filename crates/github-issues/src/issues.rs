//! Creating, reading, and updating issues.

use serde::Deserialize;
use serde_json::json;

use crate::GitHubIssueReporter;

/// The result of successfully creating a GitHub issue.
#[derive(Debug, Clone)]
pub struct CreatedIssue {
    pub number: u64,
    pub html_url: String,
}

#[derive(Deserialize)]
struct IssueResponse {
    number: u64,
    html_url: String,
}

/// The fields needed to authorize and edit an existing issue.
#[derive(Debug, Clone, Deserialize)]
pub struct ExistingIssue {
    pub body: Option<String>,
    pub html_url: String,
    pub pull_request: Option<serde_json::Value>,
}

impl GitHubIssueReporter {
    /// Create an issue and return its URL, or `None` on any failure / when unconfigured.
    pub async fn create_issue(&self, title: &str, body: &str, labels: &[&str]) -> Option<String> {
        self.create_issue_full(title, body, labels)
            .await
            .map(|i| i.html_url)
    }

    /// Create an issue and return the full `CreatedIssue` (number + URL), or `None` on failure.
    pub async fn create_issue_full(
        &self,
        title: &str,
        body: &str,
        labels: &[&str],
    ) -> Option<CreatedIssue> {
        if !self.is_configured() {
            return None;
        }
        match self.try_create_issue(title, body, labels).await {
            Ok(issue) => Some(issue),
            Err(e) => {
                tracing::error!("Failed to create GitHub issue: {e}");
                None
            }
        }
    }

    async fn try_create_issue(
        &self,
        title: &str,
        body: &str,
        labels: &[&str],
    ) -> anyhow::Result<CreatedIssue> {
        let token = self.token().await?;
        let url = format!("https://api.github.com/repos/{}/issues", self.repo);
        let labels: Vec<String> = if labels.is_empty() {
            vec!["bug".into()]
        } else {
            labels.iter().map(|s| s.to_string()).collect()
        };
        let payload = json!({ "title": title, "body": body, "labels": labels });
        let resp = self
            .http
            .post(&url)
            .header("Authorization", format!("Bearer {token}"))
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .header("User-Agent", "house-chatbot")
            .json(&payload)
            .send()
            .await?
            .error_for_status()?
            .json::<IssueResponse>()
            .await?;
        Ok(CreatedIssue {
            number: resp.number,
            html_url: resp.html_url,
        })
    }

    /// Fetch an issue from the configured repository.
    pub async fn fetch_issue(&self, issue_number: u64) -> Option<ExistingIssue> {
        if !self.is_configured() {
            return None;
        }
        match self.try_fetch_issue(issue_number).await {
            Ok(issue) => Some(issue),
            Err(e) => {
                tracing::error!(issue_number, "Failed to fetch GitHub issue: {e}");
                None
            }
        }
    }

    async fn try_fetch_issue(&self, issue_number: u64) -> anyhow::Result<ExistingIssue> {
        let token = self.token().await?;
        let url = format!(
            "https://api.github.com/repos/{}/issues/{issue_number}",
            self.repo
        );
        Ok(self
            .http
            .get(&url)
            .header("Authorization", format!("Bearer {token}"))
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .header("User-Agent", "house-chatbot")
            .send()
            .await?
            .error_for_status()?
            .json::<ExistingIssue>()
            .await?)
    }

    /// Update the title and/or body of an issue in the configured repository.
    pub async fn update_issue(
        &self,
        issue_number: u64,
        title: Option<&str>,
        body: Option<&str>,
    ) -> Option<String> {
        if !self.is_configured() {
            return None;
        }
        match self.try_update_issue(issue_number, title, body).await {
            Ok(issue) => Some(issue.html_url),
            Err(e) => {
                tracing::error!(issue_number, "Failed to update GitHub issue: {e}");
                None
            }
        }
    }

    async fn try_update_issue(
        &self,
        issue_number: u64,
        title: Option<&str>,
        body: Option<&str>,
    ) -> anyhow::Result<ExistingIssue> {
        let token = self.token().await?;
        let url = format!(
            "https://api.github.com/repos/{}/issues/{issue_number}",
            self.repo
        );
        let mut payload = serde_json::Map::new();
        if let Some(title) = title {
            payload.insert("title".into(), json!(title));
        }
        if let Some(body) = body {
            payload.insert("body".into(), json!(body));
        }
        Ok(self
            .http
            .patch(&url)
            .header("Authorization", format!("Bearer {token}"))
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .header("User-Agent", "house-chatbot")
            .json(&payload)
            .send()
            .await?
            .error_for_status()?
            .json::<ExistingIssue>()
            .await?)
    }

    /// Create an issue that references a Sentry event, with no sensitive data in the body.
    pub async fn create_error_issue(&self, sentry_event_id: &str) -> Option<String> {
        if !self.is_configured() {
            return None;
        }
        let title = format!("Bot error — Sentry event {sentry_event_id}");
        let body = format!(
            "An error occurred in the bot. Details are available in Sentry.\n\n\
             Sentry Event ID: `{sentry_event_id}`\n"
        );
        self.create_issue(&title, &body, &["bug"]).await
    }
}
