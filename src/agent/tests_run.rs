use super::*;
use crate::testing::MockChatClient;
use crate::tools::sandbox::LazySandbox;
use housebot_sandbox::SandboxClient;
use serde_json::json;
use tempfile::TempDir;

fn test_agent(client: Arc<dyn ChatClient>) -> (TempDir, Agent) {
    let tmp = TempDir::new().unwrap();
    let agent = Agent::for_test(
        client,
        History::new(tmp.path().join("history"), 30),
        Memory::new(tmp.path().join("memories")),
        ProfileStore::new(tmp.path().join("profiles")),
        Skills::new(tmp.path().join("skills.json")),
        Reminders::new(tmp.path().join("reminders.json")),
    );
    (tmp, agent)
}

fn noop_sandbox() -> LazySandbox {
    LazySandbox::new(SandboxClient::new("/dev/null"))
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
        ToolOutcome::Attachment { text, .. } => panic!("unexpected attachment: {text}"),
    }
}

#[tokio::test]
async fn dispatch_blocks_tool_banned_by_guild_vote() {
    let client = Arc::new(MockChatClient::new());
    let (temp, mut agent) = test_agent(client);
    agent.tool_permissions = ToolPermissions::new(temp.path().join("tool_permissions.json"), 2);
    let proposal = agent
        .tool_permissions
        .propose(77, 200, "translate", 100)
        .await
        .unwrap();
    agent
        .tool_permissions
        .vote(77, &proposal.id, 101, true)
        .await
        .unwrap();

    let sb = noop_sandbox();
    let outcome = agent
        .dispatch_tool(
            "translate",
            &json!({"text":"hello","target_language":"French"}),
            "200",
            "restricted-user",
            10,
            Some(77),
            &sb,
        )
        .await;
    match outcome {
        ToolOutcome::Text(text) => assert!(text.contains("permission denied")),
        _ => panic!("banned tool should return a text denial"),
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
        ProfileStore::new(tmp.path().join("profiles")),
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
    assert!(names.contains(&"translate"));
    assert!(names.contains(&"update_memory"));
    assert!(names.contains(&"common_crawl__search"));
    assert!(names.contains(&"find_discord_users"));
    assert!(names.contains(&"edit_feature_request"));
    assert!(names.contains(&"download_file"));
    assert!(names.contains(&"deep_research"));
    assert!(names.contains(&"run_lua"));
    assert!(names.contains(&"get_lua_docs"));
}

#[tokio::test]
async fn build_tools_includes_sandbox_tools_for_owner() {
    let client = Arc::new(MockChatClient::new());
    let (_t, agent) = test_agent(client);
    let tools = agent.build_tools(true, true).await;
    let names: Vec<&str> = tools
        .iter()
        .filter_map(|t| t["function"]["name"].as_str())
        .collect();
    assert!(names.contains(&"sandbox_clone_repository"));
    assert!(names.contains(&"sandbox_list_files"));
    assert!(names.contains(&"sandbox_search_code"));
    assert!(names.contains(&"sandbox_read_file"));
    assert!(names.contains(&"sandbox_run"));
    assert!(names.contains(&"translate"));
}

#[test]
fn get_lua_docs_tool_definition_is_valid() {
    let def = get_lua_docs_tool();
    let (name, desc, _params) = flatten_tool(&def);
    assert_eq!(name, "get_lua_docs");
    assert!(!desc.is_empty());
}

#[test]
fn run_lua_tool_definition_requires_script() {
    let def = run_lua_tool();
    let (name, _desc, params) = flatten_tool(&def);
    assert_eq!(name, "run_lua");
    let required = params["required"].as_array().unwrap();
    assert!(required.iter().any(|v| v.as_str() == Some("script")));
}

#[test]
fn lua_docs_constant_covers_key_apis() {
    assert!(LUA_DOCS.contains("discord.web_search"));
    assert!(LUA_DOCS.contains("discord.jellyfin_search"));
    assert!(LUA_DOCS.contains("print("));
    assert!(LUA_DOCS.contains("math"));
    assert!(LUA_DOCS.contains("table"));
    assert!(LUA_DOCS.contains("string"));
}

#[tokio::test]
async fn dispatch_get_lua_docs_returns_docs() {
    let client = Arc::new(MockChatClient::new());
    let (_t, agent) = test_agent(client);
    let sb = noop_sandbox();
    let out = agent
        .dispatch_tool("get_lua_docs", &json!({}), "u", "testuser", 0, None, &sb)
        .await;
    let ToolOutcome::Text(t) = out else {
        panic!("expected Text outcome")
    };
    assert!(t.contains("discord.web_search"));
    assert!(t.contains("math"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dispatch_run_lua_executes_script() {
    let client = Arc::new(MockChatClient::new());
    let (_t, agent) = test_agent(client);
    let sb = noop_sandbox();
    let out = agent
        .dispatch_tool(
            "run_lua",
            &json!({"script": "return 6 * 7"}),
            "u",
            "testuser",
            0,
            None,
            &sb,
        )
        .await;
    let ToolOutcome::Text(t) = out else {
        panic!("expected Text outcome")
    };
    assert_eq!(t, "42");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dispatch_run_lua_strips_code_fence() {
    let client = Arc::new(MockChatClient::new());
    let (_t, agent) = test_agent(client);
    let sb = noop_sandbox();
    let out = agent
        .dispatch_tool(
            "run_lua",
            &json!({"script": "```lua\nreturn 1 + 1\n```"}),
            "u",
            "testuser",
            0,
            None,
            &sb,
        )
        .await;
    let ToolOutcome::Text(t) = out else {
        panic!("expected Text outcome")
    };
    assert_eq!(t, "2");
}

/// Regression test for the `BotScriptHost` seam introduced when the Lua engine
/// moved to its own crate: the adapter must satisfy the engine's `ScriptHost`
/// trait and surface a bridge-not-connected error instead of panicking.
#[tokio::test]
async fn bot_script_host_is_a_script_host_and_reports_missing_bridge() {
    let (_tmp, agent) = test_agent(Arc::new(MockChatClient::new()));
    let host: Arc<dyn ScriptHost> = Arc::new(BotScriptHost {
        agent: Arc::new(agent),
        discord: Arc::new(DiscordBridge::default()),
        channel_id: 1,
    });
    let err = host
        .send_message("hi")
        .await
        .expect_err("no Discord HTTP client is connected");
    assert!(err.contains("not available"), "unexpected error: {err}");
}
