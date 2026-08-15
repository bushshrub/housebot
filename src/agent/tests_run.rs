use super::*;
use crate::llm::ChatCompletion;
use crate::testing::MockChatClient;
use crate::tools::sandbox::LazySandbox;
use housebot_sandbox::SandboxClient;
use serde_json::json;
use std::sync::atomic::{AtomicBool, Ordering};
use tempfile::TempDir;
use tokio::sync::Notify;

fn test_agent(client: Arc<dyn ChatClient>) -> (TempDir, Agent) {
    let tmp = TempDir::new().unwrap();
    let agent = Agent::for_test(
        client,
        History::new(tmp.path().join("history"), 30),
        Memory::new(tmp.path().join("memories")),
        Skills::new(tmp.path().join("skills.json")),
        Reminders::new(tmp.path().join("reminders.json")),
    );
    (tmp, agent)
}

fn noop_sandbox() -> LazySandbox {
    LazySandbox::new(SandboxClient::new("/dev/null"), "test-session")
}

#[derive(Default)]
struct StreamLifecycleHooks {
    events: std::sync::Mutex<Vec<&'static str>>,
}

struct BlockingChatClient {
    started: Arc<Notify>,
    stream_dropped: Arc<AtomicBool>,
}

struct StreamDropGuard(Arc<AtomicBool>);

impl Drop for StreamDropGuard {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

#[async_trait]
impl ChatClient for BlockingChatClient {
    async fn context_window_tokens(&self) -> anyhow::Result<Option<u64>> {
        Ok(Some(10_000))
    }

    async fn chat_stream(
        &self,
        _model: &str,
        _messages: &[Value],
        _tools: &[Value],
        _tool_choice: Option<Value>,
        _thinking: ThinkingMode,
        _max_completion_tokens: Option<u32>,
        _sink: Option<&dyn TextSink>,
    ) -> anyhow::Result<ChatCompletion> {
        let _guard = StreamDropGuard(Arc::clone(&self.stream_dropped));
        self.started.notify_one();
        std::future::pending().await
    }

    async fn chat_once(
        &self,
        _model: &str,
        _messages: &[Value],
        _max_tokens: u32,
    ) -> anyhow::Result<ChatCompletion> {
        unreachable!("cancellation test only exercises streaming")
    }
}

#[async_trait]
impl AgentHooks for StreamLifecycleHooks {
    async fn on_text_stream(&self, _partial: &str) {
        self.events.lock().unwrap().push("text");
    }

