//! Per-channel message ring buffer backing the `get_messages` tool's search mode.
//!
//! Messages live in RAM only and are never written to disk: a restart starts
//! the buffer empty, and each channel keeps at most `CHANNEL_CONTEXT_CAPACITY`
//! of its most recent messages.

use std::collections::HashMap;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use chrono::Utc;
use regex::Regex;

use housebot_config as config;

pub const DEFAULT_CAPACITY: usize = 2000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub ts: String,
    pub user_id: String,
    pub username: String,
    /// Server nickname or global display name, when it differs from the username.
    pub nick: Option<String>,
    pub content: String,
}

#[derive(Clone)]
pub struct ChannelContext {
    channels: Arc<Mutex<HashMap<u64, VecDeque<Message>>>>,
    capacity: usize,
}

impl Default for ChannelContext {
    fn default() -> Self {
        Self::new(config::env_parse(
            "CHANNEL_CONTEXT_CAPACITY",
            DEFAULT_CAPACITY,
        ))
    }
}

impl ChannelContext {
    /// Create a buffer keeping the most recent `capacity` messages per channel.
    pub fn new(capacity: usize) -> Self {
        Self {
            channels: Arc::new(Mutex::new(HashMap::new())),
            capacity: capacity.max(1),
        }
    }

    /// Record a message, evicting the channel's oldest once it is full.
    ///
    /// `nick` is the server nickname or global display name when it differs
    /// from the Discord username; pass `None` when the username is the only
    /// name to store.
    pub fn append(
        &self,
        channel_id: u64,
        user_id: u64,
        username: &str,
        nick: Option<&str>,
        content: &str,
    ) {
        let mut channels = self.lock();
        let buffer = channels.entry(channel_id).or_default();
        if buffer.len() == self.capacity {
            buffer.pop_front();
        }
        buffer.push_back(Message {
            ts: Utc::now().to_rfc3339(),
            user_id: user_id.to_string(),
            username: username.to_string(),
            nick: nick.map(str::to_string),
            content: content.to_string(),
        });
    }

    /// Messages in `channel_id` whose content or author name matches `pattern`,
    /// most recent last, capped at `max_results`.
    pub fn search(
        &self,
        channel_id: u64,
        pattern: &str,
        max_results: usize,
    ) -> Result<Vec<Message>, String> {
        let regex = Regex::new(pattern).map_err(|error| format!("Invalid regex: {error}"))?;
        let channels = self.lock();
        let Some(buffer) = channels.get(&channel_id) else {
            return Ok(Vec::new());
        };
        let matches = buffer.iter().filter(|message| {
            regex.is_match(&message.content)
                || regex.is_match(&message.username)
                || message
                    .nick
                    .as_deref()
                    .is_some_and(|nick| regex.is_match(nick))
        });
        let mut recent: VecDeque<Message> = VecDeque::with_capacity(max_results);
        for message in matches {
            if recent.len() == max_results {
                recent.pop_front();
            }
            recent.push_back(message.clone());
        }
        Ok(recent.into())
    }

    /// Forget everything a user has said, across every channel.
    pub fn remove_user_entries(&self, user_id: &str) {
        let mut channels = self.lock();
        for buffer in channels.values_mut() {
            buffer.retain(|message| message.user_id != user_id);
        }
    }

    /// How many messages are buffered for `channel_id`.
    pub fn len(&self, channel_id: u64) -> usize {
        self.lock().get(&channel_id).map_or(0, VecDeque::len)
    }

    pub fn is_empty(&self, channel_id: u64) -> bool {
        self.len(channel_id) == 0
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<u64, VecDeque<Message>>> {
        self.channels.lock().expect("channel context lock poisoned")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context() -> ChannelContext {
        ChannelContext::new(DEFAULT_CAPACITY)
    }

    #[test]
    fn append_and_search_basic() {
        let context = context();
        context.append(1, 10, "Alice", None, "hello world");
        context.append(1, 11, "Bob", None, "goodbye moon");
        let results = context.search(1, "hello", 10).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].username, "Alice");
        assert_eq!(results[0].content, "hello world");
    }

    #[test]
    fn search_returns_no_match() {
        let context = context();
        context.append(1, 10, "Alice", None, "hello world");
        assert!(context.search(1, "notfound", 10).unwrap().is_empty());
    }

    #[test]
    fn search_missing_channel_is_empty() {
        assert!(context().search(999, "anything", 10).unwrap().is_empty());
    }

    #[test]
    fn search_respects_max_results() {
        let context = context();
        for user_id in 0..10u64 {
            context.append(1, user_id, "User", None, "match");
        }
        assert_eq!(context.search(1, "match", 3).unwrap().len(), 3);
    }

