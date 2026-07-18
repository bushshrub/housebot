//! Environment-driven configuration helpers.
//!
//! The data directory is resolved from `DATA_DIR` (default `data`). Storage modules
//! read it lazily so tests can override the environment before the first access.

use std::env;
use std::path::PathBuf;

/// Root directory for all persisted state (`DATA_DIR`, default `data`).
pub fn data_dir() -> PathBuf {
    PathBuf::from(env_or("DATA_DIR", "data"))
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

/// Whether to respond to @-mentions from other bots (`RESPOND_TO_BOT_PINGS`,
/// default `false`). The bot always ignores its own pings regardless.
pub fn respond_to_bot_pings() -> bool {
    env_parse("RESPOND_TO_BOT_PINGS", false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_dir_defaults_when_unset_or_empty() {
        // Tests within this crate share the process environment, so exercise
        // both states in one test to avoid races.
        env::remove_var("DATA_DIR");
        assert_eq!(data_dir(), PathBuf::from("data"));
        env::set_var("DATA_DIR", "");
        assert_eq!(data_dir(), PathBuf::from("data"));
        env::set_var("DATA_DIR", "/tmp/housebot-test");
        assert_eq!(data_dir(), PathBuf::from("/tmp/housebot-test"));
        env::remove_var("DATA_DIR");
    }
}
