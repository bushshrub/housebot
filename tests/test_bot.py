"""Tests for pure helpers in src/bot.py."""

import os
import pytest
from unittest.mock import AsyncMock, MagicMock, patch
import discord

from src.bot import _split_text, _tool_hint, _send_final_message, _send_long_message, _extract_code_files


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


class TestExtractCodeFiles:
    def test_short_code_block_not_extracted(self):
        text = "Here:\n```python\nprint('hi')\n```"
        modified, files = _extract_code_files(text)
        assert files == []
        assert "```" in modified

    def test_large_code_block_extracted(self):
        code = "x = 1\n" * 200  # well over 800 chars
        text = f"Here:\n```python\n{code}```"
        modified, files = _extract_code_files(text)
        assert len(files) == 1
        filename, content = files[0]
        assert filename == "script_1.py"
        assert content == code.encode()
        assert "```" not in modified
        assert "script_1.py" in modified

    def test_extension_inferred_from_language(self):
        code = "echo hi\n" * 150
        text = f"```bash\n{code}```"
        _, files = _extract_code_files(text)
        assert files[0][0].endswith(".sh")

    def test_unknown_language_gets_txt_extension(self):
        code = "blah\n" * 200
        text = f"```brainfuck\n{code}```"
        _, files = _extract_code_files(text)
        assert files[0][0].endswith(".txt")

    def test_unclosed_code_block_still_extracted(self):
        code = "x = 1\n" * 200
        text = f"```python\n{code}"  # no closing ```
        modified, files = _extract_code_files(text)
        assert len(files) == 1
        assert "script_1.py" in modified

    def test_multiple_large_blocks_numbered(self):
        code = "x = 1\n" * 200
        text = f"```python\n{code}```\n```bash\n{code}```"
        _, files = _extract_code_files(text)
        assert len(files) == 2
        assert files[0][0] == "script_1.py"
        assert files[1][0] == "script_2.sh"

    def test_mixed_small_and_large_blocks(self):
        small = "print('hi')\n"
        large = "x = 1\n" * 200
        text = f"```python\n{small}```\n```python\n{large}```"
        modified, files = _extract_code_files(text)
        assert len(files) == 1
        assert "script_1.py" in modified
        assert "```python" in modified  # small block left inline


class TestRedactSecrets:
    def test_known_secret_is_redacted(self):
        import importlib
        import src.bot as bot_module

        fake_token = "super-secret-token-abc123xyz"
        with patch.dict(os.environ, {"MY_SECRET_TOKEN": fake_token}):
            # Rebuild patterns with the fake secret in env
            bot_module._SECRET_PATTERNS.clear()
            bot_module._build_secret_patterns()
            result = bot_module._redact_secrets(f"The token is {fake_token}")
            assert fake_token not in result
            assert "[REDACTED]" in result

    def test_non_secret_env_var_not_redacted(self):
        import src.bot as bot_module

        with patch.dict(os.environ, {"MY_NAME": "alice"}):
            bot_module._SECRET_PATTERNS.clear()
            bot_module._build_secret_patterns()
            result = bot_module._redact_secrets("hello alice")
            assert result == "hello alice"

    def test_short_value_not_redacted(self):
        import src.bot as bot_module

        with patch.dict(os.environ, {"MY_TOKEN": "abc"}):
            bot_module._SECRET_PATTERNS.clear()
            bot_module._build_secret_patterns()
            result = bot_module._redact_secrets("abc")
            assert result == "abc"

    def test_multiple_secrets_all_redacted(self):
        import src.bot as bot_module

        token = "discord-token-xyz987"
        api_key = "jellyfin-api-key-456def"
        with patch.dict(os.environ, {"BOT_TOKEN": token, "JELLYFIN_API_KEY": api_key}):
            bot_module._SECRET_PATTERNS.clear()
            bot_module._build_secret_patterns()
            result = bot_module._redact_secrets(f"token={token} key={api_key}")
            assert token not in result
            assert api_key not in result
            assert result.count("[REDACTED]") == 2

    def test_text_without_secrets_unchanged(self):
        import src.bot as bot_module

        original_patterns = bot_module._SECRET_PATTERNS[:]
        result = bot_module._redact_secrets("hello world, no secrets here")
        assert result == "hello world, no secrets here"
        bot_module._SECRET_PATTERNS[:] = original_patterns
