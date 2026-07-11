//! Sandbox tool — runs `opencode` in an ephemeral Docker container.
//!
//! The container is a Docker *sibling* (it talks to the host daemon), so the workspace
//! is bind-mounted from a host-visible path under `HOST_DATA_DIR`. Log lines stream back
//! to the caller via a [`TextSink`]; individual output files are copied into the artifacts
//! directory and returned for upload.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use uuid::Uuid;

use crate::config;
use crate::llm::TextSink;

/// Filenames never treated as user artifacts.
pub const EXCLUDED_FILENAMES: &[&str] = &["opencode.json", ".opencode.json"];

/// OpenAI-style tool definition (internal `input_schema` form).
pub fn definition() -> serde_json::Value {
    serde_json::json!({
        "name": "run_opencode",
        "description": "Run a software development task using OpenCode powered by a local \
            llama.cpp model. Good for general coding tasks, quick scripts, and iterative work. \
            Optionally clone a git repo or seed the workspace with files.",
        "input_schema": {
            "type": "object",
            "properties": {
                "task": {"type": "string", "description": "The software development task to perform."},
                "model": {"type": "string", "description": "Model to use, e.g. server-slop/qwen3.6-35b. Defaults to server-slop/gemma-4-12b-qat-q4kxl."},
                "repo_url": {"type": "string", "description": "Optional Git repository URL to clone into the workspace."},
                "files": {"type": "object", "description": "Optional map of relative file paths to content to seed before running.", "additionalProperties": {"type": "string"}}
            },
            "required": ["task"]
        }
    })
}

/// Result of a sandbox run: streamed output plus any collected artifact files.
#[derive(Debug, Default, Clone)]
pub struct OpencodeOutput {
    pub content: String,
    pub artifact_paths: Vec<PathBuf>,
}

/// Copy eligible workspace files into `artifacts_dir`, returning their new paths.
///
/// Skips dotfiles, dot-directories, [`EXCLUDED_FILENAMES`], and files larger than
/// `max_artifact_mb` megabytes.
pub fn collect_workspace_files(
    workspace: &Path,
    artifacts_dir: &Path,
    max_artifact_mb: f64,
) -> Vec<PathBuf> {
    let mut collected = Vec::new();
    if std::fs::create_dir_all(artifacts_dir).is_err() {
        return collected;
    }
    let uid = short_uid();
    walk(
        workspace,
        workspace,
        artifacts_dir,
        max_artifact_mb,
        &uid,
        &mut collected,
    );
    collected
}

fn walk(
    root: &Path,
    dir: &Path,
    artifacts_dir: &Path,
    max_mb: f64,
    uid: &str,
    out: &mut Vec<PathBuf>,
) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if path.is_dir() {
            if name.starts_with('.') {
                continue; // don't descend into dot-directories
            }
            walk(root, &path, artifacts_dir, max_mb, uid, out);
            continue;
        }
        if name.starts_with('.') || EXCLUDED_FILENAMES.contains(&name.as_str()) {
            continue;
        }
        let size_mb = std::fs::metadata(&path)
            .map(|m| m.len() as f64 / (1024.0 * 1024.0))
            .unwrap_or(f64::MAX);
        if size_mb > max_mb {
            continue;
        }
        let rel = path.strip_prefix(root).unwrap_or(&path);
        let flat = rel
            .to_string_lossy()
            .replace(std::path::MAIN_SEPARATOR, "_");
        let dst = artifacts_dir.join(format!("{uid}_{flat}"));
        if std::fs::copy(&path, &dst).is_ok() {
            out.push(dst);
        }
    }
}

fn short_uid() -> String {
    Uuid::new_v4().simple().to_string()[..8].to_string()
}

// ── runtime configuration ────────────────────────────────────────────────────

struct SandboxConfig {
    image: String,
    network: String,
    timeout: Duration,
    cpus: String,
    mem: String,
    artifacts_dir: PathBuf,
    max_artifact_mb: f64,
    host_data_dir: String,
    container_data_dir: String,
}

