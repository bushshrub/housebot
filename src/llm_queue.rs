//! Shared bounded, priority-aware scheduling for LLM requests.

use std::future::Future;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde_json::Value;
use tokio::sync::Notify;

use crate::llm::{ChatClient, ChatCompletion, TextSink, ThinkingMode};

/// Normal bot conversations take priority over Lua safety checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum LlmPriority {
    LuaAnalysis,
    Normal,
}

#[derive(Default)]
struct QueueState {
    next_id: u64,
    active: usize,
    pending: Vec<PendingRequest>,
}

struct PendingRequest {
    id: u64,
    priority: LlmPriority,
}

/// Snapshot of the queue's current utilization.
#[derive(Debug, Clone, Copy)]
pub struct LlmQueueInfo {
    /// How many requests are executing right now.
    pub active: usize,
    /// How many requests are waiting for a slot.
    pub pending: usize,
    /// Maximum concurrent requests (the capacity set at construction).
    pub max_parallel: usize,
}

impl LlmQueueInfo {
    /// `true` when every slot is occupied and new arrivals must wait.
    pub fn is_saturated(&self) -> bool {
        self.active >= self.max_parallel
    }
}

/// A shared scheduler allowing at most four LLM requests to execute at once.
///
/// Requests are FIFO within a priority and normal bot requests are selected
/// before queued Lua-analysis requests whenever a slot becomes available.
pub struct LlmRequestQueue {
    max_parallel: usize,
    state: Mutex<QueueState>,
    notify: Notify,
}

impl Default for LlmRequestQueue {
    fn default() -> Self {
        Self::new(4)
    }
}

impl LlmRequestQueue {
    pub fn new(max_parallel: usize) -> Self {
        assert!(max_parallel > 0, "LLM queue capacity must be positive");
        Self {
            max_parallel,
            state: Mutex::new(QueueState::default()),
            notify: Notify::new(),
        }
    }

    /// Run `operation` after this request reaches the front of the priority queue.
    pub async fn execute<T, F, Fut>(self: &Arc<Self>, priority: LlmPriority, operation: F) -> T
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = T>,
    {
        let id = {
            let mut state = self.state.lock().unwrap();
            let id = state.next_id;
            state.next_id += 1;
            state.pending.push(PendingRequest { id, priority });
            id
        };
        let mut ticket = QueueTicket {
            queue: Arc::clone(self),
            id,
            acquired: false,
        };

        loop {
            let notified = self.notify.notified();
            let can_start = {
                let mut state = self.state.lock().unwrap();
                let selected = state
                    .pending
                    .iter()
                    .min_by_key(|request| (std::cmp::Reverse(request.priority), request.id))
                    .map(|request| request.id);
                if state.active < self.max_parallel && selected == Some(id) {
                    state.pending.retain(|request| request.id != id);
                    state.active += 1;
                    true
                } else {
                    false
                }
            };
            if can_start {
                ticket.acquired = true;
                break;
            }
            notified.await;
        }

        let _permit = ActivePermit {
            queue: Arc::clone(self),
        };
        operation().await
    }

    /// Number of requests currently executing.
    pub fn active_count(&self) -> usize {
        self.state.lock().unwrap().active
    }

    /// Number of requests waiting for a slot.
    pub fn pending_count(&self) -> usize {
        self.state.lock().unwrap().pending.len()
    }

    /// Snapshot of queue utilization: active, pending, and capacity.
    pub fn info(&self) -> LlmQueueInfo {
        let state = self.state.lock().unwrap();
        LlmQueueInfo {
            active: state.active,
            pending: state.pending.len(),
            max_parallel: self.max_parallel,
        }
    }

    #[cfg(test)]
    fn active(&self) -> usize {
        self.active_count()
    }
}

struct QueueTicket {
    queue: Arc<LlmRequestQueue>,
    id: u64,
    acquired: bool,
}

impl Drop for QueueTicket {
    fn drop(&mut self) {
        if !self.acquired {
            let mut state = self.queue.state.lock().unwrap();
            state.pending.retain(|request| request.id != self.id);
            self.queue.notify.notify_waiters();
        }
    }
}

struct ActivePermit {
    queue: Arc<LlmRequestQueue>,
}

