//! Unit tests for `github_api` (split out to keep the module under 400 lines).

use super::*;
use tempfile::TempDir;

fn admin() -> ToolCaller<'static> {
    ToolCaller {
        user_id: "196556976866459648",
        username: "derp_z",
        is_admin: true,
    }
}

fn member() -> ToolCaller<'static> {
    ToolCaller {
        user_id: "42",
        username: "someone",
        is_admin: false,
    }
}

fn audit_log() -> (TempDir, MergeAuditLog) {
    let temp = TempDir::new().unwrap();
    let log = MergeAuditLog::new(temp.path().join("audit").join("pr_merge_audit.jsonl"));
    (temp, log)
}

async fn audit_entries(temp: &TempDir) -> Vec<Value> {
    let path = temp.path().join("audit").join("pr_merge_audit.jsonl");
    let raw = tokio::fs::read_to_string(path).await.unwrap_or_default();
    raw.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("audit lines are JSON"))
        .collect()
}

#[test]
fn definition_includes_new_actions() {
    let def = definition();
    let actions = def["input_schema"]["properties"]["action"]["enum"]
        .as_array()
        .expect("actions should be an array");
    let names: Vec<&str> = actions.iter().filter_map(|v| v.as_str()).collect();
    assert!(names.contains(&"get_issue"));
    assert!(names.contains(&"close_issue"));
    assert!(names.contains(&"add_labels"));
    assert!(names.contains(&"remove_labels"));
    assert!(names.contains(&"prune_issues"));
    assert!(names.contains(&"list_issues"));
    assert!(names.contains(&"search_issues"));
}

#[tokio::test]
async fn handle_github_api_returns_not_configured() {
    let reporter =
        GitHubIssueReporter::new(String::new(), String::new(), String::new(), String::new());
    let (_temp, audit) = audit_log();
    let result = handle_github_api(
        &reporter,
        "close_issue",
        &json!({"issue_number": 1}),
        &admin(),
        &audit,
    )
    .await;
    assert!(result.contains("not configured"), "got: {result}");
}

#[tokio::test]
async fn handle_github_api_validation_checks_precede_config_check() {
    // With a configured reporter (using direct token), we can test validation
    let reporter = GitHubIssueReporter::with_direct_token("test-token".into(), "owner/repo".into());

    let (_temp, audit) = audit_log();
    let result = handle_github_api(&reporter, "get_issue", &json!({}), &admin(), &audit).await;
    assert!(result.contains("issue_number is required"), "got: {result}");

    let result = handle_github_api(&reporter, "close_issue", &json!({}), &admin(), &audit).await;
    assert!(result.contains("issue_number is required"), "got: {result}");

    let result = handle_github_api(
        &reporter,
        "add_labels",
        &json!({"issue_number": 1, "label_names": ""}),
        &admin(),
        &audit,
    )
    .await;
    assert!(result.contains("label_names is required"), "got: {result}");

    let result = handle_github_api(
        &reporter,
        "prune_issues",
        &json!({"action_value": "invalid_action"}),
        &admin(),
        &audit,
    )
    .await;
    assert!(
        result.contains("requires action_value in format"),
        "got: {result}"
    );

    let result = handle_github_api(&reporter, "nonexistent", &json!({}), &admin(), &audit).await;
    assert!(
        result.contains("unknown github_api action"),
        "got: {result}"
    );
}

#[test]
fn definition_exposes_merge_pull_request() {
    let def = definition();
    let properties = &def["input_schema"]["properties"];
    let actions: Vec<&str> = properties["action"]["enum"]
        .as_array()
        .expect("actions should be an array")
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    assert!(actions.contains(&"merge_pull_request"));
    assert_eq!(properties["pull_request_number"]["type"], "integer");
    let methods: Vec<&str> = properties["merge_method"]["enum"]
        .as_array()
        .expect("merge methods should be an array")
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    assert_eq!(methods, vec!["merge", "squash", "rebase"]);
}

#[tokio::test]
async fn merge_is_refused_for_non_administrators_and_audited() {
    let reporter = GitHubIssueReporter::with_direct_token("test-token".into(), "owner/repo".into());
    let (temp, audit) = audit_log();

    let result = handle_github_api(
        &reporter,
        "merge_pull_request",
        &json!({"pull_request_number": 7}),
        &member(),
        &audit,
    )
    .await;

    assert!(result.contains("permission denied"), "got: {result}");
    let entries = audit_entries(&temp).await;
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["admin_id"], "42");
    assert_eq!(entries[0]["admin_username"], "someone");
    assert_eq!(entries[0]["pull_request"], 7);
    assert_eq!(entries[0]["authorized"], false);
    assert_eq!(entries[0]["result"], "denied");
    assert!(entries[0]["timestamp"]
        .as_str()
        .is_some_and(|ts| ts.contains('T')));
}

#[tokio::test]
async fn merge_validates_arguments_for_administrators() {
    let reporter = GitHubIssueReporter::with_direct_token("test-token".into(), "owner/repo".into());
    let (temp, audit) = audit_log();

    let result = handle_github_api(
        &reporter,
        "merge_pull_request",
        &json!({}),
        &admin(),
        &audit,
    )
    .await;
    assert!(
        result.contains("pull_request_number is required"),
        "got: {result}"
    );

    let result = handle_github_api(
        &reporter,
        "merge_pull_request",
        &json!({"pull_request_number": 7, "merge_method": "fast-forward"}),
        &admin(),
        &audit,
    )
    .await;
    assert!(
        result.contains("merge_method must be one of"),
        "got: {result}"
    );

    // Rejected arguments never reach GitHub, so nothing is audited as an attempt.
    assert!(audit_entries(&temp).await.is_empty());
}

#[tokio::test]
async fn merge_reports_unconfigured_github_and_audits_the_attempt() {
    let reporter =
        GitHubIssueReporter::new(String::new(), String::new(), String::new(), String::new());
    let (temp, audit) = audit_log();

    let result = handle_github_api(
        &reporter,
        "merge_pull_request",
        &json!({"pull_request_number": 12}),
        &admin(),
        &audit,
    )
    .await;

    assert!(result.starts_with("Error:"), "got: {result}");
    assert!(result.contains("not configured"), "got: {result}");
    let entries = audit_entries(&temp).await;
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["admin_id"], "196556976866459648");
    assert_eq!(entries[0]["pull_request"], 12);
    assert_eq!(entries[0]["authorized"], true);
    assert_eq!(entries[0]["result"], "error");
}

#[tokio::test]
async fn audit_log_appends_successive_entries() {
    let (temp, audit) = audit_log();
    audit.record(&admin(), 1, true, "success", "merged").await;
    audit
        .record(&member(), 2, false, "denied", "not admin")
        .await;
    let entries = audit_entries(&temp).await;
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0]["pull_request"], 1);
    assert_eq!(entries[1]["pull_request"], 2);
}
