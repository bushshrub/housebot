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
        "use_skill" if !get("name").is_empty() => format!(" — {}", get("name")),
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
        "use_skill" | "set_reminder" | "translate" => String::new(),
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

/// Human-readable icon and label for a tool, or `None` for tools without one.
fn tool_label(tool_name: &str) -> Option<(&'static str, &'static str)> {
    let pair = match tool_name {
        "web_search" => ("🔎", "Searching the web"),
        "fetch_webpage" => ("🌐", "Reading a webpage"),
        "use_skill" => ("🧩", "Using a skill"),
        "list_skills" | "skill_info" | "read_skill_file" => ("🧩", "Looking up skills"),
        "create_skill" | "edit_skill" | "delete_skill" | "enable_skill" | "disable_skill" => {
            ("🧩", "Updating skills")
        }
        "run_skill_script" => ("🧩", "Running a skill script"),
        "spawn_subagent" => ("🧠", "Starting a subagent"),
        "set_reminder" => ("⏰", "Setting a reminder"),
        "get_messages" => ("💬", "Reading conversations"),
        "get_bot_features" => ("🤖", "Checking my features"),
        "configure_bot" => ("⚙️", "Changing bot settings"),
        "update_memory" => ("📓", "Updating memory"),
        "search_memory" => ("📓", "Searching memory"),
        "github_api" => ("🐙", "Checking GitHub"),
        "create_feature_request" => ("📝", "Filing a feature request"),
        "edit_feature_request" => ("📝", "Updating a feature request"),
        "prepare_feature_development" => ("🛠️", "Preparing feature development"),
        "sandbox_clone_repository" => ("📦", "Cloning a repository"),
        "sandbox_list_files" => ("📦", "Listing files"),
        "sandbox_read_file" => ("📦", "Reading a file"),
        "sandbox_search_code" => ("📦", "Searching code"),
        "sandbox_run" => ("📦", "Running a command"),
        _ => return None,
    };
    Some(pair)
}

/// User-facing status shown while an agent tool is executing.
pub fn tool_status(tool_name: &str) -> String {
    match tool_label(tool_name) {
        Some((icon, label)) => format!("{icon} **{label}...**"),
        None => format!("🔧 **Running `{}`...**", display_tool_name(tool_name)),
    }
}

/// Same as [`tool_status`], marked as running inside a sub-agent.
pub fn subagent_tool_status(tool_name: &str) -> String {
    format!("╰ {}", tool_status(tool_name))
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
    fn tool_hint_redactable_command() {
        let args = json!({"command": "export MY_SECRET_KEY=hunter2"});
        let hint = tool_hint("sandbox_run", &args);
        assert_eq!(hint, " — export MY_SECRET_KEY=hunter2");
    }
}