impl Drop for ActivePermit {
    fn drop(&mut self) {
        let mut state = self.queue.state.lock().unwrap();
        state.active = state.active.saturating_sub(1);
        self.queue.notify.notify_waiters();
    }
}

/// Chat client facade that routes every chat operation through the shared queue.
pub struct QueuedChatClient {
    inner: Arc<dyn ChatClient>,
    queue: Arc<LlmRequestQueue>,
}

impl QueuedChatClient {
    pub fn new(inner: Arc<dyn ChatClient>, queue: Arc<LlmRequestQueue>) -> Self {
        Self { inner, queue }
    }

    /// Current queue utilization snapshot.
    pub fn queue_info(&self) -> LlmQueueInfo {
        self.queue.info()
    }

    pub async fn chat_once_with_priority(
        &self,
        priority: LlmPriority,
        model: &str,
        messages: &[Value],
        max_tokens: u32,
    ) -> anyhow::Result<ChatCompletion> {
        let inner = Arc::clone(&self.inner);
        let model = model.to_string();
        let messages = messages.to_vec();
        self.queue
            .execute(priority, move || async move {
                inner.chat_once(&model, &messages, max_tokens).await
            })
            .await
    }

    /// Stream a completion through the priority queue, using a specific
    /// `tool_choice` value. Pass `Some(json!("required"))` to force a tool
    /// call. Unlike `chat_stream`, this method accepts an explicit priority so
    /// lower-priority tasks (e.g. Lua safety reviews) yield to normal traffic.
    pub async fn chat_stream_with_priority(
        &self,
        priority: LlmPriority,
        model: &str,
        messages: &[Value],
        tools: &[Value],
        tool_choice: Option<Value>,
        thinking: ThinkingMode,
    ) -> anyhow::Result<ChatCompletion> {
        let inner = Arc::clone(&self.inner);
        let model = model.to_string();
        let messages = messages.to_vec();
        let tools = tools.to_vec();
        self.queue
            .execute(priority, move || async move {
                inner
                    .chat_stream(&model, &messages, &tools, tool_choice, thinking, None)
                    .await
            })
            .await
    }
}

#[async_trait]
impl ChatClient for QueuedChatClient {
    async fn context_window_tokens(&self) -> anyhow::Result<Option<u64>> {
        self.inner.context_window_tokens().await
    }

    async fn chat_stream(
        &self,
        model: &str,
        messages: &[Value],
        tools: &[Value],
        tool_choice: Option<Value>,
        thinking: ThinkingMode,
        sink: Option<&dyn TextSink>,
    ) -> anyhow::Result<ChatCompletion> {
        let inner = Arc::clone(&self.inner);
        let model = model.to_string();
        let messages = messages.to_vec();
        let tools = tools.to_vec();
        self.queue
            .execute(LlmPriority::Normal, move || async move {
                inner
                    .chat_stream(&model, &messages, &tools, tool_choice, thinking, sink)
                    .await
            })
            .await
    }

