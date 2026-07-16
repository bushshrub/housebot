//! Unit tests for `token_monitor` (split out to keep the module under 600 lines).

use super::*;
use crate::llm::PromptTokenDetails;
use serde_json::json;

fn usage(input: u64, output: u64, cached: u64) -> TokenUsage {
    TokenUsage {
        prompt_tokens: input,
        completion_tokens: output,
        prompt_tokens_details: PromptTokenDetails {
            cached_tokens: cached,
        },
    }
}

#[tokio::test]
async fn connection_failure_is_returned_instead_of_using_volatile_storage() {
    let result = connect_with_retry(
        "not-a-postgres-url",
        2,
        Duration::ZERO,
        Duration::from_secs(1),
    )
    .await;
    let Err(error) = result else {
        panic!("invalid database URL unexpectedly connected");
    };
    assert!(error.to_string().contains("after 2 attempt(s)"));
}

#[tokio::test]
async fn stalled_connection_attempt_is_bounded_by_timeout() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (_stream, _) = listener.accept().await.unwrap();
        std::future::pending::<()>().await;
    });
    let url = format!("postgres://housebot:housebot@{address}/housebot");

    let result = connect_with_retry(&url, 1, Duration::ZERO, Duration::from_millis(100)).await;
    server.abort();

    let Err(error) = result else {
        panic!("stalled PostgreSQL handshake unexpectedly connected");
    };
    assert!(error.to_string().contains("timed out"));
}

#[tokio::test]
async fn aggregates_users_and_conversations() {
    let monitor = TokenMonitor::default();
    monitor
        .start_conversation("c1", "u1", "Alice", 10)
        .await
        .unwrap();
    monitor
        .record_usage("c1", usage(100, 20, 10))
        .await
        .unwrap();
    monitor
        .start_conversation("c2", "u1", "Alice", 20)
        .await
        .unwrap();
    monitor.record_usage("c2", usage(50, 5, 0)).await.unwrap();
    monitor
        .start_conversation("c3", "u2", "Bob", 10)
        .await
        .unwrap();
    monitor.record_usage("c3", usage(10, 5, 0)).await.unwrap();

    let board = monitor.leaderboard(10).await.unwrap();
    assert_eq!(board.users[0].label, "Alice");
    assert_eq!(board.users[0].conversations, 2);
    assert_eq!(board.users[0].total_tokens(), 175);
    assert_eq!(
        board.conversations[0].conversation_id.as_deref(),
        Some("c1")
    );
}

#[tokio::test]
async fn archives_every_message_and_erases_user_data() {
    let monitor = TokenMonitor::default();
    monitor
        .start_conversation("c1", "u1", "Alice", 10)
        .await
        .unwrap();
    monitor
        .record_turn(
            "c1",
            &json!({"role":"user","content":"hello"}),
            &[json!({"role":"assistant","content":"hi"})],
        )
        .await
        .unwrap();
    if let Backend::Memory(data) = &monitor.backend {
        assert_eq!(data.lock().await.messages.len(), 2);
    }
    monitor.clear_user("u1").await.unwrap();
    assert!(monitor.leaderboard(10).await.unwrap().users.is_empty());
}

#[tokio::test]
async fn timeframe_excludes_old_conversations() {
    let monitor = TokenMonitor::default();
    monitor
        .start_conversation("old", "u1", "Alice", 10)
        .await
        .unwrap();
    monitor
        .record_usage("old", usage(500, 100, 0))
        .await
        .unwrap();
    monitor
        .start_conversation("recent", "u2", "Bob", 10)
        .await
        .unwrap();
    monitor
        .record_usage("recent", usage(50, 10, 0))
        .await
        .unwrap();

    if let Backend::Memory(data) = &monitor.backend {
        data.lock()
            .await
            .usage_events
            .iter_mut()
            .find(|event| event.conversation_id == "old")
            .unwrap()
            .created_at = SystemTime::now() - Duration::from_secs(2 * 24 * 60 * 60);
    }

    let board = monitor
        .leaderboard_for(
            LeaderboardPeriod::Daily,
            LeaderboardMetric::TotalTokens,
            10,
            None,
        )
        .await
        .unwrap();
    assert_eq!(board.users.len(), 1);
    assert_eq!(board.users[0].label, "Bob");
    assert_eq!(board.period, LeaderboardPeriod::Daily);
}

