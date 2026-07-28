//! Server-side implementation — runs inside `sandboxd`.
//!
//! Owns Docker access: listens on a Unix socket, parses `SandboxRequest`s,
//! constructs Docker commands via the `docker` module, and manages container
//! lifecycle. Request handlers live in `server_workspace` and `server_exec`;
//! the Docker subprocess plumbing lives in `server_docker`.

use std::collections::HashMap;
use std::os::unix::fs::FileTypeExt;
use std::path::Path;
use std::sync::Arc;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::Mutex;

use crate::limits;
use crate::protocol::*;
use crate::server_docker::*;
use crate::server_exec::*;
use crate::server_workspace::*;

pub(crate) type ContainerMap = Arc<Mutex<HashMap<String, ContainerState>>>;

#[allow(dead_code)]
pub(crate) struct ContainerState {
    pub(crate) container_name: String,
    pub(crate) network: NetworkAccess,
    pub(crate) created_at: std::time::Instant,
}

/// Run the sandboxd daemon.
///
/// Blocks forever, listening on `socket_path`. Call with `tokio::spawn` or as
/// a `tokio::main` entrypoint.
pub async fn run_daemon(socket_path: &str) -> anyhow::Result<()> {
    // Remove stale socket (refuse to delete non-socket paths)
    if Path::new(socket_path).exists() {
        let meta = std::fs::symlink_metadata(socket_path)?;
        if !meta.file_type().is_socket() {
            anyhow::bail!("socket path exists and is not a Unix socket: {socket_path}");
        }
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

pub(crate) async fn handle_connection(
    mut stream: UnixStream,
    containers: ContainerMap,
) -> anyhow::Result<()> {
    let (reader, mut writer) = stream.split();

    let mut buf_reader = BufReader::new(reader);
    let mut line = String::new();

    tokio::time::timeout(
        std::time::Duration::from_secs(limits::SOCKET_TIMEOUT_SECS),
        buf_reader.read_line(&mut line),
    )
    .await
    .map_err(|_| {
        anyhow::anyhow!(
            "request read timed out after {}s",
            limits::SOCKET_TIMEOUT_SECS
        )
    })?
    .map_err(|e| anyhow::anyhow!("failed to read request: {e}"))?;

    if line.trim().is_empty() {
        return Ok(());
    }

    if line.len() > limits::MAX_REQUEST_FRAME_BYTES {
        anyhow::bail!(
            "request frame too large ({} bytes, max {})",
            line.len(),
            limits::MAX_REQUEST_FRAME_BYTES
        );
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

pub(crate) async fn process_request(
    request: &SandboxRequest,
    containers: &ContainerMap,
) -> SandboxResponse {
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

pub(crate) fn get_sandbox_id(params: &serde_json::Value) -> Result<String, String> {
    params
        .get("sandbox_id")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| "missing sandbox_id".to_string())
}

pub(crate) async fn require_sandbox<'a>(
    containers: &'a ContainerMap,
    sandbox_id: &str,
) -> Result<tokio::sync::MutexGuard<'a, HashMap<String, ContainerState>>, String> {
    let map = containers.lock().await;
    if !map.contains_key(sandbox_id) {
        return Err(format!("unknown sandbox: {sandbox_id}"));
    }
    Ok(map)
}
