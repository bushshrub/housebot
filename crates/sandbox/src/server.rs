//! Server-side implementation — runs inside `sandboxd`.
//!
//! This module owns Docker access.  It:
//!   - Listens on a Unix socket.
//!   - Parses incoming `SandboxRequest`s.
//!   - Constructs Docker commands via the `docker` module.
//!   - Spawns `docker` as a subprocess and collects output.
//!   - Manages the lifecycle of sandbox containers.
//!   - Removes stale containers on startup.

use std::collections::HashMap;
use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::process::Command;
use tokio::sync::Mutex;

use crate::docker;
use crate::limits;
use crate::protocol::*;

type ContainerMap = Arc<Mutex<HashMap<String, ContainerState>>>;

#[allow(dead_code)]
struct ContainerState {
    container_name: String,
    network: NetworkAccess,
    created_at: std::time::Instant,
}

/// Run the sandboxd daemon.
///
/// Blocks forever, listening on `socket_path`. Call with `tokio::spawn` or as
/// a `tokio::main` entrypoint.
pub async fn run_daemon(socket_path: &str) -> anyhow::Result<()> {
    // Remove stale socket
    if Path::new(socket_path).exists() {
        std::fs::remove_file(socket_path)?;
    }

    // Ensure parent directory exists
    if let Some(parent) = Path::new(socket_path).parent() {
        tokio::fs::create_dir_all(parent).await?;
    }

    // Clean stale sandbox containers
    cleanup_stale_containers().await;

    let listener = UnixListener::bind(socket_path)?;
    tracing::info!(socket_path, "sandboxd listening");

    let containers: ContainerMap = Arc::new(Mutex::new(HashMap::new()));

    loop {
        let (stream, _addr) = listener.accept().await?;
        let containers = Arc::clone(&containers);
        tokio::spawn(async move {
            if let Err(e) = handle_connection(stream, containers).await {
                tracing::error!("connection handler error: {e}");
            }
        });
    }
}

async fn handle_connection(mut stream: UnixStream, containers: ContainerMap) -> anyhow::Result<()> {
    let (reader, mut writer) = stream.split();

    let mut buf_reader = BufReader::new(reader);
    let mut line = String::new();
    buf_reader.read_line(&mut line).await?;

    if line.trim().is_empty() {
        return Ok(());
    }

    let request: SandboxRequest = serde_json::from_str(line.trim())?;

    let response = process_request(&request, &containers).await;

    let response_line = serde_json::to_string(&response)?;
    let mut bytes = response_line.into_bytes();
    bytes.push(b'\n');
    writer.write_all(&bytes).await?;
    writer.shutdown().await.ok();

    Ok(())
}

async fn process_request(request: &SandboxRequest, containers: &ContainerMap) -> SandboxResponse {
    let id = &request.id;

    match request.method.as_str() {
        "start" => handle_start(id, &request.params, containers).await,
        "clone_repository" => handle_clone_repository(id, &request.params, containers).await,
        "list_files" => handle_list_files(id, &request.params, containers).await,
        "search_code" => handle_search_code(id, &request.params, containers).await,
        "read_file" => handle_read_file(id, &request.params, containers).await,
        "run" => handle_run(id, &request.params, containers).await,
        "close" => handle_close(id, &request.params, containers).await,
        _ => SandboxResponse::err(id.clone(), format!("Unknown method: {}", request.method)),
    }
}

fn get_sandbox_id(params: &serde_json::Value) -> Result<String, String> {
    params
        .get("sandbox_id")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| "missing sandbox_id".to_string())
}

async fn require_sandbox<'a>(
    containers: &'a ContainerMap,
    sandbox_id: &str,
) -> Result<tokio::sync::MutexGuard<'a, HashMap<String, ContainerState>>, String> {
    let map = containers.lock().await;
    if !map.contains_key(sandbox_id) {
        return Err(format!("unknown sandbox: {sandbox_id}"));
    }
    Ok(map)
}

// ── Handlers ────────────────────────────────────────────────────────────────

