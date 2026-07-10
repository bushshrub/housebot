use std::sync::Arc;

use axum::{extract::State, routing::get, routing::post, Json, Router};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Clone)]
struct AppState {
    http: reqwest::Client,
    base_url: String,
    api_key: String,
}

impl AppState {
    fn endpoint(&self) -> String {
        format!("{}/chat/completions", self.base_url)
    }

    fn props_endpoint(&self) -> String {
        let root = self.base_url.strip_suffix("/v1").unwrap_or(&self.base_url);
        format!("{root}/props")
    }
}

// ── request / response types ─────────────────────────────────────────────────

#[derive(Deserialize)]
struct ChatStreamRequest {
    model: String,
    messages: Vec<Value>,
    #[serde(default)]
    tools: Vec<Value>,
    #[serde(default = "default_max_tokens")]
    max_tokens: u32,
}

fn default_max_tokens() -> u32 {
    4096
}

#[derive(Deserialize)]
struct ChatOnceRequest {
    model: String,
    messages: Vec<Value>,
    #[serde(default = "default_max_tokens")]
    max_tokens: u32,
}

#[derive(Serialize, Default)]
struct ToolCall {
    id: String,
    name: String,
    arguments: String,
}

#[derive(Serialize, Default)]
struct ChatCompletionResponse {
    content: Option<String>,
    tool_calls: Vec<ToolCall>,
    finish_reason: Option<String>,
    prompt_tokens: u64,
    completion_tokens: u64,
    cached_tokens: u64,
}

// ── SSE accumulator (mirrors llm.rs logic) ───────────────────────────────────

#[derive(serde::Deserialize)]
struct StreamChunk {
    #[serde(default)]
    choices: Vec<StreamChoice>,
    #[serde(default)]
    usage: Option<UsageChunk>,
}

#[derive(serde::Deserialize, Default)]
struct StreamChoice {
    #[serde(default)]
    delta: Delta,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(serde::Deserialize, Default)]
struct Delta {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    tool_calls: Option<Vec<ToolCallDelta>>,
}

#[derive(serde::Deserialize)]
struct ToolCallDelta {
    index: usize,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    function: Option<FnDelta>,
}

#[derive(serde::Deserialize, Default)]
struct FnDelta {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    arguments: Option<String>,
}

#[derive(serde::Deserialize, Default, Clone, Copy)]
struct UsageChunk {
    #[serde(default)]
    prompt_tokens: u64,
    #[serde(default)]
    completion_tokens: u64,
    #[serde(default)]
    prompt_tokens_details: PromptDetails,
}

#[derive(serde::Deserialize, Default, Clone, Copy)]
struct PromptDetails {
    #[serde(default)]
    cached_tokens: u64,
}

#[derive(Default)]
struct Accumulator {
    content: String,
    tool_calls: Vec<(String, String, String)>,
    finish_reason: Option<String>,
    usage: UsageChunk,
}

impl Accumulator {
    fn apply(&mut self, chunk: StreamChunk) {
        if let Some(u) = chunk.usage {
            self.usage = u;
        }
        let Some(choice) = chunk.choices.into_iter().next() else {
            return;
        };
        if choice.finish_reason.is_some() {
            self.finish_reason = choice.finish_reason;
        }
        if let Some(text) = choice.delta.content {
            self.content.push_str(&text);
        }
        if let Some(tcs) = choice.delta.tool_calls {
            for tc in tcs {
                while self.tool_calls.len() <= tc.index {
                    self.tool_calls.push((String::new(), String::new(), String::new()));
                }
                let slot = &mut self.tool_calls[tc.index];
                if let Some(id) = tc.id { slot.0 = id; }
                if let Some(f) = tc.function {
                    if let Some(n) = f.name { slot.1.push_str(&n); }
                    if let Some(a) = f.arguments { slot.2.push_str(&a); }
                }
            }
        }
    }

    fn finish(self) -> ChatCompletionResponse {
        ChatCompletionResponse {
            content: if self.content.is_empty() { None } else { Some(self.content) },
            tool_calls: self.tool_calls.into_iter()
                .filter(|(_, name, _)| !name.is_empty())
                .map(|(id, name, arguments)| ToolCall { id, name, arguments })
                .collect(),
            finish_reason: self.finish_reason,
            prompt_tokens: self.usage.prompt_tokens,
            completion_tokens: self.usage.completion_tokens,
            cached_tokens: self.usage.prompt_tokens_details.cached_tokens,
        }
    }
}

