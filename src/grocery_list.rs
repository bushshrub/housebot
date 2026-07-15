//! Per-user grocery lists persisted as JSON (`<dir>/<user_id>.json`).

use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::Mutex;

use crate::config;
use crate::memory::ensure_dir;

pub const MAX_GROCERY_ITEMS: usize = 100;
pub const MAX_GROCERY_ITEM_LENGTH: usize = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddItemResult {
    Added,
    Duplicate,
    Full,
}

/// Handle to the persistent per-user grocery-list store.
#[derive(Clone)]
pub struct GroceryLists {
    dir: PathBuf,
    lock: Arc<Mutex<()>>,
}

impl Default for GroceryLists {
    fn default() -> Self {
        Self::new(config::data_dir().join("grocery_lists"))
    }
}

impl GroceryLists {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: dir.into(),
            lock: Arc::new(Mutex::new(())),
        }
    }

    fn path(&self, user_id: u64) -> PathBuf {
        self.dir.join(format!("{user_id}.json"))
    }

    async fn load_unlocked(&self, user_id: u64) -> Vec<String> {
        let raw = match tokio::fs::read_to_string(self.path(user_id)).await {
            Ok(raw) => raw,
            Err(_) => return Vec::new(),
        };
        if raw.trim().is_empty() {
            return Vec::new();
        }
        serde_json::from_str(&raw).unwrap_or_default()
    }

    async fn write_unlocked(&self, user_id: u64, items: &[String]) -> std::io::Result<()> {
        ensure_dir(&self.dir).await?;
        let body = serde_json::to_string_pretty(items).unwrap_or_else(|_| "[]".into());
        tokio::fs::write(self.path(user_id), body).await
    }

    async fn clear_unlocked(&self, user_id: u64) -> std::io::Result<()> {
        match tokio::fs::remove_file(self.path(user_id)).await {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        }
    }

    /// Load a user's grocery items in insertion order.
    pub async fn load(&self, user_id: u64) -> Vec<String> {
        let _guard = self.lock.lock().await;
        self.load_unlocked(user_id).await
    }

    /// Add an item unless an equivalent item is already present or the list is full.
    pub async fn add(&self, user_id: u64, item: &str) -> std::io::Result<AddItemResult> {
        let _guard = self.lock.lock().await;
        let mut items = self.load_unlocked(user_id).await;
        if items
            .iter()
            .any(|existing| existing.eq_ignore_ascii_case(item))
        {
            return Ok(AddItemResult::Duplicate);
        }
        if items.len() >= MAX_GROCERY_ITEMS {
            return Ok(AddItemResult::Full);
        }
        items.push(item.to_string());
        self.write_unlocked(user_id, &items).await?;
        Ok(AddItemResult::Added)
    }

    /// Remove the item at a zero-based index, returning it when it existed.
    pub async fn remove_at(&self, user_id: u64, index: usize) -> std::io::Result<Option<String>> {
        let _guard = self.lock.lock().await;
        let mut items = self.load_unlocked(user_id).await;
        if index >= items.len() {
            return Ok(None);
        }
        let removed = items.remove(index);
        if items.is_empty() {
            self.clear_unlocked(user_id).await?;
        } else {
            self.write_unlocked(user_id, &items).await?;
        }
        Ok(Some(removed))
    }

    /// Remove the first item matching `name` case-insensitively.
    pub async fn remove_named(&self, user_id: u64, name: &str) -> std::io::Result<Option<String>> {
        let _guard = self.lock.lock().await;
        let mut items = self.load_unlocked(user_id).await;
        let Some(index) = items
            .iter()
            .position(|item| item.eq_ignore_ascii_case(name))
        else {
            return Ok(None);
        };
        let removed = items.remove(index);
        if items.is_empty() {
            self.clear_unlocked(user_id).await?;
        } else {
            self.write_unlocked(user_id, &items).await?;
        }
        Ok(Some(removed))
    }

    /// Remove every item and return the previous item count.
    pub async fn flush(&self, user_id: u64) -> std::io::Result<usize> {
        let _guard = self.lock.lock().await;
        let count = self.load_unlocked(user_id).await.len();
        self.clear_unlocked(user_id).await?;
        Ok(count)
    }

    /// Delete a user's list, including for privacy-erasure flows.
    pub async fn clear(&self, user_id: u64) -> std::io::Result<()> {
        let _guard = self.lock.lock().await;
        self.clear_unlocked(user_id).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn store() -> (TempDir, GroceryLists) {
        let tmp = TempDir::new().unwrap();
        let lists = GroceryLists::new(tmp.path().join("grocery_lists"));
        (tmp, lists)
    }

    #[tokio::test]
    async fn list_persists_across_store_instances() {
        let (tmp, lists) = store();
        assert_eq!(lists.add(7, "milk").await.unwrap(), AddItemResult::Added);
        assert_eq!(
            lists.add(7, "free-range eggs").await.unwrap(),
            AddItemResult::Added
        );

        let reloaded = GroceryLists::new(tmp.path().join("grocery_lists"));
        assert_eq!(reloaded.load(7).await, ["milk", "free-range eggs"]);
    }

    #[tokio::test]
    async fn lists_are_isolated_per_user() {
        let (_tmp, lists) = store();
        lists.add(1, "milk").await.unwrap();
        lists.add(2, "bread").await.unwrap();

        assert_eq!(lists.load(1).await, ["milk"]);
        assert_eq!(lists.load(2).await, ["bread"]);
    }

    #[tokio::test]
    async fn duplicate_items_are_rejected_case_insensitively() {
        let (_tmp, lists) = store();
        lists.add(1, "Milk").await.unwrap();

        assert_eq!(
            lists.add(1, "milk").await.unwrap(),
            AddItemResult::Duplicate
        );
        assert_eq!(lists.load(1).await, ["Milk"]);
    }

    #[tokio::test]
    async fn add_rejects_items_after_list_reaches_limit() {
        let (_tmp, lists) = store();
        for index in 0..MAX_GROCERY_ITEMS {
            assert_eq!(
                lists.add(1, &format!("item {index}")).await.unwrap(),
                AddItemResult::Added
            );
        }

        assert_eq!(
            lists.add(1, "one too many").await.unwrap(),
            AddItemResult::Full
        );
        assert_eq!(lists.load(1).await.len(), MAX_GROCERY_ITEMS);
    }

    #[tokio::test]
    async fn remove_supports_position_and_name() {
        let (_tmp, lists) = store();
        lists.add(1, "milk").await.unwrap();
        lists.add(1, "bread").await.unwrap();
        lists.add(1, "eggs").await.unwrap();

        assert_eq!(
            lists.remove_at(1, 1).await.unwrap().as_deref(),
            Some("bread")
        );
        assert_eq!(
            lists.remove_named(1, "EGGS").await.unwrap().as_deref(),
            Some("eggs")
        );
        assert_eq!(lists.load(1).await, ["milk"]);
    }

    #[tokio::test]
    async fn flush_returns_count_and_clears_list() {
        let (_tmp, lists) = store();
        lists.add(1, "milk").await.unwrap();
        lists.add(1, "bread").await.unwrap();

        assert_eq!(lists.flush(1).await.unwrap(), 2);
        assert!(lists.load(1).await.is_empty());
        assert_eq!(lists.flush(1).await.unwrap(), 0);
    }
}
