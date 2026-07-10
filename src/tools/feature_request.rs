//! Tool for creating GitHub feature-request issues, with per-user rate limiting.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use chrono::Utc;
use serde_json::{json, Value};

use crate::github_issues::GitHubIssueReporter;

const RATE_LIMIT_MAX_REQUESTS: usize = 3;
const RATE_LIMIT_WINDOW: Duration = Duration::from_secs(600); // 10 minutes

/// OpenAI-style tool definition (internal `input_schema` form).
pub fn definition() -> Value {
    json!({
        "name": "create_feature_request",
        "description": "Create a GitHub issue to track a feature request made by a user. Use this \
            whenever a user asks for a new feature or improvement to the bot. Returns the URL of \
            the created issue.",
        "input_schema": {
            "type": "object",
            "properties": {
                "title": {"type": "string", "description": "Short, clear title for the feature request (under 100 chars)."},
                "description": {"type": "string", "description": "Full description of the requested feature, including context and motivation."}
            },
            "required": ["title", "description"]
        }
    })
}

/// Sliding-window per-user rate limiter.
pub struct RateLimiter {
    max: usize,
    window: Duration,
    hits: Mutex<HashMap<String, Vec<Instant>>>,
}

impl Default for RateLimiter {
    fn default() -> Self {
        Self::new(RATE_LIMIT_MAX_REQUESTS, RATE_LIMIT_WINDOW)
    }
}

impl RateLimiter {
    pub fn new(max: usize, window: Duration) -> Self {
        Self {
            max,
            window,
            hits: Mutex::new(HashMap::new()),
        }
    }

    /// Record an attempt; return `true` when the user is now over the limit (attempt rejected).
    pub fn check(&self, user: &str) -> bool {
        self.check_at(user, Instant::now())
    }

    fn check_at(&self, user: &str, now: Instant) -> bool {
        let mut hits = self.hits.lock().unwrap_or_else(|p| p.into_inner());
        let entry = hits.entry(user.to_string()).or_default();
        entry.retain(|t| now.duration_since(*t) < self.window);
        if entry.len() >= self.max {
            return true;
        }
        entry.push(now);
        false
    }
}

/// Build the issue body for a feature request.
pub fn build_body(description: &str, requested_by: &str) -> String {
    let ts = Utc::now().format("%Y-%m-%d %H:%M:%S UTC");
    format!(
        "## Feature Request\n\n**Requested by:** {requested_by}\n**Timestamp:** {ts}\n\n\
         ## Description\n\n{description}\n\n---\n*Filed automatically by house-chatbot*"
    )
}

/// File a feature request, honoring configuration and rate limits.
pub async fn create_feature_request(
    reporter: &GitHubIssueReporter,
    limiter: &RateLimiter,
    title: &str,
    description: &str,
    requested_by: &str,
) -> String {
    if !reporter.is_configured() {
        return "Error: GitHub integration is not configured — feature request was not filed."
            .to_string();
    }
    if limiter.check(requested_by) {
        return format!(
            "Error: rate limit exceeded — you can file at most {RATE_LIMIT_MAX_REQUESTS} feature \
             requests every {} minutes. Please try again later.",
            RATE_LIMIT_WINDOW.as_secs() / 60
        );
    }
    let body = build_body(description, requested_by);
    match reporter.create_issue(title, &body, &["enhancement"]).await {
        Some(url) => format!("Feature request filed: {url}"),
        None => "Error: Failed to create GitHub issue — check bot logs for details.".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_limiter_allows_up_to_max() {
        let rl = RateLimiter::new(3, Duration::from_secs(600));
        assert!(!rl.check("u"));
        assert!(!rl.check("u"));
        assert!(!rl.check("u"));
        assert!(rl.check("u")); // 4th is limited
    }

    #[test]
    fn rate_limiter_is_per_user() {
        let rl = RateLimiter::new(1, Duration::from_secs(600));
        assert!(!rl.check("a"));
        assert!(!rl.check("b"));
        assert!(rl.check("a"));
    }

    #[test]
    fn rate_limiter_forgets_old_hits() {
        let rl = RateLimiter::new(1, Duration::from_millis(0));
        assert!(!rl.check("u"));
        // Window is zero, so the previous hit is immediately stale.
        assert!(!rl.check("u"));
    }

    #[test]
    fn build_body_includes_requester_and_description() {
        let body = build_body("Add dark mode", "user42");
        assert!(body.contains("user42"));
        assert!(body.contains("Add dark mode"));
        assert!(body.contains("Feature Request"));
    }

    #[tokio::test]
    async fn unconfigured_reporter_returns_error() {
        let reporter =
            GitHubIssueReporter::new(String::new(), String::new(), String::new(), String::new());
        let rl = RateLimiter::default();
        let out = create_feature_request(&reporter, &rl, "t", "d", "u").await;
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
    }
}
