//! Shared fixtures for the `tests_run` tests.

use super::*;
use crate::llm::ChatCompletion;
use crate::tools::sandbox::LazySandbox;
use housebot_sandbox::SandboxClient;
use std::sync::atomic::{AtomicBool, Ordering};
use tempfile::TempDir;
use tokio::sync::Notify;

pub(crate) fn test_agent(client: Arc<dyn ChatClient>) -> (TempDir, Agent) {
    let tmp = TempDir::new().unwrap();
    let agent = Agent::for_test(
        client,
        History::new(tmp.path().join("history"), 30),
        Memory::new(tmp.path().join("memories")),
        ProfileStore::new(tmp.path().join("profiles")),
        Skills::new(tmp.path().join("skills.json")),
        Reminders::new(tmp.path().join("reminders.json")),
    );
    (tmp, agent)
}

pub(crate) fn noop_sandbox() -> LazySandbox {
    LazySandbox::new(SandboxClient::new("/dev/null"))
}

#[derive(Default)]
pub(crate) struct StreamLifecycleHooks {
    pub(crate) events: std::sync::Mutex<Vec<&'static str>>,
}

pub(crate) struct BlockingChatClient {
    pub(crate) started: Arc<Notify>,
    pub(crate) stream_dropped: Arc<AtomicBool>,
}

pub(crate) struct StreamDropGuard(Arc<AtomicBool>);

impl Drop for StreamDropGuard {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

#[async_trait]
impl ChatClient for BlockingChatClient {
    async fn context_window_tokens(&self) -> anyhow::Result<Option<u64>> {
        Ok(Some(10_000))
    }

    async fn chat_stream(
        &self,
        _model: &str,
        _messages: &[Value],
        _tools: &[Value],
        _tool_choice: Option<Value>,
        _thinking: ThinkingMode,
        _max_completion_tokens: Option<u32>,
        _sink: Option<&dyn TextSink>,
    ) -> anyhow::Result<ChatCompletion> {
        let _guard = StreamDropGuard(Arc::clone(&self.stream_dropped));
        self.started.notify_one();
        std::future::pending().await
    }

    async fn chat_once(
        &self,
        _model: &str,
        _messages: &[Value],
        _max_tokens: u32,
    ) -> anyhow::Result<ChatCompletion> {
        unreachable!("cancellation test only exercises streaming")
    }
}

#[async_trait]
impl AgentHooks for StreamLifecycleHooks {
    async fn on_text_stream(&self, _partial: &str) {
        self.events.lock().unwrap().push("text");
    }

    async fn on_text_stream_end(&self) {
        self.events.lock().unwrap().push("end");
    }
}

pub(crate) fn fixture_skill(name: &str, author: &str) -> Skill {
    Skill {
        name: name.to_string(),
        description: Some("desc".to_string()),
        instructions: "original instructions".to_string(),
        triggers: Vec::new(),
        enabled_tools: Vec::new(),
        examples: Vec::new(),
        version: 1,
        version_history: Vec::new(),
        created_by: Some(author.to_string()),
        editors: Vec::new(),
        created_at: 0,
        updated_at: 0,
        prompt: None,
    }
}
