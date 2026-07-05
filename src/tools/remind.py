"""Agent tool for creating timed reminders."""

import time
from typing import Any

from ..reminders import add as add_reminder

TOOL_DEFINITION: dict[str, Any] = {
    "name": "set_reminder",
    "description": (
        "Set a timed reminder for the current user. "
        "The bot will DM them the message when the delay elapses. "
        "Use this whenever a user asks to be reminded about something later."
    ),
    "input_schema": {
        "type": "object",
        "properties": {
            "message": {
                "type": "string",
                "description": "The reminder message to send to the user.",
            },
            "delay_minutes": {
                "type": "number",
                "description": "How many minutes from now to deliver the reminder (minimum 1, maximum 43200).",
            },
        },
        "required": ["message", "delay_minutes"],
    },
}


async def create_reminder(user_id: str, message: str, delay_minutes: float) -> str:
    if delay_minutes < 1:
        return "Error: delay_minutes must be at least 1."
    if delay_minutes > 43200:
        return "Error: delay_minutes cannot exceed 43200 (30 days)."

    due_ts = time.time() + delay_minutes * 60
    await add_reminder(user_id=user_id, message=message, due_ts=due_ts)

    hours, mins = divmod(int(delay_minutes), 60)
    if hours and mins:
        time_str = f"{hours}h {mins}m"
    elif hours:
        time_str = f"{hours}h"
    else:
        time_str = f"{mins}m"
    return f"✅ Reminder set! I'll DM you in {time_str}."
