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
`/session status|new|compact` — inspect the current session, start fresh, or summarise it into memory before starting fresh
`/github list [state] [labels]` — list GitHub issues with optional state/label filters
`/github show <number>` — view full issue details (description, labels, comments)
`/github close <number>` — close a GitHub issue
`/github search <query>` — search GitHub issues
`/token_leaderboard [timeframe] [metric]` — rank token usage daily, weekly, monthly, or all-time by total tokens or cache efficiency; also shows your rank
`/tool_ban propose|vote|status` — vote on server-specific user restrictions for individual tools
`/status` — show your current settings at a glance (effort, follow-up, personality)
`/effort [level]` — set thinking depth: `low` (2k tokens) · `medium` (4k, default) · `high` (8k) · `xhigh` (16k) · `max` (unlimited)
`/config personality [text]` — set (or clear) a personal tone/personality override
`/config followup enabled [timeout]` — toggle unpinged follow-up replies in a server channel
`/config channel add|remove|list|clear` — restrict which channels the bot responds in
`/config leaderboard visibility|role_add|role_remove|role_list` — administrators can make leaderboard responses public, private, or role-restricted
`/labs pagination enabled` — toggle paginated responses (experimental)
`/commit` — show the running commit hash
`/model` — show the current model name and context size
`/data profile show|clear` — inspect your stored profile or clear learned profile data and memory
`/data history show|clear` — inspect or clear your global conversation history
`/data erase confirm:true` — permanently delete all your stored data, including archived conversations and token statistics
`/privacy status|deep_memory|proactive` — view or change privacy and proactive-assistance settings
`/storage memory show|search|clear` — inspect or clear persistent memory about you
`/storage notes list|get|save|delete` — manage your named personal notes
`/lua <script>` — run a sandboxed Lua script with `discord.send_message`, `discord.web_search`, `discord.jellyfin_search`, and `graph.node`/`graph.edge`/`graph.title` to render a flowchart or network diagram as an image (requires the Scripting role or higher, or guild administrator / bot owner)

**Prefix commands**
`!grocery` — show your grocery list
`!grocery add <item>` — add an item to your grocery list
`!grocery remove <item>` — remove an item from your grocery list
`!grocery flush` — clear your entire grocery list
`!skill list|add|delete|info <name>` — manage custom prompt skills shared across all users
`!stats` — show your conversation and memory stats

**Capabilities**
- Web search, multi-step deep research with cross-referenced sources, webpage fetching, and public-file downloads delivered as Discord attachments
- Jellyfin media server queries (movies, shows, music) — read-only
- URL summarisation and translation
- Timed reminders delivered by DM
- Create and edit your own GitHub feature requests and bug reports
- Native GitHub issue management: list, view details, close, and search issues via `/github`
- Custom skills (user-defined prompt templates) via `!skill`
- Personal grocery list management (`!grocery`) with persistent storage across sessions
- Personal notes and persistent memory across sessions
- Persistent conversation archives and global token-usage leaderboards
- Guild voting for user-specific tool-call restrictions and bans
- Software development help: discuss, explain, review, advise on code, and execute sandboxed Lua scripts for calculations or data processing
- Self-executing Lua: the bot can write and run Lua 5.4 scripts internally to handle complex calculations, data processing, or algorithmic tasks (web search and Jellyfin search available from scripts)
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
