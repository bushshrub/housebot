//! tests bridge.

//! Unit tests for `lua_engine` (split out to keep the module under 600 lines).

use super::tests_support::*;
use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

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
