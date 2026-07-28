//! Unit tests for `handler` (split out to keep the module under 400 lines).

use super::*;

#[test]
fn short_content_passes_through_unchanged() {
    let content = "❌ Automatic deployment of `abcdef1` failed at `run_database_migrations`: connection refused".to_string();
    assert_eq!(truncate_for_discord(content.clone()), content);
}

#[test]
fn long_content_is_truncated_to_the_discord_limit() {
    let content = "x".repeat(DISCORD_MESSAGE_LIMIT + 500);
    let truncated = truncate_for_discord(content);
    assert_eq!(truncated.chars().count(), DISCORD_MESSAGE_LIMIT);
    assert!(truncated.ends_with('…'));
}
