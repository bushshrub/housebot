//! Integration tests for the public deployment-bot command-planning API.

use deployment_bot::{
    classify_deployment_text, commit_summary, container_commands, deploy_commands,
    deployment_changelog, valid_sha, DeploymentStage, GitHubCommit, GitHubCommitDetails,
};

fn commit(sha: &str, message: &str) -> GitHubCommit {
    GitHubCommit {
        sha: sha.into(),
        html_url: format!("https://github.com/example/repo/commit/{sha}"),
        commit: GitHubCommitDetails {
            message: message.into(),
        },
    }
}

#[test]
fn deploy_plan_is_scoped_to_a_valid_sha() {
    let plan = deploy_commands(Some("abcdef1234"), "netz").unwrap();
    let stages: Vec<_> = plan.iter().map(|c| c.stage).collect();
    assert_eq!(
        stages,
        vec![
            DeploymentStage::PullHousebotImage,
            DeploymentStage::RemovePreviousContainer,
            DeploymentStage::StartRequestedImage,
            DeploymentStage::CheckContainerState,
        ]
    );
    assert!(plan[0].args.iter().any(|a| a.ends_with(":sha-abcdef1234")));
}

#[test]
fn deploy_plan_rejects_injection_and_bad_shas() {
    assert!(deploy_commands(Some("abc; rm -rf /"), "netz").is_err());
    assert!(deploy_commands(Some("latest"), "netz").is_err());
    assert!(!valid_sha("xyz"));
    assert!(valid_sha("abcdef1234"));
}

#[test]
fn rollback_only_accepts_digest_pinned_images() {
    assert!(container_commands("ghcr.io/bushshrub/housebot@sha256:deadbeef", "netz").is_ok());
    assert!(container_commands("ghcr.io/bushshrub/housebot:latest", "netz").is_err());
}

#[test]
fn webhook_text_classification_is_strict() {
    assert_eq!(classify_deployment_text("deployment succeeded"), Some(true));
    assert_eq!(classify_deployment_text("deployment failed"), Some(false));
    assert_eq!(classify_deployment_text("nothing happened"), None);
}

#[test]
fn changelog_and_summary_render_commit_links() {
    let selected = commit("1111111aaaa", "Add feature");
    let summary = commit_summary(
        &selected,
        &[selected.clone(), commit("2222222bbbb", "Older")],
    );
    assert!(summary.contains("Add feature"));
    assert!(summary.contains("Older"));

    let changelog = deployment_changelog("1111111", "3333333", &[commit("2222222bbbb", "Ship it")]);
    assert!(changelog.contains("Ship it"));
    assert!(changelog.contains("1 commit"));
}
