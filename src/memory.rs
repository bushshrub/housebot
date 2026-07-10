//! Per-user persistent memory stored as markdown files (`<dir>/<user_id>.md`).

use std::path::{Path, PathBuf};

use crate::config;

/// Handle to the per-user memory store.
#[derive(Clone)]
pub struct Memory {
    dir: PathBuf,
}

impl Default for Memory {
    fn default() -> Self {
        Self::new(config::data_dir().join("memories"))
    }
}

impl Memory {
    /// Create a store rooted at `dir` (created on first write).
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    fn path(&self, user_id: impl std::fmt::Display) -> PathBuf {
        self.dir.join(format!("{user_id}.md"))
    }

    /// Load a user's memory, returning an empty string when none exists.
    pub async fn load(&self, user_id: impl std::fmt::Display) -> String {
        let path = self.path(user_id);
        tokio::fs::read_to_string(&path).await.unwrap_or_default()
    }

    /// Overwrite a user's memory with `content`.
    pub async fn save(
        &self,
        user_id: impl std::fmt::Display,
        content: &str,
    ) -> std::io::Result<()> {
        ensure_dir(&self.dir).await?;
        tokio::fs::write(self.path(user_id), content).await
    }
}

pub(crate) async fn ensure_dir(dir: &Path) -> std::io::Result<()> {
    tokio::fs::create_dir_all(dir).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn store() -> (TempDir, Memory) {
        let tmp = TempDir::new().unwrap();
        let mem = Memory::new(tmp.path().join("memories"));
        (tmp, mem)
    }

    #[tokio::test]
    async fn load_returns_empty_for_unknown_user() {
        let (_t, mem) = store();
        assert_eq!(mem.load("unknown").await, "");
    }

    #[tokio::test]
    async fn save_and_load_roundtrip() {
        let (_t, mem) = store();
        mem.save("user1", "Likes pizza").await.unwrap();
        assert_eq!(mem.load("user1").await, "Likes pizza");
    }

    #[tokio::test]
    async fn save_overwrites_previous() {
        let (_t, mem) = store();
        mem.save("user1", "Likes pizza").await.unwrap();
        mem.save("user1", "Likes sushi now").await.unwrap();
        assert_eq!(mem.load("user1").await, "Likes sushi now");
    }

    #[tokio::test]
    async fn users_are_isolated() {
        let (_t, mem) = store();
        mem.save("user1", "memory A").await.unwrap();
        mem.save("user2", "memory B").await.unwrap();
        assert_eq!(mem.load("user1").await, "memory A");
        assert_eq!(mem.load("user2").await, "memory B");
    }

    #[tokio::test]
    async fn load_and_save_create_missing_parent_dirs() {
        let tmp = TempDir::new().unwrap();
        let deep = tmp.path().join("nonexistent_parent").join("memories");
        let mem = Memory::new(&deep);
        // Must not error even though neither parent nor child exists yet.
        assert_eq!(mem.load("someuser").await, "");
        mem.save("someuser", "hello").await.unwrap();
        assert_eq!(mem.load("someuser").await, "hello");
    }
}
