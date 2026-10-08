//! Pure formatting helpers for Discord responses and tool progress.

use std::sync::LazyLock;

use regex::{Captures, Regex};
use serde_json::Value;

const CODE_FILE_THRESHOLD: usize = 800;
static CODE_FENCE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?s)```(\w*)\n(.*?)(?:```|$)").expect("code fence regex must be valid")
});

pub fn lang_ext(lang: &str) -> &'static str {
    match lang {
        "python" | "py" => ".py",
        "javascript" | "js" => ".js",
        "typescript" | "ts" => ".ts",
        "bash" | "sh" | "shell" => ".sh",
        "rust" => ".rs",
        "go" => ".go",
        "java" => ".java",
        "c" => ".c",
        "cpp" | "c++" => ".cpp",
        "html" => ".html",
        "css" => ".css",
        "json" => ".json",
        "yaml" | "yml" => ".yaml",
        "toml" => ".toml",
        "sql" => ".sql",
        "ruby" | "rb" => ".rb",
        "php" => ".php",
        _ => ".txt",
    }
}

fn truncate(text: &str, limit: usize) -> String {
    text.chars().take(limit).collect()
}

pub fn split_text(text: &str, limit: usize) -> Vec<String> {
    // A zero limit would produce an empty chunk without advancing, looping
    // forever; clamp so pathological callers still terminate.
    let limit = limit.max(1);
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= limit {
        return vec![text.to_string()];
    }
    let mut chunks = Vec::new();
    let mut start = 0;
    while start < chars.len() {
        if chars.len() - start <= limit {
            chunks.push(chars[start..].iter().collect());
            break;
        }
        let end = start + limit;
        let split = (start..end)
            .rev()
            .find(|&i| chars[i] == '\n')
            .filter(|&i| i > start)
            .unwrap_or(end);
        chunks.push(chars[start..split].iter().collect());
        start = split;
        while start < chars.len() && chars[start] == '\n' {
            start += 1;
        }
    }
    chunks
}

pub fn tool_hint(tool_name: &str, args: &Value) -> String {
    let get = |key| args.get(key).and_then(Value::as_str).unwrap_or("");
    match tool_name {
        "manage_skill" if !get("name").is_empty() => format!(" — {}", get("name")),
        "set_reminder" if !get("message").is_empty() => format!(
            " — in {}m: {}",
            args.get("delay_minutes")
                .map(Value::to_string)
                .unwrap_or_default(),
            truncate(get("message"), 60).replace('\n', " ")
        ),
        "translate" if !get("target_language").is_empty() => format!(
            " — → {}: {}",
            get("target_language"),
            truncate(get("text"), 40).replace('\n', " ")
        ),
        "manage_skill" | "set_reminder" | "translate" => String::new(),
        _ => [
            "query",
            "task",
            "repo_url",
            "memory_content",
            "url",
            "command",
            "path",
        ]
        .into_iter()
        .map(get)
        .find(|value| !value.is_empty())
        .map(|value| {
            let mut preview = truncate(value, 80).replace('\n', " ");
            if value.chars().count() > 80 {
                preview.push('…');
            }
            format!(" — {preview}")
        })
        .unwrap_or_default(),
    }
}

/// Longest shell command shown in full; a Discord message holds 2,000 characters.
const SHELL_COMMAND_DISPLAY_LIMIT: usize = 1500;

/// The message announcing one tool call. A shell command is shown verbatim in a
/// code fence so it can be read and copied; other tools get the label and, when
/// there is one, the detail in inline code. A URL is wrapped in `<>` so Discord
/// does not add a link preview.
pub fn tool_message(tool_name: &str, args: &Value) -> String {
    let status = tool_status(tool_name);
    let base = status.strip_suffix("...**").unwrap_or(&status);
    let label = format!("{base}**");
    let command = args.get("command").and_then(Value::as_str).unwrap_or("");
    if tool_name == "shell" {
        if command.trim().is_empty() {
            return label;
        }
        let mut shown = truncate(command.trim_end(), SHELL_COMMAND_DISPLAY_LIMIT);
        if command.trim_end().chars().count() > SHELL_COMMAND_DISPLAY_LIMIT {
            shown.push('…');
        }
        let fence = fence_for(&shown);
        return format!("{base}:**\n{fence}sh\n{shown}\n{fence}");
    }
    let hint = tool_hint(tool_name, args);
    match hint.strip_prefix(" — ") {
        None => label,
        Some(detail) if detail.starts_with("http://") || detail.starts_with("https://") => {
            format!("{label} · <{detail}>")
        }
        Some(detail) => format!("{label} · `{}`", detail.replace('`', "'")),
    }
}

/// A backtick fence longer than any run inside `text`, so the text cannot close it early.
fn fence_for(text: &str) -> String {
    let mut longest = 0;
    let mut run = 0;
    for c in text.chars() {
        if c == '`' {
            run += 1;
            longest = longest.max(run);
        } else {
            run = 0;
        }
    }
    "`".repeat((longest + 1).max(3))
}

