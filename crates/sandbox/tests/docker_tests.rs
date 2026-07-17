use housebot_sandbox::docker::*;
use housebot_sandbox::protocol::NetworkAccess;

#[test]
fn run_args_have_all_required_security_features() {
    let args = build_run_args("test-sandbox", NetworkAccess::None);
    let arg_str = args.join(" ");

    assert!(arg_str.contains("--read-only"), "must be read-only");
    assert!(arg_str.contains("--user=sandbox"), "must run as non-root");
    assert!(arg_str.contains("--cap-drop"), "must drop capabilities");
    assert!(
        arg_str.contains("--security-opt"),
        "must have security opts"
    );
    assert!(arg_str.contains("--pids-limit=128"), "must limit pids");
    assert!(arg_str.contains("--memory=2g"), "must limit memory");
    assert!(arg_str.contains("--cpus=1"), "must limit cpus");
    assert!(arg_str.contains("--ulimit"), "must set ulimit");
    assert!(
        arg_str.contains("--network=none"),
        "must not have network by default"
    );
    assert!(
        arg_str.contains("--runtime=kata-runtime"),
        "must use kata-runtime for VM-level isolation"
    );
}

#[test]
fn run_args_never_contain_privileged() {
    let args = build_run_args("test", NetworkAccess::None);
    let arg_str = args.join(" ");
    assert!(!arg_str.contains("--privileged"), "must not be privileged");
    assert!(
        !arg_str.contains("--pid=host"),
        "must not share pid namespace"
    );
    assert!(!arg_str.contains("--ipc=host"), "must not share ipc");
    assert!(
        !arg_str.contains("--network=host"),
        "must not share host network"
    );
    assert!(
        !arg_str.contains("/var/run/docker.sock"),
        "must not mount docker socket"
    );
}

#[test]
fn run_args_have_correct_image() {
    let args = build_run_args("test", NetworkAccess::None);
    assert!(
        args.iter()
            .any(|a| a.contains("ghcr.io/bushshrub/housebot/sandbox")),
        "must use the sandbox image: {args:?}"
    );
}

#[test]
fn run_args_have_correct_labels() {
    let args = build_run_args("test-123", NetworkAccess::None);
    assert!(
        args.iter().any(|a| a == "com.housebot.sandbox.id=test-123"),
        "must have sandbox id label"
    );
    assert!(
        args.iter()
            .any(|a| a == "com.housebot.sandbox.purpose=code-inspection"),
        "must have purpose label"
    );
}

#[test]
fn run_args_use_sandbox_network_for_public_internet() {
    let args = build_run_args("test", NetworkAccess::PublicInternet);
    assert!(
        args.iter().any(|a| a == "--network=housebot-sandbox-net"),
        "must use sandbox network for public internet access"
    );
}

#[test]
fn exec_args_run_via_bash() {
    let args = build_exec_args("c", "ls -la", None);
    assert_eq!(args[0], "exec");
    assert!(args.contains(&"/bin/bash".to_string()));
    assert!(args.contains(&"-c".to_string()));
    assert!(args.contains(&"ls -la".to_string()));
}

#[test]
fn exec_args_with_working_dir() {
    let args = build_exec_args("c", "pwd", Some("/workspace/src"));
    let w_idx = args.iter().position(|a| a == "-w").unwrap();
    assert_eq!(args[w_idx + 1], "/workspace/src");
}

#[test]
fn remove_args_force_remove() {
    let args = build_remove_args("housebot-sandbox-abc");
    assert!(args.contains(&"-f".to_string()));
    assert!(args.contains(&"housebot-sandbox-abc".to_string()));
}

#[test]
fn list_args_filter_by_label() {
    let args = build_list_sandbox_containers_args();
    assert!(
        args.iter().any(|a| a.contains("com.housebot.sandbox.id")),
        "must filter by sandbox label"
    );
}

#[test]
fn run_args_contain_tmpfs_mounts() {
    let args = build_run_args("test", NetworkAccess::None);
    let tmpfs_args: Vec<&String> = args.iter().filter(|a| a.starts_with('/')).collect();
    assert!(!tmpfs_args.is_empty(), "must have at least one tmpfs mount");
    assert!(
        args.iter().any(|a| a.contains("/workspace:")),
        "must mount /workspace as tmpfs"
    );
    assert!(
        args.iter().any(|a| a.contains("/tmp:")),
        "must mount /tmp as tmpfs"
    );
}

#[test]
fn git_clone_args_use_depth_1() {
    let args = build_git_clone_args("c", "https://github.com/u/r", "/workspace/r", None);
    assert!(args.contains(&"--depth=1".to_string()));
    assert!(args.contains(&"git".to_string()));
    assert!(args.contains(&"clone".to_string()));
}

#[test]
fn git_clone_args_with_branch() {
    let args = build_git_clone_args("c", "https://github.com/u/r", "/workspace/r", Some("main"));
    assert!(args.contains(&"--branch".to_string()));
    assert!(args.contains(&"main".to_string()));
}
