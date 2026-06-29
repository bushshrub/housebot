"""Discord bot interface."""

import asyncio
import base64
import collections
import logging
import os
import re
import time
from io import BytesIO

import aiohttp
import discord
import sentry_sdk
from discord import ui

from .agent import Agent, AgentResult
from .github_issues import GitHubIssueReporter
from . import skills

logger = logging.getLogger(__name__)

# Collect secret values from env vars whose names suggest they contain credentials.
# Built once at import time so we don't re-scan os.environ on every message.
_SECRET_PATTERNS: list[re.Pattern[str]] = []

def _build_secret_patterns() -> None:
    _secret_keywords = ("token", "key", "secret", "password", "dsn", "api_key", "oauth")
    for name, value in os.environ.items():
        if not value or len(value) < 8:
            continue
        if any(kw in name.lower() for kw in _secret_keywords):
            _SECRET_PATTERNS.append(re.compile(re.escape(value)))

_build_secret_patterns()


def _redact_secrets(text: str) -> str:
    for pat in _SECRET_PATTERNS:
        text = pat.sub("[REDACTED]", text)
    return text


MAX_MESSAGE_LENGTH = 2000
OWNER_ID = int(os.getenv("OWNER_DISCORD_ID", "0"))
_ANSI_RE = re.compile(r"\x1b\[[0-9;]*[a-zA-Z]|\x1b[()][AB012]|\r")
_CODE_FENCE_RE = re.compile(r"```(\w*)\n(.*?)(?:```|$)", re.DOTALL)
CODE_FILE_THRESHOLD = 800  # extract code blocks larger than this many characters

_LANG_EXT_MAP = {
    "python": ".py", "py": ".py",
    "javascript": ".js", "js": ".js",
    "typescript": ".ts", "ts": ".ts",
    "bash": ".sh", "sh": ".sh", "shell": ".sh",
    "rust": ".rs", "go": ".go", "java": ".java",
    "c": ".c", "cpp": ".cpp", "c++": ".cpp",
    "html": ".html", "css": ".css", "json": ".json",
    "yaml": ".yaml", "yml": ".yml", "toml": ".toml",
    "sql": ".sql", "ruby": ".rb", "rb": ".rb", "php": ".php",
}
APPROVAL_TIMEOUT = 300  # seconds
CONVERSATION_IDLE_TIMEOUT = int(os.getenv("CONVERSATION_IDLE_TIMEOUT", "300"))  # 5 min default


class ApprovalView(ui.View):
    def __init__(self) -> None:
        super().__init__(timeout=APPROVAL_TIMEOUT)
        self.approved: bool | None = None

    @ui.button(label="Approve", style=discord.ButtonStyle.success, emoji="✅")
    async def approve(self, interaction: discord.Interaction, button: ui.Button) -> None:
        self.approved = True
        await interaction.response.edit_message(content="✅ Approved — running now.", view=None)
        self.stop()

    @ui.button(label="Deny", style=discord.ButtonStyle.danger, emoji="❌")
    async def deny(self, interaction: discord.Interaction, button: ui.Button) -> None:
        self.approved = False
        await interaction.response.edit_message(content="❌ Denied.", view=None)
        self.stop()

    async def on_timeout(self) -> None:
        self.approved = None


class FileIssueView(ui.View):
    def __init__(self) -> None:
        super().__init__(timeout=APPROVAL_TIMEOUT)
        self.file_issue: bool | None = None

    @ui.button(label="File Issue", style=discord.ButtonStyle.primary, emoji="🐛")
    async def file_issue_btn(self, interaction: discord.Interaction, button: ui.Button) -> None:
        self.file_issue = True
        await interaction.response.edit_message(content="Filing issue...", view=None)
        self.stop()

    @ui.button(label="Dismiss", style=discord.ButtonStyle.secondary, emoji="✖️")
    async def dismiss(self, interaction: discord.Interaction, button: ui.Button) -> None:
        self.file_issue = False
        await interaction.response.edit_message(content="Dismissed.", view=None)
        self.stop()

    async def on_timeout(self) -> None:
        self.file_issue = None


