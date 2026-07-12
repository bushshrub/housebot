//! The agentic loop: builds prompts, streams completions from the LLM, dispatches tool
//! calls (built-in tools + MCP servers), and persists per-user history and memory.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use async_trait::async_trait;
use chrono::Local;
use serde_json::{json, Value};

use crate::config;
use crate::github_issues::GitHubIssueReporter;
use crate::history::History;
use crate::llm::{ChatClient, OpenAiClient, TextSink, ThinkingMode, TokenUsage};
use crate::mcp::McpServer;
use crate::memory::Memory;
use crate::rate_limit::RateLimiter;
use crate::reminders::Reminders;
use crate::skills::{Skill, Skills};
use crate::tools;
use crate::tools::common_crawl::CommonCrawl;
use crate::tools::searxng::SearxNg;
use crate::tools::web_fetch::WebFetch;

/// An inbound media attachment, base64-encoded for the multimodal API.
#[derive(Debug, Clone)]
pub struct MediaData {
    pub media_type: String,
    pub data: String,
}

/// One user turn to run through the agent.
#[derive(Debug, Clone, Copy)]
pub struct AgentRequest<'a> {
    pub user_id: &'a str,
    pub username: &'a str,
    pub text: &'a str,
    pub media: &'a [MediaData],
    /// Optional personality/tone override injected into the system prompt.
    pub personality: Option<&'a str>,
    /// Reasoning budget for this user's requests.
    pub thinking: ThinkingMode,
}

impl<'a> AgentRequest<'a> {
    /// A plain text request with default settings (used by tests and headless callers).
    pub fn text(user_id: &'a str, username: &'a str, text: &'a str) -> Self {
        Self {
            user_id,
            username,
            text,
            media: &[],
            personality: None,
            thinking: ThinkingMode::default(),
        }
    }
}

/// The outcome of one `Agent::run`.
#[derive(Debug, Clone, Default)]
pub struct AgentResult {
    pub text: String,
    pub session_notice: Option<String>,
    pub tools_called: Vec<String>,
}

#[derive(Debug, Clone, Copy)]
pub struct SessionInfo {
    pub context_tokens: usize,
    pub context_window_tokens: usize,
    pub messages: usize,
    pub requests: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cached_tokens: u64,
}

/// Per-request callbacks used to surface progress into the chat surface.
#[async_trait]
pub trait AgentHooks: Send + Sync {
    /// Cumulative assistant text as it streams in.
    async fn on_text_stream(&self, _partial: &str) {}
    /// A tool is about to run.
    async fn on_tool_called(&self, _tool: &str, _args: &Value) {}
    /// A progress update from a long-running operation.
    async fn on_progress(&self, _line: &str) {}
}

/// No-op hooks (used in tests and headless contexts).
pub struct NoHooks;
#[async_trait]
impl AgentHooks for NoHooks {}

struct TextStreamAdapter<'a>(&'a dyn AgentHooks);
#[async_trait]
impl TextSink for TextStreamAdapter<'_> {
    async fn push(&self, partial: &str) {
        self.0.on_text_stream(partial).await;
    }
}

/// Result of dispatching a single tool call.
enum ToolOutcome {
    Text(String),
}

/// The agent: LLM client, storage, tools, and connected MCP servers.
pub struct Agent {
    client: Arc<dyn ChatClient>,
    model: String,
    context_window_tokens: usize,
    history: History,
    memory: Memory,
    skills: Skills,
    reminders: Reminders,
    reporter: Arc<GitHubIssueReporter>,
    rate_limiter: RateLimiter,
    searxng: SearxNg,
    web_fetch: WebFetch,
    common_crawl: CommonCrawl,
    mcp_servers: Vec<McpServer>,
    session_stats: tokio::sync::Mutex<HashMap<String, SessionStats>>,
}

#[derive(Debug, Clone, Copy, Default)]
struct SessionStats {
    requests: u64,
    context_tokens: u64,
    input_tokens: u64,
    output_tokens: u64,
    cached_tokens: u64,
}

impl Agent {
    /// Build an agent from environment configuration and start MCP servers.
    pub async fn from_env() -> Self {
        let client: Arc<dyn ChatClient> = Arc::new(OpenAiClient::new(
            config::env_or("LLM_BASE_URL", "http://server-slop:8080/v1"),
            config::env_or("LLM_API_KEY", "not-required"),
        ));
        let mcp_servers = start_mcp_servers().await;
        let context_window_tokens = client
            .context_window_tokens()
            .await
            .ok()
            .flatten()
            .map(|tokens| tokens as usize)
            .unwrap_or_else(|| config::env_parse("MAX_CONTEXT_TOKENS", 10_000));
        Self {
            client,
            model: config::env_or("LLM_MODEL", "gemma-4-12b-qat-q4kxl"),
            context_window_tokens,
            history: History::default(),
            memory: Memory::default(),
            skills: Skills::default(),
            reminders: Reminders::default(),
            reporter: Arc::new(GitHubIssueReporter::default()),
            rate_limiter: tools::feature_request::default_rate_limiter(),
            searxng: SearxNg::from_env(),
            web_fetch: WebFetch::default(),
            common_crawl: CommonCrawl::default(),
            mcp_servers,
            session_stats: tokio::sync::Mutex::new(HashMap::new()),
        }
    }

