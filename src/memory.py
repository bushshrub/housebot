"""Per-user memory stored as markdown files."""

import os
from pathlib import Path

import aiofiles

MEMORY_DIR = Path(os.getenv("DATA_DIR", "data")) / "memories"


def _memory_path(user_id: int | str) -> Path:
    return MEMORY_DIR / f"{user_id}.md"


async def load(user_id: int | str) -> str:
    path = _memory_path(user_id)
    if not path.exists():
        return ""
    async with aiofiles.open(path, "r") as f:
        return await f.read()


async def save(user_id: int | str, content: str) -> None:
    path = _memory_path(user_id)
    os.makedirs(MEMORY_DIR, exist_ok=True)
    async with aiofiles.open(path, "w") as f:
        await f.write(content)
