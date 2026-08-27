//! Kubernetes argument and manifest builders for the sandbox runtime.
//!
//! The Docker runtime pins a sandbox to whichever host owns the daemon. The
//! Kubernetes runtime keeps no local state at all: a sandbox is a Pod, the Pod
//! name derives from the sandbox ID, and the session index is the Pod's own
//! labels. Any `sandbox-api` replica can therefore serve any request, which is
//! what makes the tier horizontally scalable.

use crate::protocol::NetworkAccess;

const DEFAULT_SANDBOX_IMAGE: &str = "ghcr.io/bushshrub/housebot/sandbox:latest";
const DEFAULT_NAMESPACE: &str = "housebot-sandboxes";
const DEFAULT_RUNTIME_CLASS: &str = "gvisor";
const SERVICE_ACCOUNT: &str = "housebot-sandbox";

pub const LABEL_NAME: &str = "app.kubernetes.io/name";
pub const LABEL_SANDBOX_ID: &str = "housebot.dev/sandbox-id";
pub const LABEL_SESSION: &str = "housebot.dev/session";
pub const LABEL_NETWORK: &str = "housebot.dev/network";
pub const ANNOTATION_SESSION_KEY: &str = "housebot.dev/session-key";
pub const ANNOTATION_LAST_USED: &str = "housebot.dev/last-used";

/// Every sandbox Pod carries this, so one selector reaps the whole tier.
pub const SANDBOX_SELECTOR: &str = "app.kubernetes.io/name=housebot-sandbox";

pub fn namespace() -> String {
    std::env::var("HOUSEBOT_SANDBOX_NAMESPACE").unwrap_or_else(|_| DEFAULT_NAMESPACE.to_string())
}

fn image() -> String {
    std::env::var("HOUSEBOT_SANDBOX_IMAGE").unwrap_or_else(|_| DEFAULT_SANDBOX_IMAGE.to_string())
}

/// The RuntimeClass that provides the syscall boundary. Override only where
/// gVisor is genuinely unavailable, such as a kind cluster in CI.
fn runtime_class() -> String {
    std::env::var("HOUSEBOT_SANDBOX_RUNTIME_CLASS")
        .unwrap_or_else(|_| DEFAULT_RUNTIME_CLASS.to_string())
}

pub fn pod_name(sandbox_id: &str) -> String {
    format!("housebot-sandbox-{sandbox_id}")
}

/// Session keys allow `_`, run up to 128 characters, and are user-derived;
/// label values allow neither. Hashing gives a fixed-width, selector-safe
/// value, and the key itself is kept verbatim in an annotation.
pub fn session_label(session_key: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in session_key.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

fn network_label(network: NetworkAccess) -> &'static str {
    match network {
        NetworkAccess::None => "none",
        NetworkAccess::PublicInternet => "public",
    }
}

pub fn network_from_label(label: &str) -> Option<NetworkAccess> {
    match label {
        "none" => Some(NetworkAccess::None),
        "public" => Some(NetworkAccess::PublicInternet),
        _ => None,
    }
}

pub fn now_epoch_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default()
}

