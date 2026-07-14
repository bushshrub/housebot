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
pub mod coding_agent;
pub mod config;
pub mod discord_bridge;
pub mod github_issues;
pub mod history;
pub mod llm;
pub mod mcp;
pub mod memory;
pub mod message_log;
pub mod notes;
pub mod rate_limit;
pub mod reminders;
pub mod skills;
pub mod testing;
pub mod tools;
