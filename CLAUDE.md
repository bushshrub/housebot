# Housebot

Architecture and conventions: `AGENTS.md`. Current state, open work, and
gotchas: `HANDOFF.md`.

## Checks

All must pass before committing.

```bash
cargo build
cargo test --workspace                        # plain `cargo test` runs only the root package
cargo clippy --all-targets -- -D warnings     # the lint CI runs
cargo fmt --check
```

## House rules

- Comments say why, never what, and only when the why is not obvious.
- No dead code. `#[allow(unused)]` needs a real reason.

## Commits and pull requests

- Author every commit as the Claude identity from the `Co-Authored-By` line
  (for example `--author="Claude Opus 5.5 <noreply@anthropic.com>"`), never
  as the repo owner.
- After opening a PR, post `/oc review` and `@coderabbitai review` as
  comments. Wait for both reviews, then fix what is valid.
- PRs merge by rebase: enable it with `gh pr merge <number> --auto --rebase`.

## Hard limits

- Never read, print, or log credentials or secrets, including `.env` and `docker-compose.yml`.
- Never connect to production infrastructure (Discord, the deployment host).
  The one exception: test requests to the LLM gateway are allowed, with free
  models only. Never call a paid model.
- Never push to `main` or `master`, force-push, or deploy. Never merge into
  `master` by hand; enabling rebase auto-merge on a PR is allowed.