async fn handle_start(
    id: &str,
    params: &serde_json::Value,
    containers: &ContainerMap,
) -> SandboxResponse {
    let start_params: StartParams = match serde_json::from_value(params.clone()) {
        Ok(p) => p,
        Err(e) => return SandboxResponse::err(id.to_string(), format!("invalid params: {e}")),
    };

    let sandbox_id = uuid::Uuid::new_v4().to_string();
    let args = docker::build_run_args(&sandbox_id, start_params.network);

    // Ensure the sandbox network exists (for public-internet mode)
    if start_params.network == NetworkAccess::PublicInternet {
        let net_args = vec![
            "network".to_string(),
            "create".to_string(),
            "--driver".to_string(),
            "bridge".to_string(),
            "housebot-sandbox-net".to_string(),
        ];
        // Ignore error if the network already exists
        let _ = run_docker(&net_args, 30).await;
    }

    let output = match run_docker(&args, 60).await {
        Ok(o) => o,
        Err(e) => return SandboxResponse::err(id.to_string(), format!("docker run failed: {e}")),
    };

    let container_id = output.trim().to_string();
    if container_id.is_empty() {
        return SandboxResponse::err(
            id.to_string(),
            "docker run produced no container ID".to_string(),
        );
    }

    let container_name = format!("housebot-sandbox-{sandbox_id}");

    let mut map = containers.lock().await;
    map.insert(
        sandbox_id.clone(),
        ContainerState {
            container_name: container_name.clone(),
            network: start_params.network,
            created_at: std::time::Instant::now(),
        },
    );

    SandboxResponse::ok(
        id.to_string(),
        serde_json::json!({"sandbox_id": sandbox_id}),
    )
}

async fn handle_clone_repository(
    id: &str,
    params: &serde_json::Value,
    containers: &ContainerMap,
) -> SandboxResponse {
    let clone_params: CloneRepositoryParams = match serde_json::from_value(params.clone()) {
        Ok(p) => p,
        Err(e) => return SandboxResponse::err(id.to_string(), format!("invalid params: {e}")),
    };

    let sandbox_id = clone_params.sandbox_id.clone();
    let container_name = {
        let guard = match require_sandbox(containers, &sandbox_id).await {
            Ok(s) => s,
            Err(e) => return SandboxResponse::err(id.to_string(), e),
        };
        guard
            .get(&sandbox_id)
            .map(|s| s.container_name.clone())
            .unwrap_or_default()
    };

    let dest = "/workspace/repo";
    let args = docker::build_git_clone_args(
        &container_name,
        &clone_params.url,
        dest,
        clone_params.branch.as_deref(),
    );

    match run_docker_with_timeout(&args, limits::TEST_TIMEOUT_SECS).await {
        Ok(output) => SandboxResponse::ok(
            id.to_string(),
            serde_json::to_value(CommandResult {
                exit_code: 0,
                stdout: output,
                stderr: String::new(),
                truncated: false,
            })
            .unwrap_or_default(),
        ),
        Err(e) => SandboxResponse::err(id.to_string(), e),
    }
}

async fn handle_list_files(
    id: &str,
    params: &serde_json::Value,
    containers: &ContainerMap,
) -> SandboxResponse {
    let list_params: ListFilesParams = match serde_json::from_value(params.clone()) {
        Ok(p) => p,
        Err(e) => return SandboxResponse::err(id.to_string(), format!("invalid params: {e}")),
    };

    let sandbox_id = list_params.sandbox_id.clone();
    let container_name = {
        let guard = match require_sandbox(containers, &sandbox_id).await {
            Ok(s) => s,
            Err(e) => return SandboxResponse::err(id.to_string(), e),
        };
        guard
            .get(&sandbox_id)
            .map(|s| s.container_name.clone())
            .unwrap_or_default()
    };

    let max_depth = list_params.max_depth.unwrap_or(3);
    let cmd = format!(
        "find {} -maxdepth {} -not -path '*/.git/*' -not -path '*/target/*' -not -path '*/node_modules/*' -printf '%y %s %p\\n' 2>/dev/null | head -{}",
        shell_escape_path(&list_params.path),
        max_depth,
        limits::MAX_FILE_LIST_ENTRIES
    );

    let args = docker::build_exec_args(&container_name, &cmd, None);

    match run_docker_with_timeout(&args, limits::DEFAULT_COMMAND_TIMEOUT_SECS).await {
        Ok(output) => {
            let mut entries = Vec::new();
            for line in output.lines() {
                let parts: Vec<&str> = line.splitn(3, ' ').collect();
                if parts.len() >= 3 {
                    let entry_type = match parts[0] {
                        "f" => "file",
                        "d" => "dir",
                        _ => "other",
                    };
                    let size = parts[1].parse::<i64>().ok();
                    let name = parts[2..].join(" ").to_string();
                    entries.push(FileEntry {
                        name,
                        entry_type: entry_type.to_string(),
                        size,
                    });
                }
            }
            SandboxResponse::ok(
                id.to_string(),
                serde_json::to_value(entries).unwrap_or_default(),
            )
        }
        Err(e) => SandboxResponse::err(id.to_string(), e),
    }
}

