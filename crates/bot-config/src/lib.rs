//! Per-server, per-user, and deployment-wide bot configuration.
//! Production persists to the PostgreSQL `bot_config` table; tests and
//! deployments without a database fall back to JSON files under DATA_DIR.

pub mod access;
pub mod backend;
pub mod server;
pub mod user;

pub use access::*;
pub use backend::*;
pub use server::*;
pub use user::*;

#[cfg(test)]
#[path = "lib_tests.rs"]
mod tests;
