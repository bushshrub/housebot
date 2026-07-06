"""Per-user reminders persisted as a JSON file."""

import json
import os
from pathlib import Path
from typing import Any

import aiofiles

REMINDERS_PATH = Path(os.getenv("DATA_DIR", "data")) / "reminders.json"


async def _load() -> list[dict[str, Any]]:
    if not REMINDERS_PATH.exists():
        return []
    async with aiofiles.open(REMINDERS_PATH, "r") as f:
        data = await f.read()
    return json.loads(data) if data.strip() else []


async def _save(reminders: list[dict[str, Any]]) -> None:
    os.makedirs(REMINDERS_PATH.parent, exist_ok=True)
    async with aiofiles.open(REMINDERS_PATH, "w") as f:
        await f.write(json.dumps(reminders, indent=2))


async def add(user_id: str, message: str, due_ts: float) -> None:
    reminders = await _load()
    reminders.append({"user_id": user_id, "message": message, "due_ts": due_ts})
    await _save(reminders)


async def pop_due(now: float) -> list[dict[str, Any]]:
    """Return and remove all reminders whose due time has passed."""
    reminders = await _load()
    due = [r for r in reminders if r["due_ts"] <= now]
    remaining = [r for r in reminders if r["due_ts"] > now]
    if due:
        await _save(remaining)
    return due
