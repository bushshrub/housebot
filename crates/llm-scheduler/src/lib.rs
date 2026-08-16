//! Priority-aware bounded scheduling for LLM requests.

use std::collections::VecDeque;
use std::future::Future;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde_json::Value;
use tokio::sync::oneshot;

use housebot_llm::{ChatClient, ChatCompletion, TextSink, ThinkingMode};

/// Scheduling class of an LLM request, highest priority first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Priority {
    /// A human is waiting on a Discord message.
    UserChat,
    /// Spawned by an agent turn.
    SubAgent,
    /// Reminders and maintenance work.
    Background,
}

impl Priority {
    const ORDER: [Priority; 3] = [Priority::UserChat, Priority::SubAgent, Priority::Background];

    fn index(self) -> usize {
        match self {
            Priority::UserChat => 0,
            Priority::SubAgent => 1,
            Priority::Background => 2,
        }
    }
}

/// Snapshot of the scheduler's current utilization.
#[derive(Debug, Clone, Copy)]
pub struct SchedulerInfo {
    /// How many requests are executing right now.
    pub active: usize,
    /// How many requests are waiting for a slot.
    pub pending: usize,
    /// Total concurrent request ceiling.
    pub max_inflight: usize,
    /// How many sub-agent requests are executing right now.
    pub subagent_active: usize,
    /// Sub-agent concurrency ceiling.
    pub max_subagent: usize,
}

impl SchedulerInfo {
    /// `true` when every slot is occupied and new arrivals must wait.
    pub fn is_saturated(&self) -> bool {
        self.active >= self.max_inflight
    }
}

pub const DEFAULT_MAX_INFLIGHT: usize = 4;
pub const DEFAULT_MAX_SUBAGENT: usize = 2;

struct State {
    max_inflight: usize,
    max_subagent: usize,
    inflight: usize,
    subagent_inflight: usize,
    waiters: [VecDeque<oneshot::Sender<Permit>>; 3],
}

impl State {
    fn can_admit(&self, priority: Priority) -> bool {
        self.inflight < self.max_inflight
            && (priority != Priority::SubAgent || self.subagent_inflight < self.max_subagent)
    }

    fn admit(&mut self, priority: Priority) {
        self.inflight += 1;
        if priority == Priority::SubAgent {
            self.subagent_inflight += 1;
        }
    }

    fn withdraw(&mut self, priority: Priority) {
        self.inflight -= 1;
        if priority == Priority::SubAgent {
            self.subagent_inflight -= 1;
        }
    }

    fn pending(&self) -> usize {
        self.waiters
            .iter()
            .map(|queue| queue.iter().filter(|tx| !tx.is_closed()).count())
            .sum()
    }
}

/// Shared scheduler owning both the total in-flight limit and the sub-agent limit.
///
/// A queued `UserChat` request always takes the next free slot ahead of a queued
/// `SubAgent`, and sub-agents are additionally capped by `max_subagent` so a
/// fan-out cannot consume the whole LLM budget.
pub struct LlmScheduler {
    state: Mutex<State>,
}

impl Default for LlmScheduler {
    fn default() -> Self {
        Self::new(DEFAULT_MAX_INFLIGHT, DEFAULT_MAX_SUBAGENT)
    }
}

impl LlmScheduler {
    pub fn new(max_inflight: usize, max_subagent: usize) -> Self {
        assert!(max_inflight > 0, "LLM in-flight limit must be positive");
        assert!(max_subagent > 0, "sub-agent limit must be positive");
        Self {
            state: Mutex::new(State {
                max_inflight,
                max_subagent,
                inflight: 0,
                subagent_inflight: 0,
                waiters: [VecDeque::new(), VecDeque::new(), VecDeque::new()],
            }),
        }
    }

    /// Wait for a slot at `priority`. Dropping the returned future cancels the
    /// wait and leaves no slot reserved.
    pub async fn acquire(self: &Arc<Self>, priority: Priority) -> Permit {
        let rx = {
            let mut state = self.lock();
            if state.can_admit(priority) {
                state.admit(priority);
                return Permit {
                    scheduler: Some(Arc::clone(self)),
                    priority,
                };
            }
            let (tx, rx) = oneshot::channel();
            state.waiters[priority.index()].push_back(tx);
            rx
        };
        rx.await.expect("scheduler never drops a queued waiter")
    }

    /// Run `operation` once a slot at `priority` is available.
    pub async fn execute<T, F, Fut>(self: &Arc<Self>, priority: Priority, operation: F) -> T
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = T>,
    {
        let _permit = self.acquire(priority).await;
        operation().await
    }