fn display_tool_name(name: &str) -> String {
    const MAX: usize = 80;
    let sanitized: String = name.chars().filter(|c| !c.is_control()).collect();
    if sanitized.chars().count() > MAX {
        let mut truncated: String = sanitized.chars().take(MAX - 1).collect();
        truncated.push('…');
        truncated
    } else {
        sanitized
    }
}

/// Icon, status label, and short noun for a tool, or `None` for tools without one.
fn tool_label(tool_name: &str) -> Option<(&'static str, &'static str, &'static str)> {
    let labels = match tool_name {
        "web_search" => ("🔎", "Searching the web", "web search"),
        "fetch_webpage" => ("🌐", "Reading a webpage", "webpage"),
        "manage_skill" => ("🧩", "Updating skills", "skill"),
        "set_reminder" => ("⏰", "Setting a reminder", "reminder"),
        "get_messages" => ("💬", "Reading conversations", "chat history"),
        "get_bot_features" => ("🤖", "Checking my features", "features"),
        "configure_bot" => ("⚙️", "Changing bot settings", "bot settings"),
        "update_memory" => ("📓", "Updating memory", "memory update"),
        "search_memory" => ("📓", "Searching memory", "memory search"),
        "github_api" => ("🐙", "Checking GitHub", "GitHub"),
        "create_feature_request" => ("📝", "Filing a feature request", "feature request"),
        "edit_feature_request" => ("📝", "Updating a feature request", "feature request"),
        "prepare_feature_development" => {
            ("🛠️", "Preparing feature development", "feature development")
        }
        "read" => ("📦", "Reading a file", "file read"),
        "write" => ("📦", "Writing a file", "file write"),
        "edit" => ("📦", "Editing a file", "file edit"),
        "shell" => ("📦", "Running a command", "command"),
        _ => return None,
    };
    Some(labels)
}

/// User-facing status shown while an agent tool is executing.
pub fn tool_status(tool_name: &str) -> String {
    match tool_label(tool_name) {
        Some((icon, label, _)) => format!("{icon} **{label}...**"),
        None => format!("🔧 **Running `{}`...**", display_tool_name(tool_name)),
    }
}

/// One line summing up a turn's tool calls, grouped by tool in first-call
/// order: `🛠️ **4 tool calls** · 🔎 web search ×2 · 🌐 webpage ×2`.
pub fn tool_summary(tools: &[String]) -> String {
    let mut groups: Vec<(&str, usize)> = Vec::new();
    for tool in tools {
        match groups.iter_mut().find(|(name, _)| name == tool) {
            Some((_, count)) => *count += 1,
            None => groups.push((tool, 1)),
        }
    }
    let calls = if tools.len() == 1 { "call" } else { "calls" };
    let mut summary = format!("🛠️ **{} tool {calls}**", tools.len());
    for (tool, count) in groups {
        let name = match tool_label(tool) {
            Some((icon, _, noun)) => format!("{icon} {noun}"),
            None => format!("🔧 `{}`", display_tool_name(tool)),
        };
        summary.push_str(&format!(" · {name}"));
        if count > 1 {
            summary.push_str(&format!(" ×{count}"));
        }
    }
    summary
}

pub fn extract_code_files(text: &str) -> (String, Vec<(String, Vec<u8>)>) {
    let mut files = Vec::new();
    let mut counter = 0;
    let modified = CODE_FENCE.replace_all(text, |caps: &Captures| {
        let lang = caps
            .get(1)
            .map(|m| m.as_str())
            .unwrap_or_default()
            .to_lowercase();
        let code = caps.get(2).map(|m| m.as_str()).unwrap_or_default();
        if code.chars().count() < CODE_FILE_THRESHOLD {
            return caps
                .get(0)
                .map(|m| m.as_str())
                .unwrap_or_default()
                .to_string();
        }
        counter += 1;
        let filename = format!("script_{counter}{}", lang_ext(&lang));
        files.push((filename.clone(), code.as_bytes().to_vec()));
        format!("*(see attached: `{filename}`)*")
    });
    (modified.into_owned(), files)
}

