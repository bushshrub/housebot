"""Sandbox tool — runs opencode or claude-code in an ephemeral Docker container."""

import asyncio
import os
import shutil
from collections.abc import Awaitable, Callable
from pathlib import Path
from uuid import uuid4
from typing import Any

import docker
import docker.errors
import sentry_sdk

SANDBOX_IMAGE = os.getenv("SANDBOX_IMAGE", "house-chatbot-sandbox:latest")
DOCKER_NETWORK = os.getenv("DOCKER_NETWORK", "house-chatbot_default")
TIMEOUT = int(os.getenv("SANDBOX_TIMEOUT", "300"))
SANDBOX_CPU_QUOTA = int(
    os.getenv("SANDBOX_CPU_QUOTA", "200000")
)  # 2 CPUs (100000 = 1 CPU per 100ms period)
SANDBOX_MEM_LIMIT = os.getenv("SANDBOX_MEM_LIMIT", "1g")
ARTIFACTS_DIR = os.getenv("ARTIFACTS_DIR", "data/artifacts")
MAX_ARTIFACT_SIZE_MB = int(os.getenv("MAX_ARTIFACT_SIZE_MB", "24"))

# HOST_DATA_DIR is the host-side absolute path to ./data, needed because sandbox containers
# are Docker siblings — volume paths must be resolvable by the Docker daemon on the host,
# not inside this container's filesystem.
HOST_DATA_DIR = os.getenv("HOST_DATA_DIR", "")
CONTAINER_DATA_DIR = os.getenv("DATA_DIR", "data")

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
                    "Defaults to server-slop/gemma-4-12b-qat-q4kxl."
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
) -> dict[str, Any] | str:
    with sentry_sdk.start_span(op="sandbox.run", name="run_opencode") as span:
        span.set_data("task", task[:500])
        span.set_data("model", model or "default")
        span.set_data("repo_url", repo_url or "")
        span.set_data("seed_files", list(files.keys()) if files else [])
        return await _call_sandbox(
            "opencode", task, repo_url, files, model=model, on_progress=on_progress
        )


async def _call_sandbox(
    agent: str,
    task: str,
    repo_url: str | None,
    files: dict[str, str] | None,
    model: str | None = None,
    on_progress: ProgressCallback | None = None,
) -> dict[str, Any] | str:
    loop = asyncio.get_running_loop()
    return await loop.run_in_executor(
        None,
        _run_container_sync,
        agent,
        task,
        repo_url,
        files,
        model,
        on_progress,
        loop,
    )


def _make_workspace() -> tuple[str, str]:
    """Create a workspace dir and return (host_path, container_path).

    The host_path is passed to Docker as the volume source (visible to the daemon).
    The container_path is where this process reads/writes the same directory.
    They point to the same filesystem location via the ./data bind mount.
    """
    uid = uuid4().hex[:8]
    container_path = os.path.join(CONTAINER_DATA_DIR, "workspaces", uid)
    os.makedirs(container_path, exist_ok=True)
    os.chmod(container_path, 0o777)
    host_path = (
        os.path.join(HOST_DATA_DIR, "workspaces", uid)
        if HOST_DATA_DIR
        else container_path
    )
    return host_path, container_path


_EXCLUDED_FILENAMES = {"opencode.json", ".opencode.json"}


def _collect_workspace_files(container_workspace: str) -> list[str]:
    """Copy individual workspace files into ARTIFACTS_DIR; return their paths."""
    collected: list[str] = []
    uid = uuid4().hex[:8]
    os.makedirs(ARTIFACTS_DIR, exist_ok=True)

    for root, _dirs, files in os.walk(container_workspace):
        _dirs[:] = [d for d in _dirs if not d.startswith(".")]
        for filename in files:
            if filename in _EXCLUDED_FILENAMES or filename.startswith("."):
                continue
            src = os.path.join(root, filename)
            size_mb = os.path.getsize(src) / (1024 * 1024)
            if size_mb > MAX_ARTIFACT_SIZE_MB:
                continue
            rel = os.path.relpath(src, container_workspace)
            flat = rel.replace(os.sep, "_")
            dst = os.path.join(ARTIFACTS_DIR, f"{uid}_{flat}")
            shutil.copy2(src, dst)
            collected.append(dst)

    return collected


def _run_container_sync(
    agent: str,
    task: str,
    repo_url: str | None,
    files: dict[str, str] | None,
    model: str | None,
    on_progress: ProgressCallback | None,
    loop: asyncio.AbstractEventLoop,
) -> dict[str, Any] | str:
    try:
        client = docker.from_env()
    except docker.errors.DockerException as exc:
        return f"Error: cannot connect to Docker socket: {exc}"

    environment: dict[str, str] = {
        "AGENT": agent,
        "TASK": task,
        "REPO_URL": repo_url or "",
        "MODEL": model or "",
        "NO_COLOR": "1",
        "TERM": "dumb",
    }
    for var in ("LLAMA_CPP_URL", "LLAMA_CPP_MODEL"):
        val = os.getenv(var, "")
        if val:
            environment[var] = val
    cc_token = os.getenv("CC_OAUTH_TOKEN") or os.getenv("CLAUDE_CODE_OAUTH_TOKEN", "")
    if cc_token:
        environment["CLAUDE_CODE_OAUTH_TOKEN"] = cc_token

    host_workspace, container_workspace = _make_workspace()
    try:
        if files:
            for rel, content in files.items():
                p = Path(container_workspace) / rel
                p.parent.mkdir(parents=True, exist_ok=True)
                p.write_text(content)

        try:
            container = client.containers.run(
                SANDBOX_IMAGE,
                name=f"sandbox-{uuid4().hex[:8]}",
                network=DOCKER_NETWORK,
                environment=environment,
                volumes={host_workspace: {"bind": "/workspace", "mode": "rw"}},
                detach=True,
                remove=False,  # we remove manually after streaming logs
                cpu_quota=SANDBOX_CPU_QUOTA,
                cpu_period=100000,
                mem_limit=SANDBOX_MEM_LIMIT,
            )
        except docker.errors.ImageNotFound:
            return f"Error: sandbox image '{SANDBOX_IMAGE}' not found — run: docker compose build sandbox"
        except docker.errors.APIError:
            raise

        lines: list[str] = []
        artifact_paths: list[str] = []
        try:
            for raw in container.logs(stream=True, follow=True):
                line = raw.decode(errors="replace")
                lines.append(line)
                if on_progress:
                    asyncio.run_coroutine_threadsafe(on_progress(line), loop).result(
                        timeout=5
                    )

            result = container.wait(timeout=TIMEOUT)
            exit_code = result.get("StatusCode", -1)
            if exit_code != 0:
                output = "".join(lines).strip()
                return f"Error: sandbox exited with code {exit_code}.\n{output}"

            artifact_paths = _collect_workspace_files(container_workspace)
        except Exception as exc:
            try:
                container.kill()
            except Exception:
                pass
            return f"Error: sandbox failed: {exc}"
        finally:
            try:
                container.remove(force=True)
            except Exception:
                pass
    finally:
        shutil.rmtree(container_workspace, ignore_errors=True)

    output = "".join(lines).strip() or "(no output)"
    result = {"content": output}
    if artifact_paths:
        result["_artifact_paths"] = artifact_paths
    return result
