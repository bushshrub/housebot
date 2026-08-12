# Handoff — Housebot rebuild

Written at the end of the session that produced the redesign plan, the database
purge, and Phase 1. Read this together with
[`docs/REDESIGN_PLAN.md`](docs/REDESIGN_PLAN.md), which is the authoritative
scope document — this file covers state and gotchas, the plan covers what to
build.

**Branch:** `claude/bot-redesign-audit-7gecv3` (pushed, 3 commits). No PR opened.

## Where things stand

| Phase | State |
|---|---|
| Plan agreed and committed | done |
| Database purge migration | done |
| Phase 1 — demolition | done |
| Phases 2–7 | not started |

Phase 1 removed ~8,500 lines. The tree builds clean: `cargo test --workspace`
passes, `cargo clippy --all-targets -- -D warnings` is clean, `cargo fmt
--check` is clean. Verify this before starting, so you know a later failure is
yours.

## Blocking question for the user

**Should skill authoring be owner-gated, or open to any user the bot talks to?**

Asked twice, not yet answered. It is not needed until Phase 4, so Phases 2–3
can proceed without it. Do not guess — the bot writes skills to a persistent
volume and executes their scripts in the sandbox, so the blast radius of
"anyone can author" is real.

## The database is empty

`001_purge_all_data` drops the entire `public` schema. **Every table the
surviving code queries is gone**, and no migration recreates any of them yet.
The bot will not run against a migrated database until Phase 7 (or earlier, as
each phase adds its own migration).

Tables that need recreating, and who queries them:

| Table | Used by |
|---|---|
| `user_memories` | `crates/memory` |
| `bot_config` | `crates/bot-config` |
| `conversations`, `conversation_messages`, `token_usage_events` | `crates/token-monitor` |
| `deployment_permissions` | `crates/deployment-bot` |

Recreate these with their **new** shapes as each phase lands, numbered from
`002`. Do not restore the deleted SQL from git history — the schemas are meant
to change (memory gains its Postgres-native shape, config gains the scheduler
limits).

**The purge must stay at index 1 in the `MIGRATIONS` array.** A migration
inserted before it would be applied, dropped along with its ledger row, and
re-applied on the next run — an invisible loop that silently wipes the database
on every deploy. `crates/database` has a test asserting the index; if you see it
fail, that is why. Add new migrations *after* the purge, always.

## Things that will surprise you

**Memory is already Postgres-backed.** `crates/memory` has a working
`Backend::Postgres` storing markdown in `user_memories`, with a file fallback
and a one-time file→Postgres import. The Phase 2 item "memory: markdown in
Postgres" is therefore mostly *recreating the table*, not writing a new store.
Read `crates/memory/src/lib.rs` before assuming otherwise.

**The model default was deliberately not changed.** It is still
`gemma-4-12b-qat-q4kxl` in two places — `src/agent/mod.rs` (the `LLM_MODEL`
default) and `.env.example`. Phase 1 was kept strictly subtractive. Phase 2
switches both to `gemma-4-26b-a4b-qat`. Note `crates/deployment-bot` also
passes `LLM_MODEL` through to the deployed container.

**Two crates on the cut list are still present on purpose.** `llm-queue` and
`channel-log` are replaced in Phase 2 by `llm-scheduler` and `channel-context`,
not deleted in Phase 1 — deleting them first would have left every LLM call and
`get_messages` as a compile hole. Swap them in place, then delete.

**`rate-limit` and `bot-commands` are keepers, despite earlier drafts.**
`rate-limit` backs the surviving feature-request tools. `bot-commands` holds the
memory, skills, and stats command implementations; only its notes/grocery/
profile handlers were stripped. The plan's crate-layout section records both.

**The existing `llm-queue` has no priorities.** It is a flat `Semaphore` with a
hardcoded default of 4. The whole point of `llm-scheduler` is the priority
ordering (`UserChat` > `SubAgent` > `Background`) plus the separate sub-agent
cap. Do not try to retrofit `llm-queue`; the shape is wrong.

**Outbound attachments are gone.** Nothing can post a file from a tool result —
`download_file` and `run_lua` were the only producers, so the whole path went
with them. Inbound attachment handling (image/PDF → model input) and the
code-block upload path both still work. If Phase 4 skill scripts need to emit
files, re-add the path *with* its producer; the shape is in git history at
`f1e02dd`.

**Profile data now comes from Discord live.** Display name, nickname, and
avatar feed the system prompt but are no longer persisted — `message_flow.rs`
fetches them each turn. That is where they originated before being cached, so
prompt content is unchanged.

**One test lost a real assertion.** The batch tool-call test previously drove a
rate-limit condition through `run_lua`'s scriptable output. No surviving tool
can force that deterministically without network, so the test now only checks
that both calls in a batch are dispatched and recorded. If you add a tool with
controllable output, consider restoring the rate-limit half.

## Suggested order

Follow the plan's phases. Two notes on sequencing:

- Phase 2's scheduler is the piece most likely to shape everything after it
  (sub-agents in Phase 3, config commands in Phase 5). Build it first within
  the phase, not last.
- Phase 6 says "Codex/Claude Code backends removed" — `crates/coding-agent/src/
  catalog.rs` still has all three variants. That is untouched Phase 1 work
  deferred by design, not an oversight.

## House rules that bit me

From `CLAUDE.md`, worth repeating because they are enforced:

- No comments describing *what* code does — only non-obvious *why*.
- No dead code. Removing a feature tends to strand helpers and enum variants;
  clippy will catch them, but only with `--all-targets`.
- Do not open pull requests. Do not push to `main`. Do not force-push.
- `cargo fmt` and `cargo clippy --all-targets -- -D warnings` must both pass.

One earned lesson: when deleting a block of Rust by matching on text, it is
easy to swallow the *following* item too. That happened three times this
session (`respond()`, `set_dev_notify_channel`, and a `/server-config` arm) and
each time the compiler caught it, but only because the deleted thing was still
referenced. If you delete something unreferenced, nothing will tell you. Prefer
deleting by explicit start/end markers and re-reading the region after.
