use super::*;

use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::time::{sleep, Duration};

#[tokio::test]
async fn never_exceeds_the_inflight_limit() {
    let scheduler = Arc::new(LlmScheduler::new(4));
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
async fn user_chat_takes_the_next_slot_ahead_of_queued_background_work() {
    let scheduler = Arc::new(LlmScheduler::new(1));
    let occupied = scheduler.acquire(Priority::UserChat).await;

    let order = Arc::new(Mutex::new(Vec::new()));

    let background = tokio::spawn({
        let scheduler = Arc::clone(&scheduler);
        let order = Arc::clone(&order);
        async move {
            let _permit = scheduler.acquire(Priority::Background).await;
            order.lock().unwrap().push(Priority::Background);
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
    background.await.unwrap();

    assert_eq!(
        *order.lock().unwrap(),
        vec![Priority::UserChat, Priority::Background],
        "the later user-chat request must overtake the queued background work"
    );
}

#[tokio::test]
async fn cancelled_waiters_release_their_place() {
    let scheduler = Arc::new(LlmScheduler::new(1));
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
    let scheduler = Arc::new(LlmScheduler::new(1));
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
    let scheduler = Arc::new(LlmScheduler::new(4));
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn all_requests_complete_under_contention() {
    let scheduler = Arc::new(LlmScheduler::new(2));
    let done = Arc::new(AtomicUsize::new(0));
    let mut tasks = Vec::new();
    for index in 0..96 {
        let scheduler = Arc::clone(&scheduler);
        let done = Arc::clone(&done);
        let priority = if index % 2 == 0 {
            Priority::UserChat
        } else {
            Priority::Background
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
    };
    assert!(!info.is_saturated());
    let info = SchedulerInfo { active: 4, ..info };
    assert!(info.is_saturated());
}
