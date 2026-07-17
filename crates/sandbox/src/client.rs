use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::time::timeout;

use crate::limits;
use crate::protocol::*;
use crate::validation;

/// Client that talks to a running `sandboxd` process over a Unix socket.
///
/// Housebot holds one `SandboxClient` and uses it to create and interact
/// with disposable sandbox containers.  The client never sees the Docker
/// socket — it sends typed requests and receives typed responses.
#[derive(Debug, Clone)]
pub struct SandboxClient {
    socket_path: String,
}

impl SandboxClient {
    /// Connect to a `sandboxd` instance at the given Unix socket path.
    ///
    /// The default path is `/run/housebot-sandbox/sandbox.sock`.
    pub fn new(socket_path: impl Into<String>) -> Self {
        Self {
            socket_path: socket_path.into(),
        }
    }

    pub fn from_env() -> Self {
        let path = std::env::var("SANDBOX_SOCKET_PATH")
            .unwrap_or_else(|_| "/run/housebot-sandbox/sandbox.sock".to_string());
        Self::new(path)
    }

    async fn send_request(&self, request: SandboxRequest) -> Result<SandboxResponse, String> {
        let timeout_dur = Duration::from_secs(limits::SOCKET_TIMEOUT_SECS);

        let stream = timeout(timeout_dur, UnixStream::connect(&self.socket_path))
            .await
            .map_err(|_| format!("timed out connecting to sandboxd after {}s", limits::SOCKET_TIMEOUT_SECS))?
            .map_err(|e| format!("failed to connect to sandboxd: {e}"))?;

        let (reader, mut writer) = stream.into_split();

        let line = serde_json::to_string(&request)
            .map_err(|e| format!("failed to serialise request: {e}"))?;
        let mut line_bytes = line.into_bytes();
        line_bytes.push(b'\n');

        timeout(timeout_dur, writer.write_all(&line_bytes))
            .await
            .map_err(|_| format!("timed out writing request after {}s", limits::SOCKET_TIMEOUT_SECS))?
            .map_err(|e| format!("failed to write request: {e}"))?;
        writer.shutdown().await.ok();

        let mut buf_reader = BufReader::new(reader);
        let mut response_line = String::with_capacity(4096);
        timeout(timeout_dur, buf_reader.read_line(&mut response_line))
            .await
            .map_err(|_| format!("timed out reading response after {}s", limits::SOCKET_TIMEOUT_SECS))?
            .map_err(|e| format!("failed to read response: {e}"))?;

        if response_line.is_empty() {
            return Err("empty response from sandboxd".to_string());
        }

        if response_line.len() > limits::MAX_REQUEST_FRAME_BYTES {
            return Err(format!(
                "response frame too large ({} bytes, max {})",
                response_line.len(),
                limits::MAX_REQUEST_FRAME_BYTES
            ));
        }

        let response: SandboxResponse = serde_json::from_str(response_line.trim())
            .map_err(|e| format!("failed to parse response: {e}"))?;

        Ok(response)
    }

    /// Request that sandboxd create a new sandbox container.
    pub async fn start(&self, network: NetworkAccess) -> Result<Sandbox, String> {
        let req = SandboxRequest::new(
            "start",
            serde_json::to_value(StartParams { network })
                .map_err(|e| format!("serialisation error: {e}"))?,
        );
        let resp = self.send_request(req).await?;
        let result = resp.into_result()?;
        let id = result
            .get("sandbox_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "sandboxd did not return a sandbox ID".to_string())?
            .to_string();
        Ok(Sandbox {
            id,
            client: self.clone(),
        })
    }
}

/// A handle to a running sandbox container.
///
/// Dropping this without calling `close()` will leak the container, but
/// sandboxd also cleans stale containers at startup.  The preferred
/// path is to call `close()` explicitly.
#[derive(Debug, Clone)]
pub struct Sandbox {
    id: String,
    client: SandboxClient,
}

impl Sandbox {
    fn request(&self, method: &str, params: serde_json::Value) -> Result<SandboxRequest, String> {
        Ok(SandboxRequest::new(method, params))
    }

    async fn send(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        let mut req = self.request(method, params)?;
        req.params["sandbox_id"] = serde_json::Value::String(self.id.clone());
        let resp = self.client.send_request(req).await?;
        resp.into_result()
    }

