//! Unit tests for `lua_engine` (split out to keep the module under 600 lines).

use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

#[derive(Default)]
struct FakeHost {
    sent: Mutex<Vec<String>>,
    searches: AtomicUsize,
    last_search: Mutex<Option<String>>,
}

#[async_trait]
impl ScriptHost for FakeHost {
    async fn send_message(&self, content: &str) -> Result<(), String> {
        self.sent.lock().unwrap().push(content.to_string());
        Ok(())
    }

    async fn web_search(&self, query: &str, _max_results: usize) -> String {
        self.searches.fetch_add(1, Ordering::SeqCst);
        *self.last_search.lock().unwrap() = Some(query.to_string());
        format!("results for: {query}")
    }

    async fn jellyfin_search(&self, query: &str) -> String {
        format!("media for: {query}")
    }
}

fn limits() -> LuaLimits {
    LuaLimits {
        timeout: Duration::from_secs(2),
        memory_bytes: 8 * 1024 * 1024,
    }
}

async fn run(script: &str, host: &Arc<FakeHost>, limits: LuaLimits) -> String {
    run_full(script, host, limits).await.text
}

async fn run_full(script: &str, host: &Arc<FakeHost>, limits: LuaLimits) -> ScriptOutput {
    run_script(
        script.to_string(),
        Arc::clone(host) as Arc<dyn ScriptHost>,
        limits,
        |s: &str| s.to_string(),
    )
    .await
}

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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn api_call_cap_enforced() {
    let host = Arc::new(FakeHost::default());
    let out = run(
        "for i = 1, 20 do discord.web_search(\"q\" .. i) end",
        &host,
        limits(),
    )
    .await;
    assert!(out.contains("API calls"), "unexpected output: {out}");
    assert_eq!(host.searches.load(Ordering::SeqCst), MAX_API_CALLS);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn errors_can_be_caught_with_pcall() {
    let host = Arc::new(FakeHost::default());
    let out = run(
        "local ok, err = pcall(function() error(\"inner\") end) return ok, err",
        &host,
        limits(),
    )
    .await;
    assert!(out.starts_with("false"), "unexpected output: {out}");
    assert!(out.contains("inner"), "unexpected output: {out}");
}

fn is_png(bytes: &[u8]) -> bool {
    bytes.starts_with(b"\x89PNG\r\n\x1a\n")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn graph_calls_render_an_image() {
    let host = Arc::new(FakeHost::default());
    let result = run_full(
        "graph.node(\"a\", \"A\") graph.node(\"b\", \"B\") graph.edge(\"a\", \"b\")",
        &host,
        limits(),
    )
    .await;
    let image = result.image.expect("expected a rendered graph image");
    assert!(is_png(&image), "output was not a PNG");
    assert_eq!(result.text, "");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn script_without_graph_calls_has_no_image() {
    let host = Arc::new(FakeHost::default());
    let result = run_full("return 1", &host, limits()).await;
    assert!(result.image.is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn graph_edge_auto_creates_endpoints() {
    let host = Arc::new(FakeHost::default());
    let result = run_full("graph.edge(\"a\", \"b\")", &host, limits()).await;
    let image = result.image.expect("expected a rendered graph image");
    assert!(is_png(&image));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn graph_node_cap_enforced() {
    let host = Arc::new(FakeHost::default());
    let out = run(
        "for i = 1, 30 do graph.node(\"n\" .. i, \"N\" .. i) end",
        &host,
        limits(),
    )
    .await;
    assert!(out.contains("graph nodes"), "unexpected output: {out}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn graph_edge_cap_enforced() {
    let host = Arc::new(FakeHost::default());
    let out = run(
        "graph.node(\"a\", \"A\") graph.node(\"b\", \"B\") \
             for i = 1, 40 do graph.edge(\"a\", \"b\") end",
        &host,
        limits(),
    )
    .await;
    assert!(out.contains("graph edges"), "unexpected output: {out}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn graph_edge_with_two_new_endpoints_does_not_exceed_node_cap() {
    let host = Arc::new(FakeHost::default());
    // One slot free before the edge call: an edge naming two brand-new
    // endpoints must not be allowed to create both and overshoot the cap.
    let script = format!(
        "for i = 1, {} do graph.node(\"n\" .. i, \"N\" .. i) end \
             graph.edge(\"new_a\", \"new_b\")",
        MAX_GRAPH_NODES - 1
    );
    let out = run(&script, &host, limits()).await;
    assert!(out.contains("graph nodes"), "unexpected output: {out}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn graph_node_relabel_does_not_duplicate() {
    let host = Arc::new(FakeHost::default());
    let out = run(
        "graph.node(\"a\", \"First\") graph.node(\"a\", \"Second\") return \"ok\"",
        &host,
        limits(),
    )
    .await;
    assert_eq!(out, "ok");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn graph_title_does_not_error() {
    let host = Arc::new(FakeHost::default());
    let result = run_full(
        "graph.title(\"My Graph\") graph.node(\"a\", \"A\")",
        &host,
        limits(),
    )
    .await;
    assert!(result.image.is_some());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn graph_text_is_redacted_before_rendering() {
    let host = Arc::new(FakeHost::default());
    let called = Arc::new(AtomicUsize::new(0));
    let called_clone = Arc::clone(&called);
    let result = run_script(
        "graph.title(\"t\") graph.node(\"a\", \"A\")".to_string(),
        Arc::clone(&host) as Arc<dyn ScriptHost>,
        limits(),
        move |s: &str| {
            called_clone.fetch_add(1, Ordering::SeqCst);
            s.to_string()
        },
    )
    .await;
    assert!(result.image.is_some());
    // Once for the title, once for the one node label.
    assert_eq!(called.load(Ordering::SeqCst), 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn script_without_a_graph_never_calls_redact() {
    let host = Arc::new(FakeHost::default());
    let called = Arc::new(AtomicUsize::new(0));
    let called_clone = Arc::clone(&called);
    run_script(
        "return 1".to_string(),
        Arc::clone(&host) as Arc<dyn ScriptHost>,
        limits(),
        move |s: &str| {
            called_clone.fetch_add(1, Ordering::SeqCst);
            s.to_string()
        },
    )
    .await;
    assert_eq!(called.load(Ordering::SeqCst), 0);
}

#[test]
fn strips_fence_with_language_tag() {
    assert_eq!(strip_code_fence("```lua\nreturn 1\n```"), "return 1");
}

#[test]
fn strips_bare_fence() {
    assert_eq!(strip_code_fence("```\nreturn 1\n```"), "return 1");
}

#[test]
fn strips_single_line_fence() {
    assert_eq!(strip_code_fence("```return 1```"), "return 1");
}

#[test]
fn strips_inline_backticks() {
    assert_eq!(strip_code_fence("`return 1`"), "return 1");
}

#[test]
fn leaves_plain_script_untouched() {
    assert_eq!(strip_code_fence("  return 1  "), "return 1");
}

fn roles() -> Vec<(u64, String, u16)> {
    vec![
        (1, "@everyone".to_string(), 0),
        (2, "Member".to_string(), 1),
        (3, "Scripting".to_string(), 5),
        (4, "Moderator".to_string(), 7),
    ]
}

#[test]
fn scripting_role_grants_access() {
    assert!(scripting_permitted(&[3], &roles(), "Scripting"));
}

#[test]
fn higher_role_grants_access() {
    assert!(scripting_permitted(&[4], &roles(), "Scripting"));
}

#[test]
fn lower_role_is_denied() {
    assert!(!scripting_permitted(&[1, 2], &roles(), "Scripting"));
}

#[test]
fn role_name_match_is_case_insensitive() {
    assert!(scripting_permitted(&[3], &roles(), "scripting"));
}

#[test]
fn missing_scripting_role_disables_feature() {
    let no_scripting = vec![
        (1, "@everyone".to_string(), 0),
        (4, "Moderator".to_string(), 7),
    ];
    assert!(!scripting_permitted(&[4], &no_scripting, "Scripting"));
}

#[test]
fn no_roles_is_denied() {
    assert!(!scripting_permitted(&[], &roles(), "Scripting"));
}
