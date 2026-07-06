"""Tests for src/notes.py."""

import pytest

import src.notes as notes_mod


@pytest.fixture(autouse=True)
def patch_notes_dir(tmp_path, monkeypatch):
    monkeypatch.setattr(notes_mod, "NOTES_DIR", tmp_path / "notes")


class TestLoadAll:
    async def test_empty_when_no_file(self):
        result = await notes_mod.load_all(99)
        assert result == {}


class TestSave:
    async def test_save_and_retrieve(self):
        await notes_mod.save(1, "shopping", "milk, eggs")
        result = await notes_mod.load_all(1)
        assert result == {"shopping": "milk, eggs"}

    async def test_save_overwrites_existing(self):
        await notes_mod.save(1, "todo", "old content")
        await notes_mod.save(1, "todo", "new content")
        notes = await notes_mod.load_all(1)
        assert notes["todo"] == "new content"

    async def test_multiple_notes_per_user(self):
        await notes_mod.save(1, "a", "alpha")
        await notes_mod.save(1, "b", "beta")
        notes = await notes_mod.load_all(1)
        assert len(notes) == 2

    async def test_notes_isolated_per_user(self):
        await notes_mod.save(1, "key", "user1")
        await notes_mod.save(2, "key", "user2")
        assert (await notes_mod.load_all(1))["key"] == "user1"
        assert (await notes_mod.load_all(2))["key"] == "user2"


class TestGet:
    async def test_get_existing_note(self):
        await notes_mod.save(1, "hello", "world")
        assert await notes_mod.get(1, "hello") == "world"

    async def test_get_missing_returns_none(self):
        assert await notes_mod.get(1, "missing") is None


class TestDelete:
    async def test_delete_existing(self):
        await notes_mod.save(1, "x", "content")
        deleted = await notes_mod.delete(1, "x")
        assert deleted is True
        assert await notes_mod.get(1, "x") is None

    async def test_delete_missing_returns_false(self):
        result = await notes_mod.delete(1, "nonexistent")
        assert result is False

    async def test_delete_leaves_other_notes(self):
        await notes_mod.save(1, "a", "keep")
        await notes_mod.save(1, "b", "remove")
        await notes_mod.delete(1, "b")
        notes = await notes_mod.load_all(1)
        assert "a" in notes
        assert "b" not in notes