    /// Clone a public HTTPS repository into the sandbox.
    pub async fn clone_repository(
        &self,
        url: &str,
        branch: Option<&str>,
    ) -> Result<CommandResult, String> {
        validation::validate_repository_url(url)?;
        if let Some(b) = branch {
            validation::validate_branch(b)?;
        }

        let params = serde_json::to_value(CloneRepositoryParams {
            sandbox_id: self.id.clone(),
            url: url.to_string(),
            branch: branch.map(|s| s.to_string()),
        })
        .map_err(|e| format!("serialisation error: {e}"))?;

        let result = self.send("clone_repository", params).await?;
        serde_json::from_value(result).map_err(|e| format!("failed to parse clone result: {e}"))
    }

    /// List files in a workspace directory.
    pub async fn list_files(
        &self,
        path: &str,
        max_depth: Option<u32>,
    ) -> Result<Vec<FileEntry>, String> {
        validation::validate_workspace_path(path)?;

        let params = serde_json::to_value(ListFilesParams {
            sandbox_id: self.id.clone(),
            path: path.to_string(),
            max_depth,
        })
        .map_err(|e| format!("serialisation error: {e}"))?;

        let result = self.send("list_files", params).await?;
        let entries: Vec<FileEntry> = serde_json::from_value(result)
            .map_err(|e| format!("failed to parse file list: {e}"))?;
        Ok(entries)
    }

    /// Search source code in the workspace.
    pub async fn search_code(
        &self,
        query: &str,
        path: Option<&str>,
        glob: Option<&str>,
    ) -> Result<SearchResult, String> {
        validation::validate_query(query)?;
        if let Some(g) = glob {
            validation::validate_glob(g)?;
        }
        if let Some(p) = path {
            validation::validate_workspace_path(p)?;
        }

        let params = serde_json::to_value(SearchCodeParams {
            sandbox_id: self.id.clone(),
            query: query.to_string(),
            path: path.map(|s| s.to_string()),
            glob: glob.map(|s| s.to_string()),
        })
        .map_err(|e| format!("serialisation error: {e}"))?;

        let result = self.send("search_code", params).await?;
        serde_json::from_value(result).map_err(|e| format!("failed to parse search result: {e}"))
    }

    /// Read a bounded section of a text file.
    pub async fn read_file(
        &self,
        path: &str,
        start_line: Option<u32>,
        end_line: Option<u32>,
    ) -> Result<FileContents, String> {
        validation::validate_workspace_path(path)?;

        let params = serde_json::to_value(ReadFileParams {
            sandbox_id: self.id.clone(),
            path: path.to_string(),
            start_line,
            end_line,
        })
        .map_err(|e| format!("serialisation error: {e}"))?;

        let result = self.send("read_file", params).await?;
        serde_json::from_value(result).map_err(|e| format!("failed to parse file contents: {e}"))
    }

    /// Run a command inside the sandbox.
    pub async fn run(
        &self,
        command: &str,
        working_dir: Option<&str>,
        timeout_secs: Option<u64>,
    ) -> Result<CommandResult, String> {
        validation::validate_command(command)?;
        if let Some(d) = working_dir {
            validation::validate_workspace_path(d)?;
        }

        let timeout = timeout_secs
            .unwrap_or(limits::DEFAULT_COMMAND_TIMEOUT_SECS)
            .min(limits::ABSOLUTE_MAX_TIMEOUT_SECS);

        let params = serde_json::to_value(RunParams {
            sandbox_id: self.id.clone(),
            command: command.to_string(),
            working_dir: working_dir.map(|s| s.to_string()),
            timeout_secs: Some(timeout),
        })
        .map_err(|e| format!("serialisation error: {e}"))?;

        let result = self.send("run", params).await?;
        serde_json::from_value(result).map_err(|e| format!("failed to parse command result: {e}"))
    }

    /// Destroy the sandbox container.
    pub async fn close(self) -> Result<(), String> {
        let params = serde_json::to_value(CloseParams {
            sandbox_id: self.id.clone(),
        })
        .map_err(|e| format!("serialisation error: {e}"))?;

        let mut req = SandboxRequest::new("close", params);
        req.params["sandbox_id"] = serde_json::Value::String(self.id.clone());
        let resp = self.client.send_request(req).await?;
        resp.into_result()?;
        Ok(())
    }

    /// The sandbox container ID.
    pub fn id(&self) -> &str {
        &self.id
    }
}
