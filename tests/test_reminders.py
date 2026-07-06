"""Tests for src/reminders.py."""

import time
import pytest

import src.reminders as reminders_mod


@pytest.fixture(autouse=True)
def patch_reminders_path(tmp_path, monkeypatch):
    monkeypatch.setattr(reminders_mod, "REMINDERS_PATH", tmp_path / "reminders.json")


class TestAddAndLoad:
    async def test_add_creates_file(self, tmp_path):
        await reminders_mod.add("123", "hello", time.time() + 60)
        assert reminders_mod.REMINDERS_PATH.exists()

    async def test_load_empty_when_no_file(self):
        result = await reminders_mod._load()
        assert result == []

    async def test_add_stores_fields(self):
        due = time.time() + 100
        await reminders_mod.add("42", "test message", due)
        loaded = await reminders_mod._load()
        assert len(loaded) == 1
        r = loaded[0]
        assert r["user_id"] == "42"
        assert r["message"] == "test message"
        assert r["due_ts"] == due

    async def test_multiple_reminders_stored(self):
        await reminders_mod.add("1", "first", time.time() + 60)
        await reminders_mod.add("2", "second", time.time() + 120)
        loaded = await reminders_mod._load()
        assert len(loaded) == 2


class TestPopDue:
    async def test_returns_due_reminders(self):
        past = time.time() - 10
        future = time.time() + 100
        await reminders_mod.add("1", "past", past)
        await reminders_mod.add("2", "future", future)

        due = await reminders_mod.pop_due(time.time())

        assert len(due) == 1
        assert due[0]["message"] == "past"

    async def test_removes_due_from_file(self):
        past = time.time() - 5
        await reminders_mod.add("1", "old", past)

        await reminders_mod.pop_due(time.time())

        remaining = await reminders_mod._load()
        assert remaining == []

    async def test_future_reminders_not_removed(self):
        future = time.time() + 3600
        await reminders_mod.add("1", "later", future)

        due = await reminders_mod.pop_due(time.time())

        assert due == []
        remaining = await reminders_mod._load()
        assert len(remaining) == 1

    async def test_empty_store_returns_empty(self):
        due = await reminders_mod.pop_due(time.time())
        assert due == []

    async def test_all_due_cleared(self):
        now = time.time()
        await reminders_mod.add("1", "a", now - 3)
        await reminders_mod.add("2", "b", now - 1)

        due = await reminders_mod.pop_due(now)

        assert len(due) == 2
        assert await reminders_mod._load() == []