async fn handle_search_code(
    id: &str,
    params: &serde_json::Value,
    containers: &ContainerMap,
) -> SandboxResponse {
    let search_params: SearchCodeParams = match serde_json::from_value(params.clone()) {
        Ok(p) => p,
        Err(e) => return SandboxResponse::err(id.to_string(), format!("invalid params: {e}")),
    };

    let sandbox_id = search_params.sandbox_id.clone();
    let container_name = {
        let guard = match require_sandbox(containers, &sandbox_id).await {
            Ok(s) => s,
            Err(e) => return SandboxResponse::err(id.to_string(), e),
        };
        guard
            .get(&sandbox_id)
            .map(|s| s.container_name.clone())
            .unwrap_or_default()
    };

    let search_path = search_params
        .path
        .unwrap_or_else(|| "/workspace".to_string());
    let mut rg_cmd = format!(
        "rg --line-number --max-count {} --no-heading",
        limits::MAX_SEARCH_MATCHES
    );

    if let Some(ref glob) = search_params.glob {
        rg_cmd.push_str(&format!(" --glob '{}'", glob.replace('\'', "'\\''")));
    }

    let escaped_query = search_params.query.replace('\'', "'\\''");
    rg_cmd.push_str(&format!(" -e '{}'", escaped_query));
    rg_cmd.push_str(&format!(" '{}'", shell_escape_path(&search_path)));

    let args = docker::build_exec_args(&container_name, &rg_cmd, None);

    match run_docker_with_timeout(&args, limits::DEFAULT_COMMAND_TIMEOUT_SECS).await {
        Ok(output) => {
            let mut matches = Vec::new();
            let mut truncated = false;
            for line in output.lines() {
                if matches.len() >= limits::MAX_SEARCH_MATCHES {
                    truncated = true;
                    break;
                }
                let parts: Vec<&str> = line.splitn(3, ':').collect();
                if parts.len() >= 3 {
                    matches.push(SearchMatch {
                        path: parts[0].to_string(),
                        line_number: parts[1].parse().unwrap_or(0),
                        line: parts[2..].join(":").to_string(),
                    });
                } else if parts.len() == 2 {
                    matches.push(SearchMatch {
                        path: parts[0].to_string(),
                        line_number: parts[1].parse().unwrap_or(0),
                        line: String::new(),
                    });
                }
            }
            SandboxResponse::ok(
                id.to_string(),
                serde_json::to_value(SearchResult { matches, truncated }).unwrap_or_default(),
            )
        }
        Err(e) => SandboxResponse::err(id.to_string(), e),
    }
}

