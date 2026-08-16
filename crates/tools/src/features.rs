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
`/status` — show your current settings at a glance (effort, follow-up, personality)
`/stats` — show your conversation, memory, and token statistics
`/token_leaderboard [timeframe] [metric]` — rank token usage daily, weekly, monthly, or all-time by total tokens or cache efficiency; also shows your rank
`/effort [level] [user]` — set how much thinking the model does before replying
`/skill list|info|delete` — manage the skills shared across all users
`/storage memory show|search|clear` — inspect or clear persistent memory about you
`/data history show|clear` — inspect or clear your conversation history
`/data erase confirm:true` — permanently delete all your stored data, including token statistics
`/privacy status|deep_memory` — view or change privacy settings
`/personalize personality [text]` — set (or clear) a personal tone/personality override
`/personalize followup enabled [timeout]` — toggle unpinged follow-up replies in a server channel
`/personalize progress enabled` — toggle intermediate progress updates
`/labs list|pagination` — experimental features
`/commit` — show the running commit hash
`/model` — show the current model name and context size
`/config access allow|revoke|list` — manage who may configure the bot (configurers only; the owner is always allowed)
`/config user limit|respond|show` — per-user output-token caps and respond policies (configurers only)
`/config scheduler show|max_inflight|max_subagent` — LLM concurrency ceilings (configurers only)
`/config dev_notify_channel [channel]` — watch a channel for feature-development completion notices (configurers only)
`/server-config channel add|remove|list|clear` — restrict which channels the bot responds in (server admins and configurers)
`/server-config leaderboard visibility|role_add|role_remove|role_list` — make leaderboard responses public, private, or role-restricted
`/server-config bot_pings enabled` — toggle responses to other bots' @-mentions

**Capabilities**
- Web search and webpage fetching
- Image and PDF attachments read directly from your message
- Timed reminders delivered by DM
- Skills: reusable instruction sets with bundled reference files and scripts, authored by any user and run in a sandbox
- A per-session gVisor sandbox for cloning repositories, searching code, and running commands
- Sub-agents for research that would otherwise crowd out the main conversation
- Chat search: search channel messages by regex, limited to channels you can read yourself
- Persistent memory across conversations, plus token-usage leaderboards
- Create and edit your own GitHub feature requests and bug reports, and hand them to a coding agent
- GitHub issue management (`github_api`): list, search, view detail, close, label, prune issues
- React with ❌ on a reply in progress to cancel it
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

#[cfg(test)]
mod tests {
    use super::features_text;

    /// The reference is a plain string constant, so nothing but this test
    /// stands between a removed feature and a user (or model) still being
    /// told it exists. Extend it whenever something is cut.
    #[test]
    fn reference_does_not_advertise_removed_features() {
        for removed in [
            "/lua",
            "/tool_ban",
            "/tool_restore",
            "/data profile",
            "!grocery",
            "notes",
            "proactive",
            "Jellyfin",
            "deep research",
            "translat",
        ] {
            assert!(
                !features_text().contains(removed),
                "the feature reference still advertises `{removed}`"
            );
        }
    }
}
