//! Tool for creating GitHub feature and bug-report issues, with per-user rate limiting.

use std::time::Duration;

use chrono::Utc;
use serde_json::{json, Value};

use crate::github_issues::GitHubIssueReporter;
use crate::rate_limit::RateLimiter;

const RATE_LIMIT_MAX_REQUESTS: usize = 3;
const RATE_LIMIT_WINDOW: Duration = Duration::from_secs(600); // 10 minutes

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RequestType {
    Feature,
    Bug,
}

impl RequestType {
    fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "" | "feature" => Some(Self::Feature),
            "bug" => Some(Self::Bug),
            _ => None,
        }
    }

    fn heading(self) -> &'static str {
        match self {
            Self::Feature => "Feature Request",
            Self::Bug => "Bug Report",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Feature => "enhancement",
            Self::Bug => "bug",
        }
    }

    fn filed_message(self) -> &'static str {
        match self {
            Self::Feature => "Feature request filed",
            Self::Bug => "Bug report filed",
        }
    }
}

/// OpenAI-style tool definition (internal `input_schema` form).
pub fn definition() -> Value {
    json!({
        "name": "create_feature_request",
        "description": "Create a GitHub issue to track a feature request or bug report made by a user. Use this \
            whenever a user requests a new feature, improvement, or reports broken behavior in the bot. Returns the URL of the created issue.",
        "input_schema": {
            "type": "object",
            "properties": {
                "title": {"type": "string", "description": "Short, clear issue title (under 100 chars)."},
                "description": {"type": "string", "description": "Full description of the requested feature or observed bug, including context, motivation, or reproduction details."},
                "type": {
                    "type": "string",
                    "enum": ["feature", "bug"],
                    "default": "feature",
                    "description": "Whether to file a feature request or bug report. Defaults to feature for backward compatibility."
                }
            },
            "required": ["title", "description"]
        }
    })
}

pub fn default_rate_limiter() -> RateLimiter {
    RateLimiter::new(RATE_LIMIT_MAX_REQUESTS, RATE_LIMIT_WINDOW)
}

/// Build a structured issue body with immutable Discord requester metadata.
pub(crate) fn build_body(
    description: &str,
    request_type: RequestType,
    requester_username: &str,
    requester_id: &str,
) -> String {
    let ts = Utc::now().format("%Y-%m-%d %H:%M:%S UTC");
    let heading = request_type.heading();
    format!(
        "## {heading}\n\n**Discord username:** {requester_username}\n**Discord user ID:** {requester_id}\n**Timestamp:** {ts}\n\n\
         ## Description\n\n{description}\n\n---\n*Filed automatically by house-chatbot*"
    )
}

/// File a feature request or bug report, honoring configuration and rate limits.
pub async fn create_feature_request(
    reporter: &GitHubIssueReporter,
    limiter: &RateLimiter,
    title: &str,
    description: &str,
    request_type: &str,
    requester_username: &str,
    requester_id: &str,
) -> String {
    let Some(request_type) = RequestType::parse(request_type) else {
        return "Error: request type must be 'feature' or 'bug'.".to_string();
    };
    if !reporter.is_configured() {
        return "Error: GitHub integration is not configured — issue was not filed.".to_string();
    }
    if limiter.check(requester_id) {
        return format!(
            "Error: rate limit exceeded — you can file at most {RATE_LIMIT_MAX_REQUESTS} reports \
             every {} minutes. Please try again later.",
            RATE_LIMIT_WINDOW.as_secs() / 60
        );
    }
    let body = build_body(description, request_type, requester_username, requester_id);
    match reporter
        .create_issue(title, &body, &[request_type.label()])
        .await
    {
        Some(url) => format!("{}: {url}", request_type.filed_message()),
        None => "Error: Failed to create GitHub issue — check bot logs for details.".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_body_includes_requester_and_description() {
        let body = build_body("Add dark mode", RequestType::Feature, "alice", "42");
        assert!(body.contains("**Discord username:** alice"));
        assert!(body.contains("**Discord user ID:** 42"));
        assert!(body.contains("Add dark mode"));
        assert!(body.contains("Feature Request"));
    }

    #[test]
    fn bug_reports_have_bug_heading_and_label() {
        let body = build_body("Crashes on startup", RequestType::Bug, "alice", "42");
        assert!(body.contains("## Bug Report"));
        assert_eq!(RequestType::Bug.label(), "bug");
        assert_eq!(RequestType::Feature.label(), "enhancement");
    }

    #[tokio::test]
    async fn unconfigured_reporter_returns_error() {
        let reporter =
            GitHubIssueReporter::new(String::new(), String::new(), String::new(), String::new());
        let rl = default_rate_limiter();
        let out = create_feature_request(&reporter, &rl, "t", "d", "feature", "alice", "42").await;
        assert!(out.starts_with("Error:"));
        assert!(out.contains("not configured"));
    }

    #[test]
    fn definition_has_required_fields() {
        let d = definition();
        assert_eq!(d["name"], "create_feature_request");
        assert_eq!(
            d["input_schema"]["required"],
            json!(["title", "description"])
        );
        assert_eq!(
            d["input_schema"]["properties"]["type"]["enum"],
            json!(["feature", "bug"])
        );
    }

    #[tokio::test]
    async fn invalid_request_type_is_rejected() {
        let reporter =
            GitHubIssueReporter::new(String::new(), String::new(), String::new(), String::new());
        let output = create_feature_request(
            &reporter,
            &default_rate_limiter(),
            "t",
            "d",
            "incident",
            "alice",
            "42",
        )
        .await;
        assert_eq!(output, "Error: request type must be 'feature' or 'bug'.");
    }
}
