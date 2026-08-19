//! Sandbox tool definitions and adapters.
//!
//! These tools call into the `housebot-sandbox` crate to create and interact
//! with disposable code-inspection containers.  They contain no Docker logic.
//!
//! Available to all users.

use std::sync::Arc;

use serde_json::{json, Value};
use tokio::sync::Mutex;

use housebot_sandbox::{NetworkAccess, Sandbox, SandboxClient};

/// A per-turn handle to the session's sandbox, attached on first tool use.
///
/// The container itself belongs to the session, not to this handle: sandboxd
/// keeps it alive between turns so the workspace survives, and reaps it once
/// the session falls idle.
pub struct LazySandbox {
    client: SandboxClient,
    session_key: String,
    inner: Arc<Mutex<Option<(Sandbox, NetworkAccess)>>>,
    /// Track whether any sandbox tool has been called (to provide better errors).
    started: Arc<Mutex<bool>>,
}

impl LazySandbox {
    pub fn new(client: SandboxClient, session_key: impl Into<String>) -> Self {
        Self {
            client,
            session_key: session_key.into(),
            inner: Arc::new(Mutex::new(None)),
            started: Arc::new(Mutex::new(false)),
        }
    }

    /// Attach to the session's sandbox, starting one if the session has none.
    ///
    /// A container's network mode is fixed at creation, so a request for
    /// internet access against a networkless sandbox replaces it: sandboxd
    /// would otherwise refuse, leaving the session unable to clone for as long
    /// as it lives. The replacement discards the workspace, which is why only
    /// `NetworkAccess::PublicInternet` triggers it — no networkless tool ever
    /// throws away another tool's files.
    async fn get_or_start(&self, network: NetworkAccess) -> Result<Sandbox, String> {
        let mut guard = self.inner.lock().await;
        match guard.take() {
            Some((sandbox, existing))
                if existing == NetworkAccess::PublicInternet || network == NetworkAccess::None =>
            {
                let result = sandbox.clone();
                *guard = Some((sandbox, existing));
                return Ok(result);
            }
            Some((sandbox, _)) => {
                let _ = sandbox.close().await;
            }
            None => {}
        }

        let sandbox = self.client.start(&self.session_key, network).await?;
        *self.started.lock().await = true;
        let result = sandbox.clone();
        *guard = Some((sandbox, network));
        Ok(result)
    }

    /// Discard the session's workspace before the idle timeout would.
    pub async fn close(&self) {
        let mut guard = self.inner.lock().await;
        if let Some((sandbox, _)) = guard.take() {
            let _ = sandbox.close().await;
        }
    }

    /// Whether any sandbox tool has been called.
    pub async fn is_started(&self) -> bool {
        *self.started.lock().await
    }

    // ── Tool operations ─────────────────────────────────────────────────

    pub async fn clone_repository(
        &self,
        url: &str,
        branch: Option<&str>,
    ) -> Result<String, String> {
        let sandbox = self.get_or_start(NetworkAccess::PublicInternet).await?;
        let result = sandbox.clone_repository(url, branch).await?;
        Ok(format!(
            "Clone complete (exit {})\n{}",
            result.exit_code,
            truncate_output(&result.stdout),
        ))
    }

    pub async fn list_files(&self, path: &str, max_depth: Option<u32>) -> Result<String, String> {
        let sandbox = self.get_or_start(NetworkAccess::None).await?;
        let entries = sandbox.list_files(path, max_depth).await?;
        if entries.is_empty() {
            return Ok("(empty directory or path not found)".to_string());
        }
        let mut lines = Vec::new();
        for entry in &entries {
            let size = entry.size.map(|s| format!(" ({})", s)).unwrap_or_default();
            lines.push(format!("{} {}{}", entry.entry_type, entry.name, size));
        }
        Ok(lines.join("\n"))
    }

    pub async fn search_code(
        &self,
        query: &str,
        path: Option<&str>,
        glob: Option<&str>,
    ) -> Result<String, String> {
        let sandbox = self.get_or_start(NetworkAccess::None).await?;
        let result = sandbox.search_code(query, path, glob).await?;
        if result.matches.is_empty() {
            return Ok("No matches found.".to_string());
        }
        let mut lines = Vec::new();
        for m in &result.matches {
            lines.push(format!("{}:{}:{}", m.path, m.line_number, m.line));
        }
        let mut text = lines.join("\n");
        if result.truncated {
            text.push_str("\n... (truncated)");
        }
        Ok(text)
    }

    pub async fn read_file(
        &self,
        path: &str,
        start_line: Option<u32>,
        end_line: Option<u32>,
    ) -> Result<String, String> {
        let sandbox = self.get_or_start(NetworkAccess::None).await?;
        let result = sandbox.read_file(path, start_line, end_line).await?;
        if result.binary {
            return Ok("(binary file — cannot display)".to_string());
        }
        let mut text = result.contents;
        if result.truncated {
            text.push_str("\n... (truncated)");
        }
        Ok(text)
    }