impl SandboxConfig {
    fn from_env() -> Self {
        Self {
            image: config::env_or("SANDBOX_IMAGE", "house-chatbot-sandbox:latest"),
            network: config::env_or("DOCKER_NETWORK", "house-chatbot_default"),
            timeout: Duration::from_secs(config::env_parse("SANDBOX_TIMEOUT", 300)),
            cpus: config::env_or("SANDBOX_CPUS", "2"),
            mem: config::env_or("SANDBOX_MEM_LIMIT", "1g"),
            artifacts_dir: PathBuf::from(config::env_or("ARTIFACTS_DIR", "data/artifacts")),
            max_artifact_mb: config::env_parse("MAX_ARTIFACT_SIZE_MB", 24.0),
            host_data_dir: config::env_or("HOST_DATA_DIR", ""),
            container_data_dir: config::env_or("DATA_DIR", "data"),
        }
    }
}

/// Run an OpenCode task in a fresh sandbox container.
pub async fn run_opencode(
    task: &str,
    model: Option<&str>,
    repo_url: Option<&str>,
    files: Option<&HashMap<String, String>>,
    sink: Option<&dyn TextSink>,
) -> OpencodeOutput {
    let cfg = SandboxConfig::from_env();

    let uid = short_uid();
    let host_workspace = if cfg.host_data_dir.is_empty() {
        PathBuf::from(&cfg.container_data_dir)
            .join("workspaces")
            .join(&uid)
    } else {
        PathBuf::from(&cfg.host_data_dir)
            .join("workspaces")
            .join(&uid)
    };
    if let Err(e) = std::fs::create_dir_all(&host_workspace) {
        return err(format!("cannot create workspace: {e}"));
    }

    // Seed files.
    if let Some(files) = files {
        for (rel, content) in files {
            let p = host_workspace.join(rel);
            if let Some(parent) = p.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::write(&p, content);
        }
    }

    let container_name = format!("sandbox-{}", short_uid());
    let mut cmd = Command::new("docker");
    cmd.arg("run")
        .arg("--rm")
        .arg("--name")
        .arg(&container_name)
        .arg("--network")
        .arg(&cfg.network)
        .arg("-e")
        .arg("AGENT=opencode")
        .arg("-e")
        .arg(format!("TASK={task}"))
        .arg("-e")
        .arg(format!("REPO_URL={}", repo_url.unwrap_or("")))
        .arg("-e")
        .arg(format!("MODEL={}", model.unwrap_or("")))
        .arg("-e")
        .arg("NO_COLOR=1")
        .arg("-e")
        .arg("TERM=dumb");
    for var in ["LLAMA_CPP_URL", "LLAMA_CPP_MODEL"] {
        if let Ok(val) = std::env::var(var) {
            if !val.is_empty() {
                cmd.arg("-e").arg(format!("{var}={val}"));
            }
        }
    }
    cmd.arg("--cpus")
        .arg(&cfg.cpus)
        .arg("--memory")
        .arg(&cfg.mem)
        .arg("-v")
        .arg(format!("{}:/workspace:rw", host_workspace.display()))
        .arg(&cfg.image)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let result = run_streaming(cmd, cfg.timeout, &container_name, sink).await;

    let output = match result {
        Ok((code, lines)) => {
            if code != 0 {
                let _ = std::fs::remove_dir_all(&host_workspace);
                return err(format!(
                    "sandbox exited with code {code}.\n{}",
                    lines.trim()
                ));
            }
            lines
        }
        Err(e) => {
            let _ = tokio::process::Command::new("docker")
                .arg("kill")
                .arg(&container_name)
                .output()
                .await;
            let _ = std::fs::remove_dir_all(&host_workspace);
            return err(format!("sandbox failed: {e}"));
        }
    };

    let artifacts =
        collect_workspace_files(&host_workspace, &cfg.artifacts_dir, cfg.max_artifact_mb);
    let _ = std::fs::remove_dir_all(&host_workspace);

    let content = {
        let t = output.trim();
        if t.is_empty() {
            "(no output)".to_string()
        } else {
            t.to_string()
        }
    };
    OpencodeOutput {
        content,
        artifact_paths: artifacts,
    }
}

fn err(msg: String) -> OpencodeOutput {
    OpencodeOutput {
        content: format!("Error: {msg}"),
        artifact_paths: vec![],
    }
}

