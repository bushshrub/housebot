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

        let response_wait = response_timeout(&request);
        let mut buf_reader = BufReader::new(reader);
        let mut response_line = String::with_capacity(4096);
        timeout(response_wait, buf_reader.read_line(&mut response_line))
            .await
            .map_err(|_| {
                format!(
                    "timed out reading response after {}s",
                    response_wait.as_secs()
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

    /// Postpone the reaping of the session's sandbox, if it has one.
    pub async fn touch(&self, session_key: &str) -> Result<(), String> {
        validation::validate_session_key(session_key)?;
        let req = SandboxRequest::new(
            "touch",
            serde_json::to_value(TouchParams {
                session_key: session_key.to_string(),
            })
            .map_err(|e| format!("serialisation error: {e}"))?,
        );
        self.send_request(req).await?.into_result()?;
        Ok(())
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

    /// Replace text in an existing workspace file.
    pub async fn edit_file(
        &self,
        path: &str,
        old_string: &str,
        new_string: &str,
        replace_all: bool,
    ) -> Result<EditFileResult, String> {
        validation::validate_workspace_path(path)?;

        let params = serde_json::to_value(EditFileParams {
            sandbox_id: self.id.clone(),
            path: path.to_string(),
            old_string: old_string.to_string(),
            new_string: new_string.to_string(),
            replace_all,
        })
        .map_err(|e| format!("serialisation error: {e}"))?;

        let result = self.send("edit_file", params).await?;
        serde_json::from_value(result).map_err(|e| format!("failed to parse edit result: {e}"))
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

/// sandboxd answers a command only once it has finished, so the reply may
/// legitimately take as long as the command's own timeout.
fn response_timeout(request: &SandboxRequest) -> Duration {
    let command_secs = request
        .params
        .get("timeout_secs")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    Duration::from_secs(limits::SOCKET_TIMEOUT_SECS + command_secs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn touch_sends_the_session_key_and_accepts_the_reply() {
        let socket = std::env::temp_dir().join(format!("touch-{}.sock", uuid::Uuid::new_v4()));
        let listener = tokio::net::UnixListener::bind(&socket).expect("bind fake sandboxd");
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("accept");
            let (reader, mut writer) = stream.into_split();
            let mut line = String::new();
            BufReader::new(reader)
                .read_line(&mut line)
                .await
                .expect("read request");
            let request: SandboxRequest = serde_json::from_str(&line).expect("request is JSON");
            let reply = SandboxResponse::ok(request.id.clone(), serde_json::json!({"touched": 1}));
            let mut bytes = serde_json::to_vec(&reply).expect("serialise reply");
            bytes.push(b'\n');
            writer.write_all(&bytes).await.expect("write reply");
            request
        });

        let client = SandboxClient::new(socket.to_string_lossy().to_string());
        client.touch("user-1").await.expect("touch succeeds");

        let request = server.await.expect("server task");
        assert_eq!(request.method, "touch");
        assert_eq!(request.params["session_key"], "user-1");
        let _ = std::fs::remove_file(&socket);
    }

    #[test]
    fn a_command_reply_is_awaited_for_the_command_timeout_plus_the_socket_timeout() {
        let run = SandboxRequest::new("run", serde_json::json!({"timeout_secs": 300}));
        let read = SandboxRequest::new("read_file", serde_json::json!({}));
        assert_eq!(
            response_timeout(&run),
            Duration::from_secs(300 + limits::SOCKET_TIMEOUT_SECS)
        );
        assert_eq!(
            response_timeout(&read),
            Duration::from_secs(limits::SOCKET_TIMEOUT_SECS)
        );
    }
}
