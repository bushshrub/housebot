//! Sandbox tool definitions and adapters.
//!
//! These tools call into the `housebot-sandbox` crate to create and interact
//! with disposable code-inspection containers.  They contain no Docker logic.
//!
//! Authorization (owner-only) is enforced in two places:
//!   1. Tool definitions are only exposed to authorized users.
//!   2. The dispatch arm rejects unauthorized calls again.

use std::sync::Arc;

use serde_json::{json, Value};
use tokio::sync::Mutex;

use housebot_sandbox::{NetworkAccess, Sandbox, SandboxClient};

/// A sandbox that is lazily created on first tool use within a single
/// `Agent::run` invocation, and destroyed when the agent finishes.
pub struct LazySandbox {
    client: SandboxClient,
    inner: Arc<Mutex<Option<Sandbox>>>,
    /// Track whether any sandbox tool has been called (to provide better errors).
    started: Arc<Mutex<bool>>,
}

impl LazySandbox {
    pub fn new(client: SandboxClient) -> Self {
        Self {
            client,
            inner: Arc::new(Mutex::new(None)),
            started: Arc::new(Mutex::new(false)),
        }
    }

    /// Get or create the sandbox container.
    async fn get_or_start(&self, network: NetworkAccess) -> Result<Sandbox, String> {
        let mut guard = self.inner.lock().await;
        if let Some(ref sandbox) = *guard {
            return Ok(sandbox.clone());
        }

        let sandbox = self.client.start(network).await?;
        *self.started.lock().await = true;
        let result = sandbox.clone();
        *guard = Some(sandbox);
        Ok(result)
    }

    /// Destroy the sandbox container if it was started.
    pub async fn close(&self) {
        let mut guard = self.inner.lock().await;
        if let Some(sandbox) = guard.take() {
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
}

fn truncate_output(s: &str) -> String {
    const MAX: usize = 64_000;
    if s.len() > MAX {
        let mut t = s[..MAX].to_string();
        t.push_str("\n... (truncated)");
        t
    } else {
        s.to_string()
    }
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
