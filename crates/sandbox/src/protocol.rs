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

/// A single entry in a file listing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileEntry {
    pub name: String,
    #[serde(rename = "type")]
    pub entry_type: String, // "file" or "dir"
    pub size: Option<i64>,
}

/// A single search match.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchMatch {
    pub path: String,
    pub line_number: u64,
    pub line: String,
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

/// A search result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub matches: Vec<SearchMatch>,
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
    pub network: NetworkAccess,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CloneRepositoryParams {
    pub sandbox_id: String,
    pub url: String,
    pub branch: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListFilesParams {
    pub sandbox_id: String,
    pub path: String,
    pub max_depth: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchCodeParams {
    pub sandbox_id: String,
    pub query: String,
    pub path: Option<String>,
    pub glob: Option<String>,
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
pub struct CloseParams {
    pub sandbox_id: String,
}
