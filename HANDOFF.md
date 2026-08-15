# Handoff — Housebot rebuild

Read this together with [`docs/REDESIGN_PLAN.md`](docs/REDESIGN_PLAN.md), which
is the authoritative scope document — this file covers state and gotchas, the
plan covers what to build.

**Branch:** `claude/bot-redesign-audit-remaining-9aj83g`. No PR opened.

## Where things stand

| Phase | State |
|---|---|
| Plan agreed and committed | done |
| Database purge migration | done |
| Phase 1 — demolition | done |
| Phase 2 — core | done |
| Phases 3–7 | not started |

Before starting, confirm the tree is green: `cargo test --workspace`,
`cargo clippy --all-targets -- -D warnings`, `cargo fmt --check`. All three
pass as of the last commit, so a later failure is yours.

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
- `ScheduledChatClient::with_priority` exists for Phase 3's sub-agent spawn
  tool — build the sub-agent's client with it so sub-agents actually schedule
  at `SubAgent`.

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

## The database is still nearly empty

`001_purge_all_data` drops the entire `public` schema. Only `user_memories` has
been recreated (`002`). **The bot will not start against a migrated database
yet** — `Agent::from_env` refuses to fall back for token monitoring and access
control, so those tables are load-bearing.

| Table | Used by | Migration |
|---|---|---|
| `user_memories` | `crates/memory` | `002` ✅ |
| `bot_config` | `crates/bot-config` | missing |
| `conversations`, `conversation_messages`, `token_usage_events` | `crates/token-monitor` | missing |
| `deployment_permissions` | `crates/deployment-bot` | missing |

Recreate the rest with their **new** shapes, numbered from `003`. Do not
restore the deleted SQL from git history — the schemas are meant to change.

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

**The workspace is not yet usable for real coding, by design — this is Phase 4
work.** `/workspace` is mounted `noexec`, so interpreted code runs but nothing
compiled does (no `./a.out`, no `cargo build && ./target/...`, no native node
modules). There is also no `write_file` method; the only way to create a file
today is a heredoc through `run`, capped at `MAX_COMMAND_LENGTH` (4096). Both
are on the Phase 4 checklist. Dropping `noexec` is deliberate loosening — keep
`nosuid`.

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

## Things that will surprise you

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
