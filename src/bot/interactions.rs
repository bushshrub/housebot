//! Slash-command interaction handlers.
//!
//! Facade over the per-area modules so `interactions::handle_*` paths keep
//! resolving from the rest of the bot.

pub(crate) use super::interactions_data::*;
pub(crate) use super::interactions_moderation::*;
pub(crate) use super::interactions_settings::*;
