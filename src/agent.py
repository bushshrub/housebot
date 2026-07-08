"""Agent using OpenAI-compatible API (llama.cpp) with MCP tool integration."""

import asyncio
import contextvars
import json
import logging
import os
from collections.abc import Awaitable, Callable
from contextlib import AsyncExitStack
from dataclasses import dataclass, field
from datetime import datetime
from typing import Any

import openai
import sentry_sdk
from openai import AsyncOpenAI
from mcp import ClientSession
from mcp.client.stdio import StdioServerParameters, stdio_client

# Per-task hook context so concurrent message handling doesn't trample
_approval_hook_cv = contextvars.ContextVar("approval_hook", default=None)
_progress_hook_cv = contextvars.ContextVar("progress_hook", default=None)
_tool_notification_hook_cv = contextvars.ContextVar("tool_notification_hook", default=None)
_text_stream_hook_cv = contextvars.ContextVar("text_stream_hook", default=None)

from . import history, memory, skills
from .tools.opencode import TOOL_DEFINITION as OPENCODE_TOOL, ProgressCallback, run_opencode
from .tools.feature_request import TOOL_DEFINITION as FEATURE_REQUEST_TOOL, create_feature_request
from .tools.remind import TOOL_DEFINITION as REMIND_TOOL, create_reminder
from .tools.summarize_url import TOOL_DEFINITION as SUMMARIZE_URL_TOOL, fetch_and_summarize
from .tools.translate import TOOL_DEFINITION as TRANSLATE_TOOL, translate_text

ApprovalCallback = Callable[[str, dict[str, Any]], Awaitable[bool]]
ToolNotificationCallback = Callable[[str, dict[str, Any]], Awaitable[None]]
TextStreamCallback = Callable[[str], Awaitable[None]]

logger = logging.getLogger(__name__)

LLM_BASE_URL = os.getenv("LLM_BASE_URL", "http://server-slop:8080/v1")
LLM_MODEL = os.getenv("LLM_MODEL", "gemma-4-12b-qat-q4kxl")
LLM_API_KEY = os.getenv("LLM_API_KEY", "not-required")
OWNER_ID = int(os.getenv("OWNER_DISCORD_ID", "0"))
MAX_CONTEXT_CHARS = int(os.getenv("MAX_CONTEXT_CHARS", "40000"))


@dataclass
class AgentResult:
    text: str
    artifact_paths: list[str] = field(default_factory=list)


@dataclass
class _ToolCall:
    id: str
    name: str
    arguments: str


@dataclass
class _ToolResult:
    content: str
    memory_update: str | None = None
    artifact_paths: list[str] = field(default_factory=list)


@dataclass
class _ToolExecResult:
    tool_call_id: str
    content: str
    memory_update: str | None = None
    artifact_paths: list[str] = field(default_factory=list)

    def to_message(self) -> dict[str, Any]:
        return {"role": "tool", "tool_call_id": self.tool_call_id, "content": self.content}


