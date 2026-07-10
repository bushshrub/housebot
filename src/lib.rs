//! house-chatbot — a Discord house-assistant bot backed by a local, OpenAI-compatible
//! LLM, MCP tool servers, and ephemeral Docker coding sandboxes.
//!
//! This is the Rust rewrite of the original Python implementation. The crate is split
//! into small, individually testable modules mirroring the original layout.

pub mod agent;
pub mod bot_config;
pub mod bot;
pub mod config;
pub mod github_issues;
pub mod history;
pub mod llm;
pub mod mcp;
pub mod memory;
pub mod notes;
pub mod reminders;
pub mod skills;
pub mod testing;
pub mod tools;
