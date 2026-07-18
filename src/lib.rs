//! house-chatbot — a Discord house-assistant bot backed by a local, OpenAI-compatible
//! LLM and MCP tool servers.
//!
//! This is the Rust rewrite of the original Python implementation. The crate is split
//! into small, individually testable modules mirroring the original layout.

pub mod agent;
pub mod bot;
mod bot_commands;
pub mod bot_config;
mod bot_formatting;
mod bot_response;
/// Re-exported from the `housebot-channel-log` workspace crate; kept at this path so
/// existing `crate::channel_log::…` references continue to resolve.
pub use housebot_channel_log as channel_log;
pub mod coding_agent;
/// Re-exported from the `housebot-config` workspace crate; kept at this path so
/// existing `crate::config::…` references continue to resolve.
pub use housebot_config as config;
/// Re-exported from the `housebot-database` workspace crate; kept at this path so
/// existing `crate::database::…` references continue to resolve.
pub use housebot_database as database;
pub mod discord_bridge;
pub mod github_issues;
/// Re-exported from the `graph-render` workspace crate; kept at this path so
/// existing `crate::graph_render::…` references continue to resolve.
pub use graph_render;
/// Re-exported from the `housebot-grocery` workspace crate; kept at this path so
/// existing `crate::grocery::…` references continue to resolve.
pub use housebot_grocery as grocery;
/// Re-exported from the `housebot-history` workspace crate; kept at this path so
/// existing `crate::history::…` references continue to resolve.
pub use housebot_history as history;
/// Re-exported from the `housebot-llm` workspace crate; kept at this path so
/// existing `crate::llm::…` references continue to resolve.
pub use housebot_llm as llm;
pub mod llm_queue;
pub mod lua_engine;
/// Re-exported from the `housebot-mcp` workspace crate; kept at this path so
/// existing `crate::mcp::…` references continue to resolve.
pub use housebot_mcp as mcp;
/// Re-exported from the `housebot-memory` workspace crate; kept at this path so
/// existing `crate::memory::…` references continue to resolve.
pub use housebot_memory as memory;
/// Re-exported from the `housebot-message-log` workspace crate; kept at this path so
/// existing `crate::message_log::…` references continue to resolve.
pub use housebot_message_log as message_log;
/// Re-exported from the `housebot-notes` workspace crate; kept at this path so
/// existing `crate::notes::…` references continue to resolve.
pub use housebot_notes as notes;
/// Re-exported from the `housebot-profile` workspace crate; kept at this path so
/// existing `crate::profile::…` references continue to resolve.
pub use housebot_profile as profile;
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
pub mod tool_permissions;
pub mod tools;
