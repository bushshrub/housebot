//! Per-user conversation history stored as JSONL (`<dir>/<user_id>.jsonl`).
//!
//! Each line is one raw chat message (system/user/assistant/tool) serialized as JSON,
//! mirroring the message objects sent to the OpenAI-compatible API.

use std::path::PathBuf;

use serde_json::Value;

use housebot_config as config;
use housebot_memory::ensure_dir;

/// Handle to the per-user history store.
#[derive(Clone)]
pub struct History {
    dir: PathBuf,
}

impl Default for History {
    fn default() -> Self {
        Self::new(config::data_dir().join("history"))
    }
}

impl History {
    /// Create a store rooted at `dir`. History is never trimmed: dropping old
    /// messages would change the prompt prefix and miss the prompt cache on
    /// every later turn. Session compaction bounds its size instead.
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    fn path(&self, user_id: impl std::fmt::Display) -> PathBuf {
        self.dir.join(format!("{user_id}.jsonl"))
    }

    /// Load a user's full history.
    pub async fn load(&self, user_id: impl std::fmt::Display) -> Vec<Value> {
        let path = self.path(user_id);
        let raw = match tokio::fs::read_to_string(&path).await {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        raw.lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect()
    }

    /// Rewrite a user's history file with `messages`.
    pub async fn save(
        &self,
        user_id: impl std::fmt::Display,
        messages: &[Value],
    ) -> std::io::Result<()> {
        ensure_dir(&self.dir).await?;
        let mut body = String::new();
        for m in messages {
            body.push_str(&serde_json::to_string(m).unwrap_or_else(|_| "{}".into()));
            body.push('\n');
        }
        tokio::fs::write(self.path(user_id), body).await
    }

    /// Delete a user's history file (no-op when it does not exist).
    pub async fn clear(&self, user_id: impl std::fmt::Display) -> std::io::Result<()> {
        match tokio::fs::remove_file(self.path(user_id)).await {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        }
    }

    /// Append a completed turn (user message + assistant/tool messages) and save.
    pub async fn append_turn(
        &self,
        user_id: impl std::fmt::Display + Copy,
        user_message: Value,
        assistant_messages: Vec<Value>,
    ) -> std::io::Result<Vec<Value>> {
        let mut history = self.load(user_id).await;
        history.push(user_message);
        history.extend(assistant_messages);
        self.save(user_id, &history).await?;
        Ok(history)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::TempDir;

    fn store() -> (TempDir, History) {
        let tmp = TempDir::new().unwrap();
        let h = History::new(tmp.path().join("history"));
        (tmp, h)
    }

    #[tokio::test]
    async fn load_returns_empty_for_unknown_user() {
        let (_t, h) = store();
        assert!(h.load("unknown_user").await.is_empty());
    }

    #[tokio::test]
    async fn save_and_load_roundtrip() {
        let (_t, h) = store();
        let msgs = vec![
            json!({"role": "user", "content": "hello"}),
            json!({"role": "assistant", "content": "hi"}),
        ];
        h.save("user1", &msgs).await.unwrap();
        assert_eq!(h.load("user1").await, msgs);
    }

    #[tokio::test]
    async fn append_turn_creates_history() {
        let (_t, h) = store();
        let user = json!({"role": "user", "content": "hello"});
        let asst = vec![json!({"role": "assistant", "content": "hi"})];
        let result = h
            .append_turn("user2", user.clone(), asst.clone())
            .await
            .unwrap();
        let mut expected = vec![user];
        expected.extend(asst);
        assert_eq!(result, expected);
    }

    #[tokio::test]
    async fn append_turn_accumulates() {
        let (_t, h) = store();
        h.append_turn(
            "u3",
            json!({"role":"user","content":"first"}),
            vec![json!({"role":"assistant","content":"r1"})],
        )
        .await
        .unwrap();
        let result = h
            .append_turn(
                "u3",
                json!({"role":"user","content":"second"}),
                vec![json!({"role":"assistant","content":"r2"})],
            )
            .await
            .unwrap();
        assert_eq!(result[0], json!({"role":"user","content":"first"}));
        assert_eq!(
            result[result.len() - 1],
            json!({"role":"assistant","content":"r2"})
        );
    }

    #[tokio::test]
    async fn clear_removes_history() {
        let (_t, h) = store();
        h.save("uc", &[json!({"role":"user","content":"hello"})])
            .await
            .unwrap();
        assert!(!h.load("uc").await.is_empty());
        h.clear("uc").await.unwrap();
        assert!(h.load("uc").await.is_empty());
    }

    #[tokio::test]
    async fn clear_noop_for_unknown_user() {
        let (_t, h) = store();
        h.clear("never_existed").await.unwrap();
    }

    #[tokio::test]
    async fn append_turn_never_drops_old_messages() {
        let (_t, h) = store();
        for i in 0..100 {
            h.append_turn(
                "u5",
                json!({"role":"user","content":i.to_string()}),
                vec![json!({"role":"assistant","content":"r"})],
            )
            .await
            .unwrap();
        }
        let history = h.load("u5").await;
        assert_eq!(history.len(), 200);
        assert_eq!(history[0], json!({"role":"user","content":"0"}));
    }
}
