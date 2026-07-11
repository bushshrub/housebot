# house-chatbot — Agent Guide

A Discord-based house assistant bot, written in **Rust** with [serenity](https://github.com/serenity-rs/serenity). The LLM backend is a local llama.cpp server (OpenAI-compatible API). The bot maintains per-user conversation history and memory, integrates MCP servers for web search and Jellyfin over stdio, and spins up ephemeral Docker sandboxes to run coding agents (OpenCode).

---

## Building and running

```bash
cargo build                                   # build
cargo test                                    # run unit tests
cargo clippy --all-targets -- -D warnings     # lint
cargo fmt --check                             # formatting

# Docker
cp .env.example .env
docker compose build sandbox
docker compose up -d
```

Logs: `docker compose logs -f house-chatbot`

## Docker publish pipeline

Pushes both Docker images to GHCR on push to `main`/`master` or on tags (`v*`):

- `ghcr.io/bushshrub/housebot:latest` (main bot — Rust binary)
- `ghcr.io/bushshrub/housebot/sandbox:latest` (coding sandbox — Node + opencode)

---

## Project layout

```
Cargo.toml
src/
  main.rs            # entry point — inits tracing, calls bot::run()
  lib.rs             # module declarations
  bot.rs             # serenity Client + EventHandler, routing, !commands, redaction, uploads
  agent.rs           # agentic loop, MCP sessions, tool dispatch, AgentResult, session summary
  llm.rs             # ChatClient trait + OpenAiClient (streaming SSE)
  mcp.rs             # McpServer — stdio JSON-RPC client
  history.rs         # per-user conversation JSONL (data/history/<user_id>.jsonl)
  memory.rs          # per-user persistent markdown (data/memories/<user_id>.md)
  notes.rs           # per-user named notes (data/notes/<user_id>.json)
  skills.rs          # global custom skills (data/skills.json)
  reminders.rs       # timed reminders (data/reminders.json)
  github_issues.rs   # GitHub App JWT (RS256) auth + issue creation
  testing.rs         # MockChatClient / RecordingSink test doubles
  tools/
    opencode.rs      # run_opencode — Docker sandbox via `docker run`, streams logs, artifacts
    remind.rs        # set_reminder
    summarize_url.rs # summarize_url
    translate.rs     # translate
    feature_request.rs # create_feature_request + per-user RateLimiter
sandbox/
  Dockerfile         # Node + opencode + Rust toolchain
  entrypoint.sh      # dispatches AGENT=opencode inside the container
data/                # runtime — gitignored
```

---

## Key environment variables

| Variable | Required | Default | Purpose |
|---|---|---|---|
| `DISCORD_BOT_TOKEN` | yes | — | Discord bot auth |
| `OWNER_DISCORD_ID` | no | `0` | Owner user ID |
| `LLM_BASE_URL` | yes | `http://server-slop:8080/v1` | OpenAI-compatible LLM endpoint |
| `LLM_MODEL` | yes | `gemma-4-12b-qat-q4kxl` | Model name |
| `LLM_API_KEY` | no | `not-required` | API key (llama.cpp ignores it) |
| `MAX_HISTORY_TURNS` | no | `30` | Conversation turn pairs kept |
| `MAX_CONTEXT_CHARS` | no | `40000` | Char budget before auto-summarizing a session |
| `CONVERSATION_IDLE_TIMEOUT` | no | `300` | Seconds a channel conversation stays "active" |
| `JELLYFIN_URL` + `JELLYFIN_API_KEY` | no | — | Enables Jellyfin MCP server |
| `SANDBOX_IMAGE` | no | `house-chatbot-sandbox:latest` | Docker image for coding sandboxes |
| `DOCKER_NETWORK` | no | `house-chatbot_default` | Network sandboxes join |
| `SANDBOX_TIMEOUT` | no | `300` | Sandbox execution timeout (seconds) |
| `HOST_DATA_DIR` | no | — | Optional host path to `./data`; omit it for fully ephemeral bot and sandbox state |
| `LLAMA_CPP_URL` / `LLAMA_CPP_MODEL` | no | — | Passed into the sandbox for OpenCode |
| `GITHUB_APP_ID` / `GITHUB_APP_PRIVATE_KEY` / `GITHUB_INSTALLATION_ID` / `GITHUB_REPO` | no | — | GitHub App creds for feature-request issue filing (all four required) |

When set, `HOST_DATA_DIR` must match the host-side absolute path of the `./data` volume mount (e.g. `/home/user/housebot/data`). If omitted, bot state and sandbox artifacts are ephemeral.

---

## Architecture

### Request flow

```
Discord message
  └─ HouseBot::message()
       ├─ !commands (!reset / !skill / !note / !stats)
       ├─ filter (DM / mention / reply-to-bot / active conversation)
       ├─ extract images (base64)
       ├─ post "⚙️ Generating..." progress message
       └─ Agent::run()
            ├─ load user memory + history (auto-summarize on overflow)
            ├─ build system prompt
            └─ agentic loop
                 ├─ ChatClient::chat_stream (streams partial text to the progress msg)
                 ├─ if tool_calls → dispatch_tool()
                 │    ├─ run_opencode → Docker sandbox
                 │    ├─ update_memory → memory.save()
                 │    ├─ set_reminder / summarize_url / translate / create_feature_request / run_skill
                 │    └─ prefix__tool → McpServer::call_tool()
                 └─ repeat until finish_reason == "stop"
```

### LLM client

`llm::ChatClient` is a trait with `chat_stream` (SSE streaming, forwards cumulative text to an
optional `TextSink`) and `chat_once` (non-streaming). `OpenAiClient` is the real implementation;
`testing::MockChatClient` scripts completions for unit tests, so the whole agent loop is testable
without a live model.

### Tool dispatch

`Agent::dispatch_tool` returns a `ToolOutcome` (plain text, or text plus collected artifact paths).
Built-in tool JSON definitions live in each `tools/*` module as `definition()`; the agent flattens
them into the OpenAI function-calling envelope alongside the tools discovered from MCP servers.

### Sandbox execution

`run_opencode` shells out to `docker run` (the bot container mounts `/var/run/docker.sock`).
Merged stdout/stderr stream back to the Discord progress message line by line. After the container
exits, individual workspace files (excluding `opencode.json` and dotfiles, and files over
`MAX_ARTIFACT_SIZE_MB`) are copied into `data/artifacts/` and uploaded to Discord.

**Workspace sharing:** the sandbox is a Docker *sibling*, so the workspace is bind-mounted from a
host-visible path under `HOST_DATA_DIR`.

**Secret redaction:** all text sent to Discord passes through `SecretRedactor`, which scans the
environment at startup for variables whose name contains `token`, `key`, `secret`, `password`,
`dsn`, or `oauth` (value length ≥ 8) and replaces any matching value with `[REDACTED]`.

**Large code responses:** `extract_code_files` pulls fenced code blocks larger than 800 chars out
of the reply, infers an extension from the language tag, and uploads them as file attachments.

### MCP servers

`mcp::McpServer` speaks newline-delimited JSON-RPC 2.0 over stdio: it performs the `initialize`
handshake, lists tools, and calls them. Tool names are namespaced `{server}__{tool}` (e.g.
`ddg__search`, `jellyfin__get_movies`). A failed MCP startup is logged and skipped.

---

## Adding a new tool

1. Add `definition()` and the async implementation in `src/tools/your_tool.rs`; register the module in `src/tools/mod.rs`.
2. Push `definition()` into `Agent::build_tools`.
3. Add a match arm in `Agent::dispatch_tool`.
4. Mention the tool in `build_system_prompt`.

## Adding a new MCP server

Add an entry in `agent::start_mcp_servers`:
```rust
if let Some(s) = McpServer::start("prefix", "mcp-binary", &args, &env).await {
    servers.push(s);
}
```
Its tools appear as `prefix__tool_name` automatically.

---

## Data

- **History** (`data/history/<user_id>.jsonl`): one JSON message per line, trimmed to `MAX_HISTORY_TURNS` pairs.
- **Memory** (`data/memories/<user_id>.md`): free-form markdown, rewritten in full on each `update_memory`.
- **Notes** (`data/notes/<user_id>.json`), **skills** (`data/skills.json`), **reminders** (`data/reminders.json`).
- **Artifacts** (`data/artifacts/<uid>_<filename>`): files copied out of the sandbox workspace.

`data/` is volume-mounted in docker-compose so it survives restarts.
