//! tests run stream.

use super::tests_run_support::*;
use super::*;
use crate::testing::MockChatClient;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::Notify;

#[tokio::test]
async fn cancellation_drops_the_active_llm_stream() {
    let started = Arc::new(Notify::new());
    let stream_dropped = Arc::new(AtomicBool::new(false));
    let client = Arc::new(BlockingChatClient {
        started: Arc::clone(&started),
        stream_dropped: Arc::clone(&stream_dropped),
    });
    let (_tmp, agent) = test_agent(client);
    let cancel = CancelToken::default();
    let run_cancel = cancel.clone();

    let run = tokio::spawn(async move {
        let mut request = AgentRequest::text("u1", "Alice", "hi");
        request.cancel = Some(run_cancel);
        let result = agent.run(request, &NoHooks).await;
        (result, agent)
    });

    started.notified().await;
    cancel.cancel();
    let (result, agent) = tokio::time::timeout(std::time::Duration::from_secs(1), run)
        .await
        .expect("agent run did not stop promptly")
        .expect("agent task panicked");

    assert!(result.cancelled);
    assert!(result.text.is_empty());
    assert!(
        stream_dropped.load(Ordering::Acquire),
        "the in-flight chat_stream future kept running"
    );
    assert_eq!(agent.llm_queue_info().active, 0);
}

#[tokio::test]
async fn run_returns_plain_text_completion() {
    let client = Arc::new(MockChatClient::new());
    client.push_text("hello there");
    let (_t, agent) = test_agent(client);
    let result = agent
        .run(AgentRequest::text("u1", "Alice", "hi"), &NoHooks)
        .await;
    assert_eq!(result.text, "hello there");
}

#[tokio::test]
async fn run_marks_text_stream_end_after_generation() {
    let client = Arc::new(MockChatClient::new());
    client.push_text("hello there");
    let (_t, agent) = test_agent(client);
    let hooks = StreamLifecycleHooks::default();

    agent
        .run(AgentRequest::text("u1", "Alice", "hi"), &hooks)
        .await;

    assert_eq!(*hooks.events.lock().unwrap(), ["text", "text", "end"]);
}

#[tokio::test]
async fn run_emits_text_stream_event_for_tool_only_completions() {
    let client = Arc::new(MockChatClient::new());
    // Tool-call-only completion (no text delta) — the model responds with
    // only a tool request, no streaming text. The proactive text event at
    // the start of the loop must still fire so the typing indicator appears.
    client.push_tool_call("c1", "get_lua_docs", "{}");
    client.push_text("Here are the docs.");
    let (_t, agent) = test_agent(client);
    let hooks = StreamLifecycleHooks::default();

    agent
        .run(AgentRequest::text("u_tool", "Alice", "list tools"), &hooks)
        .await;

    let events = hooks.events.lock().unwrap().clone();
    // Round 1 (tool call, content=None → sink not called):
    //   proactive "text", then "end"
    // Round 2 (text completion → sink pushes "text"):
    //   proactive "text", sink "text", then "end"
    assert_eq!(events, ["text", "end", "text", "text", "end"]);
}

#[tokio::test]
async fn lua_analysis_allows_safe_tool_call() {
    let client = Arc::new(MockChatClient::new());
    client.push_tool_call(
        "call_1",
        "submit_lua_verdict",
        r#"{"safe":true,"reason":"uses only the documented APIs"}"#,
    );
    let (_t, agent) = test_agent(client);
    let result = agent.analyze_lua_script("return 1").await;
    assert_eq!(
        result,
        LuaAnalysis {
            allowed: true,
            reason: "uses only the documented APIs".into()
        }
    );
}

#[tokio::test]
async fn lua_analysis_blocks_unsafe_tool_call() {
    let client = Arc::new(MockChatClient::new());
    client.push_tool_call(
        "call_1",
        "submit_lua_verdict",
        r#"{"safe":false,"reason":"attempts to access the filesystem"}"#,
    );
    let (_t, agent) = test_agent(client);
    let result = agent
        .analyze_lua_script("return io.open('/etc/passwd')")
        .await;
    assert!(!result.allowed);
    assert!(result.reason.contains("filesystem"));
}

