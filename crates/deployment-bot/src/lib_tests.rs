//! Unit tests for `lib` (split out to keep the module under 600 lines).

use super::*;

#[test]
fn rollback_is_owner_and_channel_scoped() {
    assert!(rollback_allowed(10, 10, 20, 20));
    assert!(!rollback_allowed(10, 11, 20, 20));
    assert!(!rollback_allowed(10, 10, 21, 20));
    assert!(!rollback_allowed(0, 0, 20, 20));
}

#[test]
fn invalid_numeric_environment_value_is_rejected() {
    std::env::set_var("DEPLOYMENT_BOT_TEST_ID", "not-a-number");
    let error = env_u64("DEPLOYMENT_BOT_TEST_ID").unwrap_err().to_string();
    std::env::remove_var("DEPLOYMENT_BOT_TEST_ID");
    assert!(error.contains("numeric ID"));
}

#[test]
fn optional_guild_id_accepts_unset_and_numeric_values() {
    std::env::remove_var("DEPLOYMENT_BOT_TEST_GUILD_ID");
    assert_eq!(
        optional_env_u64("DEPLOYMENT_BOT_TEST_GUILD_ID").unwrap(),
        None
    );

    std::env::set_var("DEPLOYMENT_BOT_TEST_GUILD_ID", "123456789");
    assert_eq!(
        optional_env_u64("DEPLOYMENT_BOT_TEST_GUILD_ID").unwrap(),
        Some(123456789)
    );
    std::env::remove_var("DEPLOYMENT_BOT_TEST_GUILD_ID");
}

#[test]
fn deployment_webhook_text_is_classified_strictly() {
    assert_eq!(
        classify_deployment_text("HomeLab deployment succeeded"),
        Some(true)
    );
    assert_eq!(
        classify_deployment_text("HomeLab deployment FAILED"),
        Some(false)
    );
    assert_eq!(classify_deployment_text("build succeeded"), Some(true));
    assert_eq!(classify_deployment_text("tests succeeded"), None);
}

#[test]
fn rollback_plan_uses_only_the_checkpoint_digest() {
    let digest = "ghcr.io/bushshrub/housebot@sha256:abc123";
    let commands = container_commands(digest, "network").unwrap();
    assert_eq!(commands.len(), 5);
    assert_eq!(commands[0].stage, DeploymentStage::PullHousebotImage);
    assert_eq!(commands[0].args, vec!["pull", digest]);
    assert_eq!(commands[3].stage, DeploymentStage::StartRequestedImage);
    assert_eq!(commands[3].args.last().unwrap(), digest);
}

#[test]
fn rollback_rejects_tags_and_unrelated_images() {
    assert!(container_commands("ghcr.io/bushshrub/housebot:latest", "network").is_err());
    assert!(container_commands("ghcr.io/other/image@sha256:abc", "network").is_err());
    assert!(container_commands("none", "network").is_err());
}

#[test]
fn deploy_plan_is_sha_scoped_and_rejects_injection() {
    let commands = deploy_commands(Some("abcdef123456"), "network").unwrap();
    assert_eq!(commands.len(), 5);
    assert_eq!(
        commands
            .iter()
            .map(|command| command.stage)
            .collect::<Vec<_>>(),
        vec![
            DeploymentStage::PullHousebotImage,
            DeploymentStage::RunDatabaseMigrations,
            DeploymentStage::RemovePreviousContainer,
            DeploymentStage::StartRequestedImage,
            DeploymentStage::CheckContainerState,
        ]
    );
    assert!(commands[0].args[1].ends_with(":sha-abcdef123456"));
    assert!(!commands[4].args.contains(&"/deployment".to_string()));
    assert_eq!(
        deploy_commands(None, "network").unwrap()[0].args[1],
        "ghcr.io/bushshrub/housebot:latest"
    );
    assert!(deploy_commands(Some("latest"), "network").is_err());
    assert!(deploy_commands(Some("abcdef;reboot"), "network").is_err());
}

#[test]
fn deployment_forwards_persistent_token_monitor_settings() {
    assert!(HOUSEBOT_ENV_VARS.contains(&"DATABASE_URL"));
    assert!(HOUSEBOT_ENV_VARS.contains(&"DATABASE_CONNECT_MAX_ATTEMPTS"));
    assert!(HOUSEBOT_ENV_VARS.contains(&"DATABASE_CONNECT_RETRY_SECS"));
    assert!(HOUSEBOT_ENV_VARS.contains(&"DATABASE_CONNECT_TIMEOUT_SECS"));
}

#[test]
fn completed_deployment_message_includes_container_name_and_id() {
    let summary = DeploymentRunSummary {
        container_name: HOUSE_CHATBOT_CONTAINER.into(),
        container_id: Some("abc123def456".into()),
    };

    let message = summary.completed_message("abcdef123456");

    assert!(message.contains("Container `house-chatbot`"));
    assert!(message.contains("`abc123def456`"));
}

#[test]
fn commit_summary_has_links_messages_and_alternatives() {
    let commit = |sha: &str, message: &str| GitHubCommit {
        sha: sha.into(),
        html_url: format!("https://github.com/example/repo/commit/{sha}"),
        commit: GitHubCommitDetails {
            message: message.into(),
        },
    };
    let selected = commit("abcdef1234", "selected commit\nbody");
    let summary = commit_summary(
        &selected,
        &[selected.clone(), commit("1234567890", "older")],
    );
    assert!(summary.contains("[`abcdef1`](https://github.com/example/repo/commit/abcdef1234)"));
    assert!(summary.contains("selected commit"));
    assert!(summary.contains("older"));
}

#[test]
fn deployment_changelog_lists_commits_since_previous_deployment() {
    let commit = |sha: &str, message: &str| GitHubCommit {
        sha: sha.into(),
        html_url: format!("https://github.com/example/repo/commit/{sha}"),
        commit: GitHubCommitDetails {
            message: message.into(),
        },
    };
    let changelog = deployment_changelog(
        "1111111",
        "3333333",
        &[commit("2222222", "Add deployment visibility\nDetails")],
    );
    assert!(changelog.contains("since `1111111`"));
    assert!(changelog.contains("1 commit"));
    assert!(changelog.contains("Add deployment visibility"));
    assert!(changelog.contains("https://github.com/example/repo/commit/2222222"));
}
