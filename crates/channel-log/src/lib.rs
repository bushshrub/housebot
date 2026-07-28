//! Global per-channel message log used by the `get_messages` agent tool's
//! search mode.
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

fn normalize(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric() || c.is_whitespace())
        .collect::<String>()
        .to_lowercase()
}

fn fuzzy_match_word(word: &str, target: &str) -> bool {
    let word_norm = normalize(word);
    let target_norm = normalize(target);

    if word_norm.is_empty() {
        return false;
    }

    if target_norm.contains(&word_norm) {
        return true;
    }

    let wlen = word_norm.chars().count();
    if wlen < 4 {
        return false;
    }
    let max_dist = if wlen < 6 {
        1
    } else if wlen < 8 {
        2
    } else {
        3
    };

    for tw in target_norm.split_whitespace() {
        if tw.contains(&word_norm) {
            return true;
        }
        if strsim::levenshtein(&word_norm, tw) <= max_dist {
            return true;
        }
    }

    let tchars: Vec<char> = target_norm.chars().collect();
    if tchars.len() >= wlen {
        for window in tchars.windows(wlen) {
            let substr: String = window.iter().collect();
            if strsim::levenshtein(&word_norm, &substr) <= max_dist {
                return true;
            }
        }
    }

    false
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
    let query_was_nonempty = !query.is_empty();
    let query_words: Vec<String> = query
        .split_whitespace()
        .map(normalize)
        .filter(|w| !w.is_empty())
        .collect();
    let mut matches: Vec<KnownAuthor> = authors
        .into_values()
        .filter(|author| {
            if query_words.is_empty() {
                return !query_was_nonempty;
            }
            query_words.iter().any(|word| {
                fuzzy_match_word(word, &author.user_id)
                    || fuzzy_match_word(word, &author.username)
                    || author
                        .nick
                        .as_deref()
                        .is_some_and(|nick| fuzzy_match_word(word, nick))
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
#[path = "lib_tests.rs"]
mod tests;