    /// Access to the reminders store (the bot's delivery loop needs it).
    pub fn reminders(&self) -> &Reminders {
        &self.reminders
    }

    // ── session lifecycle ────────────────────────────────────────────────────

    /// Clear conversation history and counters without preserving a summary.
    pub async fn reset_session(&self, user_id: &str) {
        self.session_stats.lock().await.remove(user_id);
        let _ = self.history.clear(user_id).await;
    }

    /// Summarize the current conversation, then start a fresh session.
    pub async fn compact_session(&self, user_id: &str) {
        self.compact_session_with_hooks(user_id, &NoHooks).await;
    }

    /// Summarize the current conversation, reporting coarse-grained progress to the caller.
    pub async fn compact_session_with_hooks(&self, user_id: &str, hooks: &dyn AgentHooks) {
        tracing::info!(target: "housebot::agent", user_id, "Compacting session");
        hooks.on_progress("compact:10").await;
        self.session_stats.lock().await.remove(user_id);
        let past = self.history.load(user_id).await;
        if past.is_empty() {
            hooks.on_progress("compact:100:Nothing to compact.").await;
            return;
        }
        hooks.on_progress("compact:25").await;
        let user_memory = self.memory.load(user_id).await;
        let convo: String = past
            .iter()
            .filter_map(|m| {
                let role = m.get("role").and_then(|r| r.as_str())?;
                let content = m.get("content").and_then(|c| c.as_str())?;
                Some(format!("{}: {}", role.to_uppercase(), content))
            })
            .collect::<Vec<_>>()
            .join("\n");

        let truncated: String = convo.chars().take(6000).collect();
        let prompt = format!(
            "The following is a conversation that has ended. Write a concise bullet-point summary \
             of the key facts, preferences, and decisions discussed. This will be appended to the \
             user's memory for future reference. Be brief — 3-8 bullets max.\n\nCONVERSATION:\n{truncated}"
        );
        hooks.on_progress("compact:45").await;
        let completion = self
            .client
            .chat_once(
                &self.model,
                &[json!({"role": "user", "content": prompt})],
                512,
            )
            .await
            .unwrap_or_default();
        self.record_usage(user_id, completion.usage).await;
        let summary = completion.content.unwrap_or_default();

        if !summary.trim().is_empty() {
            let now = Local::now().format("%Y-%m-%d %H:%M");
            let mut updated = String::new();
            if !user_memory.trim().is_empty() {
                updated.push_str(user_memory.trim_end());
                updated.push_str("\n\n");
            }
            updated.push_str(&format!("## Conversation summary ({now})\n{summary}"));
            let _ = self.memory.save(user_id, &updated).await;
        }
        hooks.on_progress("compact:80").await;
        let _ = self.history.clear(user_id).await;
        hooks
            .on_progress("compact:100:Conversation compacted.")
            .await;
    }

    pub fn model_info(&self) -> String {
        format!(
            "**Model**\nName: `{}`\nMax context: ~{} tokens",
            self.model, self.context_window_tokens
        )
    }

    pub async fn session_info(&self, user_id: &str) -> SessionInfo {
        let history = self.history.load(user_id).await;
        let context_window_tokens = self
            .client
            .context_window_tokens()
            .await
            .ok()
            .flatten()
            .map(|tokens| tokens as usize)
            .unwrap_or(self.context_window_tokens);
        let stats = self
            .session_stats
            .lock()
            .await
            .get(user_id)
            .copied()
            .unwrap_or_default();
        SessionInfo {
            context_tokens: stats.context_tokens as usize,
            context_window_tokens,
            messages: history.len(),
            requests: stats.requests,
            input_tokens: stats.input_tokens,
            output_tokens: stats.output_tokens,
            cached_tokens: stats.cached_tokens,
        }
    }

    async fn last_context_tokens(&self, user_id: &str) -> u64 {
        self.session_stats
            .lock()
            .await
            .get(user_id)
            .map_or(0, |stats| stats.context_tokens)
    }

    async fn record_usage(&self, user_id: &str, usage: TokenUsage) {
        let mut all = self.session_stats.lock().await;
        let stats = all.entry(user_id.to_string()).or_default();
        stats.requests += 1;
        stats.context_tokens = usage.prompt_tokens + usage.completion_tokens;
        stats.input_tokens += usage.prompt_tokens;
        stats.output_tokens += usage.completion_tokens;
        stats.cached_tokens += usage.prompt_tokens_details.cached_tokens;
    }

