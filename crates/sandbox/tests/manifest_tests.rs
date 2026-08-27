//! Tests that tie the Kubernetes manifests to the code that assumes them.
//!
//! The daemon builds Pods that only work if the cluster carries the matching
//! RuntimeClass, namespace, and NetworkPolicies. Nothing at compile time
//! connects the two, so these tests do.

use housebot_sandbox::kubernetes::*;
use housebot_sandbox::protocol::NetworkAccess;

fn manifest_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../deploy/kubernetes/base")
}

fn read(name: &str) -> String {
    std::fs::read_to_string(manifest_dir().join(name))
        .unwrap_or_else(|e| panic!("{name} must exist: {e}"))
}

#[test]
fn the_cluster_defines_the_runtime_class_pods_ask_for() {
    let pod = build_pod_manifest("abc", "session-1", NetworkAccess::None);
    let requested = pod["spec"]["runtimeClassName"].as_str().expect("a class");
    assert!(
        read("runtimeclass.yaml").contains(&format!("name: {requested}")),
        "a Pod requesting an undefined RuntimeClass never schedules"
    );
}

#[test]
fn the_runtime_class_points_at_gvisor() {
    assert!(
        read("runtimeclass.yaml").contains("handler: runsc"),
        "runsc is the syscall boundary the sandbox relies on"
    );
}

#[test]
fn the_sandbox_namespace_exists_and_is_what_the_api_is_pointed_at() {
    assert!(read("namespaces.yaml").contains(&format!("name: {}", namespace())));
    assert!(read("sandbox-api.yaml").contains(&format!("value: {}", namespace())));
}

#[test]
fn both_namespaces_enforce_the_restricted_pod_security_profile() {
    let namespaces = read("namespaces.yaml");
    assert_eq!(
        namespaces
            .matches("pod-security.kubernetes.io/enforce: restricted")
            .count(),
        2,
        "an unenforced namespace would accept a privileged pod"
    );
}

#[test]
fn the_network_policy_selects_on_the_label_pods_actually_carry() {
    let public = build_pod_manifest("abc", "session-1", NetworkAccess::PublicInternet);
    let label = public["metadata"]["labels"][LABEL_NETWORK]
        .as_str()
        .expect("a network label");
    assert!(
        read("networkpolicies.yaml").contains(&format!("{LABEL_NETWORK}: {label}")),
        "a policy selecting on the wrong label would silently grant or deny the internet"
    );
}

#[test]
fn sandboxes_deny_all_traffic_before_any_policy_grants_it() {
    let policies = read("networkpolicies.yaml");
    assert!(policies.contains("name: sandbox-default-deny"));
    assert!(
        !policies.contains(&format!("{LABEL_NETWORK}: none")),
        "a networkless sandbox must be covered by the default deny, not its own policy"
    );
}

#[test]
fn the_api_may_drive_pods_but_never_read_a_secret() {
    let rbac = read("rbac.yaml");
    assert!(rbac.contains("resources: [\"pods/exec\"]"));
    assert!(
        !rbac.contains("secrets"),
        "the sandbox API has no business reading Secrets"
    );
    assert!(
        !rbac.contains("kind: ClusterRole"),
        "sandbox permissions must not extend past the sandbox namespace"
    );
}

#[test]
fn the_bot_runs_as_a_single_replica() {
    assert!(
        read("bot.yaml").contains("replicas: 1"),
        "the Discord gateway allows one connection per shard"
    );
}

#[test]
fn the_sandbox_tier_scales_out() {
    let api = read("sandbox-api.yaml");
    assert!(api.contains("kind: HorizontalPodAutoscaler"));
    assert!(api.contains("kind: PodDisruptionBudget"));
}

#[test]
fn the_api_is_told_to_use_the_kubernetes_backend() {
    assert!(
        read("sandbox-api.yaml").contains("value: kubernetes"),
        "the default backend is Docker, which is not horizontally scalable"
    );
}

#[test]
fn the_postgres_cluster_can_lose_an_instance() {
    let postgres = read("postgres.yaml");
    assert!(postgres.contains("kind: Cluster"));
    assert!(postgres.contains("instances: 3"));
    assert!(postgres.contains("enablePodAntiAffinity: true"));
}

#[test]
fn no_manifest_carries_a_committed_credential() {
    for name in ["bot.yaml", "sandbox-api.yaml", "postgres.yaml"] {
        let manifest = read(name);
        assert!(
            !manifest.contains("kind: Secret"),
            "{name} must reference Secrets, never define them"
        );
    }
}
