use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::time::timeout;

use crate::limits;
use crate::protocol::*;
use crate::validation;

/// How the client reaches the sandbox tier.
///
/// A Unix socket reaches the one `sandboxd` on the same host; an HTTP base URL
/// reaches a load-balanced `sandbox-api` Service, where consecutive requests
/// may land on different replicas.
#[derive(Debug, Clone)]
enum Transport {
    Unix(String),
    Http {
        base_url: String,
        token: String,
        client: reqwest::Client,
    },
}

/// Client that talks to the sandbox tier.
///
/// Housebot holds one `SandboxClient` and uses it to create and interact
/// with disposable sandboxes.  The client never sees the Docker socket or the
/// Kubernetes API — it sends typed requests and receives typed responses.
#[derive(Debug, Clone)]
pub struct SandboxClient {
    transport: Transport,
}

impl SandboxClient {
    /// Connect to a `sandboxd` instance at the given Unix socket path.
    ///
    /// The default path is `/run/housebot-sandbox/sandbox.sock`.
    pub fn new(socket_path: impl Into<String>) -> Self {
        Self {
            transport: Transport::Unix(socket_path.into()),
        }
    }

    /// Connect to a `sandbox-api` Service.
    pub fn http(base_url: impl Into<String>, token: impl Into<String>) -> Self {
        Self {
            transport: Transport::Http {
                base_url: base_url.into().trim_end_matches('/').to_string(),
                token: token.into(),
                client: reqwest::Client::new(),
            },
        }
    }

    /// Prefer the HTTP API when one is configured, so a Kubernetes deployment
    /// needs no code change to route through the scalable tier.
    pub fn from_env() -> Self {
        match std::env::var("SANDBOX_API_URL") {
            Ok(url) if !url.is_empty() => {
                Self::http(url, std::env::var("SANDBOX_API_TOKEN").unwrap_or_default())
            }
            _ => Self::new(
                std::env::var("SANDBOX_SOCKET_PATH")
                    .unwrap_or_else(|_| "/run/housebot-sandbox/sandbox.sock".to_string()),
            ),
        }
    }

    async fn send_request(&self, request: SandboxRequest) -> Result<SandboxResponse, String> {
        match &self.transport {
            Transport::Unix(socket_path) => self.send_over_socket(socket_path, request).await,
            Transport::Http {
                base_url,
                token,
                client,
            } => send_over_http(client, base_url, token, request).await,
        }
    }

