//! A deliberately small YAML-subset parser for `SKILL.md` frontmatter.
//!
//! Only the shapes this crate writes are accepted: `key: scalar` and
//! `key: [a, b]`, with optional single or double quotes around scalars. Nested
//! maps, block lists, anchors, and multi-line scalars are rejected rather than
//! half-understood, so a malformed skill fails loudly instead of loading with
//! silently wrong metadata. A full YAML implementation is not worth a
//! dependency for four keys we also generate.

use std::collections::BTreeMap;

/// Split a `SKILL.md` into its frontmatter block and body.
///
/// Returns `None` when the file does not open with a `---` fence.
pub fn split(source: &str) -> Option<(&str, &str)> {
    let rest = source.strip_prefix("---")?;
    let rest = rest
        .strip_prefix('\n')
        .or_else(|| rest.strip_prefix("\r\n"))?;
    let mut search = 0;
    while let Some(offset) = rest[search..].find("\n---") {
        let fence_start = search + offset + 1;
        let after = &rest[fence_start + 3..];
        let after_trimmed = after.strip_prefix('\r').unwrap_or(after);
        if after_trimmed.is_empty() || after_trimmed.starts_with('\n') {
            let body = after_trimmed.strip_prefix('\n').unwrap_or("");
            return Some((&rest[..fence_start], body));
        }
        search = fence_start + 3;
    }
    None
}

/// Parse a frontmatter block into scalar and list entries.
pub fn parse(block: &str) -> Result<Frontmatter, String> {
    let mut scalars: BTreeMap<String, String> = BTreeMap::new();
    let mut lists: BTreeMap<String, Vec<String>> = BTreeMap::new();

    for (index, raw) in block.lines().enumerate() {
        let line = raw.trim_end();
        if line.trim().is_empty() || line.trim_start().starts_with('#') {
            continue;
        }
        if line.starts_with(char::is_whitespace) {
            return Err(format!(
                "line {}: indented values are not supported in skill frontmatter",
                index + 1
            ));
        }
        let Some((key, value)) = line.split_once(':') else {
            return Err(format!("line {}: expected 'key: value'", index + 1));
        };
        let key = key.trim().to_string();
        if key.is_empty() {
            return Err(format!("line {}: empty key", index + 1));
        }
        let value = value.trim();
        if let Some(inner) = value.strip_prefix('[').and_then(|v| v.strip_suffix(']')) {
            let items = inner
                .split(',')
                .map(str::trim)
                .filter(|item| !item.is_empty())
                .map(unquote)
                .collect();
            lists.insert(key, items);
        } else {
            scalars.insert(key, unquote(value));
        }
    }

    Ok(Frontmatter { scalars, lists })
}

fn unquote(value: &str) -> String {
    let value = value.trim();
    for quote in ['"', '\''] {
        if value.len() >= 2 && value.starts_with(quote) && value.ends_with(quote) {
            return value[1..value.len() - 1].to_string();
        }
    }
    value.to_string()
}

/// Escape a scalar for emission. Anything that could be misread as structure is
/// quoted, so a description containing a colon survives a write/read round trip.
pub fn emit_scalar(value: &str) -> String {
    let needs_quotes = value.is_empty()
        || value.contains(':')
        || value.contains('#')
        || value.starts_with('[')
        || value.starts_with(['"', '\'', ' '])
        || value.ends_with(' ');
    if needs_quotes {
        format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
    } else {
        value.to_string()
    }
}

/// Parsed frontmatter keys.
pub struct Frontmatter {
    scalars: BTreeMap<String, String>,
    lists: BTreeMap<String, Vec<String>>,
}

impl Frontmatter {
    pub fn scalar(&self, key: &str) -> Option<&str> {
        self.scalars.get(key).map(String::as_str)
    }

    pub fn list(&self, key: &str) -> Vec<String> {
        self.lists.get(key).cloned().unwrap_or_default()
    }

    pub fn number(&self, key: &str) -> u64 {
        self.scalar(key).and_then(|v| v.parse().ok()).unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_frontmatter_from_body() {
        let (front, body) = split("---\nname: a\n---\nBody text\n").unwrap();
        assert_eq!(front, "name: a\n");
        assert_eq!(body, "Body text\n");
    }

    #[test]
    fn body_may_contain_a_horizontal_rule() {
        let (_, body) = split("---\nname: a\n---\nintro\n\n---\n\nmore\n").unwrap();
        assert!(body.contains("intro"));
        assert!(body.contains("more"));
    }

    #[test]
    fn missing_fence_is_rejected() {
        assert!(split("name: a\nbody").is_none());
        assert!(split("---\nname: a\nnever closed").is_none());
    }

    #[test]
    fn parses_scalars_and_lists() {
        let f =
            parse("name: greet\ndescription: Say hello\nenabled_tools: [web_search, x]\n").unwrap();
        assert_eq!(f.scalar("name"), Some("greet"));
        assert_eq!(f.scalar("description"), Some("Say hello"));
        assert_eq!(f.list("enabled_tools"), vec!["web_search", "x"]);
    }

    #[test]
    fn quoted_values_keep_their_colons() {
        let f = parse("description: \"a: b\"\n").unwrap();
        assert_eq!(f.scalar("description"), Some("a: b"));
    }

    #[test]
    fn empty_list_parses_as_empty() {
        let f = parse("editors: []\n").unwrap();
        assert!(f.list("editors").is_empty());
    }

    #[test]
    fn nested_structures_are_rejected_not_guessed() {
        assert!(parse("skill:\n  name: a\n").is_err());
        assert!(parse("bare line\n").is_err());
    }

    #[test]
    fn comments_and_blank_lines_are_ignored() {
        let f = parse("# a comment\n\nname: greet\n").unwrap();
        assert_eq!(f.scalar("name"), Some("greet"));
    }

    #[test]
    fn emitted_scalars_survive_a_round_trip() {
        for value in ["plain", "a: b", "#hash", "[bracket", "trailing ", ""] {
            let line = format!("description: {}\n", emit_scalar(value));
            let parsed = parse(&line).unwrap();
            assert_eq!(parsed.scalar("description"), Some(value), "for {value:?}");
        }
    }
}
