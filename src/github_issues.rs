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
pub struct GitHubIssueReporter {
    app_id: String,
    private_key: String,
    installation_id: String,
    repo: String,
    http: reqwest::Client,
    cached: Mutex<Option<(String, u64)>>, // (token, expires_at_unix)
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
        }
    }

    /// Whether every credential needed to file issues is present.
    pub fn is_configured(&self) -> bool {
        !self.app_id.is_empty()
            && !self.private_key.is_empty()
            && !self.installation_id.is_empty()
            && !self.repo.is_empty()
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
        let token = self.installation_token().await?;
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
        let token = self.installation_token().await?;
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
        let token = self.installation_token().await?;
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
}
