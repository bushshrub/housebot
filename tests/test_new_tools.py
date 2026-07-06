"""Tests for the remind, summarize_url, and translate tools."""

import time
import pytest
from unittest.mock import AsyncMock, MagicMock, patch

import src.reminders as reminders_mod
from src.tools.remind import create_reminder, TOOL_DEFINITION as REMIND_DEF
from src.tools.summarize_url import fetch_and_summarize, TOOL_DEFINITION as SUMMARIZE_DEF
from src.tools.translate import translate_text, TOOL_DEFINITION as TRANSLATE_DEF


@pytest.fixture(autouse=True)
def patch_reminders_path(tmp_path, monkeypatch):
    monkeypatch.setattr(reminders_mod, "REMINDERS_PATH", tmp_path / "reminders.json")


# ── remind tool ──────────────────────────────────────────────────────────────

class TestCreateReminder:
    async def test_returns_confirmation(self):
        result = await create_reminder("42", "feed the cat", 30)
        assert "Reminder set" in result
        assert "30m" in result

    async def test_hours_formatted(self):
        result = await create_reminder("1", "meeting", 90)
        assert "1h 30m" in result

    async def test_exact_hours_no_minutes(self):
        result = await create_reminder("1", "standup", 120)
        assert "2h" in result
        assert "0m" not in result

    async def test_stores_reminder(self):
        before = time.time()
        await create_reminder("7", "test", 10)
        loaded = await reminders_mod._load()
        assert len(loaded) == 1
        r = loaded[0]
        assert r["user_id"] == "7"
        assert r["message"] == "test"
        assert r["due_ts"] >= before + 10 * 60

    async def test_delay_below_minimum_returns_error(self):
        result = await create_reminder("1", "now", 0)
        assert result.startswith("Error:")

    async def test_delay_above_maximum_returns_error(self):
        result = await create_reminder("1", "far future", 99999)
        assert result.startswith("Error:")

    def test_tool_definition_has_required_fields(self):
        assert REMIND_DEF["name"] == "set_reminder"
        props = REMIND_DEF["input_schema"]["properties"]
        assert "message" in props
        assert "delay_minutes" in props
        assert REMIND_DEF["input_schema"]["required"] == ["message", "delay_minutes"]


# ── summarize_url tool ────────────────────────────────────────────────────────

class TestFetchAndSummarize:
    def _mock_llm(self, reply: str) -> MagicMock:
        client = MagicMock()
        response = MagicMock()
        response.choices = [MagicMock(message=MagicMock(content=reply))]
        client.chat.completions.create = AsyncMock(return_value=response)
        return client

    async def test_returns_llm_summary(self):
        client = self._mock_llm("This page is about cats.")

        with patch("src.tools.summarize_url.aiohttp.ClientSession") as mock_session_cls:
            mock_resp = AsyncMock()
            mock_resp.status = 200
            mock_resp.text = AsyncMock(return_value="<html><body>cats content</body></html>")
            mock_session_cls.return_value.__aenter__ = AsyncMock(return_value=mock_session_cls.return_value)
            mock_session_cls.return_value.__aexit__ = AsyncMock(return_value=False)
            mock_session_cls.return_value.get = MagicMock(return_value=MagicMock(
                __aenter__=AsyncMock(return_value=mock_resp),
                __aexit__=AsyncMock(return_value=False),
            ))

            result = await fetch_and_summarize("https://example.com", client, "test-model")

        assert result == "This page is about cats."

    async def test_http_error_returns_error_string(self):
        client = self._mock_llm("ignored")

        with patch("src.tools.summarize_url.aiohttp.ClientSession") as mock_session_cls:
            mock_resp = AsyncMock()
            mock_resp.status = 404
            mock_session_cls.return_value.__aenter__ = AsyncMock(return_value=mock_session_cls.return_value)
            mock_session_cls.return_value.__aexit__ = AsyncMock(return_value=False)
            mock_session_cls.return_value.get = MagicMock(return_value=MagicMock(
                __aenter__=AsyncMock(return_value=mock_resp),
                __aexit__=AsyncMock(return_value=False),
            ))

            result = await fetch_and_summarize("https://example.com/missing", client, "test-model")

        assert result.startswith("Error:")
        assert "404" in result

    async def test_network_error_returns_error_string(self):
        import aiohttp
        client = self._mock_llm("ignored")

        with patch("src.tools.summarize_url.aiohttp.ClientSession") as mock_session_cls:
            mock_session_cls.return_value.__aenter__ = AsyncMock(return_value=mock_session_cls.return_value)
            mock_session_cls.return_value.__aexit__ = AsyncMock(return_value=False)
            mock_session_cls.return_value.get = MagicMock(return_value=MagicMock(
                __aenter__=AsyncMock(side_effect=aiohttp.ClientError("connection refused")),
                __aexit__=AsyncMock(return_value=False),
            ))

            result = await fetch_and_summarize("https://bad.invalid", client, "test-model")

        assert result.startswith("Error:")

    def test_tool_definition_has_required_fields(self):
        assert SUMMARIZE_DEF["name"] == "summarize_url"
        assert "url" in SUMMARIZE_DEF["input_schema"]["properties"]
        assert SUMMARIZE_DEF["input_schema"]["required"] == ["url"]


# ── translate tool ────────────────────────────────────────────────────────────

class TestTranslateText:
    def _mock_llm(self, reply: str) -> MagicMock:
        client = MagicMock()
        response = MagicMock()
        response.choices = [MagicMock(message=MagicMock(content=reply))]
        client.chat.completions.create = AsyncMock(return_value=response)
        return client

    async def test_returns_translation(self):
        client = self._mock_llm("Bonjour le monde")
        result = await translate_text("Hello world", "French", client, "test-model")
        assert result == "Bonjour le monde"

    async def test_prompt_includes_target_language(self):
        client = self._mock_llm("Hola")
        await translate_text("Hello", "Spanish", client, "test-model")
        call_args = client.chat.completions.create.call_args
        messages = call_args.kwargs["messages"]
        system_content = messages[0]["content"]
        assert "Spanish" in system_content

    async def test_prompt_includes_source_text(self):
        client = self._mock_llm("Ciao")
        await translate_text("Hello there", "Italian", client, "test-model")
        call_args = client.chat.completions.create.call_args
        messages = call_args.kwargs["messages"]
        all_content = " ".join(m["content"] for m in messages)
        assert "Hello there" in all_content

    async def test_empty_llm_response_returns_fallback(self):
        client = MagicMock()
        response = MagicMock()
        response.choices = [MagicMock(message=MagicMock(content=None))]
        client.chat.completions.create = AsyncMock(return_value=response)
        result = await translate_text("hello", "German", client, "test-model")
        assert "no translation" in result

    def test_tool_definition_has_required_fields(self):
        assert TRANSLATE_DEF["name"] == "translate"
        props = TRANSLATE_DEF["input_schema"]["properties"]
        assert "text" in props
        assert "target_language" in props
        assert set(TRANSLATE_DEF["input_schema"]["required"]) == {"text", "target_language"}
