# Handoff — Housebot rebuild

Read this together with [`docs/REDESIGN_PLAN.md`](docs/REDESIGN_PLAN.md), which
is the authoritative scope document — this file covers state and gotchas, the
plan covers what was built.

All seven phases have landed. The per-phase narrative that used to live here is
in the commit history; what is kept below is the part that is still load-bearing
for whoever touches this next.

## Where the work is

**Branch: `claude/phase-6-continuation-pg9rzq`.** It carries the entire rebuild —
phases 1 through 7, 22 commits — and is the only place any of it exists.

- **Nothing is merged.** `master` has none of the rebuild. Do not branch new work
  from `master` expecting to find it.
- The branch was rebased onto the phase 5 tip
  (`claude/bot-redesign-phase-five-jz9cc2`), so its history is continuous with
  the earlier phase branches rather than a parallel line. The older branches —
  `claude/bot-redesign-audit-7gecv3`, `-audit-remaining-9aj83g` (PR #320,
  "Phase 1–4 complete"), `-audit-continue-cinqt6`, `-phase-five-jz9cc2` — are
  all ancestors or subsets of it. Ignore them; they are stale by definition now.
- **No PR is open for this branch.** PR #320 covers phases 1–4 only and is
  behind. Opening one is a human decision — `CLAUDE.md` forbids automated runs
  from doing it.

Continue on this branch rather than cutting a new one. If you must, branch from
its tip, never from `master`.

Before starting, confirm the tree is green: `cargo test --workspace`
(618 passing), `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check`.
All three pass as of the last commit, so a later failure is yours.

## Start here

**Nothing in this rebuild has ever run against live infrastructure.** No session
has started the bot, reached a real Postgres, a Discord gateway, an LLM server,
or a Docker daemon. "Done" means compiled, wired, and unit-tested. The
migrations in particular have never touched a real Postgres. Treat first startup
as a debugging session, not a formality.

Two areas are most exposed, because they are new code rather than surviving
code: `write_file` / `run_skill_script` (which need a live `sandboxd` and gVisor
to prove out at all) and the directory-based skills store.

### Three things are knowingly unfinished

1. **The model/effort picker does not reach the runner.** This is the one with
   user-visible impact, and the best first task. The interactive flow offers six
   OpenCode models and several effort levels; the user's choice is written into
   the issue metadata and then ignored.
   `.github/workflows/opencode-dispatch.yml:51` hardcodes
   `model: opencode/deepseek-v4-flash-free` and `variant: high`, while the bot
   dispatches only `issue_number`, `prompt`, and `requester_id`.

   Closing it means declaring `model` and `effort` as `workflow_dispatch` inputs
   and forwarding them from the two places the inputs map is built —
   `develop_on_confirm` (`src/bot/develop_actions.rs:88`) and `develop_on_approve`
   (`:218`). Both already hold a `ValidatedAgentSelection`, so the values are to
   hand; note that `trigger_workflow_dispatch` sends every input as a **string**
   because the API returns 422 for non-string values even on `type: number`
   inputs.

   Model is the easy half. **Effort is not**: all three OpenCode effort levels
   declare `mechanism: execution_budget`, meaning the CLI has no native control
   and effort is supposed to be expressed as timeout/turn/prompt bounds. It does
   not map onto the action's `variant:` key, which is a different axis — decide
   what `execution_budget` should actually do here before wiring it, or forward
   model only and drop the effort step from the picker.

   `CLAUDE.md` forbids an automated run from editing the workflow, so the YAML
   half needs a human or an explicit exemption.
2. **Retired CI outlived its backends.** `claude-dispatch.yml`, `run-codex.sh`,
   and `run-claude.sh` are unreachable from Rust now that `CodingAgent` has one
   variant. They were left because `CLAUDE.md` forbids an automated run from
   modifying CI; delete them by hand.
3. **The agent-selection stage is a one-item menu.** `DispatchStage::ChoosingAgent`
   survives with OpenCode as its only option. Collapsing the stage means
   changing the initial stage in `feature_development.rs`, the back-button
   target in `develop_component.rs`, and `pending.rs`. Not done, because it is
   a flow refactor rather than a backend cut.

## The database

`001_purge_all_data` drops the entire `public` schema; everything since is
recreated with a new shape.

| Table | Used by | Migration |
|---|---|---|
| `user_memories` | `crates/memory` | `002` |
| `bot_config` | `crates/bot-config` | `003` |
| `conversations`, `token_usage_events` | `crates/token-monitor` | `004` |
| `deployment_permissions` | `crates/deployment-bot` | `005` |

`every_store_the_bot_needs_has_a_migration` asserts each one is created by some
migration. Add to it when you add a store.