    async fn chat_once(
        &self,
        model: &str,
        messages: &[Value],
        max_tokens: u32,
    ) -> anyhow::Result<ChatCompletion> {
        self.chat_once_with_priority(LlmPriority::Normal, model, messages, max_tokens)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::sync::{Barrier, Notify};
    use tokio::time::{sleep, Duration};

    #[tokio::test]
    async fn never_exceeds_capacity() {
        let queue = Arc::new(LlmRequestQueue::new(4));
        let active = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let mut tasks = Vec::new();
        for _ in 0..8 {
            let queue = Arc::clone(&queue);
            let active_count = Arc::clone(&active);
            let peak_count = Arc::clone(&peak);
            tasks.push(tokio::spawn(async move {
                queue
                    .execute(LlmPriority::Normal, move || async move {
                        let now = active_count.fetch_add(1, Ordering::SeqCst) + 1;
                        peak_count.fetch_max(now, Ordering::SeqCst);
                        sleep(Duration::from_millis(10)).await;
                        active_count.fetch_sub(1, Ordering::SeqCst);
                    })
                    .await;
            }));
        }
        while queue.active() < 4 {
            tokio::task::yield_now().await;
        }
        for task in tasks {
            task.await.unwrap();
        }
        assert_eq!(peak.load(Ordering::SeqCst), 4);
    }

    #[tokio::test]
    async fn normal_requests_jump_ahead_of_lua_analysis() {
        let queue = Arc::new(LlmRequestQueue::new(1));
        let first_started = Arc::new(Barrier::new(2));
        let release = Arc::new(Notify::new());
        let order = Arc::new(Mutex::new(Vec::new()));

        let queue_first = Arc::clone(&queue);
        let first_started_first = Arc::clone(&first_started);
        let release_first = Arc::clone(&release);
        let first = tokio::spawn(async move {
            queue_first
                .execute(LlmPriority::Normal, move || async move {
                    first_started_first.wait().await;
                    release_first.notified().await;
                })
                .await;
        });
        first_started.wait().await;

        let queue_lua = Arc::clone(&queue);
        let order_lua = Arc::clone(&order);
        let lua = tokio::spawn(async move {
            queue_lua
                .execute(LlmPriority::LuaAnalysis, move || async move {
                    order_lua.lock().unwrap().push("lua");
                })
                .await;
        });
        tokio::task::yield_now().await;

        let queue_normal = Arc::clone(&queue);
        let order_normal = Arc::clone(&order);
        let normal = tokio::spawn(async move {
            queue_normal
                .execute(LlmPriority::Normal, move || async move {
                    order_normal.lock().unwrap().push("normal");
                })
                .await;
        });
        tokio::task::yield_now().await;
        release.notify_one();

        first.await.unwrap();
        normal.await.unwrap();
        lua.await.unwrap();
        assert_eq!(*order.lock().unwrap(), vec!["normal", "lua"]);
    }

    #[tokio::test]
    async fn reports_active_and_pending_counts() {
        let queue = Arc::new(LlmRequestQueue::new(2));
        assert_eq!(queue.active_count(), 0);
        assert_eq!(queue.pending_count(), 0);
        assert!(!queue.info().is_saturated());

        let started = Arc::new(Barrier::new(3));
        let hold = Arc::new(Notify::new());

        let q1 = Arc::clone(&queue);
        let s1 = Arc::clone(&started);
        let h1 = Arc::clone(&hold);
        let t1 = tokio::spawn(async move {
            q1.execute(LlmPriority::Normal, move || async move {
                s1.wait().await;
                h1.notified().await;
            })
            .await;
        });

        let q2 = Arc::clone(&queue);
        let s2 = Arc::clone(&started);
        let h2 = Arc::clone(&hold);
        let t2 = tokio::spawn(async move {
            q2.execute(LlmPriority::Normal, move || async move {
                s2.wait().await;
                h2.notified().await;
            })
            .await;
        });

        started.wait().await;
        // Both slots are now active — the queue is saturated.
        assert_eq!(queue.active_count(), 2);
        assert_eq!(queue.pending_count(), 0);
        assert!(queue.info().is_saturated());

        // A third request must wait.
        let q3 = Arc::clone(&queue);
        let t3 = tokio::spawn(async move {
            q3.execute(LlmPriority::Normal, move || async {}).await;
        });
        tokio::task::yield_now().await;
        assert_eq!(queue.active_count(), 2);
        assert_eq!(queue.pending_count(), 1);

        // Let one active slot finish — the pending request should drain.
        hold.notify_one();
        t3.await.unwrap();
        assert_eq!(queue.active_count(), 1);

        // Let the remaining active slot finish.
        hold.notify_one();
        t1.await.unwrap();
        t2.await.unwrap();
        assert_eq!(queue.active_count(), 0);
        assert_eq!(queue.pending_count(), 0);
    }

    #[test]
    fn is_saturated_reflects_capacity() {
        let info = LlmQueueInfo {
            active: 3,
            pending: 5,
            max_parallel: 4,
        };
        assert!(!info.is_saturated());
        let info = LlmQueueInfo {
            active: 4,
            pending: 5,
            max_parallel: 4,
        };
        assert!(info.is_saturated());
        let info = LlmQueueInfo {
            active: 5,
            pending: 0,
            max_parallel: 4,
        };
        assert!(info.is_saturated());
    }
}
