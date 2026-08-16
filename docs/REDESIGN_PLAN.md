# Housebot Redesign Plan

Fresh rearchitecture. The bot is cut down to an agentic LLM chat core plus the
deployment bot and the OpenCode feature-development flow. This file is the
source of truth for what is in scope and what is done; update the checkboxes as
work lands.

Resuming in a new session? Start with [`HANDOFF.md`](../HANDOFF.md) — it covers
current state and gotchas. All seven phases have landed; what remains is the
first live startup and the items under "Left open".

## Decisions

| Question | Decision |
|---|---|
| Model | `gemma-4-26b-a4b-qat` |
| Skill storage | On disk, persistent volume owned by the bot |
| Memory format | Markdown, stored in PostgreSQL |
| Sub-agents | Yes, with admin-configurable concurrency |
| LLM concurrency | Admin-configurable max in-flight |
| Scheduling | Priority queue — user chat outranks sub-agents; excess is queued |
| Skill authoring | Open to any user the bot talks to, not owner-gated |
| Skill metadata | `SKILL.md` frontmatter keeps name, description, author, editors, and advisory `enabled_tools`. Version history, triggers, and examples are dropped |
| Skill descriptions | Never in the system prompt — user-authored text must not carry system authority. Surfaced via the `list_skills` tool result |
| Skill script network | None. Scripts run in the caller's session sandbox and never upgrade its network mode |
| Channel context | Bot stores every channel it can see; reads are gated on the requesting user's live Discord permissions |
| Context retention | Per-channel message cap and an age limit, whichever binds first |
| Context durability | RAM only — no durable transcript. Context dies with the process, by decision |
| Code execution | gVisor sandbox, retained for skill scripts |
| Sandbox lifetime | Per session, reaped after an admin-configurable idle timeout (default 5 min) |
| Feature dev | OpenCode only, full interactive Discord flow |
| Landing strategy | Rewrite in place on `claude/bot-redesign-audit-7gecv3` |
| Database | Purged on deploy — every legacy table dropped, schema rebuilt |

## Database reset

The new deployment starts on an empty database. `001_purge_all_data` drops the
whole `public` schema and recreates it, which also clears anything created
outside the migration ledger. The legacy migrations (`user_memories`,
`token_monitor`, `bot_config`, `deployment_permissions`) are deleted from the
repo rather than left in the ledger.

Two invariants hold this together, both covered by tests in `crates/database`:

- The purge is ordered immediately after the ledger bootstrap. Any migration
  placed before it would be dropped along with its ledger row and re-applied on
  the next run.
- The purge recreates `schema_migrations` and re-records the bootstrap itself,
  since the ledger lives in `public` and goes down with the schema. The runner
  records the purge straight after, so it executes exactly once.

New-schema migrations are numbered from `002` and land with the phase that needs
them. The surviving stores are recreated with their new shapes, not restored
from the old SQL:

| Table | Queried by | Recreated in |
|---|---|---|
| `user_memories` | `crates/memory` | Phase 2 ✅ |
| `bot_config` | `crates/bot-config` | Phase 5 ✅ |
| `conversations`, `token_usage_events` | `crates/token-monitor` | Phase 5 ✅ |
| `deployment_permissions` | `crates/deployment-bot` | Phase 6 ✅ |

Every surviving store now has a post-purge migration, asserted by
`every_store_the_bot_needs_has_a_migration` in `crates/database`.

## Scope

### Keep

- Agentic LLM loop — multi-step tool use, streaming, `gemma-4-26b-a4b-qat`
- Per-user conversation history
- Persistent memory (markdown in Postgres)
- SearXNG `web_search` + `fetch_webpage`
- Cancel reaction + progress indicator
- Image/PDF attachment handling
- `set_reminder` + DM delivery
- Skills, Claude-style: progressive disclosure, bundled scripts
- gVisor sandbox (`sandboxd`) as the skill-script execution surface
- Channel context — in-memory ring buffer, no disk logging
- Token leaderboards + `/stats`
- PostgreSQL + runtime config
- Deployment bot — `/deploy`, `/rollback`, `/commit`, `/update`, `/deployment-access`
- Feature-adding via OpenCode — full interactive flow, GitHub issue filing, auto-PR

### Cut

`deep_research` · `summarize_url` · `download_file` · `common_crawl` · Lua VM
(`lua-engine`, `graph-render`, `/lua`, `run_lua`, `get_lua_docs`) · MCP client +
Jellyfin · `notes` · `grocery` · `profile` · `message-log` + `/personalize` ·
tool permissions / `/tool_ban` / `/tool_restore` · proactive messaging ·
`translate` · `get_token_metrics` · `find_discord_users` · `get_discord_user` ·
disk-based channel logging · Codex and Claude Code dispatch backends

