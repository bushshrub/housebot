//! Server-side implementation — runs inside `sandboxd`.
//!
//! This module owns container-runtime access.  It:
//!   - Serves requests from the Unix socket and the HTTP API.
//!   - Parses incoming `SandboxRequest`s.
//!   - Builds runtime commands via the `runtime` dispatch.
//!   - Spawns `docker` or `kubectl` as a subprocess and collects output.
//!   - Manages the lifecycle of sandboxes.
//!   - Removes stale sandboxes on startup.

use std::collections::HashMap;
use std::os::unix::fs::FileTypeExt;
use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::process::Command;
use tokio::sync::Mutex;

use crate::kubernetes;
use crate::limits;
use crate::protocol::*;
use crate::runtime::{Invocation, Runtime};
use crate::validation;

type ContainerMap = Arc<Mutex<HashMap<String, ContainerState>>>;

/// How long a newly created sandbox has to reach a usable state. Only the
/// Kubernetes backend schedules, so only it can wait.
const SANDBOX_READY_TIMEOUT_SECS: u64 = 60;

struct ContainerState {
    handle: String,
    session_key: String,
    network: NetworkAccess,
    last_used_at: std::time::Instant,
}

/// The daemon's view of the sandbox tier.
///
/// Under Docker the session index lives in `local`, so one daemon owns its
/// sandboxes. Under Kubernetes the index is the Pod labels themselves and
/// `local` stays empty, which is what lets several replicas serve the same
/// session without coordinating.
pub struct Server {
    runtime: Runtime,
    local: ContainerMap,
    idle_timeout: std::time::Duration,
}

impl Server {
    pub fn from_env() -> Self {
        Self {
            runtime: Runtime::from_env(),
            local: Arc::new(Mutex::new(HashMap::new())),
            idle_timeout: idle_timeout(),
        }
    }

    /// Resolve a sandbox to its backend handle and defer its idle deadline.
    async fn handle_for(&self, sandbox_id: &str) -> Result<String, String> {
        let handle = self.runtime.handle(sandbox_id);
        match self.runtime.touch(&handle) {
            // Annotating is both the touch and the existence check: kubectl
            // fails when the Pod is gone, so no second lookup is needed.
            Some(invocation) => run_cli(&invocation, 15)
                .await
                .map(|_| handle)
                .map_err(|_| format!("unknown sandbox: {sandbox_id}")),
            None => {
                let mut map = self.local.lock().await;
                let state = map
                    .get_mut(sandbox_id)
                    .ok_or_else(|| format!("unknown sandbox: {sandbox_id}"))?;
                state.last_used_at = std::time::Instant::now();
                Ok(state.handle.clone())
            }
        }
    }

    /// Hand back the session's existing sandbox when it has one.
    async fn reuse_session(&self, id: &str, start_params: &StartParams) -> Option<SandboxResponse> {
        match self.runtime {
            Runtime::Docker => reuse_local_session(id, start_params, &self.local).await,
            Runtime::Kubernetes => self.reuse_cluster_session(id, start_params).await,
        }
    }

    async fn reuse_cluster_session(
        &self,
        id: &str,
        start_params: &StartParams,
    ) -> Option<SandboxResponse> {
        let invocation = self.runtime.list(Some(&start_params.session_key));
        let output = run_cli(&invocation, 30).await.ok()?;
        let pods = kubernetes::parse_pod_list(&output).ok()?;
        // The session label is a hash, so confirm the key before handing a
        // workspace over: a collision must not cross sessions.
        let pod = pods
            .into_iter()
            .find(|pod| pod.ready && pod.session_key == start_params.session_key)?;

        if let Some(response) = refuse_network_upgrade(id, start_params.network, pod.network) {
            return Some(response);
        }
        let _ = self.handle_for(&pod.sandbox_id).await;
        Some(SandboxResponse::ok(
            id.to_string(),
            serde_json::json!({"sandbox_id": pod.sandbox_id}),
        ))
    }

    async fn register(&self, sandbox_id: &str, handle: String, start_params: &StartParams) {
        if self.runtime != Runtime::Docker {
            return;
        }
        self.local.lock().await.insert(
            sandbox_id.to_string(),
            ContainerState {
                handle,
                session_key: start_params.session_key.clone(),
                network: start_params.network,
                last_used_at: std::time::Instant::now(),
            },
        );
    }