    // ── main loop ────────────────────────────────────────────────────────────

    /// Run one user turn to completion, returning the final assistant text.
    pub async fn run(&self, request: AgentRequest<'_>, hooks: &dyn AgentHooks) -> AgentResult {
        let AgentRequest {
            user_id,
            username,
            text,
            media,
            personality,
            thinking,
        } = request;
        let run_started = std::time::Instant::now();
        tracing::info!(
            target: "housebot::agent",
            user_id,
            username,
            thinking = %thinking,
            text_chars = text.chars().count(),
            media = media.len(),
            "Agent run started"
        );
        let mut user_memory = self.memory.load(user_id).await;
        let mut past = self.history.load(user_id).await;
        let mut session_notice = None;
        let new_user_message = build_user_message(text, media);

        let previous_usage = self.last_context_tokens(user_id).await as f64
            / self.context_window_tokens.max(1) as f64;
        if !past.is_empty() && previous_usage >= 0.8 {
            tracing::info!("Context at 80% for {user_id} — auto-compacting session");
            self.compact_session_with_hooks(user_id, hooks).await;
            past.clear();
            user_memory = self.memory.load(user_id).await;
            session_notice = Some(
                "⚠️ The context window reached 80%, so I compacted the conversation and started a new session. Use /session to check your current context usage."
                    .into(),
            );
        }

        let all_skills = self.skills.load_all().await;
        let system = json!({
            "role": "system",
            "content": build_system_prompt(username, user_id, &user_memory, &all_skills, personality),
        });
        let mut messages: Vec<Value> = Vec::with_capacity(past.len() + 2);
        messages.push(system);
        messages.extend(past);
        messages.push(new_user_message.clone());

        let tools = self.build_tools().await;
        let mut turn_messages: Vec<Value> = Vec::new();
        let mut tools_called = Vec::new();

        let final_text = 'agent_loop: loop {
            let text_sink = TextStreamAdapter(hooks);
            let completion = match self
                .client
                .chat_stream(&self.model, &messages, &tools, thinking, Some(&text_sink))
                .await
            {
                Ok(c) => c,
                Err(e) => {
                    tracing::error!("LLM error: {e}");
                    break "Sorry, something went wrong contacting the model.".to_string();
                }
            };
            let context_tokens =
                completion.usage.prompt_tokens + completion.usage.completion_tokens;
            self.record_usage(user_id, completion.usage).await;
            let usage = context_tokens as f64 / self.context_window_tokens.max(1) as f64;
            if usage >= 0.7 {
                session_notice = Some(if usage >= 0.8 {
                    "⚠️ The context window reached 80% based on the model's reported usage. It will be compacted automatically before the next message. Use /session to check your current context usage.".into()
                } else {
                    format!(
                        "⚠️ The context window is {:.0}% full based on the model's reported usage. It will compact automatically at 80%. Use /session to check your current context usage.",
                        usage * 100.0
                    )
                });
            }

            let mut assistant = json!({ "role": "assistant", "content": completion.content });
            if !completion.tool_calls.is_empty() {
                assistant["tool_calls"] = Value::Array(
                    completion
                        .tool_calls
                        .iter()
                        .map(|tc| {
                            json!({
                                "id": tc.id,
                                "type": "function",
                                "function": {"name": tc.name, "arguments": tc.arguments},
                            })
                        })
                        .collect(),
                );
            }
            messages.push(assistant.clone());
            turn_messages.push(assistant);

            let is_tool_turn = completion.finish_reason.as_deref() == Some("tool_calls")
                && !completion.tool_calls.is_empty();
            if !is_tool_turn {
                break completion.content.unwrap_or_default();
            }

            for tc in &completion.tool_calls {
                let args: Value = serde_json::from_str(&tc.arguments).unwrap_or(json!({}));
                tools_called.push(tc.name.clone());
                hooks.on_tool_called(&tc.name, &args).await;
                let outcome = self.dispatch_tool(&tc.name, &args, user_id, hooks).await;
                let ToolOutcome::Text(content) = outcome;
                let tool_msg = json!({
                    "role": "tool",
                    "tool_call_id": tc.id,
                    "content": content,
                });
                messages.push(tool_msg.clone());
                turn_messages.push(tool_msg);

                // Search rate limits are not recoverable within this run. Stop the
                // tool loop after the first limited response so the model cannot keep
                // retrying the search and waiting for another rate-limit window.
                if tc.name == "web_search" && search_rate_limited(&content) {
                    break 'agent_loop "Web search is temporarily rate-limited. Please try again in a few minutes.".to_string();
                }
            }
        };

        if let Err(e) = self
            .history
            .append_turn(user_id, new_user_message, turn_messages)
            .await
        {
            tracing::error!("Failed to save history for {user_id}: {e}");
        }

