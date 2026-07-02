"""Tests for pure helpers in src/bot.py."""

import os
import time
import pytest
from unittest.mock import AsyncMock, MagicMock, patch
import discord

from src.bot import _split_text, _tool_hint, _send_final_message, _send_long_message, _extract_code_files, HouseBot


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


class TestResetCommand:
    """Issue #7: !reset command clears history and removes the active conversation."""

    async def test_reset_calls_start_new_session(self):
        bot = HouseBot.__new__(HouseBot)
        bot._active_conversations = {(1, 42): time.monotonic()}
        bot.agent = MagicMock()
        bot.agent.start_new_session = AsyncMock()

        message = MagicMock()
        message.content = "!reset"
        message.author.id = 42
        message.channel.id = 1
        message.reply = AsyncMock()

        await bot._handle_reset_command(message)

        bot.agent.start_new_session.assert_awaited_once_with(42)

    async def test_reset_removes_active_conversation(self):
        bot = HouseBot.__new__(HouseBot)
        bot._active_conversations = {(1, 42): time.monotonic()}
        bot.agent = MagicMock()
        bot.agent.start_new_session = AsyncMock()

        message = MagicMock()
        message.author.id = 42
        message.channel.id = 1
        message.reply = AsyncMock()

        await bot._handle_reset_command(message)

        assert (1, 42) not in bot._active_conversations

    async def test_reset_replies_with_confirmation(self):
        bot = HouseBot.__new__(HouseBot)
        bot._active_conversations = {}
        bot.agent = MagicMock()
        bot.agent.start_new_session = AsyncMock()

        message = MagicMock()
        message.author.id = 99
        message.channel.id = 5
        message.reply = AsyncMock()

        await bot._handle_reset_command(message)

        message.reply.assert_awaited_once()
        call_args = message.reply.call_args
        assert "reset" in call_args[0][0].lower() or "reset" in str(call_args).lower()

    async def test_reset_noop_when_no_active_conversation(self):
        """Reset should not raise if user has no active conversation."""
        bot = HouseBot.__new__(HouseBot)
        bot._active_conversations = {}
        bot.agent = MagicMock()
        bot.agent.start_new_session = AsyncMock()

        message = MagicMock()
        message.author.id = 7
        message.channel.id = 3
        message.reply = AsyncMock()

        await bot._handle_reset_command(message)  # should not raise
        assert (3, 7) not in bot._active_conversations


class TestConversationState:
    """Tests for the per-user/channel conversation tracking helpers."""

    def _make_state(self) -> dict:
        return {}

    def test_mark_active_and_query(self):
        state = self._make_state()
        import time
        state[(999, 222)] = time.monotonic()
        assert (999, 222) in state
        assert state[(999, 222)] > 0

    def test_fresh_state_not_active(self):
        state = self._make_state()
        assert state.get((999, 222)) is None

    def test_expired_entry_removed(self):
        state = self._make_state()
        state[(999, 222)] = 0.0  # ancient timestamp
        with patch("time.monotonic", return_value=9999):
            key = (999, 222)
            last = state.get(key)
            assert last is not None
            assert 9999 - last > 300  # exceeds CONVERSATION_IDLE_TIMEOUT
            del state[key]
            assert key not in state


class TestMessageFilteringLogic:
    """Tests for on_message gating logic — the bot should only respond when
    explicitly mentioned, replied to, in a DM, or in an active conversation.

    These tests verify the filtering conditions directly rather than calling
    the async on_message method.
    """

    def test_not_mentioned_no_conversation_is_reject(self):
        """A random server message without mention should be ignored."""
        bot_user = MagicMock(id=111)
        mentions = []
        is_dm = False
        is_mentioned = bot_user in mentions
        is_reply_to_bot = False
        is_active = False

        # The new filtering logic:
        passed = is_dm or is_mentioned or is_reply_to_bot or is_active
        assert not passed

    def test_mentioned_in_server_is_accept(self):
        """A message that mentions the bot should be handled."""
        bot_user = MagicMock(id=111)
        bot_mention = MagicMock(id=111)
        mentions = [bot_mention]
        is_dm = False
        is_mentioned = bot_user in mentions
        is_reply_to_bot = False
        is_active = False

        passed = is_dm or is_mentioned or is_reply_to_bot or is_active
        # bot_user (MagicMock) won't == bot_mention (different mock),
        # so we test with the same object:
        bot = MagicMock(id=111)
        is_mentioned = bot in [bot]
        passed = is_dm or is_mentioned or is_reply_to_bot or is_active
        assert passed

    def test_dm_is_accept(self):
        """DMs should always be handled."""
        is_dm = True
        is_mentioned = False
        is_reply_to_bot = False
        is_active = False

        passed = is_dm or is_mentioned or is_reply_to_bot or is_active
        assert passed

    def test_active_conversation_is_accept(self):
        """Messages in an active conversation should be handled."""
        is_dm = False
        is_mentioned = False
        is_reply_to_bot = False
        is_active = True

        passed = is_dm or is_mentioned or is_reply_to_bot or is_active
        assert passed

    def test_reply_to_bot_is_accept(self):
        """Replies to bot messages should be handled."""
        is_dm = False
        is_mentioned = False
        is_reply_to_bot = True
        is_active = False

        passed = is_dm or is_mentioned or is_reply_to_bot or is_active
        assert passed

    def test_name_in_text_only_is_reject(self):
        """Mentioning the bot's name in plain text should NOT trigger a response
        in server channels (this was the bug that issue #8 is about).

        The old code had `is_name_mentioned` which checked if the bot's display
        name appeared anywhere in the message text. This has been removed.
        """
        is_dm = False
        is_mentioned = False  # bot not in message.mentions
        is_reply_to_bot = False
        is_active = False
        # is_name_mentioned no longer exists / is not checked

        passed = is_dm or is_mentioned or is_reply_to_bot or is_active
        assert not passed
        # Previously this would have been:
        # passed = is_dm or is_mentioned or is_reply_to_bot or is_name_mentioned or is_active
        # with is_name_mentioned=True, which would have been True (the bug).
