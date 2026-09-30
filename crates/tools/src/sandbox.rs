//! Sandbox tool definitions and adapters.
//!
//! These tools call into the `housebot-sandbox` crate to read, write, and run
//! commands in the user's session container.  They contain no Docker logic.
//!
//! Available to all users.

use std::future::Future;
use std::sync::Arc;

use serde_json::{json, Value};
use tokio::sync::Mutex;

use housebot_sandbox::{is_unknown_sandbox, NetworkAccess, Sandbox, SandboxClient};
use housebot_skills::{BundleKind, Skills};

/// Workspace directory that holds a copy of every skill.
pub const SKILLS_DIR: &str = "skills";

const RESET_NOTICE: &str = "[The sandbox was recycled and is empty, so earlier files, \
    installed packages and clones are gone. Recreate what you need.]\n";

/// A per-turn handle to the session's sandbox, attached on first tool use.
///
/// The container itself belongs to the session, not to this handle: sandboxd
/// keeps it alive between turns so the workspace survives, and reaps it once
/// the session falls idle.
pub struct LazySandbox {
    client: SandboxClient,
    session_key: String,
    skills: Skills,
    inner: Arc<Mutex<Option<Sandbox>>>,
}

impl LazySandbox {
    pub fn new(client: SandboxClient, session_key: impl Into<String>, skills: Skills) -> Self {
        Self {
            client,
            session_key: session_key.into(),
            skills,
            inner: Arc::new(Mutex::new(None)),
        }
    }

    /// Attach to the session's sandbox, starting one if the session has none.
    ///
    /// The skills are copied in on the first attach of every turn, so a skill
    /// saved or deleted since the last turn is reflected in the workspace.
    async fn attach(&self) -> Result<Sandbox, String> {
        let mut guard = self.inner.lock().await;
        if let Some(sandbox) = guard.as_ref() {
            return Ok(sandbox.clone());
        }
        let sandbox = self
            .client
            .start(&self.session_key, NetworkAccess::PublicInternet)
            .await?;
        self.copy_skills(&sandbox).await;
        *guard = Some(sandbox.clone());
        Ok(sandbox)
    }

    /// Run `op` against the sandbox, attaching to a fresh one once if sandboxd
    /// reaped the cached sandbox mid-turn. The flag says the workspace was
    /// replaced, so callers can tell the model its files are gone.
    async fn with_sandbox<T, F, Fut>(&self, op: F) -> Result<(T, bool), String>
    where
        F: Fn(Sandbox) -> Fut,
        Fut: Future<Output = Result<T, String>>,
    {
        let sandbox = self.attach().await?;
        let stale_id = sandbox.id().to_string();
        match op(sandbox).await {
            Err(error) if is_unknown_sandbox(&error) => {
                tracing::warn!(%error, "Sandbox was reaped mid-turn; starting a new one");
                let mut guard = self.inner.lock().await;
                if guard.as_ref().is_some_and(|s| s.id() == stale_id) {
                    *guard = None;
                }
                drop(guard);
                let fresh = self.attach().await?;
                Ok((op(fresh).await?, true))
            }
            result => Ok((result?, false)),
        }
    }

    /// Replace `skills/` in the workspace with the current skill store. A skill
    /// that fails to copy is skipped so it cannot take the sandbox down.
    async fn copy_skills(&self, sandbox: &Sandbox) {
        if let Err(error) = sandbox
            .run(&format!("rm -rf /workspace/{SKILLS_DIR}"), None, None)
            .await
        {
            tracing::warn!(%error, "Could not clear the sandbox skills directory");
        }
        for skill in self.skills.load_all().await.values() {
            if let Err(error) = self.copy_skill(sandbox, skill).await {
                tracing::warn!(%error, skill = %skill.name, "Could not copy a skill into the sandbox");
            }
        }
    }

    async fn copy_skill(
        &self,
        sandbox: &Sandbox,
        skill: &housebot_skills::Skill,
    ) -> Result<(), String> {
        let dir = format!("{SKILLS_DIR}/{}", skill.name);
        sandbox
            .write_file(&format!("{dir}/SKILL.md"), &skill.to_skill_md(), false)
            .await?;
        for (kind, files) in [
            (BundleKind::References, &skill.references),
            (BundleKind::Scripts, &skill.scripts),
        ] {
            for file in files {
                let content = self.skills.read_bundled(&skill.name, kind, file).await?;
                let path = format!("{dir}/{}/{file}", kind.dir_name());
                sandbox
                    .write_file(&path, &content, kind == BundleKind::Scripts)
                    .await?;
            }
        }
        Ok(())
    }

    // ── Tool operations ─────────────────────────────────────────────────

    pub async fn read(
        &self,
        path: &str,
        start_line: Option<u32>,
        end_line: Option<u32>,
    ) -> Result<String, String> {
        let (result, reset) = self
            .with_sandbox(
                |sandbox| async move { sandbox.read_file(path, start_line, end_line).await },
            )
            .await?;
        if result.binary {
            return Ok(with_reset_notice(
                "(binary file — cannot display)".to_string(),
                reset,
            ));
        }
        let mut text = result.contents;
        if result.truncated {
            text.push_str("\n... (truncated)");
        }
        Ok(with_reset_notice(text, reset))
    }

