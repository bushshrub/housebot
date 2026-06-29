"""Per-user conversation history stored as JSONL files."""

import json
import os
from pathlib import Path
from typing import Any

import aiofiles

HISTORY_DIR = Path(os.getenv("DATA_DIR", "data")) / "history"
MAX_TURNS = int(os.getenv("MAX_HISTORY_TURNS", "30"))


def _history_path(user_id: int | str) -> Path:
    HISTORY_DIR.mkdir(parents=True, exist_ok=True)
    return HISTORY_DIR / f"{user_id}.jsonl"


async def load(user_id: int | str) -> list[dict[str, Any]]:
    path = _history_path(user_id)
    if not path.exists():
        return []

    messages: list[dict[str, Any]] = []
    async with aiofiles.open(path, "r") as f:
        async for line in f:
            line = line.strip()
            if line:
                messages.append(json.loads(line))

    # Keep only the last MAX_TURNS pairs of (user, assistant) messages
    # Each turn is 2 messages; trim from the front
    cutoff = MAX_TURNS * 2
    return messages[-cutoff:] if len(messages) > cutoff else messages


async def save(user_id: int | str, messages: list[dict[str, Any]]) -> None:
    path = _history_path(user_id)
    # Rewrite the whole file — simpler than appending and avoids drift
    async with aiofiles.open(path, "w") as f:
        for msg in messages:
            await f.write(json.dumps(msg) + "\n")


async def append_turn(
    user_id: int | str,
    user_message: dict[str, Any],
    assistant_messages: list[dict[str, Any]],
) -> list[dict[str, Any]]:
    """Load history, append the new turn, trim, and save. Returns updated messages."""
    history = await load(user_id)
    history.append(user_message)
    history.extend(assistant_messages)

    cutoff = MAX_TURNS * 2
    if len(history) > cutoff:
        history = history[-cutoff:]

    await save(user_id, history)
    return history
