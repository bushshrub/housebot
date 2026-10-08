# Handoff — Housebot rebuild

Read this together with [`docs/REDESIGN_PLAN.md`](docs/REDESIGN_PLAN.md), which
is the authoritative scope document — this file covers state and gotchas, the
plan covers what was built.

## Classifier and proactive mode (2026-10-07)

Branch `claude/proactive-mode`. Never run against live Discord.

- **Classifier.** A System One decision model (`kev`) decides before the main
  model is called. Client: `crates/llm/src/system_one.rs`; questions and
  thresholds: `src/agent/classify.rs`. Through the Bifrost gateway the path is
  `<base>/typesafe/v1/systemone` (plain `/v1/systemone` returns 405), with the
  usual `LLM_API_KEY` as Bearer. The key is sent only when the classifier URL
  has the same origin as `LLM_BASE_URL`, because configurers can set the URL. Set it with
  `/labs classifier url:https://llm.robertx.net/typesafe model:kev`; it is
  stored under the `classifier` key in `bot_config`. With no setting it is off.
- **Pings.** The classifier replaces the old `select_emoji` main-model call:
  one request picks react or a full answer, plus the emoji from a fixed set
  (👍 ❤️ 😂 🎉 👋 🙏). Off or failing means a full answer.
- **Proactive mode.** `/labs proactive channel enabled` (server admins and
  configurers), off by default. An unaddressed message in that channel (or its
  threads) asks "open question or request?" and "good news or thanks?". Answer
  ≥ 0.5 escalates to a full reply to the author; else react ≥ 0.5 reacts; else
  nothing. Escalations are limited to one per channel per 120 s. The state is
  the last 10 messages from `ChannelContext::recent`.
- **Who reaches it.** Silenced users and channels off the allowlist never do.
  Another bot's unmentioned message reaches only the proactive classifier, and
  only when `/server-config bot_pings` is on.
- **Tested by hand on 2026-10-07** with 24 written cases: pings 8/8, proactive
  15/16 (the miss: "grab milk pls", a request to a person, escalates), emoji
  12/12, about 200 ms per call. Check real channel logs before trusting the
  0.5 thresholds.
- **Settings removed:** `/personalize followup` and `progress`, `/labs
  pagination` and `tool_rounds`, `/privacy` (deep memory), `/config
  dev_notify_channel` with the whole completion-notice feature, and the
  leaderboard roles. Their defaults are now fixed: follow-ups in DMs only,
  progress shown, memory on, 50 tool rounds, no pagination, `restricted`
  leaderboard means administrators only. Old stored JSON keeps the dropped
  keys until the next save; serde ignores them.
- **Tool calls go to a thread.** The progress message is one status line
  (`src/bot/progress.rs`): "Thinking" at the start of each model round,
  "Generating" only once answer text streams, the running tool while a tool
  runs. The first tool call (or text written beside it) opens a thread from
  that message; tool calls go there. At the end the status line becomes a
  grouped `tool_summary` with a link and the thread is archived; with no tools
  the progress message is deleted; a cancelled turn keeps "Cancelled". Where
  no thread can be made (DMs, inside a thread, no permission), tool lines are
  posted in the channel. "Thinking" is signalled by `ScheduledChatClient`
  once a slot is granted, so the queue-position message stays until then.
  Proactive answers (`ResponseMode::Unprompted`) get no progress message, and
  no rate-limit warning, context warning, failure notice, or development
  flow. Needs the "Create Public Threads" permission; not yet seen in a live
  server.
- **Dev notices never worked:** the GitHub secret `DEV_NOTIFY_SIGNING_KEY` was
  never set, so `opencode-dispatch.yml` skips its notify step. That step is now
  dead; it was left in place because CI workflows are off-limits.

## Deployment bot (2026-09-30)

Done on branch `claude/handoff-open-work`, never run against a live Docker host:

- **Image readiness.** `/deploy`, `/update`, and the startup deploy check the
  three `sha-<full sha>` images with `docker manifest inspect` (`images_published`
  in `docker.rs`). Only a "manifest unknown" / "no such manifest" answer counts as
  unpublished; any other error lets the deploy go ahead. `/deploy <sha>` without
  images refuses and names the newest commit that has them (last 10 commits on the
  branch). `/deploy` with no SHA and `/update` fall back to that commit, with a
  note. The confirm button now carries the real SHA, never `latest`. The startup
  deploy retries every 60 s, 30 times.
