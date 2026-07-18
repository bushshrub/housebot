//! GitHub App integration for creating issues (feature requests + error reports).
//!
//! Uses RS256 JWT auth to obtain a short-lived installation token, cached until near
//! expiry, then POSTs to the GitHub Issues REST API.

use std::env;
use std::sync::Mutex;

use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Debug, Serialize)]
struct Claims {
    iat: u64,
    exp: u64,
    iss: String,
}

/// Build the JWT claims for `app_id` relative to `now` (unix seconds).
fn build_claims(app_id: &str, now: u64) -> Claims {
    Claims {
        iat: now - 60,
        exp: now + 600,
        iss: app_id.to_string(),
    }
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[derive(Deserialize)]
struct TokenResponse {
    token: String,
}

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
}

/// Files GitHub issues on behalf of the bot's GitHub App installation.
/// Also supports direct GITHUB_TOKEN auth for integration testing.
pub struct GitHubIssueReporter {
    app_id: String,
    private_key: String,
    installation_id: String,
    repo: String,
    http: reqwest::Client,
    cached: Mutex<Option<(String, u64)>>, // (token, expires_at_unix)
    direct_token: Option<String>,
}

impl Default for GitHubIssueReporter {
    fn default() -> Self {
        Self::from_env()
    }
}

impl GitHubIssueReporter {
    /// Construct a reporter from the `GITHUB_*` environment variables.
    pub fn from_env() -> Self {
        Self::new(
            env::var("GITHUB_APP_ID").unwrap_or_default(),
            // Private keys are stored with literal `\n`; normalize to real newlines.
            env::var("GITHUB_APP_PRIVATE_KEY")
                .unwrap_or_default()
                .replace("\\n", "\n"),
            env::var("GITHUB_INSTALLATION_ID").unwrap_or_default(),
            env::var("GITHUB_REPO").unwrap_or_default(),
        )
    }

    /// Construct a reporter with explicit credentials.
    pub fn new(app_id: String, private_key: String, installation_id: String, repo: String) -> Self {
        Self {
            app_id,
            private_key,
            installation_id,
            repo,
            http: reqwest::Client::new(),
            cached: Mutex::new(None),
            direct_token: None,
        }
    }

    /// Construct a reporter that authenticates with a direct GITHUB_TOKEN
    /// instead of the GitHub App JWT flow. Useful for integration tests.
    pub fn with_direct_token(token: String, repo: String) -> Self {
        Self {
            app_id: String::new(),
            private_key: String::new(),
            installation_id: String::new(),
            repo,
            http: reqwest::Client::new(),
            cached: Mutex::new(None),
            direct_token: Some(token),
        }
    }

    /// Whether every credential needed to file issues is present.
    pub fn is_configured(&self) -> bool {
        (self.direct_token.as_deref().is_some_and(|t| !t.is_empty()) && !self.repo.is_empty())
            || (!self.app_id.is_empty()
                && !self.private_key.is_empty()
                && !self.installation_id.is_empty()
                && !self.repo.is_empty())
    }

    fn generate_jwt(&self) -> anyhow::Result<String> {
        let claims = build_claims(&self.app_id, unix_now());
        let key = EncodingKey::from_rsa_pem(self.private_key.as_bytes())?;
        Ok(encode(&Header::new(Algorithm::RS256), &claims, &key)?)
    }

    async fn installation_token(&self) -> anyhow::Result<String> {
        {
            let guard = self.cached.lock().unwrap();
            if let Some((tok, exp)) = guard.as_ref() {
                if unix_now() < exp.saturating_sub(60) {
                    return Ok(tok.clone());
                }
            }
        }

        let jwt = self.generate_jwt()?;
        let url = format!(
            "https://api.github.com/app/installations/{}/access_tokens",
            self.installation_id
        );
        let resp = self
            .http
            .post(&url)
            .header("Authorization", format!("Bearer {jwt}"))
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .header("User-Agent", "house-chatbot")
            .send()
            .await?
            .error_for_status()?
            .json::<TokenResponse>()
            .await?;

        let token = resp.token;
        *self.cached.lock().unwrap() = Some((token.clone(), unix_now() + 3600));
        Ok(token)
    }

