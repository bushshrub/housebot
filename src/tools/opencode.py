"""Sandbox tool — runs opencode or claude-code in an ephemeral Docker container."""

import asyncio
import os
import tempfile
from collections.abc import Awaitable, Callable
from pathlib import Path
from uuid import uuid4
from typing import Any

SANDBOX_IMAGE = os.getenv("SANDBOX_IMAGE", "house-chatbot-sandbox:latest")
DOCKER_NETWORK = os.getenv("DOCKER_NETWORK", "house-chatbot_default")
TIMEOUT = int(os.getenv("SANDBOX_TIMEOUT", "300"))

TOOL_DEFINITION: dict[str, Any] = {
    "name": "run_opencode",
    "description": (
        "Run a software development task using OpenCode powered by a local llama.cpp model. "
        "Good for general coding tasks, quick scripts, and iterative work. "
        "Optionally clone a git repo or seed the workspace with files."
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
                "description": (
                    "Model to use, e.g. server-slop/qwen3.6-35b or server-slop/ornith-1.0-35b. "
                    "Defaults to server-slop/qwen3.6-35b."
                ),
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

ProgressCallback = Callable[[str], Awaitable[None]]


async def run_opencode(
    task: str,
    model: str | None = None,
    repo_url: str | None = None,
    files: dict[str, str] | None = None,
    on_progress: ProgressCallback | None = None,
) -> str:
    return await _call_sandbox("opencode", task, repo_url, files, model=model, on_progress=on_progress)


async def _call_sandbox(
    agent: str,
    task: str,
    repo_url: str | None,
    files: dict[str, str] | None,
    model: str | None = None,
    on_progress: ProgressCallback | None = None,
) -> str:
    with tempfile.TemporaryDirectory() as tmpdir:
        # Seed files so the container can find them at /workspace/<rel>
        if files:
            for rel, content in files.items():
                p = Path(tmpdir) / rel
                p.parent.mkdir(parents=True, exist_ok=True)
                p.write_text(content)
        # World-writable so the container's non-root sandbox user can write freely
        os.chmod(tmpdir, 0o777)

        env_args: list[str] = [
            "-e", f"AGENT={agent}",
            "-e", f"TASK={task}",
            "-e", f"REPO_URL={repo_url or ''}",
            "-e", f"MODEL={model or ''}",
        ]

        for var in ("LLAMA_CPP_URL", "LLAMA_CPP_MODEL"):
            val = os.getenv(var, "")
            if val:
                env_args += ["-e", f"{var}={val}"]

        cc_token = os.getenv("CC_OAUTH_TOKEN") or os.getenv("CLAUDE_CODE_OAUTH_TOKEN", "")
        if cc_token:
            env_args += ["-e", f"CLAUDE_CODE_OAUTH_TOKEN={cc_token}"]

        cmd = [
            "docker", "run", "--rm",
            "--name", f"sandbox-{uuid4().hex[:8]}",
            "--network", DOCKER_NETWORK,
            "-v", f"{tmpdir}:/workspace",
            *env_args,
            SANDBOX_IMAGE,
        ]

        lines: list[str] = []
        try:
            proc = await asyncio.create_subprocess_exec(
                *cmd,
                stdout=asyncio.subprocess.PIPE,
                stderr=asyncio.subprocess.STDOUT,
            )

            async with asyncio.timeout(TIMEOUT):
                async for raw in proc.stdout:
                    line = raw.decode(errors="replace")
                    lines.append(line)
                    if on_progress:
                        await on_progress(line)

            await proc.wait()
        except asyncio.TimeoutError:
            try:
                proc.kill()
            except Exception:
                pass
            return f"Error: sandbox timed out after {TIMEOUT}s."
        except FileNotFoundError:
            return "Error: docker not found — is the Docker CLI installed and the socket mounted?"
        except Exception as exc:
            return f"Error running sandbox: {exc}"

    return "".join(lines).strip() or "(no output)"