- **First deployment changelog.** With no `house-chatbot` container, the card says
  "First deployment" instead of an error.
- **Image cleanup.** `cleanup_old_images` existed but never saw the sandbox images:
  `docker images 'ghcr.io/bushshrub/housebot*'` is a Go `path.Match` pattern and `*`
  does not cross `/`. It now lists all images and filters in Rust, keeps the new
  release and all three images of the previous one, skips images still in use
  instead of aborting, and also runs after `/update`.
- **`.env`.** `parse_dotenv` now drops an unquoted ` # comment`. The deployment bot
  already re-reads `/app/.env` (or `./.env`) on every deploy, but only if the host
  mounts it there. **Still open:** the host compose file must bind the `.env` into
  the deployment-bot container. Mount the directory rather than the single file
  (for example `./:/app/config:ro`, and add that path to `configured_env`),
  because editors replace the file and a single-file bind mount keeps the old one.

## Sandbox DNS (2026-09-30)

The dnsmasq forwarder from `b3048b4` never worked: dnsmasq exited on the
read-only root because it could not write its pidfile, and `--read-only` also
makes the sandbox's `/etc/resolv.conf` read-only, so it could not be repointed.
Public-internet sandboxes now run on Docker's default `bridge` network with
`--dns=1.1.1.1 --dns=8.8.8.8`, which Docker writes straight into resolv.conf.
A user-defined network always forces `127.0.0.11`, which gVisor cannot reach.
The old `housebot-sandbox-dns` container and `housebot-sandbox-net` network are
no longer used and must be removed by hand on the host.

## Pings always get an answer (2026-10-07)

The 2026-10-06 log showed a ping dropped as "no text or attachments after
removing the mention". Both empty-message drops in `handle_message()` are gone:
an empty ping now uses the replied-to message, or goes to the model as
`(no text)`. Also:

- A mention of the bot's managed role (`<@&id>`, which Discord's @ autocomplete
  offers) counts as a ping.
- Forwarded messages (`message_snapshots`) are read, text and attachments.
- A thread follows its parent channel for the channel allowlist and follow-ups
  (`allowed_config_channel`). Pings in channels not on the allowlist are still
  ignored without a reply.
- A silenced user who addresses the bot gets a public reply saying so, at most
  once per 5 minutes per user. It cannot be ephemeral: Discord allows that
  only on interaction responses.
- A reply that fails to send (any chunk) logs
  `Failed to send reply` at `warn`.
- A bare ping skips the classifier and reaches the model as
  `(The user pinged you without any text.)`.

## Open work: chatbot (2026-09-30)

1. **Hexagone got no answer for some messages.** Not explained. The bot did
   answer hexagone later (2026-09-29, about 9 PM). Hexagone used a real
   @-mention of the bot user. There was no reaction and nothing in the log for
   the message that got no answer. At `info` level, every answered message logs
   `Agent run started` (`src/agent/run.rs`). So that message stopped at one of
   the silent `return`s in `message()` (`src/bot/handler.rs`) or
   `handle_message()` (`src/bot/message_flow.rs`). A successful emoji-only
   reaction also returns without an `info` line. The owner suspects the LLM
   concurrency limit. That does not fully match the code: the emoji-selection
   call has a 15 s timeout that logs a `warn`, and a saturated scheduler shows
   "You are #N in line". Do item 2 first, then check again.

2. **Dropped messages are logged (done).** Every early `return` in `message()`
   and `handle_message()` logs its reason under target `housebot::message_flow`:
   `info` for access policy, disabled bot pings, a mentioned message in a channel
   that is not allowed, and an empty message; `debug` for unaddressed messages and
   unmentioned bots. An emoji-only answer logs `Answered with an emoji-only
   reaction` (target `housebot::emoji`). Check item 1 again with these.

