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
pub mod remind;
pub mod searxng;
pub mod summarize_url;
pub mod translate;
pub mod web_fetch;

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
