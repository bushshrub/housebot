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
use housebot_sandbox::{server, SandboxClient};

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
        joined.contains("--runtime="),
        "must always include a --runtime= flag"
    );
    if std::env::var("HOUSEBOT_SANDBOX_RUNTIME").is_err() {
        assert!(
            joined.contains("--runtime=kata"),
            "default runtime must be kata (Kata Containers 2.x)"
        );
    }
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

// ══════════════════════════════════════════════════════════════════════════════
// Docker lifecycle tests
//
// These tests start a real sandboxd instance and exercise the full
// client → sandboxd → Docker → container path.
//
// Requirements:
//   - Docker daemon running and accessible
//   - Sandbox image built and tagged as ghcr.io/bushshrub/housebot/sandbox:latest
//   - In CI: set HOUSEBOT_SANDBOX_RUNTIME=runc (Kata not available on hosted runners)
//   - In production: leave HOUSEBOT_SANDBOX_RUNTIME unset to use kata
//
// Run:
//   cargo test --package housebot-sandbox -- --include-ignored --test-threads=1
// ══════════════════════════════════════════════════════════════════════════════

fn test_socket(tag: &str) -> String {
    format!("/tmp/housebot-test-{}-{}.sock", tag, std::process::id())
}

async fn spawn_sandboxd(socket: &str) {
    let socket = socket.to_string();
    tokio::spawn(async move {
        server::run_daemon(&socket).await.ok();
    });
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;
}

#[tokio::test]
#[ignore = "requires Docker daemon + sandbox image; set HOUSEBOT_SANDBOX_RUNTIME=runc in CI"]
async fn docker_sandbox_run_command() {
    let socket = test_socket("run");
    spawn_sandboxd(&socket).await;

    let client = SandboxClient::new(&socket);
    let sandbox = client
        .start(NetworkAccess::None)
        .await
        .expect("start failed");

    let result = sandbox
        .run("echo integration-ok", None, None)
        .await
        .expect("run failed");
    assert_eq!(result.exit_code, 0, "echo should exit 0");
    assert!(
        result.stdout.contains("integration-ok"),
        "stdout: {}",
        result.stdout
    );

    sandbox.close().await.expect("close failed");
}

#[tokio::test]
#[ignore = "requires Docker daemon + sandbox image; set HOUSEBOT_SANDBOX_RUNTIME=runc in CI"]
async fn docker_sandbox_runs_as_non_root() {
    let socket = test_socket("nonroot");
    spawn_sandboxd(&socket).await;

    let client = SandboxClient::new(&socket);
    let sandbox = client
        .start(NetworkAccess::None)
        .await
        .expect("start failed");

    let result = sandbox.run("id -u", None, None).await.expect("id failed");
    assert_eq!(result.exit_code, 0);
    assert_ne!(result.stdout.trim(), "0", "must not run as root");

    sandbox.close().await.expect("close failed");
}

#[tokio::test]
#[ignore = "requires Docker daemon + sandbox image; set HOUSEBOT_SANDBOX_RUNTIME=runc in CI"]
async fn docker_sandbox_no_docker_socket() {
    let socket = test_socket("nosock");
    spawn_sandboxd(&socket).await;

    let client = SandboxClient::new(&socket);
    let sandbox = client
        .start(NetworkAccess::None)
        .await
        .expect("start failed");

    let result = sandbox
        .run(
            "test -e /var/run/docker.sock && echo FOUND || echo ABSENT",
            None,
            None,
        )
        .await
        .expect("check failed");
    assert!(
        result.stdout.contains("ABSENT"),
        "Docker socket must not be mounted into sandbox"
    );

    sandbox.close().await.expect("close failed");
}

