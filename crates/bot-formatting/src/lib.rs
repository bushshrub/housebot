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
        "run_skill" if !get("name").is_empty() => format!(
            " — {}: {}",
            get("name"),
            truncate(get("input"), 60).replace('\n', " ")
        ),
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
        "run_skill" | "set_reminder" | "translate" => String::new(),
        _ => [
            "query",
            "task",
            "repo_url",
            "memory_content",
            "url",
            "command",
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

/// User-facing status shown while an agent tool is executing.
pub fn tool_status(tool_name: &str) -> String {
    let icon = match tool_name {
        "web_search" | "deep_research" => "🔎",
        "fetch_webpage" | "summarize_url" => "🌐",
        "common_crawl__search" => "🗂️",
        "download_file" => "📥",
        "run_lua" => "⚙️",
        "get_lua_docs" => "📖",
        "run_skill" => "🧩",
        "translate" => "🌐",
        "set_reminder" => "⏰",
        "get_messages" => "💬",
        "find_discord_users" | "get_discord_user" => "👤",
        "get_bot_features" => "🤖",
        "create_feature_request" | "edit_feature_request" => "📝",
        "prepare_feature_development" => "🛠️",
        _ if tool_name.starts_with("jellyfin__") => "🎬",
        _ => "🔧",
    };
    format!("{icon} **Running `{}`...**", display_tool_name(tool_name))
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

pub fn append_tool_summary(text: &str, tools: &[String]) -> String {
    let summary = if tools.is_empty() {
        "none".to_string()
    } else {
        tools
            .iter()
            .map(|tool| format!("`{tool}`"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    format!("{text}\n\n🛠️ **Tools used:** {summary}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn tool_hint_sandbox_run_shows_command() {
        let args = json!({"command": "ls -la /tmp"});
        let hint = tool_hint("sandbox_run", &args);
        assert_eq!(hint, " — ls -la /tmp");
    }

    #[test]
    fn tool_hint_sandbox_run_truncates_long_command() {
        let command = "x".repeat(100);
        let args = json!({"command": command});
        let hint = tool_hint("sandbox_run", &args);
        assert_eq!(hint, format!(" — {}…", "x".repeat(80)));
    }

    #[test]
    fn tool_hint_sandbox_run_missing_command() {
        let args = json!({});
        let hint = tool_hint("sandbox_run", &args);
        assert_eq!(hint, "");
    }

    #[test]
    fn tool_hint_sandbox_run_negated_command_empty() {
        let args = json!({"command": ""});
        let hint = tool_hint("sandbox_run", &args);
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
    fn tool_hint_run_skill_shows_name_and_input() {
        let args = json!({"name": "greet", "input": "Hello world"});
        let hint = tool_hint("run_skill", &args);
        assert_eq!(hint, " — greet: Hello world");
    }

    #[test]
    fn tool_hint_run_skill_empty_input_still_shows_name() {
        let args = json!({"name": "greet", "input": ""});
        let hint = tool_hint("run_skill", &args);
        assert_eq!(hint, " — greet: ");
    }

    #[test]
    fn tool_hint_run_skill_hides_when_no_name() {
        let args = json!({"input": "something"});
        let hint = tool_hint("run_skill", &args);
        assert_eq!(hint, "");
    }

    #[test]
    fn tool_hint_set_reminder_shows_delay_and_message() {
        let args = json!({"delay_minutes": 15, "message": "check the oven"});
        let hint = tool_hint("set_reminder", &args);
        assert_eq!(hint, " — in 15m: check the oven");
    }

    #[test]
    fn tool_hint_set_reminder_hides_when_no_message() {
        let args = json!({"delay_minutes": 15});
        let hint = tool_hint("set_reminder", &args);
        assert_eq!(hint, "");
    }

    #[test]
    fn tool_hint_translate_shows_target_and_text() {
        let args = json!({"target_language": "es", "text": "hello"});
        let hint = tool_hint("translate", &args);
        assert_eq!(hint, " — → es: hello");
    }

    #[test]
    fn tool_hint_translate_hides_when_no_target() {
        let args = json!({"text": "hello"});
        let hint = tool_hint("translate", &args);
        assert_eq!(hint, "");
    }

    #[test]
    fn tool_hint_redactable_command() {
        let args = json!({"command": "export MY_SECRET_KEY=hunter2"});
        let hint = tool_hint("sandbox_run", &args);
        assert_eq!(hint, " — export MY_SECRET_KEY=hunter2");
    }
}
