"""Per-user named notes stored as JSON files."""

import json
import os
from pathlib import Path

import aiofiles

NOTES_DIR = Path(os.getenv("DATA_DIR", "data")) / "notes"


def _path(user_id: int | str) -> Path:
    os.makedirs(NOTES_DIR, exist_ok=True)
    return NOTES_DIR / f"{user_id}.json"


async def load_all(user_id: int | str) -> dict[str, str]:
    path = _path(user_id)
    if not path.exists():
        return {}
    async with aiofiles.open(path, "r") as f:
        data = await f.read()
    return json.loads(data) if data.strip() else {}


async def save(user_id: int | str, name: str, content: str) -> None:
    notes = await load_all(user_id)
    notes[name] = content
    async with aiofiles.open(_path(user_id), "w") as f:
        await f.write(json.dumps(notes, indent=2))


async def get(user_id: int | str, name: str) -> str | None:
    return (await load_all(user_id)).get(name)


async def delete(user_id: int | str, name: str) -> bool:
    notes = await load_all(user_id)
    if name not in notes:
        return False
    del notes[name]
    async with aiofiles.open(_path(user_id), "w") as f:
        await f.write(json.dumps(notes, indent=2))
    return True