    pub async fn write(&self, path: &str, content: &str) -> Result<String, String> {
        let (result, reset) = self
            .with_sandbox(|sandbox| async move { sandbox.write_file(path, content, false).await })
            .await?;
        Ok(with_reset_notice(
            format!("Wrote {} bytes to {}", result.bytes_written, result.path),
            reset,
        ))
    }

    pub async fn edit(
        &self,
        path: &str,
        old_string: &str,
        new_string: &str,
        replace_all: bool,
    ) -> Result<String, String> {
        let (result, reset) = self
            .with_sandbox(|sandbox| async move {
                sandbox
                    .edit_file(path, old_string, new_string, replace_all)
                    .await
            })
            .await?;
        Ok(with_reset_notice(
            format!(
                "Edited {} ({} replacement{})",
                result.path,
                result.replacements,
                if result.replacements == 1 { "" } else { "s" }
            ),
            reset,
        ))
    }

    pub async fn shell(
        &self,
        command: &str,
        working_dir: Option<&str>,
        timeout_secs: Option<u64>,
    ) -> Result<String, String> {
        let (result, reset) = self
            .with_sandbox(
                |sandbox| async move { sandbox.run(command, working_dir, timeout_secs).await },
            )
            .await?;
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
        Ok(with_reset_notice(text, reset))
    }
}

fn with_reset_notice(text: String, reset: bool) -> String {
    if reset {
        format!("{RESET_NOTICE}{text}")
    } else {
        text
    }
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
        read_definition(),
        write_definition(),
        edit_definition(),
        shell_definition(),
    ]
}

// ── Tool definitions ────────────────────────────────────────────────────────

pub fn read_definition() -> Value {
    json!({
        "name": "read",
        "description": "Read a bounded section of a text file from your sandbox workspace \
            (/workspace). Binary files are detected and rejected. Skills are in skills/<name>/.",
        "input_schema": {
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path relative to /workspace (e.g. 'skills/pdf/SKILL.md' or 'repo/src/main.rs')."
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

pub fn write_definition() -> Value {
    json!({
        "name": "write",
        "description": "Create a new text file, or fully replace one, in your sandbox workspace \
            (/workspace). Parent directories are created as needed. To change part of an \
            existing file use edit instead of rewriting it.",
        "input_schema": {
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path relative to /workspace."
                },
                "content": {
                    "type": "string",
                    "description": "The full file content."
                }
            },
            "required": ["path", "content"]
        }
    })
}

pub fn edit_definition() -> Value {
    json!({
        "name": "edit",
        "description": "Make a targeted change to an existing text file in your sandbox \
            workspace (/workspace) by replacing an exact string. Prefer this over write \
            for any change to a file that already exists. old_string must match the file \
            exactly, including whitespace, and must be unique in the file unless \
            replace_all is set. Files over 256 KiB cannot be edited this way.",
        "input_schema": {
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path relative to /workspace."
                },
                "old_string": {
                    "type": "string",
                    "description": "The exact text to replace. Include enough surrounding lines to make it unique."
                },
                "new_string": {
                    "type": "string",
                    "description": "The text to put in its place. Must differ from old_string."
                },
                "replace_all": {
                    "type": "boolean",
                    "description": "Replace every occurrence instead of requiring a unique match (default false)."
                }
            },
            "required": ["path", "old_string", "new_string"]
        }
    })
}