/// Build the Pod manifest for a sandbox.
///
/// Like the Docker arguments, the shape is fixed: user input reaches the
/// manifest only as the session annotation and the network mode selection.
pub fn build_pod_manifest(
    sandbox_id: &str,
    session_key: &str,
    network: NetworkAccess,
) -> serde_json::Value {
    let container_security = serde_json::json!({
        "allowPrivilegeEscalation": false,
        "privileged": false,
        "readOnlyRootFilesystem": true,
        "runAsNonRoot": true,
        "runAsUser": 1000,
        "runAsGroup": 1000,
        "capabilities": {"drop": ["ALL"]},
        "seccompProfile": {"type": "RuntimeDefault"},
    });

    serde_json::json!({
        "apiVersion": "v1",
        "kind": "Pod",
        "metadata": {
            "name": pod_name(sandbox_id),
            "namespace": namespace(),
            "labels": {
                LABEL_NAME: "housebot-sandbox",
                LABEL_SANDBOX_ID: sandbox_id,
                LABEL_SESSION: session_label(session_key),
                LABEL_NETWORK: network_label(network),
            },
            "annotations": {
                ANNOTATION_SESSION_KEY: session_key,
                ANNOTATION_LAST_USED: now_epoch_secs().to_string(),
            },
        },
        "spec": {
            "runtimeClassName": runtime_class(),
            "serviceAccountName": SERVICE_ACCOUNT,
            "automountServiceAccountToken": false,
            "enableServiceLinks": false,
            "restartPolicy": "Never",
            "hostNetwork": false,
            "hostPID": false,
            "hostIPC": false,
            // A sandbox that outlives its idle reaper — because every replica
            // died at once, say — still stops charging for CPU after an hour.
            "activeDeadlineSeconds": 3600,
            "terminationGracePeriodSeconds": 5,
            "securityContext": {
                "runAsNonRoot": true,
                "runAsUser": 1000,
                "runAsGroup": 1000,
                "fsGroup": 1000,
                "seccompProfile": {"type": "RuntimeDefault"},
            },
            "containers": [{
                "name": "sandbox",
                "image": image(),
                "command": ["/bin/sleep", "infinity"],
                "securityContext": container_security,
                "resources": {
                    "requests": {"cpu": "100m", "memory": "256Mi", "ephemeral-storage": "16Mi"},
                    "limits": {"cpu": "1", "memory": "2Gi", "ephemeral-storage": "64Mi"},
                },
                "volumeMounts": [
                    {"name": "workspace", "mountPath": "/workspace"},
                    {"name": "tmp", "mountPath": "/tmp"},
                    {"name": "home", "mountPath": "/home/sandbox"},
                ],
            }],
            // Memory-backed emptyDirs mirror the Docker tmpfs mounts: nothing
            // a sandbox writes ever reaches a node disk.
            "volumes": [
                {"name": "workspace", "emptyDir": {"medium": "Memory", "sizeLimit": "256Mi"}},
                {"name": "tmp", "emptyDir": {"medium": "Memory", "sizeLimit": "64Mi"}},
                {"name": "home", "emptyDir": {"medium": "Memory", "sizeLimit": "32Mi"}},
            ],
        },
    })
}

fn namespaced(args: &mut Vec<String>) {
    args.push("--namespace".to_string());
    args.push(namespace());
}

/// Build `kubectl apply -f -`; the manifest is fed on stdin.
pub fn build_apply_args() -> Vec<String> {
    let mut args = Vec::new();
    namespaced(&mut args);
    args.push("apply".to_string());
    args.push("--filename".to_string());
    args.push("-".to_string());
    args
}

pub fn build_wait_ready_args(sandbox_id: &str, timeout_secs: u64) -> Vec<String> {
    let mut args = Vec::new();
    namespaced(&mut args);
    args.push("wait".to_string());
    args.push("--for=condition=Ready".to_string());
    args.push(format!("pod/{}", pod_name(sandbox_id)));
    args.push(format!("--timeout={timeout_secs}s"));
    args
}

/// Build a `kubectl exec` command running `command` through bash.
///
/// `kubectl exec` has no working-directory flag, so the directory is applied
/// with a quoted `cd` inside the same shell the command already runs in.
pub fn build_exec_args(pod: &str, command: &str, working_dir: Option<&str>) -> Vec<String> {
    let script = match working_dir {
        Some(dir) => format!("cd '{}' && {command}", dir.replace('\'', "'\\''")),
        None => command.to_string(),
    };

    let mut args = Vec::new();
    namespaced(&mut args);
    args.push("exec".to_string());
    args.push(pod.to_string());
    args.push("--".to_string());
    args.push("/bin/bash".to_string());
    args.push("-c".to_string());
    args.push(script);
    args
}