#[tokio::test]
#[ignore = "requires Docker daemon + sandbox image; set HOUSEBOT_SANDBOX_RUNTIME=runc in CI"]
async fn docker_sandbox_workspace_is_writable() {
    let socket = test_socket("workspace");
    spawn_sandboxd(&socket).await;

    let client = SandboxClient::new(&socket);
    let sandbox = client
        .start(NetworkAccess::None)
        .await
        .expect("start failed");

    let result = sandbox
        .run(
            "echo hello > /workspace/test.txt && cat /workspace/test.txt",
            None,
            None,
        )
        .await
        .expect("write failed");
    assert_eq!(
        result.exit_code, 0,
        "workspace must be writable: {}",
        result.stderr
    );
    assert!(result.stdout.contains("hello"));

    sandbox.close().await.expect("close failed");
}

#[tokio::test]
#[ignore = "requires Docker daemon + sandbox image; set HOUSEBOT_SANDBOX_RUNTIME=runc in CI"]
async fn docker_sandbox_list_and_read_file() {
    let socket = test_socket("listread");
    spawn_sandboxd(&socket).await;

    let client = SandboxClient::new(&socket);
    let sandbox = client
        .start(NetworkAccess::None)
        .await
        .expect("start failed");

    // Create a file then list and read it
    sandbox
        .run("echo 'fn main() {}' > /workspace/main.rs", None, None)
        .await
        .expect("write failed");

    let entries = sandbox
        .list_files(".", None)
        .await
        .expect("list_files failed");
    assert!(
        entries.iter().any(|e| e.name.contains("main.rs")),
        "main.rs must appear in listing: {entries:?}"
    );

    let contents = sandbox
        .read_file("main.rs", None, None)
        .await
        .expect("read_file failed");
    assert!(
        contents.contents.contains("fn main"),
        "contents: {}",
        contents.contents
    );

    sandbox.close().await.expect("close failed");
}

#[tokio::test]
#[ignore = "requires Docker daemon + sandbox image; set HOUSEBOT_SANDBOX_RUNTIME=runc in CI"]
async fn docker_sandbox_search_code() {
    let socket = test_socket("search");
    spawn_sandboxd(&socket).await;

    let client = SandboxClient::new(&socket);
    let sandbox = client
        .start(NetworkAccess::None)
        .await
        .expect("start failed");

    sandbox
        .run(
            "printf 'fn hello() {}\\nfn world() {}\\n' > /workspace/lib.rs",
            None,
            None,
        )
        .await
        .expect("write failed");

    let result = sandbox
        .search_code("fn hello", None, None)
        .await
        .expect("search_code failed");
    assert!(!result.matches.is_empty(), "search must find matches");
    assert!(
        result.matches.iter().any(|m| m.line.contains("fn hello")),
        "match must contain 'fn hello'"
    );

    sandbox.close().await.expect("close failed");
}

#[tokio::test]
#[ignore = "requires Docker daemon + sandbox image; set HOUSEBOT_SANDBOX_RUNTIME=runc in CI"]
async fn docker_sandbox_nonzero_exit_returned_not_error() {
    let socket = test_socket("nonzero");
    spawn_sandboxd(&socket).await;

    let client = SandboxClient::new(&socket);
    let sandbox = client
        .start(NetworkAccess::None)
        .await
        .expect("start failed");

    let result = sandbox
        .run("exit 42", None, None)
        .await
        .expect("run must succeed even on non-zero exit");
    assert_eq!(result.exit_code, 42, "exit code must be propagated");

    sandbox.close().await.expect("close failed");
}

#[tokio::test]
#[ignore = "requires Docker daemon + sandbox image; set HOUSEBOT_SANDBOX_RUNTIME=runc in CI"]
async fn docker_sandbox_close_removes_container() {
    let socket = test_socket("cleanup");
    spawn_sandboxd(&socket).await;

    let client = SandboxClient::new(&socket);
    let sandbox = client
        .start(NetworkAccess::None)
        .await
        .expect("start failed");
    let id = sandbox.id().to_string();
    let container_name = format!("housebot-sandbox-{id}");

    sandbox.close().await.expect("close failed");

    // Container should no longer exist
    let output = std::process::Command::new("docker")
        .args(["inspect", "--format", "{{.State.Status}}", &container_name])
        .output()
        .expect("docker inspect failed");
    assert!(
        !output.status.success(),
        "container must be removed after close"
    );
}