    /// Drop a sandbox from the index and destroy it. Returns whether the
    /// sandbox existed, so a caller can tell a close from a stale handle.
    async fn discard(&self, sandbox_id: &str) -> bool {
        match self.runtime {
            Runtime::Docker => {
                let Some(state) = self.local.lock().await.remove(sandbox_id) else {
                    return false;
                };
                self.destroy(&state.handle).await;
                true
            }
            // `kubectl delete --ignore-not-found` prints what it deleted and
            // stays silent otherwise, so the output distinguishes the two
            // without a second lookup racing another replica's reaper.
            Runtime::Kubernetes => {
                let invocation = self.runtime.destroy(&self.runtime.handle(sandbox_id));
                run_cli(&invocation, 30)
                    .await
                    .is_ok_and(|output| !output.trim().is_empty())
            }
        }
    }

    /// Make sure the backend has somewhere for a networked sandbox to attach.
    /// Kubernetes needs nothing: the Pod's network label selects a
    /// NetworkPolicy that is already in the cluster.
    async fn prepare_network(&self, network: NetworkAccess) -> Result<(), String> {
        if self.runtime != Runtime::Docker || network != NetworkAccess::PublicInternet {
            return Ok(());
        }
        // A dedicated bridge, never Housebot's own network. Creating it again
        // is a harmless no-op, so an existing network is not an error.
        let _ = run_cli(
            &Invocation {
                binary: "docker",
                args: vec![
                    "network".to_string(),
                    "create".to_string(),
                    "--driver".to_string(),
                    "bridge".to_string(),
                    "housebot-sandbox-net".to_string(),
                ],
            },
            30,
        )
        .await;
        Ok(())
    }

    async fn destroy(&self, handle: &str) {
        let _ = run_cli(&self.runtime.destroy(handle), 30).await;
    }

    /// Destroy sandboxes untouched for longer than the idle timeout.
    /// Everything in a sandbox is memory-backed, so reaping discards the
    /// workspace with it.
    async fn reap_once(&self) {
        match self.runtime {
            Runtime::Docker => {
                for (id, handle) in take_expired_local(&self.local, self.idle_timeout).await {
                    tracing::info!(sandbox_id = %id, "reaping idle sandbox");
                    self.destroy(&handle).await;
                }
            }
            Runtime::Kubernetes => {
                let Ok(output) = run_cli(&self.runtime.list(None), 30).await else {
                    return;
                };
                let Ok(pods) = kubernetes::parse_pod_list(&output) else {
                    return;
                };
                let now = kubernetes::now_epoch_secs();
                for pod in pods {
                    if now.saturating_sub(pod.last_used_at) < self.idle_timeout.as_secs() {
                        continue;
                    }
                    tracing::info!(sandbox_id = %pod.sandbox_id, "reaping idle sandbox");
                    self.destroy(&self.runtime.handle(&pod.sandbox_id)).await;
                }
            }
        }
    }

    /// Discard sandboxes left over from a previous daemon. Under Kubernetes
    /// other replicas may be actively serving them, so only the idle ones go.
    pub async fn cleanup_stale(&self) {
        match self.runtime {
            Runtime::Docker => {
                let Ok(output) = run_cli(&self.runtime.list(None), 30).await else {
                    tracing::warn!("failed to list stale sandbox containers");
                    return;
                };
                for line in output.lines() {
                    let parts: Vec<&str> = line.splitn(2, ' ').collect();
                    if let [_id, name] = parts[..] {
                        tracing::info!("removing stale sandbox container: {name}");
                        self.destroy(name).await;
                    }
                }
            }
            Runtime::Kubernetes => self.reap_once().await,
        }
    }
}

async fn reap_idle_sandboxes(server: Arc<Server>) {
    let mut ticker = tokio::time::interval(sweep_interval(server.idle_timeout));
    loop {
        ticker.tick().await;
        server.reap_once().await;
    }
}

/// A container's network mode is fixed at creation, so a request needing more
/// access than the live sandbox has must be refused rather than downgraded.
fn refuse_network_upgrade(
    id: &str,
    requested: NetworkAccess,
    existing: NetworkAccess,
) -> Option<SandboxResponse> {
    (requested == NetworkAccess::PublicInternet && existing == NetworkAccess::None).then(|| {
        SandboxResponse::err(
            id.to_string(),
            "This session's sandbox is already running without network access. \
             Close it before running a tool that needs the internet."
                .to_string(),
        )
    })
}