/// Build a `kubectl exec` command that runs a program directly, with no shell.
///
/// Everything after `--` is a separate argv entry, so nothing in `argv` is
/// interpreted — use this whenever any part of the command is user input.
pub fn build_exec_argv(pod: &str, argv: &[String], interactive: bool) -> Vec<String> {
    let mut args = Vec::new();
    namespaced(&mut args);
    args.push("exec".to_string());
    if interactive {
        args.push("--stdin".to_string());
    }
    args.push(pod.to_string());
    args.push("--".to_string());
    args.extend(argv.iter().cloned());
    args
}

pub fn build_git_clone_args(pod: &str, url: &str, dest: &str, branch: Option<&str>) -> Vec<String> {
    let mut argv = vec![
        "git".to_string(),
        "clone".to_string(),
        "--depth=1".to_string(),
    ];
    if let Some(b) = branch {
        argv.push("--branch".to_string());
        argv.push(b.to_string());
    }
    argv.push(url.to_string());
    argv.push(dest.to_string());
    build_exec_argv(pod, &argv, false)
}

pub fn build_delete_args(pod: &str) -> Vec<String> {
    let mut args = Vec::new();
    namespaced(&mut args);
    args.push("delete".to_string());
    args.push("pod".to_string());
    args.push(pod.to_string());
    args.push("--ignore-not-found".to_string());
    args.push("--wait=false".to_string());
    args
}

/// List sandbox Pods, optionally narrowed to one session.
pub fn build_list_args(session_key: Option<&str>) -> Vec<String> {
    let selector = match session_key {
        Some(key) => format!("{SANDBOX_SELECTOR},{LABEL_SESSION}={}", session_label(key)),
        None => SANDBOX_SELECTOR.to_string(),
    };

    let mut args = Vec::new();
    namespaced(&mut args);
    args.push("get".to_string());
    args.push("pods".to_string());
    args.push("--selector".to_string());
    args.push(selector);
    args.push("--output".to_string());
    args.push("json".to_string());
    args
}

/// Refresh the idle deadline. The annotation lives on the Pod rather than in a
/// replica's memory, so whichever replica handles the next request sees it.
pub fn build_touch_args(pod: &str) -> Vec<String> {
    let mut args = Vec::new();
    namespaced(&mut args);
    args.push("annotate".to_string());
    args.push("pod".to_string());
    args.push(pod.to_string());
    args.push("--overwrite".to_string());
    args.push(format!("{ANNOTATION_LAST_USED}={}", now_epoch_secs()));
    args
}

/// A sandbox Pod as read back from `kubectl get pods -o json`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PodSummary {
    pub sandbox_id: String,
    pub session_key: String,
    pub network: NetworkAccess,
    pub last_used_at: u64,
    pub ready: bool,
}

/// Parse a `kubectl get pods -o json` listing into sandbox summaries.
///
/// Pods missing the labels this crate sets are skipped rather than guessed at.
pub fn parse_pod_list(output: &str) -> Result<Vec<PodSummary>, String> {
    let parsed: serde_json::Value =
        serde_json::from_str(output).map_err(|e| format!("failed to parse pod list: {e}"))?;
    let items = parsed
        .get("items")
        .and_then(|v| v.as_array())
        .ok_or_else(|| "pod list has no items array".to_string())?;

    let mut pods = Vec::new();
    for item in items {
        let metadata = &item["metadata"];
        let labels = &metadata["labels"];
        let annotations = &metadata["annotations"];

        let Some(sandbox_id) = labels[LABEL_SANDBOX_ID].as_str() else {
            continue;
        };
        let Some(network) = labels[LABEL_NETWORK].as_str().and_then(network_from_label) else {
            continue;
        };

        pods.push(PodSummary {
            sandbox_id: sandbox_id.to_string(),
            session_key: annotations[ANNOTATION_SESSION_KEY]
                .as_str()
                .unwrap_or_default()
                .to_string(),
            network,
            last_used_at: annotations[ANNOTATION_LAST_USED]
                .as_str()
                .and_then(|v| v.parse().ok())
                .unwrap_or_default(),
            ready: item["status"]["phase"].as_str() == Some("Running"),
        });
    }
    Ok(pods)
}
