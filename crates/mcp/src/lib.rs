//! Minimal MCP (Model Context Protocol) client over stdio.
//!
//! Speaks newline-delimited JSON-RPC 2.0 to a child process: performs the `initialize`
//! handshake, lists tools, and calls them. Tool calls in the agent are sequential, so a
//! single request/response lock is sufficient.

use std::process::Stdio;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Duration;

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::Mutex;

const RESPONSE_TIMEOUT: Duration = Duration::from_secs(120);

/// A tool exposed by an MCP server.
#[derive(Debug, Clone)]
pub struct McpTool {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

/// A connected MCP server child process.
pub struct McpServer {
    /// Namespace prefix used to qualify this server's tool names (`prefix__tool`).
    pub prefix: String,
    /// Held across a full write-request/read-response exchange so concurrent
    /// callers cannot consume (and discard) each other's responses.
    io: Mutex<McpIo>,
    next_id: AtomicI64,
    tools_cache: Mutex<Option<Vec<McpTool>>>,
    _child: Child,
}

struct McpIo {
    stdin: ChildStdin,
    stdout: Lines<BufReader<ChildStdout>>,
}

/// Encode a JSON-RPC request as a single newline-terminated line.
fn build_request(id: i64, method: &str, params: Value) -> String {
    let mut line = json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": method,
        "params": params,
    })
    .to_string();
    line.push('\n');
    line
}

/// Extract the text payload from an MCP `tools/call` result value.
fn extract_text(result: &Value) -> String {
    let Some(content) = result.get("content").and_then(|c| c.as_array()) else {
        return String::new();
    };
    content
        .iter()
        .map(|item| {
            item.get("text")
                .and_then(|t| t.as_str())
                .map(|s| s.to_string())
                .unwrap_or_else(|| item.to_string())
        })
        .collect::<Vec<_>>()
        .join("\n")
}

