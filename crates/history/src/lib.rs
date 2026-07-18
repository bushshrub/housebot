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
    max_turns: usize,
}

impl Default for History {
    fn default() -> Self {
        Self::new(
            config::data_dir().join("history"),
            config::env_parse("MAX_HISTORY_TURNS", 30),
        )
    }
}

impl History {
    /// Create a store rooted at `dir`, keeping the most recent `max_turns` message pairs.
    pub fn new(dir: impl Into<PathBuf>, max_turns: usize) -> Self {
        Self {
            dir: dir.into(),
            max_turns,
        }
    }

    fn path(&self, user_id: impl std::fmt::Display) -> PathBuf {
        self.dir.join(format!("{user_id}.jsonl"))
    }

    fn cutoff(&self) -> usize {
        self.max_turns * 2
    }

    /// Load a user's history, trimmed to the last `max_turns * 2` messages.
    pub async fn load(&self, user_id: impl std::fmt::Display) -> Vec<Value> {
        let path = self.path(user_id);
        let raw = match tokio::fs::read_to_string(&path).await {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        let mut msgs: Vec<Value> = raw
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect();
        trim(&mut msgs, self.cutoff());
        msgs
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

    /// Append a completed turn (user message + assistant/tool messages), trim, and save.
    pub async fn append_turn(
        &self,
        user_id: impl std::fmt::Display + Copy,
        user_message: Value,
        assistant_messages: Vec<Value>,
    ) -> std::io::Result<Vec<Value>> {
        let mut history = self.load(user_id).await;
        history.push(user_message);
        history.extend(assistant_messages);
        trim(&mut history, self.cutoff());
        self.save(user_id, &history).await?;
        Ok(history)
    }
}

fn trim(msgs: &mut Vec<Value>, cutoff: usize) {
    if msgs.len() > cutoff {
        let start = msgs.len() - cutoff;
        msgs.drain(0..start);
        // The cut can land mid-turn, leaving an assistant or tool message
        // first. A history that does not start with a user message is
        // rejected by strict OpenAI-compatible servers (a tool message
        // requires its preceding assistant tool call), so drop the partial
        // turn as well.
        let keep_from = msgs
            .iter()
            .position(|m| m.get("role").and_then(Value::as_str) == Some("user"))
            .unwrap_or(msgs.len());
        msgs.drain(0..keep_from);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::TempDir;

    fn store(max_turns: usize) -> (TempDir, History) {
        let tmp = TempDir::new().unwrap();
        let h = History::new(tmp.path().join("history"), max_turns);
        (tmp, h)
    }

    #[tokio::test]
    async fn load_returns_empty_for_unknown_user() {
        let (_t, h) = store(30);
        assert!(h.load("unknown_user").await.is_empty());
    }

    #[tokio::test]
    async fn save_and_load_roundtrip() {
        let (_t, h) = store(30);
        let msgs = vec![
            json!({"role": "user", "content": "hello"}),
            json!({"role": "assistant", "content": "hi"}),
        ];
        h.save("user1", &msgs).await.unwrap();
        assert_eq!(h.load("user1").await, msgs);
    }

    #[tokio::test]
    async fn load_respects_max_turns() {
        let (_t, h) = store(2);
        let msgs: Vec<Value> = (0..10)
            .map(|i| json!({"role": "user", "content": i.to_string()}))
            .collect();
        h.save("user1", &msgs).await.unwrap();
        let loaded = h.load("user1").await;
        assert_eq!(loaded, msgs[msgs.len() - 4..].to_vec());
    }

    #[tokio::test]
    async fn append_turn_creates_history() {
        let (_t, h) = store(30);
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
        let (_t, h) = store(30);
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
    async fn trimmed_history_always_starts_with_a_user_message() {
        let (_t, h) = store(1);
        // One turn = user + assistant tool call + tool result + assistant.
        h.append_turn(
            "u_trim",
            json!({"role":"user","content":"first"}),
            vec![
                json!({"role":"assistant","content":null,"tool_calls":[{"id":"c1"}]}),
                json!({"role":"tool","tool_call_id":"c1","content":"result"}),
                json!({"role":"assistant","content":"r1"}),
            ],
        )
        .await
        .unwrap();
        let result = h
            .append_turn(
                "u_trim",
                json!({"role":"user","content":"second"}),
                vec![json!({"role":"assistant","content":"r2"})],
            )
            .await
            .unwrap();
        assert_eq!(
            result.first().and_then(|m| m["role"].as_str()),
            Some("user"),
            "history must never start mid-turn: {result:?}"
        );
    }

    #[tokio::test]
    async fn clear_removes_history() {
        let (_t, h) = store(30);
        h.save("uc", &[json!({"role":"user","content":"hello"})])
            .await
            .unwrap();
        assert!(!h.load("uc").await.is_empty());
        h.clear("uc").await.unwrap();
        assert!(h.load("uc").await.is_empty());
    }

    #[tokio::test]
    async fn clear_noop_for_unknown_user() {
        let (_t, h) = store(30);
        h.clear("never_existed").await.unwrap();
    }

    #[tokio::test]
    async fn append_turn_trims_to_max_turns() {
        let (_t, h) = store(1);
        h.append_turn(
            "u4",
            json!({"role":"user","content":"first"}),
            vec![json!({"role":"assistant","content":"r1"})],
        )
        .await
        .unwrap();
        let result = h
            .append_turn(
                "u4",
                json!({"role":"user","content":"second"}),
                vec![json!({"role":"assistant","content":"r2"})],
            )
            .await
            .unwrap();
        assert_eq!(result.len(), 2);
        assert_eq!(result[0], json!({"role":"user","content":"second"}));
        assert_eq!(result[1], json!({"role":"assistant","content":"r2"}));
    }
}