    /// Number of requests currently executing.
    pub fn active_count(&self) -> usize {
        self.lock().inflight
    }

    /// Number of requests waiting for a slot.
    pub fn pending_count(&self) -> usize {
        self.lock().pending()
    }

    /// Snapshot of utilization: active, pending, and both ceilings.
    pub fn info(&self) -> SchedulerInfo {
        let state = self.lock();
        SchedulerInfo {
            active: state.inflight,
            pending: state.pending(),
            max_inflight: state.max_inflight,
            subagent_active: state.subagent_inflight,
            max_subagent: state.max_subagent,
        }
    }

    /// Raise or lower the total in-flight ceiling at runtime. Lowering it never
    /// interrupts running requests; the surplus drains as they finish.
    pub fn set_max_inflight(self: &Arc<Self>, max_inflight: usize) {
        assert!(max_inflight > 0, "LLM in-flight limit must be positive");
        let mut state = self.lock();
        state.max_inflight = max_inflight;
        self.pump(&mut state);
    }

    /// Raise or lower the sub-agent ceiling at runtime.
    pub fn set_max_subagent(self: &Arc<Self>, max_subagent: usize) {
        assert!(max_subagent > 0, "sub-agent limit must be positive");
        let mut state = self.lock();
        state.max_subagent = max_subagent;
        self.pump(&mut state);
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().expect("scheduler state lock poisoned")
    }

    fn release(self: &Arc<Self>, priority: Priority) {
        let mut state = self.lock();
        state.withdraw(priority);
        self.pump(&mut state);
    }

    /// Hand free slots to queued waiters, highest priority first. A waiter that
    /// cannot be admitted is skipped rather than blocking the ones behind it,
    /// so a sub-agent stuck on its own cap never stalls background work.
    fn pump(self: &Arc<Self>, state: &mut State) {
        loop {
            let Some(priority) = Priority::ORDER.into_iter().find(|&priority| {
                state.waiters[priority.index()].retain(|tx| !tx.is_closed());
                !state.waiters[priority.index()].is_empty() && state.can_admit(priority)
            }) else {
                return;
            };
            let tx = state.waiters[priority.index()]
                .pop_front()
                .expect("queue was non-empty");
            state.admit(priority);
            let permit = Permit {
                scheduler: Some(Arc::clone(self)),
                priority,
            };
            if let Err(permit) = tx.send(permit) {
                // The waiter was cancelled between the liveness check and the
                // send; reclaim the slot without running Drop under the lock.
                permit.forget();
                state.withdraw(priority);
            }
        }
    }
}

/// Proof that a slot is held. Releasing happens on drop.
pub struct Permit {
    scheduler: Option<Arc<LlmScheduler>>,
    priority: Priority,
}

impl Permit {
    /// Priority this slot was granted at.
    pub fn priority(&self) -> Priority {
        self.priority
    }

    fn forget(mut self) {
        self.scheduler = None;
    }
}

impl Drop for Permit {
    fn drop(&mut self) {
        if let Some(scheduler) = self.scheduler.take() {
            scheduler.release(self.priority);
        }
    }
}

/// Chat client facade routing every chat operation through the scheduler at a
/// fixed priority.
pub struct ScheduledChatClient {
    inner: Arc<dyn ChatClient>,
    scheduler: Arc<LlmScheduler>,
    priority: Priority,
}

impl ScheduledChatClient {
    pub fn new(inner: Arc<dyn ChatClient>, scheduler: Arc<LlmScheduler>) -> Self {
        Self {
            inner,
            scheduler,
            priority: Priority::UserChat,
        }
    }

    /// The same underlying client and scheduler, scheduled at `priority`.
    pub fn with_priority(&self, priority: Priority) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
            scheduler: Arc::clone(&self.scheduler),
            priority,
        }
    }

    /// Current scheduler utilization snapshot.
    pub fn scheduler_info(&self) -> SchedulerInfo {
        self.scheduler.info()
    }

    /// The shared scheduler, for callers that need to acquire slots directly.
    pub fn scheduler(&self) -> &Arc<LlmScheduler> {
        &self.scheduler
    }
}

#[async_trait]
impl ChatClient for ScheduledChatClient {
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
        self.scheduler
            .execute(self.priority, move || async move {
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
        self.scheduler
            .execute(self.priority, move || async move {
                inner.chat_once(&model, &messages, max_tokens).await
            })
            .await
    }
}

#[cfg(test)]
mod tests;
