//! Hardening tests for the Kubernetes sandbox runtime.
//!
//! The Pod manifest is the whole isolation boundary under Kubernetes, the way
//! the `docker run` flags are under Docker. Every guarantee the Docker tests
//! assert has a counterpart here.

use housebot_sandbox::kubernetes::*;
use housebot_sandbox::protocol::NetworkAccess;

fn manifest(network: NetworkAccess) -> serde_json::Value {
    build_pod_manifest("test-sandbox", "session-1", network)
}

#[test]
fn pods_run_under_gvisor_by_default() {
    if std::env::var("HOUSEBOT_SANDBOX_RUNTIME_CLASS").is_ok() {
        return; // env override active; default assertion skipped
    }
    assert_eq!(
        manifest(NetworkAccess::None)["spec"]["runtimeClassName"],
        "gvisor",
        "the syscall boundary is the point of the runtime class"
    );
}

#[test]
fn pods_always_declare_a_runtime_class() {
    assert!(manifest(NetworkAccess::None)["spec"]["runtimeClassName"].is_string());
}

#[test]
fn the_container_can_never_gain_privileges() {
    let security = &manifest(NetworkAccess::None)["spec"]["containers"][0]["securityContext"];
    assert_eq!(security["allowPrivilegeEscalation"], false);
    assert_eq!(security["privileged"], false);
    assert_eq!(security["capabilities"]["drop"][0], "ALL");
    assert_eq!(security["seccompProfile"]["type"], "RuntimeDefault");
}

#[test]
fn the_container_runs_as_a_non_root_user_on_a_read_only_root() {
    let security = &manifest(NetworkAccess::None)["spec"]["containers"][0]["securityContext"];
    assert_eq!(security["runAsNonRoot"], true);
    assert_eq!(security["runAsUser"], 1000);
    assert_eq!(security["readOnlyRootFilesystem"], true);
}

#[test]
fn the_pod_never_shares_a_host_namespace() {
    let spec = &manifest(NetworkAccess::None)["spec"];
    for key in ["hostNetwork", "hostPID", "hostIPC"] {
        assert_eq!(spec[key], false, "{key} would dissolve the sandbox");
    }
}

#[test]
fn the_pod_carries_no_api_credentials() {
    let spec = &manifest(NetworkAccess::None)["spec"];
    assert_eq!(
        spec["automountServiceAccountToken"], false,
        "a sandbox with a service account token could drive the cluster"
    );
    assert_eq!(
        spec["enableServiceLinks"], false,
        "service environment variables would leak the cluster's topology"
    );
}

#[test]
fn every_writable_path_is_memory_backed() {
    let volumes = manifest(NetworkAccess::None)["spec"]["volumes"]
        .as_array()
        .expect("volumes")
        .clone();
    assert_eq!(volumes.len(), 3);
    for volume in volumes {
        assert_eq!(
            volume["emptyDir"]["medium"], "Memory",
            "nothing a sandbox writes may reach a node disk: {volume}"
        );
        assert!(volume["emptyDir"]["sizeLimit"].is_string());
    }
}

#[test]
fn the_pod_never_mounts_a_host_path() {
    let manifest = manifest(NetworkAccess::None).to_string();
    for forbidden in ["hostPath", "docker.sock", "persistentVolumeClaim"] {
        assert!(
            !manifest.contains(forbidden),
            "{forbidden} would expose the host to the sandbox"
        );
    }
}

#[test]
fn the_pod_is_capped_on_cpu_memory_and_disk() {
    let limits = &manifest(NetworkAccess::None)["spec"]["containers"][0]["resources"]["limits"];
    assert_eq!(limits["cpu"], "1");
    assert_eq!(limits["memory"], "2Gi");
    assert_eq!(limits["ephemeral-storage"], "64Mi");
}

#[test]
fn a_pod_stops_charging_even_if_every_reaper_dies() {
    assert!(manifest(NetworkAccess::None)["spec"]["activeDeadlineSeconds"].is_number());
}

#[test]
fn the_network_label_records_the_mode_the_policies_select_on() {
    assert_eq!(
        manifest(NetworkAccess::None)["metadata"]["labels"][LABEL_NETWORK],
        "none"
    );
    assert_eq!(
        manifest(NetworkAccess::PublicInternet)["metadata"]["labels"][LABEL_NETWORK],
        "public"
    );
}

#[test]
fn the_session_label_is_a_valid_label_value() {
    let long_key = "a".repeat(128);
    let label = session_label(&long_key);
    assert_eq!(label.len(), 16, "label values are capped at 63 characters");
    assert!(label.chars().all(|c| c.is_ascii_alphanumeric()));
}

#[test]
fn the_session_label_is_stable_and_distinguishing() {
    assert_eq!(session_label("user-1"), session_label("user-1"));
    assert_ne!(session_label("user-1"), session_label("user-2"));
}

#[test]
fn the_session_key_itself_travels_in_an_annotation() {
    assert_eq!(
        manifest(NetworkAccess::None)["metadata"]["annotations"][ANNOTATION_SESSION_KEY],
        "session-1",
        "the hashed label cannot prove a session owns a sandbox on its own"
    );
}

