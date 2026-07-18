//! Agent tools: each exposes a JSON `definition()` (name/description/input_schema) and
//! an async implementation invoked by the agent's dispatch loop.

use std::time::{Duration, Instant};

use tokio::sync::Mutex;

pub mod common_crawl;
pub mod edit_feature_request;
pub mod feature_development;
pub mod feature_request;
pub mod features;
pub mod file_download;
pub mod github_api;
pub mod remind;
pub mod sandbox;
pub mod searxng;
pub mod summarize_url;
pub mod token_metrics;
pub mod translate;
pub mod web_fetch;

/// Single Source of Truth for all built-in tool names (used by autocomplete and
/// tool-ban validation). Does not include dynamically-discovered MCP tools.
pub fn all_tool_names() -> &'static [&'static str] {
    &[
        "web_search",
        "deep_research",
        "fetch_webpage",
        "download_file",
        "common_crawl__search",
        "run_skill",
        "create_feature_request",
        "edit_feature_request",
        "prepare_feature_development",
        "github_api",
        "set_reminder",
        "summarize_url",
        "get_token_metrics",
        "translate",
        "get_bot_features",
        "search_messages",
        "get_recent_messages",
        "find_discord_users",
        "get_discord_user",
        "run_lua",
        "get_lua_docs",
        "update_memory",
        "search_memory",
        "sandbox_clone_repository",
        "sandbox_list_files",
        "sandbox_search_code",
        "sandbox_read_file",
        "sandbox_run",
    ]
}

/// Block until fewer than `limit` requests happened in the last 60 seconds, then
/// record one. The lock is released while sleeping so other tasks can queue up.
pub(crate) async fn wait_for_slot(requests: &Mutex<Vec<Instant>>, limit: usize) {
    loop {
        let wait = {
            let mut requests = requests.lock().await;
            let now = Instant::now();
            requests.retain(|at| now.duration_since(*at) < Duration::from_secs(60));
            if requests.len() < limit {
                requests.push(now);
                None
            } else {
                Some(Duration::from_secs(60) - now.duration_since(requests[0]))
            }
        };
        match wait {
            Some(wait) => tokio::time::sleep(wait).await,
            None => break,
        }
    }
}
