//! Unit tests for `lib` (split out to keep the module under 600 lines).

use super::*;

#[test]
fn deployment_access_command_exposes_allow_revoke_and_list() {
    let commands = serde_json::to_value(deployment_commands()).unwrap();
    let access = commands
        .as_array()
        .unwrap()
        .iter()
        .find(|command| command["name"] == "deployment-access")
        .expect("deployment access command must be registered");
    assert_eq!(
        access["options"]
            .as_array()
            .unwrap()
            .iter()
            .map(|option| option["name"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["allow", "revoke", "list"]
    );
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
    assert_eq!(commands[0].stage, DeploymentStage::PullHousebotImage);
    assert_eq!(commands[0].args, vec!["pull", digest]);
    let start = commands
        .iter()
        .find(|command| command.stage == DeploymentStage::StartRequestedImage)
        .unwrap();
    assert_eq!(start.args.last().unwrap(), digest);
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
    assert_eq!(
        commands
            .iter()
            .map(|command| command.stage)
            .collect::<Vec<_>>(),
        vec![
            DeploymentStage::PullHousebotImage,
            DeploymentStage::PullSandboxDaemonImage,
            DeploymentStage::PullSandboxImage,
            DeploymentStage::RunDatabaseMigrations,
            DeploymentStage::RemovePreviousContainer,
            DeploymentStage::RemovePreviousSandboxDaemon,
            DeploymentStage::CreateSandboxSocketVolume,
            DeploymentStage::StartSandboxDaemon,
            DeploymentStage::CheckSandboxDaemon,
            DeploymentStage::StartRequestedImage,
            DeploymentStage::CheckContainerState,
        ]
    );
    assert!(commands[0].args[1].ends_with(":sha-abcdef123456"));
    assert!(!commands
        .last()
        .unwrap()
        .args
        .contains(&"/deployment".to_string()));
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

/// Variables the chatbot reads that must not be forwarded from the deployment
/// bot's own environment, with the reason each is excluded.
const NOT_FORWARDED: &[&str] = &[
    // Fixed to the container's own path by the run command.
    "DATA_DIR",
    // Read inside the sandboxd container, which gets its own env.
    "HOUSEBOT_SANDBOX_IMAGE",
    "HOUSEBOT_SANDBOX_RUNTIME",
];

fn env_vars_read_by(relative_path: &str) -> std::collections::BTreeSet<String> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(relative_path);
    let mut found = std::collections::BTreeSet::new();
    let mut stack = vec![root];
    while let Some(path) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&path) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if path
                    .file_name()
                    .is_some_and(|name| name == "deployment-bot")
                {
                    continue;
                }
                stack.push(path);
                continue;
            }
            if path.extension().is_none_or(|ext| ext != "rs") {
                continue;
            }
            let Ok(source) = std::fs::read_to_string(&path) else {
                continue;
            };
            for (index, _) in source.match_indices('"') {
                let rest = &source[index + 1..];
                let Some(end) = rest.find('"') else { continue };
                let name = &rest[..end];
                if name.len() > 3
                    && name
                        .bytes()
                        .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
                    && name.starts_with(|c: char| c.is_ascii_uppercase())
                    && source[..index].ends_with(|c: char| c == '(' || c == ' ' || c == '\n')
                {
                    found.insert(name.to_string());
                }
            }
        }
    }
    found
}

/// Restores a variable to what it was, including being unset, when dropped — a
/// bare `remove_var` would drop a value the environment came with, and a panic
/// between set and remove would leak the test's value into the tests that
/// follow.
struct EnvVarGuard {
    name: &'static str,
    previous: Option<String>,
}

impl EnvVarGuard {
    fn set(name: &'static str, value: &str) -> Self {
        let previous = std::env::var(name).ok();
        std::env::set_var(name, value);
        Self { name, previous }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        match &self.previous {
            Some(previous) => std::env::set_var(self.name, previous),
            None => std::env::remove_var(self.name),
        }
    }
}

