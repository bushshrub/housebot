//! Shared fixtures for the `tests` tests.

//! Unit tests for `lua_engine` (split out to keep the module under 600 lines).

use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

#[derive(Default)]
pub(crate) struct FakeHost {
    pub(crate) sent: Mutex<Vec<String>>,
    pub(crate) searches: AtomicUsize,
    pub(crate) last_search: Mutex<Option<String>>,
}

#[async_trait]
impl ScriptHost for FakeHost {
    async fn send_message(&self, content: &str) -> Result<(), String> {
        self.sent.lock().unwrap().push(content.to_string());
        Ok(())
    }

    async fn web_search(&self, query: &str, _max_results: usize) -> String {
        self.searches.fetch_add(1, Ordering::SeqCst);
        *self.last_search.lock().unwrap() = Some(query.to_string());
        format!("results for: {query}")
    }

    async fn jellyfin_search(&self, query: &str) -> String {
        format!("media for: {query}")
    }
}

pub(crate) fn limits() -> LuaLimits {
    LuaLimits {
        timeout: Duration::from_secs(2),
        memory_bytes: 8 * 1024 * 1024,
    }
}

pub(crate) async fn run(script: &str, host: &Arc<FakeHost>, limits: LuaLimits) -> String {
    run_full(script, host, limits).await.text
}

pub(crate) async fn run_full(
    script: &str,
    host: &Arc<FakeHost>,
    limits: LuaLimits,
) -> ScriptOutput {
    run_script(
        script.to_string(),
        Arc::clone(host) as Arc<dyn ScriptHost>,
        limits,
        |s: &str| s.to_string(),
    )
    .await
}

pub(crate) fn is_png(bytes: &[u8]) -> bool {
    bytes.starts_with(b"\x89PNG\r\n\x1a\n")
}

pub(crate) fn roles() -> Vec<(u64, String, u16)> {
    vec![
        (1, "@everyone".to_string(), 0),
        (2, "Member".to_string(), 1),
        (3, "Scripting".to_string(), 5),
        (4, "Moderator".to_string(), 7),
    ]
}
