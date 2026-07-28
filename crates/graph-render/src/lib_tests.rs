//! Unit tests for `lib` (split out to keep the module under 400 lines).

use super::*;

#[test]
fn add_node_dedups_by_id_and_updates_label() {
    let mut g = GraphBuilder::default();
    let a = g.add_node("a", "Alpha");
    let a2 = g.add_node("a", "Alpha v2");
    assert_eq!(a, a2);
    assert_eq!(g.node_count(), 1);
    assert_eq!(g.labels[0], "Alpha v2");
}

#[test]
fn layers_follow_bfs_distance_from_root() {
    // a -> b -> c
    let layers = compute_layers(3, &[(0, 1), (1, 2)]);
    assert_eq!(layers, vec![0, 1, 2]);
}

#[test]
fn layers_handle_disconnected_nodes() {
    // a -> b, c isolated
    let layers = compute_layers(3, &[(0, 1)]);
    assert_eq!(layers[0], 0);
    assert_eq!(layers[1], 1);
    assert_eq!(layers[2], 0);
}

#[test]
fn layers_terminate_on_a_cycle() {
    // a -> b -> a, must not hang and must assign every node a layer.
    let layers = compute_layers(2, &[(0, 1), (1, 0)]);
    assert_eq!(layers.len(), 2);
}

#[test]
fn render_png_rejects_empty_graph() {
    let g = GraphBuilder::default();
    assert!(render_png(&g).is_err());
}

#[test]
fn render_png_produces_a_valid_png() {
    let mut g = GraphBuilder::default();
    g.set_title("Test");
    let a = g.add_node("a", "Node A");
    let b = g.add_node("b", "Node B");
    g.add_edge(a, b);
    let bytes = render_png(&g).expect("render should succeed");
    assert_eq!(&bytes[0..8], b"\x89PNG\r\n\x1a\n");
}

#[test]
fn render_png_handles_self_loop_without_crashing() {
    let mut g = GraphBuilder::default();
    let a = g.add_node("a", "Solo");
    g.add_edge(a, a);
    let bytes = render_png(&g).expect("render should succeed");
    assert_eq!(&bytes[0..8], b"\x89PNG\r\n\x1a\n");
}

#[test]
fn truncate_label_adds_ellipsis_when_too_long() {
    let long = "a".repeat(40);
    let truncated = truncate_label(&long);
    assert_eq!(truncated.chars().count(), MAX_LABEL_CHARS);
    assert!(truncated.ends_with('…'));
}

#[test]
fn temp_file_guard_removes_file_on_drop() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("scratch.png");
    std::fs::write(&path, b"x").unwrap();
    {
        let _guard = TempFileGuard(path.clone());
        assert!(path.exists());
    }
    assert!(!path.exists());
}

#[test]
fn sweep_removes_only_stale_files_matching_our_prefix() {
    let dir = tempfile::TempDir::new().unwrap();
    let ours = dir.path().join(format!("{TEMP_FILE_PREFIX}abc.png"));
    let unrelated = dir.path().join("something-else.png");
    std::fs::write(&ours, b"x").unwrap();
    std::fs::write(&unrelated, b"x").unwrap();

    // A generous max_age: nothing this fresh should be swept yet.
    assert_eq!(
        sweep_stale_temp_files(dir.path(), Duration::from_secs(3600)),
        0
    );
    assert!(ours.exists());
    assert!(unrelated.exists());

    // max_age of zero treats both files as stale, but only the one
    // matching our naming prefix should be removed.
    assert_eq!(sweep_stale_temp_files(dir.path(), Duration::ZERO), 1);
    assert!(!ours.exists());
    assert!(unrelated.exists());
}

#[test]
fn sweep_on_missing_dir_returns_zero() {
    let missing = std::path::Path::new("/nonexistent/housebot-graph-sweep-test");
    assert_eq!(sweep_stale_temp_files(missing, Duration::ZERO), 0);
}

#[test]
fn redact_with_applies_to_every_label_and_the_title() {
    let mut g = GraphBuilder::default();
    g.set_title("leaked: secret-token-value");
    let a = g.add_node("a", "contains secret-token-value here");
    let b = g.add_node("b", "clean label");
    let _ = (a, b);
    g.redact_with(|s| s.replace("secret-token-value", "[REDACTED]"));
    assert_eq!(g.title.as_deref(), Some("leaked: [REDACTED]"));
    assert_eq!(g.labels[0], "contains [REDACTED] here");
    assert_eq!(g.labels[1], "clean label");
}
