//! Integration tests for the sandbox crate.
//!
//! These tests require a running Docker daemon and are marked
//! `#[ignore]` by default.  Run with:
//!
//! ```sh
//! cargo test --package housebot-sandbox -- --include-ignored --test-threads=1
//! ```
//!
//! Or skip Docker tests entirely:
//!
//! ```sh
//! cargo test --package housebot-sandbox -- --skip integration
//! ```

use housebot_sandbox::docker;
use housebot_sandbox::protocol::NetworkAccess;
use housebot_sandbox::validation;

// ══════════════════════════════════════════════════════════════════════════════
// Validation tests (no Docker required)
// ══════════════════════════════════════════════════════════════════════════════

#[test]
fn validate_accepts_github_urls() {
    assert!(validation::validate_repository_url("https://github.com/rust-lang/rust").is_ok());
    assert!(validation::validate_repository_url("https://github.com/user/repo.git").is_ok());
    assert!(validation::validate_repository_url("https://gitlab.com/user/project").is_ok());
}

#[test]
fn validate_rejects_ssh_urls() {
    assert!(validation::validate_repository_url("ssh://git@github.com/user/repo").is_err());
    assert!(validation::validate_repository_url("git://github.com/user/repo").is_err());
}

#[test]
fn validate_rejects_credentials_in_urls() {
    assert!(validation::validate_repository_url("https://token@github.com/repo").is_err());
    assert!(validation::validate_repository_url("https://user:pass@github.com/repo").is_err());
}

#[test]
fn validate_rejects_private_addresses() {
    assert!(validation::validate_repository_url("https://localhost/repo").is_err());
    assert!(validation::validate_repository_url("https://127.0.0.1/repo").is_err());
    assert!(validation::validate_repository_url("https://192.168.1.1/repo").is_err());
    assert!(validation::validate_repository_url("https://10.0.0.1/repo").is_err());
    assert!(validation::validate_repository_url("https://172.16.0.1/repo").is_err());
    assert!(validation::validate_repository_url("https://host.docker.internal/repo").is_err());
}

#[test]
fn validate_rejects_file_urls() {
    assert!(validation::validate_repository_url("file:///etc/passwd").is_err());
}

#[test]
fn validate_rejects_malformed_urls() {
    assert!(validation::validate_repository_url("").is_err());
    assert!(validation::validate_repository_url("not-a-url").is_err());
}

#[test]
fn validate_workspace_path_normal() {
    assert!(validation::validate_workspace_path("src/main.rs").is_ok());
    assert!(validation::validate_workspace_path("src/lib/foo.rs").is_ok());
    assert!(validation::validate_workspace_path("Cargo.toml").is_ok());
    assert!(validation::validate_workspace_path("src").is_ok());
}

#[test]
fn validate_workspace_path_rejects_escape() {
    assert!(validation::validate_workspace_path("..").is_err());
    assert!(validation::validate_workspace_path("../etc/passwd").is_err());
    assert!(validation::validate_workspace_path("src/../../outside").is_err());
}

#[test]
fn validate_workspace_path_rejects_absolute() {
    assert!(validation::validate_workspace_path("/etc/passwd").is_err());
    assert!(validation::validate_workspace_path("/workspace").is_err());
}

#[test]
fn validate_workspace_path_rejects_null() {
    assert!(validation::validate_workspace_path("src\0/main.rs").is_err());
}

#[test]
fn validate_query_normal() {
    assert!(validation::validate_query("fn main").is_ok());
    assert!(validation::validate_query("TODO:").is_ok());
}

#[test]
fn validate_query_rejects_empty() {
    assert!(validation::validate_query("").is_err());
}

#[test]
fn validate_command_normal() {
    assert!(validation::validate_command("ls -la").is_ok());
    assert!(validation::validate_command("echo hello").is_ok());
    assert!(validation::validate_command("cargo test").is_ok());
}

#[test]
fn validate_command_rejects_empty() {
    assert!(validation::validate_command("").is_err());
}

#[test]
fn validate_command_rejects_null() {
    assert!(validation::validate_command("echo\0hello").is_err());
}

#[test]
fn validate_branch_normal() {
    assert!(validation::validate_branch("main").is_ok());
    assert!(validation::validate_branch("feature/my-feature").is_ok());
    assert!(validation::validate_branch("abc123def").is_ok());
}

#[test]
fn validate_branch_rejects_dangerous() {
    assert!(validation::validate_branch("main; rm -rf /").is_err());
    assert!(validation::validate_branch("main\nother").is_err());
    assert!(validation::validate_branch("main\0extra").is_err());
}

