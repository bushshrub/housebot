use super::*;

use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::time::{sleep, Duration};

#[tokio::test]
async fn never_exceeds_the_inflight_limit() {
    let scheduler = Arc::new(LlmScheduler::new(4, 4));
    let active = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));
    let mut tasks = Vec::new();
    for _ in 0..16 {
        let scheduler = Arc::clone(&scheduler);
        let active = Arc::clone(&active);
        let peak = Arc::clone(&peak);
        tasks.push(tokio::spawn(async move {
            scheduler
                .execute(Priority::UserChat, move || async move {
                    let now = active.fetch_add(1, Ordering::SeqCst) + 1;
                    peak.fetch_max(now, Ordering::SeqCst);
                    sleep(Duration::from_millis(5)).await;
                    active.fetch_sub(1, Ordering::SeqCst);
                })
                .await;
        }));
    }
    for task in tasks {
        task.await.unwrap();
    }
    assert_eq!(peak.load(Ordering::SeqCst), 4);
    assert_eq!(scheduler.active_count(), 0);
    assert_eq!(scheduler.pending_count(), 0);
}

#[tokio::test]
async fn user_chat_takes_the_next_slot_ahead_of_queued_subagents() {
    let scheduler = Arc::new(LlmScheduler::new(1, 4));
    let occupied = scheduler.acquire(Priority::Background).await;

    let order = Arc::new(Mutex::new(Vec::new()));

    let sub = tokio::spawn({
        let scheduler = Arc::clone(&scheduler);
        let order = Arc::clone(&order);
        async move {
            let _permit = scheduler.acquire(Priority::SubAgent).await;
            order.lock().unwrap().push(Priority::SubAgent);
        }
    });
    while scheduler.pending_count() != 1 {
        tokio::task::yield_now().await;
    }

    let chat = tokio::spawn({
        let scheduler = Arc::clone(&scheduler);
        let order = Arc::clone(&order);
        async move {
            let _permit = scheduler.acquire(Priority::UserChat).await;
            order.lock().unwrap().push(Priority::UserChat);
        }
    });
    while scheduler.pending_count() != 2 {
        tokio::task::yield_now().await;
    }

    drop(occupied);
    chat.await.unwrap();
    sub.await.unwrap();

    assert_eq!(
        *order.lock().unwrap(),
        vec![Priority::UserChat, Priority::SubAgent],
        "the later user-chat request must overtake the queued sub-agent"
    );
}

#[tokio::test]
async fn subagents_cannot_consume_the_whole_budget() {
    let scheduler = Arc::new(LlmScheduler::new(4, 2));

    let a = scheduler.acquire(Priority::SubAgent).await;
    let b = scheduler.acquire(Priority::SubAgent).await;
    assert_eq!(scheduler.info().subagent_active, 2);

    let blocked = tokio::spawn({
        let scheduler = Arc::clone(&scheduler);
        async move { scheduler.acquire(Priority::SubAgent).await }
    });
    while scheduler.pending_count() != 1 {
        tokio::task::yield_now().await;
    }
    assert_eq!(scheduler.active_count(), 2, "the third sub-agent must wait");

    // Two slots remain free and non-sub-agent work must still reach them.
    let chat = scheduler.acquire(Priority::UserChat).await;
    assert_eq!(scheduler.active_count(), 3);

    drop(a);
    let third = blocked.await.unwrap();
    assert_eq!(scheduler.info().subagent_active, 2);

    drop((b, chat, third));
    assert_eq!(scheduler.active_count(), 0);
}

#[tokio::test]
async fn a_blocked_subagent_does_not_stall_background_work() {
    let scheduler = Arc::new(LlmScheduler::new(4, 1));
    let sub = scheduler.acquire(Priority::SubAgent).await;

    let queued_sub = tokio::spawn({
        let scheduler = Arc::clone(&scheduler);
        async move { scheduler.acquire(Priority::SubAgent).await }
    });
    while scheduler.pending_count() != 1 {
        tokio::task::yield_now().await;
    }

    let background = tokio::time::timeout(
        Duration::from_secs(5),
        Arc::clone(&scheduler).acquire(Priority::Background),
    )
    .await
    .expect("background work must not queue behind a capped sub-agent");

    drop((sub, background));
    let _ = queued_sub.await.unwrap();
    assert_eq!(scheduler.active_count(), 0);
}

