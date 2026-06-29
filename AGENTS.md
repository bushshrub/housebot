# house-chatbot — Agent Guide

A Discord-based house assistant bot. The LLM backend is a local llama.cpp server (OpenAI-compatible API). The bot maintains per-user conversation history and memory, integrates MCP servers for web search and Jellyfin, and can spin up ephemeral Docker sandboxes to run coding agents (OpenCode or Claude Code).

---

## Running the bot

```bash
# First-time setup
cp .env.example .env          # fill in required values
docker compose build sandbox  # build the sandbox image locally

# Start
docker compose up -d
```

Logs: `docker compose logs -f house-chatbot`

## Docker publish pipeline

Pushes both Docker images to GitHub Container Registry (GHCR) on push to `main`/`master` or on tags (`v*`).

Images published:
- `ghcr.io/bushshrub/housebot:latest` (main bot)
- `ghcr.io/bushshrub/housebot/sandbox:latest` (coding sandbox)

Each push also gets a `sha-<commit>` tag. Tags get an exact version tag (e.g. `v1.0.0`).

To use a published image instead of building locally, set in `.env`:
```
SANDBOX_IMAGE=ghcr.io/bushshrub/housebot/sandbox:latest
```

Trigger manually: `Actions` tab → `Build and publish Docker images` → `Run workflow`.

---

## Project layout

```
main.py                   # entry point — loads .env, calls src.bot.run()
src/
  bot.py                  # Discord client (HouseBot), message routing, approval flow
  agent.py                # agentic loop, MCP sessions, tool dispatch, AgentResult
  history.py              # per-user conversation JSONL (data/history/<user_id>.jsonl)
  memory.py               # per-user persistent markdown (data/memories/<user_id>.md)
  github_issues.py        # GitHub App JWT auth + Sentry-backed error issue filing
  tools/
    opencode.py           # run_opencode — Docker sandbox, streams logs, returns artifacts
    claude_code.py        # run_claude_code — same sandbox, owner-approval required
sandbox/
  Dockerfile              # Node + claude-code + opencode + Rust toolchain
  entrypoint.sh           # dispatches AGENT=opencode|claude inside the container
data/
  history/                # runtime — gitignored
  memories/               # runtime — gitignored
  artifacts/              # individual workspace files from sandbox — gitignored
  workspaces/             # ephemeral sandbox working dirs (created & deleted per run)
```

---

## Key environment variables

| Variable | Required | Default | Purpose |
|---|---|---|---|
| `DISCORD_BOT_TOKEN` | yes | — | Discord bot auth |
| `OWNER_DISCORD_ID` | yes | `0` | Owner user ID; gates `run_claude_code` approval |
| `LLM_BASE_URL` | yes | `http://server-slop:8080/v1` | OpenAI-compatible LLM endpoint |
| `LLM_MODEL` | yes | `gemma-4-12b-qat-q4kxl` | Model name |
| `JELLYFIN_URL` + `JELLYFIN_API_KEY` | no | — | Enables Jellyfin MCP server |
| `CC_OAUTH_TOKEN` | no | — | Claude Code OAuth token |
| `SANDBOX_IMAGE` | no | `house-chatbot-sandbox:latest` | Docker image for coding sandboxes |
| `DOCKER_NETWORK` | no | `house-chatbot_default` | Network sandboxes join |
| `SANDBOX_TIMEOUT` | no | `300` | Sandbox execution timeout (seconds) |
| `HOST_DATA_DIR` | yes* | — | Absolute host path to `./data`; required so sibling sandbox containers can share the workspace volume |
| `LLAMA_CPP_URL` / `LLAMA_CPP_MODEL` | no | — | Passed into sandbox for OpenCode |
| `SENTRY_DSN` | yes | — | Sentry DSN for error tracking |
| `GITHUB_APP_ID` | no | — | GitHub App ID for issue filing |
| `GITHUB_APP_PRIVATE_KEY` | no | — | PEM key (escape newlines as `\n`) |
| `GITHUB_INSTALLATION_ID` | no | — | GitHub App installation ID |
| `GITHUB_REPO` | no | — | `owner/repo` to file issues against |

All four `GITHUB_*` vars must be set together; the reporter silently no-ops if any is missing.

`HOST_DATA_DIR` must match the host-side absolute path of the `./data` volume mount (e.g. `/home/user/housebot/data`). Without it, sandbox workspace files won't be visible to the bot after the container exits.

---

## Architecture

### Request flow