pub fn shell_definition() -> Value {
    json!({
        "name": "shell",
        "description": "Run a Bash command in your sandbox. The sandbox has internet access, \
            git, Python, Node, and Rust. Use it to clone repositories, list and search files \
            (ls, rg), run tests, and run skill scripts. The working directory defaults to \
            /workspace. Files persist between turns until the sandbox has been idle for 5 \
            minutes. Output is limited to 64 KiB. Timeout defaults to 30 seconds (max 300). \
            A non-zero exit code is NOT an error — it is returned to you so you can explain it.",
        "input_schema": {
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "Bash command to execute."
                },
                "working_dir": {
                    "type": "string",
                    "description": "Optional working directory relative to /workspace."
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
    use housebot_skills::Skill;

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
                    let params = &request["params"];
                    let result = match request["method"].as_str().unwrap_or_default() {
                        "start" => {
                            calls
                                .lock()
                                .await
                                .push(format!("start:{}", params["network"].as_str().unwrap()));
                            json!({"sandbox_id": "sandbox-1"})
                        }
                        "run" => {
                            calls
                                .lock()
                                .await
                                .push(format!("run:{}", params["command"].as_str().unwrap()));
                            json!({"exit_code": 0, "stdout": "", "stderr": "", "truncated": false})
                        }
                        "write_file" => {
                            let path = params["path"].as_str().unwrap().to_string();
                            calls.lock().await.push(format!("write:{path}"));
                            json!({"path": format!("/workspace/{path}"), "bytes_written": 1})
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

    #[tokio::test]
    async fn the_first_call_starts_a_networked_sandbox_and_copies_the_skills() {
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

        let skills = Skills::new(dir.path().join("skills"));
        skills
            .save(Skill {
                name: "greet".into(),
                description: Some("Say hello".into()),
                instructions: "Say hello.".into(),
                ..Skill::default()
            })
            .await
            .unwrap();
        let client = SandboxClient::new(socket.to_string_lossy().to_string());
        let sandbox = LazySandbox::new(client, "session-1", skills);

        sandbox.shell("ls", None, None).await.unwrap();
        sandbox.shell("pwd", None, None).await.unwrap();

        let calls = calls.lock().await;
        assert_eq!(calls[0], "start:public");
        assert_eq!(calls[1], "run:rm -rf /workspace/skills");
        assert!(calls.contains(&"write:skills/greet/SKILL.md".to_string()));
        assert_eq!(
            calls.iter().filter(|c| c.starts_with("start")).count(),
            1,
            "one turn attaches once: {calls:?}"
        );
        assert_eq!(&calls[calls.len() - 2..], ["run:ls", "run:pwd"]);
    }

    #[tokio::test]
    async fn a_second_unknown_sandbox_error_is_returned_instead_of_retried_again() {
        let dir = tempfile::tempdir().expect("temp dir");
        let socket = dir.path().join("sandbox.sock");
        let listener = tokio::net::UnixListener::bind(&socket).expect("bind fake sandboxd");
        let starts = Arc::new(Mutex::new(0u32));
        let seen = Arc::clone(&starts);
        tokio::spawn(async move {
            use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    return;
                };
                let starts = Arc::clone(&seen);
                tokio::spawn(async move {
                    let (reader, mut writer) = stream.into_split();
                    let mut lines = BufReader::new(reader).lines();
                    while let Ok(Some(line)) = lines.next_line().await {
                        let request: Value = serde_json::from_str(&line).expect("request is JSON");
                        let reply = if request["method"] == "start" {
                            *starts.lock().await += 1;
                            json!({"id": request["id"], "result": {"sandbox_id": "sandbox-x"}})
                        } else {
                            json!({"id": request["id"], "error": "unknown sandbox: sandbox-x"})
                        };
                        let mut bytes = serde_json::to_vec(&reply).expect("serialise response");
                        bytes.push(b'\n');
                        let _ = writer.write_all(&bytes).await;
                    }
                });
            }
        });
        for _ in 0..100 {
            if socket.exists() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }

        let skills = Skills::new(dir.path().join("skills"));
        let client = SandboxClient::new(socket.to_string_lossy().to_string());
        let sandbox = LazySandbox::new(client, "session-1", skills);

        let error = sandbox.shell("ls", None, None).await.unwrap_err();

        assert!(is_unknown_sandbox(&error), "{error}");
        assert_eq!(*starts.lock().await, 2, "one retry, not a loop");
    }

    #[tokio::test]
    async fn a_reaped_sandbox_is_replaced_and_the_call_retried() {
        let dir = tempfile::tempdir().expect("temp dir");
        let socket = dir.path().join("sandbox.sock");
        let listener = tokio::net::UnixListener::bind(&socket).expect("bind fake sandboxd");
        let starts = Arc::new(Mutex::new(0u32));
        let seen = Arc::clone(&starts);
        tokio::spawn(async move {
            use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    return;
                };
                let starts = Arc::clone(&seen);
                tokio::spawn(async move {
                    let (reader, mut writer) = stream.into_split();
                    let mut lines = BufReader::new(reader).lines();
                    while let Ok(Some(line)) = lines.next_line().await {
                        let request: Value = serde_json::from_str(&line).expect("request is JSON");
                        let mut starts = starts.lock().await;
                        let reply = match request["method"].as_str().unwrap_or_default() {
                            "start" => {
                                *starts += 1;
                                json!({"id": request["id"], "result": {"sandbox_id": format!("sandbox-{starts}")}})
                            }
                            _ if request["params"]["sandbox_id"] == "sandbox-1" => {
                                json!({"id": request["id"], "error": "unknown sandbox: sandbox-1"})
                            }
                            _ => json!({"id": request["id"], "result": {
                                "exit_code": 0, "stdout": "ok", "stderr": "", "truncated": false
                            }}),
                        };
                        let mut bytes = serde_json::to_vec(&reply).expect("serialise response");
                        bytes.push(b'\n');
                        let _ = writer.write_all(&bytes).await;
                    }
                });
            }
        });
        for _ in 0..100 {
            if socket.exists() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }

        let skills = Skills::new(dir.path().join("skills"));
        let client = SandboxClient::new(socket.to_string_lossy().to_string());
        let sandbox = LazySandbox::new(client, "session-1", skills);

        let output = sandbox.shell("ls", None, None).await.unwrap();

        assert!(output.starts_with(RESET_NOTICE), "{output}");
        assert!(output.contains("Exit code: 0"));
        assert_eq!(*starts.lock().await, 2);
    }
}
