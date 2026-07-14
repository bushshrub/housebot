# Automated Feature Development

Housebot can dispatch automated coding jobs to one of three agents — Codex, Claude Code, or OpenCode — by creating a structured GitHub issue and triggering a self-hosted runner workflow.

## Overview

```
Discord message (owner only)
  └─ LLM calls prepare_feature_development
       └─ Bot intercepts DISPATCH_FLOW:<uuid>
            └─ Discord component UI (agent → model → effort → confirm)
                 └─ GitHub issue created with hidden metadata
                      └─ develop-feature.yml workflow triggered
                           └─ Agent runs on self-hosted runner
                                └─ PR opened for review
```

Only the configured bot owner can initiate a dispatch. The LLM cannot start a job on its own — it only prepares a structured specification, which the owner reviews and confirms in Discord.

---

## Triggering a job

1. In Discord, ask the bot to develop a feature. It will call `prepare_feature_development` to draft the spec.
2. The bot shows a component UI with four steps: choose an agent, choose a model, choose an effort level, then confirm.
3. On confirmation, a GitHub issue is created and labeled `agent:queued`.
4. The `develop-feature.yml` workflow picks up the issue and runs the selected agent.
5. If the agent produces changes, a pull request is opened. Review and merge it manually.

The entire dispatch can be cancelled at any point in the Discord UI before confirmation.

---

## Agents

| Agent | CLI | Authentication | Effort |
|---|---|---|---|
| **Codex** | `codex` | OAuth session on runner | Single level (account default) |
| **Claude Code** | `claude` | OAuth session on runner | low / medium / high (via `--max-turns`) |
| **OpenCode** | `opencode` | `NVIDIA_API_KEY` secret | low / medium / high (execution timeout) |

Agent and model combinations are defined in `.github/agents/catalog.json`. That file is the single source of truth; the Discord UI is generated from it at runtime.

---

## Catalog

`.github/agents/catalog.json` contains a versioned list of agents, models, and effort levels.

```jsonc
{
  "schema_version": 1,
  "catalog_revision": "2026-07-14-1",  // bump when adding/removing entries
  "cli_versions": { ... },
  "agents": {
    "claude": {
      "default_model": "default",
      "models": [
        {
          "id": "default",
          "efforts": [
            { "id": "low",    "mechanism": "native" },
            { "id": "medium", "mechanism": "native" },
            { "id": "high",   "mechanism": "native" }
          ]
        }
      ]
    }
    // ...
  }
}
```

**Updating the catalog:** edit the JSON, bump `catalog_revision`, and commit to `main`. Pending jobs that were dispatched before the bump will fail catalog validation — they must be re-dispatched by the owner.

---

## Workflow

`.github/workflows/develop-feature.yml` runs on the `housebot-agent` self-hosted runner label when an issue is labeled `agent:queued`.

### Steps

1. Resolve issue number.
2. Ensure required labels exist in the repo.
3. Extract `housebot-development-job` metadata from the issue body.
4. Transition label: `agent:queued` → `agent:running`.
5. Checkout the repository.
6. Validate catalog (revision + agent/model/effort combination).
7. Configure git identity (`Housebot Agent`).
8. Create a branch: `agent/<agent>/issue-<number>`.
9. Run the adapter script (`run-<agent>.sh`).
10. Detect changes (`git diff --cached`).
11. Commit and push (if changes).
12. Open a pull request (if changes).
13. Transition label: `agent:running` → `agent:completed` / `agent:no-changes`.
14. On failure: `agent:running` → `agent:failed`.

---

## Runner requirements

The `housebot-agent` self-hosted runner needs:

| Requirement | Notes |
|---|---|
| GitHub Actions runner | Registered with the `housebot-agent` label |
| `codex` CLI | Logged in via OAuth |
| `claude` CLI | Logged in via OAuth (`~/.claude/`) |
| `opencode` CLI | Installed; uses `NVIDIA_API_KEY` at runtime |
| `python3` | For catalog validation |
| `git` | Standard |
| `gh` | GitHub CLI, authenticated |

**The runner account must NOT have access to:**
- Housebot production `.env` / `docker-compose.yml`
- Discord bot token
- Housebot GitHub App private key
- Production Docker socket
- Deployment SSH keys
- Any unrelated household infrastructure credentials

---

## GitHub labels

The workflow manages these labels automatically:

| Label | Meaning |
|---|---|
| `agent:queued` | Issue created; job waiting for runner |
| `agent:running` | Runner picked up the job |
| `agent:completed` | Agent produced changes; PR opened |
| `agent:no-changes` | Agent ran successfully but made no changes |
| `agent:failed` | Adapter or workflow step failed |
| `agent:failed-auth` | Auth failure (OAuth expired, API key missing) |
| `agent:failed-config` | Configuration error (catalog mismatch, missing secret) |
| `agent:codex` | Job dispatched to Codex |
| `agent:claude` | Job dispatched to Claude Code |
| `agent:opencode` | Job dispatched to OpenCode |
| `source:discord` | Job originated from the Discord bot |

Labels are created automatically by the workflow on first run.

---

## Security model

- **Owner-only dispatch:** the Rust bot enforces that only the configured `OWNER_DISCORD_ID` can use `prepare_feature_development`. This check is in `src/tools/feature_development.rs` — it does not rely on the system prompt or Discord channel permissions.
- **No LLM-initiated dispatch:** the LLM can only _prepare_ a specification. Actual dispatch requires the owner to complete the Discord component UI and explicitly confirm.
- **Catalog validation:** the workflow validates agent/model/effort against the embedded catalog before running, preventing dispatch of unknown combinations.
- **No shell injection:** issue content is written to a temp file by the `actions/github-script` step and passed to adapters via file path — it is never interpolated into shell commands.
- **No production secrets on runner:** the runner account is isolated from production infrastructure. See runner requirements above.
- **No force-push, no auto-merge, no auto-deploy:** the workflow only opens a PR. All merging and deployment remains a manual, human-approved step.
- **Rate limiting:** at most 2 dispatch jobs per 10 minutes per owner (enforced in Rust).

---

## Runner health check

`.github/workflows/check-agent-runner.yml` runs daily at 09:00 UTC and verifies that `codex`, `claude`, `opencode`, `python3`, and `git` are all available on the runner. Failures appear in the Actions tab.

---

## Adapter scripts

| Script | Agent |
|---|---|
| `.github/agents/run-codex.sh` | Codex |
| `.github/agents/run-claude.sh` | Claude Code |
| `.github/agents/run-opencode.sh` | OpenCode + NVIDIA NIM |
| `.github/agents/common.sh` | Shared utilities (sourced by all adapters) |

Each adapter accepts `<prompt_file> <model> <effort>` and is responsible only for running the agent. Label transitions and PR creation are handled by the workflow.
