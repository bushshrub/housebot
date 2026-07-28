//! Read-only listing/search endpoints plus bulk pruning.

use serde_json::json;

use crate::{format_issue_list, next_page_url, urlencoding, GitHubIssueReporter};

impl GitHubIssueReporter {
    /// List issues with optional state and label filters.
    pub async fn list_issues(&self, state: &str, labels: &str) -> String {
        let state = urlencoding(state);
        let path = format!("/issues?state={state}&per_page=20");
        let path = if labels.is_empty() {
            path
        } else {
            format!("{path}&labels={}", urlencoding(labels))
        };
        match self.authed_get(&path).await {
            Ok(body) => format_issue_list(&body),
            Err(e) => format!("Error: {e}"),
        }
    }

    /// Search issues in the repository.
    pub async fn search_issues(&self, query: &str) -> String {
        let q = urlencoding(query);
        let repo_q = urlencoding(&self.repo);
        let path = format!("/search/issues?q=repo%3A{repo_q}+is%3Aissue+{q}&per_page=20");
        match self.authed_get(&path).await {
            Ok(body) => format_issue_list(&body),
            Err(e) => format!("Error: {e}"),
        }
    }

    /// Get basic repository metadata (stars, forks, description, etc.).
    pub async fn get_repo(&self) -> String {
        match self.authed_get("").await {
            Ok(body) => match serde_json::from_str::<serde_json::Value>(&body) {
                Ok(repo) => json!({
                    "full_name": repo["full_name"],
                    "description": repo["description"],
                    "default_branch": repo["default_branch"],
                    "stars": repo["stargazers_count"],
                    "forks": repo["forks_count"],
                    "open_issues": repo["open_issues_count"],
                    "language": repo["language"],
                    "topics": repo["topics"],
                    "visibility": repo["visibility"],
                    "html_url": repo["html_url"],
                    "created_at": repo["created_at"],
                    "updated_at": repo["updated_at"],
                })
                .to_string(),
                Err(e) => format!("Error: failed to parse repo info — {e}"),
            },
            Err(e) => format!("Error: {e}"),
        }
    }

    /// Prune issues matching criteria: optionally close stale issues or bulk-label them.
    /// Returns a human-readable summary of what was done.
    pub async fn prune_issues(
        &self,
        state: &str,
        labels: &str,
        action: &str,
        action_value: &str,
    ) -> String {
        if !self.is_configured() {
            return "Error: GitHub integration is not configured.".to_string();
        }
        match self
            .try_prune_issues(state, labels, action, action_value)
            .await
        {
            Ok(summary) => summary,
            Err(e) => format!("Error: {e}"),
        }
    }

    async fn try_prune_issues(
        &self,
        state: &str,
        labels: &str,
        action: &str,
        action_value: &str,
    ) -> anyhow::Result<String> {
        let token = self.token().await?;
        let state_e = urlencoding(state);
        let mut path = format!(
            "https://api.github.com/repos/{}/issues?state={state_e}&per_page=100",
            self.repo
        );
        if !labels.is_empty() {
            path.push_str(&format!("&labels={}", urlencoding(labels)));
        }

        // Fetch all pages of issues
        let mut all_issues: Vec<serde_json::Value> = Vec::new();
        let mut url = path;
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
            // Filter out PRs
            for issue in page {
                if issue.get("pull_request").is_none() {
                    all_issues.push(issue);
                }
            }
            match next {
                Some(u) => url = u,
                None => break,
            }
        }

        if all_issues.is_empty() {
            return Ok("No issues found matching the criteria.".to_string());
        }

        let numbers: Vec<u64> = all_issues
            .iter()
            .filter_map(|i| i["number"].as_u64())
            .collect();

        let mut results: Vec<String> = Vec::new();
        let mut successes = 0u64;
        match action {
            "close" => {
                for &num in &numbers {
                    if self.close_issue(num).await {
                        successes += 1;
                        results.push(format!("#{num} closed"));
                    } else {
                        results.push(format!("#{num} failed to close"));
                    }
                }
            }
            "label" => {
                let new_labels: Vec<&str> = action_value
                    .split(',')
                    .map(|s| s.trim())
                    .filter(|s| !s.is_empty())
                    .collect();
                if new_labels.is_empty() {
                    return Err(anyhow::anyhow!(
                        "No valid labels provided for 'label' action."
                    ));
                }
                for &num in &numbers {
                    if self.add_labels(num, &new_labels).await {
                        successes += 1;
                        results.push(format!("#{num} labelled with [{}]", new_labels.join(", ")));
                    } else {
                        results.push(format!("#{num} failed to label"));
                    }
                }
            }
            "unlabel" => {
                let remove_labels: Vec<&str> = action_value
                    .split(',')
                    .map(|s| s.trim())
                    .filter(|s| !s.is_empty())
                    .collect();
                if remove_labels.is_empty() {
                    return Err(anyhow::anyhow!(
                        "No valid labels provided for 'unlabel' action."
                    ));
                }
                for &num in &numbers {
                    if self.remove_labels(num, &remove_labels).await {
                        successes += 1;
                        results.push(format!("#{num} unlabelled [{}]", remove_labels.join(", ")));
                    } else {
                        results.push(format!("#{num} failed to unlabel"));
                    }
                }
            }
            other => return Err(anyhow::anyhow!("Unknown prune action: {other}")),
        }

        if successes > 0 {
            Ok(format!(
                "Pruned {} of {} issue(s):\n{}",
                successes,
                numbers.len(),
                results.join("\n")
            ))
        } else {
            Ok(format!(
                "No issues were successfully pruned.\n{}",
                results.join("\n")
            ))
        }
    }
}
