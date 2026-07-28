//! Unit tests for `docker` (split out to keep the module under 400 lines).

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
fn run_args_contain_runtime_flag() {
    let args = build_run_args("test-1", NetworkAccess::None);
    assert!(
        args.iter().any(|a| a.starts_with("--runtime=")),
        "must always include a --runtime= flag"
    );
}

#[test]
fn run_args_default_runtime_is_runsc() {
    if std::env::var("HOUSEBOT_SANDBOX_RUNTIME").is_ok() {
        return; // env override active; default assertion skipped
    }
    let args = build_run_args("test-1", NetworkAccess::None);
    assert!(
        args.contains(&"--runtime=runsc".to_string()),
        "default runtime must be runsc (gVisor)"
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
    let args = build_git_clone_args("c", "https://github.com/user/repo", "/workspace/repo", None);
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
