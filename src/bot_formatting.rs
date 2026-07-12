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
        _ => ["query", "task", "repo_url", "memory_content", "url"]
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

pub(crate) fn append_tool_summary(text: &str, tools: &[String]) -> String {
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