    /// Return a bearer token — either the direct token (GITHUB_TOKEN) or a
    /// GitHub App installation token obtained via the JWT flow.
    async fn token(&self) -> anyhow::Result<String> {
        if let Some(token) = &self.direct_token {
            return Ok(token.clone());
        }
        self.installation_token().await
    }

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

    /// Perform an authenticated GET request to the GitHub API.
    /// Paths starting with `/search/` are treated as root-level API paths;
    /// all others are prefixed with `/repos/{repo}`.
    async fn authed_get(&self, path: &str) -> anyhow::Result<String> {
        let token = self.token().await?;
        let url = if path.starts_with("/search/") {
            format!("https://api.github.com{path}")
        } else {
            format!("https://api.github.com/repos/{}{}", self.repo, path)
        };
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
        Ok(resp.text().await?)
    }

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
        } else if errors.len() < labels.len() {
            tracing::warn!(
                issue_number,
                "Partial label removal ({} of {} failed): {}",
                errors.len(),
                labels.len(),
                errors.join("; ")
            );
            Ok(())
        } else {
            Err(anyhow::anyhow!(
                "Failed to remove all requested labels: {}",
                errors.join("; ")
            ))
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

/// Format a GitHub issues API response as a compact text list.
fn format_issue_list(body: &str) -> String {
    match serde_json::from_str::<serde_json::Value>(body) {
        Ok(val) => {
            let issues: Vec<&serde_json::Value> = if let Some(arr) = val.as_array() {
                arr.iter()
                    .filter(|i| i.get("pull_request").is_none())
                    .collect()
            } else if let Some(items) = val.get("items").and_then(|v| v.as_array()) {
                items
                    .iter()
                    .filter(|i| i.get("pull_request").is_none())
                    .collect()
            } else {
                return "Error: unexpected API response format.".to_string();
            };
            if issues.is_empty() {
                return "No issues found.".to_string();
            }
            let lines: Vec<String> = issues
                .iter()
                .map(|i| {
                    let number = i["number"].as_u64().unwrap_or(0);
                    let title = i["title"].as_str().unwrap_or("(untitled)");
                    let state = i["state"].as_str().unwrap_or("unknown");
                    let labels: Vec<String> = i["labels"]
                        .as_array()
                        .map(|labels| {
                            labels
                                .iter()
                                .filter_map(|l| l["name"].as_str().map(|n| n.to_string()))
                                .collect()
                        })
                        .unwrap_or_default();
                    let label_str = if labels.is_empty() {
                        String::new()
                    } else {
                        format!(" [{}]", labels.join(", "))
                    };
                    format!("#{number} ({state}){label_str} — {title}")
                })
                .collect();
            lines.join("\n")
        }
        Err(e) => format!("Error: failed to parse response — {e}"),
    }
}

/// Percent-encode a string for URL query parameters.
fn urlencoding(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    for byte in s.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                result.push(byte as char);
            }
            b' ' => result.push_str("%20"),
            _ => {
                result.push_str(&format!("%{byte:02X}"));
            }
        }
    }
    result
}

