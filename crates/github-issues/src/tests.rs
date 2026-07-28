//! Unit tests for configuration, merge-outcome mapping, and formatting.

use crate::auth::build_claims;
use crate::merge::merge_outcome;
use crate::*;

#[test]
fn not_configured_when_fields_missing() {
    let r = GitHubIssueReporter::new(String::new(), String::new(), String::new(), String::new());
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
    let r = GitHubIssueReporter::new("123".into(), "key".into(), "".into(), "owner/repo".into());
    assert!(!r.is_configured());
}

#[tokio::test]
async fn create_issue_returns_none_when_unconfigured() {
    let r = GitHubIssueReporter::new(String::new(), String::new(), String::new(), String::new());
    assert!(r.create_issue("t", "b", &["bug"]).await.is_none());
    assert!(r.create_error_issue("evt123").await.is_none());
}

#[tokio::test]
async fn merge_pull_request_reports_missing_configuration() {
    let r = GitHubIssueReporter::new(String::new(), String::new(), String::new(), String::new());
    let outcome = r.merge_pull_request(5, "merge").await;
    assert_eq!(outcome.status(), "error");
    assert!(matches!(outcome, MergeOutcome::Error(msg) if msg.contains("not configured")));
}

#[tokio::test]
async fn merge_pull_request_validates_arguments_before_calling_github() {
    let r = GitHubIssueReporter::with_direct_token("token".into(), "owner/repo".into());
    assert!(
        matches!(r.merge_pull_request(0, "merge").await, MergeOutcome::Error(msg) if msg.contains("pull request number"))
    );
    assert!(
        matches!(r.merge_pull_request(7, "fast-forward").await, MergeOutcome::Error(msg) if msg.contains("unsupported merge_method"))
    );
}

#[test]
fn merge_outcome_maps_github_status_codes() {
    assert_eq!(
        merge_outcome(
            200,
            r#"{"sha":"abc123","merged":true,"message":"Pull Request successfully merged"}"#
        ),
        MergeOutcome::Merged {
            sha: "abc123".into(),
            message: "Pull Request successfully merged".into()
        }
    );
    assert_eq!(
        merge_outcome(409, r#"{"message":"Head branch was modified"}"#),
        MergeOutcome::Conflict("Head branch was modified".into())
    );
    assert_eq!(
        merge_outcome(405, r#"{"message":"Pull Request is not mergeable"}"#),
        MergeOutcome::Blocked("Pull Request is not mergeable".into())
    );
    assert_eq!(
        merge_outcome(
            403,
            r#"{"message":"Resource not accessible by integration"}"#
        ),
        MergeOutcome::NotPermitted("Resource not accessible by integration".into())
    );
    assert_eq!(
        merge_outcome(404, r#"{"message":"Not Found"}"#),
        MergeOutcome::NotFound("Not Found".into())
    );
    assert!(
        matches!(merge_outcome(500, "<html>"), MergeOutcome::Error(msg) if msg.contains("HTTP 500"))
    );
}

#[test]
fn merge_outcome_status_words_are_stable() {
    assert_eq!(
        MergeOutcome::Merged {
            sha: "a".into(),
            message: "m".into()
        }
        .status(),
        "success"
    );
    assert_eq!(MergeOutcome::Conflict("c".into()).status(), "conflict");
    assert_eq!(MergeOutcome::Blocked("b".into()).status(), "blocked");
    assert_eq!(
        MergeOutcome::NotPermitted("p".into()).status(),
        "not_permitted"
    );
    assert_eq!(MergeOutcome::NotFound("n".into()).status(), "not_found");
    assert_eq!(MergeOutcome::Error("e".into()).status(), "error");
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
    let r = GitHubIssueReporter::new(String::new(), String::new(), String::new(), String::new());
    assert!(!r.close_issue(42).await);
}

#[tokio::test]
async fn get_issue_detail_returns_none_when_unconfigured() {
    let r = GitHubIssueReporter::new(String::new(), String::new(), String::new(), String::new());
    assert!(r.get_issue_detail(42).await.is_none());
}

#[tokio::test]
async fn add_labels_returns_false_when_unconfigured() {
    let r = GitHubIssueReporter::new(String::new(), String::new(), String::new(), String::new());
    assert!(!r.add_labels(42, &["bug"]).await);
}

#[tokio::test]
async fn remove_labels_returns_false_when_unconfigured() {
    let r = GitHubIssueReporter::new(String::new(), String::new(), String::new(), String::new());
    assert!(!r.remove_labels(42, &["bug"]).await);
}

#[tokio::test]
async fn prune_issues_returns_not_configured_when_unconfigured() {
    let r = GitHubIssueReporter::new(String::new(), String::new(), String::new(), String::new());
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