    async fn send_over_socket(
        &self,
        socket_path: &str,
        request: SandboxRequest,
    ) -> Result<SandboxResponse, String> {
        let timeout_dur = Duration::from_secs(limits::SOCKET_TIMEOUT_SECS);

        let stream = timeout(timeout_dur, UnixStream::connect(socket_path))
            .await
            .map_err(|_| {
                format!(
                    "timed out connecting to sandboxd after {}s",
                    limits::SOCKET_TIMEOUT_SECS
                )
            })?
            .map_err(|e| format!("failed to connect to sandboxd: {e}"))?;

        let (reader, mut writer) = stream.into_split();

        let line = serde_json::to_string(&request)
            .map_err(|e| format!("failed to serialise request: {e}"))?;
        let mut line_bytes = line.into_bytes();
        line_bytes.push(b'\n');

        timeout(timeout_dur, writer.write_all(&line_bytes))
            .await
            .map_err(|_| {
                format!(
                    "timed out writing request after {}s",
                    limits::SOCKET_TIMEOUT_SECS
                )
            })?
            .map_err(|e| format!("failed to write request: {e}"))?;
        writer.shutdown().await.ok();

        let mut buf_reader = BufReader::new(reader);
        let mut response_line = String::with_capacity(4096);
        timeout(timeout_dur, buf_reader.read_line(&mut response_line))
            .await
            .map_err(|_| {
                format!(
                    "timed out reading response after {}s",
                    limits::SOCKET_TIMEOUT_SECS
                )
            })?
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

    /// Get the session's sandbox container, creating it if the session has none.
    ///
    /// The container outlives the request that created it and is destroyed by
    /// sandboxd once the session has been idle past its timeout.
    pub async fn start(
        &self,
        session_key: &str,
        network: NetworkAccess,
    ) -> Result<Sandbox, String> {
        validation::validate_session_key(session_key)?;
        let req = SandboxRequest::new(
            "start",
            serde_json::to_value(StartParams {
                session_key: session_key.to_string(),
                network,
            })
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

/// Map a protocol method onto its REST route.
///
/// The sandbox ID lives in the path, so a request can only ever reach the
/// sandbox its URL names.
fn route(method: &str, sandbox_id: &str) -> Result<(reqwest::Method, String), String> {
    let sandboxes = "/v1/sandboxes".to_string();
    let one = format!("{sandboxes}/{sandbox_id}");
    Ok(match method {
        "start" => (reqwest::Method::POST, sandboxes),
        "close" => (reqwest::Method::DELETE, one),
        "clone_repository" => (reqwest::Method::POST, format!("{one}/clone")),
        "run" => (reqwest::Method::POST, format!("{one}/exec")),
        "search_code" => (reqwest::Method::POST, format!("{one}/search")),
        "list_files" => (reqwest::Method::POST, format!("{one}/list")),
        "read_file" => (reqwest::Method::POST, format!("{one}/read")),
        "write_file" => (reqwest::Method::PUT, format!("{one}/file")),
        other => return Err(format!("unsupported method: {other}")),
    })
}

async fn send_over_http(
    client: &reqwest::Client,
    base_url: &str,
    token: &str,
    request: SandboxRequest,
) -> Result<SandboxResponse, String> {
    let sandbox_id = request
        .params
        .get("sandbox_id")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    let (method, path) = route(&request.method, sandbox_id)?;

    let response = client
        .request(method, format!("{base_url}{path}"))
        .bearer_auth(token)
        .timeout(Duration::from_secs(limits::SOCKET_TIMEOUT_SECS))
        .json(&request.params)
        .send()
        .await
        .map_err(|e| format!("failed to reach the sandbox API: {e}"))?;

    let status = response.status();
    let body: serde_json::Value = response
        .json()
        .await
        .map_err(|e| format!("failed to parse sandbox API response: {e}"))?;

    if status.is_success() {
        Ok(SandboxResponse::ok(request.id, body))
    } else {
        Ok(SandboxResponse::err(
            request.id,
            body.get("error")
                .and_then(|v| v.as_str())
                .unwrap_or("sandbox API request failed")
                .to_string(),
        ))
    }
}

/// A handle to a running sandbox container.
///
/// The handle is cheap and does not own the container: sandboxd keeps the
/// container alive for the session and destroys it once it falls idle.  Call
/// `close()` to discard the workspace before then.
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

    /// Write a file into the workspace.
    ///
    /// The content travels as request data and is fed to the container on
    /// stdin, so it is never interpreted as part of a command. Set
    /// `executable` for scripts — `/workspace` permits execution.
    pub async fn write_file(
        &self,
        path: &str,
        content: &str,
        executable: bool,
    ) -> Result<WriteFileResult, String> {
        validation::validate_workspace_path(path)?;
        if content.len() > limits::MAX_WRITE_FILE_BYTES {
            return Err(format!(
                "content exceeds {} bytes",
                limits::MAX_WRITE_FILE_BYTES
            ));
        }

        let params = serde_json::to_value(WriteFileParams {
            sandbox_id: self.id.clone(),
            path: path.to_string(),
            content: content.to_string(),
            executable,
        })
        .map_err(|e| format!("serialisation error: {e}"))?;

        let result = self.send("write_file", params).await?;
        serde_json::from_value(result).map_err(|e| format!("failed to parse write result: {e}"))
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