/// Extract the `rel="next"` page URL from a GitHub API response's Link header.
fn next_page_url(resp: &reqwest::Response) -> Option<String> {
    let link = resp.headers().get("link")?.to_str().ok()?;
    for part in link.split(',') {
        let part = part.trim();
        if part.contains("rel=\"next\"") {
            let start = part.find('<')?;
            let end = part.find('>')?;
            return Some(part[start + 1..end].to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn not_configured_when_fields_missing() {
        let r =
            GitHubIssueReporter::new(String::new(), String::new(), String::new(), String::new());
        assert!(!r.is_configured());
    }

    #[test]
    fn configured_when_all_fields_present() {
        let r = GitHubIssueReporter::new(
            "123".into(),
            "-----BEGIN KEY-----".into(),
            "456".into(),
            "owner/repo".into(),
        );
        assert!(r.is_configured());
    }

    #[test]
    fn partial_config_is_not_configured() {
        let r =
            GitHubIssueReporter::new("123".into(), "key".into(), "".into(), "owner/repo".into());
        assert!(!r.is_configured());
    }

    #[tokio::test]
    async fn create_issue_returns_none_when_unconfigured() {
        let r =
            GitHubIssueReporter::new(String::new(), String::new(), String::new(), String::new());
        assert!(r.create_issue("t", "b", &["bug"]).await.is_none());
        assert!(r.create_error_issue("evt123").await.is_none());
    }

    #[test]
    fn claims_have_expected_window() {
        let c = build_claims("42", 1_000_000);
        assert_eq!(c.iat, 1_000_000 - 60);
        assert_eq!(c.exp, 1_000_000 + 600);
        assert_eq!(c.iss, "42");
    }

    #[test]
    fn private_key_newlines_are_normalized() {
        std::env::set_var("GITHUB_APP_PRIVATE_KEY", "line1\\nline2");
        let r = GitHubIssueReporter::from_env();
        assert!(r.private_key.contains("line1\nline2"));
        std::env::remove_var("GITHUB_APP_PRIVATE_KEY");
    }

    #[test]
    fn with_direct_token_is_configured() {
        let r = GitHubIssueReporter::with_direct_token("ghp_token".into(), "owner/repo".into());
        assert!(r.is_configured());
    }

    #[test]
    fn with_direct_token_empty_token_not_configured() {
        let r = GitHubIssueReporter::with_direct_token("".into(), "owner/repo".into());
        assert!(!r.is_configured());
    }

    #[test]
    fn with_direct_token_empty_repo_not_configured() {
        let r = GitHubIssueReporter::with_direct_token("ghp_token".into(), "".into());
        assert!(!r.is_configured());
    }

    #[test]
    fn format_issue_list_filters_out_pull_requests() {
        let body = r#"[
            {"number": 1, "title": "Real issue", "state": "open", "labels": []},
            {"number": 2, "title": "PR", "state": "open", "labels": [], "pull_request": {"url": "..."}},
            {"number": 3, "title": "Another issue", "state": "closed", "labels": [{"name": "bug"}]}
        ]"#;
        let result = format_issue_list(body);
        assert!(result.contains("#1"));
        assert!(result.contains("Real issue"));
        assert!(result.contains("#3"));
        assert!(result.contains("Another issue"));
        assert!(!result.contains("#2"));
        assert!(!result.contains("PR"));
    }

    #[test]
    fn format_issue_list_filters_prs_from_search_response() {
        let body = r#"{
            "total_count": 2,
            "items": [
                {"number": 10, "title": "Search issue", "state": "open", "labels": []},
                {"number": 11, "title": "Search PR", "state": "open", "labels": [], "pull_request": {"url": "..."}}
            ]
        }"#;
        let result = format_issue_list(body);
        assert!(result.contains("#10"));
        assert!(result.contains("Search issue"));
        assert!(!result.contains("#11"));
        assert!(!result.contains("Search PR"));
    }

    #[test]
    fn format_issue_list_returns_not_found_for_empty() {
        let result = format_issue_list("[]");
        assert_eq!(result, "No issues found.");
    }

    #[test]
    fn format_issue_list_returns_not_found_for_empty_search() {
        let body = r#"{"total_count": 0, "items": []}"#;
        let result = format_issue_list(body);
        assert_eq!(result, "No issues found.");
    }

    #[test]
    fn format_issue_list_filters_prs_away_from_empty_result() {
        // Only PRs in the response — should show "No issues found."
        let body = r#"[
            {"number": 5, "title": "Only PR", "state": "open", "labels": [], "pull_request": {"url": "..."}}
        ]"#;
        let result = format_issue_list(body);
        assert_eq!(result, "No issues found.");
    }

    #[test]
    fn urlencoding_encodes_correctly() {
        assert_eq!(urlencoding("hello"), "hello");
        assert_eq!(urlencoding("hello world"), "hello%20world");
        assert_eq!(urlencoding("a/b"), "a%2Fb");
        assert_eq!(urlencoding("repo:owner/repo"), "repo%3Aowner%2Frepo");
    }

    // ── New lifecycle method tests ─────────────────────────────────────────

    #[tokio::test]
    async fn close_issue_returns_false_when_unconfigured() {
        let r =
            GitHubIssueReporter::new(String::new(), String::new(), String::new(), String::new());
        assert!(!r.close_issue(42).await);
    }

    #[tokio::test]
    async fn get_issue_detail_returns_none_when_unconfigured() {
        let r =
            GitHubIssueReporter::new(String::new(), String::new(), String::new(), String::new());
        assert!(r.get_issue_detail(42).await.is_none());
    }

    #[tokio::test]
    async fn add_labels_returns_false_when_unconfigured() {
        let r =
            GitHubIssueReporter::new(String::new(), String::new(), String::new(), String::new());
        assert!(!r.add_labels(42, &["bug"]).await);
    }

    #[tokio::test]
    async fn remove_labels_returns_false_when_unconfigured() {
        let r =
            GitHubIssueReporter::new(String::new(), String::new(), String::new(), String::new());
        assert!(!r.remove_labels(42, &["bug"]).await);
    }

    #[tokio::test]
    async fn prune_issues_returns_not_configured_when_unconfigured() {
        let r =
            GitHubIssueReporter::new(String::new(), String::new(), String::new(), String::new());
        let result = r.prune_issues("open", "", "close", "").await;
        assert!(result.contains("not configured"));
    }

    #[test]
    fn format_issue_list_parses_issue_numbers_for_prune() {
        let body = r#"[
            {"number": 10, "title": "Bug fix", "state": "open", "labels": [{"name": "bug"}]},
            {"number": 20, "title": "Feature", "state": "open", "labels": [{"name": "enhancement"}]},
            {"number": 30, "title": "PR", "state": "open", "labels": [], "pull_request": {"url": "..."}}
        ]"#;
        let result = format_issue_list(body);
        // Should filter out PRs
        assert!(result.contains("#10"));
        assert!(result.contains("#20"));
        assert!(!result.contains("#30"));
        // Each line starts with #
        for line in result.lines() {
            assert!(line.starts_with('#'), "unexpected line: {line}");
        }
    }

    // ── Integration tests (require GITHUB_TOKEN env var) ────────────────────

    /// Create a reporter from `GITHUB_TOKEN` + `GITHUB_REPO`, or skip.
    fn integration_reporter() -> Option<GitHubIssueReporter> {
        let token = std::env::var("GITHUB_TOKEN").ok()?;
        let repo = std::env::var("GITHUB_REPO")
            .ok()
            .filter(|r| !r.is_empty())
            .unwrap_or_else(|| "bushshrub/housebot".to_string());
        Some(GitHubIssueReporter::with_direct_token(token, repo))
    }

    #[tokio::test]
    async fn integration_get_repo() {
        let reporter = match integration_reporter() {
            Some(r) => r,
            None => return,
        };
        let result = reporter.get_repo().await;
        assert!(!result.starts_with("Error:"), "get_repo failed: {result}");
        let v: serde_json::Value =
            serde_json::from_str(&result).expect("get_repo should return valid JSON");
        assert_eq!(v["full_name"], "bushshrub/housebot");
        assert!(v["stars"].as_u64().is_some());
        assert!(v["forks"].as_u64().is_some());
        assert!(v["open_issues"].as_u64().is_some());
        assert!(!v["language"].as_str().unwrap_or("").is_empty());
        assert!(!v["default_branch"].as_str().unwrap_or("").is_empty());
    }

    #[tokio::test]
    async fn integration_list_issues() {
        let reporter = match integration_reporter() {
            Some(r) => r,
            None => return,
        };
        let result = reporter.list_issues("open", "").await;
        assert!(
            !result.starts_with("Error:"),
            "list_issues failed: {result}"
        );
        // Text response from format_issue_list — should contain issue entries
        assert!(
            result.contains('#'),
            "expected issue numbers in list_issues output:\n{result}"
        );
        // Every line should start with #
        for line in result.lines() {
            assert!(line.starts_with('#'), "unexpected line format: {line}");
        }
    }

    #[tokio::test]
    async fn integration_search_issues() {
        let reporter = match integration_reporter() {
            Some(r) => r,
            None => return,
        };
        let result = reporter.search_issues("bug").await;
        assert!(
            !result.starts_with("Error:"),
            "search_issues failed: {result}"
        );
        // Search results should also be formatted as issue lines with #
        if !result.contains("No issues found.") {
            assert!(
                result.contains('#'),
                "expected issue numbers in search_issues output:\n{result}"
            );
        }
    }

    #[tokio::test]
    async fn integration_list_workflows() {
        let reporter = match integration_reporter() {
            Some(r) => r,
            None => return,
        };
        let result = reporter.list_workflows().await;
        assert!(
            !result.starts_with("Error:"),
            "list_workflows failed: {result}"
        );
        let v: serde_json::Value =
            serde_json::from_str(&result).expect("list_workflows should return valid JSON");
        let workflows = v["workflows"]
            .as_array()
            .expect("workflows should be an array");
        assert!(!workflows.is_empty(), "expected at least one workflow");
        for w in workflows {
            assert!(w["id"].as_u64().is_some(), "workflow missing id: {w}");
            assert!(
                !w["name"].as_str().unwrap_or("").is_empty(),
                "workflow missing name: {w}"
            );
            assert!(
                !w["state"].as_str().unwrap_or("").is_empty(),
                "workflow missing state: {w}"
            );
        }
    }

    #[tokio::test]
    async fn integration_list_workflow_runs() {
        let reporter = match integration_reporter() {
            Some(r) => r,
            None => return,
        };
        let result = reporter.list_workflow_runs("", "master", "", "", "").await;
        assert!(
            !result.starts_with("Error:"),
            "list_workflow_runs failed: {result}"
        );
        let v: serde_json::Value =
            serde_json::from_str(&result).expect("list_workflow_runs should return valid JSON");
        let runs = v["workflow_runs"]
            .as_array()
            .expect("workflow_runs should be an array");
        assert!(!runs.is_empty(), "expected at least one workflow run");
        for r in runs {
            assert!(r["id"].as_u64().is_some(), "run missing id: {r}");
            assert!(
                !r["head_branch"].as_str().unwrap_or("").is_empty(),
                "run missing head_branch: {r}"
            );
            assert!(
                !r["status"].as_str().unwrap_or("").is_empty(),
                "run missing status: {r}"
            );
        }
    }

    #[tokio::test]
    async fn integration_list_workflow_runs_with_created() {
        let reporter = match integration_reporter() {
            Some(r) => r,
            None => return,
        };
        let result = reporter
            .list_workflow_runs("", "", "", "", ">=2026-01-01")
            .await;
        assert!(
            !result.starts_with("Error:"),
            "list_workflow_runs with created failed: {result}"
        );
        let v: serde_json::Value =
            serde_json::from_str(&result).expect("list_workflow_runs should return valid JSON");
        let runs = v["workflow_runs"]
            .as_array()
            .expect("workflow_runs should be an array");
        assert!(
            !runs.is_empty(),
            "expected at least one workflow run since 2026-01-01"
        );
        for r in runs {
            let created = r["created_at"].as_str().expect("run missing created_at");
            assert!(
                created >= "2026-01-01",
                "run {id} created_at ({created}) is before 2026-01-01",
                id = r["id"]
            );
        }
    }
}