#[tokio::test]
async fn efficiency_metric_and_requester_rank_are_reported() {
    let monitor = TokenMonitor::default();
    for (id, user, name, token_usage) in [
        ("c1", "u1", "Efficient", usage(100, 1, 90)),
        ("c2", "u2", "Heavy", usage(1_000, 1_000, 5)),
        ("c3", "u3", "Requester", usage(100, 1, 10)),
    ] {
        monitor
            .start_conversation(id, user, name, 10)
            .await
            .unwrap();
        monitor.record_usage(id, token_usage).await.unwrap();
    }

    let board = monitor
        .leaderboard_for(
            LeaderboardPeriod::AllTime,
            LeaderboardMetric::CacheEfficiency,
            1,
            Some("u3"),
        )
        .await
        .unwrap();
    assert_eq!(board.users[0].label, "Efficient");
    assert_eq!(board.requester_rank.as_ref().unwrap().position, 2);
    assert_eq!(
        board.requester_rank.as_ref().unwrap().entry.label,
        "Requester"
    );
}

#[tokio::test]
async fn get_active_conversation_id_returns_none_for_memory_backend() {
    // The in-memory backend has no recovery mechanism; None tells callers
    // to start a fresh conversation (which still accumulates correctly in
    // the leaderboard across the session).
    let monitor = TokenMonitor::default();
    monitor
        .start_conversation("conv1", "u1", "Alice", 10)
        .await
        .unwrap();
    assert_eq!(monitor.get_active_conversation_id("u1").await, None);
    assert_eq!(monitor.get_active_conversation_id("unknown").await, None);
}

#[tokio::test]
async fn global_stats_returns_zeros_for_empty_data() {
    let monitor = TokenMonitor::default();
    let stats = monitor
        .get_global_stats(LeaderboardPeriod::AllTime)
        .await
        .unwrap();
    assert_eq!(stats.total_users, 0);
    assert_eq!(stats.total_conversations, 0);
    assert_eq!(stats.total_input_tokens, 0);
    assert_eq!(stats.total_output_tokens, 0);
    assert_eq!(stats.total_cached_tokens, 0);
    assert_eq!(stats.period, LeaderboardPeriod::AllTime);
}

#[tokio::test]
async fn global_stats_returns_zeros_for_empty_daily_period() {
    let monitor = TokenMonitor::default();
    let stats = monitor
        .get_global_stats(LeaderboardPeriod::Daily)
        .await
        .unwrap();
    assert_eq!(stats.total_users, 0);
    assert_eq!(stats.total_conversations, 0);
    assert_eq!(stats.total_input_tokens, 0);
    assert_eq!(stats.total_output_tokens, 0);
    assert_eq!(stats.total_cached_tokens, 0);
    assert_eq!(stats.period, LeaderboardPeriod::Daily);
}

#[tokio::test]
async fn leaderboard_accumulates_across_multiple_conversations() {
    // Verify that even when a new conversation is created (simulating a
    // restart with the in-memory backend), the leaderboard sums tokens
    // from all conversations for the same user.
    let monitor = TokenMonitor::default();
    monitor
        .start_conversation("c1", "u1", "Alice", 10)
        .await
        .unwrap();
    monitor.record_usage("c1", usage(100, 40, 0)).await.unwrap();
    monitor.finish_conversation("c1").await.unwrap();
    // Simulate restart: new conversation created for the same user.
    monitor
        .start_conversation("c2", "u1", "Alice", 10)
        .await
        .unwrap();
    monitor.record_usage("c2", usage(60, 20, 0)).await.unwrap();

    let board = monitor.leaderboard(10).await.unwrap();
    assert_eq!(board.users.len(), 1);
    assert_eq!(board.users[0].label, "Alice");
    assert_eq!(board.users[0].conversations, 2);
    assert_eq!(
        board.users[0].total_tokens(),
        220,
        "tokens must sum across conversations"
    );
}
