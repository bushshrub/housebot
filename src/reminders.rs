//! Per-user reminders persisted as a single JSON array (`<dir>/reminders.json`).

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::config;
use crate::memory::ensure_dir;

/// A pending reminder: a message to DM `user_id` once `due_ts` (unix seconds) passes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Reminder {
    pub user_id: String,
    pub message: String,
    pub due_ts: f64,
}

/// Handle to the reminders store.
#[derive(Clone)]
pub struct Reminders {
    path: PathBuf,
}

impl Default for Reminders {
    fn default() -> Self {
        Self::new(config::data_dir().join("reminders.json"))
    }
}

impl Reminders {
    /// Create a store backed by the JSON file at `path`.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub async fn load(&self) -> Vec<Reminder> {
        let raw = match tokio::fs::read_to_string(&self.path).await {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        if raw.trim().is_empty() {
            return Vec::new();
        }
        serde_json::from_str(&raw).unwrap_or_default()
    }

    /// Save reminders to disk.
    pub async fn store(&self, reminders: &[Reminder]) -> std::io::Result<()> {
        if let Some(parent) = self.path.parent() {
            ensure_dir(parent).await?;
        }
        let body = serde_json::to_string_pretty(reminders).unwrap_or_else(|_| "[]".into());
        tokio::fs::write(&self.path, body).await
    }

    /// Add a reminder for `user_id` due at `due_ts` (unix seconds).
    pub async fn add(&self, user_id: &str, message: &str, due_ts: f64) -> std::io::Result<()> {
        let mut reminders = self.load().await;
        reminders.push(Reminder {
            user_id: user_id.to_string(),
            message: message.to_string(),
            due_ts,
        });
        self.store(&reminders).await
    }

    /// Return and remove every reminder whose due time is at or before `now`.
    pub async fn pop_due(&self, now: f64) -> Vec<Reminder> {
        let reminders = self.load().await;
        let (due, remaining): (Vec<_>, Vec<_>) =
            reminders.into_iter().partition(|r| r.due_ts <= now);
        if !due.is_empty() {
            let _ = self.store(&remaining).await;
        }
        due
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn store() -> (TempDir, Reminders) {
        let tmp = TempDir::new().unwrap();
        let r = Reminders::new(tmp.path().join("reminders.json"));
        (tmp, r)
    }

    fn now() -> f64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs_f64()
    }

    #[tokio::test]
    async fn add_creates_file() {
        let (_t, r) = store();
        r.add("123", "hello", now() + 60.0).await.unwrap();
        assert!(r.load().await.len() == 1);
    }

    #[tokio::test]
    async fn load_empty_when_no_file() {
        let (_t, r) = store();
        assert!(r.load().await.is_empty());
    }

    #[tokio::test]
    async fn add_stores_fields() {
        let (_t, r) = store();
        let due = now() + 100.0;
        r.add("42", "test message", due).await.unwrap();
        let loaded = r.load().await;
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].user_id, "42");
        assert_eq!(loaded[0].message, "test message");
        assert!((loaded[0].due_ts - due).abs() < 1e-3);
    }

    #[tokio::test]
    async fn multiple_reminders_stored() {
        let (_t, r) = store();
        r.add("1", "first", now() + 60.0).await.unwrap();
        r.add("2", "second", now() + 120.0).await.unwrap();
        assert_eq!(r.load().await.len(), 2);
    }

    #[tokio::test]
    async fn pop_due_returns_due_reminders() {
        let (_t, r) = store();
        r.add("1", "past", now() - 10.0).await.unwrap();
        r.add("2", "future", now() + 100.0).await.unwrap();
        let due = r.pop_due(now()).await;
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].message, "past");
    }

    #[tokio::test]
    async fn pop_due_removes_due_from_file() {
        let (_t, r) = store();
        r.add("1", "old", now() - 5.0).await.unwrap();
        r.pop_due(now()).await;
        assert!(r.load().await.is_empty());
    }

    #[tokio::test]
    async fn future_reminders_not_removed() {
        let (_t, r) = store();
        r.add("1", "later", now() + 3600.0).await.unwrap();
        let due = r.pop_due(now()).await;
        assert!(due.is_empty());
        assert_eq!(r.load().await.len(), 1);
    }

    #[tokio::test]
    async fn empty_store_returns_empty() {
        let (_t, r) = store();
        assert!(r.pop_due(now()).await.is_empty());
    }

    #[tokio::test]
    async fn all_due_cleared() {
        let (_t, r) = store();
        let n = now();
        r.add("1", "a", n - 3.0).await.unwrap();
        r.add("2", "b", n - 1.0).await.unwrap();
        let due = r.pop_due(n).await;
        assert_eq!(due.len(), 2);
        assert!(r.load().await.is_empty());
    }
}