#[tokio::test]
async fn cancelled_waiters_release_their_place() {
    let scheduler = Arc::new(LlmScheduler::new(1, 4));
    let held = scheduler.acquire(Priority::UserChat).await;

    let waiting = tokio::spawn({
        let scheduler = Arc::clone(&scheduler);
        async move {
            let _permit = scheduler.acquire(Priority::UserChat).await;
        }
    });
    while scheduler.pending_count() != 1 {
        tokio::task::yield_now().await;
    }
    waiting.abort();
    let _ = waiting.await;

    drop(held);
    // The cancelled waiter must not have consumed the freed slot.
    assert_eq!(scheduler.active_count(), 0);
    assert_eq!(scheduler.pending_count(), 0);
    let _permit = tokio::time::timeout(
        Duration::from_secs(5),
        Arc::clone(&scheduler).acquire(Priority::UserChat),
    )
    .await
    .expect("the freed slot must still be grantable");
}

#[tokio::test]
async fn raising_the_inflight_limit_drains_waiters() {
    let scheduler = Arc::new(LlmScheduler::new(1, 4));
    let held = scheduler.acquire(Priority::UserChat).await;

    let waiting = tokio::spawn({
        let scheduler = Arc::clone(&scheduler);
        async move { scheduler.acquire(Priority::UserChat).await }
    });
    while scheduler.pending_count() != 1 {
        tokio::task::yield_now().await;
    }

    scheduler.set_max_inflight(2);
    let admitted = tokio::time::timeout(Duration::from_secs(5), waiting)
        .await
        .expect("raising the limit must admit the waiter")
        .unwrap();

    assert_eq!(scheduler.active_count(), 2);
    drop((held, admitted));
    assert_eq!(scheduler.active_count(), 0);
}

#[tokio::test]
async fn lowering_the_inflight_limit_drains_as_requests_finish() {
    let scheduler = Arc::new(LlmScheduler::new(4, 4));
    let permits: Vec<Permit> = {
        let mut permits = Vec::new();
        for _ in 0..4 {
            permits.push(scheduler.acquire(Priority::UserChat).await);
        }
        permits
    };
    scheduler.set_max_inflight(2);
    assert_eq!(
        scheduler.active_count(),
        4,
        "running requests are never interrupted"
    );

    let waiting = tokio::spawn({
        let scheduler = Arc::clone(&scheduler);
        async move { scheduler.acquire(Priority::UserChat).await }
    });
    while scheduler.pending_count() != 1 {
        tokio::task::yield_now().await;
    }

    drop(permits);
    let admitted = tokio::time::timeout(Duration::from_secs(5), waiting)
        .await
        .expect("the waiter must be admitted once the surplus drains")
        .unwrap();
    assert_eq!(scheduler.active_count(), 1);
    drop(admitted);
}

#[tokio::test]
async fn raising_the_subagent_limit_drains_subagent_waiters() {
    let scheduler = Arc::new(LlmScheduler::new(4, 1));
    let held = scheduler.acquire(Priority::SubAgent).await;

    let waiting = tokio::spawn({
        let scheduler = Arc::clone(&scheduler);
        async move { scheduler.acquire(Priority::SubAgent).await }
    });
    while scheduler.pending_count() != 1 {
        tokio::task::yield_now().await;
    }

    scheduler.set_max_subagent(2);
    let admitted = tokio::time::timeout(Duration::from_secs(5), waiting)
        .await
        .expect("raising the sub-agent limit must admit the waiter")
        .unwrap();
    assert_eq!(scheduler.info().subagent_active, 2);
    drop((held, admitted));
    assert_eq!(scheduler.info().subagent_active, 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn all_requests_complete_under_contention() {
    let scheduler = Arc::new(LlmScheduler::new(2, 1));
    let done = Arc::new(AtomicUsize::new(0));
    let mut tasks = Vec::new();
    for index in 0..96 {
        let scheduler = Arc::clone(&scheduler);
        let done = Arc::clone(&done);
        let priority = match index % 3 {
            0 => Priority::UserChat,
            1 => Priority::SubAgent,
            _ => Priority::Background,
        };
        tasks.push(tokio::spawn(async move {
            scheduler
                .execute(priority, move || async move {
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
    .expect("the scheduler must not strand waiters");
    assert_eq!(done.load(Ordering::SeqCst), 96);
    assert_eq!(scheduler.active_count(), 0);
    assert_eq!(scheduler.pending_count(), 0);
}

#[test]
fn is_saturated_reflects_the_inflight_ceiling() {
    let info = SchedulerInfo {
        active: 3,
        pending: 5,
        max_inflight: 4,
        subagent_active: 1,
        max_subagent: 2,
    };
    assert!(!info.is_saturated());
    let info = SchedulerInfo { active: 4, ..info };
    assert!(info.is_saturated());
}
