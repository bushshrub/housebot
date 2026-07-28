//! Handlers that search, read, execute, and tear down.

use crate::docker;
use crate::limits;
use crate::protocol::*;
use crate::server::*;
use crate::server_docker::*;
use crate::validation;

pub(crate) async fn handle_search_code(
    id: &str,
    params: &serde_json::Value,
    containers: &ContainerMap,
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

pub(crate) async fn handle_read_file(
    id: &str,
    params: &serde_json::Value,
    containers: &ContainerMap,
) -> SandboxResponse {
    let read_params: ReadFileParams = match serde_json::from_value(params.clone()) {
        Ok(p) => p,
        Err(e) => return SandboxResponse::err(id.to_string(), format!("invalid params: {e}")),
    };

    if let Err(e) = validation::validate_workspace_path(&read_params.path) {
        return SandboxResponse::err(id.to_string(), format!("invalid path: {e}"));
    }

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

    // Canonicalize path to prevent symlink escape, then verify it's under /workspace
    let resolve_cmd = format!(
        "realpath -q /workspace/{} 2>/dev/null || true",
        shell_escape_path(&read_params.path)
    );
    let resolve_args = docker::build_exec_args(&container_name, &resolve_cmd, None);
    let resolved = match run_docker_with_timeout(&resolve_args, 10).await {
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

pub(crate) async fn handle_run(
    id: &str,
    params: &serde_json::Value,
    containers: &ContainerMap,
) -> SandboxResponse {
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
            if e.contains("timed out") {
                // Destroy the container on timeout (lock is already released
                // since container_name was extracted earlier)
                let _ = destroy_container(&container_name).await;
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

pub(crate) async fn handle_close(
    id: &str,
    params: &serde_json::Value,
    containers: &ContainerMap,
) -> SandboxResponse {
    let sandbox_id = match get_sandbox_id(params) {
        Ok(s) => s,
        Err(e) => return SandboxResponse::err(id.to_string(), e),
    };

    let state = {
        let mut map = containers.lock().await;
        map.remove(&sandbox_id)
    };
    match state {
        Some(s) => {
            let _ = destroy_container(&s.container_name).await;
            SandboxResponse::ok(id.to_string(), serde_json::json!({"closed": true}))
        }
        None => SandboxResponse::err(id.to_string(), format!("unknown sandbox: {sandbox_id}")),
    }
}
