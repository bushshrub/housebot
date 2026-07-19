//! Global per-channel message log used by the `search_messages` agent tool.
//!
//! Each channel has a JSONL file (`<dir>/<channel_id>.jsonl`). Every non-bot
//! guild message is appended on arrival. The search function reads the file and
//! applies a regex to message content, returning only matching entries — which
//! keeps token usage proportional to what the model actually needs.

use std::collections::HashMap;
use std::io::{BufRead as _, BufReader};
use std::path::{Path, PathBuf};

use chrono::{Duration, Utc};
use regex::Regex;
use serde_json::{json, Value};

use housebot_config as config;
use housebot_memory::ensure_dir;

#[derive(Debug, Clone)]
pub struct LogEntry {
    pub ts: String,
    pub user_id: String,
    pub username: String,
    /// Server nickname or global display name, if different from username.
    pub nick: Option<String>,
    pub content: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KnownAuthor {
    pub user_id: String,
    pub username: String,
    pub nick: Option<String>,
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
    ///
    /// `nick` is the server nickname or global display name when it differs from the
    /// Discord username; pass `None` if the username is the only name to store.
    pub async fn append(
        &self,
        channel_id: u64,
        user_id: u64,
        username: &str,
        nick: Option<&str>,
        content: &str,
    ) {
        if let Err(e) = self
            .try_append(channel_id, user_id, username, nick, content)
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
        nick: Option<&str>,
        content: &str,
    ) -> std::io::Result<()> {
        ensure_dir(&self.dir).await?;
        let entry = json!({
            "ts": Utc::now().to_rfc3339(),
            "uid": user_id.to_string(),
            "name": username,
            "nick": nick,
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
            // Every kept line keeps its own trailing newline: rewriting the
            // file without one would make the next `append` glue its JSON
            // onto the last line, corrupting both entries.
            let new_content: String = raw
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
                .map(|line| format!("{line}\n"))
                .collect();
            tokio::fs::write(&path, new_content).await?;
        }
        Ok(())
    }

    /// Search messages in `channel_id` whose content or author name matches `pattern` (regex).
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

    /// Return all messages in `channel_id` from the last `minutes` minutes, in
    /// chronological order.
    pub async fn get_recent(&self, channel_id: u64, minutes: u32) -> Result<Vec<LogEntry>, String> {
        let path = self.path(channel_id);
        tokio::task::spawn_blocking(move || get_recent_sync(&path, minutes))
            .await
            .map_err(|e| format!("Error: {e}"))?
    }

    /// Find distinct authors previously seen in a channel by username, nickname, or ID.
    pub async fn find_authors(
        &self,
        channel_id: u64,
        query: &str,
        max_results: usize,
    ) -> Result<Vec<KnownAuthor>, String> {
        let path = self.path(channel_id);
        let query = query.trim().to_lowercase();
        tokio::task::spawn_blocking(move || find_authors_sync(&path, &query, max_results))
            .await
            .map_err(|e| format!("Author search error: {e}"))?
    }
}

fn find_authors_sync(
    path: &Path,
    query: &str,
    max_results: usize,
) -> Result<Vec<KnownAuthor>, String> {
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(error) => return Err(format!("Could not open channel log: {error}")),
    };
    let mut authors = HashMap::new();
    for line in BufReader::new(file).lines() {
        let Ok(line) = line else { continue };
        let Ok(value) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let user_id = value["uid"].as_str().unwrap_or("").to_string();
        if user_id.is_empty() {
            continue;
        }
        authors.insert(
            user_id.clone(),
            KnownAuthor {
                user_id,
                username: value["name"].as_str().unwrap_or("").to_string(),
                nick: value["nick"].as_str().map(str::to_string),
            },
        );
    }
    let query_words: Vec<&str> = query.split_whitespace().filter(|w| !w.is_empty()).collect();
    let mut matches: Vec<KnownAuthor> = authors
        .into_values()
        .filter(|author| {
            if query_words.is_empty() {
                return true;
            }
            query_words.iter().any(|word| {
                author.user_id.contains(*word)
                    || author.username.to_lowercase().contains(word)
                    || author
                        .nick
                        .as_deref()
                        .is_some_and(|nick| nick.to_lowercase().contains(word))
            })
        })
        .collect();
    matches.sort_by(|left, right| {
        left.username
            .to_lowercase()
            .cmp(&right.username.to_lowercase())
            .then_with(|| left.user_id.cmp(&right.user_id))
    });
    matches.truncate(max_results);
    Ok(matches)
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
        let nick = val["nick"].as_str().map(str::to_string);
        let matches_nick = nick.as_deref().is_some_and(|n| re.is_match(n));
        if re.is_match(&content) || re.is_match(&username) || matches_nick {
            matches.push(LogEntry {
                ts: val["ts"].as_str().unwrap_or("").to_string(),
                user_id: val["uid"].as_str().unwrap_or("").to_string(),
                username,
                nick,
                content,
            });
        }
    }
    let skip = matches.len().saturating_sub(max_results);
    Ok(matches.into_iter().skip(skip).collect())
}