3. **Unexplained "Unexpected reasoning effort high".** Before commit `2b38ed0`
   the bot sent `reasoning: {"enabled": true, "max_tokens": 8192}` for users on
   the old `high` level, and the gateway answered 400 "Unexpected reasoning
   effort high. Supported types are xhigh (default), medium, and low." That
   exact field, and even `reasoning_effort: "high"`, did not fail when sent by
   hand (with and without tools), so the cause was never found. `2b38ed0` sends
   only `low` / `medium` / `xhigh` as `reasoning_effort`, and retries a
   reasoning-related 400 once with a `reasoning.max_tokens` limit. Watch the
   logs for "Model rejected reasoning_effort" to see whether the retry fires.

4. **The gateway may route `slopbot` to another model.** `slopbot` is an alias
   on the Bifrost gateway (`LLM_BASE_URL`), routed by its "Slopbot Routing" rule
   to `vllm/swift-1.5-qwen3.8-27b-paro-mxfp6`, which supports only the effort
   levels `low`, `medium`, `xhigh`. llama.cpp models ignore
   `reasoning_effort`. The bot's `/props` probe always fails through Bifrost
   (it is a llama.cpp endpoint), so set `MAX_CONTEXT_TOKENS` in `.env` to the
   routed model's real context size.

## Done in the working tree, not committed (2026-09-29)

Made on local `master`, with no commit yet. Commit it to a branch before you
continue. All checks pass: `cargo test --workspace` (521),
`cargo clippy --all-targets -- -D warnings`, and `cargo fmt --check`.

- **No time in the system prompt.** A new native tool, `get_current_time`
  (optional IANA `timezone`, UTC by default), replaces it. The zone data comes
  from `chrono-tz`, which is built into the binary. The sandbox image
  (`crates/sandbox/docker/Dockerfile`) now installs `tzdata` for scripts. The
  image must be rebuilt.
- **No default output-token cap.** Normal requests send only
  `reasoning_effort` and no `max_tokens`. The per-level thinking limit
  (`thinking_tokens`) is sent only in the fallback retry, when the model
  rejects `reasoning_effort`. The per-user `max_output_tokens` policy still
  applies when it is set. The cause was an xhigh request that used all 20,480
  tokens thinking and never answered.
- **Prompt cache fixes.** A very high cache hit rate is the top priority.
  **Never rewrite or delete history, and never put per-turn data before the
  history.**
  - History is never trimmed. The 60-message sliding window is gone, and
    `MAX_HISTORY_TURNS` is removed. The 90% compaction limits the size.
  - The request order is: system prompt → skills list → history → new message.
  - Memory and profile are out of the system prompt. They go in a "session
    context" message (`build_session_context_message`), saved as the first
    history message of each session.
  - `discord_context` is removed from history messages before they are sent,
    so each message is sent again byte for byte.
  - Each LLM round logs `Prompt cache usage` (target `housebot::cache`) with
    `prompt_tokens`, `cached_tokens`, and `hit_percent`.
  - The test `each_turn_extends_the_previous_request_so_the_cache_hits` guards
    this. It must keep passing.

## Web fetch and search (done 2026-09-30)

- `fetch_webpage` takes only `url`. Chrome (`nav`, `footer`, `aside`, `form`, …) is
  removed, then `dom_smoothie` extracts the article as Markdown; with no article,
  the whole page is converted by `dom_query`. The Markdown escaping of punctuation
  is undone so the text can be searched.
- Under 200 characters of text, the keyless Firecrawl `/v2/scrape` is tried
  (`crates/tools/src/firecrawl.rs`), limited to 200 credits per UTC month by the
  `firecrawl_usage` table (migration `006`). The check and increment are one
  statement. Without the database (tests), Firecrawl is off.
- Text over 6,000 characters is written to `web/<host>-<hash>.md` in the sandbox
  through `LazySandbox::write`; the tool returns title, size, heading outline with
  line numbers, and the first 1,500 characters. If the sandbox fails, the first
  6,000 characters are returned inline with a note that the text was cut.
- `web_search`: 5 results by default, at most 10, no `(via engine)` tag, snippets
  cut at 300 characters.

Do not remove old tool results from history to save context. That breaks the
cache (see above).

## CI build time (done 2026-09-30, unverified)

`docker-publish.yml` now has a `compile` matrix job (`main`, `sandboxd`) that runs in
parallel with `test`, with rust-cache keys `musl-release-<name>`, and uploads the
binaries as artifacts. `build` needs `test` and `compile`, downloads the binary,
and only builds and pushes the images. The first run after merge will still miss
the cache.