/// Spawn `cmd`, stream merged stdout/stderr lines to `sink`, and return (exit_code, full_output).
async fn run_streaming(
    mut cmd: Command,
    timeout: Duration,
    container_name: &str,
    sink: Option<&dyn TextSink>,
) -> anyhow::Result<(i32, String)> {
    let mut child = cmd.spawn()?;
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    if let Some(out) = stdout {
        let tx = tx.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(out).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let _ = tx.send(line);
            }
        });
    }
    if let Some(err) = stderr {
        let tx = tx.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(err).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let _ = tx.send(line);
            }
        });
    }
    drop(tx);

    let mut collected = String::new();
    let collect = async {
        while let Some(line) = rx.recv().await {
            collected.push_str(&line);
            collected.push('\n');
            if let Some(s) = sink {
                s.push(&line).await;
            }
        }
        let status = child.wait().await?;
        Ok::<i32, anyhow::Error>(status.code().unwrap_or(-1))
    };

    match tokio::time::timeout(timeout, collect).await {
        Ok(code) => Ok((code?, collected)),
        Err(_) => {
            let _ = Command::new("docker")
                .arg("kill")
                .arg(container_name)
                .output()
                .await;
            anyhow::bail!("timed out after {}s", timeout.as_secs())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn setup() -> (TempDir, PathBuf, PathBuf) {
        let tmp = TempDir::new().unwrap();
        let ws = tmp.path().join("workspace");
        std::fs::create_dir_all(&ws).unwrap();
        let artifacts = tmp.path().join("artifacts");
        (tmp, ws, artifacts)
    }

    #[test]
    fn empty_workspace_returns_empty() {
        let (_t, ws, art) = setup();
        assert!(collect_workspace_files(&ws, &art, 24.0).is_empty());
    }

    #[test]
    fn regular_file_is_collected() {
        let (_t, ws, art) = setup();
        std::fs::write(ws.join("hello.py"), "print('hi')").unwrap();
        let r = collect_workspace_files(&ws, &art, 24.0);
        assert_eq!(r.len(), 1);
        assert!(r[0].to_string_lossy().contains("hello"));
    }

    #[test]
    fn excluded_opencode_json_is_skipped() {
        let (_t, ws, art) = setup();
        std::fs::write(ws.join("opencode.json"), "{}").unwrap();
        assert!(collect_workspace_files(&ws, &art, 24.0).is_empty());
    }

    #[test]
    fn excluded_dot_opencode_json_is_skipped() {
        let (_t, ws, art) = setup();
        std::fs::write(ws.join(".opencode.json"), "{}").unwrap();
        assert!(collect_workspace_files(&ws, &art, 24.0).is_empty());
    }

    #[test]
    fn dotfile_is_skipped() {
        let (_t, ws, art) = setup();
        std::fs::write(ws.join(".gitignore"), "*.pyc").unwrap();
        assert!(collect_workspace_files(&ws, &art, 24.0).is_empty());
    }

    #[test]
    fn dotdir_contents_are_skipped() {
        let (_t, ws, art) = setup();
        let hidden = ws.join(".git");
        std::fs::create_dir_all(&hidden).unwrap();
        std::fs::write(hidden.join("config"), "data").unwrap();
        assert!(collect_workspace_files(&ws, &art, 24.0).is_empty());
    }

    #[test]
    fn oversized_file_is_skipped() {
        let (_t, ws, art) = setup();
        std::fs::write(ws.join("big.txt"), "data").unwrap();
        assert!(collect_workspace_files(&ws, &art, 0.0).is_empty());
    }

    #[test]
    fn multiple_files_all_collected() {
        let (_t, ws, art) = setup();
        std::fs::write(ws.join("a.py"), "a").unwrap();
        std::fs::write(ws.join("b.sh"), "b").unwrap();
        assert_eq!(collect_workspace_files(&ws, &art, 24.0).len(), 2);
    }

    #[test]
    fn nested_file_is_collected() {
        let (_t, ws, art) = setup();
        let sub = ws.join("src");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(sub.join("main.py"), "code").unwrap();
        assert_eq!(collect_workspace_files(&ws, &art, 24.0).len(), 1);
    }

    #[test]
    fn collected_files_exist_in_artifacts_dir() {
        let (_t, ws, art) = setup();
        std::fs::write(ws.join("out.txt"), "output").unwrap();
        let r = collect_workspace_files(&ws, &art, 24.0);
        assert_eq!(r.len(), 1);
        assert!(r[0].is_file());
    }

    #[test]
    fn excluded_filenames_constant() {
        assert!(EXCLUDED_FILENAMES.contains(&"opencode.json"));
        assert!(EXCLUDED_FILENAMES.contains(&".opencode.json"));
    }

    #[test]
    fn definition_requires_task() {
        assert_eq!(
            definition()["input_schema"]["required"],
            serde_json::json!(["task"])
        );
    }
}
