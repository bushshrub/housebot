//! Live GitHub API tests. All `#[ignore]`d; run explicitly with `--ignored`.

use crate::*;

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
#[ignore = "live GitHub API call; run explicitly with --ignored"]
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
#[ignore = "live GitHub API call; run explicitly with --ignored"]
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
#[ignore = "live GitHub API call; run explicitly with --ignored"]
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
#[ignore = "live GitHub API call; run explicitly with --ignored"]
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
#[ignore = "live GitHub API call; run explicitly with --ignored"]
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
#[ignore = "live GitHub API call; run explicitly with --ignored"]
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
