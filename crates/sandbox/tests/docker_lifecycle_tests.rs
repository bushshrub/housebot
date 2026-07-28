//! Docker lifecycle tests: client -> sandboxd -> Docker -> container.
//!
//! All `#[ignore]`d. Run with:
//!
//! ```sh
//! cargo test --package housebot-sandbox -- --include-ignored --test-threads=1
//! ```
//!
//! Requires a Docker daemon, the sandbox image tagged as
//! `ghcr.io/bushshrub/housebot/sandbox:latest`, and in CI
//! `HOUSEBOT_SANDBOX_RUNTIME=runc` (gVisor is not available on hosted runners).

use housebot_sandbox::protocol::NetworkAccess;
use housebot_sandbox::{server, SandboxClient};

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
