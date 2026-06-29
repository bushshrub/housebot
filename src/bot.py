"""Discord bot interface."""

import base64
import logging
import os
from io import BytesIO

import aiohttp
import discord
from discord import ui

from .agent import Agent

logger = logging.getLogger(__name__)

MAX_MESSAGE_LENGTH = 2000
OWNER_ID = int(os.getenv("OWNER_DISCORD_ID", "0"))
APPROVAL_TIMEOUT = 300  # seconds


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


class HouseBot(discord.Client):
    def __init__(self) -> None:
        intents = discord.Intents.default()
        intents.message_content = True
        super().__init__(intents=intents)
        self.agent = Agent()

    async def setup_hook(self) -> None:
        await self.agent.start()
        logger.info("Agent and MCP servers ready")

    async def close(self) -> None:
        await self.agent.stop()
        await super().close()

    async def on_ready(self) -> None:
        logger.info("Logged in as %s (ID: %s)", self.user, self.user.id)

    async def on_message(self, message: discord.Message) -> None:
        if message.author.bot:
            return

        # Respond to DMs always; in guilds only when mentioned or replied to
        is_dm = isinstance(message.channel, discord.DMChannel)
        is_mentioned = self.user in message.mentions
        is_reply_to_bot = (
            message.reference is not None
            and message.reference.resolved is not None
            and isinstance(message.reference.resolved, discord.Message)
            and message.reference.resolved.author == self.user
        )

        if not (is_dm or is_mentioned or is_reply_to_bot):
            return

        text = message.content
        if self.user:
            text = text.replace(f"<@{self.user.id}>", "").replace(
                f"<@!{self.user.id}>", ""
            ).strip()

        if not text and not message.attachments:
            return

        image_data = await _extract_images(message)

        progress_msg: discord.Message | None = None
        progress_lines: list[str] = []

        async def on_progress(line: str) -> None:
            nonlocal progress_msg, progress_lines
            progress_lines.append(line)
            tail = "".join(progress_lines[-50:])[-1800:]
            content = f"⚙️ **Working...**\n```\n{tail}\n```"
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

        self.agent.approval_hook = on_approval
        self.agent.progress_hook = on_progress
        try:
            async with message.channel.typing():
                try:
                    response_text = await self.agent.run(
                        user_id=message.author.id,
                        username=message.author.display_name,
                        text=text or "(no text)",
                        image_data=image_data or None,
                    )
                except Exception:
                    logger.exception("Agent error for user %s", message.author.id)
                    response_text = "Sorry, something went wrong. Please try again."
        finally:
            self.agent.approval_hook = None
            self.agent.progress_hook = None

        if progress_msg is not None:
            try:
                await progress_msg.delete()
            except discord.HTTPException:
                pass

        await _send_long_message(message.channel, response_text, reply_to=message)


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
