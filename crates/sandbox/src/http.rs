//! The sandbox HTTP API.
//!
//! `sandboxd` serves one host over a Unix socket. `sandbox-api` serves the
//! same request set over HTTP so the bot can reach any replica through a
//! Service, and so the sandbox tier scales independently of the bot.
//!
//! Handlers do no work of their own: each one shapes a `SandboxRequest` and
//! hands it to `server::process_request`, which is the single implementation
//! both transports share.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};

use crate::protocol::*;
use crate::server::{self, Server};

#[derive(Clone)]
pub struct ApiState {
    server: Arc<Server>,
    token: Arc<String>,
}

/// Read the bearer token every request must present.
///
/// There is no unauthenticated mode: an open sandbox API is arbitrary code
/// execution for anyone who can reach the Service.
pub fn token_from_env() -> anyhow::Result<String> {
    let token = std::env::var("SANDBOX_API_TOKEN").unwrap_or_default();
    if token.len() < MIN_TOKEN_LENGTH {
        anyhow::bail!("SANDBOX_API_TOKEN must be set to at least {MIN_TOKEN_LENGTH} characters");
    }
    Ok(token)
}

const MIN_TOKEN_LENGTH: usize = 32;

pub fn router(server: Arc<Server>, token: String) -> Router {
    let state = ApiState {
        server,
        token: Arc::new(token),
    };

    let sandboxes = Router::new()
        .route("/v1/sandboxes", post(start))
        .route("/v1/sandboxes/{id}", delete(close))
        .route("/v1/sandboxes/{id}/clone", post(clone_repository))
        .route("/v1/sandboxes/{id}/exec", post(exec))
        .route("/v1/sandboxes/{id}/search", post(search_code))
        .route("/v1/sandboxes/{id}/list", post(list_files))
        .route("/v1/sandboxes/{id}/read", post(read_file))
        .route("/v1/sandboxes/{id}/file", put(write_file))
        // Authentication runs as a layer rather than inside each handler so
        // it precedes body extraction: an unauthenticated caller must not be
        // able to probe request schemas through validation errors.
        .route_layer(middleware::from_fn_with_state(state.clone(), authenticate));

    Router::new()
        // Probes stay outside the authenticated routes: the kubelet has no
        // token, and neither reveals anything about a sandbox.
        .route("/healthz", get(health))
        .route("/readyz", get(health))
        .merge(sandboxes)
        .with_state(state)
}

async fn health() -> StatusCode {
    StatusCode::OK
}

/// The error body every failing route returns.
struct ApiError {
    status: StatusCode,
    message: String,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(serde_json::json!({"error": self.message})),
        )
            .into_response()
    }
}

/// Map a daemon error onto a status code.
///
/// The daemon reports failures as prose, so the mapping keys off the same
/// prefixes the handlers produce and defaults to a server-side fault.
fn status_for(message: &str) -> StatusCode {
    if message.starts_with("unknown sandbox") {
        StatusCode::NOT_FOUND
    } else if message.starts_with("invalid ") || message.contains("exceeds") {
        StatusCode::BAD_REQUEST
    } else {
        StatusCode::BAD_GATEWAY
    }
}

/// Compare tokens without leaking their common prefix through timing.
fn token_matches(expected: &str, presented: &str) -> bool {
    if expected.len() != presented.len() {
        return false;
    }
    expected
        .bytes()
        .zip(presented.bytes())
        .fold(0u8, |acc, (a, b)| acc | (a ^ b))
        == 0
}

async fn authenticate(
    State(state): State<ApiState>,
    request: axum::extract::Request,
    next: Next,
) -> Response {
    match authorize(&state, request.headers()) {
        Ok(()) => next.run(request).await,
        Err(error) => error.into_response(),
    }
}

fn authorize(state: &ApiState, headers: &HeaderMap) -> Result<(), ApiError> {
    let presented = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .unwrap_or_default();

    if token_matches(&state.token, presented) {
        Ok(())
    } else {
        Err(ApiError {
            status: StatusCode::UNAUTHORIZED,
            message: "invalid or missing bearer token".to_string(),
        })
    }
}

/// Dispatch to the shared request handler and shape the result.
async fn dispatch(
    state: &ApiState,
    method: &str,
    params: serde_json::Value,
) -> Result<Json<serde_json::Value>, ApiError> {
    let request = SandboxRequest::new(method, params);
    server::process_request(&request, &state.server)
        .await
        .into_result()
        .map(Json)
        .map_err(|message| ApiError {
            status: status_for(&message),
            message,
        })
}

