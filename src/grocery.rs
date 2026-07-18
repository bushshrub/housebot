use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::config;
use crate::memory::ensure_dir;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroceryItem {
    pub name: String,
}

#[derive(Clone)]
pub struct GroceryList {
    dir: PathBuf,
}

impl Default for GroceryList {
    fn default() -> Self {
        Self::new(config::data_dir().join("grocery"))
    }
}

impl GroceryList {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    fn path(&self, user_id: impl std::fmt::Display) -> PathBuf {
        self.dir.join(format!("{user_id}.json"))
    }

    pub async fn load(&self, user_id: impl std::fmt::Display + Copy) -> Vec<GroceryItem> {
        let path = self.path(user_id);
        let raw = match tokio::fs::read_to_string(&path).await {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        if raw.trim().is_empty() {
            return Vec::new();
        }
        serde_json::from_str(&raw).unwrap_or_default()
    }

    async fn save(
        &self,
        user_id: impl std::fmt::Display + Copy,
        items: &[GroceryItem],
    ) -> std::io::Result<()> {
        ensure_dir(&self.dir).await?;
        let body = serde_json::to_string_pretty(items).unwrap_or_else(|_| "[]".into());
        tokio::fs::write(self.path(user_id), body).await
    }

    pub async fn add(
        &self,
        user_id: impl std::fmt::Display + Copy,
        name: &str,
    ) -> std::io::Result<String> {
        let mut items = self.load(user_id).await;
        let trimmed = name.trim().to_string();
        if trimmed.is_empty() {
            return Ok("Item name cannot be empty.".into());
        }
        if items.iter().any(|i| i.name.eq_ignore_ascii_case(&trimmed)) {
            return Ok(format!("`{trimmed}` is already on your grocery list."));
        }
        items.push(GroceryItem {
            name: trimmed.clone(),
        });
        self.save(user_id, &items).await?;
        Ok(format!("✅ Added **{trimmed}** to your grocery list."))
    }

    pub async fn remove(
        &self,
        user_id: impl std::fmt::Display + Copy,
        name: &str,
    ) -> std::io::Result<String> {
        let mut items = self.load(user_id).await;
        let trimmed = name.trim();
        let before = items.len();
        items.retain(|i| !i.name.eq_ignore_ascii_case(trimmed));
        if items.len() == before {
            return Ok(format!("`{trimmed}` is not on your grocery list."));
        }
        self.save(user_id, &items).await?;
        Ok(format!("✅ Removed **{trimmed}** from your grocery list."))
    }

    pub async fn flush(&self, user_id: impl std::fmt::Display + Copy) -> std::io::Result<String> {
        let items = self.load(user_id).await;
        if items.is_empty() {
            return Ok("Your grocery list is already empty.".into());
        }
        let count = items.len();
        self.save(user_id, &[]).await?;
        Ok(format!(
            "✅ Cleared **{count}** item(s) from your grocery list."
        ))
    }

    pub async fn display(&self, user_id: impl std::fmt::Display + Copy) -> String {
        let items = self.load(user_id).await;
        if items.is_empty() {
            return "Your grocery list is empty. Add items with `!grocery add <item>`.".into();
        }
        let mut lines = vec!["**🛒 Grocery List**".to_string()];
        for (i, item) in items.iter().enumerate() {
            lines.push(format!("{}. {}", i + 1, item.name));
        }
        lines.push(format!(
            "\n**{} item(s)** — use `!grocery remove <item>` or `!grocery flush` to manage.",
            items.len()
        ));
        lines.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn store() -> (TempDir, GroceryList) {
        let tmp = TempDir::new().unwrap();
        let g = GroceryList::new(tmp.path().join("grocery"));
        (tmp, g)
    }

    #[tokio::test]
    async fn empty_when_no_file() {
        let (_t, g) = store();
        assert!(g.load(1).await.is_empty());
    }

    #[tokio::test]
    async fn add_and_display() {
        let (_t, g) = store();
        g.add(1, "milk").await.unwrap();
        let items = g.load(1).await;
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].name, "milk");
    }

    #[tokio::test]
    async fn add_duplicate() {
        let (_t, g) = store();
        g.add(1, "milk").await.unwrap();
        let reply = g.add(1, "Milk").await.unwrap();
        assert!(reply.contains("already on your grocery list"));
        assert_eq!(g.load(1).await.len(), 1);
    }

    #[tokio::test]
    async fn remove_existing() {
        let (_t, g) = store();
        g.add(1, "milk").await.unwrap();
        let reply = g.remove(1, "milk").await.unwrap();
        assert!(reply.contains("Removed"));
        assert!(g.load(1).await.is_empty());
    }

    #[tokio::test]
    async fn remove_missing() {
        let (_t, g) = store();
        let reply = g.remove(1, "nonexistent").await.unwrap();
        assert!(reply.contains("not on your grocery list"));
    }

    #[tokio::test]
    async fn flush_clears_all() {
        let (_t, g) = store();
        g.add(1, "a").await.unwrap();
        g.add(1, "b").await.unwrap();
        let reply = g.flush(1).await.unwrap();
        assert!(reply.contains("Cleared"));
        assert!(g.load(1).await.is_empty());
    }

    #[tokio::test]
    async fn flush_empty() {
        let (_t, g) = store();
        let reply = g.flush(1).await.unwrap();
        assert!(reply.contains("already empty"));
    }

    #[test]
    fn grocery_item_serialization() {
        let item = GroceryItem {
            name: "milk".into(),
        };
        let json = serde_json::to_string(&item).unwrap();
        assert_eq!(json, r#"{"name":"milk"}"#);
    }

    #[tokio::test]
    async fn lists_isolated_per_user() {
        let (_t, g) = store();
        g.add(1, "user1_item").await.unwrap();
        g.add(2, "user2_item").await.unwrap();
        let u1 = g.load(1).await;
        let u2 = g.load(2).await;
        assert_eq!(u1.len(), 1);
        assert_eq!(u1[0].name, "user1_item");
        assert_eq!(u2.len(), 1);
        assert_eq!(u2[0].name, "user2_item");
    }
}
