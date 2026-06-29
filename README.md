# house-chatbot

A Discord-based house assistant bot powered by a local LLM (llama.cpp) with MCP server integration and ephemeral Docker sandboxes for running coding agents.

## Features

- **LLM-powered chat** — per-user conversation history and persistent memory
- **Web search** — DuckDuckGo MCP integration for live information retrieval
- **Jellyfin media server** — browse and query your media library via MCP
- **Coding sandboxes** — ephemeral Docker containers running OpenCode or Claude Code for automated software engineering tasks
- **Error self-reporting** — automatically files GitHub issues on unhandled exceptions

## Quick start

```bash
cp .env.example .env          # fill in required values
docker compose build sandbox  # build the sandbox image locally
docker compose up -d          # start the bot
```

## Configuration

See `.env.example` for all available options. Key variables:

| Variable | Purpose |
|---|---|
| `DISCORD_BOT_TOKEN` | Discord bot auth |
| `OWNER_DISCORD_ID` | Owner user ID (required for `run_claude_code` approval) |
| `LLM_BASE_URL` | OpenAI-compatible LLM endpoint (llama.cpp) |
| `JELLYFIN_URL` + `JELLYFIN_API_KEY` | Enables Jellyfin MCP |
| `CC_OAUTH_TOKEN` | Claude Code OAuth token |
| `GITHUB_*` | GitHub App credentials for error issue filing |

## Architecture

```
Discord message → HouseBot.on_message() → Agent.run()
  ├── LLM agentic loop with tool dispatch
  │   ├── run_opencode / run_claude_code → Docker sandbox
  │   ├── update_memory → user memory (markdown)
  │   └── MCP tools → ddg__search, jellyfin__*
  └── Response back to Discord
```

See [AGENTS.md](AGENTS.md) for detailed architecture and development guidance.

## License

AGPLv3 — see [LICENSE](LICENSE).
