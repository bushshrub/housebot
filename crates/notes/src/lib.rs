//! Per-user named notes stored as a JSON object (`<dir>/<user_id>.json`).

use std::collections::BTreeMap;
use std::path::PathBuf;

use housebot_config as config;
use housebot_memory::ensure_dir;

/// Handle to the per-user notes store.
#[derive(Clone)]
pub struct Notes {
    dir: PathBuf,
}

impl Default for Notes {
    fn default() -> Self {
        Self::new(config::data_dir().join("notes"))
    }
}

impl Notes {
    /// Create a store rooted at `dir`.
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    fn path(&self, user_id: impl std::fmt::Display) -> PathBuf {
        self.dir.join(format!("{user_id}.json"))
    }

    /// Load all notes for a user as an ordered name → content map.
    pub async fn load_all(&self, user_id: impl std::fmt::Display) -> BTreeMap<String, String> {
        let path = self.path(user_id);
        let raw = match tokio::fs::read_to_string(&path).await {
            Ok(s) => s,
            Err(_) => return BTreeMap::new(),
        };
        if raw.trim().is_empty() {
            return BTreeMap::new();
        }
        serde_json::from_str(&raw).unwrap_or_default()
    }

    async fn write_all(
        &self,
        user_id: impl std::fmt::Display,
        notes: &BTreeMap<String, String>,
    ) -> std::io::Result<()> {
        ensure_dir(&self.dir).await?;
        let body = serde_json::to_string_pretty(notes).unwrap_or_else(|_| "{}".into());
        tokio::fs::write(self.path(user_id), body).await
    }

    /// Save (or overwrite) a single named note.
    pub async fn save(
        &self,
        user_id: impl std::fmt::Display + Copy,
        name: &str,
        content: &str,
    ) -> std::io::Result<()> {
        let mut notes = self.load_all(user_id).await;
        notes.insert(name.to_string(), content.to_string());
        self.write_all(user_id, &notes).await
    }

    /// Fetch a single note by name.
    pub async fn get(&self, user_id: impl std::fmt::Display, name: &str) -> Option<String> {
        self.load_all(user_id).await.get(name).cloned()
    }

    /// Delete all notes for a user (no-op when no file exists).
    pub async fn clear(&self, user_id: impl std::fmt::Display) -> std::io::Result<()> {
        match tokio::fs::remove_file(self.path(user_id)).await {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        }
    }

    /// Delete a note, returning whether it existed.
    pub async fn delete(
        &self,
        user_id: impl std::fmt::Display + Copy,
        name: &str,
    ) -> std::io::Result<bool> {
        let mut notes = self.load_all(user_id).await;
        if notes.remove(name).is_none() {
            return Ok(false);
        }
        self.write_all(user_id, &notes).await?;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn store() -> (TempDir, Notes) {
        let tmp = TempDir::new().unwrap();
        let n = Notes::new(tmp.path().join("notes"));
        (tmp, n)
    }

    #[tokio::test]
    async fn empty_when_no_file() {
        let (_t, n) = store();
        assert!(n.load_all(99).await.is_empty());
    }

    #[tokio::test]
    async fn save_and_retrieve() {
        let (_t, n) = store();
        n.save(1, "shopping", "milk, eggs").await.unwrap();
        let all = n.load_all(1).await;
        assert_eq!(all.get("shopping").map(String::as_str), Some("milk, eggs"));
    }

    #[tokio::test]
    async fn save_overwrites_existing() {
        let (_t, n) = store();
        n.save(1, "todo", "old content").await.unwrap();
        n.save(1, "todo", "new content").await.unwrap();
        assert_eq!(n.get(1, "todo").await.as_deref(), Some("new content"));
    }

    #[tokio::test]
    async fn multiple_notes_per_user() {
        let (_t, n) = store();
        n.save(1, "a", "alpha").await.unwrap();
        n.save(1, "b", "beta").await.unwrap();
        assert_eq!(n.load_all(1).await.len(), 2);
    }

    #[tokio::test]
    async fn notes_isolated_per_user() {
        let (_t, n) = store();
        n.save(1, "key", "user1").await.unwrap();
        n.save(2, "key", "user2").await.unwrap();
        assert_eq!(n.get(1, "key").await.as_deref(), Some("user1"));
        assert_eq!(n.get(2, "key").await.as_deref(), Some("user2"));
    }

    #[tokio::test]
    async fn get_missing_returns_none() {
        let (_t, n) = store();
        assert!(n.get(1, "missing").await.is_none());
    }

    #[tokio::test]
    async fn delete_existing() {
        let (_t, n) = store();
        n.save(1, "x", "content").await.unwrap();
        assert!(n.delete(1, "x").await.unwrap());
        assert!(n.get(1, "x").await.is_none());
    }

    #[tokio::test]
    async fn delete_missing_returns_false() {
        let (_t, n) = store();
        assert!(!n.delete(1, "nonexistent").await.unwrap());
    }

    #[tokio::test]
    async fn delete_leaves_other_notes() {
        let (_t, n) = store();
        n.save(1, "a", "keep").await.unwrap();
        n.save(1, "b", "remove").await.unwrap();
        n.delete(1, "b").await.unwrap();
        let all = n.load_all(1).await;
        assert!(all.contains_key("a"));
        assert!(!all.contains_key("b"));
    }
}
