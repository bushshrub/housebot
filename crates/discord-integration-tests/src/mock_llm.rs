use anyhow::Context;
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

pub async fn run() -> anyhow::Result<()> {
    let address =
        std::env::var("MOCK_LLM_ADDRESS").unwrap_or_else(|_| "127.0.0.1:18080".to_string());
    let listener = TcpListener::bind(&address).await?;
    println!("mock LLM listening on {address}");
    loop {
        let (stream, _) = listener.accept().await?;
        tokio::spawn(async move {
            if let Err(error) = handle(stream).await {
                eprintln!("mock LLM request failed: {error:#}");
            }
        });
    }
}

async fn handle(mut stream: TcpStream) -> anyhow::Result<()> {
    let request = read_request(&mut stream).await?;
    let first_line = request.lines().next().unwrap_or_default();
    if first_line.starts_with("GET /props ") {
        return respond_json(
            &mut stream,
            &json!({"default_generation_settings":{"n_ctx":32768}}),
        )
        .await;
    }
    if !first_line.starts_with("POST /v1/chat/completions ") {
        return respond_status(&mut stream, "404 Not Found", "not found").await;
    }

    let body = request
        .split_once("\r\n\r\n")
        .map(|(_, body)| body)
        .unwrap_or_default();
    let value: Value = serde_json::from_str(body).context("invalid request JSON")?;
    let serialized = value
        .get("messages")
        .cloned()
        .unwrap_or(Value::Null)
        .to_string();
    let response = deterministic_response(&serialized);

    if value.get("stream").and_then(Value::as_bool) == Some(true) {
        let chunk = json!({
            "choices": [{
                "delta": {"content": response},
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": 10,
                "completion_tokens": 5,
                "prompt_tokens_details": {"cached_tokens": 0}
            }
        });
        let body = format!("data: {chunk}\n\ndata: [DONE]\n\n");
        let headers = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
            body.len()
        );
        stream.write_all(headers.as_bytes()).await?;
        stream.write_all(body.as_bytes()).await?;
        return Ok(());
    }

    respond_json(
        &mut stream,
        &json!({
            "choices": [{"message": {"content": response}}],
            "usage": {
                "prompt_tokens": 10,
                "completion_tokens": 5,
                "prompt_tokens_details": {"cached_tokens": 0}
            }
        }),
    )
    .await
}

fn deterministic_response(messages: &str) -> String {
    let latest = [
        ("E2E_ECHO:", "echo"),
        ("E2E_REPLY:", "echo"),
        ("E2E_LONG:", "long"),
        ("E2E_SECRET:", "secret"),
    ]
    .into_iter()
    .filter_map(|(prefix, kind)| {
        let start = messages.rfind(prefix)?;
        Some((start, kind, nonce_at(messages, start, prefix)?))
    })
    .max_by_key(|(start, _, _)| *start);
    match latest {
        Some((_, "long", nonce)) => format!(
            "E2E_LONG_BEGIN:{nonce}\n{}\nE2E_LONG_END:{nonce}",
            "integration-output ".repeat(240)
        ),
        Some((_, "secret", nonce)) => {
            let secret = std::env::var("E2E_FAKE_SECRET")
                .unwrap_or_else(|_| "housebot-e2e-secret-redacted".to_string());
            format!("E2E_SECRET_BEGIN:{nonce} {secret} E2E_SECRET_END:{nonce}")
        }
        Some((_, _, nonce)) => format!("E2E_OK:{nonce}"),
        None => "E2E_FALLBACK".to_string(),
    }
}

fn nonce_at<'a>(messages: &'a str, start: usize, prefix: &str) -> Option<&'a str> {
    messages[start + prefix.len()..]
        .split(|character: char| !(character.is_ascii_alphanumeric() || character == '-'))
        .next()
}

async fn read_request(stream: &mut TcpStream) -> anyhow::Result<String> {
    let mut bytes = Vec::new();
    let header_end = loop {
        let mut buffer = [0u8; 4096];
        let read = stream.read(&mut buffer).await?;
        anyhow::ensure!(read > 0, "connection closed before request headers");
        bytes.extend_from_slice(&buffer[..read]);
        if let Some(index) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            break index + 4;
        }
        anyhow::ensure!(bytes.len() < 1024 * 1024, "request headers too large");
    };
    let headers = String::from_utf8_lossy(&bytes[..header_end]);
    let content_length = headers
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().ok())
                .flatten()
        })
        .unwrap_or(0);
    while bytes.len() < header_end + content_length {
        let mut buffer = [0u8; 4096];
        let read = stream.read(&mut buffer).await?;
        anyhow::ensure!(read > 0, "connection closed before request body");
        bytes.extend_from_slice(&buffer[..read]);
    }
    Ok(String::from_utf8(bytes)?)
}

async fn respond_json(stream: &mut TcpStream, value: &Value) -> anyhow::Result<()> {
    let body = value.to_string();
    let headers = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(headers.as_bytes()).await?;
    stream.write_all(body.as_bytes()).await?;
    Ok(())
}

async fn respond_status(stream: &mut TcpStream, status: &str, body: &str) -> anyhow::Result<()> {
    let headers = format!(
        "HTTP/1.1 {status}\r\ncontent-type: text/plain\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(headers.as_bytes()).await?;
    stream.write_all(body.as_bytes()).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn echo_response_preserves_nonce() {
        assert_eq!(
            deterministic_response(r#"[{"role":"user","content":"E2E_ECHO:abc-123"}]"#),
            "E2E_OK:abc-123"
        );
    }

    #[test]
    fn long_response_crosses_discord_message_limit() {
        let response = deterministic_response("E2E_LONG:abc-123");
        assert!(response.len() > 4_000);
        assert!(response.starts_with("E2E_LONG_BEGIN:abc-123"));
        assert!(response.ends_with("E2E_LONG_END:abc-123"));
    }

    #[test]
    fn latest_conversation_marker_wins() {
        let messages = "E2E_ECHO:old-nonce context E2E_REPLY:new-nonce";
        assert_eq!(deterministic_response(messages), "E2E_OK:new-nonce");

        let messages = "E2E_LONG:old-nonce context E2E_SECRET:new-nonce";
        assert!(deterministic_response(messages).ends_with("E2E_SECRET_END:new-nonce"));
    }
}