    async fn on_text_stream_end(&self) {
        self.events.lock().unwrap().push("end");
    }
}

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
    assert_eq!(agent.llm_scheduler_info().active, 0);
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
async fn run_dispatches_a_tool_then_answers() {
    let client = Arc::new(MockChatClient::new());
    // First completion asks for a tool call; second finishes with text.
    client.push_tool_call("call_1", "get_bot_features", "{}");
    client.push_text("Here is what I can do.");
    let (_t, agent) = test_agent(client);
    let result = agent
        .run(AgentRequest::text("u3", "Cy", "what can you do?"), &NoHooks)
        .await;
    assert_eq!(result.text, "Here is what I can do.");
    // History should contain the assistant tool-call turn and the tool result.
    let hist = agent.history.load("u3").await;
    assert!(hist.iter().any(|m| m["role"] == "tool"));
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

#[tokio::test]
async fn empty_completion_cut_off_by_the_token_ceiling_says_so() {
    let client = Arc::new(MockChatClient::new());
    client.push_completion(crate::llm::ChatCompletion {
        content: None,
        tool_calls: vec![],
        finish_reason: Some("length".into()),
        usage: Default::default(),
    });
    let (_t, agent) = test_agent(client);
    let result = agent
        .run(AgentRequest::text("u_len", "Al", "think hard"), &NoHooks)
        .await;
    assert!(
        result.text.contains("ran out of output tokens"),
        "unexpected: {}",
        result.text
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tool_calls_are_dispatched_when_finish_reason_is_not_tool_calls() {
    let client = Arc::new(MockChatClient::new());
    client.push_completion(crate::llm::ChatCompletion {
        content: None,
        tool_calls: vec![crate::llm::ToolCall {
            id: "call_a".into(),
            name: "get_bot_features".into(),
            arguments: "{}".into(),
        }],
        finish_reason: Some("stop".into()),
        usage: Default::default(),
    });
    client.push_text("Here is what the docs say.");
    let (_t, agent) = test_agent(client);
    let result = agent
        .run(AgentRequest::text("u_finish", "Al", "features"), &NoHooks)
        .await;
    assert_eq!(result.text, "Here is what the docs say.");
    assert_eq!(result.tools_called, vec!["get_bot_features".to_string()]);
    let hist = agent.history.load("u_finish").await;
    let assistant_tool_calls: usize = hist
        .iter()
        .filter_map(|m| m.get("tool_calls").and_then(|tc| tc.as_array()))
        .map(Vec::len)
        .sum();
    let tool_results = hist.iter().filter(|m| m["role"] == "tool").count();
    assert_eq!(assistant_tool_calls, 1);
    assert_eq!(tool_results, 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_tool_call_in_a_batch_is_answered() {
    let client = Arc::new(MockChatClient::new());
    // One completion carrying two tool calls: both must be dispatched and
    // recorded, not just the first.
    client.push_completion(crate::llm::ChatCompletion {
        content: None,
        tool_calls: vec![
            crate::llm::ToolCall {
                id: "call_a".into(),
                name: "get_bot_features".into(),
                arguments: "{}".into(),
            },
            crate::llm::ToolCall {
                id: "call_b".into(),
                name: "get_bot_features".into(),
                arguments: "{}".into(),
            },
        ],
        finish_reason: Some("tool_calls".into()),
        usage: Default::default(),
    });
    let (_t, agent) = test_agent(client);
    let result = agent
        .run(
            AgentRequest::text("u_batch", "Al", "list features twice"),
            &NoHooks,
        )
        .await;
    assert!(!result.text.is_empty());
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

#[tokio::test]
async fn run_update_memory_tool_persists() {
    let client = Arc::new(MockChatClient::new());
    client.push_tool_call("c1", "update_memory", r#"{"memory_content":"Likes tea"}"#);
    client.push_text("Noted.");
    let (_t, agent) = test_agent(client);
    agent
        .run(
            AgentRequest::text("u4", "Dee", "remember I like tea"),
            &NoHooks,
        )
        .await;
    assert_eq!(agent.memory.load("u4").await, "Likes tea");
}

fn fixture_skill(name: &str, author: &str) -> Skill {
    Skill {
        name: name.to_string(),
        description: Some("desc".to_string()),
        instructions: "original instructions".to_string(),
        triggers: Vec::new(),
        enabled_tools: Vec::new(),
        examples: Vec::new(),
        version: 1,
        version_history: Vec::new(),
        created_by: Some(author.to_string()),
        editors: Vec::new(),
        created_at: 0,
        updated_at: 0,
        prompt: None,
    }
}

/// End-to-end: the LLM creates a skill, then edits it, purely through tool
/// calls dispatched by the normal agent loop — no direct store access.
#[tokio::test]
async fn run_creates_and_edits_skill_via_conversation() {
    let client = Arc::new(MockChatClient::new());
    client.push_tool_call(
        "call_1",
        "create_skill",
        r#"{"name":"greeter","instructions":"Say hello."}"#,
    );
    client.push_tool_call(
        "call_2",
        "edit_skill",
        r#"{"name":"greeter","instructions":"Say hello warmly."}"#,
    );
    client.push_text("Done — created and refined the greeter skill.");
    let (_t, agent) = test_agent(client);

    let result = agent
        .run(
            AgentRequest::text("555", "Sky", "make me a greeter skill, then warm it up"),
            &NoHooks,
        )
        .await;

    assert_eq!(result.text, "Done — created and refined the greeter skill.");

    let skill = agent.skills.get("greeter").await.expect("skill saved");
    assert_eq!(skill.instructions, "Say hello warmly.");
    assert_eq!(skill.version, 2, "edit_skill must bump the version");
    assert_eq!(skill.version_history.len(), 1);
    assert_eq!(skill.created_by.as_deref(), Some("555"));

    // create_skill auto-enables the new skill for its creator.
    assert!(
        agent
            .user_config
            .load(555)
            .await
            .enabled_skills
            .contains(&"greeter".to_string()),
        "creator should have the skill auto-enabled"
    );

    let hist = agent.history.load("555").await;
    assert!(hist.iter().any(|m| m["role"] == "tool"
        && m["content"]
            .as_str()
            .unwrap_or("")
            .contains("updated to version 2")));
}

/// Regression test for the vulnerability this change fixes: the removed
/// `!skill add` / `/skill add` commands let anyone silently overwrite a
/// skill they didn't own. The replacement `edit_skill` tool must enforce
/// author/editor ownership at the dispatch layer and leave the skill
/// completely untouched when denied.
#[tokio::test]
async fn dispatch_edit_skill_denies_non_owner_and_leaves_skill_unchanged() {
    let client = Arc::new(MockChatClient::new());
    let (_t, agent) = test_agent(client);
    agent
        .skills
        .save(fixture_skill("locked", "owner_1"))
        .await
        .unwrap();
    let sb = noop_sandbox();

    let out = agent
        .dispatch_tool(
            "edit_skill",
            &json!({"name": "locked", "instructions": "hacked instructions"}),
            "intruder_2",
            "Intruder",
            0,
            None,
            &sb,
        )
        .await;
    match out {
        ToolOutcome::Text(t) => assert!(t.contains('⛔'), "unexpected: {t}"),
        other => panic!("unexpected outcome: {other:?}"),
    }

    let unchanged = agent.skills.get("locked").await.unwrap();
    assert_eq!(unchanged.instructions, "original instructions");
    assert_eq!(unchanged.version, 1);
    assert!(unchanged.version_history.is_empty());
}

/// Integration test stitching create → edit → use together through the
/// dispatch layer: the instructions loaded by `use_skill` must reflect the
/// most recent `edit_skill` update, and only the author can edit.
#[tokio::test]
async fn dispatch_edit_skill_then_use_skill_reflects_update() {
    let client = Arc::new(MockChatClient::new());
    let (_t, agent) = test_agent(client);
    agent
        .skills
        .save(fixture_skill("greeter", "owner_1"))
        .await
        .unwrap();
    agent.enable_skill_for_user("owner_1", "greeter").await;
    let sb = noop_sandbox();

    let edit_out = agent
        .dispatch_tool(
            "edit_skill",
            &json!({"name": "greeter", "instructions": "Say hello warmly."}),
            "owner_1",
            "Owner",
            0,
            None,
            &sb,
        )
        .await;
    match edit_out {
        ToolOutcome::Text(t) => assert!(t.contains("updated to version 2"), "unexpected: {t}"),
        other => panic!("unexpected outcome: {other:?}"),
    }

    let use_out = agent
        .dispatch_tool(
            "use_skill",
            &json!({"name": "greeter"}),
            "owner_1",
            "Owner",
            0,
            None,
            &sb,
        )
        .await;
    match use_out {
        ToolOutcome::Text(t) => assert!(
            t.contains("Say hello warmly."),
            "use_skill did not reflect the edit: {t}"
        ),
        other => panic!("unexpected outcome: {other:?}"),
    }
}

#[tokio::test]
async fn dispatch_unknown_tool_returns_error() {
    let client = Arc::new(MockChatClient::new());
    let (_t, agent) = test_agent(client);
    let sb = noop_sandbox();
    let out = agent
        .dispatch_tool(
            "run_unknown_code_agent",
            &json!({}),
            "u",
            "testuser",
            0,
            None,
            &sb,
        )
        .await;
    match out {
        ToolOutcome::Text(t) => assert!(t.contains("Unknown tool")),
        ToolOutcome::DevelopmentAction { text, .. } => {
            panic!("unexpected development action: {text}")
        }
    }
}

#[tokio::test]
async fn context_overflow_triggers_new_session() {
    let client = Arc::new(MockChatClient::new());
    client.push_text_with_usage(
        "ok",
        TokenUsage {
            prompt_tokens: 50,
            completion_tokens: 10,
            ..Default::default()
        },
    );
    client.push_text("ok again");
    let tmp = TempDir::new().unwrap();
    let mut agent = Agent::for_test(
        client,
        History::new(tmp.path().join("history"), 30),
        Memory::new(tmp.path().join("memories")),
        Skills::new(tmp.path().join("skills.json")),
        Reminders::new(tmp.path().join("reminders.json")),
    );
    agent.set_max_context_tokens(50);
    let big = "x".repeat(200);
    agent
        .history
        .save(
            "u5",
            &[
                json!({"role": "user", "content": big.clone()}),
                json!({"role": "assistant", "content": "ok"}),
            ],
        )
        .await
        .unwrap();

    agent
        .run(AgentRequest::text("u5", "Ed", "hi again"), &NoHooks)
        .await;
    agent
        .run(AgentRequest::text("u5", "Ed", "one more"), &NoHooks)
        .await;

    // The oversized message must have been summarized away; only the new turn remains.
    let hist = agent.history.load("u5").await;
    assert!(!hist
        .iter()
        .any(|m| m["content"].as_str() == Some(big.as_str())));
    assert_eq!(hist.last().unwrap()["content"], "ok again");
}

#[tokio::test]
async fn compaction_records_summary_token_usage() {
    let usage = TokenUsage {
        prompt_tokens: 100,
        completion_tokens: 50,
        ..Default::default()
    };
    let client = Arc::new(
        MockChatClient::new()
            .with_once_reply("- Likes tea")
            .with_once_usage(usage),
    );
    let (_t, agent) = test_agent(client);
    agent
        .history
        .save(
            "u6",
            &[
                json!({"role": "user", "content": "I like tea"}),
                json!({"role": "assistant", "content": "Noted"}),
            ],
        )
        .await
        .unwrap();

    agent.compact_session("u6", true).await;

    let info = agent.session_info("u6").await;
    assert_eq!(info.context_tokens, 0);
    assert_eq!(info.requests, 0);
    assert_eq!(info.input_tokens, 0);
    assert_eq!(info.output_tokens, 0);
}

#[tokio::test]
async fn disabled_memory_compaction_clears_history_without_writing_memory() {
    let client = Arc::new(MockChatClient::new().with_once_reply("should not be called"));
    let (_t, agent) = test_agent(client);
    agent.memory.save("u7", "Keep this memory").await.unwrap();
    agent
        .history
        .save(
            "u7",
            &[
                json!({"role": "user", "content": "private conversation"}),
                json!({"role": "assistant", "content": "reply"}),
            ],
        )
        .await
        .unwrap();

    agent.compact_session("u7", false).await;

    assert_eq!(agent.memory.load("u7").await, "Keep this memory");
    assert!(agent.history.load("u7").await.is_empty());
}

/// Regression test for issue #302: compaction must never write persistent
/// memory on its own — memory changes only when the user asks for it via
/// update_memory. The summary carries over in the new session's history.
#[tokio::test]
async fn compaction_never_writes_persistent_memory() {
    let client = Arc::new(MockChatClient::new().with_once_reply("- Likes tea"));
    let (_t, agent) = test_agent(client);
    agent.memory.save("u8", "Existing memory").await.unwrap();
    agent
        .history
        .save(
            "u8",
            &[
                json!({"role": "user", "content": "I like tea"}),
                json!({"role": "assistant", "content": "Noted"}),
            ],
        )
        .await
        .unwrap();

    agent.compact_session("u8", true).await;

    assert_eq!(agent.memory.load("u8").await, "Existing memory");
    let history = agent.history.load("u8").await;
    assert!(
        history.iter().any(|m| m["content"]
            .as_str()
            .is_some_and(|c| c.contains("Likes tea"))),
        "summary should carry over into the new session: {history:?}"
    );
    assert_eq!(history.first().unwrap()["role"], "user");
    assert_eq!(history.last().unwrap()["role"], "assistant");
}

/// Compaction on a user with no stored memory must still leave memory empty.
#[tokio::test]
async fn compaction_leaves_empty_memory_empty() {
    let client = Arc::new(MockChatClient::new().with_once_reply("- Discussed steak recipes"));
    let (_t, agent) = test_agent(client);
    agent
        .history
        .save(
            "u9",
            &[
                json!({"role": "user", "content": "how do I cook steak"}),
                json!({"role": "assistant", "content": "Sear it hot"}),
            ],
        )
        .await
        .unwrap();

    agent.compact_session("u9", true).await;

    assert_eq!(agent.memory.load("u9").await, "");
}

/// An explicit update_memory tool call remains the only path that writes
/// persistent memory, and it must survive a subsequent compaction.
#[tokio::test]
async fn explicit_memory_update_survives_compaction() {
    let client = Arc::new(MockChatClient::new().with_once_reply("- Session summary"));
    let (_t, agent) = test_agent(client);
    let sb = noop_sandbox();

    agent
        .dispatch_tool(
            "update_memory",
            &json!({"memory_content": "Prefers ribeye"}),
            "u10",
            "Ed",
            0,
            None,
            &sb,
        )
        .await;
    agent
        .history
        .save(
            "u10",
            &[
                json!({"role": "user", "content": "hi"}),
                json!({"role": "assistant", "content": "hello"}),
            ],
        )
        .await
        .unwrap();

    agent.compact_session("u10", true).await;

    assert_eq!(agent.memory.load("u10").await, "Prefers ribeye");
}

#[tokio::test]
async fn history_turn_contains_discord_context_metadata() {
    let client = Arc::new(MockChatClient::new().with_once_reply("ok"));
    let (_t, agent) = test_agent(client);
    let mut request = AgentRequest::text("u8", "alice", "hello");
    request.channel_id = 42;
    request.guild_id = Some(7);
    request.display_name = "Alice";
    request.avatar_url = "https://cdn.discordapp.com/avatars/u8/avatar.png";
    agent.run(request, &NoHooks).await;

    let history = agent.history.load("u8").await;
    assert_eq!(history[0]["discord_context"]["guild_id"], 7);
    assert_eq!(history[0]["discord_context"]["channel_id"], 42);
    assert_eq!(history[0]["discord_context"]["username"], "alice");
    assert_eq!(
        history[0]["discord_context"]["avatar_url"],
        "https://cdn.discordapp.com/avatars/u8/avatar.png"
    );
    assert!(history[0]["discord_context"]["timestamp"].is_string());
}

/// Regression test for issue #301: merging a pull request must be refused for
/// anyone outside the administrator list, and the attempt must be audited.
#[tokio::test]
async fn dispatch_github_api_merge_denies_non_administrators() {
    let client = Arc::new(MockChatClient::new());
    let (temp, mut agent) = test_agent(client);
    let audit_path = temp.path().join("pr_merge_audit.jsonl");
    agent.set_merge_audit_path(&audit_path);
    agent.access_control = AccessControlStore::new(temp.path().join("access_control"));
    let sb = noop_sandbox();

    let out = agent
        .dispatch_tool(
            "github_api",
            &json!({"action": "merge_pull_request", "pull_request_number": 42}),
            "999",
            "outsider",
            0,
            None,
            &sb,
        )
        .await;

    match out {
        ToolOutcome::Text(text) => {
            assert!(text.contains("permission denied"), "unexpected: {text}")
        }
        other => panic!("unexpected outcome: {other:?}"),
    }

    let logged = tokio::fs::read_to_string(&audit_path).await.unwrap();
    let entry: Value = serde_json::from_str(logged.trim()).unwrap();
    assert_eq!(entry["admin_id"], "999");
    assert_eq!(entry["admin_username"], "outsider");
    assert_eq!(entry["pull_request"], 42);
    assert_eq!(entry["authorized"], false);
    assert_eq!(entry["result"], "denied");
}

/// Integration test for issue #301: an authorized configurer passes the admin
/// gate, reaches the GitHub layer, and the authorized attempt is audited.
#[tokio::test]
async fn dispatch_github_api_merge_allows_configurers_and_audits_the_attempt() {
    let client = Arc::new(MockChatClient::new());
    let (temp, mut agent) = test_agent(client);
    let audit_path = temp.path().join("pr_merge_audit.jsonl");
    agent.set_merge_audit_path(&audit_path);
    agent.access_control = AccessControlStore::new(temp.path().join("access_control"));
    agent
        .access_control
        .update(|access| {
            access.configurer_ids.insert(7);
        })
        .await
        .unwrap();
    let sb = noop_sandbox();

    let out = agent
        .dispatch_tool(
            "github_api",
            &json!({"action": "merge_pull_request", "pull_request_number": 42}),
            "7",
            "admin_user",
            0,
            None,
            &sb,
        )
        .await;

    // The test reporter has no credentials, so the call stops at the GitHub
    // layer rather than at the permission gate.
    match out {
        ToolOutcome::Text(text) => {
            assert!(!text.contains("permission denied"), "unexpected: {text}");
            assert!(text.contains("not configured"), "unexpected: {text}");
        }
        other => panic!("unexpected outcome: {other:?}"),
    }

    let logged = tokio::fs::read_to_string(&audit_path).await.unwrap();
    let entry: Value = serde_json::from_str(logged.trim()).unwrap();
    assert_eq!(entry["admin_id"], "7");
    assert_eq!(entry["pull_request"], 42);
    assert_eq!(entry["authorized"], true);
    assert_eq!(entry["result"], "error");
}

#[tokio::test]
async fn build_tools_excludes_code_execution() {
    let client = Arc::new(MockChatClient::new());
    let (_t, agent) = test_agent(client);
    let tools = agent.build_tools(true, false).await;
    let names: Vec<&str> = tools
        .iter()
        .filter_map(|t| t["function"]["name"].as_str())
        .collect();
    assert!(!names.contains(&"code_tool"));
    assert!(!names.contains(&"configure_bot"));
    assert!(names.contains(&"update_memory"));
    assert!(names.contains(&"edit_feature_request"));
}

#[tokio::test]
async fn build_tools_includes_sandbox_tools() {
    let client = Arc::new(MockChatClient::new());
    let (_t, agent) = test_agent(client);
    let tools = agent.build_tools(true, false).await;
    let names: Vec<&str> = tools
        .iter()
        .filter_map(|t| t["function"]["name"].as_str())
        .collect();
    assert!(names.contains(&"sandbox_clone_repository"));
    assert!(names.contains(&"sandbox_list_files"));
    assert!(names.contains(&"sandbox_search_code"));
    assert!(names.contains(&"sandbox_read_file"));
    assert!(names.contains(&"sandbox_run"));
}

#[tokio::test]
async fn build_tools_includes_configure_bot_only_for_configurers() {
    let client = Arc::new(MockChatClient::new());
    let (_t, agent) = test_agent(client);
    let tools = agent.build_tools(true, true).await;
    let names: Vec<&str> = tools
        .iter()
        .filter_map(|t| t["function"]["name"].as_str())
        .collect();
    assert!(names.contains(&"configure_bot"));
}
