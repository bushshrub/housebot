//! Minimal MCP (Model Context Protocol) client over stdio.
//!
//! Speaks newline-delimited JSON-RPC 2.0 to a child process: performs the `initialize`
//! handshake, lists tools, and calls them. Tool calls in the agent are sequential, so a
//! single request/response lock is sufficient.

use std::process::Stdio;
use std::sync::atomic::{AtomicI64, Ordering};

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::Mutex;

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
    stdin: Mutex<ChildStdin>,
    stdout: Mutex<Lines<BufReader<ChildStdout>>>,
    next_id: AtomicI64,
    tools_cache: Mutex<Option<Vec<McpTool>>>,
    _child: Child,
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
            .stderr(Stdio::null());
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
            stdin: Mutex::new(stdin),
            stdout: Mutex::new(BufReader::new(stdout).lines()),
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
        self.stdin.lock().await.write_all(line.as_bytes()).await?;
        Ok(())
    }

    async fn request(&self, method: &str, params: Value) -> anyhow::Result<Value> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let line = build_request(id, method, params);
        {
            let mut stdin = self.stdin.lock().await;
            stdin.write_all(line.as_bytes()).await?;
            stdin.flush().await?;
        }
        let mut stdout = self.stdout.lock().await;
        while let Some(raw) = stdout.next_line().await? {
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
    }

    /// List every tool the server exposes (cached after first call).
    pub async fn list_tools(&self) -> Vec<McpTool> {
        let mut cache = self.tools_cache.lock().await;
        if let Some(tools) = &*cache {
            return tools.clone();
        }
        let result = match self.request("tools/list", json!({})).await {
            Ok(r) => r,
            Err(e) => {
                tracing::error!("tools/list failed for '{}': {e}", self.prefix);
                return vec![];
            }
        };
        let Some(arr) = result.get("tools").and_then(|t| t.as_array()) else {
            tracing::error!(
                "tools/list response for '{}' missing valid 'tools' array",
                self.prefix
            );
            return vec![];
        };
        let tools: Vec<McpTool> = arr
            .iter()
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
            .collect();
        *cache = Some(tools.clone());
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
}
