"""Sandbox API server — runs Claude Code or OpenCode in /workspace."""

import asyncio
import json
import logging
import os
import shutil
from pathlib import Path
from typing import AsyncIterator, Optional

from fastapi import FastAPI, HTTPException, Security
from fastapi.responses import StreamingResponse
from fastapi.security.api_key import APIKeyHeader
from pydantic import BaseModel

logging.basicConfig(level=logging.INFO, format="%(asctime)s %(levelname)s %(message)s")
logger = logging.getLogger("sandbox")

app = FastAPI(title="coding-sandbox")

WORKSPACE = Path("/workspace")
API_KEY = os.getenv("SANDBOX_API_KEY", "")
LLAMA_CPP_URL = os.getenv("LLAMA_CPP_URL", "")
LLAMA_CPP_MODEL = os.getenv("LLAMA_CPP_MODEL", "local")
TIMEOUT = int(os.getenv("SANDBOX_TIMEOUT", "300"))
CLAUDE_DEFAULT_MODEL = os.getenv("CLAUDE_MODEL", "claude-haiku-4-5-20251001")

_lock = asyncio.Lock()
_key_header = APIKeyHeader(name="X-Api-Key", auto_error=False)


def _check_key(key: str = Security(_key_header)) -> None:
    if API_KEY and key != API_KEY:
        raise HTTPException(status_code=401, detail="Invalid API key")


class RunRequest(BaseModel):
    task: str
    agent: str = "opencode"  # "opencode" | "claude"
    model: Optional[str] = None  # claude model override
    repo_url: Optional[str] = None
    files: Optional[dict[str, str]] = None


@app.post("/run")
async def run(req: RunRequest, _: None = Security(_check_key)) -> StreamingResponse:
    """Stream output as newline-delimited JSON: {"line": "..."} per line, then {"done": true, "exit_code": N}."""

    async def generate():
        async with _lock:
            async for chunk in _run_stream(req):
                yield json.dumps(chunk) + "\n"

    return StreamingResponse(generate(), media_type="application/x-ndjson")


@app.get("/health")
async def health() -> dict:
    return {"status": "ok"}


@app.get("/workspace/files")
async def list_files(_: None = Security(_check_key)) -> dict:
    if not WORKSPACE.exists():
        return {"files": []}
    files = sorted(
        str(p.relative_to(WORKSPACE))
        for p in WORKSPACE.rglob("*")
        if p.is_file()
        and not any(part.startswith(".") for part in p.parts[len(WORKSPACE.parts) :])
    )
    return {"files": files}


# ── internals ────────────────────────────────────────────────────────────────


def _clear_workspace() -> None:
    """Clear workspace contents without removing the directory itself
    (the sandbox user can't rmdir /workspace since / is owned by root)."""
    for child in WORKSPACE.iterdir():
        if child.is_dir():
            shutil.rmtree(child)
        else:
            child.unlink()


async def _run_stream(req: RunRequest) -> AsyncIterator[dict]:
    _clear_workspace()

    if req.repo_url:
        yield {"line": f"Cloning {req.repo_url}...\n"}
        proc = await asyncio.create_subprocess_exec(
            "git",
            "clone",
            "--depth=1",
            req.repo_url,
            str(WORKSPACE),
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.PIPE,
        )
        _, stderr = await asyncio.wait_for(proc.communicate(), timeout=120)
        if proc.returncode != 0:
            yield {
                "done": True,
                "exit_code": proc.returncode,
                "error": f"git clone failed: {stderr.decode().strip()}",
            }
            return

    for rel, content in (req.files or {}).items():
        target = WORKSPACE / rel
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(content)

    if req.agent == "opencode":
        async for chunk in _stream_opencode(req.task):
            yield chunk
    elif req.agent == "claude":
        async for chunk in _stream_claude(req.task, model=req.model):
            yield chunk
    else:
        yield {"done": True, "exit_code": 1, "error": f"Unknown agent '{req.agent}'."}


async def _stream_opencode(task: str) -> AsyncIterator[dict]:
    if not LLAMA_CPP_URL:
        yield {"done": True, "exit_code": 1, "error": "LLAMA_CPP_URL is not set."}
        return

    cfg_path = WORKSPACE / "opencode.json"
    if not cfg_path.exists():
        cfg_path.write_text(
            json.dumps(
                {
                    "model": f"llamacpp/{LLAMA_CPP_MODEL}",
                    "provider": {
                        "llamacpp": {
                            "npm": "@ai-sdk/openai-compatible",
                            "name": "llama.cpp",
                            "options": {
                                "baseURL": LLAMA_CPP_URL.rstrip("/") + "/v1",
                                "apiKey": "not-required",
                            },
                            "models": {
                                LLAMA_CPP_MODEL: {
                                    "name": f"llama.cpp / {LLAMA_CPP_MODEL}"
                                },
                            },
                        }
                    },
                },
                indent=2,
            )
        )

    async for chunk in _stream_exec(
        "opencode",
        "run",
        task,
        f"--model=llamacpp/{LLAMA_CPP_MODEL}",
        "--dangerously-skip-permissions",
    ):
        yield chunk


async def _stream_claude(task: str, model: Optional[str] = None) -> AsyncIterator[dict]:
    model = model or CLAUDE_DEFAULT_MODEL
    async for chunk in _stream_exec(
        "claude", "--dangerously-skip-permissions", "-p", task, "--model", model
    ):
        yield chunk


async def _stream_exec(*cmd: str) -> AsyncIterator[dict]:
    logger.info("Running: %s", " ".join(cmd))
    proc = await asyncio.create_subprocess_exec(
        *cmd,
        stdout=asyncio.subprocess.PIPE,
        stderr=asyncio.subprocess.STDOUT,  # merge stderr so all output appears in docker logs
        cwd=str(WORKSPACE),
    )

    try:
        async with asyncio.timeout(TIMEOUT):
            async for raw in proc.stdout:
                line = raw.decode(errors="replace")
                logger.info("[agent] %s", line.rstrip())
                yield {"line": line}
    except asyncio.TimeoutError:
        proc.kill()
        yield {"done": True, "exit_code": -1, "error": f"Timed out after {TIMEOUT}s."}
        return

    await proc.wait()
    logger.info("Agent exited with code %d", proc.returncode)
    yield {"done": True, "exit_code": proc.returncode}