    pub async fn run(
        &self,
        command: &str,
        working_dir: Option<&str>,
        timeout_secs: Option<u64>,
    ) -> Result<String, String> {
        let sandbox = self.get_or_start(NetworkAccess::None).await?;
        let result = sandbox.run(command, working_dir, timeout_secs).await?;
        let mut parts = Vec::new();
        if !result.stdout.is_empty() {
            parts.push(truncate_output(&result.stdout));
        }
        if !result.stderr.is_empty() {
            parts.push(format!("[stderr]\n{}", truncate_output(&result.stderr)));
        }
        parts.push(format!("Exit code: {}", result.exit_code));
        let mut text = parts.join("\n");
        if result.truncated {
            text.push_str("\n(output truncated)");
        }
        Ok(text)
    }

    /// Copy a skill script into the workspace and execute it.
    ///
    /// Requests `NetworkAccess::None`, so a skill script never *causes* a
    /// session's sandbox to gain network it would not otherwise have.
    ///
    /// It does not guarantee the script runs without network: if the session
    /// already started networked — say a `sandbox_clone_repository` ran first —
    /// the script executes in that container. The containment that matters is
    /// the sandbox itself (gVisor, tmpfs, no host mounts, no secrets), not the
    /// network mode.
    pub async fn run_skill_script(
        &self,
        skill: &str,
        file: &str,
        source: &str,
        args: &[String],
        timeout_secs: Option<u64>,
    ) -> Result<String, String> {
        let interpreter = script_interpreter(file)?;

        let sandbox = self.get_or_start(NetworkAccess::None).await?;
        let path = format!("skills/{skill}/{file}");
        sandbox.write_file(&path, source, true).await?;

        let quoted: Vec<String> = args.iter().map(|a| shell_quote(a)).collect();
        let command = format!("{interpreter} /workspace/{path} {}", quoted.join(" "));
        self.run(&command, None, timeout_secs).await
    }
}

/// Map a script's extension to its interpreter, rejecting anything else.
fn script_interpreter(file: &str) -> Result<&'static str, String> {
    match file.rsplit_once('.').map(|(_, ext)| ext) {
        Some("py") => Ok("python3"),
        Some("sh") => Ok("bash"),
        Some("js") => Ok("node"),
        _ => Err(format!(
            "Error: cannot run '{file}' — supported script types are .py, .sh, and .js."
        )),
    }
}

/// Single-quote an argument for the shell, escaping embedded quotes.
fn shell_quote(arg: &str) -> String {
    format!("'{}'", arg.replace('\'', "'\\''"))
}

fn truncate_output(s: &str) -> String {
    const MAX: usize = 64_000;
    if s.len() > MAX {
        let mut end = MAX;
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        let mut t = s[..end].to_string();
        t.push_str("\n... (truncated)");
        t
    } else {
        s.to_string()
    }
}

pub fn all_definitions() -> Vec<Value> {
    vec![
        sandbox_clone_repository_definition(),
        sandbox_list_files_definition(),
        sandbox_search_code_definition(),
        sandbox_read_file_definition(),
        sandbox_run_definition(),
    ]
}

// ── Tool definitions ────────────────────────────────────────────────────────

pub fn sandbox_clone_repository_definition() -> Value {
    json!({
        "name": "sandbox_clone_repository",
        "description": "Clone a public HTTPS repository into the sandbox for inspection. \
            Use this before other sandbox tools when you need to examine repository contents.",
        "input_schema": {
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "Public HTTPS repository URL (e.g. https://github.com/owner/repo). \
                        SSH URLs, credentials, and private-network URLs are rejected."
                },
                "branch": {
                    "type": "string",
                    "description": "Optional branch name, tag, or commit hash to clone."
                }
            },
            "required": ["url"]
        }
    })
}

pub fn sandbox_list_files_definition() -> Value {
    json!({
        "name": "sandbox_list_files",
        "description": "List files in a workspace directory within the sandbox. \
            Large generated directories (.git, target, node_modules) are excluded automatically.",
        "input_schema": {
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Relative workspace path (e.g. 'src' or 'repo/src')."
                },
                "max_depth": {
                    "type": "integer",
                    "description": "Maximum directory depth (1–10, default 3)."
                }
            },
            "required": ["path"]
        }
    })
}

pub fn sandbox_search_code_definition() -> Value {
    json!({
        "name": "sandbox_search_code",
        "description": "Search source code text in the sandbox workspace using ripgrep. \
            Returns matching file paths, line numbers, and line content.",
        "input_schema": {
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Search query (supports ripgrep regex syntax)."
                },
                "path": {
                    "type": "string",
                    "description": "Optional relative workspace path to restrict the search."
                },
                "glob": {
                    "type": "string",
                    "description": "Optional file glob pattern (e.g. '*.rs' or '*.py')."
                }
            },
            "required": ["query"]
        }
    })
}

pub fn sandbox_read_file_definition() -> Value {
    json!({
        "name": "sandbox_read_file",
        "description": "Read a bounded section of a text file from the sandbox workspace. \
            Binary files are detected and rejected.",
        "input_schema": {
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Relative workspace path (e.g. 'repo/src/main.rs')."
                },
                "start_line": {
                    "type": "integer",
                    "description": "Optional start line (1-indexed)."
                },
                "end_line": {
                    "type": "integer",
                    "description": "Optional end line (inclusive)."
                }
            },
            "required": ["path"]
        }
    })
}