        tracing::info!(
            target: "housebot::agent",
            user_id,
            tools_called = tools_called.len(),
            response_chars = final_text.chars().count(),
            elapsed_ms = run_started.elapsed().as_millis() as u64,
            "Agent run finished"
        );
        AgentResult {
            text: if final_text.is_empty() {
                "(no response)".to_string()
            } else {
                final_text
            },
            session_notice,
            tools_called,
        }
    }

    async fn build_tools(&self) -> Vec<Value> {
        let mut tools = Vec::new();
        for server in &self.mcp_servers {
            for tool in server.list_tools().await {
                tools.push(to_openai_tool(
                    &format!("{}__{}", server.prefix, tool.name),
                    &tool.description,
                    tool.input_schema,
                ));
            }
        }
        for def in [
            tools::searxng::definition(),
            tools::web_fetch::definition(),
            tools::common_crawl::definition(),
            update_memory_tool(),
            run_skill_tool(),
            tools::feature_request::definition(),
            tools::remind::definition(),
            tools::summarize_url::definition(),
            tools::translate::definition(),
        ] {
            let (name, desc, params) = flatten_tool(&def);
            tools.push(to_openai_tool(&name, &desc, params));
        }
        tools
    }

    async fn dispatch_tool(
        &self,
        name: &str,
        args: &Value,
        user_id: &str,
        _hooks: &dyn AgentHooks,
    ) -> ToolOutcome {
        let started = std::time::Instant::now();
        let outcome = self.dispatch_tool_inner(name, args, user_id).await;
        let ToolOutcome::Text(content) = &outcome;
        tracing::info!(
            target: "housebot::agent",
            user_id,
            tool = name,
            result_chars = content.chars().count(),
            is_error = content.starts_with("Error:"),
            elapsed_ms = started.elapsed().as_millis() as u64,
            "Tool call finished"
        );
        outcome
    }

    async fn dispatch_tool_inner(&self, name: &str, args: &Value, user_id: &str) -> ToolOutcome {
        match name {
            "web_search" => ToolOutcome::Text(
                self.searxng
                    .search(
                        str_arg(args, "query"),
                        u64_arg(args, "max_results", 10) as usize,
                        str_arg(args, "language"),
                    )
                    .await,
            ),
            "fetch_webpage" => ToolOutcome::Text(
                self.web_fetch
                    .fetch_content(
                        str_arg(args, "url"),
                        u64_arg(args, "start_index", 0) as usize,
                        u64_arg(args, "max_length", 8000) as usize,
                    )
                    .await,
            ),
            "common_crawl__search" => ToolOutcome::Text(
                self.common_crawl
                    .search(
                        str_arg(args, "pattern"),
                        str_arg(args, "crawl"),
                        args.get("match_type")
                            .and_then(Value::as_str)
                            .unwrap_or("exact"),
                        u64_arg(args, "max_results", 10) as usize,
                    )
                    .await,
            ),
            "update_memory" => {
                let new_content = str_arg(args, "memory_content");
                let _ = self.memory.save(user_id, new_content).await;
                ToolOutcome::Text("Memory updated.".to_string())
            }
            "create_feature_request" => ToolOutcome::Text(
                tools::feature_request::create_feature_request(
                    &self.reporter,
                    &self.rate_limiter,
                    str_arg(args, "title"),
                    str_arg(args, "description"),
                    user_id,
                )
                .await,
            ),
            "set_reminder" => {
                let delay = args
                    .get("delay_minutes")
                    .and_then(Value::as_f64)
                    .unwrap_or(0.0);
                ToolOutcome::Text(
                    tools::remind::create_reminder(
                        &self.reminders,
                        user_id,
                        str_arg(args, "message"),
                        delay,
                    )
                    .await,
                )
            }
            "summarize_url" => ToolOutcome::Text(
                tools::summarize_url::fetch_and_summarize(
                    &*self.client,
                    &self.model,
                    str_arg(args, "url"),
                )
                .await,
            ),
            "translate" => ToolOutcome::Text(
                tools::translate::translate_text(
                    &*self.client,
                    &self.model,
                    str_arg(args, "text"),
                    str_arg(args, "target_language"),
                )
                .await,
            ),
            "run_skill" => {
                let skill_name = str_arg(args, "name");
                let input = str_arg(args, "input");
                match self.skills.get(skill_name).await {
                    None => ToolOutcome::Text(format!("Error: Skill '{skill_name}' not found.")),
                    Some(skill) => {
                        let msgs = vec![
                            json!({"role": "system", "content": skill.prompt}),
                            json!({"role": "user", "content": input}),
                        ];
                        let completion = self
                            .client
                            .chat_once(&self.model, &msgs, 4096)
                            .await
                            .unwrap_or_default();
                        ToolOutcome::Text(completion.content.unwrap_or_default())
                    }
                }
            }
            _ if name.contains("__") => {
                let (prefix, tool_name) = name.split_once("__").unwrap();
                for server in &self.mcp_servers {
                    if server.prefix == prefix {
                        return match server.call_tool(tool_name, args.clone()).await {
                            Ok(text) => ToolOutcome::Text(text),
                            Err(e) => ToolOutcome::Text(format!("Error: {e}")),
                        };
                    }
                }
                ToolOutcome::Text(format!("Unknown tool: {name}"))
            }
            _ => ToolOutcome::Text(format!("Unknown tool: {name}")),
        }
    }
}

