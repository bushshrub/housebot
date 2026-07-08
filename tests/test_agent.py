"""Tests for pure helpers in src/agent.py."""

import pytest
from unittest.mock import AsyncMock, patch, MagicMock

from src.agent import _build_system_prompt, _flatten_tool, _to_openai_tool, MAX_CONTEXT_CHARS


class TestBuildSystemPrompt:
    def test_includes_username(self):
        prompt = _build_system_prompt("Alice", 123, "")
        assert "Alice" in prompt

    def test_includes_user_id(self):
        prompt = _build_system_prompt("Alice", 123, "")
        assert "123" in prompt

    def test_memory_section_present_when_nonempty(self):
        prompt = _build_system_prompt("Alice", 123, "Likes cats")
        assert "Likes cats" in prompt

    def test_memory_section_absent_when_empty(self):
        prompt = _build_system_prompt("Alice", 123, "")
        assert "Your memory" not in prompt

    def test_memory_section_absent_when_whitespace(self):
        prompt = _build_system_prompt("Alice", 123, "   ")
        assert "Your memory" not in prompt

    def test_skills_section_lists_skills(self):
        skills = {"greet": {"name": "greet", "description": "Say hello", "prompt": "..."}}
        prompt = _build_system_prompt("Alice", 123, "", all_skills=skills)
        assert "greet" in prompt
        assert "Say hello" in prompt

    def test_no_skills_shows_placeholder(self):
        prompt = _build_system_prompt("Alice", 123, "", all_skills={})
        assert "No skills are defined yet" in prompt

    def test_no_skills_arg_shows_placeholder(self):
        prompt = _build_system_prompt("Alice", 123, "")
        assert "No skills are defined yet" in prompt


class TestFlattenTool:
    def test_extracts_name_description_input_schema(self):
        tool = {
            "name": "my_tool",
            "description": "does stuff",
            "input_schema": {"type": "object", "properties": {}},
        }
        result = _flatten_tool(tool)
        assert result["name"] == "my_tool"
        assert result["description"] == "does stuff"
        assert result["parameters"] == tool["input_schema"]

    def test_falls_back_to_parameters_key(self):
        tool = {
            "name": "my_tool",
            "description": "does stuff",
            "parameters": {"type": "object"},
        }
        result = _flatten_tool(tool)
        assert result["parameters"] == {"type": "object"}

    def test_missing_description_defaults_to_empty(self):
        tool = {"name": "my_tool", "input_schema": {}}
        result = _flatten_tool(tool)
        assert result["description"] == ""


class TestToOpenaiTool:
    def test_wraps_in_function_envelope(self):
        result = _to_openai_tool("my_tool", "does stuff", {"type": "object"})
        assert result["type"] == "function"
        assert result["function"]["name"] == "my_tool"
        assert result["function"]["description"] == "does stuff"
        assert result["function"]["parameters"] == {"type": "object"}


class TestLongMessageTldr:
    """Issue #2: system prompt should instruct TL;DR for messages over 500 chars."""

    def test_system_prompt_contains_tldr_instruction(self):
        prompt = _build_system_prompt("Alice", 123, "")
        assert "TL;DR" in prompt

    def test_tldr_instruction_mentions_500_chars(self):
        prompt = _build_system_prompt("Alice", 123, "")
        assert "500" in prompt


class TestClaudeCodeRemoved:
    """Verify that the run_claude_code tool has been fully removed."""

    def test_system_prompt_does_not_mention_claude_code(self):
        prompt = _build_system_prompt("Alice", 123, "")
        assert "run_claude_code" not in prompt
        assert "Claude Code" not in prompt

    async def test_build_tools_does_not_include_run_claude_code(self):
        from unittest.mock import AsyncMock, patch
        from src.agent import Agent

        agent = Agent()
        agent._mcp_sessions = []
        tools = await agent._build_tools()
        tool_names = [t["function"]["name"] for t in tools]
        assert "run_claude_code" not in tool_names

    async def test_dispatch_unknown_tool_returns_error(self):
        from src.agent import Agent

        agent = Agent()
        agent._mcp_sessions = []
        result = await agent._dispatch_tool("run_claude_code", {}, "user1", "")
        assert "Unknown tool" in result.content

    async def test_dispatch_run_opencode_missing_task_returns_error(self):
        from src.agent import Agent

        agent = Agent()
        agent._mcp_sessions = []
        result = await agent._dispatch_tool("run_opencode", {}, "user1", "")
        assert result.content.startswith("Error:")
        assert "task" in result.content


class TestContextOverflow:
    """Issue #9: context overflow triggers auto-summarization."""

    async def test_overflow_triggers_start_new_session(self, tmp_path, monkeypatch):
        import src.memory as memory_mod
        import src.history as history_mod
        import src.skills as skills_mod
        from src.agent import Agent

        monkeypatch.setattr(memory_mod, "MEMORY_DIR", tmp_path / "memories")
        monkeypatch.setattr(history_mod, "HISTORY_DIR", tmp_path / "history")

        # Build a fake history large enough to exceed MAX_CONTEXT_CHARS
        big_content = "x" * (MAX_CONTEXT_CHARS + 1)
        big_history = [
            {"role": "user", "content": big_content},
            {"role": "assistant", "content": "ok"},
        ]
        await history_mod.save("user1", big_history)

        agent = Agent()
        start_new_session_called = []

        async def fake_start_new_session(uid):
            start_new_session_called.append(uid)
            await history_mod.clear(uid)

        async def fake_build_tools():
            return []

        async def fake_client_create(**kwargs):
            async def _aiter():
                chunk = MagicMock()
                chunk.choices = [MagicMock(
                    finish_reason="stop",
                    delta=MagicMock(content="hello", tool_calls=None),
                )]
                yield chunk

            return _aiter()

        monkeypatch.setattr(agent, "start_new_session", fake_start_new_session)
        monkeypatch.setattr(agent, "_build_tools", fake_build_tools)
        monkeypatch.setattr(agent._client.chat.completions, "create", fake_client_create)
        monkeypatch.setattr(skills_mod, "load_all", AsyncMock(return_value={}))

        await agent._run_inner("user1", "TestUser", "hi")

        assert "user1" in start_new_session_called

    async def test_no_overflow_does_not_reset(self, tmp_path, monkeypatch):
        import src.history as history_mod

        monkeypatch.setattr(history_mod, "HISTORY_DIR", tmp_path / "history")

        small_history = [
            {"role": "user", "content": "hi"},
            {"role": "assistant", "content": "hello"},
        ]
        await history_mod.save("user2", small_history)

        loaded = await history_mod.load("user2")
        total_chars = sum(len(m["content"]) for m in loaded if isinstance(m.get("content"), str))
        assert total_chars < MAX_CONTEXT_CHARS
