//! Typed request/response protocol for the sandbox daemon.
//!
//! Messages are JSON-serialised and exchanged over a Unix socket,
//! one JSON object per line (`\n`-delimited).

use serde::{Deserialize, Serialize};

/// Whether the sandbox container can access the public internet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NetworkAccess {
    #[serde(rename = "none")]
    None,
    #[serde(rename = "public")]
    PublicInternet,
}

/// A file read result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileContents {
    pub contents: String,
    pub truncated: bool,
    pub binary: bool,
    pub line_count: usize,
}

/// Result of executing a command.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandResult {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
    pub truncated: bool,
}

/// Request sent from housebot to sandboxd.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxRequest {
    pub id: String,
    pub method: String,
    pub params: serde_json::Value,
}

/// Response sent from sandboxd to housebot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxResponse {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl SandboxRequest {
    pub fn new(method: &str, params: serde_json::Value) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            method: method.to_string(),
            params,
        }
    }
}

impl SandboxResponse {
    pub fn ok(id: String, result: serde_json::Value) -> Self {
        Self {
            id,
            result: Some(result),
            error: None,
        }
    }

    pub fn err(id: String, error: String) -> Self {
        Self {
            id,
            result: None,
            error: Some(error),
        }
    }

    pub fn into_result(self) -> Result<serde_json::Value, String> {
        match self.error {
            Some(e) => Err(e),
            None => self.result.ok_or_else(|| "empty response".to_string()),
        }
    }
}

// ══════════════════════════════════════════════════════════════════════════════
// Request parameter types
// ══════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StartParams {
    /// Identifies the owning session. Two `start` calls with the same key share
    /// one container, so work survives across turns.
    pub session_key: String,
    pub network: NetworkAccess,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReadFileParams {
    pub sandbox_id: String,
    pub path: String,
    pub start_line: Option<u32>,
    pub end_line: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunParams {
    pub sandbox_id: String,
    pub command: String,
    pub working_dir: Option<String>,
    pub timeout_secs: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WriteFileParams {
    pub sandbox_id: String,
    pub path: String,
    pub content: String,
    /// Mark the file executable after writing. `/workspace` allows execution,
    /// so skill scripts need this.
    #[serde(default)]
    pub executable: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WriteFileResult {
    pub path: String,
    pub bytes_written: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CloseParams {
    pub sandbox_id: String,
}