/// How long a sandbox may sit unused before it is destroyed.
fn idle_timeout() -> std::time::Duration {
    let secs = std::env::var("SANDBOX_IDLE_TIMEOUT_SECS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|secs| *secs > 0)
        .unwrap_or(limits::DEFAULT_SANDBOX_IDLE_TIMEOUT_SECS);
    std::time::Duration::from_secs(secs)
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

    let server = start();
    server.cleanup_stale().await;

    let listener = UnixListener::bind(socket_path)?;
    tracing::info!(socket_path, "sandboxd listening");

    loop {
        let (stream, _addr) = listener.accept().await?;
        let server = Arc::clone(&server);
        tokio::spawn(async move {
            if let Err(e) = handle_connection(stream, server).await {
                tracing::error!("connection handler error: {e}");
            }
        });
    }
}

/// Start the shared server and its idle reaper.
pub fn start() -> Arc<Server> {
    let server = Arc::new(Server::from_env());
    tokio::spawn(reap_idle_sandboxes(Arc::clone(&server)));
    server
}

async fn handle_connection(mut stream: UnixStream, server: Arc<Server>) -> anyhow::Result<()> {
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

    let response = process_request(&request, &server).await;

    let response_line = serde_json::to_string(&response)?;
    let mut bytes = response_line.into_bytes();
    bytes.push(b'\n');
    writer.write_all(&bytes).await?;
    writer.shutdown().await.ok();

    Ok(())
}

pub async fn process_request(request: &SandboxRequest, server: &Server) -> SandboxResponse {
    let id = &request.id;

    match request.method.as_str() {
        "start" => handle_start(id, &request.params, server).await,
        "clone_repository" => handle_clone_repository(id, &request.params, server).await,
        "list_files" => handle_list_files(id, &request.params, server).await,
        "search_code" => handle_search_code(id, &request.params, server).await,
        "read_file" => handle_read_file(id, &request.params, server).await,
        "run" => handle_run(id, &request.params, server).await,
        "write_file" => handle_write_file(id, &request.params, server).await,
        "close" => handle_close(id, &request.params, server).await,
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

/// Remove every locally indexed sandbox past `timeout` and report what was
/// dropped, so the caller can destroy them without holding the lock.
async fn take_expired_local(
    containers: &ContainerMap,
    timeout: std::time::Duration,
) -> Vec<(String, String)> {
    let mut map = containers.lock().await;
    let expired: Vec<(String, String)> = map
        .iter()
        .filter(|(_, state)| state.last_used_at.elapsed() >= timeout)
        .map(|(id, state)| (id.clone(), state.handle.clone()))
        .collect();
    for (id, _) in &expired {
        map.remove(id);
    }
    expired
}

fn sweep_interval(timeout: std::time::Duration) -> std::time::Duration {
    (timeout / 2).max(std::time::Duration::from_secs(1))
}

// ── Handlers ────────────────────────────────────────────────────────────────

async fn handle_start(id: &str, params: &serde_json::Value, server: &Server) -> SandboxResponse {
    let start_params: StartParams = match serde_json::from_value(params.clone()) {
        Ok(p) => p,
        Err(e) => return SandboxResponse::err(id.to_string(), format!("invalid params: {e}")),
    };

    if let Err(e) = validation::validate_session_key(&start_params.session_key) {
        return SandboxResponse::err(id.to_string(), format!("invalid session key: {e}"));
    }

    if let Some(response) = server.reuse_session(id, &start_params).await {
        return response;
    }

    if let Err(e) = server.prepare_network(start_params.network).await {
        return SandboxResponse::err(id.to_string(), e);
    }

    let sandbox_id = uuid::Uuid::new_v4().to_string();
    let (invocation, stdin) =
        server
            .runtime
            .create(&sandbox_id, &start_params.session_key, start_params.network);

    let created = match stdin {
        Some(manifest) => run_cli_with_stdin(&invocation, &manifest, 60)
            .await
            .and_then(|(stderr, code)| {
                (code == 0)
                    .then_some(String::new())
                    .ok_or_else(|| format!("exited with code {code}: {stderr}"))
            }),
        None => run_cli(&invocation, 60).await,
    };
    if let Err(e) = created {
        return SandboxResponse::err(id.to_string(), format!("failed to create sandbox: {e}"));
    }

    if let Some(wait) = server
        .runtime
        .wait_ready(&sandbox_id, SANDBOX_READY_TIMEOUT_SECS)
    {
        if let Err(e) = run_cli(&wait, SANDBOX_READY_TIMEOUT_SECS + 5).await {
            let _ = server.discard(&sandbox_id).await;
            return SandboxResponse::err(
                id.to_string(),
                format!("sandbox never became ready: {e}"),
            );
        }
    }

    server
        .register(
            &sandbox_id,
            server.runtime.handle(&sandbox_id),
            &start_params,
        )
        .await;

    SandboxResponse::ok(
        id.to_string(),
        serde_json::json!({"sandbox_id": sandbox_id}),
    )
}

/// Hand back the session's existing sandbox from the local Docker index.
async fn reuse_local_session(
    id: &str,
    start_params: &StartParams,
    containers: &ContainerMap,
) -> Option<SandboxResponse> {
    let mut map = containers.lock().await;
    let (sandbox_id, state) = map
        .iter_mut()
        .find(|(_, state)| state.session_key == start_params.session_key)?;
    if let Some(response) = refuse_network_upgrade(id, start_params.network, state.network) {
        return Some(response);
    }
    state.last_used_at = std::time::Instant::now();
    Some(SandboxResponse::ok(
        id.to_string(),
        serde_json::json!({"sandbox_id": sandbox_id}),
    ))
}

async fn handle_clone_repository(
    id: &str,
    params: &serde_json::Value,
    server: &Server,
) -> SandboxResponse {
    let clone_params: CloneRepositoryParams = match serde_json::from_value(params.clone()) {
        Ok(p) => p,
        Err(e) => return SandboxResponse::err(id.to_string(), format!("invalid params: {e}")),
    };

    if let Err(e) = validation::validate_repository_url(&clone_params.url) {
        return SandboxResponse::err(id.to_string(), format!("invalid URL: {e}"));
    }
    if let Some(ref branch) = clone_params.branch {
        if let Err(e) = validation::validate_branch(branch) {
            return SandboxResponse::err(id.to_string(), format!("invalid branch: {e}"));
        }
    }

    let sandbox_id = clone_params.sandbox_id.clone();
    let handle = match server.handle_for(&sandbox_id).await {
        Ok(handle) => handle,
        Err(e) => return SandboxResponse::err(id.to_string(), e),
    };

    let dest = "/workspace/repo";
    let invocation = server.runtime.git_clone(
        &handle,
        &clone_params.url,
        dest,
        clone_params.branch.as_deref(),
    );

    match run_cli_checked(&invocation, limits::TEST_TIMEOUT_SECS).await {
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
    server: &Server,
) -> SandboxResponse {
    let list_params: ListFilesParams = match serde_json::from_value(params.clone()) {
        Ok(p) => p,
        Err(e) => return SandboxResponse::err(id.to_string(), format!("invalid params: {e}")),
    };

    if let Err(e) = validation::validate_workspace_path(&list_params.path) {
        return SandboxResponse::err(id.to_string(), format!("invalid path: {e}"));
    }

    let sandbox_id = list_params.sandbox_id.clone();
    let handle = match server.handle_for(&sandbox_id).await {
        Ok(handle) => handle,
        Err(e) => return SandboxResponse::err(id.to_string(), e),
    };

    let max_depth = list_params.max_depth.unwrap_or(3);
    let cmd = format!(
        "find {} -maxdepth {} -not -path '*/.git/*' -not -path '*/target/*' -not -path '*/node_modules/*' -printf '%y %s %p\\n' 2>/dev/null | head -{}",
        shell_escape_path(&list_params.path),
        max_depth,
        limits::MAX_FILE_LIST_ENTRIES
    );

    let invocation = server.runtime.exec(&handle, &cmd, None);

    match run_cli_checked(&invocation, limits::DEFAULT_COMMAND_TIMEOUT_SECS).await {
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
    server: &Server,
) -> SandboxResponse {
    let search_params: SearchCodeParams = match serde_json::from_value(params.clone()) {
        Ok(p) => p,
        Err(e) => return SandboxResponse::err(id.to_string(), format!("invalid params: {e}")),
    };

    if let Err(e) = validation::validate_query(&search_params.query) {
        return SandboxResponse::err(id.to_string(), format!("invalid query: {e}"));
    }
    if let Some(ref glob) = search_params.glob {
        if let Err(e) = validation::validate_glob(glob) {
            return SandboxResponse::err(id.to_string(), format!("invalid glob: {e}"));
        }
    }
    if let Some(ref path) = search_params.path {
        if let Err(e) = validation::validate_workspace_path(path) {
            return SandboxResponse::err(id.to_string(), format!("invalid path: {e}"));
        }
    }

    let sandbox_id = search_params.sandbox_id.clone();
    let handle = match server.handle_for(&sandbox_id).await {
        Ok(handle) => handle,
        Err(e) => return SandboxResponse::err(id.to_string(), e),
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

    let invocation = server.runtime.exec(&handle, &rg_cmd, None);

    match run_cli_checked(&invocation, limits::DEFAULT_COMMAND_TIMEOUT_SECS).await {
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
    server: &Server,
) -> SandboxResponse {
    let read_params: ReadFileParams = match serde_json::from_value(params.clone()) {
        Ok(p) => p,
        Err(e) => return SandboxResponse::err(id.to_string(), format!("invalid params: {e}")),
    };

    if let Err(e) = validation::validate_workspace_path(&read_params.path) {
        return SandboxResponse::err(id.to_string(), format!("invalid path: {e}"));
    }

    let sandbox_id = read_params.sandbox_id.clone();
    let handle = match server.handle_for(&sandbox_id).await {
        Ok(handle) => handle,
        Err(e) => return SandboxResponse::err(id.to_string(), e),
    };

    // Canonicalize path to prevent symlink escape, then verify it's under /workspace
    let resolve_cmd = format!(
        "realpath -q /workspace/{} 2>/dev/null || true",
        shell_escape_path(&read_params.path)
    );
    let resolve = server.runtime.exec(&handle, &resolve_cmd, None);
    let resolved = match run_cli_checked(&resolve, 10).await {
        Ok(out) => out.trim().to_string(),
        Err(_) => {
            return SandboxResponse::err(id.to_string(), "failed to resolve path".to_string())
        }
    };

    if resolved.is_empty() || !resolved.starts_with("/workspace/") {
        return SandboxResponse::err(
            id.to_string(),
            "path escapes workspace via symlink".to_string(),
        );
    }

    let cmd = if let (Some(start), Some(end)) = (read_params.start_line, read_params.end_line) {
        if start > end || end - start > limits::MAX_FILE_READ_LINES as u32 {
            return SandboxResponse::err(id.to_string(), "line range exceeds maximum".to_string());
        }
        format!(
            "head -n {} {} 2>/dev/null | tail -n +{} 2>/dev/null | head -c {}",
            end,
            shell_escape_path(&resolved),
            start,
            limits::MAX_FILE_READ_BYTES
        )
    } else {
        format!(
            "head -c {} {} 2>/dev/null",
            limits::MAX_FILE_READ_BYTES,
            shell_escape_path(&resolved)
        )
    };

    let invocation = server.runtime.exec(&handle, &cmd, None);

    match run_cli_checked(&invocation, limits::DEFAULT_COMMAND_TIMEOUT_SECS).await {
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

async fn handle_run(id: &str, params: &serde_json::Value, server: &Server) -> SandboxResponse {
    let run_params: RunParams = match serde_json::from_value(params.clone()) {
        Ok(p) => p,
        Err(e) => return SandboxResponse::err(id.to_string(), format!("invalid params: {e}")),
    };

    if let Err(e) = validation::validate_command(&run_params.command) {
        return SandboxResponse::err(id.to_string(), format!("invalid command: {e}"));
    }
    if let Some(ref dir) = run_params.working_dir {
        if let Err(e) = validation::validate_workspace_path(dir) {
            return SandboxResponse::err(id.to_string(), format!("invalid working dir: {e}"));
        }
    }

    let sandbox_id = run_params.sandbox_id.clone();
    let timeout = run_params
        .timeout_secs
        .unwrap_or(limits::DEFAULT_COMMAND_TIMEOUT_SECS)
        .min(limits::ABSOLUTE_MAX_TIMEOUT_SECS);

    let handle = match server.handle_for(&sandbox_id).await {
        Ok(handle) => handle,
        Err(e) => return SandboxResponse::err(id.to_string(), e),
    };

    let invocation = server.runtime.exec(
        &handle,
        &run_params.command,
        run_params.working_dir.as_deref(),
    );

    match run_cli_raw(&invocation, timeout).await {
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
            if e.contains("timed out") {
                let _ = server.discard(&sandbox_id).await;
                SandboxResponse::err(
                    id.to_string(),
                    format!("command timed out ({timeout}s) and the sandbox was destroyed"),
                )
            } else {
                SandboxResponse::err(id.to_string(), e)
            }
        }
    }
}

async fn handle_close(id: &str, params: &serde_json::Value, server: &Server) -> SandboxResponse {
    let sandbox_id = match get_sandbox_id(params) {
        Ok(s) => s,
        Err(e) => return SandboxResponse::err(id.to_string(), e),
    };

    if server.discard(&sandbox_id).await {
        SandboxResponse::ok(id.to_string(), serde_json::json!({"closed": true}))
    } else {
        SandboxResponse::err(id.to_string(), format!("unknown sandbox: {sandbox_id}"))
    }
}

// ── Runtime process helpers ─────────────────────────────────────────────────

/// Run a runtime command and return stdout.
async fn run_cli(invocation: &Invocation, timeout_secs: u64) -> Result<String, String> {
    let binary = invocation.binary;
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(timeout_secs),
        Command::new(binary)
            .args(&invocation.args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .output(),
    )
    .await
    .map_err(|_| format!("{binary} command timed out after {timeout_secs}s"))?
    .map_err(|e| format!("failed to execute {binary}: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("{binary} command failed: {stderr}"));
    }

    Ok(utf8_safe_string(&output.stdout))
}

/// Run a runtime command feeding `stdin` to the child, returning its exit code.
///
/// Content passed this way never appears in argv or a shell command line, so a
/// file body cannot be reinterpreted as part of the command.
async fn run_cli_with_stdin(
    invocation: &Invocation,
    stdin_data: &[u8],
    timeout_secs: u64,
) -> Result<(String, i32), String> {
    use tokio::io::AsyncWriteExt;

    let binary = invocation.binary;
    let mut child = Command::new(binary)
        .args(&invocation.args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| format!("failed to execute {binary}: {e}"))?;

    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| format!("failed to open {binary} stdin"))?;
    let data = stdin_data.to_vec();
    let writer = tokio::spawn(async move {
        let _ = stdin.write_all(&data).await;
        let _ = stdin.flush().await;
        drop(stdin);
    });

    let output = tokio::time::timeout(
        std::time::Duration::from_secs(timeout_secs),
        child.wait_with_output(),
    )
    .await
    .map_err(|_| format!("command timed out after {timeout_secs}s"))?
    .map_err(|e| format!("failed to execute {binary}: {e}"))?;
    let _ = writer.await;

    Ok((
        utf8_safe_string(&output.stderr),
        output.status.code().unwrap_or(-1),
    ))
}

async fn handle_write_file(
    id: &str,
    params: &serde_json::Value,
    server: &Server,
) -> SandboxResponse {
    let write_params: WriteFileParams = match serde_json::from_value(params.clone()) {
        Ok(p) => p,
        Err(e) => return SandboxResponse::err(id.to_string(), format!("invalid params: {e}")),
    };

    if let Err(e) = validation::validate_workspace_path(&write_params.path) {
        return SandboxResponse::err(id.to_string(), format!("invalid path: {e}"));
    }
    if write_params.content.len() > limits::MAX_WRITE_FILE_BYTES {
        return SandboxResponse::err(
            id.to_string(),
            format!("content exceeds {} bytes", limits::MAX_WRITE_FILE_BYTES),
        );
    }

    let sandbox_id = write_params.sandbox_id.clone();
    let handle = match server.handle_for(&sandbox_id).await {
        Ok(handle) => handle,
        Err(e) => return SandboxResponse::err(id.to_string(), e),
    };

    // Resolve before writing so an existing symlink at the target cannot
    // redirect the write outside /workspace. `-m` allows the file itself not to
    // exist yet while still resolving the directories above it.
    let resolve_cmd = format!(
        "realpath -m /workspace/{} 2>/dev/null || true",
        shell_escape_path(&write_params.path)
    );
    let resolve = server.runtime.exec(&handle, &resolve_cmd, None);
    let resolved = match run_cli_checked(&resolve, 10).await {
        Ok(out) => out.trim().to_string(),
        Err(_) => {
            return SandboxResponse::err(id.to_string(), "failed to resolve path".to_string())
        }
    };
    if resolved.is_empty() || !resolved.starts_with("/workspace/") {
        return SandboxResponse::err(id.to_string(), "path escapes /workspace".to_string());
    }

    if let Some(parent) = std::path::Path::new(&resolved).parent() {
        let mkdir = vec![
            "/bin/mkdir".to_string(),
            "-p".to_string(),
            parent.to_string_lossy().to_string(),
        ];
        let invocation = server.runtime.exec_argv(&handle, &mkdir, false);
        if let Err(e) = run_cli_checked(&invocation, 10).await {
            return SandboxResponse::err(
                id.to_string(),
                format!("failed to create directory: {e}"),
            );
        }
    }

    let tee = vec!["/usr/bin/tee".to_string(), resolved.clone()];
    let invocation = server.runtime.exec_argv(&handle, &tee, true);
    match run_cli_with_stdin(&invocation, write_params.content.as_bytes(), 30).await {
        Ok((_, 0)) => {}
        Ok((stderr, code)) => {
            return SandboxResponse::err(
                id.to_string(),
                format!("write failed with code {code}: {stderr}"),
            )
        }
        Err(e) => return SandboxResponse::err(id.to_string(), e),
    }

    if write_params.executable {
        let chmod = vec!["/bin/chmod".to_string(), "+x".to_string(), resolved.clone()];
        let invocation = server.runtime.exec_argv(&handle, &chmod, false);
        if let Err(e) = run_cli_checked(&invocation, 10).await {
            return SandboxResponse::err(id.to_string(), format!("chmod failed: {e}"));
        }
    }

    SandboxResponse::ok(
        id.to_string(),
        serde_json::to_value(WriteFileResult {
            path: resolved,
            bytes_written: write_params.content.len(),
        })
        .unwrap_or_default(),
    )
}

async fn run_cli_raw(
    invocation: &Invocation,
    timeout_secs: u64,
) -> Result<(String, String, i32), String> {
    let binary = invocation.binary;
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(timeout_secs),
        Command::new(binary)
            .args(&invocation.args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .output(),
    )
    .await
    .map_err(|_| format!("command timed out after {timeout_secs}s"))?
    .map_err(|e| format!("failed to execute {binary}: {e}"))?;

    let stdout = utf8_safe_string(&output.stdout);
    let stderr = utf8_safe_string(&output.stderr);
    let exit_code = output.status.code().unwrap_or(-1);

    Ok((stdout, stderr, exit_code))
}

/// Run a runtime command and return stdout only, for commands where nothing
/// but success matters.
async fn run_cli_checked(invocation: &Invocation, timeout_secs: u64) -> Result<String, String> {
    let (stdout, stderr, exit_code) = run_cli_raw(invocation, timeout_secs).await?;
    if exit_code != 0 {
        return Err(format!("command exited with code {exit_code}: {stderr}",));
    }
    Ok(stdout)
}

/// Truncate a String at a UTF-8 boundary if it exceeds MAX_OUTPUT_BYTES.
fn truncate_output(output: String) -> (String, bool) {
    if output.len() > limits::MAX_OUTPUT_BYTES {
        let mut end = limits::MAX_OUTPUT_BYTES;
        while !output.is_char_boundary(end) {
            end -= 1;
        }
        let mut t = output[..end].to_string();
        t.push_str("\n... (truncated)");
        (t, true)
    } else {
        (output, false)
    }
}

fn truncate_output_raw(output: String) -> (String, bool) {
    truncate_output(output)
}

/// Lossy UTF-8 decode without truncation.
fn utf8_safe_string(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).to_string()
}

/// Escape a path for safe use in a shell command (wraps in single quotes).
fn shell_escape_path(path: &str) -> String {
    format!("'{}'", path.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(
        session_key: &str,
        network: NetworkAccess,
        idle: std::time::Duration,
    ) -> ContainerState {
        ContainerState {
            handle: format!("housebot-sandbox-{session_key}"),
            session_key: session_key.to_string(),
            network,
            last_used_at: std::time::Instant::now() - idle,
        }
    }

    /// A Docker-backed server sharing the test's session index, so the local
    /// path can be exercised without a container runtime.
    fn local_server(local: ContainerMap) -> Server {
        Server {
            runtime: Runtime::Docker,
            local,
            idle_timeout: std::time::Duration::from_secs(300),
        }
    }

    fn start_params(session_key: &str, network: NetworkAccess) -> StartParams {
        StartParams {
            session_key: session_key.to_string(),
            network,
        }
    }

    #[tokio::test]
    async fn a_session_reuses_its_existing_sandbox() {
        let containers: ContainerMap = Arc::new(Mutex::new(HashMap::new()));
        containers.lock().await.insert(
            "sandbox-1".to_string(),
            state("user-1", NetworkAccess::None, std::time::Duration::ZERO),
        );

        let response = reuse_local_session(
            "req",
            &start_params("user-1", NetworkAccess::None),
            &containers,
        )
        .await
        .expect("the session already has a sandbox");

        assert_eq!(
            response.result.unwrap()["sandbox_id"],
            "sandbox-1",
            "a second turn must land in the same container"
        );
        assert_eq!(containers.lock().await.len(), 1);
    }

    #[tokio::test]
    async fn a_different_session_gets_its_own_sandbox() {
        let containers: ContainerMap = Arc::new(Mutex::new(HashMap::new()));
        containers.lock().await.insert(
            "sandbox-1".to_string(),
            state("user-1", NetworkAccess::None, std::time::Duration::ZERO),
        );

        assert!(
            reuse_local_session(
                "req",
                &start_params("user-2", NetworkAccess::None),
                &containers,
            )
            .await
            .is_none(),
            "one user's workspace must never be handed to another"
        );
    }

    #[tokio::test]
    async fn reusing_a_networkless_sandbox_for_network_work_is_refused() {
        let containers: ContainerMap = Arc::new(Mutex::new(HashMap::new()));
        containers.lock().await.insert(
            "sandbox-1".to_string(),
            state("user-1", NetworkAccess::None, std::time::Duration::ZERO),
        );

        let response = reuse_local_session(
            "req",
            &start_params("user-1", NetworkAccess::PublicInternet),
            &containers,
        )
        .await
        .expect("the session has a sandbox");
        assert!(
            response.error.is_some(),
            "network mode is fixed at creation"
        );
    }

    #[tokio::test]
    async fn reuse_defers_the_idle_deadline() {
        let containers: ContainerMap = Arc::new(Mutex::new(HashMap::new()));
        containers.lock().await.insert(
            "sandbox-1".to_string(),
            state(
                "user-1",
                NetworkAccess::None,
                std::time::Duration::from_secs(120),
            ),
        );

        reuse_local_session(
            "req",
            &start_params("user-1", NetworkAccess::None),
            &containers,
        )
        .await
        .expect("the session has a sandbox");

        let map = containers.lock().await;
        assert!(
            map["sandbox-1"].last_used_at.elapsed() < std::time::Duration::from_secs(1),
            "an active session must not be reaped mid-use"
        );
    }

    #[tokio::test]
    async fn require_sandbox_defers_the_idle_deadline() {
        let containers: ContainerMap = Arc::new(Mutex::new(HashMap::new()));
        containers.lock().await.insert(
            "sandbox-1".to_string(),
            state(
                "user-1",
                NetworkAccess::None,
                std::time::Duration::from_secs(120),
            ),
        );

        drop(
            local_server(Arc::clone(&containers))
                .handle_for("sandbox-1")
                .await
                .expect("sandbox is registered"),
        );

        let map = containers.lock().await;
        assert!(map["sandbox-1"].last_used_at.elapsed() < std::time::Duration::from_secs(1));
    }

    #[tokio::test]
    async fn the_reaper_drops_only_idle_sandboxes() {
        let containers: ContainerMap = Arc::new(Mutex::new(HashMap::new()));
        {
            let mut map = containers.lock().await;
            map.insert(
                "idle".to_string(),
                state(
                    "user-1",
                    NetworkAccess::None,
                    std::time::Duration::from_secs(600),
                ),
            );
            map.insert(
                "busy".to_string(),
                state("user-2", NetworkAccess::None, std::time::Duration::ZERO),
            );
        }

        let expired = take_expired_local(&containers, std::time::Duration::from_secs(300)).await;
        assert_eq!(
            expired,
            vec![("idle".to_string(), "housebot-sandbox-user-1".to_string())],
            "only the idle sandbox may be handed to the reaper"
        );

        let map = containers.lock().await;
        assert!(map.contains_key("busy"), "an active session must survive");
        assert!(!map.contains_key("idle"));
    }

    #[test]
    fn the_sweep_runs_at_least_twice_per_timeout() {
        assert_eq!(
            sweep_interval(std::time::Duration::from_secs(300)),
            std::time::Duration::from_secs(150)
        );
        assert_eq!(
            sweep_interval(std::time::Duration::from_secs(1)),
            std::time::Duration::from_secs(1),
            "a very short timeout must not produce a zero-length interval"
        );
    }
}