/// Attach the path's sandbox ID to a body that does not carry one.
///
/// The path is the only source of the ID, so a body that also names one cannot
/// reach a sandbox the URL did not address.
fn with_sandbox_id(mut params: serde_json::Value, id: &str) -> serde_json::Value {
    params["sandbox_id"] = serde_json::Value::String(id.to_string());
    params
}

async fn start(
    State(state): State<ApiState>,
    Json(body): Json<StartParams>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let params = serde_json::to_value(body).unwrap_or_default();
    dispatch(&state, "start", params).await
}

async fn close(
    State(state): State<ApiState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let params = with_sandbox_id(serde_json::json!({}), &id);
    dispatch(&state, "close", params).await
}

/// The body of a route that takes its sandbox ID from the path.
#[derive(serde::Deserialize)]
struct Body(serde_json::Value);

async fn clone_repository(
    State(state): State<ApiState>,
    Path(id): Path<String>,
    Json(Body(body)): Json<Body>,
) -> Result<Json<serde_json::Value>, ApiError> {
    dispatch(&state, "clone_repository", with_sandbox_id(body, &id)).await
}

async fn exec(
    State(state): State<ApiState>,
    Path(id): Path<String>,
    Json(Body(body)): Json<Body>,
) -> Result<Json<serde_json::Value>, ApiError> {
    dispatch(&state, "run", with_sandbox_id(body, &id)).await
}

async fn search_code(
    State(state): State<ApiState>,
    Path(id): Path<String>,
    Json(Body(body)): Json<Body>,
) -> Result<Json<serde_json::Value>, ApiError> {
    dispatch(&state, "search_code", with_sandbox_id(body, &id)).await
}

async fn list_files(
    State(state): State<ApiState>,
    Path(id): Path<String>,
    Json(Body(body)): Json<Body>,
) -> Result<Json<serde_json::Value>, ApiError> {
    dispatch(&state, "list_files", with_sandbox_id(body, &id)).await
}

async fn read_file(
    State(state): State<ApiState>,
    Path(id): Path<String>,
    Json(Body(body)): Json<Body>,
) -> Result<Json<serde_json::Value>, ApiError> {
    dispatch(&state, "read_file", with_sandbox_id(body, &id)).await
}

async fn write_file(
    State(state): State<ApiState>,
    Path(id): Path<String>,
    Json(Body(body)): Json<Body>,
) -> Result<Json<serde_json::Value>, ApiError> {
    dispatch(&state, "write_file", with_sandbox_id(body, &id)).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_short_token_is_refused_at_startup() {
        std::env::set_var("SANDBOX_API_TOKEN", "too-short");
        assert!(token_from_env().is_err());
        std::env::remove_var("SANDBOX_API_TOKEN");
    }

    #[test]
    fn a_missing_token_is_refused_at_startup() {
        std::env::remove_var("SANDBOX_API_TOKEN");
        assert!(
            token_from_env().is_err(),
            "an unauthenticated sandbox API is remote code execution"
        );
    }

    #[test]
    fn tokens_of_different_lengths_never_match() {
        assert!(!token_matches("abcdef", "abc"));
    }

    #[test]
    fn only_the_exact_token_matches() {
        assert!(token_matches("s3cret", "s3cret"));
        assert!(!token_matches("s3cret", "s3crft"));
    }

    #[test]
    fn an_unknown_sandbox_is_not_found_rather_than_a_fault() {
        assert_eq!(
            status_for("unknown sandbox: abc"),
            StatusCode::NOT_FOUND,
            "a closed sandbox is the caller's stale handle, not a server fault"
        );
    }

    #[test]
    fn rejected_input_is_a_client_error() {
        assert_eq!(status_for("invalid path: escapes"), StatusCode::BAD_REQUEST);
        assert_eq!(
            status_for("content exceeds 100 bytes"),
            StatusCode::BAD_REQUEST
        );
    }

    #[test]
    fn a_runtime_failure_is_a_gateway_error() {
        assert_eq!(
            status_for("docker command failed: no such image"),
            StatusCode::BAD_GATEWAY
        );
    }

    #[test]
    fn the_path_id_overrides_the_body_id() {
        let body = serde_json::json!({"sandbox_id": "someone-elses", "path": "/workspace"});
        assert_eq!(with_sandbox_id(body, "mine")["sandbox_id"], "mine");
    }
}
