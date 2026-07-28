//! Issue comments and the assembled issue-detail view.

use serde_json::json;

use crate::{next_page_url, GitHubIssueReporter};

impl GitHubIssueReporter {
    /// Post a comment on an issue. Returns `false` on any failure / when unconfigured.
    pub async fn post_issue_comment(&self, issue_number: u64, body: &str) -> bool {
        if !self.is_configured() {
            return false;
        }
        match self.try_post_issue_comment(issue_number, body).await {
            Ok(()) => true,
            Err(e) => {
                tracing::error!(issue_number, "Failed to post GitHub issue comment: {e}");
                false
            }
        }
    }

    async fn try_post_issue_comment(&self, issue_number: u64, body: &str) -> anyhow::Result<()> {
        let token = self.token().await?;
        let url = format!(
            "https://api.github.com/repos/{}/issues/{issue_number}/comments",
            self.repo
        );
        self.http
            .post(&url)
            .header("Authorization", format!("Bearer {token}"))
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .header("User-Agent", "house-chatbot")
            .json(&json!({ "body": body }))
            .send()
            .await?
            .error_for_status()?;
        Ok(())
    }

    /// Fetch all pages of comments for an issue by following Link headers.
    async fn fetch_all_comments(
        &self,
        comments_url: &str,
        token: &str,
    ) -> anyhow::Result<Vec<serde_json::Value>> {
        let mut all_comments = Vec::new();
        let mut url = comments_url.to_string();
        loop {
            let resp = self
                .http
                .get(&url)
                .header("Authorization", format!("Bearer {token}"))
                .header("Accept", "application/vnd.github+json")
                .header("X-GitHub-Api-Version", "2022-11-28")
                .header("User-Agent", "house-chatbot")
                .send()
                .await?
                .error_for_status()?;
            let next = next_page_url(&resp);
            let page: Vec<serde_json::Value> = resp.json().await?;
            all_comments.extend(page);
            match next {
                Some(u) => url = u,
                None => break,
            }
        }
        Ok(all_comments)
    }

    /// Fetch full issue detail including body, labels, and comments.
    pub async fn get_issue_detail(&self, issue_number: u64) -> Option<String> {
        if !self.is_configured() {
            return None;
        }
        match self.try_get_issue_detail(issue_number).await {
            Ok(info) => Some(info),
            Err(e) => {
                tracing::error!(issue_number, "Failed to fetch GitHub issue detail: {e}");
                Some(format!("Error: {e}"))
            }
        }
    }

    async fn try_get_issue_detail(&self, issue_number: u64) -> anyhow::Result<String> {
        let token = self.token().await?;
        let url = format!(
            "https://api.github.com/repos/{}/issues/{issue_number}",
            self.repo
        );
        let issue: serde_json::Value = self
            .http
            .get(&url)
            .header("Authorization", format!("Bearer {token}"))
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .header("User-Agent", "house-chatbot")
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        let number = issue["number"].as_u64().unwrap_or(0);
        let title = issue["title"].as_str().unwrap_or("(untitled)");
        let state = issue["state"].as_str().unwrap_or("unknown");
        let body = issue["body"].as_str().unwrap_or("*(no description)*");
        let labels: Vec<String> = issue["labels"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|l| l["name"].as_str().map(|n| n.to_string()))
                    .collect()
            })
            .unwrap_or_default();
        let label_str = if labels.is_empty() {
            String::new()
        } else {
            format!("\nLabels: {}", labels.join(", "))
        };

        // Fetch comments (all pages via Link header pagination)
        let comments = self
            .fetch_all_comments(&format!("{url}/comments?per_page=100"), &token)
            .await?;

        let comment_lines: Vec<String> = comments
            .iter()
            .map(|c| {
                let author = c["user"]["login"].as_str().unwrap_or("unknown");
                let cbody = c["body"].as_str().unwrap_or("");
                format!("> **{author}:**\n{cbody}")
            })
            .collect();
        let comments_section = if comment_lines.is_empty() {
            String::new()
        } else {
            format!(
                "\n\n**Comments ({}):**\n{}",
                comment_lines.len(),
                comment_lines.join("\n\n")
            )
        };

        Ok(format!(
            "#{number} **{title}** ({state}){label_str}\n\n{body}{comments_section}",
        ))
    }
}