class HouseBot(discord.Client):
    def __init__(self) -> None:
        intents = discord.Intents.default()
        intents.message_content = True
        super().__init__(intents=intents)
        self.agent = Agent()
        self.issue_reporter = GitHubIssueReporter()
        # Maps (channel_id, user_id) -> last activity timestamp
        self._active_conversations: dict[tuple[int, int], float] = {}
        # Message IDs currently being processed — prevents concurrent duplicate handling
        self._processing_messages: set[int] = set()
        # Recently responded message IDs — catches gateway replays after processing finishes
        self._responded_messages: collections.deque[int] = collections.deque(maxlen=200)

    async def setup_hook(self) -> None:
        await self.agent.start()
        logger.info("Agent and MCP servers ready")

    async def close(self) -> None:
        await self.agent.stop()
        await super().close()

    async def on_ready(self) -> None:
        logger.info("Logged in as %s (ID: %s)", self.user, self.user.id)

    def _is_in_active_conversation(self, channel_id: int, user_id: int) -> bool:
        """Return True if the conversation is still within the idle window."""
        key = (channel_id, user_id)
        last_active = self._active_conversations.get(key)
        if last_active is None:
            return False
        if time.monotonic() - last_active > CONVERSATION_IDLE_TIMEOUT:
            return False
        return True

    def _pop_timed_out_conversation(self, channel_id: int, user_id: int) -> bool:
        """Remove an expired conversation entry and return True if one existed."""
        key = (channel_id, user_id)
        last_active = self._active_conversations.get(key)
        if last_active is not None and time.monotonic() - last_active > CONVERSATION_IDLE_TIMEOUT:
            del self._active_conversations[key]
            return True
        return False

    def _mark_conversation_active(self, channel_id: int, user_id: int) -> None:
        self._active_conversations[(channel_id, user_id)] = time.monotonic()

    def _report_error(self, exc: Exception) -> None:
        sentry_event_id = sentry_sdk.capture_exception(exc)
        logger.info("Captured error in Sentry (event: %s)", sentry_event_id)
        if OWNER_ID and sentry_event_id:
            asyncio.get_running_loop().create_task(self._notify_owner_of_error(sentry_event_id))

    async def _notify_owner_of_error(self, sentry_event_id: str) -> None:
        try:
            owner = await self.fetch_user(OWNER_ID)
            view = FileIssueView()
            await owner.send(
                f"An error occurred (Sentry: `{sentry_event_id}`). File a GitHub issue?",
                view=view,
            )
            await view.wait()
            if view.file_issue:
                issue_url = await self.issue_reporter.create_error_issue(sentry_event_id)
                if issue_url:
                    await owner.send(f"Issue filed: {issue_url}")
                else:
                    await owner.send("Failed to file issue (GitHub reporter not configured?).")
        except Exception:
            logger.exception("Failed to DM owner about error")

    async def _handle_skill_command(self, message: discord.Message) -> None:
        content = message.content.strip()
        lines = content.split("\n", 1)
        first_line = lines[0].strip()
        rest = lines[1].strip() if len(lines) > 1 else ""

        parts = first_line.split(None, 2)
        if len(parts) < 2:
            await message.reply(
                "Usage: `!skill list` | `!skill add <name>` | `!skill delete <name>` | `!skill info <name>`",
                mention_author=False,
            )
            return

        subcmd = parts[1].lower()

        if subcmd == "list":
            all_skills = await skills.load_all()
            if not all_skills:
                await message.reply(
                    "No skills defined yet. Use `!skill add <name>` (with the prompt on the next line).",
                    mention_author=False,
                )
                return
            lines_out = ["**Skills:**"]
            for s in all_skills.values():
                desc = s.get("description", "")[:80]
                lines_out.append(f"• **{s['name']}** — {desc}")
            await message.reply("\n".join(lines_out), mention_author=False)
            return

        if subcmd == "info":
            if len(parts) < 3:
                await message.reply("Usage: `!skill info <name>`", mention_author=False)
                return
            skill_name = parts[2].lower()
            skill = await skills.get(skill_name)
            if skill is None:
                await message.reply(f"Skill `{skill_name}` not found.", mention_author=False)
                return
            prompt_preview = skill["prompt"][:500]
            if len(skill["prompt"]) > 500:
                prompt_preview += "…"
            await message.reply(
                f"**Skill: {skill['name']}**\n"
                f"Description: {skill.get('description', '(none)')}\n"
                f"```\n{prompt_preview}\n```",
                mention_author=False,
            )
            return

        if subcmd == "add":
            if len(parts) < 3:
                await message.reply(
                    "Usage: `!skill add <name>` with the skill prompt on the next line.",
                    mention_author=False,
                )
                return
            skill_name = parts[2].lower().strip()
            if not re.match(r"^[a-z0-9_]+$", skill_name):
                await message.reply(
                    "Skill name must be lowercase letters, numbers, and underscores only.",
                    mention_author=False,
                )
                return
            if not rest:
                await message.reply(
                    "Please include the skill prompt on a new line after the command.\n"
                    "Example:\n```\n!skill add my_skill\nYou are a helpful assistant that...\n```",
                    mention_author=False,
                )
                return
            description = rest[:100] + ("…" if len(rest) > 100 else "")
            skill = {
                "name": skill_name,
                "description": description,
                "prompt": rest,
                "created_by": str(message.author.id),
            }
            await skills.save_skill(skill)
            await message.reply(f"✅ Skill **{skill_name}** saved.", mention_author=False)
            return

        if subcmd == "delete":
            if len(parts) < 3:
                await message.reply("Usage: `!skill delete <name>`", mention_author=False)
                return
            skill_name = parts[2].lower()
            deleted = await skills.delete_skill(skill_name)
            if deleted:
                await message.reply(f"✅ Skill **{skill_name}** deleted.", mention_author=False)
            else:
                await message.reply(f"Skill `{skill_name}` not found.", mention_author=False)
            return

        await message.reply(
            f"Unknown subcommand `{subcmd}`. Options: `list`, `add`, `delete`, `info`",
            mention_author=False,
        )

    async def on_message(self, message: discord.Message) -> None:
        if message.author == self.user:
            return

        if message.content.startswith("!skill"):
            await self._handle_skill_command(message)
            return

        is_dm = isinstance(message.channel, discord.DMChannel)
        is_mentioned = self.user in message.mentions
        is_reply_to_bot = (
            message.reference is not None
            and message.reference.resolved is not None
            and isinstance(message.reference.resolved, discord.Message)
            and message.reference.resolved.author == self.user
        )

        # Check if bot's name appears in the message text (case-insensitive)
        bot_name = (self.user.display_name if self.user else "").lower()
        is_name_mentioned = bool(bot_name) and bot_name in message.content.lower()

        is_active = self._is_in_active_conversation(message.channel.id, message.author.id)
        session_expired = not is_active and self._pop_timed_out_conversation(message.channel.id, message.author.id)

        if not (is_dm or is_mentioned or is_reply_to_bot or is_name_mentioned or is_active):
            return

        if message.id in self._processing_messages or message.id in self._responded_messages:
            logger.warning("Duplicate on_message for %s — skipping", message.id)
            return
        self._processing_messages.add(message.id)
        try:
            await self._handle_message(message, session_expired=session_expired)
        finally:
            self._processing_messages.discard(message.id)
            self._responded_messages.append(message.id)

    async def _handle_message(self, message: discord.Message, *, session_expired: bool = False) -> None:
        text = message.content
        if self.user:
            text = text.replace(f"<@{self.user.id}>", "").replace(
                f"<@!{self.user.id}>", ""
            ).strip()

        if not text and not message.attachments:
            return

        if session_expired:
            try:
                await self.agent.start_new_session(message.author.id)
            except Exception:
                logger.exception("Failed to start new session for user %s", message.author.id)

        with sentry_sdk.new_scope() as scope:
            scope.set_user({"id": str(message.author.id), "username": message.author.display_name})
            scope.set_context("discord", {
                "message_id": str(message.id),
                "channel": getattr(message.channel, "name", "DM"),
                "channel_id": str(message.channel.id),
                "content": text[:1000],
                "author": message.author.display_name,
                "author_id": str(message.author.id),
                "has_attachments": bool(message.attachments),
            })

            image_data = await _extract_images(message)

            progress_msg: discord.Message | None = None
            progress_lines: list[str] = []
            _last_stream_edit: float = 0.0
            _STREAM_EDIT_INTERVAL = 1.2  # seconds between Discord edits while streaming

            async def on_tool_called(tool_name: str, args: dict) -> None:
                nonlocal progress_msg, progress_lines
                hint = _tool_hint(tool_name, args)
                content = f"⚙️ **`{tool_name}`**{hint}"
                try:
                    if progress_msg is None:
                        progress_msg = await message.reply(content, mention_author=False)
                    else:
                        await progress_msg.edit(content=content)
                except discord.HTTPException:
                    pass
                # Reset log lines so sandbox output starts fresh below the tool header
                progress_lines.clear()

            async def on_progress(line: str) -> None:
                nonlocal progress_msg, progress_lines
                clean = _ANSI_RE.sub("", line)
                if not clean.strip():
                    return
                progress_lines.append(clean)
                tail = "".join(progress_lines[-50:])[-1800:]
                content = f"⚙️ **Working...**\n```\n{tail}\n```"
                try:
                    if progress_msg is None:
                        progress_msg = await message.reply(content, mention_author=False)
                    else:
                        await progress_msg.edit(content=content)
                except discord.HTTPException:
                    pass

            async def on_text_stream(partial_text: str) -> None:
                nonlocal progress_msg, _last_stream_edit
                now = asyncio.get_event_loop().time()
                if now - _last_stream_edit < _STREAM_EDIT_INTERVAL:
                    return
                _last_stream_edit = now
                chunks = _split_text(partial_text)
                content = chunks[0] + ("…" if len(chunks) > 1 else "")
                try:
                    if progress_msg is None:
                        progress_msg = await message.reply(content, mention_author=False)
                    else:
                        await progress_msg.edit(content=content)
                except discord.HTTPException:
                    pass

            async def on_approval(tool_name: str, args: dict) -> bool:
                if not OWNER_ID:
                    logger.warning("OWNER_DISCORD_ID not set — auto-approving %s", tool_name)
                    return True
                if message.author.id != OWNER_ID:
                    logger.info("User %s is not the owner — denying %s", message.author.id, tool_name)
                    return False
                try:
                    owner = await self.fetch_user(OWNER_ID)
                    task_preview = args.get("task", "")[:400]
                    view = ApprovalView()
                    approval_msg = await owner.send(
                        f"**Approval required: `{tool_name}`**\n"
                        f"Requested by **{message.author.display_name}**\n\n"
                        f"**Task:**\n```\n{task_preview}\n```",
                        view=view,
                    )
                    await view.wait()
                    if view.approved is None:
                        await approval_msg.edit(
                            content=f"⏰ Approval timed out — `{tool_name}` was not run.",
                            view=None,
                        )
                        return False
                    return view.approved
                except Exception:
                    logger.exception("Approval flow failed for %s — denying", tool_name)
                    return False

            with sentry_sdk.start_transaction(
                op="discord.message",
                name=f"on_message/{message.author.display_name}",
            ) as txn:
                txn.set_tag("author_id", str(message.author.id))
                txn.set_data("content", text[:500])
                txn.set_data("channel", getattr(message.channel, "name", "DM"))

                async with message.channel.typing():
                    try:
                        result: AgentResult = await self.agent.run(
                            user_id=message.author.id,
                            username=message.author.display_name,
                            text=text or "(no text)",
                            image_data=image_data or None,
                            approval_hook=on_approval,
                            progress_hook=on_progress,
                            tool_notification_hook=on_tool_called,
                            text_stream_hook=on_text_stream,
                        )
                    except Exception as exc:
                        logger.exception("Agent error for user %s", message.author.id)
                        result = AgentResult(text="Sorry, something went wrong. Please try again.")
                        self._report_error(exc)

                self._mark_conversation_active(message.channel.id, message.author.id)
                safe_text = _redact_secrets(result.text)
                display_text, code_files = _extract_code_files(safe_text)
                await _send_final_message(message.channel, display_text, progress_msg=progress_msg, reply_to=message)

                # Upload extracted code files (redacted)
                for filename, content in code_files:
                    try:
                        safe_content = _redact_secrets(content.decode(errors="replace")).encode()
                        await message.channel.send(file=discord.File(BytesIO(safe_content), filename=filename))
                    except Exception:
                        logger.exception("Failed to upload code file %s", filename)

                # Upload workspace files — strip the uid_ prefix for display, redact contents
                for path in result.artifact_paths:
                    try:
                        raw_name = os.path.basename(path)
                        display_name = raw_name.split("_", 1)[1] if "_" in raw_name else raw_name
                        with open(path, "rb") as f:
                            raw = f.read()
                        safe = _redact_secrets(raw.decode(errors="replace")).encode()
                        await message.channel.send(
                            file=discord.File(BytesIO(safe), filename=display_name)
                        )
                    except Exception:
                        logger.exception("Failed to upload artifact %s", path)