```
Discord message
  └─ HouseBot.on_message()
       ├─ filter (DM / mention / name / active conversation)
       ├─ extract images (base64)
       ├─ show "⚙️ Working..." progress message
       └─ Agent.run()
            ├─ load user memory + history
            ├─ build system prompt
            └─ agentic loop
                 ├─ LLM call (OpenAI-compatible)
                 ├─ if tool_calls → _execute_tools() → _dispatch_tool()
                 │    ├─ run_opencode / run_claude_code → Docker sandbox
                 │    ├─ update_memory → memory.save()
                 │    └─ ddg__* / jellyfin__* → MCP session.call_tool()
                 └─ repeat until finish_reason == "stop"
```

### Tool result protocol

`_dispatch_tool` returns either a plain string or a dict with special sideband keys:

- `{"content": str, "_memory_update": str}` — triggers a memory write before the next LLM call
- `{"content": str, "_artifact_paths": list[str]}` — individual workspace files uploaded to Discord as attachments

The `_` keys are stripped from the message before it reaches the LLM.

### Sandbox execution

`run_opencode` / `run_claude_code` both call `_call_sandbox()` in `opencode.py`. Execution is synchronous Docker SDK work offloaded to a thread via `run_in_executor` so it doesn't block the async event loop. Log lines stream back to the Discord progress message in real time via `run_coroutine_threadsafe`.

Container limits: 2 CPUs (`cpu_quota=200000`), 1 GB RAM.

**Workspace sharing:** The sandbox mounts a directory from `data/workspaces/<uid>/` (host path via `HOST_DATA_DIR`) as `/workspace`. Because the sandbox is a Docker sibling (not a child), volume paths must be resolvable by the Docker daemon on the host — a plain `tempfile.TemporaryDirectory()` inside the bot container would not be visible. After the sandbox exits, individual files (excluding `opencode.json` and dotfiles) are copied into `data/artifacts/` and uploaded to Discord. Files over 24 MB are skipped. The workspace dir is always cleaned up on exit.

**Secret redaction:** All text sent to Discord — LLM responses, inline code blocks, and file contents — is passed through `_redact_secrets()` before delivery. This scans `os.environ` at startup for any variable whose name contains `token`, `key`, `secret`, `password`, `dsn`, or `oauth`, and replaces any matching value in outbound text with `[REDACTED]`.

**Large code responses:** If the LLM's final reply contains a fenced code block larger than 800 characters, `_extract_code_files()` pulls it out, infers a file extension from the language specifier (e.g. ` ```python` → `.py`), and uploads it as a Discord file attachment instead of pasting it inline. Unclosed code blocks (LLM hit token limit) are handled via `(?:```|$)` in the regex.

### MCP servers

MCP servers are connected once at startup (`Agent.start()`) via stdio. Tool names are namespaced: `{server}__{tool}` (e.g. `ddg__search`, `jellyfin__get_movies`). A failed MCP startup is logged and skipped — the bot continues without that server.

### Error reporting

Errors are captured by **Sentry** via `sentry_sdk.capture_exception()`. The Sentry event ID is then used to create a GitHub issue via `GitHubIssueReporter.create_error_issue()` — the issue body contains only the Sentry event ID and no sensitive data. The owner is DMed the issue URL on creation.

`GitHubIssueReporter` in `src/github_issues.py` uses GitHub App JWT auth (RS256) to obtain an installation token, then POSTs to the GitHub Issues API.

---

## Adding a new tool

1. Define `TOOL_DEFINITION` (name, description, `input_schema`) in `src/tools/your_tool.py`
2. Implement the async function
3. Import and register in `agent.py`:
   - Add to `_build_tools()`: `tools.append(_to_openai_tool(**_flatten_tool(YOUR_TOOL)))`
   - Add a branch in `_dispatch_tool()`
4. Document the tool in the system prompt in `_build_system_prompt()`

## Adding a new MCP server

Add an entry to `_mcp_server_configs()` in `agent.py`:
```python
configs.append(("prefix", StdioServerParameters(command="mcp-server-binary", env={...})))
```
Tools from that server will appear as `prefix__tool_name` automatically.

---

## Data

- **History** (`data/history/<user_id>.jsonl`): one JSON object per turn, trimmed to `MAX_HISTORY_TURNS` (default 30) pairs. Format is raw OpenAI message dicts.
- **Memory** (`data/memories/<user_id>.md`): free-form markdown, rewritten in full on each `update_memory` call.
- **Artifacts** (`data/artifacts/<uid>_<filename>`): individual files copied out of the sandbox workspace after each run. Not cleaned up automatically beyond the 24 MB per-file gate. `opencode.json` and dotfiles are excluded.

Both `data/` paths are volume-mounted in docker-compose so they survive container restarts.
