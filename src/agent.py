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
from .tools.claude_code import TOOL_DEFINITION as CLAUDE_CODE_TOOL, run_claude_code
from .tools.feature_request import TOOL_DEFINITION as FEATURE_REQUEST_TOOL, create_feature_request

ApprovalCallback = Callable[[str, dict[str, Any]], Awaitable[bool]]
ToolNotificationCallback = Callable[[str, dict[str, Any]], Awaitable[None]]
TextStreamCallback = Callable[[str], Awaitable[None]]

logger = logging.getLogger(__name__)

LLM_BASE_URL = os.getenv("LLM_BASE_URL", "http://server-slop:8080/v1")
LLM_MODEL = os.getenv("LLM_MODEL", "gemma-4-12b-qat-q4kxl")
LLM_API_KEY = os.getenv("LLM_API_KEY", "not-required")
OWNER_ID = int(os.getenv("OWNER_DISCORD_ID", "0"))


@dataclass
class AgentResult:
    text: str
    artifact_paths: list[str] = field(default_factory=list)


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
                "max_tokens": 8096,
                "stream": True,
            }
            if tools:
                kwargs["tools"] = tools
                kwargs["tool_choice"] = "auto"

            text_stream_hook = _text_stream_hook_cv.get()

            try:
                stream = await self._client.chat.completions.create(**kwargs)  # type: ignore[arg-type]
            except openai.APIConnectionError:
                logger.warning("LLM API connection error, retrying once...")
                stream = await self._client.chat.completions.create(**kwargs)  # type: ignore[arg-type]

            # Accumulate streaming response
            content_parts: list[str] = []
            tool_calls_acc: dict[int, dict[str, Any]] = {}
            finish_reason: str | None = None

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

            content_text = "".join(content_parts) or None
            reconstructed_tool_calls = [
                type("ToolCall", (), {
                    "id": v["id"],
                    "function": type("Fn", (), {
                        "name": v["name"],
                        "arguments": v["arguments"],
                    })(),
                })()
                for v in (tool_calls_acc[i] for i in sorted(tool_calls_acc))
            ] if tool_calls_acc else None

            # Serialize assistant message for history and next API call
            assistant_message: dict[str, Any] = {"role": "assistant", "content": content_text}
            if reconstructed_tool_calls:
                assistant_message["tool_calls"] = [
                    {
                        "id": tc.id,
                        "type": "function",
                        "function": {
                            "name": tc.function.name,
                            "arguments": tc.function.arguments,
                        },
                    }
                    for tc in reconstructed_tool_calls
                ]

            messages.append(assistant_message)
            turn_messages.append(assistant_message)

            if finish_reason == "stop" or not reconstructed_tool_calls:
                final_text = content_text or ""
                break

            if finish_reason == "tool_calls":
                tool_result_messages = await self._execute_tools(
                    reconstructed_tool_calls, user_id, user_memory
                )
                # Check for memory updates and artifacts before appending
                for trm in tool_result_messages:
                    if isinstance(trm, BaseException):
                        continue
                    if "_memory_update" in trm:
                        user_memory = trm.pop("_memory_update")
                    if "_artifact_paths" in trm:
                        all_artifacts.extend(trm.pop("_artifact_paths"))
                messages.extend(m for m in tool_result_messages if isinstance(m, dict))
                turn_messages.extend(m for m in tool_result_messages if isinstance(m, dict))
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
        tools.append(_to_openai_tool(**_flatten_tool(CLAUDE_CODE_TOOL)))
        tools.append(_to_openai_tool(**_flatten_tool(_update_memory_tool())))
        tools.append(_to_openai_tool(**_flatten_tool(_run_skill_tool())))
        tools.append(_to_openai_tool(**_flatten_tool(FEATURE_REQUEST_TOOL)))
        return tools

    async def _execute_tools(
        self,
        tool_calls: list[Any],
        user_id: int | str,
        user_memory: str,
    ) -> list[dict[str, Any]]:
        async def run_one(tc: Any) -> dict[str, Any]:
            try:
                args = json.loads(tc.function.arguments)
                logger.info("Tool call: %s args=%s", tc.function.name, json.dumps(args)[:200])
                tool_notification_hook = _tool_notification_hook_cv.get()
                if tool_notification_hook is not None:
                    await tool_notification_hook(tc.function.name, args)
                result = await self._dispatch_tool(tc.function.name, args, user_id, user_memory)
                if isinstance(result, dict) and "_memory_update" in result:
                    return {
                        "role": "tool",
                        "tool_call_id": tc.id,
                        "content": result["content"],
                        "_memory_update": result["_memory_update"],
                    }
                if isinstance(result, dict) and "_artifact_paths" in result:
                    return {
                        "role": "tool",
                        "tool_call_id": tc.id,
                        "content": result["content"],
                        "_artifact_paths": result["_artifact_paths"],
                    }
                content = str(result)
                if content.startswith("Error:"):
                    logger.error("Tool '%s' returned error: %s", tc.function.name, content)
                    sentry_sdk.capture_message(
                        f"Tool error [{tc.function.name}]: {content}",
                        level="error",
                    )
                return {
                    "role": "tool",
                    "tool_call_id": tc.id,
                    "content": content,
                }
            except Exception as exc:
                logger.exception("Tool '%s' raised an exception", tc.function.name)
                sentry_sdk.capture_exception(exc)
                return {
                    "role": "tool",
                    "tool_call_id": tc.id,
                    "content": f"Error: {exc}",
                }

        try:
            return list(await asyncio.gather(*[run_one(tc) for tc in tool_calls], return_exceptions=True))
        except Exception as exc:
            logger.exception("asyncio.gather in _execute_tools failed")
            return [{"role": "tool", "tool_call_id": tc.id, "content": f"Error: {exc}"} for tc in tool_calls]

    async def _dispatch_tool(
        self,
        name: str,
        args: dict[str, Any],
        user_id: int | str,
        user_memory: str,
    ) -> Any:
        if name == "run_opencode":
            return await run_opencode(
                task=args["task"],
                model=args.get("model"),
                repo_url=args.get("repo_url"),
                files=args.get("files"),
                on_progress=_progress_hook_cv.get(),
            )

        if name == "run_claude_code":
            approval_hook = _approval_hook_cv.get()
            if approval_hook is not None:
                approved = await approval_hook("run_claude_code", args)
                if not approved:
                    return "run_claude_code was not approved by the owner."
            return await run_claude_code(
                task=args["task"],
                model=args.get("model"),
                repo_url=args.get("repo_url"),
                files=args.get("files"),
                on_progress=_progress_hook_cv.get(),
            )

        if name == "update_memory":
            new_content = args["memory_content"]
            await memory.save(user_id, new_content)
            return {"content": "Memory updated.", "_memory_update": new_content}

        if name == "create_feature_request":
            return await create_feature_request(
                title=args["title"],
                description=args["description"],
                requested_by=str(user_id),
            )

        if name == "run_skill":
            skill_name = args["name"]
            skill_input = args.get("input", "")
            skill = await skills.get(skill_name)
            if skill is None:
                return f"Error: Skill '{skill_name}' not found."
            response = await self._client.chat.completions.create(
                model=LLM_MODEL,
                messages=[
                    {"role": "system", "content": skill["prompt"]},
                    {"role": "user", "content": skill_input},
                ],
                max_tokens=4096,
            )
            return response.choices[0].message.content or ""

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
                    return "\n".join(parts)

        return f"Unknown tool: {name}"


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
- run_claude_code — Run a coding task using Claude Code (Anthropic). Best for complex, multi-file, or reasoning-heavy work.
- update_memory — Persist important facts about the current user for future conversations. Write the full memory each time.
- create_feature_request — File a GitHub issue for a feature the user wants added to this bot. Use whenever a user asks for a new feature or improvement.{skills_section}

## Guidelines
- Be conversational and friendly.
- Use Jellyfin tools for any media questions before guessing.
- Use DuckDuckGo for factual or current-events questions.
- For any coding request that goes beyond a trivial one-liner, immediately use run_opencode — don't write code yourself. If in doubt, use it.
- run_claude_code is only available to the bot owner (user ID: {OWNER_ID}). Do not offer or attempt it for any other user.
- Update memory when you learn something worth remembering.
- Keep responses concise unless asked for detail.
- If a user requests a feature or improvement to this bot, immediately call create_feature_request with a clear title and description, then tell them the issue URL.
- If a tool returns an error message (starts with "Error:"), quote it exactly — do not paraphrase or soften it.
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
