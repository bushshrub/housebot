//! house-chatbot — a Discord house-assistant bot backed by a local, OpenAI-compatible
//! LLM.
//!
//! Most functionality lives in small workspace crates under `crates/`, re-exported here
//! at their original module paths; this crate keeps only the agent core and the Discord
//! frontend.

pub mod agent;
pub mod bot;
/// Re-exported from the `housebot-bot-commands` workspace crate; kept at this path so
/// existing `crate::bot_commands::…` references continue to resolve.
pub use housebot_bot_commands as bot_commands;
/// Re-exported from the `housebot-bot-config` workspace crate; kept at this path so
/// existing `crate::bot_config::…` references continue to resolve.
pub use housebot_bot_config as bot_config;
/// Re-exported from the `housebot-bot-formatting` workspace crate; kept at this path so
/// existing `crate::bot_formatting::…` references continue to resolve.
pub use housebot_bot_formatting as bot_formatting;
/// Re-exported from the `housebot-bot-response` workspace crate; kept at this path so
/// existing `crate::bot_response::…` references continue to resolve.
pub use housebot_bot_response as bot_response;
/// Re-exported from the `housebot-channel-log` workspace crate; kept at this path so
/// existing `crate::channel_log::…` references continue to resolve.
pub use housebot_channel_log as channel_log;
/// Re-exported from the `housebot-coding-agent` workspace crate; kept at this path so
/// existing `crate::coding_agent::…` references continue to resolve.
pub use housebot_coding_agent as coding_agent;
/// Re-exported from the `housebot-config` workspace crate; kept at this path so
/// existing `crate::config::…` references continue to resolve.
pub use housebot_config as config;
/// Re-exported from the `housebot-database` workspace crate; kept at this path so
/// existing `crate::database::…` references continue to resolve.
pub use housebot_database as database;
/// Re-exported from the `housebot-discord-bridge` workspace crate; kept at this path so
/// existing `crate::discord_bridge::…` references continue to resolve.
pub use housebot_discord_bridge as discord_bridge;
/// Re-exported from the `housebot-github-issues` workspace crate; kept at this path so
/// existing `crate::github_issues::…` references continue to resolve.
pub use housebot_github_issues as github_issues;
/// Re-exported from the `housebot-history` workspace crate; kept at this path so
/// existing `crate::history::…` references continue to resolve.
pub use housebot_history as history;
/// Re-exported from the `housebot-llm` workspace crate; kept at this path so
/// existing `crate::llm::…` references continue to resolve.
pub use housebot_llm as llm;
/// Re-exported from the `housebot-llm-scheduler` workspace crate; kept at this path so
/// existing `crate::llm_scheduler::…` references continue to resolve.
pub use housebot_llm_scheduler as llm_scheduler;
/// Re-exported from the `housebot-memory` workspace crate; kept at this path so
/// existing `crate::memory::…` references continue to resolve.
pub use housebot_memory as memory;
/// Re-exported from the `housebot-rate-limit` workspace crate; kept at this path so
/// existing `crate::rate_limit::…` references continue to resolve.
pub use housebot_rate_limit as rate_limit;
/// Re-exported from the `housebot-reminders` workspace crate; kept at this path so
/// existing `crate::reminders::…` references continue to resolve.
pub use housebot_reminders as reminders;
/// Re-exported from the `housebot-skills` workspace crate; kept at this path so
/// existing `crate::skills::…` references continue to resolve.
pub use housebot_skills as skills;
/// Re-exported from the `housebot-testing` workspace crate; kept at this path so
/// existing `crate::testing::…` references continue to resolve.
pub use housebot_testing as testing;
/// Re-exported from the `housebot-token-monitor` workspace crate; kept at this
/// path so existing `crate::token_monitor::…` references continue to resolve.
pub use housebot_token_monitor as token_monitor;
/// Re-exported from the `housebot-tools` workspace crate; kept at this path so
/// existing `crate::tools::…` references continue to resolve.
pub use housebot_tools as tools;
