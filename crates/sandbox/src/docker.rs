use crate::protocol::NetworkAccess;

const DEFAULT_SANDBOX_IMAGE: &str = "ghcr.io/bushshrub/housebot/sandbox:latest";
const SANDBOX_LABEL_PREFIX: &str = "com.housebot.sandbox";

pub struct ContainerConfig {
    pub image: String,
    pub container_name: String,
    pub labels: Vec<(String, String)>,
    pub read_only: bool,
    pub user: String,
    pub cap_drop: Vec<String>,
    pub security_opt: Vec<String>,
    pub pids_limit: u64,
    pub memory: String,
    pub memory_swap: String,
    pub cpus: f64,
    pub ulimit: Vec<(String, String)>,
    pub tmpfs: Vec<String>,
    pub network: NetworkAccess,
    pub network_name: Option<String>,
    pub cmd: Vec<String>,
}

impl ContainerConfig {
    fn new(id: &str, network: NetworkAccess) -> Self {
        Self {
            image: std::env::var("HOUSEBOT_SANDBOX_IMAGE")
                .unwrap_or_else(|_| DEFAULT_SANDBOX_IMAGE.to_string()),
            container_name: format!("housebot-sandbox-{id}"),
            labels: vec![
                (format!("{SANDBOX_LABEL_PREFIX}.id"), id.to_string()),
                (
                    format!("{SANDBOX_LABEL_PREFIX}.purpose"),
                    "code-inspection".to_string(),
                ),
            ],
            read_only: true,
            user: "sandbox".to_string(),
            cap_drop: vec!["ALL".to_string()],
            security_opt: vec!["no-new-privileges:true".to_string()],
            pids_limit: 128,
            memory: "2g".to_string(),
            memory_swap: "2g".to_string(),
            cpus: 1.0,
            ulimit: vec![("nofile".to_string(), "512:512".to_string())],
            tmpfs: vec![
                "/workspace:size=256m,noexec,nosuid,uid=1000,gid=1000".to_string(),
                "/tmp:size=64m,noexec,nosuid".to_string(),
                "/home/sandbox:size=32m,noexec,nosuid".to_string(),
            ],
            network,
            network_name: None,
            cmd: vec![],
        }
    }
}

/// Build the `docker run` arguments for creating a sandbox container.
///
/// The returned arguments are fixed — they must never be influenced by user
/// input beyond the `network` mode selection.
pub fn build_run_args(id: &str, network: NetworkAccess) -> Vec<String> {
    let cfg = ContainerConfig::new(id, network);
    let mut args = Vec::new();

    args.push("run".to_string());
    args.push("--detach".to_string());
    args.push("--rm".to_string());

    // Kernel-level sandbox via gVisor (runsc).  gVisor intercepts syscalls
    // in userspace, providing a secure isolation boundary without requiring
    // hardware virtualization or nested VM support.
    // Override with HOUSEBOT_SANDBOX_RUNTIME=runc for CI or dev environments
    // that don't have gVisor installed.
    let runtime = std::env::var("HOUSEBOT_SANDBOX_RUNTIME").unwrap_or_else(|_| "runsc".to_string());
    args.push(format!("--runtime={runtime}"));

    // Container identity
    args.push(format!("--name={}", cfg.container_name));

    // Labels
    for (k, v) in &cfg.labels {
        args.push("--label".to_string());
        args.push(format!("{k}={v}"));
    }

    // Security
    if cfg.read_only {
        args.push("--read-only".to_string());
    }
    args.push(format!("--user={}", cfg.user));
    for cap in &cfg.cap_drop {
        args.push("--cap-drop".to_string());
        args.push(cap.clone());
    }
    for opt in &cfg.security_opt {
        args.push("--security-opt".to_string());
        args.push(opt.clone());
    }

    // Resource limits
    args.push(format!("--pids-limit={}", cfg.pids_limit));
    args.push(format!("--memory={}", cfg.memory));
    args.push(format!("--memory-swap={}", cfg.memory_swap));
    args.push(format!("--cpus={}", cfg.cpus));
    for (name, value) in &cfg.ulimit {
        args.push("--ulimit".to_string());
        args.push(format!("{name}={value}"));
    }

    // Writable tmpfs mounts
    for mount in &cfg.tmpfs {
        args.push("--tmpfs".to_string());
        args.push(mount.clone());
    }

    // Network
    match network {
        NetworkAccess::None => {
            args.push("--network=none".to_string());
        }
        NetworkAccess::PublicInternet => {
            // Use a dedicated bridge network; do NOT join Housebot's network.
            // The sandboxd creates this network on startup if needed.
            args.push("--network=housebot-sandbox-net".to_string());
        }
    }

    // Image
    args.push(cfg.image.clone());

    // Command
    args.push("/bin/sleep".to_string());
    args.push("infinity".to_string());

    args
}

/// Build a `docker exec` command for running a task inside the sandbox.
pub fn build_exec_args(
    container_name: &str,
    command: &str,
    working_dir: Option<&str>,
) -> Vec<String> {
    let mut args = Vec::new();

    args.push("exec".to_string());

    if let Some(dir) = working_dir {
        args.push("-w".to_string());
        args.push(dir.to_string());
    }

    args.push(container_name.to_string());

    // Run the command via bash
    args.push("/bin/bash".to_string());
    args.push("-c".to_string());
    args.push(command.to_string());

    args
}

/// Build a `docker exec git clone` command using separate argv elements.
///
/// Every argument is passed individually to avoid shell interpretation of
/// branch names, URLs, or destination paths.
pub fn build_git_clone_args(
    container_name: &str,
    url: &str,
    dest: &str,
    branch: Option<&str>,
) -> Vec<String> {
    let mut args = vec![
        "exec".to_string(),
        container_name.to_string(),
        "git".to_string(),
        "clone".to_string(),
        "--depth=1".to_string(),
    ];
    if let Some(b) = branch {
        args.push("--branch".to_string());
        args.push(b.to_string());
    }
    args.push(url.to_string());
    args.push(dest.to_string());
    args
}

/// Build a `docker inspect` command to verify a container exists and is managed by us.
pub fn build_inspect_args(container_name: &str) -> Vec<String> {
    vec![
        "inspect".to_string(),
        "--format".to_string(),
        "{{.State.Status}}".to_string(),
        container_name.to_string(),
    ]
}

/// Build a `docker rm -f` command for cleanup.
pub fn build_remove_args(container_name: &str) -> Vec<String> {
    vec![
        "rm".to_string(),
        "-f".to_string(),
        container_name.to_string(),
    ]
}

/// Build a `docker ps` filter command to find stale sandbox containers.
pub fn build_list_sandbox_containers_args() -> Vec<String> {
    vec![
        "ps".to_string(),
        "-a".to_string(),
        "--filter".to_string(),
        format!("label={SANDBOX_LABEL_PREFIX}.id"),
        "--format".to_string(),
        "{{.ID}} {{.Names}}".to_string(),
    ]
}

/// Verify that the Docker arguments never contain dangerous flags.
#[cfg(test)]
#[path = "docker_tests.rs"]
mod tests;
