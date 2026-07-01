"""Claude Code tool — runs claude CLI in an ephemeral sandbox container."""

from typing import Any

import sentry_sdk

from .opencode import ProgressCallback, _call_sandbox

TOOL_DEFINITION: dict[str, Any] = {
    "name": "run_claude_code",
    "description": (
        "Run a software development task using Claude Code (Anthropic's coding agent). "
        "Best for complex tasks requiring deep reasoning, large refactors, or multi-file changes."
    ),
    "input_schema": {
        "type": "object",
        "properties": {
            "task": {
                "type": "string",
                "description": "The software development task to perform.",
            },
            "model": {
                "type": "string",
                "description": "Claude model to use, e.g. claude-haiku-4-5-20251001 or claude-opus-4-8. Defaults to claude-haiku-4-5-20251001.",
            },
            "repo_url": {
                "type": "string",
                "description": "Optional Git repository URL to clone into the workspace.",
            },
            "files": {
                "type": "object",
                "description": "Optional map of relative file paths to content to seed before running.",
                "additionalProperties": {"type": "string"},
            },
        },
        "required": ["task"],
    },
}


async def run_claude_code(
    task: str,
    model: str | None = None,
    repo_url: str | None = None,
    files: dict[str, str] | None = None,
    on_progress: ProgressCallback | None = None,
) -> dict[str, Any] | str:
    with sentry_sdk.start_span(op="sandbox.run", name="run_claude_code") as span:
        span.set_data("task", task[:500])
        span.set_data("model", model or "default")
        span.set_data("repo_url", repo_url or "")
        span.set_data("seed_files", list(files.keys()) if files else [])
        return await _call_sandbox(
            "claude", task, repo_url, files, model=model, on_progress=on_progress
        )
