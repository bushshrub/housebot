# house-chatbot — Agent Guide

A Discord-based house assistant bot. The LLM backend is a local llama.cpp server (OpenAI-compatible API). The bot maintains per-user conversation history and memory, integrates MCP servers for web search and Jellyfin, and can spin up ephemeral Docker sandboxes to run coding agents (OpenCode or Claude Code).

---

## Running the bot

```bash
# First-time setup
cp .env.example .env          # fill in required values
docker compose build sandbox  # build the sandbox image once

# Start
docker compose up -d
```

Logs: `docker compose logs -f house-chatbot`

---

## Project layout

```
main.py                   # entry point — loads .env, calls src.bot.run()
src/
  bot.py                  # Discord client (HouseBot), message routing, approval flow
  agent.py                # agentic loop, MCP sessions, tool dispatch, AgentResult
  history.py              # per-user conversation JSONL (data/history/<user_id>.jsonl)
  memory.py               # per-user persistent markdown (data/memories/<user_id>.md)
  github_issues.py        # GitHub App JWT auth + automatic error issue filing
  tools/
    opencode.py           # run_opencode — Docker sandbox, streams logs, returns artifacts
    claude_code.py        # run_claude_code — same sandbox, owner-approval required
sandbox/
  Dockerfile              # Node + claude-code + opencode + Rust toolchain
  entrypoint.sh           # dispatches AGENT=opencode|claude inside the container
data/
  history/                # runtime — gitignored
  memories/               # runtime — gitignored
  artifacts/              # sandbox zip outputs — gitignored
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
| `LLAMA_CPP_URL` / `LLAMA_CPP_MODEL` | no | — | Passed into sandbox for OpenCode |
| `GITHUB_APP_ID` | no | — | GitHub App ID for error auto-reporting |
| `GITHUB_APP_PRIVATE_KEY` | no | — | PEM key (escape newlines as `\n`) |
| `GITHUB_INSTALLATION_ID` | no | — | GitHub App installation ID |
| `GITHUB_REPO` | no | — | `owner/repo` to file issues against |

All four `GITHUB_*` vars must be set together; the reporter silently no-ops if any is missing.

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
- `{"content": str, "_artifact_paths": list[str]}` — zipped workspace files uploaded to Discord

The `_` keys are stripped from the message before it reaches the LLM.

### Sandbox execution

`run_opencode` / `run_claude_code` both call `_call_sandbox()` in `opencode.py`. Execution is synchronous Docker SDK work offloaded to a thread via `run_in_executor` so it doesn't block the async event loop. Log lines stream back to the Discord progress message in real time via `run_coroutine_threadsafe`.

Container limits: 2 CPUs (`cpu_quota=200000`), 1 GB RAM. After exit, the workspace is zipped and saved to `data/artifacts/` if under 24 MB.

### MCP servers

MCP servers are connected once at startup (`Agent.start()`) via stdio. Tool names are namespaced: `{server}__{tool}` (e.g. `ddg__search`, `jellyfin__get_movies`). A failed MCP startup is logged and skipped — the bot continues without that server.

### Error self-reporting

`GitHubIssueReporter` in `src/github_issues.py` uses GitHub App JWT auth (RS256) to obtain an installation token, then POSTs to the GitHub Issues API. Issues are deduplicated by a SHA-256 fingerprint of the exception type + last 3 traceback frames; the same error won't be re-filed within 1 hour. On a new issue, the owner is DMed the URL.

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
- **Artifacts** (`data/artifacts/sandbox-*.zip`): ephemeral — not cleaned up automatically beyond the 24 MB gate.

Both `data/` paths are volume-mounted in docker-compose so they survive container restarts.
