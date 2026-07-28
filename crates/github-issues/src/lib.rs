//! GitHub App integration for creating issues (feature requests + error reports).
//!
//! Uses RS256 JWT auth to obtain a short-lived installation token, cached until near
//! expiry, then POSTs to the GitHub Issues REST API.

use std::env;
use std::sync::Mutex;

mod auth;
mod format;
mod issue_comments;
mod issue_labels;
mod issue_queries;
mod issues;
mod merge;
mod workflows;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_integration;

pub use issues::{CreatedIssue, ExistingIssue};
pub use merge::{MergeOutcome, MERGE_METHODS};

use format::{format_issue_list, next_page_url, urlencoding};

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
}
