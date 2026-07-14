//! Global per-channel message log used by the `search_messages` agent tool.
//!
//! Each channel has a JSONL file (`<dir>/<channel_id>.jsonl`). Every non-bot
//! guild message is appended on arrival. The search function reads the file and
//! applies a regex to message content, returning only matching entries — which
//! keeps token usage proportional to what the model actually needs.

use std::io::{BufRead as _, BufReader};
use std::path::{Path, PathBuf};

use chrono::Utc;
use regex::Regex;
use serde_json::{json, Value};

use crate::config;
use crate::memory::ensure_dir;

#[derive(Debug, Clone)]
pub struct LogEntry {
    pub ts: String,
    pub user_id: String,
    pub username: String,
    pub content: String,
}

#[derive(Clone)]
pub struct ChannelLog {
    dir: PathBuf,
}

impl Default for ChannelLog {
    fn default() -> Self {
        Self::new(config::data_dir().join("channel_log"))
    }
}

impl ChannelLog {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    fn path(&self, channel_id: u64) -> PathBuf {
        self.dir.join(format!("{channel_id}.jsonl"))
    }

    /// Append a message (fire-and-forget; errors are logged but not returned).
    pub async fn append(&self, channel_id: u64, user_id: u64, username: &str, content: &str) {
        if let Err(e) = self
            .try_append(channel_id, user_id, username, content)
            .await
        {
            tracing::warn!(target: "housebot::channel_log", "Failed to append: {e}");
        }
    }

    async fn try_append(
        &self,
        channel_id: u64,
        user_id: u64,
        username: &str,
        content: &str,
    ) -> std::io::Result<()> {
        ensure_dir(&self.dir).await?;
        let entry = json!({
            "ts": Utc::now().to_rfc3339(),
            "uid": user_id.to_string(),
            "name": username,
            "msg": content,
        });
        let mut line = serde_json::to_string(&entry).unwrap_or_else(|_| "{}".into());
        line.push('\n');
        append_line(&self.path(channel_id), &line).await
    }

    /// Remove all entries for a given user from this channel log file.
    pub async fn remove_user_entries(&self, user_id: String) -> std::io::Result<()> {
        // Read all channel log files and remove entries for this user.
        let entries = match tokio::fs::read_dir(&self.dir).await {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
        };
        let mut paths: Vec<std::path::PathBuf> = Vec::new();
        let mut entries_iter = entries;
        while let Some(entry) = entries_iter.next_entry().await? {
            if entry.path().is_file() {
                paths.push(entry.path());
            }
        }
        for path in paths {
            let raw = match tokio::fs::read_to_string(&path).await {
                Ok(s) => s,
                Err(_) => continue,
            };
            let filtered: Vec<String> = raw
                .lines()
                .filter(|line| {
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        return true;
                    }
                    match serde_json::from_str::<Value>(trimmed) {
                        Ok(val) => val.get("uid").and_then(Value::as_str) != Some(&user_id),
                        Err(_) => true, // Keep non-JSON lines
                    }
                })
                .map(|l| l.to_string())
                .collect();
            let new_content = filtered.join("\n");
            tokio::fs::write(&path, new_content).await?;
        }
        Ok(())
    }

    /// Search messages in `channel_id` whose content matches `pattern` (regex).
    /// Returns up to `max_results` of the most recent matches.
    /// Returns an error string if the regex is invalid.
    pub async fn search(
        &self,
        channel_id: u64,
        pattern: &str,
        max_results: usize,
    ) -> Result<Vec<LogEntry>, String> {
        let re = Regex::new(pattern).map_err(|e| format!("Invalid regex: {e}"))?;
        let path = self.path(channel_id);
        tokio::task::spawn_blocking(move || search_sync(&path, &re, max_results))
            .await
            .map_err(|e| format!("Search error: {e}"))?
    }
}

fn search_sync(path: &Path, re: &Regex, max_results: usize) -> Result<Vec<LogEntry>, String> {
    let file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(e) => return Err(format!("Could not open channel log: {e}")),
    };
    let mut matches = Vec::new();
    for line in BufReader::new(file).lines() {
        let Ok(line) = line else { continue };
        let Ok(val) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let content = val["msg"].as_str().unwrap_or("").to_string();
        let username = val["name"].as_str().unwrap_or("").to_string();
        if re.is_match(&content) || re.is_match(&username) {
            matches.push(LogEntry {
                ts: val["ts"].as_str().unwrap_or("").to_string(),
                user_id: val["uid"].as_str().unwrap_or("").to_string(),
                username,
                content,
            });
        }
    }
    let skip = matches.len().saturating_sub(max_results);
    Ok(matches.into_iter().skip(skip).collect())
}