async fn handle_read_file(
    id: &str,
    params: &serde_json::Value,
    containers: &ContainerMap,
) -> SandboxResponse {
    let read_params: ReadFileParams = match serde_json::from_value(params.clone()) {
        Ok(p) => p,
        Err(e) => return SandboxResponse::err(id.to_string(), format!("invalid params: {e}")),
    };

    let sandbox_id = read_params.sandbox_id.clone();
    let container_name = {
        let guard = match require_sandbox(containers, &sandbox_id).await {
            Ok(s) => s,
            Err(e) => return SandboxResponse::err(id.to_string(), e),
        };
        guard
            .get(&sandbox_id)
            .map(|s| s.container_name.clone())
            .unwrap_or_default()
    };

    // Guard against symlink escape and directories
    let check_cmd = format!(
        "cd /workspace && test -L {} && echo SYMLINK || (test -f {} && echo FILE || echo NOT_FILE)",
        shell_escape_path(&read_params.path),
        shell_escape_path(&read_params.path),
    );

    let check_args = docker::build_exec_args(&container_name, &check_cmd, None);
    match run_docker_with_timeout(&check_args, 10).await {
        Ok(check_out) => {
            let check = check_out.trim();
            if check == "SYMLINK" {
                return SandboxResponse::err(
                    id.to_string(),
                    "refusing to read symlink: path escapes".to_string(),
                );
            }
            if check != "FILE" {
                return SandboxResponse::err(
                    id.to_string(),
                    "path is not a regular file".to_string(),
                );
            }
        }
        Err(e) => {
            return SandboxResponse::err(id.to_string(), format!("path check failed: {e}"));
        }
    }

    let cmd = if let (Some(start), Some(end)) = (read_params.start_line, read_params.end_line) {
        if start > end || end - start > limits::MAX_FILE_READ_LINES as u32 {
            return SandboxResponse::err(id.to_string(), "line range exceeds maximum".to_string());
        }
        format!(
            r#"sed -n '{};{}p' "/workspace/{}" 2>/dev/null | head -c {}"#,
            start,
            end,
            read_params.path.replace('"', "\\\""),
            limits::MAX_FILE_READ_BYTES
        )
    } else {
        format!(
            r#"head -c {} "/workspace/{}" 2>/dev/null"#,
            limits::MAX_FILE_READ_BYTES,
            read_params.path.replace('"', "\\\"")
        )
    };

    let args = docker::build_exec_args(&container_name, &cmd, None);

    match run_docker_with_timeout(&args, limits::DEFAULT_COMMAND_TIMEOUT_SECS).await {
        Ok(output) => {
            let truncated = output.len() >= limits::MAX_FILE_READ_BYTES;
            let line_count = output.lines().count();
            let binary = output.contains('\0');
            SandboxResponse::ok(
                id.to_string(),
                serde_json::to_value(FileContents {
                    contents: output,
                    truncated,
                    binary,
                    line_count,
                })
                .unwrap_or_default(),
            )
        }
        Err(e) => SandboxResponse::err(id.to_string(), e),
    }
}

async fn handle_run(
    id: &str,
    params: &serde_json::Value,
    containers: &ContainerMap,
) -> SandboxResponse {
    let run_params: RunParams = match serde_json::from_value(params.clone()) {
        Ok(p) => p,
        Err(e) => return SandboxResponse::err(id.to_string(), format!("invalid params: {e}")),
    };

    let sandbox_id = run_params.sandbox_id.clone();
    let timeout = run_params
        .timeout_secs
        .unwrap_or(limits::DEFAULT_COMMAND_TIMEOUT_SECS)
        .min(limits::ABSOLUTE_MAX_TIMEOUT_SECS);

    // Extract container name while holding the lock, then release it
    let container_name = {
        let guard = match require_sandbox(containers, &sandbox_id).await {
            Ok(s) => s,
            Err(e) => return SandboxResponse::err(id.to_string(), e),
        };
        guard
            .get(&sandbox_id)
            .map(|s| s.container_name.clone())
            .unwrap_or_default()
    };

    let args = docker::build_exec_args(
        &container_name,
        &run_params.command,
        run_params.working_dir.as_deref(),
    );

    match run_docker_with_timeout_raw(&args, timeout).await {
        Ok((stdout, stderr, exit_code)) => {
            let (stdout, truncated) = truncate_output(stdout);
            SandboxResponse::ok(
                id.to_string(),
                serde_json::to_value(CommandResult {
                    exit_code,
                    stdout,
                    stderr: truncate_output_raw(stderr).0,
                    truncated,
                })
                .unwrap_or_default(),
            )
        }
        Err(e) => {
            // Check if it was a timeout
            if e.contains("timed out") {
                // Destroy the container on timeout
                let _ = destroy_container(&container_name).await;
                // Remove from our map
                let mut map = containers.lock().await;
                map.remove(&sandbox_id);
                SandboxResponse::err(
                    id.to_string(),
                    format!("command timed out ({timeout}s) and container was destroyed"),
                )
            } else {
                SandboxResponse::err(id.to_string(), e)
            }
        }
    }
}