- **The purge must stay at index 1 in the `MIGRATIONS` array.** A migration
  inserted before it would be applied, dropped along with its ledger row, and
  re-applied on the next run — an invisible loop that wipes the database on
  every deploy. There is a test asserting the index.
- `token_usage_events.conversation_id` is a foreign key **`ON DELETE CASCADE`**.
  `clear_user` only deletes from `conversations`; without the cascade, `/data
  erase` would leave a user's usage events behind and the windowed leaderboards
  would keep billing them.
- `deployment_permissions` uses BIGINT IDs, unlike the TEXT the chatbot stores,
  because `permissions.rs` converts to i64 and refuses anything that does not
  fit. It holds only delegated grants — the owner is authorized without a row.
- No store creates its own tables; they all assume migrations have run.

## Things that rot silently

This rebuild found the same class of bug in five consecutive phases: **data that
describes behaviour, which no compiler checks.** Each now has a test, and each
test needs updating when you cut something.

| What rotted | Guard |
|---|---|
| System prompt advertising deleted tools | `system_prompt_does_not_advertise_removed_tools` |
| `/help` + `get_bot_features` advertising cut features | `reference_does_not_advertise_removed_features` |
| Slash commands registered with no handler | grep `command_defs.rs` when you cut a command |
| Deployment env allowlist missing new vars, keeping dead ones | `housebot_env_vars_cover_every_variable_the_bot_reads` and `housebot_env_vars_are_all_still_read_somewhere` |
| A retired agent left in `catalog.json` | `retired_agents_are_rejected_by_the_catalog` |

The env-allowlist pair is worth understanding before you trust it: it scans the
bot's own sources for env reads and compares against `HOUSEBOT_ENV_VARS`. The
"missing" direction is filtered to known prefixes, so a variable with a brand-new
prefix still slips through — extend the filter when you add one. `NOT_FORWARDED`
lists the three that are deliberately excluded, with reasons.

Related: **`pub fn` in a library crate is never reported as dead.** Three phases
running turned up public functions with no callers anywhere —
`get_global_stats`, `deep_research`'s whole implementation, half of
`channel-log`. Clippy will not tell you. Grep for callers before assuming
something is live.

## Decisions that are settled

Do not re-ask these; they are in the plan's decisions table.

- **Skill authoring is open to any user**, not owner-gated. The gVisor sandbox
  is therefore the only barrier between an authored script and the host — treat
  its limits as load-bearing.
- **Sandbox session key is the user ID**, and reaping destroys the workspace.
- **Channel context stores every channel the bot can see**, and filters per
  requesting user at query time. That makes the permission gate load-bearing
  security, not a convenience. Denying the bot `VIEW_CHANNEL` in Discord remains
  the way to keep a channel out of the store entirely.
- **Cross-channel reads are allowed**, for any channel the requesting user can
  read.
- **No durable transcript.** Channel context stays RAM-only and dies with the
  process. This was proposed and declined — do not re-propose a
  `channel_messages` table, and do not reintroduce the `conversation_messages`
  archive that `token-monitor` used to write and nothing ever read.
- **Skill descriptions never enter the system prompt.** Descriptions are
  user-authored and authoring is open to everyone, so a description in the
  prompt is a prompt-injection surface — it would carry the bot's own authority.
  The prompt lists **names only**; descriptions reach the model through the
  `list_skills` *tool result*, where they are data the bot read rather than
  orders it believes. There is a test asserting this.
- **`enabled_tools` is advisory, never enforced.** Enforcing it would rebuild
  the tool-permission system Phase 1 deliberately cut.
- **Skill scripts get no network.** They run in the caller's existing session
  sandbox and must never trigger a network upgrade — a container's network mode
  is fixed at creation and `reuse_session` refuses to change it. Scripts
  compute; the agent gathers data and passes it in.
- **No skills migration.** The directory-based store starts empty.

## Security invariants

Each of these is a deliberate property with a test behind it. Breaking one is a
bug, not a config choice.

- **Nothing from the host is mounted into a sandbox.** Every writable path is an
  in-memory tmpfs and the root filesystem is read-only. A test asserts no
  `-v`/`--volume`/`--mount`/`--volumes-from` ever reaches the run arguments.
- **`/workspace` is the only executable path** (`nosuid` stays; `/tmp` and
  `/home/sandbox` keep `noexec`).
- **`write_file` feeds content to `tee` on stdin** via `build_exec_argv`, which
  runs a program with no shell at all, so file bodies never touch argv. Use
  `build_exec_argv` for anything user-derived; `build_exec_args` goes through
  `bash -c` and is only for fixed commands. The write target is resolved with
  `realpath -m` and checked against `/workspace/` **before** writing, so a
  pre-existing symlink cannot redirect it.
