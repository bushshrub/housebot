//! Selection between the two container backends the daemon can drive.
//!
//! `Docker` keeps a sandbox on the local daemon and indexes sessions in
//! process memory. `Kubernetes` puts every sandbox in a Pod and indexes
//! sessions by Pod label, so replicas share no state and scale out freely.

use crate::docker;
use crate::kubernetes;
use crate::protocol::NetworkAccess;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Runtime {
    Docker,
    Kubernetes,
}

/// A command to spawn: the CLI to invoke plus its arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invocation {
    pub binary: &'static str,
    pub args: Vec<String>,
}

impl Runtime {
    pub fn from_env() -> Self {
        match std::env::var("SANDBOX_RUNTIME_BACKEND").as_deref() {
            Ok("kubernetes") | Ok("k8s") => Self::Kubernetes,
            _ => Self::Docker,
        }
    }

    fn binary(&self) -> &'static str {
        match self {
            Self::Docker => "docker",
            Self::Kubernetes => "kubectl",
        }
    }

    fn invoke(&self, args: Vec<String>) -> Invocation {
        Invocation {
            binary: self.binary(),
            args,
        }
    }

    /// The backend-specific name a sandbox is addressed by: a container name
    /// under Docker, a Pod name under Kubernetes. Both derive from the sandbox
    /// ID alone, so no lookup table is needed to reconstruct one.
    pub fn handle(&self, sandbox_id: &str) -> String {
        match self {
            Self::Docker => format!("housebot-sandbox-{sandbox_id}"),
            Self::Kubernetes => kubernetes::pod_name(sandbox_id),
        }
    }

    /// Build the create command. Kubernetes applies a manifest from stdin, so
    /// the second element is the payload to feed the child process.
    pub fn create(
        &self,
        sandbox_id: &str,
        session_key: &str,
        network: NetworkAccess,
    ) -> (Invocation, Option<Vec<u8>>) {
        match self {
            Self::Docker => (
                self.invoke(docker::build_run_args(sandbox_id, network)),
                None,
            ),
            Self::Kubernetes => {
                let manifest =
                    kubernetes::build_pod_manifest(sandbox_id, session_key, network).to_string();
                (
                    self.invoke(kubernetes::build_apply_args()),
                    Some(manifest.into_bytes()),
                )
            }
        }
    }

    pub fn exec(&self, handle: &str, command: &str, working_dir: Option<&str>) -> Invocation {
        let args = match self {
            Self::Docker => docker::build_exec_args(handle, command, working_dir),
            Self::Kubernetes => kubernetes::build_exec_args(handle, command, working_dir),
        };
        self.invoke(args)
    }

    pub fn exec_argv(&self, handle: &str, argv: &[String], interactive: bool) -> Invocation {
        let args = match self {
            Self::Docker => docker::build_exec_argv(handle, argv, interactive),
            Self::Kubernetes => kubernetes::build_exec_argv(handle, argv, interactive),
        };
        self.invoke(args)
    }

    pub fn git_clone(
        &self,
        handle: &str,
        url: &str,
        dest: &str,
        branch: Option<&str>,
    ) -> Invocation {
        let args = match self {
            Self::Docker => docker::build_git_clone_args(handle, url, dest, branch),
            Self::Kubernetes => kubernetes::build_git_clone_args(handle, url, dest, branch),
        };
        self.invoke(args)
    }

    pub fn destroy(&self, handle: &str) -> Invocation {
        let args = match self {
            Self::Docker => docker::build_remove_args(handle),
            Self::Kubernetes => kubernetes::build_delete_args(handle),
        };
        self.invoke(args)
    }

    /// List every sandbox this backend knows about, for startup cleanup and
    /// for the idle reaper.
    pub fn list(&self, session_key: Option<&str>) -> Invocation {
        let args = match self {
            Self::Docker => docker::build_list_sandbox_containers_args(),
            Self::Kubernetes => kubernetes::build_list_args(session_key),
        };
        self.invoke(args)
    }

    /// Refresh a sandbox's idle deadline in backend-visible state. Docker
    /// tracks this in the daemon's own map, so there is nothing to run.
    pub fn touch(&self, handle: &str) -> Option<Invocation> {
        match self {
            Self::Docker => None,
            Self::Kubernetes => Some(self.invoke(kubernetes::build_touch_args(handle))),
        }
    }

    /// Wait for a freshly created sandbox to accept commands. Docker's
    /// detached run has already started the container by the time it returns.
    pub fn wait_ready(&self, sandbox_id: &str, timeout_secs: u64) -> Option<Invocation> {
        match self {
            Self::Docker => None,
            Self::Kubernetes => {
                Some(self.invoke(kubernetes::build_wait_ready_args(sandbox_id, timeout_secs)))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn docker_is_the_default_backend() {
        std::env::remove_var("SANDBOX_RUNTIME_BACKEND");
        assert_eq!(Runtime::from_env(), Runtime::Docker);
    }

    #[test]
    fn kubernetes_creation_carries_a_manifest_on_stdin() {
        let (invocation, stdin) =
            Runtime::Kubernetes.create("abc", "session-1", NetworkAccess::None);
        assert_eq!(invocation.binary, "kubectl");
        assert!(invocation.args.contains(&"apply".to_string()));
        let manifest: serde_json::Value =
            serde_json::from_slice(&stdin.expect("a manifest to apply")).expect("valid JSON");
        assert_eq!(manifest["kind"], "Pod");
    }

    #[test]
    fn docker_creation_needs_no_stdin() {
        let (invocation, stdin) = Runtime::Docker.create("abc", "session-1", NetworkAccess::None);
        assert_eq!(invocation.binary, "docker");
        assert!(stdin.is_none());
    }

    #[test]
    fn both_backends_derive_the_handle_from_the_sandbox_id() {
        assert_eq!(Runtime::Docker.handle("abc"), "housebot-sandbox-abc");
        assert_eq!(Runtime::Kubernetes.handle("abc"), "housebot-sandbox-abc");
    }

    #[test]
    fn only_kubernetes_records_use_outside_the_process() {
        assert!(Runtime::Docker.touch("housebot-sandbox-abc").is_none());
        assert!(Runtime::Kubernetes.touch("housebot-sandbox-abc").is_some());
    }

    #[test]
    fn only_kubernetes_waits_for_readiness() {
        assert!(Runtime::Docker.wait_ready("abc", 60).is_none());
        assert!(Runtime::Kubernetes.wait_ready("abc", 60).is_some());
    }
}
