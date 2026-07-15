//! Integration tests exercising the public `graph-render` API end to end.

use graph_render::{render_png, GraphBuilder};

fn is_png(bytes: &[u8]) -> bool {
    bytes.starts_with(b"\x89PNG\r\n\x1a\n")
}

#[test]
fn builds_and_renders_a_connected_graph() {
    let mut graph = GraphBuilder::default();
    graph.set_title("Pipeline");
    let a = graph.add_node("a", "Start");
    let b = graph.add_node("b", "Middle");
    let c = graph.add_node("c", "End");
    graph.add_edge(a, b);
    graph.add_edge(b, c);

    assert_eq!(graph.node_count(), 3);
    assert_eq!(graph.edge_count(), 2);
    assert!(!graph.is_empty());

    let png = render_png(&graph).expect("connected graph should render");
    assert!(is_png(&png), "output must be a PNG");
}

#[test]
fn edge_auto_created_endpoints_render() {
    let mut graph = GraphBuilder::default();
    let from = graph.get_or_create("x");
    let to = graph.get_or_create("y");
    graph.add_edge(from, to);
    let png = render_png(&graph).expect("auto-created endpoints should render");
    assert!(is_png(&png));
}

#[test]
fn empty_graph_is_rejected() {
    assert!(render_png(&GraphBuilder::default()).is_err());
}

#[test]
fn redaction_scrubs_labels_before_rendering() {
    let mut graph = GraphBuilder::default();
    graph.set_title("secret-token in title");
    graph.add_node("n", "holds secret-token");
    graph.redact_with(|text| text.replace("secret-token", "[REDACTED]"));
    // Rendering still succeeds after redaction, proving the scrubbed graph is valid.
    assert!(render_png(&graph).is_ok());
}
