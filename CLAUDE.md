# Housebot — Claude Code Agent Instructions

You are running as an automated coding agent dispatched via the Housebot Discord bot.
These instructions apply to automated runs only.

## Objective

Implement the feature described in the GitHub issue you were given.
Your output is a committed set of code changes on the current branch.
A pull request will be opened automatically after you finish — do not open one yourself.

## Build and test commands

```bash
cargo build                                   # compile
cargo test                                    # run all unit tests
cargo clippy --all-targets -- -D warnings     # lint (must pass)
cargo fmt --check                             # formatting check
```

Fix any `cargo fmt` or `cargo clippy` warnings before finishing.

The release operator under `operator/` is a separate Go module and is **not**
covered by `cargo test --all`. When you change it, run its own commands:

```bash
go -C operator build ./...
go -C operator test ./...
go -C operator vet ./...
gofmt -l operator                             # must print nothing
```

Its CRD and RBAC are generated from the Go types and the kubebuilder markers on
the reconciler, so regenerate them rather than editing the YAML by hand:

```bash
cd operator
go tool controller-gen object paths=./api/...
go tool controller-gen crd paths=./api/... \
  output:crd:artifacts:config=../deploy/helm/housebot-operator/crds
# The ClusterRole in the chart is transcribed from these markers by hand:
go tool controller-gen rbac:roleName=housebot-operator paths=./internal/... output:stdout
```

## Code conventions

- **No comments that describe what the code does** — only add a comment when the WHY is non-obvious.
- Match the style of the surrounding code exactly.
- No dead code, no `#[allow(unused)]` without a real reason.
- Prefer editing existing files over creating new ones.
- Do not add abstractions or features beyond what the issue requires.

## Security requirements

**These are hard constraints. Violating them is a disqualifying failure.**

- Do NOT access, read, print, or log any production credentials or secrets.
- Do NOT read `.env`, `docker-compose.yml`, or any file that contains tokens, keys, or passwords.
- Do NOT connect to production infrastructure (the Discord bot, the LLM server, Jellyfin, etc.).
- Do NOT push to `main` or `master`.
- Do NOT force-push.
- Do NOT merge branches.
- Do NOT deploy anything.
- Do NOT open pull requests — the workflow does this for you.

## Scope

- Keep changes strictly scoped to what the issue asks for.
- Do not refactor unrelated code.
- Do not bump dependency versions unless the issue specifically requires it.
- Do not modify CI workflows or this file.