- **`validate_name` is security, not tidiness.** Skill names come from users and
  become directory names. Names are refused, never sanitised. `read_bundled`
  separately refuses separators and `..` in file names.
- **`Agent::authorize_channel_read` gates every mode of `get_messages`**, and
  **fails closed** — unreachable bridge, unparseable user ID, channel in another
  guild, non-member all refuse. Verification resolves the user's *current* roles
  live; do not record access at ingest time, because a user who loses a role
  must lose the history that came with it. It costs three HTTP calls per
  cross-channel read; if that becomes a rate-limit problem, cache with a short
  TTL, never indefinitely. `channel-context` itself performs no access control
  and its doc comment says so — one gate, in the dispatcher, is easier to audit
  than a gate per store.
- **`fetch_webpage` resolves DNS and rejects loopback/private ranges.** Keep it.
- **Sub-agent recursion is structurally impossible, not depth-limited.** The
  sub-agent's tool surface is exactly `web_search` + `fetch_webpage`;
  `spawn_subagent` is not in it. If you widen `subagent_tools`, keep it out, and
  keep memory/reminders/skills out too: the sub-agent has no user and must not
  mutate anything the parent owns.

## Gotchas that will cost you an afternoon

- **`LlmScheduler` panics on a zero ceiling.** The bound is enforced three times
  over: Discord's `min_int_value`, the command handler, and a `.max(1)` when
  loading a stored record. Keep all three; only the last covers a corrupt row.
- **Scheduler env vars are startup defaults only.** Once a configurer sets a
  ceiling with `/config scheduler`, the value stored in `bot_config` wins on
  every later boot.
- **`pump()` skips a waiter it cannot admit** rather than stopping at it.
  Without that, a `SubAgent` parked on its own cap head-of-line blocks every
  `Background` waiter behind it. There is a test for exactly this.
- **A `Permit` must never be dropped inside `pump`.** It releases on drop and
  `pump` runs while holding the state lock, so dropping there deadlocks;
  `Permit::forget()` exists for the one path where a waiter is cancelled between
  the liveness check and the hand-off.
- **`tokio::fs::File` does not flush on drop.** A merge-audit record was being
  lost this way, which made two tests flaky under load. Always
  `file.flush().await` after `write_all`.
- **Skill frontmatter is parsed by a hand-written YAML subset**
  (`frontmatter.rs`), because the workspace has no YAML crate and `serde_yaml`
  has been archived since 2024. It accepts only `key: scalar` and `key: [a, b]`
  and **rejects** anything else rather than half-understanding it. If you add a
  field, add it there and to `Skill::to_skill_md`, and keep the round-trip test
  green — a description containing a colon is the case that breaks naive
  emitters.
- **`create_skill`'s `update: true` flag is doing a real job.** It replaced the
  version number that used to provide optimistic concurrency, which is what
  stopped a create silently clobbering an existing skill. Do not drop it without
  putting something else in its place.
- **Sub-agents take no `CancelToken`.** Cancelling the parent turn abandons the
  sub-agent in place rather than stopping it — acceptable at
  `MAX_SUBAGENT_ROUNDS = 8`, but thread the token through if that grows.
- **Sub-agent token usage is billed to the parent's conversation**, so the
  leaderboard charges the user who caused the spawn.
- **Outbound attachments are gone.** Nothing can post a file from a tool result;
  `download_file` and `run_lua` were the only producers. Inbound handling and
  the code-block upload path still work. If skill scripts need to emit files,
  re-add the path *with* its producer; the shape is in git history at `f1e02dd`.
- **Profile data comes from Discord live** — display name, nickname, and avatar
  feed the system prompt but are not persisted; `message_flow.rs` fetches them
  each turn.
- **`/personalize` survives**, despite the plan's cut list pairing it with
  `message-log`. What was cut was the profile-learning behind it; the command
  still carries personality, follow-up, and progress toggles that have no other
  home. Raise it before removing it.
- **`rate-limit` and `bot-commands` are keepers**, despite earlier drafts.

## House rules that bite

From `CLAUDE.md`, worth repeating because they are enforced:

- No comments describing *what* code does — only non-obvious *why*.
- No dead code. Removing a feature strands helpers and enum variants; clippy
  catches them, but only with `--all-targets`.
- Do not open pull requests. Do not push to `main`. Do not force-push. Do not
  modify CI workflows — which is why two of the three open items above are open.
- `cargo fmt` and `cargo clippy --all-targets -- -D warnings` must both pass.

One earned lesson: deleting a block of Rust by matching on text easily swallows
the *following* item. It happened three times in Phase 1 and the compiler caught
it each time only because the deleted thing was still referenced. If you delete
something unreferenced, nothing will tell you. Prefer explicit start/end markers
and re-read the region after.