// ── MCP server configuration ─────────────────────────────────────────────────

async fn start_mcp_servers() -> Vec<McpServer> {
    let mut servers = Vec::new();
    match (
        std::env::var("JELLYFIN_URL"),
        std::env::var("JELLYFIN_API_KEY"),
    ) {
        (Ok(url), Ok(key)) if !url.is_empty() && !key.is_empty() => {
            if let Some(s) = McpServer::start(
                "jellyfin",
                "jellyfin-mcp",
                &["--read-only".to_string()],
                &[
                    ("JELLYFIN_URL".into(), url),
                    ("JELLYFIN_API_KEY".into(), key),
                ],
            )
            .await
            {
                servers.push(s);
            }
        }
        _ => tracing::warn!("JELLYFIN_URL or JELLYFIN_API_KEY not set — Jellyfin MCP disabled"),
    }
    servers
}

// ── pure helpers ─────────────────────────────────────────────────────────────

fn build_user_message(text: &str, media_data: &[MediaData]) -> Value {
    if media_data.is_empty() {
        return json!({"role": "user", "content": text});
    }
    let mut content: Vec<Value> = media_data
        .iter()
        .map(|media| {
            if media.media_type.starts_with("image/") {
                json!({
                    "type": "image_url",
                    "image_url": {"url": format!("data:{};base64,{}", media.media_type, media.data)},
                })
            } else if media.media_type.starts_with("audio/") {
                json!({
                    "type": "input_audio",
                    "input_audio": {"data": media.data},
                })
            } else {
                json!({
                    "type": "input_video",
                    "input_video": {"data": media.data},
                })
            }
        })
        .collect();
    content.push(json!({"type": "text", "text": text}));
    json!({"role": "user", "content": content})
}

/// Build the system prompt for a turn.
pub fn build_system_prompt(
    username: &str,
    user_id: &str,
    user_memory: &str,
    all_skills: &BTreeMap<String, Skill>,
    personality: Option<&str>,
) -> String {
    let now = Local::now().format("%Y-%m-%d %H:%M");
    let memory_section = if user_memory.trim().is_empty() {
        String::new()
    } else {
        format!("\n\n## Your memory about {username}\n{user_memory}")
    };
    let personality_section = match personality {
        Some(p) if !p.trim().is_empty() => {
            format!("\n\n## Personality / tone for this user\n{}", p.trim())
        }
        _ => String::new(),
    };
    let skills_section = if all_skills.is_empty() {
        "\n- run_skill — Execute a custom skill by name. No skills are defined yet; users can add \
         them with `!skill add`."
            .to_string()
    } else {
        let lines: Vec<String> = all_skills
            .values()
            .map(|s| format!("  - **{}**: {}", s.name, s.description_or_name()))
            .collect();
        format!(
            "\n- run_skill — Execute a custom skill by name with an input string. Available skills:\n{}",
            lines.join("\n")
        )
    };
    format!(
        "You are a helpful house assistant bot in a Discord server. You help with media, web \
search, and software development tasks.\n\nCurrent date/time: {now}\nCurrent user: {username} \
(ID: {user_id}){memory_section}{personality_section}\n\n## Tools\n- web_search — Search the web (SearXNG) for current \
information.\n- fetch_webpage — Fetch and read the text of a public webpage.\n- common_crawl__search — Search historical URL captures in the Common Crawl index.\n- jellyfin__* — Query the household Jellyfin media server for movies, shows, music. \
READ ONLY — only call get_* / search_* / list_* methods; never call mutating actions.\n- \
Programming tasks are outside the bot's scope.\n- update_memory — Persist important facts about the current user for future \
conversations. Write the full memory each time.\n- create_feature_request — File a GitHub issue \
for a feature the user wants added to this bot.\n- set_reminder — Set a timed reminder; the bot \
will DM the user when the delay elapses.\n- summarize_url — Fetch a public web URL and return a \
concise summary.\n- translate — Translate text to any language using the LLM.{skills_section}\n\n\
## Guidelines\n- Be conversational and friendly.\n- Use Jellyfin tools for any media questions \
before guessing.\n- Use web_search for factual or current-events questions. If web_search returns a rate-limit \
error, stop using it for this request and do not retry it repeatedly; use \
common_crawl__search for historical URL evidence when appropriate, or explain that the search \
service is temporarily unavailable.\n- Do not execute code or delegate coding tasks.\n- Update memory when you learn \
something worth remembering.\n- Keep responses concise unless asked for detail.\n- If a user \
requests a feature or improvement to this bot, immediately call create_feature_request with a \
clear title and description, then tell them the issue URL.\n- If a tool returns an error message \
(starts with \"Error:\"), quote it exactly — do not paraphrase or soften it.\n- When the user's \
message exceeds 500 characters, begin your reply with a **TL;DR:** line (one sentence) \
summarizing what they asked.\n"
    )
}