fn parse_sse(line: &str) -> Option<StreamChunk> {
    let payload = line.strip_prefix("data:")?.trim();
    if payload.is_empty() || payload == "[DONE]" { return None; }
    serde_json::from_str(payload).ok()
}

// ── handlers ─────────────────────────────────────────────────────────────────

async fn health() -> &'static str { "ok" }

async fn context_window(State(state): State<Arc<AppState>>) -> Json<Value> {
    let result = state.http.get(state.props_endpoint())
        .bearer_auth(&state.api_key)
        .send().await;
    match result {
        Ok(r) => {
            let v: Value = r.json().await.unwrap_or(json!({}));
            let n_ctx = v.pointer("/default_generation_settings/n_ctx")
                .and_then(|v| v.as_u64());
            Json(json!({ "tokens": n_ctx }))
        }
        Err(_) => Json(json!({ "tokens": null })),
    }
}

async fn chat_stream(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ChatStreamRequest>,
) -> Json<ChatCompletionResponse> {
    let mut body = json!({
        "model": req.model,
        "messages": req.messages,
        "max_tokens": req.max_tokens,
        "stream": true,
        "stream_options": {"include_usage": true},
    });
    if !req.tools.is_empty() {
        body["tools"] = Value::Array(req.tools);
        body["tool_choice"] = Value::String("auto".into());
    }

    let resp = match state.http.post(state.endpoint())
        .bearer_auth(&state.api_key)
        .json(&body)
        .send().await
        .and_then(|r| r.error_for_status())
    {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("LLM request failed: {e}");
            return Json(ChatCompletionResponse::default());
        }
    };

    let mut acc = Accumulator::default();
    let mut buf = String::new();
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let Ok(bytes) = chunk else { break };
        buf.push_str(&String::from_utf8_lossy(&bytes));
        while let Some(nl) = buf.find('\n') {
            let line: String = buf.drain(..=nl).collect();
            let line = line.trim_end();
            if line.is_empty() { continue; }
            if let Some(parsed) = parse_sse(line) {
                acc.apply(parsed);
            }
        }
    }
    Json(acc.finish())
}

#[derive(serde::Deserialize)]
struct OnceResponse { choices: Vec<OnceChoice> }
#[derive(serde::Deserialize)]
struct OnceChoice { message: OnceMessage }
#[derive(serde::Deserialize)]
struct OnceMessage { #[serde(default)] content: Option<String> }

async fn chat_once(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ChatOnceRequest>,
) -> Json<Value> {
    let body = json!({
        "model": req.model,
        "messages": req.messages,
        "max_tokens": req.max_tokens,
    });
    let result = state.http.post(state.endpoint())
        .bearer_auth(&state.api_key)
        .json(&body)
        .send().await;
    match result {
        Ok(r) => {
            let resp: OnceResponse = r.json().await.unwrap_or(OnceResponse { choices: vec![] });
            let content = resp.choices.into_iter().next()
                .and_then(|c| c.message.content)
                .unwrap_or_default();
            Json(json!({ "content": content }))
        }
        Err(e) => {
            tracing::error!("LLM once failed: {e}");
            Json(json!({ "content": "" }))
        }
    }
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt().with_env_filter(
        std::env::var("RUST_LOG").unwrap_or_else(|_| "info".into())
    ).init();

    let state = Arc::new(AppState {
        http: reqwest::Client::new(),
        base_url: std::env::var("LLM_BASE_URL")
            .unwrap_or_else(|_| "http://server-slop:8080/v1".into()),
        api_key: std::env::var("LLM_API_KEY")
            .unwrap_or_else(|_| "not-required".into()),
    });

    let app = Router::new()
        .route("/health", get(health))
        .route("/context_window", get(context_window))
        .route("/chat/stream", post(chat_stream))
        .route("/chat/once", post(chat_once))
        .with_state(state);

    let addr = "0.0.0.0:3002";
    tracing::info!("llm-client listening on {addr}");
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