impl McpServer {
    /// Spawn `command` and complete the MCP handshake. Returns `None` on any failure.
    pub async fn start(
        prefix: &str,
        command: &str,
        args: &[String],
        env: &[(String, String)],
    ) -> Option<Self> {
        let mut cmd = Command::new(command);
        cmd.args(args)
            .envs(env.iter().cloned())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            // Reap the child on any startup failure below (and whenever the
            // server handle itself is dropped) instead of leaking it.
            .kill_on_drop(true);
        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                tracing::error!("Failed to spawn MCP server '{prefix}': {e}");
                return None;
            }
        };
        let stdin = child.stdin.take()?;
        let stdout = child.stdout.take()?;
        let server = Self {
            prefix: prefix.to_string(),
            io: Mutex::new(McpIo {
                stdin,
                stdout: BufReader::new(stdout).lines(),
            }),
            next_id: AtomicI64::new(1),
            tools_cache: Mutex::new(None),
            _child: child,
        };

        if let Err(e) = server.handshake().await {
            tracing::error!("MCP server '{prefix}' handshake failed: {e}");
            return None;
        }
        tracing::info!("MCP server '{prefix}' ready");
        Some(server)
    }

    async fn handshake(&self) -> anyhow::Result<()> {
        self.request(
            "initialize",
            json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {"name": "house-chatbot", "version": "0.1.0"},
            }),
        )
        .await?;
        // Fire-and-forget the initialized notification.
        let mut line = json!({"jsonrpc": "2.0", "method": "notifications/initialized"}).to_string();
        line.push('\n');
        self.io
            .lock()
            .await
            .stdin
            .write_all(line.as_bytes())
            .await?;
        Ok(())
    }

    async fn request(&self, method: &str, params: Value) -> anyhow::Result<Value> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let line = build_request(id, method, params);
        // Lock acquisition, the write, and the response read all share one
        // deadline: a hung child (or a long queue behind one) must not let a
        // caller wait forever, and a child that stops reading stdin must not
        // block write_all while holding the io mutex.
        tokio::time::timeout(RESPONSE_TIMEOUT, async {
            let mut io = self.io.lock().await;
            io.stdin.write_all(line.as_bytes()).await?;
            io.stdin.flush().await?;
            while let Some(raw) = io.stdout.next_line().await? {
                let Ok(msg) = serde_json::from_str::<Value>(&raw) else {
                    continue; // skip non-JSON log lines
                };
                if msg.get("id").and_then(|v| v.as_i64()) != Some(id) {
                    continue; // notification or unrelated response
                }
                if let Some(err) = msg.get("error") {
                    anyhow::bail!("MCP error: {err}");
                }
                return Ok(msg.get("result").cloned().unwrap_or(Value::Null));
            }
            anyhow::bail!("MCP server closed before responding")
        })
        .await
        .map_err(|_| anyhow::anyhow!("MCP server did not respond within {RESPONSE_TIMEOUT:?}"))?
    }

    /// List every tool the server exposes (cached after first call).
    pub async fn list_tools(&self) -> Vec<McpTool> {
        {
            let cache = self.tools_cache.lock().await;
            if let Some(tools) = &*cache {
                return tools.clone();
            }
        }
        let result = match self.request("tools/list", json!({})).await {
            Ok(r) => r,
            Err(e) => {
                tracing::error!("tools/list failed for '{}': {e}", self.prefix);
                return vec![];
            }
        };
        let tools: Vec<McpTool> = result
            .get("tools")
            .and_then(|t| t.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|t| {
                        Some(McpTool {
                            name: t.get("name")?.as_str()?.to_string(),
                            description: t
                                .get("description")
                                .and_then(|d| d.as_str())
                                .unwrap_or("")
                                .to_string(),
                            input_schema: t
                                .get("inputSchema")
                                .cloned()
                                .unwrap_or_else(|| json!({"type": "object"})),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        *self.tools_cache.lock().await = Some(tools.clone());
        tools
    }

    /// Call a tool by name and return its concatenated text content.
    pub async fn call_tool(&self, name: &str, args: Value) -> anyhow::Result<String> {
        let result = self
            .request("tools/call", json!({"name": name, "arguments": args}))
            .await?;
        Ok(extract_text(&result))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_request_is_newline_terminated_jsonrpc() {
        let line = build_request(7, "tools/list", json!({"a": 1}));
        assert!(line.ends_with('\n'));
        let v: Value = serde_json::from_str(line.trim()).unwrap();
        assert_eq!(v["jsonrpc"], "2.0");
        assert_eq!(v["id"], 7);
        assert_eq!(v["method"], "tools/list");
        assert_eq!(v["params"]["a"], 1);
    }

    #[test]
    fn extract_text_joins_content_items() {
        let result = json!({"content": [{"type": "text", "text": "hello"}, {"type": "text", "text": "world"}]});
        assert_eq!(extract_text(&result), "hello\nworld");
    }

    #[test]
    fn extract_text_empty_when_no_content() {
        assert_eq!(extract_text(&json!({})), "");
    }

    #[test]
    fn extract_text_falls_back_to_json_for_non_text_items() {
        let result = json!({"content": [{"type": "image", "data": "x"}]});
        assert!(extract_text(&result).contains("image"));
    }

    /// A tiny shell responder answers every request with a `result` carrying
    /// the request's own `marker` param, so each caller can verify it received
    /// its own response. Before the protocol mutex spanned the whole exchange,
    /// one caller could consume and discard another's response — stranding
    /// that caller and, in the cross-consumption case, mixing up replies.
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread")]
    async fn concurrent_requests_each_get_their_own_response() {
        let responder = r#"while IFS= read -r line; do
            id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
            m=$(printf '%s' "$line" | sed -n 's/.*"marker":\([0-9][0-9]*\).*/\1/p')
            if [ -n "$id" ]; then
                printf '{"jsonrpc":"2.0","id":%s,"result":{"marker":%s}}\n' "$id" "${m:-0}"
            fi
        done"#;
        let server = McpServer::start("echo", "sh", &["-c".into(), responder.into()], &[])
            .await
            .expect("shell responder starts");
        let server = std::sync::Arc::new(server);
        let mut tasks = Vec::new();
        for i in 1..=16i64 {
            let server = std::sync::Arc::clone(&server);
            tasks.push(tokio::spawn(async move {
                let result = server
                    .request("tools/call", json!({"marker": i}))
                    .await
                    .unwrap();
                assert_eq!(
                    result["marker"].as_i64(),
                    Some(i),
                    "request {i} received someone else's response: {result}"
                );
            }));
        }
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            for task in tasks {
                task.await.unwrap();
            }
        })
        .await
        .expect("no request may be stranded waiting for its response");
    }
}