class Agent:
    """Manages MCP connections and runs the agentic loop via OpenAI-compatible API."""

    def __init__(self) -> None:
        self._exit_stack = AsyncExitStack()
        self._mcp_sessions: list[tuple[str, ClientSession]] = []
        self._client = AsyncOpenAI(base_url=LLM_BASE_URL, api_key=LLM_API_KEY)

    async def start(self) -> None:
        await self._exit_stack.__aenter__()
        for name, params in _mcp_server_configs():
            try:
                read, write = await self._exit_stack.enter_async_context(
                    stdio_client(params)
                )
                session = await self._exit_stack.enter_async_context(
                    ClientSession(read, write)
                )
                await session.initialize()
                self._mcp_sessions.append((name, session))
                logger.info("MCP server '%s' ready", name)
            except Exception:
                logger.exception("Failed to start MCP server '%s'", name)

    async def stop(self) -> None:
        await self._exit_stack.__aexit__(None, None, None)

    async def start_new_session(self, user_id: int | str) -> None:
        """Summarize the previous conversation into memory and clear history."""
        past_messages = await history.load(user_id)
        if not past_messages:
            return

        user_memory = await memory.load(user_id)
        convo_text = "\n".join(
            f"{m['role'].upper()}: {m['content'] if isinstance(m['content'], str) else '[media]'}"
            for m in past_messages
            if isinstance(m.get("content"), str)
        )

        prompt = (
            "The following is a conversation that has ended. "
            "Write a concise bullet-point summary of the key facts, preferences, and decisions "
            "discussed. This will be appended to the user's memory for future reference. "
            "Be brief — 3-8 bullets max.\n\n"
            f"CONVERSATION:\n{convo_text[:6000]}"
        )
        try:
            response = await self._client.chat.completions.create(
                model=LLM_MODEL,
                messages=[{"role": "user", "content": prompt}],
                max_tokens=512,
            )
            summary = response.choices[0].message.content or ""
        except Exception:
            logger.exception("Failed to summarize conversation for user %s", user_id)
            summary = ""

        if summary:
            now = datetime.now().strftime("%Y-%m-%d %H:%M")
            updated_memory = (
                (user_memory.rstrip() + "\n\n" if user_memory.strip() else "")
                + f"## Conversation summary ({now})\n{summary}"
            )
            await memory.save(user_id, updated_memory)

        await history.clear(user_id)
        logger.info("New session started for user %s — history cleared, summary saved", user_id)

    async def run(
        self,
        user_id: int | str,
        username: str,
        text: str,
        image_data: list[dict[str, str]] | None = None,
        *,
        approval_hook: ApprovalCallback | None = None,
        progress_hook: ProgressCallback | None = None,
        tool_notification_hook: ToolNotificationCallback | None = None,
        text_stream_hook: TextStreamCallback | None = None,
    ) -> AgentResult:
        # Set per-task hook context so concurrent messages don't trample
        prev_approval = _approval_hook_cv.set(approval_hook)
        prev_progress = _progress_hook_cv.set(progress_hook)
        prev_tool_notification = _tool_notification_hook_cv.set(tool_notification_hook)
        prev_text_stream = _text_stream_hook_cv.set(text_stream_hook)
        try:
            return await self._run_inner(user_id, username, text, image_data)
        finally:
            _approval_hook_cv.reset(prev_approval)
            _progress_hook_cv.reset(prev_progress)
            _tool_notification_hook_cv.reset(prev_tool_notification)
            _text_stream_hook_cv.reset(prev_text_stream)

    async def _run_inner(
        self,
        user_id: int | str,
        username: str,
        text: str,
        image_data: list[dict[str, str]] | None = None,
    ) -> AgentResult:
        user_memory = await memory.load(user_id)
        past_messages = await history.load(user_id)

        if past_messages:
            context_chars = 0
            for m in past_messages:
                c = m.get("content")
                context_chars += len(c) if isinstance(c, str) else (len(json.dumps(c)) if c is not None else 0)
            if context_chars > MAX_CONTEXT_CHARS:
                logger.info(
                    "Context overflow for user %s (%d chars) — auto-summarizing session",
                    user_id,
                    context_chars,
                )
                await self.start_new_session(user_id)
                past_messages = []
                user_memory = await memory.load(user_id)

        all_skills = await skills.load_all()

        system_message: dict[str, Any] = {
            "role": "system",
            "content": _build_system_prompt(username, user_id, user_memory, all_skills),
        }

        # Build user message — images use OpenAI's image_url format
        if image_data:
            content: list[dict[str, Any]] = []
            for img in image_data:
                content.append({
                    "type": "image_url",
                    "image_url": {
                        "url": f"data:{img['media_type']};base64,{img['data']}"
                    },
                })
            content.append({"type": "text", "text": text})
            new_user_message: dict[str, Any] = {"role": "user", "content": content}
        else:
            new_user_message = {"role": "user", "content": text}

        messages: list[dict[str, Any]] = [system_message] + past_messages + [new_user_message]
        tools = await self._build_tools()

        turn_messages: list[dict[str, Any]] = []
        final_text = ""
        all_artifacts: list[str] = []

        while True:
            kwargs: dict[str, Any] = {
                "model": LLM_MODEL,
                "messages": messages,
                "max_tokens": 4096,
                "stream": True,
            }
            if tools:
                kwargs["tools"] = tools
                kwargs["tool_choice"] = "auto"

            text_stream_hook = _text_stream_hook_cv.get()

            # Accumulate streaming response
            content_parts: list[str] = []
            tool_calls_acc: dict[int, dict[str, Any]] = {}
            finish_reason: str | None = None

            with sentry_sdk.start_span(op="llm.completion", name=f"LLM/{LLM_MODEL}") as llm_span:
                llm_span.set_data("model", LLM_MODEL)
                llm_span.set_data("message_count", len(messages))
                llm_span.set_data("has_tools", bool(tools))
                try:
                    stream = await self._client.chat.completions.create(**kwargs)  # type: ignore[arg-type]
                except openai.APIConnectionError:
                    logger.warning("LLM API connection error, retrying once...")
                    stream = await self._client.chat.completions.create(**kwargs)  # type: ignore[arg-type]

                async for chunk in stream:
                    ch_choice = chunk.choices[0]
                    if ch_choice.finish_reason:
                        finish_reason = ch_choice.finish_reason
                    delta = ch_choice.delta
                    if delta.content:
                        content_parts.append(delta.content)
                        if text_stream_hook:
                            await text_stream_hook("".join(content_parts))
                    if delta.tool_calls:
                        for tc_delta in delta.tool_calls:
                            idx = tc_delta.index
                            if idx not in tool_calls_acc:
                                tool_calls_acc[idx] = {"id": "", "name": "", "arguments": ""}
                            if tc_delta.id:
                                tool_calls_acc[idx]["id"] = tc_delta.id
                            if tc_delta.function:
                                if tc_delta.function.name:
                                    tool_calls_acc[idx]["name"] += tc_delta.function.name
                                if tc_delta.function.arguments:
                                    tool_calls_acc[idx]["arguments"] += tc_delta.function.arguments

                llm_span.set_data("finish_reason", finish_reason)
                llm_span.set_data("tool_calls_count", len(tool_calls_acc))

            content_text = "".join(content_parts) or None
            tool_calls = [
                _ToolCall(id=v["id"], name=v["name"], arguments=v["arguments"])
                for v in (tool_calls_acc[i] for i in sorted(tool_calls_acc))
            ] if tool_calls_acc else None

            assistant_message: dict[str, Any] = {"role": "assistant", "content": content_text}
            if tool_calls:
                assistant_message["tool_calls"] = [
                    {
                        "id": tc.id,
                        "type": "function",
                        "function": {"name": tc.name, "arguments": tc.arguments},
                    }
                    for tc in tool_calls
                ]

            messages.append(assistant_message)
            turn_messages.append(assistant_message)

            if finish_reason == "stop" or not tool_calls:
                final_text = content_text or ""
                break

            if finish_reason == "tool_calls":
                tool_names = [tc.name for tc in tool_calls]
                with sentry_sdk.start_span(op="agent.tools", name=f"tools/{','.join(tool_names)}") as tools_span:
                    tools_span.set_data("tools", tool_names)
                    tool_results = await self._execute_tools(tool_calls, user_id, user_memory)
                for r in tool_results:
                    if r.memory_update is not None:
                        user_memory = r.memory_update
                    all_artifacts.extend(r.artifact_paths)
                msgs = [r.to_message() for r in tool_results]
                messages.extend(msgs)
                turn_messages.extend(msgs)
            else:
                final_text = content_text or ""
                break

        try:
            await history.append_turn(user_id, new_user_message, turn_messages)
        except Exception:
            logger.exception("Failed to save history for user %s", user_id)
        return AgentResult(text=final_text or "(no response)", artifact_paths=all_artifacts)

    async def _build_tools(self) -> list[dict[str, Any]]:
        tools: list[dict[str, Any]] = []
        for name, session in self._mcp_sessions:
            try:
                result = await session.list_tools()
                for tool in result.tools:
                    tools.append(_to_openai_tool(
                        name=f"{name}__{tool.name}",
                        description=tool.description or "",
                        parameters=tool.inputSchema,
                    ))
            except Exception:
                logger.exception("Failed to list tools for MCP server '%s'", name)

        tools.append(_to_openai_tool(**_flatten_tool(OPENCODE_TOOL)))
        tools.append(_to_openai_tool(**_flatten_tool(_update_memory_tool())))
        tools.append(_to_openai_tool(**_flatten_tool(_run_skill_tool())))
        tools.append(_to_openai_tool(**_flatten_tool(FEATURE_REQUEST_TOOL)))
        tools.append(_to_openai_tool(**_flatten_tool(REMIND_TOOL)))
        tools.append(_to_openai_tool(**_flatten_tool(SUMMARIZE_URL_TOOL)))
        tools.append(_to_openai_tool(**_flatten_tool(TRANSLATE_TOOL)))
        return tools

    async def _execute_tools(
        self,
        tool_calls: list[_ToolCall],
        user_id: int | str,
        user_memory: str,
    ) -> list[_ToolExecResult]:
        async def run_one(tc: _ToolCall) -> _ToolExecResult:
            try:
                args = json.loads(tc.arguments)
                logger.info("Tool call: %s args=%s", tc.name, json.dumps(args)[:200])
                tool_notification_hook = _tool_notification_hook_cv.get()
                if tool_notification_hook is not None:
                    await tool_notification_hook(tc.name, args)
                result = await self._dispatch_tool(tc.name, args, user_id, user_memory)
                if result.content.startswith("Error:"):
                    logger.error("Tool '%s' returned error: %s", tc.name, result.content)
                    sentry_sdk.capture_message(
                        f"Tool error [{tc.name}]: {result.content}",
                        level="error",
                    )
                return _ToolExecResult(
                    tool_call_id=tc.id,
                    content=result.content,
                    memory_update=result.memory_update,
                    artifact_paths=result.artifact_paths,
                )
            except Exception as exc:
                logger.exception("Tool '%s' raised an exception", tc.name)
                sentry_sdk.capture_exception(exc)
                return _ToolExecResult(tool_call_id=tc.id, content=f"Error: {exc}")

        return list(await asyncio.gather(*[run_one(tc) for tc in tool_calls]))

    async def _dispatch_tool(
        self,
        name: str,
        args: dict[str, Any],
        user_id: int | str,
        user_memory: str,
    ) -> _ToolResult:
        if name == "run_opencode":
            task = args.get("task")
            if not task:
                return _ToolResult(
                    content="Error: 'task' argument is required for run_opencode. Please provide a description of the coding task to perform."
                )
            raw = await run_opencode(
                task=task,
                model=args.get("model"),
                repo_url=args.get("repo_url"),
                files=args.get("files"),
                on_progress=_progress_hook_cv.get(),
            )
            if isinstance(raw, dict):
                return _ToolResult(
                    content=raw.get("content", ""),
                    artifact_paths=raw.get("_artifact_paths", []),
                )
            return _ToolResult(content=str(raw))

        if name == "update_memory":
            new_content = args["memory_content"]
            await memory.save(user_id, new_content)
            return _ToolResult(content="Memory updated.", memory_update=new_content)

        if name == "create_feature_request":
            content = await create_feature_request(
                title=args["title"],
                description=args["description"],
                requested_by=str(user_id),
            )
            return _ToolResult(content=content)

        if name == "set_reminder":
            content = await create_reminder(
                user_id=str(user_id),
                message=args["message"],
                delay_minutes=float(args["delay_minutes"]),
            )
            return _ToolResult(content=content)

        if name == "summarize_url":
            content = await fetch_and_summarize(
                url=args["url"],
                llm_client=self._client,
                model=LLM_MODEL,
            )
            return _ToolResult(content=content)

        if name == "translate":
            content = await translate_text(
                text=args["text"],
                target_language=args["target_language"],
                llm_client=self._client,
                model=LLM_MODEL,
            )
            return _ToolResult(content=content)

        if name == "run_skill":
            skill_name = args["name"]
            skill_input = args.get("input", "")
            skill = await skills.get(skill_name)
            if skill is None:
                return _ToolResult(content=f"Error: Skill '{skill_name}' not found.")
            response = await self._client.chat.completions.create(
                model=LLM_MODEL,
                messages=[
                    {"role": "system", "content": skill["prompt"]},
                    {"role": "user", "content": skill_input},
                ],
                max_tokens=4096,
            )
            return _ToolResult(content=response.choices[0].message.content or "")

        # MCP tools — name format: "<server_prefix>__<tool_name>"
        if "__" in name:
            prefix, tool_name = name.split("__", 1)
            for server_prefix, session in self._mcp_sessions:
                if server_prefix == prefix:
                    result = await session.call_tool(tool_name, args)
                    parts = [
                        item.text if hasattr(item, "text") else str(item)
                        for item in result.content
                    ]
                    return _ToolResult(content="\n".join(parts))

        return _ToolResult(content=f"Unknown tool: {name}")


