# Handoff — Housebot rebuild

Read this together with [`docs/REDESIGN_PLAN.md`](docs/REDESIGN_PLAN.md), which
is the authoritative scope document — this file covers state and gotchas, the
plan covers what to build.

**Branch:** `claude/bot-redesign-phase-five-jz9cc2`, which continues
`claude/bot-redesign-audit-remaining-9aj83g` (PR #320).

## Where things stand

| Phase | State |
|---|---|
| Plan agreed and committed | done |
| Database purge migration | done |
| Phase 1 — demolition | done |
| Phase 2 — core | done |
| Phase 3 — tools | done |
| Phase 4 — skills + sandbox | done |
| Phase 5 — Discord surface | done |
| Phases 6–7 | not started |

Before starting, confirm the tree is green: `cargo test --workspace`,
`cargo clippy --all-targets -- -D warnings`, `cargo fmt --check`. All three
pass as of the last commit, so a later failure is yours.

## Start here — Phase 6

Two items, and the second is bigger than it reads:

1. **Deployment bot.** `crates/deployment-bot` is a separate binary with its own
   container (`Dockerfile.deployment-bot`) and is not linked into `housebot` at
   all, so nothing in Phases 1–5 touched it. It needs the
   `deployment_permissions` migration (number it `005`, new shape, do not
   restore the old SQL) and a read-through against the new core.
2. **OpenCode flow**, plus the Phase 1 work deferred into this phase: drop the
   `Codex` and `ClaudeCode` variants from `crates/coding-agent/src/catalog.rs`.
   All three backends are still there.

The bot is **bootable** as of Phase 5 — `bot_config`, `conversations`, and
`token_usage_events` all exist now, and those were the three tables
`Agent::from_env` refused to start without. It has still never actually been
started; see "Things that will surprise you".

## What Phase 5 actually changed

A fourth consecutive audit. The Discord surface itself — streaming render,
cancel reaction, progress hooks, message flow — was intact from Phase 1 and
needed nothing. What was rotten was everything *describing* that surface.

### Three cut commands were still being registered

`/lua`, `/tool_ban`, and `/tool_restore` were deleted in Phase 1 but their
`CreateCommand` definitions survived in `command_defs.rs`, so the bot kept
publishing them to Discord with no handler behind them: a user clicking one got
silence. `/lua` was global (now in `RETIRED_SLASH_COMMANDS`, which the startup
sweep deletes); the other two were guild-scoped, and the guild registration is a
bulk `set_commands` overwrite, so dropping them from the list is enough.

**Registration is the same blind spot as the system prompt.** A `CreateCommand`
is data, not a call — deleting a handler leaves the registration compiling
happily. When you cut a command, grep `command_defs.rs` for it too.

### `/help` was lying, at length

`features_text()` in `crates/tools/src/features.rs` backs both `/help` **and**
the `get_bot_features` tool result, so its 60 lines of prose were being read by
users and by the model. It still advertised `!grocery`, `/lua`, `/data profile`,
`/storage notes`, Jellyfin, deep research, translation, downloads, and proactive
assistance — all cut in Phase 1. Rewritten to the surviving surface, and
`reference_does_not_advertise_removed_features` now guards it the way
`system_prompt_does_not_advertise_removed_tools` guards the prompt. **Add to
both when you cut something.**

`tool_status` in `crates/bot-formatting` had the same rot in icon form
(`jellyfin__`, `run_lua`, `deep_research`); it now covers the surviving tools
and gives `sandbox_*` its own icon.

### `/help` was also being truncated

It is ~3 KB and message content caps at 2000 characters, so the tail was cut off
in Discord and nothing said so. Slash-command replies longer than
`MAX_MESSAGE_LENGTH` are now sent as an embed description (4096), via
`truncate_reply`, which is `truncate_memory_reply` with the limit as a
parameter. A test asserts the reference still fits.

### Scheduler limits are configurable and persistent

`/config scheduler show|max_inflight|max_subagent` drives `set_max_inflight` /
`set_max_subagent`, the two methods Phase 2 left with no callers.

- The limits are stored in `bot_config` under `scheduler_limits`
  (`SchedulerLimitsStore`), and `Agent::from_env` seeds the scheduler from them,
  falling back to `MAX_INFLIGHT_LLM` / `MAX_SUBAGENT_CONCURRENCY`. **The env
  vars are startup defaults only** — once a configurer sets a ceiling, the
  stored value wins on every later boot.
- `LlmScheduler` **panics** on a zero ceiling. The bound is enforced three
  times over: Discord's `min_int_value`, the command handler, and a `.max(1)`
  when loading a stored record. Keep all three; only the last one covers a
  corrupt row.
- `Agent::from_env` now opens **one** bot-config Postgres client and shares it
  between `AccessControlStore` and `SchedulerLimitsStore`, so
  `AccessControlStore::from_env` was removed. The fail-closed behaviour is
  unchanged: no client, no start.

### `/stats` reports tokens

`TokenMonitor::get_user_stats` existed with no callers; `/stats` now shows the
caller's all-time totals through `Agent::user_token_summary`.
`get_global_stats` and `GlobalTokenStats` had no caller either and no place in
the plan, so they were deleted. That makes three phases in a row where a `pub
fn` in a library crate sat dead without a single warning — assume there is more.

### The two migrations

`003_create_bot_config` and `004_create_token_monitor`, written to match what
the code actually queries rather than the pre-purge schemas:

- `token_usage_events.conversation_id` is a foreign key **`ON DELETE
  CASCADE`**. `clear_user` only deletes from `conversations`; without the
  cascade, `/data erase` would leave a user's usage events behind and the
  windowed leaderboards would keep billing them.
- `conversations` has a partial index on `(user_id, started_at DESC) WHERE
  ended_at IS NULL`, which is exactly the resume-on-restart lookup.
- `every_store_the_bot_needs_has_a_migration` in `crates/database` asserts each
  required table is created by some migration. Add `deployment_permissions` to
  it when Phase 6 lands.

## What Phase 2 actually changed

Three of the seven Phase 2 items turned out to be **already built** and were
kept rather than rewritten. Read the code before assuming a gap:

- **The agent loop** (`src/agent/run.rs`) already did multi-step tool dispatch
  (bounded at `MAX_TOOL_ROUNDS = 16`), cancellation (a token checked before each
  LLM call and raced against the stream), and progress (`AgentHooks`).
- **Memory** was already Postgres-backed markdown in `crates/memory`; only the
  table needed recreating.
- **History** was already per-user JSONL on the data volume in `crates/history`.

The real work was the scheduler and the ring buffer.

## What Phase 3 actually changed

The same pattern, harder: **three of the four Phase 3 items were already
built and wired**, so the phase was mostly an audit. Read the code before
assuming a gap — this has now happened in two consecutive phases.

- **`web_search` + `fetch_webpage`** already existed in `crates/tools`
  (`searxng.rs`, `web_fetch.rs`), registered in `build_tools` and dispatched.
  `fetch_webpage` resolves DNS and rejects loopback/private ranges, so it is
  SSRF-guarded; keep that if you touch it.
- **`set_reminder`** already had all three pieces: the tool
  (`crates/tools/src/remind.rs`), the JSON-backed store (`crates/reminders`),
  and the delivery loop in `handler.rs`, which polls `pop_due` every 30s and
  DMs the user. The loop is spawned once, guarded by `reminder_started`.
- **Attachment handling** already lived in `src/bot/media.rs`: images inline
  as base64, PDFs rendered to PNG pages, GIFs converted to video (Gemma reads
  animation as video, not stills), all size-capped and URL-validated.

Only the sub-agent tool was genuinely missing.

### `spawn_subagent`

The definition is `crates/tools/src/subagent.rs`; the loop is
`src/agent/subagent.rs`, dispatched from `dispatch.rs`.

- Its client is `self.scheduled_client.with_priority(Priority::SubAgent)`.
  That is the whole point of the tool — build it any other way and sub-agents
  compete with user chat. There is a test (`subagent_runs_at_subagent_priority`)
  that watches `SchedulerInfo::subagent_active` from inside a probe client,
  since occupancy is the only externally visible proof of priority.
- **Recursion is structurally impossible, not depth-limited.** The sub-agent's
  tool surface is exactly `web_search` + `fetch_webpage` — `spawn_subagent` is
  not in it, so it cannot fan out. If you widen `subagent_tools`, keep it out,
  and keep memory/reminders/skills out too: the sub-agent has no user and must
  not mutate anything the parent owns.
- Bounded at `MAX_SUBAGENT_ROUNDS = 8`, half the parent's 16.
- Token usage is recorded against the **parent's** conversation, so the
  leaderboard bills the user who caused the spawn. `dispatch.rs` looks the
  conversation ID up via `current_conversation_id`, which hits the already
  populated in-memory map rather than the database.
- It takes no `CancelToken`. Cancelling the parent turn abandons the sub-agent
  in place rather than stopping it — acceptable at 8 rounds, but if sub-agents
  ever get longer, thread the token through.

### The system prompt was lying to the model

`STATIC_BASE` in `src/agent/prompt.rs` still advertised roughly a dozen tools
Phase 1 deleted — Jellyfin, `download_file`, `run_lua`/`get_lua_docs`,
`deep_research`, `translate`, `common_crawl__search`, `get_token_metrics`,
`find_discord_users`, `get_discord_user` — and the Guidelines told the model to
route complex questions to `deep_research`. Nothing catches this: the prompt is
a string constant, so the compiler and clippy are both blind to it. Four tests
actually *asserted* the dead tools were present and had to be replaced.

`system_prompt_does_not_advertise_removed_tools` now guards the list. **Add to
it whenever you cut a tool** — it is the only thing standing between a deleted
tool and a model that keeps trying to call it.

`deep_research` was the same story in code: cut from the plan in Phase 1, but
its dispatch arm, its SearXNG implementation, and a 100-line dossier formatter
all survived because they were `pub` and therefore invisible to dead-code
analysis. All removed. Expect more of this in `crates/tools` — `pub fn` in a
library crate is never reported as dead.

## What Phase 4 actually changed

This one was real work, not an audit. Skills went from a single JSON blob to
one directory per skill under `SKILLS_DIR`.

### Skills are directories now

`<SKILLS_DIR>/<name>/SKILL.md` — YAML frontmatter plus an instruction body —
beside optional `references/` and `scripts/`. Progressive disclosure is now
literal: names in the prompt, body on `use_skill`, bundled files listed by name
and opened only by `read_skill_file` / `run_skill_script`.

- **Frontmatter is parsed by a hand-written YAML subset** (`frontmatter.rs`),
  not a dependency: the workspace has no YAML crate and `serde_yaml` has been
  archived since 2024. It accepts only `key: scalar` and `key: [a, b]` and
  **rejects** anything else rather than half-understanding it. If you add a
  field, add it there and to `Skill::to_skill_md`, and keep the round-trip test
  green — a description containing a colon is the case that breaks naive
  emitters.
- **`validate_name` is security, not tidiness.** Skill names come from users and
  become directory names. Names are refused, never sanitised. `read_bundled`
  separately refuses separators and `..` in file names, since those also come
  from the model.
- **The version number was load-bearing** in a way that is easy to miss. It was
  optimistic concurrency: you had to pass the current version to update, which
  is what stopped `create_skill` silently clobbering an existing skill. Versions
  are gone, so an explicit `update: true` flag now carries that job. Do not drop
  it without putting something else in its place.
- One unreadable skill is skipped with a log rather than failing the whole scan.
- `skill_creator` is still a built-in, injected on every load and never written
  to disk.

### Sandbox: `write_file` and skill scripts

- `/workspace` no longer mounts `noexec` (`nosuid` stays; `/tmp` and
  `/home/sandbox` keep `noexec`). The workspace is the only executable path.
- **Nothing from the host is mounted into a sandbox** — every writable path is
  an in-memory tmpfs and the root filesystem is read-only. There is now a test
  asserting no `-v`/`--volume`/`--mount`/`--volumes-from` ever reaches the run
  arguments. Keep it that way; a bind mount here would be a bug, not a config
  choice.
- `write_file` feeds content to `tee` **on stdin** via `build_exec_argv`, which
  runs a program with no shell at all. File bodies therefore never touch argv or
  a command line. Use `build_exec_argv` for anything user-derived; only use
  `build_exec_args` (which goes through `bash -c`) for fixed commands.
- The write target is resolved with `realpath -m` and checked against
  `/workspace/` **before** writing, so a pre-existing symlink cannot redirect it.
- **Skill scripts get no network, ever.** `run_skill_script` attaches at
  `NetworkAccess::None`. A container's network mode is fixed at creation, so
  attaching with network here would silently upgrade the user's whole session
  sandbox. Scripts compute; the agent gathers data and passes it in as args.
- Supported types are `.py`, `.sh`, `.js`, checked before any sandbox work.

### `llm-scheduler` replaced `llm-queue`

`llm-queue` was a flat `Semaphore`. `llm-scheduler` is a `Mutex`-guarded state
with three FIFO waiter queues, because both limits must be adjustable at
runtime for the Phase 5 config commands — a semaphore can add permits but not
remove them.

Things worth knowing before you touch it:

- `pump()` **skips** a waiter it cannot admit instead of stopping at it.
  Without that, a `SubAgent` parked on its own cap head-of-line blocks every
  `Background` waiter behind it. There is a test for exactly this.
- A `Permit` releases on drop, and `pump` runs while holding the state lock, so
  a permit must never be dropped inside `pump` — `Permit::forget()` exists for
  the one path where a waiter is cancelled between the liveness check and the
  hand-off. Dropping it there deadlocks.
- Limits come from `MAX_INFLIGHT_LLM` and `MAX_SUBAGENT_CONCURRENCY`.
  `set_max_inflight` / `set_max_subagent` are already there for Phase 5's
  runtime config commands; nothing calls them yet.
- `ScheduledChatClient::with_priority` is now used by `spawn_subagent`
  (Phase 3). Any future background work — reminders, maintenance — should get
  its own client the same way, at `Background`.

### `channel-context` replaced `channel-log`

Now an in-memory ring buffer per channel, RAM-only and gone on restart, bounded
by whichever limit binds first: `CHANNEL_CONTEXT_CAPACITY` (default 2000) or
`CHANNEL_CONTEXT_RETENTION_SECS` (default 30 days). Expiry only ever removes a
prefix, since the buffer is chronological. It carries only `append`, `search`,
and `remove_user_entries` — `channel-log`'s `get_recent`, `find_authors`, and
all the fuzzy/levenshtein matching had **no callers outside their own tests**
(`get_messages`'s recent/before/after modes go to the Discord API), so they
were dropped rather than ported. Its operations are in-memory and therefore no
longer `async`.

## The database

`001_purge_all_data` drops the entire `public` schema; everything since is
recreated with a new shape.

| Table | Used by | Migration |
|---|---|---|
| `user_memories` | `crates/memory` | `002` ✅ |
| `bot_config` | `crates/bot-config` | `003` ✅ |
| `conversations`, `token_usage_events` | `crates/token-monitor` | `004` ✅ |
| `deployment_permissions` | `crates/deployment-bot` | missing (Phase 6) |

Recreate the last one with its **new** shape, numbered `005`. Do not restore
the deleted SQL from git history — the schemas are meant to change.

`deployment_permissions` does not block the bot: `crates/deployment-bot` is
**not linked into the `housebot` binary at all** — separate crate, separate
container (`Dockerfile.deployment-bot`).

No store creates its own tables; they all assume migrations have run.

**The purge must stay at index 1 in the `MIGRATIONS` array.** A migration
inserted before it would be applied, dropped along with its ledger row, and
re-applied on the next run — an invisible loop that wipes the database on every
deploy. `crates/database` has a test asserting the index.

## Sandboxes are session-scoped now

A sandbox used to be created per `Agent::run` and destroyed when the response
ended, so a follow-up message got an empty container. Containers are now keyed
by **user ID**, live in `sandboxd`'s map, and are destroyed by a reaper task
after `SANDBOX_IDLE_TIMEOUT_SECS` (default 300).

- `LazySandbox` is a per-turn *handle*; it no longer owns the lifetime, and
  `run.rs` no longer closes it.
- Everything in a container is tmpfs, so reaping genuinely discards the
  workspace. That is the agreed behaviour — the workspace is free and not
  meant to be durable.
- A container's network mode is fixed at creation. `reuse_session` refuses a
  request needing the internet against a network-less session sandbox rather
  than silently downgrading it. If a session needs network, it has to start
  that way.

The workspace is usable for coding as of Phase 4: `noexec` is gone from
`/workspace` and `write_file` exists (capped at `MAX_WRITE_FILE_BYTES`, 256
KiB). Creating a file no longer means a heredoc through `run` bounded by
`MAX_COMMAND_LENGTH` (4096).

## Channel reads are permission-gated

`get_messages` used to take a `channel_id` straight from the model with no
check at all, while `handler.rs` buffered every guild message the bot could
see. A user in one channel could have the bot regex-search a channel they
cannot open. Both Discord fetch paths have the same shape — they run with the
*bot's* permissions, not the caller's.

`Agent::authorize_channel_read` now gates every mode of `get_messages`:

- Reading the channel the conversation is in needs no check — the user is
  demonstrably there. Everything else is verified.
- Verification is `DiscordBridge::can_view_channel`, which resolves the user's
  **current** roles and the channel's overwrites live. Do not be tempted to
  record access at ingest time: a user who loses a role must lose the history
  that came with it.
- It **fails closed**. An unreachable bridge, an unparseable user ID, a channel
  in another guild, a non-member — all refuse.
- It costs three HTTP calls per cross-channel read. If that becomes a rate-limit
  problem, cache with a short TTL; do not cache indefinitely.

`channel-context` itself performs no access control and its doc comment says so.
Keep it that way — one gate, in the dispatcher, is easier to audit than a gate
per store. If you add another reader over the buffer, it needs the same gate.

## Decisions that are settled

Do not re-ask these; they are in the plan's decisions table.

- **Skill authoring is open to any user**, not owner-gated. The gVisor sandbox
  is therefore the only barrier between an authored script and the host — treat
  its limits as load-bearing.
- **Sandbox session key is the user ID**, and reaping destroys the workspace.
- **Channel context stores every channel the bot can see**, and filters per
  requesting user at query time — rather than only storing @everyone-visible
  channels. That makes the permission gate load-bearing security, not a
  convenience. Denying the bot `VIEW_CHANNEL` in Discord remains the way to
  keep a channel out of the store entirely.
- **Cross-channel reads are allowed**, for any channel the requesting user can
  read.
- **Retention is both a count cap and an age limit**, whichever binds first.
- **No durable transcript.** Channel context stays RAM-only and dies with the
  process. This was proposed and declined — do not re-propose a
  `channel_messages` table.
- **Skill descriptions never enter the system prompt.** The plan's
  progressive-disclosure level 1 says "name + description"; that was decided
  against. Descriptions are user-authored, authoring is open to everyone, and
  the system prompt is the one place text carries the bot's own authority — so
  a description there is a prompt-injection surface. The prompt lists **names
  only**; descriptions reach the model through the `list_skills` *tool result*,
  where they are data the bot read rather than orders it believes. This is
  already how the code behaves; keep it that way. There is a test asserting a
  skill's description does not appear in the prompt.
- **`enabled_tools` is advisory, never enforced.** It is shown as recommended
  tools when a skill loads; it does not narrow the agent's tool surface.
  Enforcing it would rebuild the tool-permission system Phase 1 deliberately
  cut.
- **Skill scripts get no network.** They run in the caller's existing session
  sandbox and must never trigger a network upgrade — a container's network mode
  is fixed at creation and `reuse_session` refuses to change it. Scripts
  compute; the agent gathers data with `web_search` / `fetch_webpage` and passes
  it in.
- **No skills migration.** There is no live `skills.json` worth preserving, so
  the directory-based store starts empty. Do not write a migration path.
- **No conversation archive either.** `token-monitor` used to mirror every
  message of every turn into `conversation_messages` — user text, assistant
  replies, and all tool traffic. Nothing ever read it: no command, no
  leaderboard, no context rebuild. The working history the bot actually uses is
  `crates/history` (per-user JSONL on the data volume). The table and its
  `record_turn` write path were removed rather than recreated post-purge. It
  was the only record that outlived compaction, and that was decided against
  deliberately — do not reintroduce it. `token-monitor` now does exactly one
  job: counting tokens.

## Things that will surprise you

**Nothing in this rebuild has run against live infrastructure.** Every phase so
far was verified by `cargo test`, clippy, and reading code — no session has
started the bot, reached a real Postgres, a Discord gateway, an LLM server, or a
Docker daemon. "Done" in the table above means compiled, wired, and unit-tested.
Phase 5 removed the last thing *blocking* a first startup; it did not perform
one. The migrations in particular have never touched a real Postgres.
Two areas are most exposed, because they are new code rather than surviving
code: `write_file` / `run_skill_script` (which need a live `sandboxd` and gVisor
to prove out at all) and the directory-based skills store. Treat first startup
as a debugging session, not a formality.

**Two crates on the Phase 1 cut list were replaced, not deleted** — that work is
done: `llm-queue` → `llm-scheduler` and `channel-log` → `channel-context` both
landed in Phase 2.

**`rate-limit` and `bot-commands` are keepers**, despite earlier drafts.
`rate-limit` backs the surviving feature-request tools. `bot-commands` holds the
memory, skills, and stats command implementations; only its notes/grocery/
profile handlers were stripped.

**Outbound attachments are gone.** Nothing can post a file from a tool result —
`download_file` and `run_lua` were the only producers, so the whole path went
with them. Inbound attachment handling and the code-block upload path still
work. If Phase 4 skill scripts need to emit files, re-add the path *with* its
producer; the shape is in git history at `f1e02dd`.

**Profile data comes from Discord live.** Display name, nickname, and avatar
feed the system prompt but are no longer persisted — `message_flow.rs` fetches
them each turn.

**One test lost a real assertion in Phase 1.** The batch tool-call test
previously drove a rate-limit condition through `run_lua`'s scriptable output.
No surviving tool can force that deterministically without network, so the test
now only checks that both calls in a batch are dispatched and recorded.

**Phase 6 is still owed Phase 1 work.** `crates/coding-agent/src/catalog.rs`
still has all three backends; dropping `Codex` and `ClaudeCode` was deferred by
design, not overlooked.

**`/personalize` survives**, despite the plan's cut list pairing it with
`message-log`. What was cut was the profile-learning behind it; the command
itself still carries personality, follow-up, and progress toggles that have no
other home. Left alone deliberately in Phase 5 — raise it before removing it.

**`tokio::fs::File` does not flush on drop.** A merge-audit record was being
lost this way, which made two tests flaky under load. The codebase's correct
pattern is an explicit `file.flush().await` after `write_all`. If you add a
file-append path, flush it.

## House rules that bite

From `CLAUDE.md`, worth repeating because they are enforced:

- No comments describing *what* code does — only non-obvious *why*.
- No dead code. Removing a feature strands helpers and enum variants; clippy
  catches them, but only with `--all-targets`.
- Do not open pull requests. Do not push to `main`. Do not force-push.
- `cargo fmt` and `cargo clippy --all-targets -- -D warnings` must both pass.

One earned lesson from Phase 1: deleting a block of Rust by matching on text
easily swallows the *following* item. It happened three times and the compiler
caught it each time only because the deleted thing was still referenced. If you
delete something unreferenced, nothing will tell you. Prefer explicit
start/end markers and re-read the region after.
