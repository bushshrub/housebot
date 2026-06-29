"""Tests for src/skills.py."""

import pytest

import src.skills as skills_mod


@pytest.fixture(autouse=True)
def tmp_skills_path(tmp_path, monkeypatch):
    monkeypatch.setattr(skills_mod, "SKILLS_PATH", tmp_path / "skills.json")
    yield tmp_path


async def test_load_all_returns_empty_when_no_file():
    result = await skills_mod.load_all()
    assert result == {}


async def test_save_and_load_skill():
    skill = {"name": "greet", "description": "Say hello", "prompt": "Hello!"}
    await skills_mod.save_skill(skill)
    loaded = await skills_mod.load_all()
    assert "greet" in loaded
    assert loaded["greet"]["prompt"] == "Hello!"


async def test_get_existing_skill():
    skill = {"name": "greet", "description": "Say hello", "prompt": "Hello!"}
    await skills_mod.save_skill(skill)
    result = await skills_mod.get("greet")
    assert result is not None
    assert result["name"] == "greet"


async def test_get_missing_skill_returns_none():
    result = await skills_mod.get("nonexistent")
    assert result is None


async def test_save_skill_overwrites_existing():
    skill_v1 = {"name": "greet", "description": "old", "prompt": "Hi"}
    skill_v2 = {"name": "greet", "description": "new", "prompt": "Hey"}
    await skills_mod.save_skill(skill_v1)
    await skills_mod.save_skill(skill_v2)
    result = await skills_mod.get("greet")
    assert result["description"] == "new"


async def test_delete_existing_skill():
    skill = {"name": "greet", "description": "Say hello", "prompt": "Hello!"}
    await skills_mod.save_skill(skill)
    deleted = await skills_mod.delete_skill("greet")
    assert deleted is True
    assert await skills_mod.get("greet") is None


async def test_delete_missing_skill_returns_false():
    deleted = await skills_mod.delete_skill("nonexistent")
    assert deleted is False


async def test_multiple_skills_coexist():
    await skills_mod.save_skill({"name": "a", "prompt": "A prompt"})
    await skills_mod.save_skill({"name": "b", "prompt": "B prompt"})
    all_skills = await skills_mod.load_all()
    assert "a" in all_skills
    assert "b" in all_skills
