"""Agent using OpenAI-compatible API (llama.cpp) with MCP tool integration."""

import asyncio
import json
import logging
import os
from collections.abc import Awaitable, Callable
from contextlib import AsyncExitStack
from dataclasses import dataclass, field
from datetime import datetime
from typing import Any

import openai
from openai import AsyncOpenAI
from mcp import ClientSession
from mcp.client.stdio import StdioServerParameters, stdio_client

from . import history, memory
from .tools.opencode import TOOL_DEFINITION as OPENCODE_TOOL, ProgressCallback, run_opencode
from .tools.claude_code import TOOL_DEFINITION as CLAUDE_CODE_TOOL, run_claude_code

ApprovalCallback = Callable[[str, dict[str, Any]], Awaitable[bool]]

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
        self.progress_hook: ProgressCallback | None = None
        self.approval_hook: ApprovalCallback | None = None

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
    ) -> AgentResult:
        user_memory = await memory.load(user_id)
        past_messages = await history.load(user_id)

        system_message: dict[str, Any] = {
            "role": "system",
            "content": _build_system_prompt(username, user_id, user_memory),
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
            }
            if tools:
                kwargs["tools"] = tools
                kwargs["tool_choice"] = "auto"

            response = await self._client.chat.completions.create(**kwargs)  # type: ignore[arg-type]

            choice = response.choices[0]
            msg = choice.message

            # Serialize assistant message for history and next API call
            assistant_message: dict[str, Any] = {"role": "assistant", "content": msg.content}
            if msg.tool_calls:
                assistant_message["tool_calls"] = [
                    {
                        "id": tc.id,
                        "type": "function",
                        "function": {
                            "name": tc.function.name,
                            "arguments": tc.function.arguments,
                        },
                    }
                    for tc in msg.tool_calls
                ]

            messages.append(assistant_message)
            turn_messages.append(assistant_message)

            if choice.finish_reason == "stop" or not msg.tool_calls:
                final_text = msg.content or ""
                break

            if choice.finish_reason == "tool_calls":
                tool_result_messages = await self._execute_tools(
                    msg.tool_calls, user_id, user_memory
                )
                # Check for memory updates and artifacts before appending
                for trm in tool_result_messages:
                    if "_memory_update" in trm:
                        user_memory = trm.pop("_memory_update")
                    if "_artifact_paths" in trm:
                        all_artifacts.extend(trm.pop("_artifact_paths"))
                messages.extend(tool_result_messages)
                turn_messages.extend(tool_result_messages)
            else:
                final_text = msg.content or ""
                break

        await history.append_turn(user_id, new_user_message, turn_messages)
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
                return {
                    "role": "tool",
                    "tool_call_id": tc.id,
                    "content": content,
                }
            except Exception as exc:
                logger.exception("Tool '%s' raised an exception", tc.function.name)
                return {
                    "role": "tool",
                    "tool_call_id": tc.id,
                    "content": f"Error: {exc}",
                }

        return list(await asyncio.gather(*[run_one(tc) for tc in tool_calls]))

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
                on_progress=self.progress_hook,
            )

        if name == "run_claude_code":
            if self.approval_hook is not None:
                approved = await self.approval_hook("run_claude_code", args)
                if not approved:
                    return "run_claude_code was not approved by the owner."
            return await run_claude_code(
                task=args["task"],
                model=args.get("model"),
                repo_url=args.get("repo_url"),
                files=args.get("files"),
                on_progress=self.progress_hook,
            )

        if name == "update_memory":
            new_content = args["memory_content"]
            await memory.save(user_id, new_content)
            return {"content": "Memory updated.", "_memory_update": new_content}

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


def _build_system_prompt(username: str, user_id: int | str, user_memory: str) -> str:
    now = datetime.now().strftime("%Y-%m-%d %H:%M")
    memory_section = (
        f"\n\n## Your memory about {username}\n{user_memory}" if user_memory.strip() else ""
    )
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

## Guidelines
- Be conversational and friendly.
- Use Jellyfin tools for any media questions before guessing.
- Use DuckDuckGo for factual or current-events questions.
- For any coding request that goes beyond a trivial one-liner, immediately use run_opencode — don't write code yourself. If in doubt, use it.
- run_claude_code is only available to the bot owner (user ID: {OWNER_ID}). Do not offer or attempt it for any other user.
- Update memory when you learn something worth remembering.
- Keep responses concise unless asked for detail.
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
