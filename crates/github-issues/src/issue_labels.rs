//! Label mutation and issue closing.

use serde_json::json;

use crate::{urlencoding, GitHubIssueReporter};

impl GitHubIssueReporter {
    /// Close an issue by number. Returns `true` on success.
    pub async fn close_issue(&self, issue_number: u64) -> bool {
        if !self.is_configured() {
            return false;
        }
        match self.try_close_issue(issue_number).await {
            Ok(()) => true,
            Err(e) => {
                tracing::error!(issue_number, "Failed to close GitHub issue: {e}");
                false
            }
        }
    }

    async fn try_close_issue(&self, issue_number: u64) -> anyhow::Result<()> {
        let token = self.token().await?;
        let url = format!(
            "https://api.github.com/repos/{}/issues/{issue_number}",
            self.repo
        );
        self.http
            .patch(&url)
            .header("Authorization", format!("Bearer {token}"))
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .header("User-Agent", "house-chatbot")
            .json(&json!({"state": "closed"}))
            .send()
            .await?
            .error_for_status()?;
        Ok(())
    }

    /// Add labels to an issue. Returns `true` on success.
    pub async fn add_labels(&self, issue_number: u64, labels: &[&str]) -> bool {
        if !self.is_configured() {
            return false;
        }
        match self.try_add_labels(issue_number, labels).await {
            Ok(()) => true,
            Err(e) => {
                tracing::error!(issue_number, "Failed to add labels to GitHub issue: {e}");
                false
            }
        }
    }

    async fn try_add_labels(&self, issue_number: u64, labels: &[&str]) -> anyhow::Result<()> {
        let token = self.token().await?;
        let url = format!(
            "https://api.github.com/repos/{}/issues/{issue_number}/labels",
            self.repo
        );
        self.http
            .post(&url)
            .header("Authorization", format!("Bearer {token}"))
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .header("User-Agent", "house-chatbot")
            .json(&json!({ "labels": labels }))
            .send()
            .await?
            .error_for_status()?;
        Ok(())
    }

    /// Remove labels from an issue. Returns `true` on success.
    pub async fn remove_labels(&self, issue_number: u64, labels: &[&str]) -> bool {
        if !self.is_configured() {
            return false;
        }
        match self.try_remove_labels(issue_number, labels).await {
            Ok(()) => true,
            Err(e) => {
                tracing::error!(
                    issue_number,
                    "Failed to remove labels from GitHub issue: {e}"
                );
                false
            }
        }
    }

    async fn try_remove_labels(&self, issue_number: u64, labels: &[&str]) -> anyhow::Result<()> {
        let token = self.token().await?;
        let mut errors = Vec::new();
        for label in labels {
            let url = format!(
                "https://api.github.com/repos/{}/issues/{issue_number}/labels/{}",
                self.repo,
                urlencoding(label)
            );
            match self
                .http
                .delete(&url)
                .header("Authorization", format!("Bearer {token}"))
                .header("Accept", "application/vnd.github+json")
                .header("X-GitHub-Api-Version", "2022-11-28")
                .header("User-Agent", "house-chatbot")
                .send()
                .await
            {
                Ok(resp) => {
                    if let Err(e) = resp.error_for_status() {
                        errors.push(format!("'{label}': {e}"));
                    }
                }
                Err(e) => {
                    errors.push(format!("'{label}': {e}"));
                }
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            // Partial failure is still failure: callers use the result to
            // decide whether the labels are gone, so leftover labels must not
            // be reported as removed.
            Err(anyhow::anyhow!(
                "Failed to remove {} of {} requested labels: {}",
                errors.len(),
                labels.len(),
                errors.join("; ")
            ))
        }
    }
}