/// The allowlist is a hand-maintained copy of the chatbot's env surface, so a
/// variable added to the bot and forgotten here is dropped at deploy time with
/// no error anywhere. Jellyfin and llama.cpp entries survived their features
/// this way.
#[test]
fn housebot_env_vars_cover_every_variable_the_bot_reads() {
    let mut source = env_vars_read_by("src");
    source.extend(env_vars_read_by("crates"));

    let missing: Vec<&String> = source
        .iter()
        .filter(|name| {
            name.starts_with("LLM_")
                || name.starts_with("SEARXNG_")
                || name.starts_with("SENTRY_")
                || name.starts_with("SKILLS_")
                || name.starts_with("SANDBOX_")
                || name.starts_with("CHANNEL_CONTEXT_")
                || name.starts_with("CHAT_RATE_LIMIT_")
                || name.starts_with("DEVELOPMENT_")
                || name.starts_with("MAX_")
        })
        .filter(|name| !HOUSEBOT_ENV_VARS.contains(&name.as_str()))
        .filter(|name| !NOT_FORWARDED.contains(&name.as_str()))
        .collect();

    assert!(
        missing.is_empty(),
        "these variables are read by the bot but never forwarded to its container: {missing:?}"
    );
}

/// The mirror of the test above: a variable forwarded for a feature that no
/// longer exists is dead configuration nobody will notice.
#[test]
fn housebot_env_vars_are_all_still_read_somewhere() {
    let mut source = env_vars_read_by("src");
    source.extend(env_vars_read_by("crates"));

    let unread: Vec<&&str> = HOUSEBOT_ENV_VARS
        .iter()
        .filter(|name| !source.contains(**name))
        .collect();

    assert!(
        unread.is_empty(),
        "these variables are forwarded but nothing reads them: {unread:?}"
    );
}

#[test]
fn deployment_passes_database_url_to_migration_and_bot_containers() {
    let url = "postgres://housebot:secret@postgres/housebot";
    let restore = EnvVarGuard::set("DATABASE_URL", url);
    let commands = deploy_commands(Some("abcdef123456"), "network").unwrap();
    drop(restore);

    let args_for = |stage| {
        &commands
            .iter()
            .find(|command| command.stage == stage)
            .unwrap()
            .args
    };
    let migration_args = args_for(DeploymentStage::RunDatabaseMigrations);
    let bot_args = args_for(DeploymentStage::StartRequestedImage);
    let expected = format!("DATABASE_URL={url}");
    let has_database_url = |args: &[String]| {
        args.windows(2)
            .any(|pair| pair[0] == "--env" && pair[1] == expected)
    };
    assert!(has_database_url(migration_args));
    assert!(has_database_url(bot_args));
}

#[test]
fn deployment_runs_migrations_through_the_housebot_binary() {
    let commands = deploy_commands(Some("abcdef123456"), "network").unwrap();
    let migration_args = &commands
        .iter()
        .find(|command| command.stage == DeploymentStage::RunDatabaseMigrations)
        .unwrap()
        .args;

    assert!(migration_args.ends_with(&[
        "ghcr.io/bushshrub/housebot:sha-abcdef123456".into(),
        "housebot".into(),
        "migrate".into(),
    ]));
}

#[test]
fn deployment_starts_sandboxd_sidecar_and_shares_only_its_socket() {
    let commands = deploy_commands(Some("abcdef123456"), "network").unwrap();
    let args_for = |stage| {
        &commands
            .iter()
            .find(|command| command.stage == stage)
            .unwrap()
            .args
    };

    let daemon = args_for(DeploymentStage::StartSandboxDaemon);
    assert!(daemon.contains(&"/var/run/docker.sock:/var/run/docker.sock".to_string()));
    assert!(daemon.contains(&"housebot-sandbox-socket:/run/housebot-sandbox".to_string()));
    assert!(daemon
        .last()
        .unwrap()
        .ends_with("/sandboxd:sha-abcdef123456"));
    assert!(daemon.contains(
        &"HOUSEBOT_SANDBOX_IMAGE=ghcr.io/bushshrub/housebot/sandbox:sha-abcdef123456".to_string()
    ));

    let bot = args_for(DeploymentStage::StartRequestedImage);
    assert!(bot.contains(&"housebot-sandbox-socket:/run/housebot-sandbox".to_string()));
    assert!(!bot.iter().any(|arg| arg.contains("docker.sock")));

    let check = args_for(DeploymentStage::CheckSandboxDaemon);
    assert_eq!(
        check,
        &[
            "exec",
            "housebot-sandboxd",
            "test",
            "-S",
            "/run/housebot-sandbox/sandbox.sock"
        ]
    );
}

