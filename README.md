# house-chatbot

A Discord-based house assistant bot powered by a local LLM (llama.cpp). **Written in prompts**, implemented in Rust, and connected to Discord with [serenity](https://github.com/serenity-rs/serenity).

## Features

- **LLM-powered chat** — per-user conversation history and persistent memory
- **Skills** — user-authored `SKILL.md` directories on a persistent volume, disclosed progressively: names in the prompt, instructions on `use_skill`, bundled files opened only on request
- **Sub-agents** — `spawn_subagent` researches in the background at a lower scheduling priority than user chat
- **Priority scheduling** — user chat outranks sub-agents; both ceilings are adjustable at runtime with `/config scheduler`
- **Multi-tier token leaderboards** — durable PostgreSQL daily, weekly, monthly, and all-time rankings that survive restarts, with cache-efficiency metrics and administrator-controlled visibility
- **Web search** — SearXNG JSON API integration for live information retrieval
- **Channel context** — an in-memory ring buffer per channel, gated on the requesting user's live Discord permissions
- **Adjustable thinking effort** — `/effort low|medium|high|xhigh|max` sets the model's reasoning budget (2k/4k/8k/16k/unlimited thinking tokens)
- **Built-in tools** — reminders, web fetch, and GitHub feature-request filing
- **Attachments and cancellation** — images and PDFs are read inline; an ❌ reaction stops an in-flight response
- **Automated feature development** — owner-approved jobs can dispatch OpenCode to open reviewable pull requests
- **Code inspection sandbox** — tools for cloning public repos, browsing files, searching code, reading files, and running short commands inside an isolated gVisor container

> **Rebuild in progress.** The bot is being rearchitected; see
> [`docs/REDESIGN_PLAN.md`](docs/REDESIGN_PLAN.md) for the target scope and
> what has landed so far.

## Quick start

```bash
cp .env.example .env          # fill in required values
docker compose up -d          # start postgres and the deployment bot
```

Compose runs postgres and the deployment bot only. The deployment bot owns the
`house-chatbot` and `housebot-sandboxd` containers: it deploys the latest commit
when nothing is running one, and replaces them on `/deploy`, `/update`, and
`/rollback`.

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
| `GITHUB_*` | GitHub App credentials for issue filing and coding-agent dispatch |
| `SENTRY_DSN` / `SENTRY_ENVIRONMENT` | Optional Sentry error reporting for the chatbot |
| `SANDBOX_SOCKET_PATH` | Unix socket path for sandboxd (default `/run/housebot-sandbox/sandbox.sock`) |
| `SANDBOX_IDLE_TIMEOUT_SECS` | Idle time before a user's sandbox is reaped (default 300) |
| `HOUSEBOT_SANDBOX_RUNTIME` | Override container runtime (default `runsc`; set to `runc` for dev/CI) |
| `SKILLS_DIR` | Skill directories on the data volume (default `<DATA_DIR>/skills`) |
| `MAX_INFLIGHT_LLM` / `MAX_SUBAGENT_CONCURRENCY` | Scheduler ceilings — **startup defaults only**, overridden once `/config scheduler` stores a value |
| `CHANNEL_CONTEXT_CAPACITY` / `CHANNEL_CONTEXT_RETENTION_SECS` | Ring-buffer bounds, whichever binds first |

## Architecture

```
Discord message → HouseBot::message() → Agent::run()
  ├── LLM agentic loop with tool dispatch
  │   ├── update_memory → user memory (markdown)
  │   ├── set_reminder / create_feature_request
  │   ├── web_search / fetch_webpage → SearXNG + guarded HTTP fetch
  │   └── sandbox_* → sandboxd (Unix socket) → gVisor container
  └── streamed response back to Discord
```

## Code inspection sandbox

The five `sandbox_*` tools let the bot clone a public repository,
browse its files, and run short commands for diagnostic purposes. Skill scripts
run here too. They request a networkless container, but the session's network
mode is fixed by whichever tool starts it first — a script invoked after a
repository clone shares that networked container. The sandbox itself is the
boundary: gVisor, tmpfs-only writable paths, no host mounts, no secrets.

Containers are **session-scoped, keyed by user**: a follow-up message reuses the
same workspace, and a reaper destroys it after `SANDBOX_IDLE_TIMEOUT_SECS` of
inactivity. Every writable path is tmpfs and nothing from the host is mounted,
so reaping genuinely discards the workspace — it is scratch space, not storage.
A container's network mode is fixed when it is created; a request needing the
internet is refused against a network-less session rather than silently
upgrading it.

### Security model

```
Housebot  →  typed request  →  sandboxd  →  docker run --runtime=runsc  →  gVisor
```

- **Housebot never holds the Docker socket** — only `sandboxd` does.
- **gVisor (runsc)** runs each container with a userspace kernel that intercepts
  syscalls, preventing container escape without requiring hardware virtualization.
- The container is read-only, non-root, cap-dropped, network-isolated by
  default, and destroyed once its session goes idle.

### Host requirements for the sandbox

- gVisor installed and registered as the `runsc` Docker runtime
  in `/etc/docker/daemon.json`
- For manual or Compose deployments, the `sandboxd` binary running beside
  Housebot with Docker socket access

The bot starts and operates normally when `sandboxd` is unavailable; only the
sandbox tools return an error.

Deployment-bot-managed production deployments already satisfy this requirement:
the deployment bot pulls and starts the single `sandboxd` sidecar, creates a
named volume for its Unix socket, and mounts only that socket volume into
Housebot. Do not add a duplicate Compose service. Disposable code containers
continue to run with `HOUSEBOT_SANDBOX_RUNTIME=runsc` by default.

The binary is a thin shell over a workspace of individually unit-tested crates:

```
src/
  main.rs      # entry point
  bot/         # serenity client, routing, commands, streaming render, attachments
  agent/       # agentic loop, prompt building, tool dispatch, sub-agents

crates/
  llm, llm-scheduler        # streaming chat client; priority scheduler over it
  history, memory           # per-user JSONL transcript; persistent markdown
  skills                    # SKILL.md directories on the data volume
  sandbox                   # sandboxd daemon + client (gVisor containers)
  tools                     # searxng, web_fetch, remind, skills, github, sandbox
  channel-context           # in-memory per-channel ring buffer
  token-monitor, database   # usage accounting; ordered migrations
  coding-agent              # OpenCode dispatch catalog and pending jobs
  deployment-bot            # separate binary: /deploy, /rollback, /update
```

See [AGENTS.md](AGENTS.md) for detailed architecture and development guidance.

## Automated feature development

The configured Discord owner can ask the bot to implement a feature with an external coding agent. The bot drafts a specification, requires an explicit owner confirmation, and then creates a labeled GitHub issue for the OpenCode runner. The runner works on an isolated branch and opens a pull request for human review; it never auto-merges or auto-deploys.

See [Automated development](docs/automated-development.md) for setup, permissions, runner requirements, state transitions, and the security model.

## License

AGPLv3 — see [LICENSE](LICENSE).
