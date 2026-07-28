//! tests run dispatch.

use super::tests_run_support::*;
use super::*;
use crate::testing::MockChatClient;
use serde_json::json;

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
    assert!(names.contains(&"translate"));
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
