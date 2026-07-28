//! Shared fixtures for the `tests` tests.

//! Unit tests for `agent` (split out to keep the module under 600 lines).

use super::*;

pub(crate) fn empty_skills() -> BTreeMap<String, Skill> {
    BTreeMap::new()
}

/// Returns the byte index of the first user/turn-specific marker.
pub(crate) fn dynamic_suffix_start(prompt: &str) -> usize {
    let markers = [
        "\n\n## User profile",
        "\n\n## Your memory about",
        "\n\n## Personality / tone",
        "\n\nCurrent date/time:",
    ];
    let mut earliest = prompt.len();
    for m in &markers {
        if let Some(pos) = prompt.find(m) {
            earliest = earliest.min(pos);
        }
    }
    earliest
}
