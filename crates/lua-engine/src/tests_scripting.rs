//! tests scripting.

//! Unit tests for `lua_engine` (split out to keep the module under 600 lines).

use super::tests_support::*;
use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn returns_expression_value() {
    let host = Arc::new(FakeHost::default());
    assert_eq!(run("return 1 + 2", &host, limits()).await, "3");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn captures_print_output() {
    let host = Arc::new(FakeHost::default());
    let out = run("print(\"hello\", 42) print(\"second\")", &host, limits()).await;
    assert_eq!(out, "hello\t42\nsecond");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn no_output_reports_completion() {
    let host = Arc::new(FakeHost::default());
    let out = run("local x = 1", &host, limits()).await;
    assert_eq!(out, "(script completed with no output)");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sandbox_hides_dangerous_globals() {
    let host = Arc::new(FakeHost::default());
    let out = run(
            "return type(os), type(io), type(require), type(load), type(dofile), type(loadfile), type(debug), type(package)",
            &host,
            limits(),
        )
        .await;
    assert_eq!(out, "nil\tnil\tnil\tnil\tnil\tnil\tnil\tnil");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sandbox_removes_collectgarbage_warn_and_global_table_ref() {
    let host = Arc::new(FakeHost::default());
    let out = run(
        "return type(collectgarbage), type(warn), type(_G)",
        &host,
        limits(),
    )
    .await;
    assert_eq!(out, "nil\tnil\tnil");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn web_search_query_is_truncated_to_limit() {
    let host = Arc::new(FakeHost::default());
    let long_query = "x".repeat(MAX_QUERY_CHARS + 100);
    let out = run(
        &format!("return discord.web_search(\"{long_query}\")"),
        &host,
        limits(),
    )
    .await;
    let searched = host.last_search.lock().unwrap().clone().unwrap_or_default();
    assert_eq!(searched.chars().count(), MAX_QUERY_CHARS);
    assert!(out.contains("results for:"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pattern_matching_functions_are_removed() {
    let host = Arc::new(FakeHost::default());
    let out = run(
        "return type(string.find), type(string.match), type(string.gmatch), type(string.gsub)",
        &host,
        limits(),
    )
    .await;
    assert_eq!(out, "nil\tnil\tnil\tnil");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn string_dump_is_removed() {
    let host = Arc::new(FakeHost::default());
    let out = run("return type(string.dump)", &host, limits()).await;
    assert_eq!(out, "nil");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pattern_method_form_is_removed() {
    let host = Arc::new(FakeHost::default());
    let out = run(
        "return pcall(function() return (\"ab\"):find(\"a\") end)",
        &host,
        limits(),
    )
    .await;
    assert!(out.starts_with("false"), "unexpected output: {out}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn safe_string_functions_still_work() {
    let host = Arc::new(FakeHost::default());
    let out = run(
        "return string.format(\"%s-%d\", string.upper(\"ab\"), 3) .. string.rep(\"!\", 2)",
        &host,
        limits(),
    )
    .await;
    assert_eq!(out, "AB-3!!");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn busy_loop_hits_time_limit() {
    let host = Arc::new(FakeHost::default());
    let short = LuaLimits {
        timeout: Duration::from_millis(200),
        memory_bytes: 8 * 1024 * 1024,
    };
    let out = run("while true do end", &host, short).await;
    assert!(out.contains("time limit"), "unexpected output: {out}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pcall_cannot_swallow_the_time_limit() {
    let host = Arc::new(FakeHost::default());
    let short = LuaLimits {
        timeout: Duration::from_millis(200),
        memory_bytes: 8 * 1024 * 1024,
    };
    let started = Instant::now();
    let out = run(
        "while true do pcall(function() while true do end end) end",
        &host,
        short,
    )
    .await;
    assert!(out.contains("time limit"), "unexpected output: {out}");
    assert!(started.elapsed() < Duration::from_secs(2));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn error_is_visible_after_truncated_output() {
    let host = Arc::new(FakeHost::default());
    let out = run(
        "for i = 1, 100 do print(string.rep(\"a\", 100)) end error(\"boom\")",
        &host,
        limits(),
    )
    .await;
    assert!(out.contains("output truncated"), "unexpected output: {out}");
    assert!(out.contains("boom"), "unexpected output: {out}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn memory_limit_enforced() {
    let host = Arc::new(FakeHost::default());
    let small = LuaLimits {
        timeout: Duration::from_secs(5),
        memory_bytes: 1024 * 1024,
    };
    let out = run("local s = \"x\" while true do s = s .. s end", &host, small).await;
    assert!(out.contains("memory limit"), "unexpected output: {out}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn runtime_errors_are_reported() {
    let host = Arc::new(FakeHost::default());
    let out = run("error(\"boom\")", &host, limits()).await;
    assert!(out.starts_with("Error:"), "unexpected output: {out}");
    assert!(out.contains("boom"), "unexpected output: {out}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn output_before_error_is_kept() {
    let host = Arc::new(FakeHost::default());
    let out = run("print(\"before\") error(\"boom\")", &host, limits()).await;
    assert!(out.starts_with("before\n"), "unexpected output: {out}");
    assert!(out.contains("boom"), "unexpected output: {out}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn output_is_truncated() {
    let host = Arc::new(FakeHost::default());
    let out = run(
        "for i = 1, 100 do print(string.rep(\"a\", 100)) end",
        &host,
        limits(),
    )
    .await;
    assert!(out.contains("output truncated"), "unexpected output: {out}");
    assert!(out.chars().count() <= MAX_OUTPUT_CHARS + OUTPUT_TRUNCATED_MARKER.chars().count());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn web_search_bridge_works() {
    let host = Arc::new(FakeHost::default());
    let out = run("return discord.web_search(\"rust\")", &host, limits()).await;
    assert_eq!(out, "results for: rust");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn jellyfin_bridge_works() {
    let host = Arc::new(FakeHost::default());
    let out = run(
        "return discord.jellyfin_search(\"matrix\")",
        &host,
        limits(),
    )
    .await;
    assert_eq!(out, "media for: matrix");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn send_message_bridge_delivers() {
    let host = Arc::new(FakeHost::default());
    let out = run("discord.send_message(\"hi there\")", &host, limits()).await;
    assert_eq!(out, "(script completed with no output)");
    assert_eq!(*host.sent.lock().unwrap(), vec!["hi there".to_string()]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn send_message_cap_enforced() {
    let host = Arc::new(FakeHost::default());
    let out = run(
        "for i = 1, 10 do discord.send_message(\"spam\" .. i) end",
        &host,
        limits(),
    )
    .await;
    assert!(out.contains("limit"), "unexpected output: {out}");
    assert_eq!(host.sent.lock().unwrap().len(), MAX_MESSAGES_SENT);
}