async fn append_line(path: &Path, line: &str) -> std::io::Result<()> {
    use tokio::io::AsyncWriteExt as _;
    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .await?;
    file.write_all(line.as_bytes()).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn store() -> (TempDir, ChannelLog) {
        let tmp = TempDir::new().unwrap();
        let log = ChannelLog::new(tmp.path().join("channel_log"));
        (tmp, log)
    }

    #[tokio::test]
    async fn append_and_search_basic() {
        let (_t, log) = store();
        log.append(1, 10, "Alice", "hello world").await;
        log.append(1, 11, "Bob", "goodbye moon").await;
        let results = log.search(1, "hello", 10).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].username, "Alice");
        assert_eq!(results[0].content, "hello world");
    }

    #[tokio::test]
    async fn search_returns_no_match() {
        let (_t, log) = store();
        log.append(1, 10, "Alice", "hello world").await;
        let results = log.search(1, "notfound", 10).await.unwrap();
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn search_missing_channel_is_empty() {
        let (_t, log) = store();
        let results = log.search(999, "anything", 10).await.unwrap();
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn search_respects_max_results() {
        let (_t, log) = store();
        for i in 0..10u64 {
            log.append(1, i, "User", "match").await;
        }
        let results = log.search(1, "match", 3).await.unwrap();
        assert_eq!(results.len(), 3);
    }

    #[tokio::test]
    async fn search_returns_most_recent_when_capped() {
        let (_t, log) = store();
        log.append(1, 1, "First", "match").await;
        log.append(1, 2, "Second", "match").await;
        log.append(1, 3, "Third", "match").await;
        let results = log.search(1, "match", 2).await.unwrap();
        assert_eq!(results[0].username, "Second");
        assert_eq!(results[1].username, "Third");
    }

    #[tokio::test]
    async fn search_invalid_regex_returns_error() {
        let (_t, log) = store();
        assert!(log.search(1, "[invalid", 10).await.is_err());
    }

    #[tokio::test]
    async fn channels_are_isolated() {
        let (_t, log) = store();
        log.append(1, 10, "Alice", "channel one").await;
        log.append(2, 11, "Bob", "channel two").await;
        assert_eq!(log.search(1, "channel", 10).await.unwrap().len(), 1);
        assert_eq!(log.search(2, "channel", 10).await.unwrap().len(), 1);
        assert!(log.search(1, "two", 10).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn search_matches_username() {
        let (_t, log) = store();
        log.append(1, 10, "AliceWonder", "some message").await;
        log.append(1, 11, "BobSmith", "another message").await;
        let results = log.search(1, "Alice", 10).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].username, "AliceWonder");
    }

    #[tokio::test]
    async fn entries_have_valid_timestamp_and_ids() {
        let (_t, log) = store();
        log.append(1, 42, "TestUser", "content").await;
        let results = log.search(1, "content", 10).await.unwrap();
        assert_eq!(results[0].user_id, "42");
        assert!(!results[0].ts.is_empty());
    }

    #[tokio::test]
    async fn remove_user_entries_removes_matching_user() {
        let (_t, log) = store();
        log.append(1, 10, "Alice", "hello").await;
        log.append(1, 20, "Bob", "world").await;
        log.append(1, 10, "Alice", "foo").await;
        log.remove_user_entries("10".to_string()).await.unwrap();
        let results = log.search(1, ".*", 10).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].user_id, "20");
    }

    #[tokio::test]
    async fn remove_user_entries_preserves_other_users() {
        let (_t, log) = store();
        log.append(1, 10, "Alice", "hello").await;
        log.append(1, 20, "Bob", "world").await;
        log.append(1, 30, "Charlie", "bar").await;
        log.remove_user_entries("10".to_string()).await.unwrap();
        let results = log.search(1, ".*", 10).await.unwrap();
        assert_eq!(results.len(), 2);
        assert!(results.iter().any(|r| r.user_id == "20"));
        assert!(results.iter().any(|r| r.user_id == "30"));
    }

    #[tokio::test]
    async fn remove_user_entries_noop_when_user_not_found() {
        let (_t, log) = store();
        log.append(1, 10, "Alice", "hello").await;
        log.remove_user_entries("999".to_string()).await.unwrap();
        let results = log.search(1, ".*", 10).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].user_id, "10");
    }

    #[tokio::test]
    async fn remove_user_entries_noop_when_directory_is_missing() {
        let (_t, log) = store();
        log.remove_user_entries("10".to_string()).await.unwrap();
    }

    #[tokio::test]
    async fn remove_user_entries_removes_from_all_channels() {
        let (_t, log) = store();
        log.append(1, 10, "Alice", "channel one").await;
        log.append(2, 10, "Alice", "channel two").await;
        log.append(1, 20, "Bob", "channel one bob").await;
        log.remove_user_entries("10".to_string()).await.unwrap();
        let results1 = log.search(1, ".*", 10).await.unwrap();
        let results2 = log.search(2, ".*", 10).await.unwrap();
        assert_eq!(results1.len(), 1);
        assert_eq!(results1[0].user_id, "20");
        assert!(results2.is_empty());
    }
}