    #[test]
    fn search_returns_most_recent_when_capped() {
        let context = context();
        context.append(1, 1, "First", None, "match");
        context.append(1, 2, "Second", None, "match");
        context.append(1, 3, "Third", None, "match");
        let results = context.search(1, "match", 2).unwrap();
        assert_eq!(results[0].username, "Second");
        assert_eq!(results[1].username, "Third");
    }

    #[test]
    fn search_invalid_regex_returns_error() {
        assert!(context().search(1, "[invalid", 10).is_err());
    }

    #[test]
    fn search_matches_username_and_nick() {
        let context = context();
        context.append(1, 10, "username1", Some("Teddio"), "some message");
        context.append(1, 11, "AliceWonder", None, "another message");
        context.append(1, 12, "username3", None, "unrelated");

        let by_nick = context.search(1, "(?i)teddio", 10).unwrap();
        assert_eq!(by_nick.len(), 1);
        assert_eq!(by_nick[0].nick.as_deref(), Some("Teddio"));

        let by_username = context.search(1, "Alice", 10).unwrap();
        assert_eq!(by_username.len(), 1);
        assert_eq!(by_username[0].username, "AliceWonder");
    }

    #[test]
    fn channels_are_isolated() {
        let context = context();
        context.append(1, 10, "Alice", None, "channel one");
        context.append(2, 11, "Bob", None, "channel two");
        assert_eq!(context.search(1, "channel", 10).unwrap().len(), 1);
        assert_eq!(context.search(2, "channel", 10).unwrap().len(), 1);
        assert!(context.search(1, "two", 10).unwrap().is_empty());
    }

    #[test]
    fn entries_have_a_timestamp_and_author_id() {
        let context = context();
        context.append(1, 42, "TestUser", None, "content");
        let results = context.search(1, "content", 10).unwrap();
        assert_eq!(results[0].user_id, "42");
        assert!(!results[0].ts.is_empty());
    }

    #[test]
    fn the_buffer_evicts_the_oldest_message_when_full() {
        let context = ChannelContext::new(3);
        for index in 0..5u64 {
            context.append(1, index, "User", None, &format!("message {index}"));
        }
        assert_eq!(context.len(1), 3, "the buffer is bounded");
        let results = context.search(1, "message", 10).unwrap();
        assert_eq!(results[0].content, "message 2", "the oldest are dropped");
        assert_eq!(results[2].content, "message 4");
    }

    #[test]
    fn a_full_buffer_holds_capacity_per_channel_not_in_total() {
        let context = ChannelContext::new(2);
        for channel_id in 1..=3u64 {
            for index in 0..4u64 {
                context.append(channel_id, index, "User", None, "match");
            }
        }
        for channel_id in 1..=3u64 {
            assert_eq!(context.len(channel_id), 2);
        }
    }

    #[test]
    fn a_zero_capacity_still_buffers_one_message() {
        let context = ChannelContext::new(0);
        context.append(1, 10, "Alice", None, "hello");
        assert_eq!(context.len(1), 1);
    }

    #[test]
    fn remove_user_entries_removes_only_that_user() {
        let context = context();
        context.append(1, 10, "Alice", None, "hello");
        context.append(1, 20, "Bob", None, "world");
        context.append(1, 10, "Alice", None, "foo");
        context.remove_user_entries("10");
        let results = context.search(1, ".*", 10).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].user_id, "20");
    }

    #[test]
    fn remove_user_entries_spans_every_channel() {
        let context = context();
        context.append(1, 10, "Alice", None, "channel one");
        context.append(2, 10, "Alice", None, "channel two");
        context.append(1, 20, "Bob", None, "channel one bob");
        context.remove_user_entries("10");
        assert_eq!(context.search(1, ".*", 10).unwrap().len(), 1);
        assert!(context.is_empty(2));
    }

    #[test]
    fn remove_user_entries_is_a_noop_for_an_unknown_user() {
        let context = context();
        context.append(1, 10, "Alice", None, "hello");
        context.remove_user_entries("999");
        assert_eq!(context.len(1), 1);
    }

    #[test]
    fn appending_after_a_removal_keeps_the_buffer_ordered() {
        let context = context();
        context.append(1, 10, "Alice", None, "hello");
        context.append(1, 20, "Bob", None, "world");
        context.remove_user_entries("10");
        context.append(1, 30, "Charlie", None, "after removal");
        let results = context.search(1, ".*", 10).unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].user_id, "20");
        assert_eq!(results[1].content, "after removal");
    }

    #[test]
    fn the_buffer_is_shared_between_clones() {
        let context = context();
        let clone = context.clone();
        clone.append(1, 10, "Alice", None, "hello");
        assert_eq!(context.len(1), 1, "handles must see one another's writes");
    }
}