/// Without this the sidecar always asks Docker for gVisor, and a host without
/// `runsc` installed cannot start a sandbox container at all.
#[test]
fn deployment_forwards_the_sandbox_runtime_to_the_sidecar() {
    let restore = EnvVarGuard::set("HOUSEBOT_SANDBOX_RUNTIME", "runc");
    let commands = deploy_commands(Some("abcdef123456"), "network").unwrap();
    drop(restore);

    let daemon = &commands
        .iter()
        .find(|command| command.stage == DeploymentStage::StartSandboxDaemon)
        .unwrap()
        .args;
    assert!(daemon.contains(&"HOUSEBOT_SANDBOX_RUNTIME=runc".to_string()));

    let bot = &commands
        .iter()
        .find(|command| command.stage == DeploymentStage::StartRequestedImage)
        .unwrap()
        .args;
    assert!(!bot.iter().any(|arg| arg.starts_with("HOUSEBOT_SANDBOX_")));
}

/// Compose keeps restarting a container it created even after the service is
/// gone from the file, so the duplicate chatbot outlives the compose edit.
#[test]
fn compose_duplicates_are_matched_by_project_and_service_together() {
    assert_eq!(COMPOSE_DUPLICATE_SERVICES, &["housebot", "sandboxd"]);

    let args = compose_duplicate_query("housebot");
    assert!(args.contains(&"label=com.docker.compose.project=house-chatbot".to_string()));
    assert!(args.contains(&"label=com.docker.compose.service=housebot".to_string()));
    assert!(args.contains(&"--all".to_string()));
}

/// Postgres and this bot are still compose's to manage; removing them would
/// take the database and the deployment surface down with the duplicate.
#[test]
fn compose_duplicates_exclude_the_services_this_bot_does_not_own() {
    for service in ["postgres", "deployment-bot"] {
        assert!(
            !COMPOSE_DUPLICATE_SERVICES.contains(&service),
            "{service} must not be removed by the deployment bot"
        );
    }
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

/// The Dockerfile lists every workspace manifest by hand so the dependency
/// layer caches. Nothing checks that list against reality: a crate deleted from
/// the workspace leaves a `COPY` of a path that no longer exists, and the image
/// build fails with "not found" long after the crate was removed.
#[test]
fn deployment_dockerfile_copies_exactly_the_workspace_crates() {
    let workspace = include_str!("../../../Cargo.toml");
    let members: Vec<&str> = workspace
        .split("members = [")
        .nth(1)
        .and_then(|s| s.split(']').next())
        .expect("workspace manifest must declare members")
        .lines()
        .filter_map(|l| {
            l.trim()
                .trim_matches(|c| c == '"' || c == ',')
                .strip_prefix("crates/")
        })
        .collect();

    let dockerfile = include_str!("../../../Dockerfile.deployment-bot");
    let copied: Vec<&str> = dockerfile
        .lines()
        .filter_map(|l| l.strip_prefix("COPY crates/"))
        .filter_map(|l| l.split('/').next())
        .collect();

    for member in &members {
        assert!(
            copied.contains(member),
            "workspace crate '{member}' is missing from Dockerfile.deployment-bot"
        );
    }
    for crate_name in &copied {
        assert!(
            members.contains(crate_name),
            "Dockerfile.deployment-bot copies '{crate_name}', which is not a workspace crate"
        );
    }
}