## Architecture

```
Discord message
  └── Bot handler
        ├── attachment decode (image/PDF → model input)
        ├── channel ring buffer (in-memory, RAM-only)
        └── Agent::run(priority = UserChat)
              ├── scheduler.acquire(priority)  ──┐
              ├── LLM streaming turn             │  priority queue
              ├── tool dispatch                  │  bounded in-flight
              │     ├── web_search / fetch_webpage
              │     ├── memory read/write (Postgres)
              │     ├── skills: list / load / run  → sandboxd → gVisor
              │     ├── set_reminder
              │     ├── feature request → GitHub → OpenCode
              │     └── spawn_subagent  ──────────┘ (priority = SubAgent)
              └── streamed response → Discord
```

### Scheduling

One shared scheduler owns both limits:

- `max_inflight_llm` — total concurrent LLM requests, admin-configurable
- `max_subagent_concurrency` — ceiling on sub-agent slots, admin-configurable

Priorities, highest first:

1. `UserChat` — a human is waiting on a Discord message
2. `SubAgent` — spawned by an agent turn
3. `Background` — reminders, maintenance

Requests above the in-flight limit queue rather than fail. Sub-agents can never
starve user chat: a waiting `UserChat` request takes the next free slot ahead of
any queued `SubAgent`, and sub-agents are additionally capped by their own
semaphore so a fan-out cannot consume the whole LLM budget.

### Crate layout

Surviving crates, reworked as needed:

```
config          bot-config      database        llm
llm-scheduler   (replaces llm-queue)
history         memory          channel-context (replaces channel-log)
skills          sandbox         tools           reminders
token-monitor   bot-formatting  bot-response    discord-bridge
coding-agent    github-issues   deployment-bot  testing
```

Deleted in Phase 1: `lua-engine`, `graph-render`, `mcp`, `notes`, `grocery`,
`profile`, `message-log`, `tool-permissions`, `common-crawl`, `bot-commands`.

Two crates are replaced in Phase 2 rather than deleted first, because removing
them up front would leave every LLM call and `get_messages` as a compile hole
for no benefit: `llm-queue` → `llm-scheduler`, `channel-log` →
`channel-context`.

`rate-limit` is **kept**, not cut: the surviving feature-request tools depend on
it. `bot-commands` is kept too — it holds the command implementations for
memory, skills, and stats, so only its notes/grocery/profile handlers were
stripped.

### Skills

Claude Skills model. Each skill is a directory on the persistent volume:

```
<SKILLS_DIR>/<skill-name>/
  SKILL.md          # YAML frontmatter: name, description; body = instructions
  references/       # loaded on demand, not up front
  scripts/          # executed in the gVisor sandbox
```

Progressive disclosure, three levels:

1. **Name only** of every enabled skill sits in the system prompt. Descriptions
   are user-authored and deliberately excluded — see the decisions table — and
   reach the model through the `list_skills` tool result instead
2. `SKILL.md` body is loaded when the agent invokes the skill
3. `references/` files and `scripts/` are read or run only when the body calls for them

Skills the bot authors itself are written to the same directory and persist
across restarts.

## Work plan

### Phase 1 — demolition ✅
- [x] Purge migration: drop the whole schema, delete legacy migrations
- [x] Delete ten cut crates and their workspace members
- [x] Delete cut tool modules from `crates/tools`
- [x] Delete cut slash commands, subcommands, and their handlers
- [x] Remove proactive messaging end to end (agent, config, commands, schema)
- [x] Remove the outbound tool-attachment path (its only producers were cut)
- [x] Drop the Jellyfin MCP build stage from the Dockerfile
- [x] Workspace compiles clean; `cargo test`, `clippy -D warnings`, `fmt` all pass

### Phase 2 — core ✅

Build the scheduler first: sub-agents (Phase 3) and the config commands
(Phase 5) both depend on its shape.

- [x] `llm-scheduler`: priority queue, configurable in-flight cap, sub-agent cap
- [x] Delete `llm-queue` once the scheduler replaces it
- [x] Switch the model default to `gemma-4-26b-a4b-qat` in `src/agent/mod.rs`
      and `.env.example`
- [x] Agent loop: multi-step tool dispatch, cancellation, progress — the loop
      in `src/agent/run.rs` already had all three and survived Phase 1 intact
