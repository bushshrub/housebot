//! Emoji classification for the reaction-selection prompt.

pub(crate) fn is_emoji(c: char) -> bool {
    let code = c as u32;
    matches!(code,
        0x231A..=0x23FA |
        0x25AA..=0x25FE |
        0x2600..=0x27BF |
        0x2934..=0x2935 |
        0x2B05..=0x2B55 |
        0x3030 | 0x303D | 0x3297 | 0x3299 |
        0x1F000..=0x1FFFF |
        0xFE00..=0xFE0F   // variation selectors (applied after emoji)
    )
}

pub(crate) fn is_emoji_modifier(c: char) -> bool {
    matches!(c as u32, 0x1F3FB..=0x1F3FF)
}

pub(crate) fn is_regional_indicator(c: char) -> bool {
    matches!(c as u32, 0x1F1E6..=0x1F1FF)
}

pub(crate) fn parse_emoji_selection(value: &str) -> Option<String> {
    let value = value.trim();
    if value.eq_ignore_ascii_case("none") || value.is_empty() {
        return None;
    }
    let mut chars = value.chars();
    let first = chars.next()?;
    if !is_emoji(first) {
        return None;
    }
    let mut after_joiner = false;
    let mut regional_pair = false;
    for c in chars {
        if c == '\u{200D}' {
            after_joiner = true;
        } else if c == '\u{FE0F}' || is_emoji_modifier(c) {
            continue;
        } else if is_regional_indicator(first) && is_regional_indicator(c) && !regional_pair {
            regional_pair = true;
        } else if !is_emoji(c) || !after_joiner {
            return None;
        } else {
            after_joiner = false;
        }
    }
    if after_joiner {
        return None;
    }
    Some(value.to_string())
}

// ── MCP server configuration ─────────────────────────────────────────────────
