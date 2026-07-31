//! Minimal HTTP/1.1 server plumbing: enough to serve a game and a small JSON
//! API, and nothing else.  Every connection is answered once and closed.

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub const MAX_HEAD_BYTES: usize = 8 * 1024;
pub const MAX_BODY_BYTES: usize = 4 * 1024;

const HEAD_END: &[u8] = b"\r\n\r\n";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub method: String,
    pub path: String,
    pub body: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    pub status: u16,
    pub content_type: &'static str,
    pub body: Vec<u8>,
}

impl Response {
    pub fn new(status: u16, content_type: &'static str, body: impl Into<Vec<u8>>) -> Self {
        Self {
            status,
            content_type,
            body: body.into(),
        }
    }

    pub fn html(body: &str) -> Self {
        Self::new(200, "text/html; charset=utf-8", body)
    }

    pub fn javascript(body: &str) -> Self {
        Self::new(200, "text/javascript; charset=utf-8", body)
    }

    pub fn json(status: u16, body: impl Into<Vec<u8>>) -> Self {
        Self::new(status, "application/json", body)
    }

    pub fn no_content() -> Self {
        Self::new(204, "", Vec::new())
    }

    pub fn error(status: u16, message: &str) -> Self {
        Self::json(status, serde_json::json!({ "error": message }).to_string())
    }

    pub fn to_bytes(&self, keep_alive: bool) -> Vec<u8> {
        let mut head = format!("HTTP/1.1 {} {}\r\n", self.status, status_text(self.status));
        if self.status != 204 {
            head.push_str(&format!(
                "Content-Type: {}\r\nContent-Length: {}\r\n",
                self.content_type,
                self.body.len(),
            ));
        }
        head.push_str("Cache-Control: no-store\r\n");
        head.push_str(if keep_alive {
            "Connection: keep-alive\r\n\r\n"
        } else {
            "Connection: close\r\n\r\n"
        });
        let mut out = head.into_bytes();
        out.extend_from_slice(&self.body);
        out
    }
}

fn status_text(status: u16) -> &'static str {
    match status {
        200 => "OK",
        204 => "No Content",
        400 => "Bad Request",
        404 => "Not Found",
        405 => "Method Not Allowed",
        413 => "Payload Too Large",
        _ => "Internal Server Error",
    }
}

/// Reads one request.  Returns `None` for anything malformed or oversized —
/// the caller answers those with a 400 and hangs up.
pub async fn read_request<R: AsyncRead + Unpin>(stream: &mut R) -> Option<Request> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 1024];

    let head_end = loop {
        if let Some(at) = find_head_end(&buf) {
            break at;
        }
        if buf.len() > MAX_HEAD_BYTES {
            return None;
        }
        let read = stream.read(&mut chunk).await.ok()?;
        if read == 0 {
            return None;
        }
        buf.extend_from_slice(&chunk[..read]);
    };

    let head = std::str::from_utf8(&buf[..head_end]).ok()?;
    let mut lines = head.split("\r\n");
    let mut request_line = lines.next()?.split(' ');
    let method = request_line.next()?.to_string();
    let target = request_line.next()?;
    if method.is_empty() || !target.starts_with('/') {
        return None;
    }
    let path = target.split(['?', '#']).next()?.to_string();

    let content_length = lines
        .filter_map(|line| line.split_once(':'))
        .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
        .map(|(_, value)| value.trim().parse::<usize>())
        .transpose()
        .ok()?
        .unwrap_or(0);
    if content_length > MAX_BODY_BYTES {
        return None;
    }

    let mut body = buf.split_off(head_end + HEAD_END.len());
    body.truncate(content_length);
    while body.len() < content_length {
        let read = stream.read(&mut chunk).await.ok()?;
        if read == 0 {
            return None;
        }
        let wanted = content_length - body.len();
        body.extend_from_slice(&chunk[..read.min(wanted)]);
    }

    Some(Request { method, path, body })
}

pub async fn write_response<W: AsyncWrite + Unpin>(
    stream: &mut W,
    response: &Response,
    keep_alive: bool,
) -> std::io::Result<()> {
    stream.write_all(&response.to_bytes(keep_alive)).await?;
    stream.flush().await
}

fn find_head_end(buf: &[u8]) -> Option<usize> {
    buf.windows(HEAD_END.len())
        .position(|window| window == HEAD_END)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    async fn parse(raw: &str) -> Option<Request> {
        read_request(&mut Cursor::new(raw.as_bytes().to_vec())).await
    }

    #[tokio::test]
    async fn parses_a_get_with_a_query_string() {
        let request = parse("GET /api/scores?limit=5 HTTP/1.1\r\nHost: x\r\n\r\n")
            .await
            .unwrap();
        assert_eq!(request.method, "GET");
        assert_eq!(request.path, "/api/scores");
        assert!(request.body.is_empty());
    }

    #[tokio::test]
    async fn parses_a_post_body_of_the_declared_length() {
        let request = parse("POST /api/scores HTTP/1.1\r\nContent-Length: 7\r\n\r\n{\"a\":1}extra")
            .await
            .unwrap();
        assert_eq!(request.method, "POST");
        assert_eq!(request.body, b"{\"a\":1}");
    }

    #[tokio::test]
    async fn rejects_a_truncated_body() {
        assert!(
            parse("POST /api/scores HTTP/1.1\r\nContent-Length: 32\r\n\r\n{}")
                .await
                .is_none()
        );
    }

    #[tokio::test]
    async fn rejects_an_oversized_body() {
        let raw = format!(
            "POST /api/scores HTTP/1.1\r\nContent-Length: {}\r\n\r\n",
            MAX_BODY_BYTES + 1
        );
        assert!(parse(&raw).await.is_none());
    }

    #[tokio::test]
    async fn rejects_a_non_absolute_target() {
        assert!(parse("GET http://elsewhere/ HTTP/1.1\r\n\r\n")
            .await
            .is_none());
    }

    #[test]
    fn serializes_status_line_and_length() {
        let bytes = Response::json(200, "[]").to_bytes(false);
        let text = String::from_utf8(bytes).unwrap();
        assert!(text.starts_with("HTTP/1.1 200 OK\r\n"));
        assert!(text.contains("Content-Length: 2\r\n"));
        assert!(text.contains("Connection: close\r\n"));
        assert!(text.ends_with("\r\n\r\n[]"));
    }

    #[test]
    fn a_reused_connection_is_advertised_as_such() {
        let text = String::from_utf8(Response::json(200, "[]").to_bytes(true)).unwrap();
        assert!(text.contains("Connection: keep-alive\r\n"));
    }

    #[test]
    fn a_no_content_reply_carries_no_body_headers() {
        let text = String::from_utf8(Response::no_content().to_bytes(false)).unwrap();
        assert!(text.starts_with("HTTP/1.1 204 No Content\r\n"));
        assert!(!text.contains("Content-Length"));
        assert!(!text.contains("Content-Type"));
    }
}
