//! Canonical feature/command reference for this bot.
//!
//! This is the single source of truth consumed by both the `/help` slash
//! command and the `get_bot_features` LLM tool.

use serde_json::{json, Value};

/// Full human-readable reference of every command and feature the bot supports.
pub fn features_text() -> &'static str {
    "\
**Slash commands**
`/help` — show this reference
`/new` / `/reset` — start a fresh conversation
`/compact` — summarise the conversation into memory and start fresh (or clear it without saving when deep memory is disabled)
`/session` — show token and context usage for the current session
`/status` — show your current settings at a glance (effort, follow-up, personality)
`/effort [level]` — set thinking depth: `low` (2k tokens) · `medium` (4k, default) · `high` (8k) · `xhigh` (16k) · `max` (unlimited)
`/config personality [text]` — set (or clear) a personal tone/personality override
`/config followup enabled [timeout]` — toggle unpinged follow-up replies in a server channel
`/config channel add|remove|list|clear` — restrict which channels the bot responds in
`/labs pagination enabled` — toggle paginated responses (experimental)
`/commit` — show the running commit hash
`/model` — show the current model name and context size
`/profile show|clear` — inspect your stored profile or clear learned profile data and memory
`/history show|clear` — inspect or clear your global conversation history
`/privacy status|deep_memory|proactive` — view or change privacy and proactive-assistance settings
`/memory show|clear` — view or clear the bot's persistent memory about you (requires deep memory to be enabled)
`/erase_my_data` — permanently delete all your stored data (messages, history, memory, notes)
`/lua <script>` — run a sandboxed Lua script with `discord.send_message`, `discord.web_search`, and `discord.jellyfin_search` (requires the Scripting role or higher, or guild administrator / bot owner)

**Prefix commands**
`!skill list|add|delete|info <name>` — manage custom prompt skills shared across all users
`!note list|save|get|delete <name>` — manage your personal notes
`!stats` — show your conversation and memory stats
`!new` / `!reset` / `!compact` — same as the slash variants

**Capabilities**
- Web search, multi-step deep research with cross-referenced sources, webpage fetching, and public-file downloads delivered as Discord attachments
- Jellyfin media server queries (movies, shows, music) — read-only
- URL summarisation and translation
- Timed reminders delivered by DM
- Create and edit your own GitHub feature requests
- Custom skills (user-defined prompt templates) via `!skill`
- Personal notes and persistent memory across sessions
- Software development help: discuss, explain, review, and advise on code (cannot execute)
- Chat search: search channel messages by regex to find what was said or who mentioned something
- Discord user profiles: look up a user's username, display name, and account creation date by their user ID
- Opt-in proactive assistance plus privacy-aware greetings and contextual quick-action suggestions
"
}

pub fn definition() -> Value {
    json!({
        "name": "get_bot_features",
        "description": "Return the full list of this bot's commands and capabilities. \
            Call this whenever a user asks what the bot can do, what commands are available, \
            or how to use a specific command or feature.",
        "input_schema": {
            "type": "object",
            "properties": {}
        }
    })
}
