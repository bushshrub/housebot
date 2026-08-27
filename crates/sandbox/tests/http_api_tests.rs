//! Integration tests for the sandbox HTTP API.
//!
//! Every case here stops before a container runtime is reached — on
//! authentication, on validation, or on an unknown sandbox — so the suite runs
//! without Docker or a cluster.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

use housebot_sandbox::{http, server};

const TOKEN: &str = "0123456789abcdef0123456789abcdef";

fn router() -> axum::Router {
    http::router(server::start(), TOKEN.to_string())
}

fn request(
    method: &str,
    path: &str,
    token: Option<&str>,
    body: serde_json::Value,
) -> Request<Body> {
    let mut builder = Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json");
    if let Some(token) = token {
        builder = builder.header("authorization", format!("Bearer {token}"));
    }
    builder
        .body(Body::from(body.to_string()))
        .expect("a request")
}

async fn send(request: Request<Body>) -> (StatusCode, serde_json::Value) {
    let response = router().oneshot(request).await.expect("a response");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 64 * 1024)
        .await
        .expect("a body");
    let body = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, body)
}

#[tokio::test]
async fn a_request_without_a_token_is_rejected() {
    let (status, _) = send(request(
        "POST",
        "/v1/sandboxes",
        None,
        serde_json::json!({"session_key": "user-1", "network": "none"}),
    ))
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn a_request_with_the_wrong_token_is_rejected() {
    let (status, body) = send(request(
        "POST",
        "/v1/sandboxes/abc/exec",
        Some("0123456789abcdef0123456789abcdee"),
        serde_json::json!({"command": "ls"}),
    ))
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(body["error"].is_string());
}

#[tokio::test]
async fn every_route_demands_the_token() {
    let routes = [
        ("POST", "/v1/sandboxes"),
        ("DELETE", "/v1/sandboxes/abc"),
        ("POST", "/v1/sandboxes/abc/clone"),
        ("POST", "/v1/sandboxes/abc/exec"),
        ("POST", "/v1/sandboxes/abc/search"),
        ("POST", "/v1/sandboxes/abc/list"),
        ("POST", "/v1/sandboxes/abc/read"),
        ("PUT", "/v1/sandboxes/abc/file"),
    ];
    for (method, path) in routes {
        let (status, _) = send(request(method, path, None, serde_json::json!({}))).await;
        assert_eq!(
            status,
            StatusCode::UNAUTHORIZED,
            "{method} {path} must not be reachable unauthenticated"
        );
    }
}

#[tokio::test]
async fn the_probes_need_no_token() {
    for path in ["/healthz", "/readyz"] {
        let response = router()
            .oneshot(
                Request::builder()
                    .uri(path)
                    .body(Body::empty())
                    .expect("a request"),
            )
            .await
            .expect("a response");
        assert_eq!(
            response.status(),
            StatusCode::OK,
            "{path} must answer the kubelet"
        );
    }
}

#[tokio::test]
async fn an_unknown_sandbox_is_reported_as_not_found() {
    let (status, body) = send(request(
        "POST",
        "/v1/sandboxes/does-not-exist/exec",
        Some(TOKEN),
        serde_json::json!({"command": "ls"}),
    ))
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(body["error"]
        .as_str()
        .expect("an error")
        .contains("unknown sandbox"));
}

#[tokio::test]
async fn closing_an_unknown_sandbox_is_reported_as_not_found() {
    let (status, _) = send(request(
        "DELETE",
        "/v1/sandboxes/does-not-exist",
        Some(TOKEN),
        serde_json::json!({}),
    ))
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_path_escaping_the_workspace_is_a_client_error() {
    let (status, body) = send(request(
        "POST",
        "/v1/sandboxes/abc/read",
        Some(TOKEN),
        serde_json::json!({"path": "../../etc/passwd"}),
    ))
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "validation must reject the path before any sandbox is consulted"
    );
    assert!(body["error"]
        .as_str()
        .expect("an error")
        .contains("invalid path"));
}

#[tokio::test]
async fn an_invalid_session_key_never_creates_a_sandbox() {
    let (status, body) = send(request(
        "POST",
        "/v1/sandboxes",
        Some(TOKEN),
        serde_json::json!({"session_key": "user 1; rm -rf /", "network": "none"}),
    ))
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["error"]
        .as_str()
        .expect("an error")
        .contains("invalid session key"));
}

#[tokio::test]
async fn an_oversized_write_is_refused_before_it_reaches_a_sandbox() {
    let (status, _) = send(request(
        "PUT",
        "/v1/sandboxes/abc/file",
        Some(TOKEN),
        serde_json::json!({
            "path": "/workspace/big",
            "content": "x".repeat(512 * 1024),
        }),
    ))
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn the_path_id_wins_over_a_body_that_names_another_sandbox() {
    let (status, body) = send(request(
        "POST",
        "/v1/sandboxes/mine/exec",
        Some(TOKEN),
        serde_json::json!({"sandbox_id": "someone-elses", "command": "ls"}),
    ))
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(
        body["error"].as_str().expect("an error").contains("mine"),
        "the URL alone decides which sandbox a request reaches: {body}"
    );
}

#[tokio::test]
async fn an_unrouted_path_is_not_found() {
    let (status, _) = send(request(
        "POST",
        "/v1/sandboxes/abc/anything-else",
        Some(TOKEN),
        serde_json::json!({}),
    ))
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