// ══════════════════════════════════════════════════════════════════════════════
// Docker argument construction tests
// ══════════════════════════════════════════════════════════════════════════════

#[test]
fn docker_args_all_security_options_present() {
    let args = docker::build_run_args("test-sandbox-id", NetworkAccess::None);
    let joined = args.join(" ");

    assert!(joined.contains("--read-only"), "read-only required");
    assert!(joined.contains("--user=sandbox"), "non-root user required");
    assert!(joined.contains("--cap-drop"), "cap-drop required");
    assert!(
        joined.contains("no-new-privileges"),
        "no-new-privileges required"
    );
    assert!(joined.contains("--pids-limit=128"), "pids limit required");
    assert!(joined.contains("--memory=2g"), "memory limit required");
    assert!(joined.contains("--cpus=1"), "cpus limit required");
    assert!(joined.contains("--ulimit"), "ulimit required");
    assert!(
        joined.contains("--runtime=kata"),
        "kata required for VM-level isolation"
    );
}

#[test]
fn docker_args_no_dangerous_flags() {
    let args = docker::build_run_args("test", NetworkAccess::None);
    let joined = args.join(" ");

    assert!(!joined.contains("--privileged"), "no privileged");
    assert!(!joined.contains("--pid=host"), "no host pid");
    assert!(!joined.contains("--ipc=host"), "no host ipc");
    assert!(!joined.contains("--network=host"), "no host network");
    assert!(!joined.contains("--device"), "no device mounts");
    assert!(!joined.contains("docker.sock"), "no docker socket");
}

#[test]
fn docker_args_tmpfs_mounts_present() {
    let args = docker::build_run_args("test", NetworkAccess::None);
    assert!(
        args.iter().any(|a| a.starts_with("/workspace:")),
        "must have /workspace tmpfs"
    );
    assert!(
        args.iter().any(|a| a.starts_with("/tmp:")),
        "must have /tmp tmpfs"
    );
}

#[test]
fn docker_args_sandbox_label_present() {
    let args = docker::build_run_args("my-id", NetworkAccess::None);
    assert!(
        args.iter().any(|a| a == "com.housebot.sandbox.id=my-id"),
        "must label container with sandbox id"
    );
}

#[test]
fn docker_args_network_isolation() {
    let no_net = docker::build_run_args("test", NetworkAccess::None);
    assert!(
        no_net.iter().any(|a| a == "--network=none"),
        "no-network mode must use --network=none"
    );

    let pub_net = docker::build_run_args("test", NetworkAccess::PublicInternet);
    assert!(
        pub_net
            .iter()
            .any(|a| a == "--network=housebot-sandbox-net"),
        "public internet mode must use sandbox network"
    );
}

#[test]
fn docker_exec_args_correct_structure() {
    let args = docker::build_exec_args("my-container", "cargo test", Some("/workspace/repo"));
    assert_eq!(args[0], "exec", "first arg must be 'exec'");
    assert!(args.contains(&"-w".to_string()), "must have -w flag");
    assert!(
        args.contains(&"/workspace/repo".to_string()),
        "must have working dir"
    );
    assert!(args.contains(&"/bin/bash".to_string()), "must use bash");
    assert!(args.contains(&"-c".to_string()), "must use -c");
    assert!(
        args.contains(&"cargo test".to_string()),
        "must pass command"
    );
}

#[test]
fn docker_remove_args_force() {
    let args = docker::build_remove_args("housebot-sandbox-abc123");
    assert!(args.contains(&"-f".to_string()), "must force remove");
    assert!(
        args.contains(&"housebot-sandbox-abc123".to_string()),
        "must name correct container"
    );
}

#[test]
fn docker_list_args_filter_by_label() {
    let args = docker::build_list_sandbox_containers_args();
    assert!(
        args.iter().any(|a| a.contains("com.housebot.sandbox")),
        "must filter by sandbox label"
    );
}

// ══════════════════════════════════════════════════════════════════════════════
// Limits constants
// ══════════════════════════════════════════════════════════════════════════════

#[test]
fn limits_are_sane() {
    use housebot_sandbox::limits;
    assert!(limits::DEFAULT_COMMAND_TIMEOUT_SECS <= limits::ABSOLUTE_MAX_TIMEOUT_SECS);
    assert!(limits::MAX_OUTPUT_BYTES > 0);
    assert!(limits::MAX_FILE_READ_BYTES > 0);
    assert!(limits::MAX_SEARCH_MATCHES > 0);
    assert!(limits::MAX_FILE_LIST_ENTRIES > 0);
}
