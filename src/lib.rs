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
pub mod channel_log;
pub mod coding_agent;
pub mod config;
pub mod database;
pub mod discord_bridge;
pub mod github_issues;
pub mod grocery;
/// Re-exported from the `graph-render` workspace crate; kept at this path so
/// existing `crate::graph_render::…` references continue to resolve.
pub use graph_render;
pub mod history;
/// Re-exported from the `housebot-llm` workspace crate; kept at this path so
/// existing `crate::llm::…` references continue to resolve.
pub use housebot_llm as llm;
pub mod llm_queue;
pub mod lua_engine;
pub mod mcp;
pub mod memory;
pub mod message_log;
pub mod notes;
pub mod profile;
pub mod rate_limit;
pub mod reminders;
pub mod skills;
pub mod testing;
pub mod token_monitor;
pub mod tool_permissions;
pub mod tools;