def _tool_hint(tool_name: str, args: dict) -> str:
    """Return a short human-readable suffix describing the tool call args."""
    if tool_name == "run_skill":
        name = args.get("name", "")
        inp = args.get("input", "")[:60].replace("\n", " ")
        return f" — {name}: {inp}" if name else ""
    for key in ("query", "task", "repo_url", "memory_content", "url"):
        val = args.get(key)
        if val and isinstance(val, str):
            preview = val[:80].replace("\n", " ")
            if len(val) > 80:
                preview += "…"
            return f" — {preview}"
    return ""


async def _extract_images(message: discord.Message) -> list[dict[str, str]]:
    """Download image attachments and return them as base64-encoded dicts."""
    images: list[dict[str, str]] = []
    image_extensions = {".png", ".jpg", ".jpeg", ".gif", ".webp"}

    for attachment in message.attachments:
        ext = "." + attachment.filename.rsplit(".", 1)[-1].lower() if "." in attachment.filename else ""
        if ext not in image_extensions:
            continue

        media_type_map = {
            ".png": "image/png",
            ".jpg": "image/jpeg",
            ".jpeg": "image/jpeg",
            ".gif": "image/gif",
            ".webp": "image/webp",
        }
        media_type = media_type_map.get(ext, "image/jpeg")

        try:
            async with aiohttp.ClientSession() as http:
                async with http.get(attachment.url) as resp:
                    data = await resp.read()
            images.append({
                "media_type": media_type,
                "data": base64.b64encode(data).decode(),
            })
        except Exception:
            logger.exception("Failed to download attachment %s", attachment.filename)

    return images