pub fn sandbox_run_definition() -> Value {
    json!({
        "name": "sandbox_run",
        "description": "Run a short Bash command inside the sandbox. \
            Use this to run tests, execute existing scripts, reproduce errors, \
            compile code, inspect git metadata, or run small commands. \
            Output is limited to 64 KiB. Timeout defaults to 30 seconds (max 300). \
            A non-zero exit code is NOT an error — it is returned to you so you can explain it.",
        "input_schema": {
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "Bash command to execute inside the sandbox."
                },
                "working_dir": {
                    "type": "string",
                    "description": "Optional relative working directory for the command."
                },
                "timeout": {
                    "type": "integer",
                    "description": "Optional timeout in seconds (1–300, default 30)."
                }
            },
            "required": ["command"]
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_script_types_map_to_interpreters() {
        assert_eq!(script_interpreter("run.py").unwrap(), "python3");
        assert_eq!(script_interpreter("run.sh").unwrap(), "bash");
        assert_eq!(script_interpreter("run.js").unwrap(), "node");
    }

    #[test]
    fn unknown_script_types_are_refused_before_any_sandbox_work() {
        for file in ["run", "run.rb", "run.exe", "a.out", ""] {
            assert!(
                script_interpreter(file).is_err(),
                "{file} should be refused"
            );
        }
    }

    #[test]
    fn script_arguments_cannot_break_out_of_their_quoting() {
        let quoted = shell_quote("; rm -rf /");
        assert_eq!(quoted, "'; rm -rf /'");
        assert_eq!(shell_quote("it's"), r"'it'\''s'");
    }

    /// A stand-in sandboxd that records the methods it is asked for.
    async fn fake_sandboxd(path: std::path::PathBuf, calls: Arc<Mutex<Vec<String>>>) {
        let listener = tokio::net::UnixListener::bind(&path).expect("bind fake sandboxd");
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                return;
            };
            let calls = Arc::clone(&calls);
            tokio::spawn(async move {
                use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
                let (reader, mut writer) = stream.into_split();
                let mut lines = BufReader::new(reader).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    let request: Value = serde_json::from_str(&line).expect("request is JSON");
                    let method = request["method"].as_str().unwrap_or_default().to_string();
                    let result = match method.as_str() {
                        "start" => {
                            calls.lock().await.push(format!(
                                "start:{}",
                                request["params"]["network"].as_str().unwrap_or_default()
                            ));
                            json!({"sandbox_id": "sandbox-1"})
                        }
                        "close" => {
                            calls.lock().await.push("close".to_string());
                            json!({"closed": true})
                        }
                        "list_files" => {
                            calls.lock().await.push("list_files".to_string());
                            json!([])
                        }
                        "clone_repository" => {
                            calls.lock().await.push("clone_repository".to_string());
                            json!({"exit_code": 0, "stdout": "", "stderr": "", "truncated": false})
                        }
                        other => panic!("unexpected method {other}"),
                    };
                    let response = json!({"id": request["id"], "result": result});
                    let mut bytes = serde_json::to_vec(&response).expect("serialise response");
                    bytes.push(b'\n');
                    let _ = writer.write_all(&bytes).await;
                }
            });
        }
    }

    async fn lazy_sandbox_against_fake() -> (LazySandbox, Arc<Mutex<Vec<String>>>, tempfile::TempDir)
    {
        let dir = tempfile::tempdir().expect("temp dir");
        let socket = dir.path().join("sandbox.sock");
        let calls = Arc::new(Mutex::new(Vec::new()));
        tokio::spawn(fake_sandboxd(socket.clone(), Arc::clone(&calls)));
        for _ in 0..100 {
            if socket.exists() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        let client = SandboxClient::new(socket.to_string_lossy().to_string());
        (LazySandbox::new(client, "session-1"), calls, dir)
    }

    #[tokio::test]
    async fn cloning_replaces_a_networkless_sandbox_instead_of_failing() {
        let (sandbox, calls, _dir) = lazy_sandbox_against_fake().await;
        sandbox.list_files(".", None).await.unwrap();
        sandbox
            .clone_repository("https://github.com/owner/repo", None)
            .await
            .unwrap();
        assert_eq!(
            *calls.lock().await,
            vec![
                "start:none",
                "list_files",
                "close",
                "start:public",
                "clone_repository"
            ]
        );
    }

    #[tokio::test]
    async fn a_networked_sandbox_is_reused_by_networkless_tools() {
        let (sandbox, calls, _dir) = lazy_sandbox_against_fake().await;
        sandbox
            .clone_repository("https://github.com/owner/repo", None)
            .await
            .unwrap();
        sandbox.list_files(".", None).await.unwrap();
        assert_eq!(
            *calls.lock().await,
            vec!["start:public", "clone_repository", "list_files"]
        );
    }
}