fn update_memory_tool() -> Value {
    json!({
        "name": "update_memory",
        "description": "Update your persistent memory about the current user. Write the complete \
            updated memory content each time, not just the new piece.",
        "input_schema": {
            "type": "object",
            "properties": {"memory_content": {"type": "string", "description": "Full updated memory in markdown format."}},
            "required": ["memory_content"]
        }
    })
}

fn run_skill_tool() -> Value {
    json!({
        "name": "run_skill",
        "description": "Execute a named skill — a custom prompt template saved by users. Pass the \
            skill name and the text input to process.",
        "input_schema": {
            "type": "object",
            "properties": {
                "name": {"type": "string", "description": "The skill name to execute."},
                "input": {"type": "string", "description": "The text input to pass to the skill."}
            },
            "required": ["name", "input"]
        }
    })
}

/// Wrap a tool in the OpenAI function-calling envelope.
pub fn to_openai_tool(name: &str, description: &str, parameters: Value) -> Value {
    json!({
        "type": "function",
        "function": {"name": name, "description": description, "parameters": parameters},
    })
}

/// Convert an internal tool definition into `(name, description, parameters)`.
pub fn flatten_tool(tool_def: &Value) -> (String, String, Value) {
    let name = tool_def
        .get("name")
        .and_then(|n| n.as_str())
        .unwrap_or("")
        .to_string();
    let description = tool_def
        .get("description")
        .and_then(|d| d.as_str())
        .unwrap_or("")
        .to_string();
    let parameters = tool_def
        .get("input_schema")
        .or_else(|| tool_def.get("parameters"))
        .cloned()
        .unwrap_or_else(|| json!({}));
    (name, description, parameters)
}

/// Extract a string argument from tool-call args, defaulting to empty.
fn str_arg<'a>(args: &'a Value, key: &str) -> &'a str {
    args.get(key).and_then(Value::as_str).unwrap_or("")
}

/// Extract an unsigned integer argument from tool-call args.
fn u64_arg(args: &Value, key: &str, default: u64) -> u64 {
    args.get(key).and_then(Value::as_u64).unwrap_or(default)
}

fn search_rate_limited(content: &str) -> bool {
    let content = content.to_ascii_lowercase();
    content.contains("returned http 429")
        || content.contains("too many requests")
        || content.contains("rate limit")
        || content.contains("temporarily blocked")
}

#[cfg(test)]
impl Agent {
    /// Construct an agent wired to a test client and temp-backed stores.
    pub fn for_test(
        client: Arc<dyn ChatClient>,
        history: History,
        memory: Memory,
        skills: Skills,
        reminders: Reminders,
    ) -> Self {
        Self {
            client,
            model: "test-model".into(),
            context_window_tokens: 10_000,
            history,
            memory,
            skills,
            reminders,
            reporter: Arc::new(GitHubIssueReporter::new(
                String::new(),
                String::new(),
                String::new(),
                String::new(),
            )),
            rate_limiter: tools::feature_request::default_rate_limiter(),
            searxng: SearxNg::from_env(),
            web_fetch: WebFetch::default(),
            common_crawl: CommonCrawl::default(),
            mcp_servers: vec![],
            session_stats: tokio::sync::Mutex::new(HashMap::new()),
        }
    }

