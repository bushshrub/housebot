//! tests run persistence.

use super::tests_run_support::*;
use super::*;
use crate::testing::MockChatClient;
use serde_json::json;
use tempfile::TempDir;

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
