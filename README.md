# house-chatbot

A Discord-based house assistant bot powered by a local LLM (llama.cpp) with MCP server integration. **Written in prompts**, implemented in Rust, and connected to Discord with [serenity](https://github.com/serenity-rs/serenity).

## Features

- **LLM-powered chat** — per-user conversation history and persistent memory
- **Global token monitor** — PostgreSQL conversation archives and usage leaderboards
- **Tool permissions** — server votes can restrict individual users from specific agent tools
- **Web search** — SearXNG JSON API integration for live information retrieval
- **Adjustable thinking effort** — `/effort low|medium|high|xhigh|max` sets the model's reasoning budget (2k/4k/8k/16k/unlimited thinking tokens)
- **Jellyfin media server** — browse and query your media library via MCP
- **Built-in tools** — reminders, URL summarization, translation, and GitHub feature-request filing
- **Automated feature development** — owner-approved jobs can dispatch Codex, Claude Code, or OpenCode to open reviewable pull requests

## Quick start

```bash
cp .env.example .env          # fill in required values
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
| `SEARXNG_URL` | SearXNG instance for the `web_search` tool |
| `JELLYFIN_URL` + `JELLYFIN_API_KEY` | Enables Jellyfin MCP |
| `GITHUB_*` | GitHub App credentials for issue filing and coding-agent dispatch |
| `SENTRY_DSN` / `SENTRY_ENVIRONMENT` | Optional Sentry error reporting for the chatbot |

## Architecture

```
Discord message → HouseBot::message() → Agent::run()
  ├── LLM agentic loop with tool dispatch
  │   ├── update_memory → user memory (markdown)
  │   ├── set_reminder / summarize_url / translate / create_feature_request
  │   ├── web_search / fetch_webpage → SearXNG + guarded HTTP fetch
  │   └── MCP tools → jellyfin__* (stdio JSON-RPC)
  └── streamed response back to Discord
```

The crate is split into small, individually unit-tested modules:

```
src/
  main.rs            # entry point
  lib.rs             # module declarations
  bot.rs             # serenity client, routing, commands, redaction, code file uploads
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
  tools/             # searxng, web_fetch, common_crawl, remind, summarize_url, translate, feature_request
```

See [AGENTS.md](AGENTS.md) for detailed architecture and development guidance.

## Automated feature development

The configured Discord owner can ask the bot to implement a feature with an external coding agent. The bot drafts a specification, requires an explicit owner confirmation, and then creates a labeled GitHub issue for the selected Codex, Claude Code, or OpenCode runner. The runner works on an isolated branch and opens a pull request for human review; it never auto-merges or auto-deploys.

See [Automated development](docs/automated-development.md) for setup, permissions, runner requirements, state transitions, and the security model.

## License

AGPLv3 — see [LICENSE](LICENSE).
