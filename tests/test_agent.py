"""Tests for pure helpers in src/agent.py."""

import pytest

from src.agent import _build_system_prompt, _flatten_tool, _to_openai_tool


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
