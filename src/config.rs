//! Environment-driven configuration helpers.
//!
//! The data directory is resolved from `DATA_DIR` (default `data`). Storage modules
//! read it lazily so tests can override the environment before the first access.

use std::env;
use std::path::PathBuf;

/// Root directory for all persisted state (`DATA_DIR`, default `data`).
pub fn data_dir() -> PathBuf {
    PathBuf::from(env::var("DATA_DIR").unwrap_or_else(|_| "data".to_string()))
}

/// Read an environment variable, returning `default` when unset or empty.
pub fn env_or(name: &str, default: &str) -> String {
    match env::var(name) {
        Ok(v) if !v.is_empty() => v,
        _ => default.to_string(),
    }
}

/// Read an environment variable as `T`, falling back to `default` on any error.
pub fn env_parse<T: std::str::FromStr>(name: &str, default: T) -> T {
    env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// The owning user's Discord ID (`OWNER_DISCORD_ID`, `0` when unset).
pub fn owner_id() -> u64 {
    env_parse("OWNER_DISCORD_ID", 0)
}