async def _send_final_message(
    channel: discord.abc.Messageable,
    text: str,
    progress_msg: discord.Message | None = None,
    reply_to: discord.Message | None = None,
) -> None:
    """Send the final response, reusing the progress message when possible to avoid double-posting."""
    chunks = _split_text(text)
    if progress_msg is not None:
        try:
            await progress_msg.edit(content=chunks[0])
            for chunk in chunks[1:]:
                await channel.send(chunk)
            return
        except discord.HTTPException:
            try:
                await progress_msg.delete()
            except discord.HTTPException:
                pass
    await _send_long_message(channel, text, reply_to=reply_to)


async def _send_long_message(
    channel: discord.abc.Messageable,
    text: str,
    reply_to: discord.Message | None = None,
) -> None:
    chunks = _split_text(text)
    for i, chunk in enumerate(chunks):
        if i == 0 and reply_to is not None:
            await reply_to.reply(chunk, mention_author=False)
        else:
            await channel.send(chunk)


def _extract_code_files(text: str) -> tuple[str, list[tuple[str, bytes]]]:
    """Replace large fenced code blocks with file references; return modified text + files."""
    files: list[tuple[str, bytes]] = []
    counter = [0]

    def replace(m: re.Match) -> str:
        lang = m.group(1).lower()
        code = m.group(2)
        if len(code) < CODE_FILE_THRESHOLD:
            return m.group(0)
        counter[0] += 1
        ext = _LANG_EXT_MAP.get(lang, ".txt")
        filename = f"script_{counter[0]}{ext}"
        files.append((filename, code.encode()))
        return f"*(see attached: `{filename}`)*"

    modified = _CODE_FENCE_RE.sub(replace, text)
    return modified, files


def _split_text(text: str, limit: int = MAX_MESSAGE_LENGTH) -> list[str]:
    if len(text) <= limit:
        return [text]

    chunks: list[str] = []
    while text:
        if len(text) <= limit:
            chunks.append(text)
            break
        split_at = text.rfind("\n", 0, limit)
        if split_at == -1:
            split_at = limit
        chunks.append(text[:split_at])
        text = text[split_at:].lstrip("\n")
    return chunks


def run() -> None:
    logging.basicConfig(
        level=logging.INFO,
        format="%(asctime)s %(levelname)s %(name)s: %(message)s",
    )
    token = os.environ["DISCORD_BOT_TOKEN"]
    bot = HouseBot()
    bot.run(token)