# ── helpers ──────────────────────────────────────────────────────────────────

def _mcp_server_configs() -> list[tuple[str, StdioServerParameters]]:
    configs: list[tuple[str, StdioServerParameters]] = []

    configs.append(("ddg", StdioServerParameters(command="duckduckgo-mcp-server")))

    jellyfin_url = os.getenv("JELLYFIN_URL")
    jellyfin_key = os.getenv("JELLYFIN_API_KEY")
    if jellyfin_url and jellyfin_key:
        configs.append((
            "jellyfin",
            StdioServerParameters(
                command="jellyfin-mcp",
                args=["--read-only"],
                env={"JELLYFIN_URL": jellyfin_url, "JELLYFIN_API_KEY": jellyfin_key},
            ),
        ))
    else:
        logger.warning("JELLYFIN_URL or JELLYFIN_API_KEY not set — Jellyfin MCP disabled")

    return configs


def _build_system_prompt(
    username: str,
    user_id: int | str,
    user_memory: str,
    all_skills: dict[str, Any] | None = None,
) -> str:
    now = datetime.now().strftime("%Y-%m-%d %H:%M")
    memory_section = (
        f"\n\n## Your memory about {username}\n{user_memory}" if user_memory.strip() else ""
    )
    if all_skills:
        skill_lines = "\n".join(
            f"  - **{s['name']}**: {s.get('description', s['name'])}"
            for s in all_skills.values()
        )
        skills_section = f"\n- run_skill — Execute a custom skill by name with an input string. Available skills:\n{skill_lines}"
    else:
        skills_section = "\n- run_skill — Execute a custom skill by name. No skills are defined yet; users can add them with `!skill add`."
    return f"""\
You are a helpful house assistant bot in a Discord server. You help with media, web search, and software development tasks.

Current date/time: {now}
Current user: {username} (ID: {user_id}){memory_section}

## Tools
- ddg__* — Search the web via DuckDuckGo for current information.
- jellyfin__* — Query the household Jellyfin media server for movies, shows, music. READ ONLY — only call get_* / search_* / list_* methods; never call create_*, add_*, remove_*, update_*, revoke_*, restore_*, or any mutating action.
- run_opencode — Run a coding task using OpenCode + local llama.cpp model. Good for quick scripts and general work.
- update_memory — Persist important facts about the current user for future conversations. Write the full memory each time.
- create_feature_request — File a GitHub issue for a feature the user wants added to this bot. Use whenever a user asks for a new feature or improvement.
- set_reminder — Set a timed reminder; the bot will DM the user when the delay elapses. Use whenever a user asks to be reminded about something.
- summarize_url — Fetch a public web URL and return a concise summary. Use when the user shares a link or wants to read a page.
- translate — Translate text to any language using the LLM. Use whenever a user asks to translate something.{skills_section}

## Guidelines
- Be conversational and friendly.
- Use Jellyfin tools for any media questions before guessing.
- Use DuckDuckGo for factual or current-events questions.
- For ANY programming or coding task — including trivial one-liners, scripts, debugging, code review, or anything that involves writing or analyzing code — immediately use run_opencode. Never write or analyze code yourself in your response. Always delegate to the tool.
- After a coding tool runs, give a brief summary of what was done. Do NOT reproduce the full code or script in your reply — it will be sent as a file attachment automatically if it's large.
- Update memory when you learn something worth remembering.
- Keep responses concise unless asked for detail.
- If a user requests a feature or improvement to this bot, immediately call create_feature_request with a clear title and description, then tell them the issue URL.
- If a tool returns an error message (starts with "Error:"), quote it exactly — do not paraphrase or soften it.
- When the user's message exceeds 500 characters, begin your reply with a **TL;DR:** line (one sentence) summarizing what they asked.
"""


