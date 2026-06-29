"""Tests for pure helpers in src/bot.py."""

import pytest
from unittest.mock import AsyncMock, MagicMock, patch
import discord

from src.bot import _split_text, _tool_hint, _send_final_message, _send_long_message


class TestSplitText:
    def test_short_text_returned_as_single_chunk(self):
        assert _split_text("hello") == ["hello"]

    def test_exact_limit_not_split(self):
        text = "a" * 2000
        assert _split_text(text) == [text]

    def test_over_limit_splits_on_newline(self):
        text = "a" * 1900 + "\n" + "b" * 200
        chunks = _split_text(text)
        assert len(chunks) == 2
        assert all(len(c) <= 2000 for c in chunks)
        assert "".join(chunks) == text.replace("\n", "", 1)

    def test_over_limit_no_newline_splits_at_limit(self):
        text = "x" * 2500
        chunks = _split_text(text)
        assert len(chunks) == 2
        assert chunks[0] == "x" * 2000
        assert chunks[1] == "x" * 500

    def test_multiple_chunks(self):
        text = "\n".join(["a" * 1999] * 3)
        chunks = _split_text(text)
        assert len(chunks) == 3
        assert all(len(c) <= 2000 for c in chunks)

    def test_empty_string(self):
        assert _split_text("") == [""]

    def test_custom_limit(self):
        # no newline → splits at byte boundary; "hello\nworld" splits cleanly
        chunks = _split_text("hello\nworld", limit=6)
        assert len(chunks) == 2
        assert chunks[0] == "hello"
        assert chunks[1] == "world"


class TestToolHint:
    def test_run_skill_with_name_and_input(self):
        hint = _tool_hint("run_skill", {"name": "summarize", "input": "some text"})
        assert "summarize" in hint
        assert "some text" in hint

    def test_run_skill_no_name(self):
        hint = _tool_hint("run_skill", {"input": "some text"})
        assert hint == ""

    def test_falls_back_to_query_key(self):
        hint = _tool_hint("ddg__search", {"query": "latest news"})
        assert "latest news" in hint

    def test_falls_back_to_task_key(self):
        hint = _tool_hint("run_opencode", {"task": "write a script"})
        assert "write a script" in hint

    def test_long_value_truncated(self):
        hint = _tool_hint("run_opencode", {"task": "x" * 200})
        assert len(hint) <= 85  # " — " + 80 chars + "…"

    def test_unknown_tool_no_known_key(self):
        hint = _tool_hint("some_tool", {"foo": "bar"})
        assert hint == ""

    def test_multiline_value_flattened(self):
        hint = _tool_hint("run_opencode", {"task": "line1\nline2"})
        assert "\n" not in hint


class TestSendFinalMessage:
    async def test_edits_progress_msg_when_present(self):
        channel = AsyncMock()
        progress_msg = AsyncMock()
        progress_msg.edit = AsyncMock()

        await _send_final_message(channel, "hello", progress_msg=progress_msg)

        progress_msg.edit.assert_called_once_with(content="hello")
        channel.send.assert_not_called()

    async def test_falls_back_to_new_reply_when_edit_fails(self):
        channel = AsyncMock()
        reply_to = AsyncMock()
        reply_to.reply = AsyncMock()
        progress_msg = AsyncMock()
        progress_msg.edit = AsyncMock(side_effect=discord.HTTPException(MagicMock(), "fail"))
        progress_msg.delete = AsyncMock()

        await _send_final_message(channel, "hello", progress_msg=progress_msg, reply_to=reply_to)

        reply_to.reply.assert_called_once_with("hello", mention_author=False)

    async def test_no_progress_msg_sends_reply(self):
        channel = AsyncMock()
        reply_to = AsyncMock()
        reply_to.reply = AsyncMock()

        await _send_final_message(channel, "hello", reply_to=reply_to)

        reply_to.reply.assert_called_once_with("hello", mention_author=False)

    async def test_multi_chunk_sends_overflow_to_channel(self):
        channel = AsyncMock()
        progress_msg = AsyncMock()
        progress_msg.edit = AsyncMock()
        long_text = "a" * 1999 + "\n" + "b" * 200

        await _send_final_message(channel, long_text, progress_msg=progress_msg)

        progress_msg.edit.assert_called_once()
        channel.send.assert_called_once()
