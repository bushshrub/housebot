"""Tests for src/history.py."""

import json
import os
import pytest

import src.history as history_mod


@pytest.fixture(autouse=True)
def tmp_history_dir(tmp_path, monkeypatch):
    monkeypatch.setenv("DATA_DIR", str(tmp_path))
    # Reset module-level HISTORY_DIR so it picks up the patched env var
    monkeypatch.setattr(history_mod, "HISTORY_DIR", tmp_path / "history")
    yield tmp_path


async def test_load_returns_empty_for_unknown_user():
    result = await history_mod.load("unknown_user")
    assert result == []


async def test_save_and_load_roundtrip():
    msgs = [
        {"role": "user", "content": "hello"},
        {"role": "assistant", "content": "hi"},
    ]
    await history_mod.save("user1", msgs)
    loaded = await history_mod.load("user1")
    assert loaded == msgs


async def test_load_respects_max_turns(monkeypatch):
    monkeypatch.setattr(history_mod, "MAX_TURNS", 2)
    msgs = [{"role": "user", "content": str(i)} for i in range(10)]
    await history_mod.save("user1", msgs)
    loaded = await history_mod.load("user1")
    assert loaded == msgs[-4:]  # MAX_TURNS * 2


async def test_append_turn_creates_history():
    user_msg = {"role": "user", "content": "hello"}
    asst_msgs = [{"role": "assistant", "content": "hi"}]
    result = await history_mod.append_turn("user2", user_msg, asst_msgs)
    assert result == [user_msg] + asst_msgs


async def test_append_turn_accumulates():
    user1 = {"role": "user", "content": "first"}
    asst1 = [{"role": "assistant", "content": "reply1"}]
    await history_mod.append_turn("user3", user1, asst1)

    user2 = {"role": "user", "content": "second"}
    asst2 = [{"role": "assistant", "content": "reply2"}]
    result = await history_mod.append_turn("user3", user2, asst2)

    assert result[0] == user1
    assert result[-1] == asst2[0]


async def test_append_turn_trims_to_max_turns(monkeypatch):
    monkeypatch.setattr(history_mod, "MAX_TURNS", 1)
    user1 = {"role": "user", "content": "first"}
    asst1 = [{"role": "assistant", "content": "reply1"}]
    await history_mod.append_turn("user4", user1, asst1)

    user2 = {"role": "user", "content": "second"}
    asst2 = [{"role": "assistant", "content": "reply2"}]
    result = await history_mod.append_turn("user4", user2, asst2)

    assert len(result) == 2  # MAX_TURNS * 2
    assert result[0] == user2
    assert result[1] == asst2[0]
