//! Per-user persistent memory. Production uses PostgreSQL; tests may use markdown files.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use housebot_config as config;
use serde_json::{json, Value};
use tokio_postgres::NoTls;

const DEFAULT_DATABASE_URL: &str = "postgres://housebot:housebot@postgres/housebot";

#[derive(Clone)]
enum Backend {
    Files(PathBuf),
    Postgres(Arc<tokio_postgres::Client>),
}

/// Handle to the per-user memory store.
#[derive(Clone)]
pub struct Memory {
    backend: Backend,
}

impl Default for Memory {
    fn default() -> Self {
        Self::new(config::data_dir().join("memories"))
    }
}

impl Memory {
    /// Create a store rooted at `dir` (created on first write).
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self {
            backend: Backend::Files(dir.into()),
        }
    }

    /// Connect to the deployment's PostgreSQL memory store and create its schema.
    pub async fn from_env() -> anyhow::Result<Self> {
        let url = config::env_or("DATABASE_URL", DEFAULT_DATABASE_URL);
        let (client, connection) = tokio_postgres::connect(&url, NoTls).await?;
        tokio::spawn(async move {
            if let Err(error) = connection.await {
                tracing::error!(%error, "PostgreSQL memory connection closed");
            }
        });
        migrate_markdown_memories(&client, &config::data_dir().join("memories")).await?;
        Ok(Self {
            backend: Backend::Postgres(Arc::new(client)),
        })
    }

    fn path(dir: &Path, user_id: impl std::fmt::Display) -> PathBuf {
        dir.join(format!("{user_id}.md"))
    }

    /// Load a user's memory, returning an empty string when none exists.
    pub async fn load(&self, user_id: impl std::fmt::Display) -> String {
        let user_id = user_id.to_string();
        match &self.backend {
            Backend::Files(dir) => tokio::fs::read_to_string(Self::path(dir, user_id))
                .await
                .unwrap_or_default(),
            Backend::Postgres(client) => match client
                .query_opt(
                    "SELECT content FROM user_memories WHERE user_id = $1",
                    &[&user_id],
                )
                .await
            {
                Ok(Some(row)) => row.get(0),
                Ok(None) => String::new(),
                Err(error) => {
                    tracing::error!(%error, %user_id, "failed to load user memory");
                    String::new()
                }
            },
        }
    }

    /// Delete a user's memory file (no-op when it does not exist).
    pub async fn clear(&self, user_id: impl std::fmt::Display) -> anyhow::Result<()> {
        let user_id = user_id.to_string();
        match &self.backend {
            Backend::Files(dir) => match tokio::fs::remove_file(Self::path(dir, user_id)).await {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(e.into()),
            },
            Backend::Postgres(client) => {
                client
                    .execute("DELETE FROM user_memories WHERE user_id = $1", &[&user_id])
                    .await?;
                Ok(())
            }
        }
    }

    /// Overwrite a user's memory with `content`.
    pub async fn save(&self, user_id: impl std::fmt::Display, content: &str) -> anyhow::Result<()> {
        let user_id = user_id.to_string();
        match &self.backend {
            Backend::Files(dir) => {
                ensure_dir(dir).await?;
                tokio::fs::write(Self::path(dir, user_id), content).await?;
            }
            Backend::Postgres(client) => {
                client
                    .execute(
                        "INSERT INTO user_memories (user_id, content) VALUES ($1, $2) \
                         ON CONFLICT (user_id) DO UPDATE SET content = EXCLUDED.content, updated_at = NOW()",
                        &[&user_id, &content],
                    )
                    .await?;
            }
        }
        Ok(())
    }
}

/// One-time, non-destructive import from the former markdown-file backend.
async fn migrate_markdown_memories(
    client: &tokio_postgres::Client,
    dir: &Path,
) -> anyhow::Result<()> {
    let mut entries = match tokio::fs::read_dir(dir).await {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("md") {
            continue;
        }
        let Some(user_id) = path.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        let content = tokio::fs::read_to_string(&path).await?;
        client
            .execute(
                "INSERT INTO user_memories (user_id, content) VALUES ($1, $2) \
                 ON CONFLICT (user_id) DO NOTHING",
                &[&user_id, &content],
            )
            .await?;
    }
    Ok(())
}

pub fn update_memory_tool() -> Value {
    json!({
        "name": "update_memory",
        "description": "Update your persistent memory about the current user. Write the complete \
            updated memory content each time, not just the new piece.",
        "input_schema": {
            "type": "object",
            "properties": {"memory_content": {"type": "string", "description": "Full updated memory in markdown format."}},
            "required": ["memory_content"]
        }
    })
}

pub fn search_memory_tool() -> Value {
    json!({
        "name": "search_memory",
        "description": "Search the persistent memory for entries matching a keyword or phrase. \
            Use this when the user asks about something specific you might have remembered, \
            or when you want to check whether you already know something about a topic.",
        "input_schema": {
            "type": "object",
            "properties": {"query": {"type": "string", "minLength": 1, "description": "Keyword or phrase to search for in memory."}},
            "required": ["query"]
        }
    })
}

pub async fn ensure_dir(dir: &Path) -> std::io::Result<()> {
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