fn get_recent_sync(path: &Path, minutes: u32) -> Result<Vec<LogEntry>, String> {
    let file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(e) => return Err(format!("Could not open channel log: {e}")),
    };
    let cutoff = Utc::now() - Duration::minutes(i64::from(minutes));
    let mut entries = Vec::new();
    for line in BufReader::new(file).lines() {
        let Ok(line) = line else { continue };
        let Ok(val) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let ts_str = val["ts"].as_str().unwrap_or("");
        let Ok(ts) = ts_str.parse::<chrono::DateTime<Utc>>() else {
            continue;
        };
        if ts >= cutoff {
            let username = val["name"].as_str().unwrap_or("").to_string();
            let nick = val["nick"].as_str().map(str::to_string);
            entries.push(LogEntry {
                ts: ts_str.to_string(),
                user_id: val["uid"].as_str().unwrap_or("").to_string(),
                username,
                nick,
                content: val["msg"].as_str().unwrap_or("").to_string(),
            });
        }
    }
    Ok(entries)
}

async fn append_line(path: &Path, line: &str) -> std::io::Result<()> {
    use tokio::io::AsyncWriteExt as _;
    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .await?;
    file.write_all(line.as_bytes()).await?;
    file.flush().await
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
        log.append(1, 10, "Alice", None, "hello world").await;
        log.append(1, 11, "Bob", None, "goodbye moon").await;
        let results = log.search(1, "hello", 10).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].username, "Alice");
        assert_eq!(results[0].content, "hello world");
    }

    #[tokio::test]
    async fn find_authors_matches_username_nickname_and_id() {
        let (_t, log) = store();
        log.append(1, 10, "alice_dev", Some("Alice"), "hello").await;
        log.append(1, 11, "bob", Some("Builder"), "hi").await;
        log.append(2, 12, "outside", None, "hidden").await;

        assert_eq!(
            log.find_authors(1, "ALICE", 10).await.unwrap()[0].user_id,
            "10"
        );
        assert_eq!(
            log.find_authors(1, "build", 10).await.unwrap()[0].user_id,
            "11"
        );
        assert_eq!(
            log.find_authors(1, "10", 10).await.unwrap()[0].username,
            "alice_dev"
        );
        assert!(log.find_authors(1, "outside", 10).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn find_authors_fuzzy_matches_any_word() {
        let (_t, log) = store();
        log.append(1, 10, "rice_grower", Some("Grower"), "hello")
            .await;
        log.append(1, 11, "wheat_grower", Some("Wheat Farmer"), "hi")
            .await;
        log.append(1, 12, "corn_king", Some("Corn"), "hey").await;

        // "rice farmer" should match both users 10 (username has "rice") and 11 (nick has "farmer")
        let results = log.find_authors(1, "rice farmer", 10).await.unwrap();
        assert_eq!(results.len(), 2);
        assert!(results.iter().any(|a| a.user_id == "10"));
        assert!(results.iter().any(|a| a.user_id == "11"));
    }

    #[tokio::test]
    async fn find_authors_deduplicates_and_keeps_latest_names() {
        let (_t, log) = store();
        log.append(1, 10, "alice", None, "hello").await;
        log.append(1, 10, "alice_new", Some("Ali"), "again").await;
        let authors = log.find_authors(1, "", 10).await.unwrap();
        assert_eq!(authors.len(), 1);
        assert_eq!(authors[0].username, "alice_new");
        assert_eq!(authors[0].nick.as_deref(), Some("Ali"));
    }

    #[tokio::test]
    async fn search_returns_no_match() {
        let (_t, log) = store();
        log.append(1, 10, "Alice", None, "hello world").await;
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
            log.append(1, i, "User", None, "match").await;
        }
        let results = log.search(1, "match", 3).await.unwrap();
        assert_eq!(results.len(), 3);
    }

    #[tokio::test]
    async fn search_returns_most_recent_when_capped() {
        let (_t, log) = store();
        log.append(1, 1, "First", None, "match").await;
        log.append(1, 2, "Second", None, "match").await;
        log.append(1, 3, "Third", None, "match").await;
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
        log.append(1, 10, "Alice", None, "channel one").await;
        log.append(2, 11, "Bob", None, "channel two").await;
        assert_eq!(log.search(1, "channel", 10).await.unwrap().len(), 1);
        assert_eq!(log.search(2, "channel", 10).await.unwrap().len(), 1);
        assert!(log.search(1, "two", 10).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn search_matches_username() {
        let (_t, log) = store();
        log.append(1, 10, "AliceWonder", None, "some message").await;
        log.append(1, 11, "BobSmith", None, "another message").await;
        let results = log.search(1, "Alice", 10).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].username, "AliceWonder");
    }

    #[tokio::test]
    async fn entries_have_valid_timestamp_and_ids() {
        let (_t, log) = store();
        log.append(1, 42, "TestUser", None, "content").await;
        let results = log.search(1, "content", 10).await.unwrap();
        assert_eq!(results[0].user_id, "42");
        assert!(!results[0].ts.is_empty());
    }

    #[tokio::test]
    async fn remove_user_entries_removes_matching_user() {
        let (_t, log) = store();
        log.append(1, 10, "Alice", None, "hello").await;
        log.append(1, 20, "Bob", None, "world").await;
        log.append(1, 10, "Alice", None, "foo").await;
        log.remove_user_entries("10".to_string()).await.unwrap();
        let results = log.search(1, ".*", 10).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].user_id, "20");
    }

    #[tokio::test]
    async fn remove_user_entries_preserves_other_users() {
        let (_t, log) = store();
        log.append(1, 10, "Alice", None, "hello").await;
        log.append(1, 20, "Bob", None, "world").await;
        log.append(1, 30, "Charlie", None, "bar").await;
        log.remove_user_entries("10".to_string()).await.unwrap();
        let results = log.search(1, ".*", 10).await.unwrap();
        assert_eq!(results.len(), 2);
        assert!(results.iter().any(|r| r.user_id == "20"));
        assert!(results.iter().any(|r| r.user_id == "30"));
    }

    #[tokio::test]
    async fn remove_user_entries_noop_when_user_not_found() {
        let (_t, log) = store();
        log.append(1, 10, "Alice", None, "hello").await;
        log.remove_user_entries("999".to_string()).await.unwrap();
        let results = log.search(1, ".*", 10).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].user_id, "10");
    }

    #[tokio::test]
    async fn append_after_remove_user_entries_does_not_corrupt_the_log() {
        let (_t, log) = store();
        log.append(1, 10, "Alice", None, "hello").await;
        log.append(1, 20, "Bob", None, "world").await;
        log.remove_user_entries("10".to_string()).await.unwrap();
        log.append(1, 30, "Charlie", None, "after removal").await;
        let results = log.search(1, ".*", 10).await.unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].user_id, "20");
        assert_eq!(results[1].user_id, "30");
        assert_eq!(results[1].content, "after removal");
    }

    #[tokio::test]
    async fn remove_user_entries_noop_when_directory_is_missing() {
        let (_t, log) = store();
        log.remove_user_entries("10".to_string()).await.unwrap();
    }

    #[tokio::test]
    async fn remove_user_entries_removes_from_all_channels() {
        let (_t, log) = store();
        log.append(1, 10, "Alice", None, "channel one").await;
        log.append(2, 10, "Alice", None, "channel two").await;
        log.append(1, 20, "Bob", None, "channel one bob").await;
        log.remove_user_entries("10".to_string()).await.unwrap();
        let results1 = log.search(1, ".*", 10).await.unwrap();
        let results2 = log.search(2, ".*", 10).await.unwrap();
        assert_eq!(results1.len(), 1);
        assert_eq!(results1[0].user_id, "20");
        assert!(results2.is_empty());
    }

    #[tokio::test]
    async fn search_matches_nick() {
        let (_t, log) = store();
        log.append(1, 10, "username1", Some("Teddio"), "some message")
            .await;
        log.append(1, 11, "username2", None, "another message")
            .await;
        let results = log.search(1, "(?i)teddio", 10).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].username, "username1");
        assert_eq!(results[0].nick, Some("Teddio".to_string()));
    }

    #[tokio::test]
    async fn get_recent_returns_messages_within_window() {
        let (_t, log) = store();
        log.append(1, 10, "Alice", None, "recent message").await;
        let results = log.get_recent(1, 5).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].content, "recent message");
    }

    #[tokio::test]
    async fn get_recent_empty_for_missing_channel() {
        let (_t, log) = store();
        let results = log.get_recent(999, 30).await.unwrap();
        assert!(results.is_empty());
    }
}
