//! Handlers that set up and inspect a sandbox workspace.

use crate::docker;
use crate::limits;
use crate::protocol::*;
use crate::server::*;
use crate::server_docker::*;
use crate::validation;

// ── Handlers ────────────────────────────────────────────────────────────────

pub(crate) async fn handle_start(
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

pub(crate) async fn handle_clone_repository(
    id: &str,
    params: &serde_json::Value,
    containers: &ContainerMap,
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

pub(crate) async fn handle_list_files(
    id: &str,
    params: &serde_json::Value,
    containers: &ContainerMap,
) -> SandboxResponse {
    let list_params: ListFilesParams = match serde_json::from_value(params.clone()) {
        Ok(p) => p,
        Err(e) => return SandboxResponse::err(id.to_string(), format!("invalid params: {e}")),
    };

    if let Err(e) = validation::validate_workspace_path(&list_params.path) {
        return SandboxResponse::err(id.to_string(), format!("invalid path: {e}"));
    }

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
