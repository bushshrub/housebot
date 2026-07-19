# Automated Feature Development

Housebot can dispatch automated coding jobs to Claude Code or OpenCode for an existing GitHub issue by triggering the selected workflow through the GitHub App.

## Overview

```
Discord message (owner only)
  └─ LLM calls prepare_feature_development
       └─ Bot intercepts DISPATCH_FLOW:<uuid>
            └─ Discord component UI (agent → model → effort → confirm)
                 └─ Existing issue validated
                      └─ Selected workflow dispatched through GitHub App
                           └─ Agent runs against the issue
                                └─ PR opened for review
```

Only the configured bot owner can initiate a dispatch. The LLM cannot start a job on its own — it only prepares a structured specification, which the owner reviews and confirms in Discord.

---

## Triggering a job

1. In Discord, ask the bot to develop a feature. It will call `prepare_feature_development` to draft the spec.
2. The bot shows a component UI with four steps: choose an agent, choose a model, choose an effort level, then confirm.
3. On confirmation, the bot validates the existing issue and dispatches the selected agent workflow through the GitHub App.
4. The selected workflow runs the agent against that issue.
5. If the agent produces changes, a pull request is opened. Review and merge it manually.

The entire dispatch can be cancelled at any point in the Discord UI before confirmation.

---

## Agents

| Agent | CLI | Authentication | Effort |
|---|---|---|---|
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

`.github/workflows/claude-dispatch.yml` and `.github/workflows/opencode-dispatch.yml` are manually triggered through `workflow_dispatch` by the Housebot GitHub App. Codex dispatch is temporarily disabled until a trusted self-hosted runner is available.

### Steps

1. Receive the existing issue number and prompt as workflow inputs.
2. Verify the dispatch actor is a GitHub App bot.
3. Checkout the repository and run the selected agent.
4. Open a pull request against the existing issue.

---

## Dispatch environment

The Claude Code and OpenCode dispatch workflows run on GitHub-hosted
`ubuntu-latest` runners. They do not depend on the `housebot-agent` self-hosted
runner, locally installed agent CLIs, or persisted OAuth state.

| Requirement | Notes |
|---|---|
| `CLAUDE_CODE_OAUTH_TOKEN` | Repository secret consumed by the Claude Code action |
| `CONTEXT7_API_KEY` | Repository secret consumed by the OpenCode action when configured |
| `GITHUB_TOKEN` | Built-in workflow token used to access the repository and open the PR |

---

## GitHub labels

New dispatches do not create, add, or remove issue labels. Existing labels remain owned by repository maintainers and workflows.

| Label | Meaning |
|---|---|
| `agent:queued` | Legacy label-driven queue; new dispatches do not add it |
| `agent:running` | Runner picked up the job |
| `agent:completed` | Agent produced changes; PR opened |
| `agent:no-changes` | Agent ran successfully but made no changes |
| `agent:failed` | Adapter or workflow step failed |
| `agent:failed-auth` | Auth failure (OAuth expired, API key missing) |
| `agent:failed-config` | Configuration error (catalog mismatch, missing secret) |
| `agent:claude` | Job dispatched to Claude Code |
| `agent:opencode` | Job dispatched to OpenCode |
| `source:discord` | Job originated from the Discord bot |

Legacy label definitions may still exist for older jobs; new dispatches do not depend on them.

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

## Self-hosted runner health check

`.github/workflows/check-agent-runner.yml` is a separate diagnostic for the
optional `housebot-agent` self-hosted runner. It is not used by the Claude Code
or OpenCode dispatch workflows and only verifies tools installed on that
runner.

---

## Adapter scripts

| Script | Agent |
|---|---|
| `.github/agents/run-codex.sh` | Codex |
| `.github/agents/run-claude.sh` | Claude Code |
| `.github/agents/run-opencode.sh` | OpenCode + NVIDIA NIM |
| `.github/agents/common.sh` | Shared utilities (sourced by all adapters) |

Each adapter accepts `<prompt_file> <model> <effort>` and is responsible only for running the agent. Label transitions and PR creation are handled by the workflow.
