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
`/session status|new|compact` — inspect the current session, start fresh, or summarise it into a carry-over note before starting fresh (persistent memory is only changed when you ask)
`/token_leaderboard [timeframe] [metric]` — rank token usage daily, weekly, monthly, or all-time by total tokens or cache efficiency; also shows your rank
`/status` — show your current settings at a glance (effort, follow-up, personality)
`/effort [level] [user]` — set thinking depth: `instant` (off) · `low` (2k tokens) · `medium` (4k, default) · `high` (8k) · `xhigh` (16k) · `max` (unlimited); server administrators and bot configurers may target another user
`/personalize personality [text]` — set (or clear) a personal tone/personality override
`/personalize followup enabled [timeout]` — toggle unpinged follow-up replies in a server channel
`/personalize progress enabled [user]` — show or hide intermediate reasoning/tool progress; server administrators and bot configurers may target another user
`/config dev_notify_channel [channel]` — set which channel receives feature-development completion notices (configurers only)
`/config access allow|revoke|list` — manage who may configure the bot (configurers only; the owner is always allowed)
`/config user limit|respond|show` — per-user output-token caps and respond policies (configurers only)
`/server-config channel add|remove|list|clear` — restrict which channels the bot responds in (server admins and configurers)
`/server-config leaderboard visibility|role_add|role_remove|role_list` — make leaderboard responses public, private, or role-restricted
`/server-config bot_pings enabled` — toggle responses to other bots' @-mentions
`/labs list|pagination enabled` — list experimental features, or toggle paginated responses
`/commit` — show the running commit hash
`/model` — show the current model name and context size
`/data history show|clear` — inspect or clear your global conversation history
`/data erase confirm:true` — permanently delete all your stored data, including archived conversations and token statistics
`/privacy status|deep_memory` — view or change privacy settings
`/storage memory show|search|clear` — inspect or clear persistent memory the bot has saved about you
`/skill list|info|delete` — manage custom prompt skills shared across all users
`/stats` — show your conversation and memory statistics

**Capabilities**
- Web search and webpage fetching
- Timed reminders delivered by DM
- Create and edit your own GitHub feature requests and bug reports
- Native GitHub issue management via LLM tool (`github_api`): list, search, view detail, close, label, prune issues
- Custom skills (user-defined prompt templates), runnable via scripts in a sandboxed container
- Persistent memory about you across sessions, updated only when you ask
- Persistent conversation archives and global token-usage leaderboards
- Sandboxed development environment: clone a repository, list/read/search files, and run commands in an isolated container
- Bounded sub-agent delegation for research tasks that need several search/fetch steps
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
