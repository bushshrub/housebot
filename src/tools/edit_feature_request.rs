//! Tool for editing bot-filed feature requests and bug reports, with ownership checks.

use std::time::Duration;

use serde_json::{json, Value};

use crate::github_issues::GitHubIssueReporter;
use crate::rate_limit::RateLimiter;

const RATE_LIMIT_MAX_REQUESTS: usize = 3;
const RATE_LIMIT_WINDOW: Duration = Duration::from_secs(600);
const FILED_FOOTER: &str = "*Filed automatically by house-chatbot*";
const DESCRIPTION_HEADING: &str = "## Description\n\n";

/// OpenAI-style tool definition (internal `input_schema` form).
pub fn definition() -> Value {
    json!({
        "name": "edit_feature_request",
        "description": "Edit the title or description of a GitHub feature request or bug report previously filed by the current user. The issue's original requester is verified before any update.",
        "input_schema": {
            "type": "object",
            "properties": {
                "issue_number": {"type": "integer", "minimum": 1, "description": "GitHub issue number to edit."},
                "title": {"type": "string", "minLength": 1, "maxLength": 100, "description": "Optional replacement title."},
                "description": {"type": "string", "minLength": 1, "description": "Optional replacement feature description."}
            },
            "required": ["issue_number"],
            "anyOf": [
                {"required": ["title"]},
                {"required": ["description"]}
            ]
        }
    })
}

pub fn default_rate_limiter() -> RateLimiter {
    RateLimiter::new(RATE_LIMIT_MAX_REQUESTS, RATE_LIMIT_WINDOW)
}

/// Extract the Discord requester ID from current or legacy bot-filed issue metadata.
fn requester_from_body(body: &str) -> Option<&str> {
    if !body.trim_end().ends_with(FILED_FOOTER) {
        return None;
    }
    body.lines()
        .find_map(|line| {
            line.strip_prefix("**Discord user ID:** ")
                .or_else(|| line.strip_prefix("**Requested by:** "))
        })
        .filter(|requester| !requester.is_empty())
}

/// Replace only the description while preserving immutable requester metadata.
fn replace_description(body: &str, description: &str) -> Option<String> {
    let (prefix, rest) = body.split_once(DESCRIPTION_HEADING)?;
    let footer_start = rest.rfind("\n\n---\n*Filed automatically by house-chatbot*")?;
    Some(format!(
        "{prefix}{DESCRIPTION_HEADING}{}{}",
        description.trim(),
        &rest[footer_start..]
    ))
}

/// Edit a bot-filed request after verifying that it belongs to `requested_by`.
pub async fn edit_feature_request(
    reporter: &GitHubIssueReporter,
    limiter: &RateLimiter,
    issue_number: u64,
    title: Option<&str>,
    description: Option<&str>,
    requested_by: &str,
) -> String {
    if !reporter.is_configured() {
        return "Error: GitHub integration is not configured — issue was not edited.".to_string();
    }
    if issue_number == 0 {
        return "Error: issue number must be greater than zero.".to_string();
    }
    let title = title.map(str::trim).filter(|value| !value.is_empty());
    let description = description.map(str::trim).filter(|value| !value.is_empty());
    if title.is_none() && description.is_none() {
        return "Error: provide a non-empty title or description to edit.".to_string();
    }
    if title.is_some_and(|value| value.chars().count() > 100) {
        return "Error: issue titles must be 100 characters or fewer.".to_string();
    }
    if limiter.check(requested_by) {
        return format!(
            "Error: rate limit exceeded — you can edit at most {RATE_LIMIT_MAX_REQUESTS} reports every {} minutes. Please try again later.",
            RATE_LIMIT_WINDOW.as_secs() / 60
        );
    }

    let Some(issue) = reporter.fetch_issue(issue_number).await else {
        return "Error: GitHub issue could not be found or retrieved.".to_string();
    };
    let Some(body) = issue.body.as_deref() else {
        return "Error: you can only edit reports that you created through this bot.".to_string();
    };
    if requester_from_body(body) != Some(requested_by) {
        return "Error: you can only edit reports that you created through this bot.".to_string();
    }

    let updated_body = match description {
        Some(description) => match replace_description(body, description) {
            Some(body) => Some(body),
            None => return "Error: this report has an unsupported body format.".to_string(),
        },
        None => None,
    };
    match reporter
        .update_issue(issue_number, title, updated_body.as_deref())
        .await
    {
        Some(url) => format!("Report updated: {url}"),
        None => "Error: Failed to update GitHub issue — check bot logs for details.".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::feature_request::{build_body, RequestType};

    #[test]
    fn definition_requires_issue_number_and_an_edit() {
        let definition = definition();
        assert_eq!(definition["name"], "edit_feature_request");
        assert_eq!(
            definition["input_schema"]["required"],
            json!(["issue_number"])
        );
        assert_eq!(
            definition["input_schema"]["anyOf"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
    }

    #[test]
    fn requester_is_read_only_from_bot_filed_body() {
        let body = build_body("Add dark mode", RequestType::Feature, "alice", "42");
        assert_eq!(requester_from_body(&body), Some("42"));
        assert_eq!(requester_from_body("**Requested by:** user42"), None);
        let legacy = "## Feature Request\n\n**Requested by:** user42\n\n---\n*Filed automatically by house-chatbot*";
        assert_eq!(requester_from_body(legacy), Some("user42"));
    }

    #[test]
    fn description_replacement_preserves_requester_and_footer() {
        let body = build_body("Old description", RequestType::Bug, "alice", "42");
        let updated = replace_description(&body, "New description").unwrap();
        assert_eq!(requester_from_body(&updated), Some("42"));
        assert!(updated.contains("**Discord username:** alice"));
        assert!(updated.contains("## Bug Report"));
        assert!(updated.contains("## Description\n\nNew description"));
        assert!(!updated.contains("Old description"));
        assert!(updated.ends_with(FILED_FOOTER));
    }

    #[tokio::test]
    async fn unconfigured_reporter_returns_error() {
        let reporter =
            GitHubIssueReporter::new(String::new(), String::new(), String::new(), String::new());
        let output = edit_feature_request(
            &reporter,
            &default_rate_limiter(),
            46,
            Some("New title"),
            None,
            "user42",
        )
        .await;
        assert!(output.starts_with("Error:"));
        assert!(output.contains("not configured"));
    }

    #[tokio::test]
    async fn zero_issue_number_is_rejected_before_network_access() {
        let reporter = GitHubIssueReporter::new(
            "app".into(),
            "key".into(),
            "installation".into(),
            "owner/repo".into(),
        );
        let output = edit_feature_request(
            &reporter,
            &default_rate_limiter(),
            0,
            Some("New title"),
            None,
            "user42",
        )
        .await;
        assert_eq!(output, "Error: issue number must be greater than zero.");
    }
}
