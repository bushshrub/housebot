# house-chatbot

A Discord-based house assistant bot powered by a local LLM (llama.cpp) with MCP server integration and ephemeral Docker sandboxes for running coding agents. Written in **Rust** using [serenity](https://github.com/serenity-rs/serenity) for the Discord connector.

## Features

- **LLM-powered chat** — per-user conversation history and persistent memory
- **Web search** — DuckDuckGo MCP integration for live information retrieval
- **Jellyfin media server** — browse and query your media library via MCP
- **Coding sandboxes** — ephemeral Docker containers running OpenCode for automated software engineering tasks
- **Built-in tools** — reminders, URL summarization, translation, and GitHub feature-request filing

## Quick start

```bash
cp .env.example .env          # fill in required values
docker compose build sandbox  # build the sandbox image locally
docker compose up -d          # start the bot
```

## Development

```bash
cargo build            # build
cargo test             # run the unit tests
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

## Configuration

See `.env.example` for all available options. Key variables:

| Variable | Purpose |
|---|---|
| `DISCORD_BOT_TOKEN` | Discord bot auth |
| `OWNER_DISCORD_ID` | Owner user ID |
| `LLM_BASE_URL` / `LLM_MODEL` | OpenAI-compatible LLM endpoint (llama.cpp) |
| `JELLYFIN_URL` + `JELLYFIN_API_KEY` | Enables Jellyfin MCP |
| `GITHUB_*` | GitHub App credentials for issue filing |

## Architecture

```
Discord message → HouseBot::message() → Agent::run()
  ├── LLM agentic loop with tool dispatch
  │   ├── run_opencode → Docker sandbox (docker run)
  │   ├── update_memory → user memory (markdown)
  │   ├── set_reminder / summarize_url / translate / create_feature_request
  │   └── MCP tools → ddg__*, jellyfin__* (stdio JSON-RPC)
  └── streamed response back to Discord
```

The crate is split into small, individually unit-tested modules:

```
src/
  main.rs            # entry point
  lib.rs             # module declarations
  bot.rs             # serenity client, routing, commands, redaction, code/artifact uploads
  agent.rs           # agentic loop, prompt building, tool dispatch, session summarization
  llm.rs             # OpenAI-compatible streaming chat client (ChatClient trait)
  mcp.rs             # stdio MCP JSON-RPC client
  history.rs         # per-user conversation JSONL
  memory.rs          # per-user persistent markdown
  notes.rs           # per-user named notes
  skills.rs          # global custom skills
  reminders.rs       # timed reminders
  github_issues.rs   # GitHub App JWT auth + issue creation
  testing.rs         # shared test doubles (MockChatClient)
  tools/             # run_opencode, remind, summarize_url, translate, feature_request
```

See [AGENTS.md](AGENTS.md) for detailed architecture and development guidance.

## License

AGPLv3 — see [LICENSE](LICENSE).