#[test]
fn exec_argv_passes_every_element_separately() {
    let argv = vec![
        "/usr/bin/tee".to_string(),
        "/workspace/a b;rm -rf /".to_string(),
    ];
    let args = build_exec_argv("pod-1", &argv, true);
    let separator = args.iter().position(|a| a == "--").expect("argv separator");
    assert_eq!(
        &args[separator + 1..],
        &argv[..],
        "no shell may reinterpret a user-supplied path"
    );
    assert!(!args.iter().any(|a| a == "/bin/bash"));
}

#[test]
fn exec_argv_omits_stdin_when_not_requested() {
    let args = build_exec_argv("pod-1", &["/bin/mkdir".to_string()], false);
    assert!(!args.contains(&"--stdin".to_string()));
}

#[test]
fn exec_applies_the_working_directory_inside_the_shell() {
    let args = build_exec_args("pod-1", "pwd", Some("/workspace/src"));
    assert_eq!(
        args.last().expect("script"),
        "cd '/workspace/src' && pwd",
        "kubectl exec has no working-directory flag"
    );
}

#[test]
fn exec_quotes_a_working_directory_containing_a_quote() {
    let args = build_exec_args("pod-1", "pwd", Some("/workspace/it's"));
    assert_eq!(
        args.last().expect("script"),
        "cd '/workspace/it'\\''s' && pwd"
    );
}

#[test]
fn git_clone_runs_without_a_shell() {
    let args = build_git_clone_args(
        "pod-1",
        "https://github.com/user/repo",
        "/workspace/repo",
        Some("main"),
    );
    let separator = args.iter().position(|a| a == "--").expect("argv separator");
    assert_eq!(
        &args[separator + 1..],
        &[
            "git".to_string(),
            "clone".to_string(),
            "--depth=1".to_string(),
            "--branch".to_string(),
            "main".to_string(),
            "https://github.com/user/repo".to_string(),
            "/workspace/repo".to_string(),
        ]
    );
}

#[test]
fn listing_one_session_narrows_the_shared_selector() {
    let all = build_list_args(None).join(" ");
    let one = build_list_args(Some("session-1")).join(" ");
    assert!(all.contains(SANDBOX_SELECTOR));
    assert!(one.contains(&format!("{LABEL_SESSION}={}", session_label("session-1"))));
}

#[test]
fn deleting_a_pod_tolerates_a_replica_that_got_there_first() {
    let args = build_delete_args("housebot-sandbox-abc");
    assert!(args.contains(&"--ignore-not-found".to_string()));
    assert!(args.contains(&"housebot-sandbox-abc".to_string()));
}

#[test]
fn every_command_is_pinned_to_the_sandbox_namespace() {
    let commands = [
        build_apply_args(),
        build_wait_ready_args("abc", 60),
        build_exec_args("pod-1", "ls", None),
        build_exec_argv("pod-1", &["ls".to_string()], false),
        build_delete_args("pod-1"),
        build_list_args(None),
        build_touch_args("pod-1"),
    ];
    for args in commands {
        assert_eq!(
            args.first().map(String::as_str),
            Some("--namespace"),
            "an unpinned command would act on whatever namespace the kubeconfig names: {args:?}"
        );
        assert_eq!(args[1], namespace());
    }
}

#[test]
fn a_pod_listing_round_trips_through_the_parser() {
    let listing = serde_json::json!({
        "items": [{
            "metadata": {
                "labels": {
                    LABEL_SANDBOX_ID: "abc",
                    LABEL_NETWORK: "public",
                },
                "annotations": {
                    ANNOTATION_SESSION_KEY: "session-1",
                    ANNOTATION_LAST_USED: "1700000000",
                },
            },
            "status": {"phase": "Running"},
        }],
    })
    .to_string();

    let pods = parse_pod_list(&listing).expect("a valid listing");
    assert_eq!(pods.len(), 1);
    assert_eq!(pods[0].sandbox_id, "abc");
    assert_eq!(pods[0].session_key, "session-1");
    assert_eq!(pods[0].network, NetworkAccess::PublicInternet);
    assert_eq!(pods[0].last_used_at, 1_700_000_000);
    assert!(pods[0].ready);
}

#[test]
fn a_pending_pod_is_not_reported_ready() {
    let listing = serde_json::json!({
        "items": [{
            "metadata": {"labels": {LABEL_SANDBOX_ID: "abc", LABEL_NETWORK: "none"}},
            "status": {"phase": "Pending"},
        }],
    })
    .to_string();
    assert!(!parse_pod_list(&listing).expect("a valid listing")[0].ready);
}

#[test]
fn pods_without_our_labels_are_skipped_rather_than_guessed_at() {
    let listing =
        serde_json::json!({"items": [{"metadata": {"name": "someone-elses"}}]}).to_string();
    assert!(parse_pod_list(&listing)
        .expect("a valid listing")
        .is_empty());
}

#[test]
fn a_malformed_listing_is_an_error_not_an_empty_reap_list() {
    assert!(parse_pod_list("not json").is_err());
    assert!(
        parse_pod_list("{}").is_err(),
        "treating a broken listing as 'no pods' would silently strand every sandbox"
    );
}