/// Render a token count to three significant figures once it reaches the
/// thousands (`12,345` → `12.3k`), so long counts stay readable at a glance.
pub fn format_tokens(count: u64) -> String {
    if count < 1_000 {
        return count.to_string();
    }
    let mut value = count as f64;
    for suffix in ["k", "M", "B"] {
        value /= 1_000.0;
        let digits_before_point = value.log10().floor() as i32 + 1;
        let scale = 10f64.powi(3 - digits_before_point);
        let rounded = (value * scale).round() / scale;
        if rounded < 1_000.0 || suffix == "B" {
            let decimals = if rounded < 10.0 {
                2
            } else if rounded < 100.0 {
                1
            } else {
                0
            };
            return format!("{rounded:.decimals$}{suffix}");
        }
    }
    unreachable!("the last suffix always returns")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn tool_message_shows_a_shell_command_in_a_code_fence() {
        let message = tool_message("shell", &json!({"command": "ls -la\npwd"}));
        assert_eq!(
            message,
            "📦 **Running a command:**\n```sh\nls -la\npwd\n```"
        );
    }

    #[test]
    fn tool_message_fence_outlasts_backticks_in_the_command() {
        let message = tool_message("shell", &json!({"command": "echo '```'"}));
        assert!(message.contains("````sh\necho '```'\n````"), "{message}");
    }

    #[test]
    fn tool_message_truncates_a_long_shell_command_within_discords_limit() {
        let message = tool_message("shell", &json!({"command": "x".repeat(5000)}));
        assert!(
            message.chars().count() < 2000,
            "{}",
            message.chars().count()
        );
        assert!(message.contains("…\n```"));
    }

    #[test]
    fn tool_message_puts_the_detail_after_the_label() {
        let message = tool_message("web_search", &json!({"query": "rust `async`"}));
        assert_eq!(message, "🔎 **Searching the web** · `rust 'async'`");
        assert_eq!(
            tool_message("get_bot_features", &json!({})),
            "🤖 **Checking my features**"
        );
    }

    #[test]
    fn tool_message_suppresses_the_link_preview_of_a_url() {
        let message = tool_message("fetch_webpage", &json!({"url": "https://example.com/a"}));
        assert_eq!(
            message,
            "🌐 **Reading a webpage** · <https://example.com/a>"
        );
    }

    #[test]
    fn tool_summary_groups_calls_in_first_call_order() {
        let tools: Vec<String> = ["web_search", "fetch_webpage", "web_search", "new_tool"]
            .into_iter()
            .map(String::from)
            .collect();
        assert_eq!(
            tool_summary(&tools),
            "🛠️ **4 tool calls** · 🔎 web search ×2 · 🌐 webpage · 🔧 `new_tool`"
        );
        assert_eq!(
            tool_summary(&["shell".to_string()]),
            "🛠️ **1 tool call** · 📦 command"
        );
    }

    #[test]
    fn tool_message_falls_back_to_the_label_for_an_empty_shell_command() {
        assert_eq!(
            tool_message("shell", &json!({"command": "  "})),
            "📦 **Running a command**"
        );
    }

    #[test]
    fn format_tokens_leaves_sub_thousand_counts_exact() {
        assert_eq!(format_tokens(0), "0");
        assert_eq!(format_tokens(999), "999");
    }

    #[test]
    fn format_tokens_rounds_to_three_significant_figures() {
        assert_eq!(format_tokens(1_000), "1.00k");
        assert_eq!(format_tokens(1_234), "1.23k");
        assert_eq!(format_tokens(12_345), "12.3k");
        assert_eq!(format_tokens(123_456), "123k");
        assert_eq!(format_tokens(1_234_567), "1.23M");
        assert_eq!(format_tokens(12_345_678_901), "12.3B");
    }

    #[test]
    fn format_tokens_carries_into_the_next_unit_when_rounding_up() {
        assert_eq!(format_tokens(9_996), "10.0k");
        assert_eq!(format_tokens(999_499), "999k");
        assert_eq!(format_tokens(999_500), "1.00M");
    }

    #[test]
    fn tool_hint_shell_shows_command() {
        let args = json!({"command": "ls -la /tmp"});
        let hint = tool_hint("shell", &args);
        assert_eq!(hint, " — ls -la /tmp");
    }

    #[test]
    fn tool_hint_shell_truncates_long_command() {
        let command = "x".repeat(100);
        let args = json!({"command": command});
        let hint = tool_hint("shell", &args);
        assert_eq!(hint, format!(" — {}…", "x".repeat(80)));
    }

    #[test]
    fn tool_hint_shell_missing_command() {
        let args = json!({});
        let hint = tool_hint("shell", &args);
        assert_eq!(hint, "");
    }

    #[test]
    fn tool_hint_shell_negated_command_empty() {
        let args = json!({"command": ""});
        let hint = tool_hint("shell", &args);
        assert_eq!(hint, "");
    }

    #[test]
    fn tool_hint_query_is_shown() {
        let args = json!({"query": "rust async patterns"});
        let hint = tool_hint("web_search", &args);
        assert_eq!(hint, " — rust async patterns");
    }

    #[test]
    fn tool_hint_task_is_shown() {
        let args = json!({"task": "implement feature"});
        let hint = tool_hint("some_tool", &args);
        assert_eq!(hint, " — implement feature");
    }

    #[test]
    fn tool_hint_url_is_shown() {
        let args = json!({"url": "https://example.com"});
        let hint = tool_hint("fetch_webpage", &args);
        assert_eq!(hint, " — https://example.com");
    }

    #[test]
    fn tool_hint_fallback_prefers_first_nonempty_key() {
        let args = json!({"query": "", "task": "real task", "url": "https://example.com"});
        let hint = tool_hint("generic_tool", &args);
        assert_eq!(hint, " — real task");
    }

    #[test]
    fn tool_hint_redactable_command() {
        let args = json!({"command": "export MY_SECRET_KEY=hunter2"});
        let hint = tool_hint("shell", &args);
        assert_eq!(hint, " — export MY_SECRET_KEY=hunter2");
    }
}
