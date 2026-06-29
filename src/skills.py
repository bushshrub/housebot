"""Global custom skills stored as a JSON file."""

import json
from pathlib import Path
from typing import Any

import aiofiles

SKILLS_PATH = Path("data") / "skills.json"


async def load_all() -> dict[str, Any]:
    if not SKILLS_PATH.exists():
        return {}
    async with aiofiles.open(SKILLS_PATH, "r") as f:
        data = await f.read()
        return json.loads(data) if data.strip() else {}


async def get(name: str) -> dict[str, Any] | None:
    all_skills = await load_all()
    return all_skills.get(name)


async def save_skill(skill: dict[str, Any]) -> None:
    all_skills = await load_all()
    all_skills[skill["name"]] = skill
    SKILLS_PATH.parent.mkdir(parents=True, exist_ok=True)
    async with aiofiles.open(SKILLS_PATH, "w") as f:
        await f.write(json.dumps(all_skills, indent=2))


async def delete_skill(name: str) -> bool:
    all_skills = await load_all()
    if name not in all_skills:
        return False
    del all_skills[name]
    SKILLS_PATH.parent.mkdir(parents=True, exist_ok=True)
    async with aiofiles.open(SKILLS_PATH, "w") as f:
        await f.write(json.dumps(all_skills, indent=2))
    return True