    pub fn set_max_context_tokens(&mut self, n: usize) {
        self.context_window_tokens = n;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::MockChatClient;
    use tempfile::TempDir;

    fn empty_skills() -> BTreeMap<String, Skill> {
        BTreeMap::new()
    }

    #[test]
    fn system_prompt_includes_username_and_id() {
        let p = build_system_prompt("Alice", "123", "", &empty_skills(), None);
        assert!(p.contains("Alice"));
        assert!(p.contains("123"));
    }

    #[test]
    fn system_prompt_memory_section_present_when_nonempty() {
        let p = build_system_prompt("Alice", "123", "Likes cats", &empty_skills(), None);
        assert!(p.contains("Likes cats"));
        assert!(p.contains("Your memory"));
    }

    #[test]
    fn system_prompt_memory_absent_when_blank() {
        assert!(
            !build_system_prompt("Alice", "123", "   ", &empty_skills(), None)
                .contains("Your memory")
        );
    }

    #[test]
    fn system_prompt_lists_skills() {
        let mut skills = BTreeMap::new();
        skills.insert(
            "greet".into(),
            Skill {
                name: "greet".into(),
                description: Some("Say hello".into()),
                prompt: "..".into(),
                created_by: None,
            },
        );
        let p = build_system_prompt("Alice", "123", "", &skills, None);
        assert!(p.contains("greet"));
        assert!(p.contains("Say hello"));
    }

    #[test]
    fn system_prompt_placeholder_without_skills() {
        assert!(
            build_system_prompt("Alice", "123", "", &empty_skills(), None)
                .contains("No skills are defined yet")
        );
    }

    #[test]
    fn system_prompt_has_tldr_and_500() {
        let p = build_system_prompt("Alice", "123", "", &empty_skills(), None);
        assert!(p.contains("TL;DR"));
        assert!(p.contains("500"));
    }

    #[test]
    fn system_prompt_excludes_code_execution() {
        let p = build_system_prompt("Alice", "123", "", &empty_skills(), None);
        assert!(!p.contains("code execution"));
    }

    #[test]
    fn flatten_tool_extracts_fields() {
        let tool = json!({"name": "my_tool", "description": "does stuff", "input_schema": {"type": "object"}});
        let (n, d, p) = flatten_tool(&tool);
        assert_eq!(n, "my_tool");
        assert_eq!(d, "does stuff");
        assert_eq!(p, json!({"type": "object"}));
    }

    #[test]
    fn flatten_tool_falls_back_to_parameters() {
        let tool = json!({"name": "t", "parameters": {"type": "object"}});
        assert_eq!(flatten_tool(&tool).2, json!({"type": "object"}));
    }

    #[test]
    fn to_openai_tool_wraps_in_envelope() {
        let t = to_openai_tool("my_tool", "does stuff", json!({"type": "object"}));
        assert_eq!(t["type"], "function");
        assert_eq!(t["function"]["name"], "my_tool");
        assert_eq!(t["function"]["parameters"], json!({"type": "object"}));
    }

    #[test]
    fn search_rate_limit_errors_are_detected() {
        assert!(search_rate_limited(
            "Error: SearXNG returned HTTP 429 Too Many Requests"
        ));
        assert!(search_rate_limited("SearXNG rate limit reached"));
        assert!(!search_rate_limited(
            "Error: search request failed: timeout"
        ));
    }

    #[test]
    fn build_user_message_plain_text() {
        let m = build_user_message("hi", &[]);
        assert_eq!(m["content"], "hi");
    }

    #[test]
    fn build_user_message_with_image() {
        let imgs = vec![MediaData {
            media_type: "image/png".into(),
            data: "abc".into(),
        }];
        let m = build_user_message("look", &imgs);
        assert_eq!(m["content"][0]["type"], "image_url");
        assert!(m["content"][0]["image_url"]["url"]
            .as_str()
            .unwrap()
            .contains("data:image/png;base64,abc"));
        assert_eq!(m["content"][1]["text"], "look");
    }

    #[test]
    fn build_user_message_with_audio_and_video() {
        let media = vec![
            MediaData {
                media_type: "audio/mpeg".into(),
                data: "audio-bytes".into(),
            },
            MediaData {
                media_type: "video/mp4".into(),
                data: "video-bytes".into(),
            },
        ];
        let message = build_user_message("analyze", &media);
        assert_eq!(message["content"][0]["type"], "input_audio");
        assert_eq!(message["content"][0]["input_audio"]["data"], "audio-bytes");
        assert_eq!(message["content"][1]["type"], "input_video");
        assert_eq!(message["content"][1]["input_video"]["data"], "video-bytes");
    }

    fn test_agent(client: Arc<dyn ChatClient>) -> (TempDir, Agent) {
        let tmp = TempDir::new().unwrap();
        let agent = Agent::for_test(
            client,
            History::new(tmp.path().join("history"), 30),
            Memory::new(tmp.path().join("memories")),
            Skills::new(tmp.path().join("skills.json")),
            Reminders::new(tmp.path().join("reminders.json")),
        );
        (tmp, agent)
    }

    #[tokio::test]
    async fn run_returns_plain_text_completion() {
        let client = Arc::new(MockChatClient::new());
        client.push_text("hello there");
        let (_t, agent) = test_agent(client);
        let result = agent
            .run(AgentRequest::text("u1", "Alice", "hi"), &NoHooks)
            .await;
        assert_eq!(result.text, "hello there");
    }

    #[tokio::test]
    async fn run_persists_history() {
        let client = Arc::new(MockChatClient::new());
        client.push_text("saved reply");
        let (_t, agent) = test_agent(client);
        agent
            .run(AgentRequest::text("u2", "Bob", "remember this"), &NoHooks)
            .await;
        let hist = agent.history.load("u2").await;
        assert_eq!(hist.len(), 2); // user + assistant
        assert_eq!(hist[0]["content"], "remember this");
    }

    #[tokio::test]
    async fn run_dispatches_translate_tool_then_answers() {
        let client = Arc::new(MockChatClient::new().with_once_reply("Bonjour"));
        // First completion asks for a translate tool call; second finishes with text.
        client.push_tool_call(
            "call_1",
            "translate",
            r#"{"text":"Hello","target_language":"French"}"#,
        );
        client.push_text("It means Bonjour.");
        let (_t, agent) = test_agent(client);
        let result = agent
            .run(
                AgentRequest::text("u3", "Cy", "translate Hello to French"),
                &NoHooks,
            )
            .await;
        assert_eq!(result.text, "It means Bonjour.");
        // History should contain the assistant tool-call turn and the tool result.
        let hist = agent.history.load("u3").await;
        assert!(hist
            .iter()
            .any(|m| m["role"] == "tool" && m["content"] == "Bonjour"));
    }

    #[tokio::test]
    async fn run_update_memory_tool_persists() {
        let client = Arc::new(MockChatClient::new());
        client.push_tool_call("c1", "update_memory", r#"{"memory_content":"Likes tea"}"#);
        client.push_text("Noted.");
        let (_t, agent) = test_agent(client);
        agent
            .run(
                AgentRequest::text("u4", "Dee", "remember I like tea"),
                &NoHooks,
            )
            .await;
        assert_eq!(agent.memory.load("u4").await, "Likes tea");
    }

    #[tokio::test]
    async fn dispatch_unknown_tool_returns_error() {
        let client = Arc::new(MockChatClient::new());
        let (_t, agent) = test_agent(client);
        let out = agent
            .dispatch_tool("run_unknown_code_agent", &json!({}), "u", &NoHooks)
            .await;
        match out {
            ToolOutcome::Text(t) => assert!(t.contains("Unknown tool")),
        }
    }

    #[tokio::test]
    async fn context_overflow_triggers_new_session() {
        let client = Arc::new(MockChatClient::new());
        client.push_text_with_usage(
            "ok",
            TokenUsage {
                prompt_tokens: 40,
                completion_tokens: 10,
                ..Default::default()
            },
        );
        client.push_text("ok again");
        let tmp = TempDir::new().unwrap();
        let mut agent = Agent::for_test(
            client,
            History::new(tmp.path().join("history"), 30),
            Memory::new(tmp.path().join("memories")),
            Skills::new(tmp.path().join("skills.json")),
            Reminders::new(tmp.path().join("reminders.json")),
        );
        agent.set_max_context_tokens(50);
        let big = "x".repeat(200);
        agent
            .history
            .save(
                "u5",
                &[
                    json!({"role": "user", "content": big.clone()}),
                    json!({"role": "assistant", "content": "ok"}),
                ],
            )
            .await
            .unwrap();

        agent
            .run(AgentRequest::text("u5", "Ed", "hi again"), &NoHooks)
            .await;
        agent
            .run(AgentRequest::text("u5", "Ed", "one more"), &NoHooks)
            .await;

        // The oversized message must have been summarized away; only the new turn remains.
        let hist = agent.history.load("u5").await;
        assert!(!hist
            .iter()
            .any(|m| m["content"].as_str() == Some(big.as_str())));
        assert_eq!(hist.last().unwrap()["content"], "ok again");
    }

    #[tokio::test]
    async fn compaction_records_summary_token_usage() {
        let usage = TokenUsage {
            prompt_tokens: 100,
            completion_tokens: 50,
            ..Default::default()
        };
        let client = Arc::new(
            MockChatClient::new()
                .with_once_reply("- Likes tea")
                .with_once_usage(usage),
        );
        let (_t, agent) = test_agent(client);
        agent
            .history
            .save(
                "u6",
                &[
                    json!({"role": "user", "content": "I like tea"}),
                    json!({"role": "assistant", "content": "Noted"}),
                ],
            )
            .await
            .unwrap();

        agent.compact_session("u6").await;

        let info = agent.session_info("u6").await;
        assert_eq!(info.context_tokens, 150);
        assert_eq!(info.requests, 1);
        assert_eq!(info.input_tokens, 100);
        assert_eq!(info.output_tokens, 50);
    }

    #[tokio::test]
    async fn build_tools_excludes_code_execution() {
        let client = Arc::new(MockChatClient::new());
        let (_t, agent) = test_agent(client);
        let tools = agent.build_tools().await;
        let names: Vec<&str> = tools
            .iter()
            .filter_map(|t| t["function"]["name"].as_str())
            .collect();
        assert!(!names.contains(&"code_tool"));
        assert!(names.contains(&"translate"));
        assert!(names.contains(&"update_memory"));
        assert!(names.contains(&"common_crawl__search"));
    }
}
