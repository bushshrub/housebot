"""Tests for src/memory.py."""

import pytest

import src.memory as memory_mod


@pytest.fixture(autouse=True)
def tmp_memory_dir(tmp_path, monkeypatch):
    monkeypatch.setattr(memory_mod, "MEMORY_DIR", tmp_path / "memories")
    yield tmp_path


async def test_load_returns_empty_for_unknown_user():
    result = await memory_mod.load("unknown")
    assert result == ""


async def test_save_and_load_roundtrip():
    await memory_mod.save("user1", "Likes pizza")
    result = await memory_mod.load("user1")
    assert result == "Likes pizza"


async def test_save_overwrites_previous():
    await memory_mod.save("user1", "Likes pizza")
    await memory_mod.save("user1", "Likes sushi now")
    result = await memory_mod.load("user1")
    assert result == "Likes sushi now"


async def test_users_are_isolated():
    await memory_mod.save("user1", "memory A")
    await memory_mod.save("user2", "memory B")
    assert await memory_mod.load("user1") == "memory A"
    assert await memory_mod.load("user2") == "memory B"


async def test_load_does_not_raise_when_dir_missing(tmp_path, monkeypatch):
    """load() must return "" gracefully when neither the data dir nor its parent exist."""
    deep_dir = tmp_path / "nonexistent_parent" / "memories"
    assert not deep_dir.exists()
    assert not deep_dir.parent.exists()

    monkeypatch.setattr(memory_mod, "MEMORY_DIR", deep_dir)

    result = await memory_mod.load("someuser")
    assert result == ""


async def test_save_creates_parent_directories_when_missing(tmp_path, monkeypatch):
    """Regression: save must also succeed when the directory tree is absent."""
    deep_dir = tmp_path / "also_nonexistent" / "memories"
    monkeypatch.setattr(memory_mod, "MEMORY_DIR", deep_dir)

    await memory_mod.save("someuser", "hello")
    assert (deep_dir / "someuser.md").read_text() == "hello"
