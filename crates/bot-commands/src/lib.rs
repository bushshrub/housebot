//! Store-backed command handlers, independent of Discord transport.

pub mod skills;
pub mod user_data;

pub use skills::*;
pub use user_data::*;

fn truncate_chars(value: &str, limit: usize) -> String {
    value.chars().take(limit).collect()
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// Parse a Discord user mention (`<@123>` or `<@!123>`) into a user ID string,
/// or return the raw string if it doesn't look like a mention.
fn parse_mention(raw: &str) -> &str {
    let raw = raw.trim();
    if let Some(inner) = raw.strip_prefix("<@!").or_else(|| raw.strip_prefix("<@")) {
        if let Some(id) = inner.strip_suffix('>') {
            return id;
        }
    }
    raw
}

/// Prepend `header` to `body`, truncating the combined result to Discord's 2000-char limit.
fn truncate_discord(header: &str, body: &str) -> String {
    const LIMIT: usize = 2000;
    const ELLIPSIS: &str = "\n…(truncated)";
    let full = format!("{header}{body}");
    if full.chars().count() <= LIMIT {
        return full;
    }
    let keep = LIMIT.saturating_sub(ELLIPSIS.chars().count());
    let truncated: String = full.chars().take(keep).collect();
    format!("{truncated}{ELLIPSIS}")
}

#[cfg(test)]
#[path = "lib_tests.rs"]
mod tests;