#[tokio::test]
async fn lua_analysis_fails_closed_when_no_tool_call_returned() {
    // Model responds with text only (no tool call) → blocked as invalid verdict.
    let client = Arc::new(MockChatClient::new());
    client.push_text("I think it is safe");
    let (_t, agent) = test_agent(client);
    let result = agent.analyze_lua_script("return 1").await;
    assert!(!result.allowed);
    assert!(result.reason.contains("invalid verdict"));
}

#[tokio::test]
async fn lua_analysis_fails_closed_when_tool_call_args_malformed() {
    let client = Arc::new(MockChatClient::new());
    client.push_tool_call("call_1", "submit_lua_verdict", "not json at all");
    let (_t, agent) = test_agent(client);
    let result = agent.analyze_lua_script("return 1").await;
    assert!(!result.allowed);
    assert!(result.reason.contains("incomplete verdict"));
}

#[tokio::test]
async fn lua_analysis_fails_closed_when_safe_field_missing() {
    let client = Arc::new(MockChatClient::new());
    client.push_tool_call("call_1", "submit_lua_verdict", r#"{"reason":"looks fine"}"#);
    let (_t, agent) = test_agent(client);
    let result = agent.analyze_lua_script("return 1").await;
    assert!(!result.allowed);
    assert!(result.reason.contains("incomplete verdict"));
}

#[tokio::test]
async fn lua_analysis_uses_default_reason_when_reason_empty() {
    let client = Arc::new(MockChatClient::new());
    client.push_tool_call(
        "call_1",
        "submit_lua_verdict",
        r#"{"safe":true,"reason":""}"#,
    );
    let (_t, agent) = test_agent(client);
    let result = agent.analyze_lua_script("return 1").await;
    assert!(result.allowed);
    assert_eq!(result.reason, "script passed review");
}

#[tokio::test]
async fn run_persists_history() {
    let client = Arc::new(MockChatClient::new());
    client.push_text("saved reply");
    let (_t, agent) = test_agent(client);
    agent
        .run(AgentRequest::text("u2", "Bob", "remember this"), &NoHooks)
        .await;
    let hist = agent.history.load("u2").await;
    assert_eq!(hist.len(), 2); // user + assistant
    assert_eq!(hist[0]["content"], "remember this");
}

#[tokio::test]
async fn run_persists_tokens_by_conversation() {
    let client = Arc::new(MockChatClient::new());
    client.push_text_with_usage(
        "first reply",
        TokenUsage {
            prompt_tokens: 40,
            completion_tokens: 10,
            ..Default::default()
        },
    );
    client.push_text_with_usage(
        "second reply",
        TokenUsage {
            prompt_tokens: 20,
            completion_tokens: 5,
            ..Default::default()
        },
    );
    let (_t, agent) = test_agent(client);
    agent
        .run(AgentRequest::text("u_tokens", "Alice", "first"), &NoHooks)
        .await;
    agent.reset_session("u_tokens").await;
    agent
        .run(AgentRequest::text("u_tokens", "Alice", "second"), &NoHooks)
        .await;

    let board = agent.token_monitor.leaderboard(10).await.unwrap();
    assert_eq!(board.users[0].label, "Alice");
    assert_eq!(board.users[0].conversations, 2);
    assert_eq!(board.users[0].total_tokens(), 75);
    assert_eq!(board.conversations.len(), 2);
}

#[tokio::test]
async fn token_leaderboard_accumulates_across_simulated_restart() {
    // After a restart the in-memory active_conversations map is empty.
    // For the in-memory backend get_active_conversation_id returns None,
    // so a new conversation is created. Verify that the leaderboard still
    // sums tokens from BOTH conversations for the same user.
    let client = Arc::new(MockChatClient::new());
    client.push_text_with_usage(
        "pre-restart reply",
        TokenUsage {
            prompt_tokens: 100,
            completion_tokens: 50,
            ..Default::default()
        },
    );
    client.push_text_with_usage(
        "post-restart reply",
        TokenUsage {
            prompt_tokens: 30,
            completion_tokens: 10,
            ..Default::default()
        },
    );
    let (_t, agent) = test_agent(client);
    agent
        .run(AgentRequest::text("u_restart", "Carol", "first"), &NoHooks)
        .await;

    // Simulate a restart: clear the in-memory conversation map but keep the
    // token_monitor data intact.
    agent.active_conversations.lock().await.clear();

    agent
        .run(
            AgentRequest::text("u_restart", "Carol", "after restart"),
            &NoHooks,
        )
        .await;

    let board = agent.token_monitor.leaderboard(10).await.unwrap();
    let carol = board
        .users
        .iter()
        .find(|e| e.label == "Carol")
        .expect("Carol must appear in leaderboard");
    assert_eq!(
        carol.total_tokens(),
        190,
        "tokens must survive simulated restart"
    );
}

#[tokio::test]
async fn run_dispatches_translate_tool_then_answers() {
    let client = Arc::new(MockChatClient::new().with_once_reply("Bonjour"));
    // First completion asks for a translate tool call; second finishes with text.
    client.push_tool_call(
        "call_1",
        "translate",
        r#"{"text":"Hello","target_language":"French"}"#,
    );
    client.push_text("It means Bonjour.");
    let (_t, agent) = test_agent(client);
    let result = agent
        .run(
            AgentRequest::text("u3", "Cy", "translate Hello to French"),
            &NoHooks,
        )
        .await;
    assert_eq!(result.text, "It means Bonjour.");
    // History should contain the assistant tool-call turn and the tool result.
    let hist = agent.history.load("u3").await;
    assert!(hist
        .iter()
        .any(|m| m["role"] == "tool" && m["content"] == "Bonjour"));
}

#[tokio::test]
async fn tool_loop_is_bounded() {
    let client = Arc::new(MockChatClient::new());
    // Script far more tool rounds than the loop allows.
    for i in 0..40 {
        client.push_tool_call(&format!("call_{i}"), "get_lua_docs", "{}");
    }
    let (_t, agent) = test_agent(client);
    let result = agent
        .run(AgentRequest::text("u_loop", "Al", "loop forever"), &NoHooks)
        .await;
    assert!(
        result.text.contains("too many tool calls"),
        "unexpected: {}",
        result.text
    );
    assert!(result.tools_called.len() <= 16);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rate_limited_search_still_answers_every_tool_call_in_the_batch() {
    let client = Arc::new(MockChatClient::new());
    // One completion with two tool calls where the first result reads as a
    // rate limit (run_lua is in the rate-limit tool set). The run must end
    // early AND still record a tool result for the second call.
    client.push_completion(crate::llm::ChatCompletion {
        content: None,
        tool_calls: vec![
            crate::llm::ToolCall {
                id: "call_a".into(),
                name: "run_lua".into(),
                arguments: r#"{"script":"print(\"Error: too many requests\")"}"#.into(),
            },
            crate::llm::ToolCall {
                id: "call_b".into(),
                name: "get_lua_docs".into(),
                arguments: "{}".into(),
            },
        ],
        finish_reason: Some("tool_calls".into()),
        usage: Default::default(),
    });
    let (_t, agent) = test_agent(client);
    let result = agent
        .run(
            AgentRequest::text("u_batch", "Al", "search twice"),
            &NoHooks,
        )
        .await;
    assert!(
        result.text.contains("rate-limited"),
        "unexpected: {}",
        result.text
    );
    let hist = agent.history.load("u_batch").await;
    let assistant_tool_calls: usize = hist
        .iter()
        .filter_map(|m| m.get("tool_calls").and_then(|tc| tc.as_array()))
        .map(Vec::len)
        .sum();
    let tool_results = hist.iter().filter(|m| m["role"] == "tool").count();
    assert_eq!(assistant_tool_calls, 2);
    assert_eq!(tool_results, 2);
}
