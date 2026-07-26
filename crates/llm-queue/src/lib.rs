//! Shared bounded scheduling for LLM requests.

use std::future::Future;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;
use tokio::sync::Semaphore;

use housebot_llm::{ChatClient, ChatCompletion, TextSink, ThinkingMode};

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

/// A shared semaphore allowing at most four LLM requests to execute at once.
pub struct LlmRequestQueue {
    max_parallel: usize,
    permits: Arc<Semaphore>,
    active: AtomicUsize,
    pending: AtomicUsize,
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
            permits: Arc::new(Semaphore::new(max_parallel)),
            active: AtomicUsize::new(0),
            pending: AtomicUsize::new(0),
        }
    }

    /// Run `operation` once one of the shared LLM slots is available.
    pub async fn execute<T, F, Fut>(self: &Arc<Self>, operation: F) -> T
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = T>,
    {
        self.pending.fetch_add(1, Ordering::SeqCst);
        let pending = PendingGuard { queue: self };
        let permit = Arc::clone(&self.permits)
            .acquire_owned()
            .await
            .expect("LLM semaphore is never closed");
        drop(pending);
        self.active.fetch_add(1, Ordering::SeqCst);
        let _active = ActiveGuard {
            queue: self,
            _permit: permit,
        };
        operation().await
    }

    /// Number of requests currently executing.
    pub fn active_count(&self) -> usize {
        self.active.load(Ordering::SeqCst)
    }

    /// Number of requests waiting for a slot.
    pub fn pending_count(&self) -> usize {
        self.pending.load(Ordering::SeqCst)
    }

    /// Snapshot of queue utilization: active, pending, and capacity.
    pub fn info(&self) -> LlmQueueInfo {
        LlmQueueInfo {
            active: self.active_count(),
            pending: self.pending_count(),
            max_parallel: self.max_parallel,
        }
    }

    #[cfg(test)]
    fn active(&self) -> usize {
        self.active_count()
    }
}

struct PendingGuard<'a> {
    queue: &'a LlmRequestQueue,
}

impl Drop for PendingGuard<'_> {
    fn drop(&mut self) {
        self.queue.pending.fetch_sub(1, Ordering::SeqCst);
    }
}

struct ActiveGuard<'a> {
    queue: &'a LlmRequestQueue,
    _permit: tokio::sync::OwnedSemaphorePermit,
}

impl Drop for ActiveGuard<'_> {
    fn drop(&mut self) {
        self.queue.active.fetch_sub(1, Ordering::SeqCst);
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
        max_completion_tokens: Option<u32>,
        sink: Option<&dyn TextSink>,
    ) -> anyhow::Result<ChatCompletion> {
        let inner = Arc::clone(&self.inner);
        let model = model.to_string();
        let messages = messages.to_vec();
        let tools = tools.to_vec();
        self.queue
            .execute(move || async move {
                inner
                    .chat_stream(
                        &model,
                        &messages,
                        &tools,
                        tool_choice,
                        thinking,
                        max_completion_tokens,
                        sink,
                    )
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
        let inner = Arc::clone(&self.inner);
        let model = model.to_string();
        let messages = messages.to_vec();
        self.queue
            .execute(move || async move { inner.chat_once(&model, &messages, max_tokens).await })
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
    async fn never_exceeds_four_currently_running_requests() {
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
                    .execute(move || async move {
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
    async fn cancelled_waiter_releases_pending_count() {
        let queue = Arc::new(LlmRequestQueue::new(1));
        let hold = Arc::new(Notify::new());
        let active_queue = Arc::clone(&queue);
        let active_hold = Arc::clone(&hold);
        let active = tokio::spawn(async move {
            active_queue
                .execute(move || async move { active_hold.notified().await })
                .await;
        });
        while queue.active_count() != 1 {
            tokio::task::yield_now().await;
        }

        let waiting_queue = Arc::clone(&queue);
        let waiting = tokio::spawn(async move {
            waiting_queue.execute(|| async {}).await;
        });
        while queue.pending_count() != 1 {
            tokio::task::yield_now().await;
        }
        waiting.abort();
        let _ = waiting.await;
        assert_eq!(queue.pending_count(), 0);

        hold.notify_one();
        active.await.unwrap();
        assert_eq!(queue.active_count(), 0);
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
            q1.execute(move || async move {
                s1.wait().await;
                h1.notified().await;
            })
            .await;
        });

        let q2 = Arc::clone(&queue);
        let s2 = Arc::clone(&started);
        let h2 = Arc::clone(&hold);
        let t2 = tokio::spawn(async move {
            q2.execute(move || async move {
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
            q3.execute(move || async {}).await;
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

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn all_requests_complete_under_contention() {
        let queue = Arc::new(LlmRequestQueue::new(2));
        let done = Arc::new(AtomicUsize::new(0));
        let mut tasks = Vec::new();
        for _ in 0..64 {
            let queue = Arc::clone(&queue);
            let done = Arc::clone(&done);
            tasks.push(tokio::spawn(async move {
                queue
                    .execute(move || async move {
                        tokio::task::yield_now().await;
                        done.fetch_add(1, Ordering::SeqCst);
                    })
                    .await;
            }));
        }
        tokio::time::timeout(Duration::from_secs(30), async {
            for task in tasks {
                task.await.unwrap();
            }
        })
        .await
        .expect("queue must not strand pending requests");
        assert_eq!(done.load(Ordering::SeqCst), 64);
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