- [x] Memory: migration recreating `user_memories` (`002`) — the Postgres
      backend in `crates/memory` already existed and already stored markdown
- [x] History: per-user conversation persistence — `crates/history` already
      does this as JSONL on the data volume; kept as is
- [x] `channel-context`: in-memory ring buffer + `get_messages`
- [x] Delete `channel-log` once the ring buffer replaces it

### Phase 3 — tools ✅
- [x] `web_search` (SearXNG) + `fetch_webpage` — both already existed in
      `crates/tools` and survived Phase 1 wired; the dead `deep_research` path
      beside them was removed
- [x] `set_reminder` + DM delivery — the tool, the JSON store, and the 30s
      delivery loop in `handler.rs` all already existed
- [x] Attachment handling (image/PDF) — `src/bot/media.rs` already decoded
      images, rendered PDFs to PNG pages, and converted GIFs to video
- [x] Sub-agent spawn tool, priority-aware — `spawn_subagent`, built on
      `ScheduledChatClient::with_priority(Priority::SubAgent)`

### Phase 4 — skills + sandbox ✅
- [x] Add `SKILLS_DIR` (approved) pointing at the persistent skills volume
- [x] Skill discovery, frontmatter parsing, progressive disclosure
- [x] Skill authoring tools writing to the persistent volume
- [x] `sandboxd` wired as the script execution surface
- [x] Skill scripts run sandboxed with bounded time and memory
- [x] Session-scoped sandboxes: keyed by user, reaped by an idle timer in
      `sandboxd` (`SANDBOX_IDLE_TIMEOUT_SECS`, default 300, admin-configurable)
      instead of destroyed at the end of every `Agent::run`
- [x] Make the workspace usable for coding: `noexec` dropped from the
      `/workspace` tmpfs, plus a `write_file` method

### Phase 5 — Discord surface ✅
- [x] Message handler, streaming render, cancel reaction, progress — all four
      survived Phase 1 intact; the audit found the stale surface *around* them
      (three cut commands still registered, a `/help` reference advertising a
      dozen removed features) rather than gaps
- [x] Runtime config commands, including scheduler limits — `/config scheduler
      show|max_inflight|max_subagent`, persisted in `bot_config` so a ceiling
      set during an incident survives a redeploy
- [x] `/stats` + token leaderboards — the leaderboard already existed; `/stats`
      now reports the caller's token totals
- [x] Migrations `003_create_bot_config` and `004_create_token_monitor`, which
      together make the bot bootable again after the purge

### Phase 6 — subsystems
- [x] Deployment bot verified against the new core; `deployment_permissions`
      migration. Its env allowlist had rotted in both directions and is now
      test-guarded against the bot's actual env surface
- [x] OpenCode feature-development flow; dropped the `Codex` and `ClaudeCode`
      variants from `crates/coding-agent/src/catalog.rs`

### Phase 7 — finish
- [x] Every surviving store has a post-purge migration (see the table above)
- [x] `HANDOFF.md` rewritten for what remains
- [x] `cargo test`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check`
- [x] README and `.env.example` updated

### Left open
- [ ] **The model and effort picker does not reach the runner.**
      `opencode-dispatch.yml` hardcodes `model: opencode/deepseek-v4-flash-free`
      and `variant: high`; the bot passes only `issue_number`, `prompt`, and
      `requester_id`. Whatever the user selects is recorded in the issue
      metadata and then ignored. Fixing it means declaring `model` and `effort`
      as workflow inputs, which `CLAUDE.md` forbids an automated run from doing.
- [ ] **Retired CI still present.** `.github/workflows/claude-dispatch.yml` and
      the `run-codex.sh` / `run-claude.sh` adapters outlived the backends they
      served. Nothing in Rust can reach them. Same reason they were left.
- [ ] **First live startup.** Nothing in this rebuild has run against real
      Postgres, Discord, an LLM server, or Docker.

## Future work

- **Embeddings.** An embedding model plus a vector index over the transcript,
  for the "what were people saying about X" queries regex loses. Needs a second
  resident model on the llama.cpp server, a chunking choice (per message
  retrieves poorly — Discord messages are short and context-dependent), and a
  vector store. Layer it over the durable transcript, not instead of it: the
  raw text is still needed to quote exactly and to re-embed after a model
  change — which, with no durable transcript, means the index can only ever
  cover what is still in the ring buffer and must be rebuilt from it on restart.
  Embed on ingest at `Background` priority so it cannot compete with user chat.
  The permission gate applies unchanged — filter candidates by channel access
  *before* returning them, never after ranking.
