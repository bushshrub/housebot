# house-chatbot — Agent Guide

A Discord-based house assistant bot, written in **Rust** with [serenity](https://github.com/serenity-rs/serenity). The LLM backend is a local llama.cpp server (OpenAI-compatible API). The bot maintains per-user conversation history and memory, searches the web through a SearXNG instance, and integrates MCP servers (Jellyfin) over stdio.

---

## Building and running

```bash
cargo build                                   # build
cargo test                                    # run unit tests
cargo clippy --all-targets -- -D warnings     # lint
cargo fmt --check                             # formatting

# Docker
cp .env.example .env
docker compose up -d
```

Logs: `docker compose logs -f house-chatbot`

## Docker publish pipeline

Pushes both Docker images to GHCR on push to `main`/`master` or on tags (`v*`):

- `ghcr.io/bushshrub/housebot:latest` (main bot — Rust binary)
- `ghcr.io/bushshrub/housebot/sandbox:latest` (code-inspection sandbox — Alpine, bash, git, python3, node, rust, ripgrep)

---

## Project layout

```
Cargo.toml
crates/
  deployment-bot/     # independent deployment controller crate and binary
  sandbox/            # housebot-sandbox crate + sandboxd binary
    src/
      lib.rs          # public re-exports (SandboxClient, Sandbox, NetworkAccess)
      client.rs       # SandboxClient / Sandbox — connects to sandboxd over Unix socket
      server.rs       # sandboxd daemon — Unix socket listener, Docker subprocess execution
      docker.rs       # builds all Docker CLI args (never derived from user input)
      protocol.rs     # typed JSON-newline request/response messages
      validation.rs   # URL, path, query, command, branch validation
      limits.rs       # compile-time constants (timeouts, output sizes, limits)
      bin/sandboxd.rs # sandboxd entry point
    docker/
      Dockerfile      # sandbox execution image (inert sleep process)
      entrypoint.sh
    tests/
      docker_tests.rs      # Docker arg construction tests
      integration_tests.rs # validation, arg, limits, and Docker lifecycle tests (#[ignore])
      protocol_tests.rs    # serialization/deserialization tests
src/
  main.rs            # entry point — inits tracing, calls bot::run()
  lib.rs             # module declarations
  bot.rs             # serenity Client + EventHandler, routing, !commands, redaction, uploads
  agent.rs           # agentic loop, MCP sessions, tool dispatch, AgentResult, session summary
  llm.rs             # ChatClient trait + OpenAiClient (streaming SSE)
  mcp.rs             # McpServer — stdio JSON-RPC client
  lua_engine.rs      # sandboxed Lua VM for /lua (time/memory limits, discord.* bridge)
  history.rs         # per-user conversation JSONL (data/history/<user_id>.jsonl)
  memory.rs          # per-user persistent markdown (data/memories/<user_id>.md)
  notes.rs           # per-user named notes (data/notes/<user_id>.json)
  skills.rs          # global custom skills (data/skills.json)
  reminders.rs       # timed reminders (data/reminders.json)
  github_issues.rs   # GitHub App JWT (RS256) auth + issue creation
  testing.rs         # MockChatClient / RecordingSink test doubles
  tools/
    searxng.rs       # web_search — SearXNG JSON API client
    web_fetch.rs     # fetch_webpage — SSRF-guarded page fetcher
    common_crawl.rs  # common_crawl__search
    remind.rs        # set_reminder
    summarize_url.rs # summarize_url
    translate.rs     # translate
    feature_request.rs # create_feature_request + per-user RateLimiter
    feature_development.rs # prepare_feature_development + owner auth + rate limit
    sandbox.rs         # LazySandbox + five tool definitions (owner-only)
  coding_agent/
    catalog.rs       # versioned agent/model/effort catalog (loaded from .github/agents/catalog.json)
    pending.rs       # PendingDevelopmentJob state machine (15-min expiry, atomic dispatch guard)
    issue.rs         # GitHub issue body builder + hidden metadata comment
.github/
  agents/
    catalog.json     # single source of truth for selectable agent/model/effort combos
    common.sh        # shared shell utilities for adapter scripts
    run-codex.sh     # Codex adapter
    run-claude.sh    # Claude Code adapter
    run-opencode.sh  # OpenCode + NVIDIA NIM adapter
  workflows/
    develop-feature.yml    # triggered by agent:queued label; runs the selected agent
    check-agent-runner.yml # daily runner health check
CLAUDE.md            # instructions for Claude Code when running as the automated agent
docs/
  automated-development.md # full dispatch flow documentation
sandbox/             # legacy coding-sandbox image (migrated to crates/sandbox/docker/; kept for reference)
data/                # runtime — gitignored
```

---

## Key environment variables

| Variable | Required | Default | Purpose |
|---|---|---|---|
| `DISCORD_BOT_TOKEN` | yes | — | Discord bot auth |
| `OWNER_DISCORD_ID` | no | `0` | Owner user ID |
| `DATABASE_URL` | no | `postgres://housebot:housebot@postgres/housebot` | PostgreSQL persistence for memory, archives, and token leaderboards |
| `DATABASE_CONNECT_MAX_ATTEMPTS` | no | `10` | Persistent token-monitor connection attempts at startup |
| `DATABASE_CONNECT_RETRY_SECS` | no | `2` | Delay between token-monitor connection attempts |
| `DATABASE_CONNECT_TIMEOUT_SECS` | no | `10` | Deadline for each token-monitor connection attempt |
| `LLM_BASE_URL` | yes | `http://server-slop:8080/v1` | OpenAI-compatible LLM endpoint |
| `LLM_MODEL` | yes | `gemma-4-12b-qat-q4kxl` | Model name |
| `LLM_API_KEY` | no | `not-required` | API key (llama.cpp ignores it) |
| `MAX_HISTORY_TURNS` | no | `30` | Conversation turn pairs kept |
| `MAX_CONTEXT_TOKENS` | no | `10000` | Fallback context window (tokens) when the LLM server's `/props` probe fails |
| `CONVERSATION_IDLE_TIMEOUT` | no | `300` | Seconds a channel conversation stays "active" |
| `CHAT_RATE_LIMIT_MAX` | no | `20` | Max chat messages per user per window |
| `CHAT_RATE_LIMIT_WINDOW_SECS` | no | `60` | Sliding window size for chat rate limiting (seconds) |
| `SEARXNG_URL` | no | `http://searxng:8080` | SearXNG instance for the `web_search` tool (JSON format must be enabled) |
| `SEARXNG_LANGUAGE` | no | — | Default search language (e.g. `en`) |
| `SEARXNG_SAFE_SEARCH` | no | moderate | `OFF` / moderate / `STRICT` |
| `JELLYFIN_URL` + `JELLYFIN_API_KEY` | no | — | Enables Jellyfin MCP server |
| `GITHUB_APP_ID` / `GITHUB_APP_PRIVATE_KEY` / `GITHUB_INSTALLATION_ID` / `GITHUB_REPO` | no | — | GitHub App creds for feature-request issue filing (all four required) |
| `OWNER_DISCORD_ID` | no | `0` | Discord user ID allowed to dispatch coding jobs; `0` disables dispatch |
| `SANDBOX_SOCKET_PATH` | no | `/run/housebot-sandbox/sandbox.sock` | Unix socket path for sandboxd |
| `HOUSEBOT_SANDBOX_RUNTIME` | no | `runsc` | Container runtime for sandboxd (gVisor); set to `runc` in dev/CI |

(`DOCKER_NETWORK` is read only by the independent `deployment-bot` crate, not the chatbot.)

---

## Architecture

### Request flow

```
Discord message
  └─ HouseBot::message()
       ├─ commands (/session, /storage, /data; !session / !storage / !skill / !stats)
       ├─ filter (DM / mention / reply-to-bot / active conversation)
       ├─ extract media attachments (base64)
       ├─ post "⚙️ Generating..." progress message
       └─ Agent::run()
            ├─ load user memory + history (auto-summarize on overflow)
            ├─ build system prompt
            └─ agentic loop
                 ├─ ChatClient::chat_stream (streams partial text to the progress msg,
                 │   reasoning budget from the user's /effort setting)
                 ├─ if tool_calls → dispatch_tool()
                 │    ├─ web_search → SearXNG / fetch_webpage → guarded HTTP fetch
                 │    ├─ update_memory → memory.save()
                 │    ├─ set_reminder / summarize_url / translate / create_feature_request / run_skill
                 │    ├─ prepare_feature_development → PendingJobStore (owner-only; returns DISPATCH_FLOW:<uuid>)
                 │    └─ prefix__tool → McpServer::call_tool()
                 └─ repeat until finish_reason == "stop"
```

### LLM client

`llm::ChatClient` is a trait with `chat_stream` (SSE streaming, forwards cumulative text to an
optional `TextSink`) and `chat_once` (non-streaming). `OpenAiClient` is the real implementation;
`testing::MockChatClient` scripts completions for unit tests, so the whole agent loop is testable
without a live model.

`llm::ThinkingMode` (low / medium / high / xhigh / max → 2k / 4k / 8k / 16k / unlimited thinking
tokens) is stored per user in `UserConfig`, changed with the `/effort` slash command, and sent to
the backend as an OpenRouter-style `reasoning` request field alongside a matching `max_tokens`
ceiling.

### Tool dispatch

`Agent::dispatch_tool` returns a `ToolOutcome` (plain text).
Built-in tool JSON definitions live in each `tools/*` module as `definition()`; the agent flattens
them into the OpenAI function-calling envelope alongside the tools discovered from MCP servers.

### Outbound responses

**Secret redaction:** all text sent to Discord passes through `SecretRedactor`, which scans the
environment at startup for variables whose name contains `token`, `key`, `secret`, `password`,
`dsn`, or `oauth` (value length ≥ 8) and replaces any matching value with `[REDACTED]`.

**Large code responses:** `extract_code_files` pulls fenced code blocks larger than 800 chars out
of the reply, infers an extension from the language tag, and uploads them as file attachments.

### MCP servers

`mcp::McpServer` speaks newline-delimited JSON-RPC 2.0 over stdio: it performs the `initialize`
handshake, lists tools, and calls them. Tool names are namespaced `{server}__{tool}` (e.g.
`jellyfin__get_movies`). A failed MCP startup is logged and skipped.

---

## Code inspection sandbox

Five owner-only tools (`sandbox_clone_repository`, `sandbox_list_files`,
`sandbox_search_code`, `sandbox_read_file`, `sandbox_run`) let the bot inspect
and run short commands in a temporary isolated container.

### Security boundary

```
Housebot  →  Unix socket  →  sandboxd  →  docker run --runtime=runsc  →  gVisor
```

- **Housebot never holds the Docker socket.**  Only `sandboxd` does.
- **gVisor (runsc)** runs each container with a userspace kernel that intercepts
  syscalls, preventing container escape without requiring hardware virtualization.
- Container is `--read-only`, `--cap-drop=ALL`, `--no-new-privileges`,
  `--user=sandbox`, with tmpfs mounts only on `/workspace`, `/tmp`,
  `/home/sandbox`.
- One sandbox per `Agent::run`; destroyed unconditionally when the response ends.

### sandboxd

`sandboxd` (from `crates/sandbox/src/bin/sandboxd.rs`) must run beside the
bot.  It listens on `SANDBOX_SOCKET_PATH` and accepts typed JSON requests.
The Docker socket is mounted only into `sandboxd`, not into Housebot.

If `sandboxd` is unreachable the bot starts normally; sandbox tools return an
error message rather than crashing.

### Sandbox tool checklist

When adding or changing a sandbox tool:

1. Define it in `src/tools/sandbox.rs` (`all_definitions()` + the `LazySandbox` method).
2. Add a dispatch arm in `src/agent/dispatch.rs` under the `name if name.starts_with("sandbox_")` block.
3. Keep the operation in `crates/sandbox/src/client.rs` / `server.rs`.
4. Update validation in `crates/sandbox/src/validation.rs` if new input types are introduced.
5. Add unit tests in `crates/sandbox/src/docker.rs` or `tests/` for argument construction.
6. Add a Docker lifecycle test (marked `#[ignore]`) in `tests/integration_tests.rs`.

### Running the Docker integration tests locally

```bash
# Build the sandbox image
docker build -t ghcr.io/bushshrub/housebot/sandbox:latest crates/sandbox/docker

# With gVisor (production-equivalent)
cargo test --package housebot-sandbox -- --include-ignored --test-threads=1

# Without gVisor (dev / CI)
HOUSEBOT_SANDBOX_RUNTIME=runc \
  cargo test --package housebot-sandbox -- --include-ignored --test-threads=1
```

---

## Automated coding-agent dispatch

The bot can dispatch automated feature-development jobs to a self-hosted runner.
See [`docs/automated-development.md`](docs/automated-development.md) for the full
specification, runner requirements, and security model.

### Key points for agents working on this repo

- **Owner-only.** Only the configured `OWNER_DISCORD_ID` can dispatch.  
  This is enforced in Rust (`src/tools/feature_development.rs`) — not in the system prompt.
- **Two-step flow.** The LLM calls `prepare_feature_development` to draft a spec, then the
  Discord owner selects agent/model/effort and explicitly confirms.  The LLM cannot dispatch
  unilaterally.
- **Catalog.** Agent, model, and effort combinations are defined in
  `.github/agents/catalog.json`.  Update `catalog_revision` whenever you add or remove entries.
- **Labels.** `agent:queued` → `agent:running` → `agent:completed` / `agent:no-changes` /
  `agent:failed`.  Do not add or remove these labels manually outside the workflow.
- **No force-push. No auto-merge. No auto-deploy.** All PRs opened by the agent require
  a human review and explicit merge.
- **Autonomous development workflow.** A user only needs to provide the task. For every
  development task, the agent must: fetch `origin`, fast-forward local `master` from
  `origin/master`, create a new task branch from that updated `master`, implement and validate the
  work there, commit and push it, then open a normal ready-for-review (non-draft) PR against
  `master`. Do not wait for intermediate confirmation unless new authority or a material scope
  decision is required. Never commit directly to `master`.
- **Post-PR follow-through.** After opening a PR, monitor it for review comments and requested
  changes. Address every actionable comment on the same branch, validate the fix, commit and push
  it, and resolve the corresponding review thread only after the fix is present. Continue until no
  actionable review feedback remains or human direction is required.
- **Issue linkage.** Before starting development, use issue numbers explicitly linked in the task.
  If none are supplied, search open repository issues for the task's feature, symptom, or affected
  component. Include every confirmed related issue in the PR body as `Closes #<number>`; do not
  invent issue links when the search finds no match.
- **Always commit automatically.** The coding agent must create a commit for every set of
  changes it produces before handing work back; do not leave implemented changes uncommitted.
- **Codex attribution.** Every commit created by the Codex coding agent must include a
  `Co-authored-by` trailer naming Codex and the model used for that run. Derive the model
  identifier from the current runtime/model identity at commit time; do not hardcode a model
  name in the instructions. For example, this run should use
  `Co-authored-by: codex (GPT-5.6-luna) <codex@openai.com>`. Never use a generic or omitted model name.
- **Agent-only attribution.** Commits created by a coding agent are attributed solely to that
  agent: the agent is the git author and committer, and any `Co-authored-by` trailer names the
  agent only. Never include the user who triggered the run — as author, committer, or
  co-author — since they did not write the changes.
- **Always commit as yourself.** The agent must set its own identity explicitly on every commit
  (e.g. `git -c user.name=<agent> -c user.email=<agent-email> commit …`) rather than inheriting
  the ambient git config, which typically names the repository owner.
- **Secrets on runner.** `NVIDIA_API_KEY` is the only secret injected at runtime.
  OAuth sessions for Codex and Claude are pre-configured on the runner and must never be
  read, printed, or uploaded by workflow steps.

### New tool checklist

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
- **Memory** (PostgreSQL `user_memories`): per-user markdown, rewritten in full on each `update_memory`.
- **Notes** (`data/notes/<user_id>.json`), **skills** (`data/skills.json`), **reminders** (`data/reminders.json`).

`data/` and the PostgreSQL volume are mounted in docker-compose so they survive restarts.

### Persistent-memory schema safety

- **SQL migrations required.** Every database schema change must be an ordered, append-only SQL
  migration in `db/migrations/`; never add, alter, or drop schema directly in application startup
  code. Add a regression test that covers the migration or the bug it fixes.
- Future changes must not destructively alter or replace the persistent-memory database schema.
- Any necessary schema change must include a clear, ordered, backward-compatible migration path
  that preserves all existing user memories across upgrades and rollbacks.
- Never silently reset, truncate, or invalidate stored memory as part of application startup or deployment.