Unrelated and pre-existing: `cargo clippy --workspace --all-targets` fails on
`crates/deployment-bot/src/lib_tests.rs` (`manual_pattern_char_comparison`) and
`crates/token-monitor/src/tests.rs` (unused import). CI runs clippy on the root
package only, so it does not see them.

All seven phases have landed. The per-phase narrative that used to live here is
in the commit history; what is kept below is the part that is still load-bearing
for whoever touches this next.

## Where the work is

**Branch: `claude/phase-6-continuation-pg9rzq`.** It carries the entire rebuild —
phases 1 through 7, 25 commits — and is the only place any of it exists.

- **Nothing is merged.** `master` has none of the rebuild. Do not branch new work
  from `master` expecting to find it.
- The branch was rebased onto the phase 5 tip
  (`claude/bot-redesign-phase-five-jz9cc2`), so its history is continuous with
  the earlier phase branches rather than a parallel line. The older branches —
  `claude/bot-redesign-audit-7gecv3`, `-audit-remaining-9aj83g` (PR #320,
  "Phase 1–4 complete"), `-audit-continue-cinqt6`, `-phase-five-jz9cc2` — are
  all ancestors or subsets of it. Ignore them; they are stale by definition now.
- **PR #321 tracks this branch** (github.com/bushshrub/housebot/pull/321).
  Pushing to the branch updates it. PR #320 covers phases 1–4 only and is
  behind — ignore it.

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

### What is left

Both flow-level items from the previous handoff are closed. The model picker
reaches the runner (`opencode-dispatch.yml` declares a `model` input; both
dispatch paths forward the selection), effort is gone entirely, the retired
Claude/Codex CI is deleted, and the one-item agent stage is collapsed — jobs now
open on `ChoosingModel` with the agent preset, so the flow is model → confirm.

Dead code is swept: the Lua-era leftovers (`LuaAnalysis`, `Agent::web_search`,
the bridge's unreachable `send_message` and its redaction path), the unused
sandbox constants and `build_inspect_args`, `DISPATCH_TRIGGER_COMMENT`, and all
four adapter scripts with `check-agent-runner.yml`.

What remains:

1. **Nothing has run live.** No real Postgres, Discord gateway, LLM server, or
   Docker daemon, and no real workflow dispatch. The model input's round trip is
   unverified — the 422-on-undeclared-input behaviour is asserted by a test that
   reads the YAML, not by an observed API call. The compose files now declare a
   `sandboxd` sidecar sharing a `sandbox-socket` volume with the bot, but that
   stack has never been brought up; `docker compose config` validating is all
   the assurance there is.

   **Back the database up before the first deploy.** `001_purge_all_data` drops
   the `public` schema and nothing in `scripts/deploy.sh` or the compose files
   takes a backup.
2. **Skill scripts are not guaranteed networkless.** `run_skill_script` asks for
   `NetworkAccess::None`, but `get_or_start` is first-wins: a script invoked
   after `sandbox_clone_repository` runs in that networked container. This is
   accepted — the sandbox itself (gVisor, tmpfs, no host mounts, no secrets) is
   the boundary — and the docs that claimed otherwise have been corrected.
3. **The Dockerfile crate list is hand-maintained.** `Dockerfile.deployment-bot`
   names every workspace manifest so the dependency layer caches. It had drifted
   eleven crates out of date and broke the image build; a test now asserts it
   matches `Cargo.toml` exactly. The main `Dockerfile` copies a prebuilt binary
   and has no such list.

## The database

`001_purge_all_data` drops the entire `public` schema; everything since is
recreated with a new shape.

| Table | Used by | Migration |
|---|---|---|
| `user_memories` | `crates/memory` | `002` |
| `bot_config` | `crates/bot-config` | `003` |
| `conversations`, `token_usage_events` | `crates/token-monitor` | `004` |
| `deployment_permissions` | `crates/deployment-bot` | `005` |
| `firecrawl_usage` | `crates/tools` (`firecrawl.rs`) | `006` |

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
| A deleted crate still `COPY`d by the deployment Dockerfile | `deployment_dockerfile_copies_exactly_the_workspace_crates` |
| A dispatch input the workflow does not declare | `every_dispatch_input_is_declared_by_the_workflow` |

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
  still carries the personality override, which has no other home. Raise it
  before removing it.
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