def _update_memory_tool() -> dict[str, Any]:
    return {
        "name": "update_memory",
        "description": (
            "Update your persistent memory about the current user. "
            "Write the complete updated memory content each time, not just the new piece."
        ),
        "input_schema": {
            "type": "object",
            "properties": {
                "memory_content": {
                    "type": "string",
                    "description": "Full updated memory in markdown format.",
                }
            },
            "required": ["memory_content"],
        },
    }


def _run_skill_tool() -> dict[str, Any]:
    return {
        "name": "run_skill",
        "description": (
            "Execute a named skill — a custom prompt template saved by users. "
            "Pass the skill name and the text input to process."
        ),
        "input_schema": {
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "The skill name to execute.",
                },
                "input": {
                    "type": "string",
                    "description": "The text input to pass to the skill.",
                },
            },
            "required": ["name", "input"],
        },
    }


def _to_openai_tool(name: str, description: str, parameters: dict[str, Any]) -> dict[str, Any]:
    """Wrap a tool definition in OpenAI's function-calling envelope."""
    return {
        "type": "function",
        "function": {
            "name": name,
            "description": description,
            "parameters": parameters,
        },
    }


def _flatten_tool(tool_def: dict[str, Any]) -> dict[str, Any]:
    """Convert our internal tool format (input_schema) to kwargs for _to_openai_tool."""
    return {
        "name": tool_def["name"],
        "description": tool_def.get("description", ""),
        "parameters": tool_def.get("input_schema", tool_def.get("parameters", {})),
    }
