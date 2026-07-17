use crate::protocol::NetworkAccess;

const SANDBOX_IMAGE: &str = "ghcr.io/bushshrub/housebot/sandbox:latest";
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
            image: SANDBOX_IMAGE.to_string(),
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

    // VM-level isolation via Kata Containers; prevents container-escape from
    // reaching the Docker host even if the in-container command is malicious.
    args.push("--runtime=kata-runtime".to_string());

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
mod tests {
    use super::*;

    #[test]
    fn run_args_contain_read_only() {
        let args = build_run_args("test-1", NetworkAccess::None);
        assert!(args.contains(&"--read-only".to_string()));
    }

    #[test]
    fn run_args_contain_non_root_user() {
        let args = build_run_args("test-1", NetworkAccess::None);
        assert!(
            args.iter().any(|a| a == "--user=sandbox"),
            "args should contain --user=sandbox"
        );
    }

    #[test]
    fn run_args_contain_dropped_capabilities() {
        let args = build_run_args("test-1", NetworkAccess::None);
        assert!(args.contains(&"--cap-drop".to_string()));
        let cap_idx = args.iter().position(|a| a == "--cap-drop");
        assert!(cap_idx.is_some());
        assert_eq!(args[cap_idx.unwrap() + 1], "ALL");
    }

    #[test]
    fn run_args_contain_no_new_privileges() {
        let args = build_run_args("test-1", NetworkAccess::None);
        assert!(args.contains(&"--security-opt".to_string()));
        let opt_idx = args.iter().position(|a| a == "--security-opt");
        assert!(opt_idx.is_some());
        assert_eq!(args[opt_idx.unwrap() + 1], "no-new-privileges:true");
    }

    #[test]
    fn run_args_contain_cpu_limit() {
        let args = build_run_args("test-1", NetworkAccess::None);
        assert!(args.contains(&"--cpus=1".to_string()));
    }

    #[test]
    fn run_args_contain_memory_limit() {
        let args = build_run_args("test-1", NetworkAccess::None);
        assert!(args.contains(&"--memory=2g".to_string()));
    }

    #[test]
    fn run_args_contain_pids_limit() {
        let args = build_run_args("test-1", NetworkAccess::None);
        assert!(args.contains(&"--pids-limit=128".to_string()));
    }

    #[test]
    fn run_args_contain_tmpfs_workspace() {
        let args = build_run_args("test-1", NetworkAccess::None);
        let has_workspace = args.iter().any(|a| a.starts_with("/workspace:"));
        assert!(
            has_workspace,
            "args should contain /workspace tmpfs: {args:?}"
        );
    }

    #[test]
    fn run_args_network_none_for_no_access() {
        let args = build_run_args("test-1", NetworkAccess::None);
        assert!(args.contains(&"--network=none".to_string()));
    }

    #[test]
    fn run_args_network_bridge_for_public() {
        let args = build_run_args("test-1", NetworkAccess::PublicInternet);
        assert!(args.contains(&"--network=housebot-sandbox-net".to_string()));
    }

    #[test]
    fn run_args_use_kata_runtime() {
        let args = build_run_args("test-1", NetworkAccess::None);
        assert!(
            args.contains(&"--runtime=kata-runtime".to_string()),
            "must use kata-runtime for VM-level isolation"
        );
    }

    #[test]
    fn run_args_never_contain_privileged() {
        let args = build_run_args("test-1", NetworkAccess::None);
        assert!(!args.contains(&"--privileged".to_string()));
    }

    #[test]
    fn run_args_never_contain_host_pid() {
        let args = build_run_args("test-1", NetworkAccess::None);
        assert!(!args
            .iter()
            .any(|a| a == "--pid=host" || a.starts_with("--pid=")));
    }

    #[test]
    fn run_args_never_contain_host_network() {
        let args = build_run_args("test-1", NetworkAccess::None);
        assert!(!args.contains(&"--network=host".to_string()));
    }

    #[test]
    fn run_args_never_contain_docker_socket_mount() {
        let args = build_run_args("test-1", NetworkAccess::None);
        let is_docker_socket = |a: &str| a.contains("/var/run/docker.sock");
        assert!(!args.iter().any(|a| is_docker_socket(a)));
    }

    #[test]
    fn run_args_contain_sandbox_labels() {
        let args = build_run_args("test-1", NetworkAccess::None);
        let has_id_label = args.iter().any(|a| a == "com.housebot.sandbox.id=test-1");
        assert!(has_id_label, "args should contain sandbox ID label");
    }

    #[test]
    fn exec_args_use_bash_c() {
        let args = build_exec_args("container-name", "ls -la", None);
        assert_eq!(
            args,
            vec!["exec", "container-name", "/bin/bash", "-c", "ls -la"]
        );
    }

    #[test]
    fn exec_args_with_working_dir() {
        let args = build_exec_args("c", "pwd", Some("/workspace/src"));
        let expected = vec![
            "exec",
            "-w",
            "/workspace/src",
            "c",
            "/bin/bash",
            "-c",
            "pwd",
        ];
        assert_eq!(args, expected);
    }

    #[test]
    fn remove_args_include_force() {
        let args = build_remove_args("housebot-sandbox-test-1");
        assert!(args.contains(&"-f".to_string()));
    }

    #[test]
    fn remove_args_target_correct_container() {
        let args = build_remove_args("housebot-sandbox-abc123");
        assert!(args.contains(&"housebot-sandbox-abc123".to_string()));
    }

    #[test]
    fn git_clone_args_use_argv_instead_of_shell_string() {
        let args = build_git_clone_args(
            "c",
            "https://github.com/user/repo",
            "/workspace/repo",
            Some("main"),
        );
        assert_eq!(
            args,
            vec![
                "exec",
                "c",
                "git",
                "clone",
                "--depth=1",
                "--branch",
                "main",
                "https://github.com/user/repo",
                "/workspace/repo"
            ]
        );
    }

    #[test]
    fn git_clone_args_without_branch() {
        let args =
            build_git_clone_args("c", "https://github.com/user/repo", "/workspace/repo", None);
        assert_eq!(
            args,
            vec![
                "exec",
                "c",
                "git",
                "clone",
                "--depth=1",
                "https://github.com/user/repo",
                "/workspace/repo"
            ]
        );
    }

    #[test]
    fn list_sandbox_containers_uses_label_filter() {
        let args = build_list_sandbox_containers_args();
        assert!(args.iter().any(|a| a.contains("com.housebot.sandbox.id")));
    }
}
