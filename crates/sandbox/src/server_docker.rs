//! Docker subprocess plumbing and output shaping.

use std::process::Stdio;

use tokio::process::Command;

use crate::docker;
use crate::limits;

// ── Docker process helpers ───────────────────────────────────────────────────

/// Run a docker command and return stdout.
pub(crate) async fn run_docker(args: &[String], timeout_secs: u64) -> Result<String, String> {
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(timeout_secs),
        Command::new("docker")
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .output(),
    )
    .await
    .map_err(|_| format!("docker command timed out after {timeout_secs}s"))?
    .map_err(|e| format!("failed to execute docker: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("docker command failed: {stderr}"));
    }

    Ok(utf8_safe_string(&output.stdout))
}

/// Run a docker command and return (stdout, stderr, exit_code) with timeout.
pub(crate) async fn run_docker_with_timeout_raw(
    args: &[String],
    timeout_secs: u64,
) -> Result<(String, String, i32), String> {
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(timeout_secs),
        Command::new("docker")
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .output(),
    )
    .await
    .map_err(|_| format!("command timed out after {timeout_secs}s"))?
    .map_err(|e| format!("failed to execute docker: {e}"))?;

    let stdout = utf8_safe_string(&output.stdout);
    let stderr = utf8_safe_string(&output.stderr);
    let exit_code = output.status.code().unwrap_or(-1);

    Ok((stdout, stderr, exit_code))
}

/// Run a docker command and return stdout only (for commands where we only care about success).
pub(crate) async fn run_docker_with_timeout(
    args: &[String],
    timeout_secs: u64,
) -> Result<String, String> {
    let (stdout, stderr, exit_code) = run_docker_with_timeout_raw(args, timeout_secs).await?;
    if exit_code != 0 {
        return Err(format!("command exited with code {exit_code}: {stderr}",));
    }
    Ok(stdout)
}

/// Truncate a String at a UTF-8 boundary if it exceeds MAX_OUTPUT_BYTES.
pub(crate) fn truncate_output(output: String) -> (String, bool) {
    if output.len() > limits::MAX_OUTPUT_BYTES {
        let mut end = limits::MAX_OUTPUT_BYTES;
        while !output.is_char_boundary(end) {
            end -= 1;
        }
        let mut t = output[..end].to_string();
        t.push_str("\n... (truncated)");
        (t, true)
    } else {
        (output, false)
    }
}

pub(crate) fn truncate_output_raw(output: String) -> (String, bool) {
    truncate_output(output)
}

/// Lossy UTF-8 decode without truncation.
pub(crate) fn utf8_safe_string(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).to_string()
}

pub(crate) async fn destroy_container(container_name: &str) -> Result<(), String> {
    let args = docker::build_remove_args(container_name);
    let _ = run_docker(&args, 30).await;
    Ok(())
}

/// Escape a path for safe use in a shell command (wraps in single quotes).
pub(crate) fn shell_escape_path(path: &str) -> String {
    format!("'{}'", path.replace('\'', "'\\''"))
}

/// Remove all containers with sandbox labels at startup.
pub async fn cleanup_stale_containers() {
    let args = docker::build_list_sandbox_containers_args();
    match run_docker(&args, 30).await {
        Ok(output) => {
            for line in output.lines() {
                let parts: Vec<&str> = line.splitn(2, ' ').collect();
                if parts.len() == 2 {
                    let (_cid, name) = (parts[0], parts[1]);
                    tracing::info!("removing stale sandbox container: {name}");
                    let _ = destroy_container(name).await;
                }
            }
        }
        Err(e) => {
            tracing::warn!("failed to list stale sandbox containers: {e}");
        }
    }
}