async fn handle_close(
    id: &str,
    params: &serde_json::Value,
    containers: &ContainerMap,
) -> SandboxResponse {
    let sandbox_id = match get_sandbox_id(params) {
        Ok(s) => s,
        Err(e) => return SandboxResponse::err(id.to_string(), e),
    };

    let mut map = containers.lock().await;
    match map.remove(&sandbox_id) {
        Some(state) => {
            let _ = destroy_container(&state.container_name).await;
            SandboxResponse::ok(id.to_string(), serde_json::json!({"closed": true}))
        }
        None => SandboxResponse::err(id.to_string(), format!("unknown sandbox: {sandbox_id}")),
    }
}

// ── Docker process helpers ───────────────────────────────────────────────────

/// Run a docker command and return stdout.
async fn run_docker(args: &[String], timeout_secs: u64) -> Result<String, String> {
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(timeout_secs),
        Command::new("docker")
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output(),
    )
    .await
    .map_err(|_| format!("docker command timed out after {timeout_secs}s"))?
    .map_err(|e| format!("failed to execute docker: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("docker command failed: {stderr}"));
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// Run a docker command and return (stdout, stderr, exit_code) with timeout.
async fn run_docker_with_timeout_raw(
    args: &[String],
    timeout_secs: u64,
) -> Result<(String, String, i32), String> {
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(timeout_secs),
        Command::new("docker")
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output(),
    )
    .await
    .map_err(|_| format!("command timed out after {timeout_secs}s"))?
    .map_err(|e| format!("failed to execute docker: {e}"))?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let exit_code = output.status.code().unwrap_or(-1);

    Ok((stdout, stderr, exit_code))
}

/// Run a docker command and return stdout only (for commands where we only care about success).
async fn run_docker_with_timeout(args: &[String], timeout_secs: u64) -> Result<String, String> {
    let (stdout, stderr, exit_code) = run_docker_with_timeout_raw(args, timeout_secs).await?;
    if exit_code != 0 {
        return Err(format!("command exited with code {exit_code}: {stderr}",));
    }
    Ok(stdout)
}

fn truncate_output(output: String) -> (String, bool) {
    if output.len() > limits::MAX_OUTPUT_BYTES {
        let mut truncated = output;
        truncated.truncate(limits::MAX_OUTPUT_BYTES);
        (truncated, true)
    } else {
        (output, false)
    }
}

fn truncate_output_raw(output: String) -> (String, bool) {
    truncate_output(output)
}

async fn destroy_container(container_name: &str) -> Result<(), String> {
    let args = docker::build_remove_args(container_name);
    let _ = run_docker(&args, 30).await;
    Ok(())
}

/// Escape a path for safe use in a shell command (wraps in single quotes).
fn shell_escape_path(path: &str) -> String {
    format!("'{}'", path.replace('\'', "'\\''"))
}

/// Remove all containers with sandbox labels at startup.
pub async fn cleanup_stale_containers() {
    let args = docker::build_list_sandbox_containers_args();
    match run_docker(&args, 30).await {
        Ok(output) => {
            for line in output.lines() {
                let parts: Vec<&str> = line.splitn(2, ' ').collect();
                if parts.len() == 2 {
                    let (_cid, name) = (parts[0], parts[1]);
                    tracing::info!("removing stale sandbox container: {name}");
                    let _ = destroy_container(name).await;
                }
            }
        }
        Err(e) => {
            tracing::warn!("failed to list stale sandbox containers: {e}");
        }
    }
}
