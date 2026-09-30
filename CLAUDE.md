# Housebot — Claude Code Instructions

Architecture and conventions are in `AGENTS.md`. Current state, open work, and
gotchas are in `HANDOFF.md`.

## Checks

All must pass before committing.

```bash
cargo build
cargo test --workspace                        # plain `cargo test` runs only the root package
cargo clippy --all-targets -- -D warnings     # the lint CI runs
cargo fmt --check
```

## Code conventions

- No comments that describe what the code does — only the WHY, when it is non-obvious.
- Match the style of the surrounding code.
- No dead code, no `#[allow(unused)]` without a real reason.
- Prefer editing existing files over creating new ones.
- No abstractions or features beyond what the task requires.
- Keep changes scoped to the task: no unrelated refactors, no dependency bumps unless required.

## Hard limits

- Never read, print, or log credentials or secrets, including `.env` and `docker-compose.yml`.
- Never connect to production infrastructure (Discord, the LLM gateway, the deployment host).
- Never push to `main` or `master`, force-push, merge into `master`, or deploy.
