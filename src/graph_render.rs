//! Static PNG rendering for graphs (nodes/edges) defined by `/lua` scripts.
//!
//! Layout is breadth-first layering: nodes are stacked into rows by BFS
//! distance from a root, and rows unreachable from any root seed their own
//! layer. This isn't a general-purpose graph-drawing algorithm, but it's
//! deterministic, always terminates (even on cyclic input), and reads well
//! for the small flowcharts/network diagrams scripts are expected to build.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use plotters::coord::Shift;
use plotters::prelude::*;
use plotters::style::text_anchor::{HPos, Pos, VPos};

const NODE_WIDTH: i32 = 130;
const NODE_HEIGHT: i32 = 56;
const H_GAP: i32 = 30;
const V_GAP: i32 = 60;
const MARGIN: i32 = 30;
const TITLE_HEIGHT: i32 = 40;
const MAX_LABEL_CHARS: usize = 18;

const NODE_FILL: RGBColor = RGBColor(198, 224, 244);
const NODE_BORDER: RGBColor = RGBColor(41, 98, 155);
const EDGE_COLOR: RGBColor = RGBColor(90, 90, 90);

const FONT_BYTES: &[u8] = include_bytes!("../assets/fonts/LiberationSans-Regular.ttf");

/// Naming prefix for `render_png`'s scratch files, shared with
/// `sweep_stale_temp_files` so the sweep only ever touches files we made.
const TEMP_FILE_PREFIX: &str = "housebot-lua-graph-";

fn ensure_font_registered() {
    static REGISTER: std::sync::Once = std::sync::Once::new();
    REGISTER.call_once(|| {
        let _ = plotters::style::register_font("sans-serif", FontStyle::Normal, FONT_BYTES);
    });
}

/// Accumulates nodes and edges as a `/lua` script calls `graph.node`/`graph.edge`.
/// Caps on node/edge counts are enforced by the caller (`lua_engine`), which
/// tracks them alongside the other per-script API limits.
#[derive(Default)]
pub struct GraphBuilder {
    index: HashMap<String, usize>,
    labels: Vec<String>,
    edges: Vec<(usize, usize)>,
    title: Option<String>,
}

impl GraphBuilder {
    pub fn node_count(&self) -> usize {
        self.labels.len()
    }

    pub fn edge_count(&self) -> usize {
        self.edges.len()
    }

    pub fn is_empty(&self) -> bool {
        self.labels.is_empty()
    }

    pub fn has_node(&self, id: &str) -> bool {
        self.index.contains_key(id)
    }

    pub fn set_title(&mut self, title: &str) {
        self.title = Some(title.to_string());
    }

    /// Add a node, or update its label if `id` was already used. Returns the node's index.
    pub fn add_node(&mut self, id: &str, label: &str) -> usize {
        if let Some(&i) = self.index.get(id) {
            self.labels[i] = label.to_string();
            i
        } else {
            let i = self.labels.len();
            self.index.insert(id.to_string(), i);
            self.labels.push(label.to_string());
            i
        }
    }

    /// Look up a node by id, creating it (labelled with its id) if it isn't
    /// there yet. Used when an edge references an endpoint that was never
    /// explicitly declared with `add_node`, without clobbering a label a
    /// prior `add_node` call may have set.
    pub fn get_or_create(&mut self, id: &str) -> usize {
        match self.index.get(id) {
            Some(&i) => i,
            None => self.add_node(id, id),
        }
    }

    pub fn add_edge(&mut self, from: usize, to: usize) {
        self.edges.push((from, to));
    }
}

/// BFS layer (row) index for every node, by distance from the nearest root
/// (a node with no incoming edges). Nodes never reached this way — because
/// every path to them runs through a cycle, or they sit in a disconnected
/// component with no zero-indegree node — seed their own BFS afterwards.
/// Every node is visited exactly once, so this always terminates.
fn compute_layers(n: usize, edges: &[(usize, usize)]) -> Vec<usize> {
    let mut adj = vec![Vec::new(); n];
    let mut indeg = vec![0usize; n];
    for &(a, b) in edges {
        adj[a].push(b);
        indeg[b] += 1;
    }

    let mut layer = vec![usize::MAX; n];
    let mut queue = VecDeque::new();
    for i in 0..n {
        if indeg[i] == 0 {
            layer[i] = 0;
            queue.push_back(i);
        }
    }
    if queue.is_empty() && n > 0 {
        layer[0] = 0;
        queue.push_back(0);
    }
    bfs_assign(&adj, &mut layer, &mut queue);

    for i in 0..n {
        if layer[i] == usize::MAX {
            layer[i] = 0;
            let mut queue = VecDeque::from([i]);
            bfs_assign(&adj, &mut layer, &mut queue);
        }
    }
    layer
}

fn bfs_assign(adj: &[Vec<usize>], layer: &mut [usize], queue: &mut VecDeque<usize>) {
    while let Some(u) = queue.pop_front() {
        for &v in &adj[u] {
            if layer[v] == usize::MAX {
                layer[v] = layer[u] + 1;
                queue.push_back(v);
            }
        }
    }
}

/// Places each node at the center of its box, packing each layer's nodes
/// evenly in a centered row. Returns the positions plus the content size
/// (before any title header is added).
fn layout_positions(n: usize, layers: &[usize]) -> (Vec<(i32, i32)>, i32, i32) {
    let num_layers = layers.iter().copied().max().map_or(1, |m| m + 1);
    let mut rows: Vec<Vec<usize>> = vec![Vec::new(); num_layers];
    for i in 0..n {
        rows[layers[i]].push(i);
    }
    let max_row_len = rows.iter().map(Vec::len).max().unwrap_or(1).max(1) as i32;
    let canvas_w = MARGIN * 2 + max_row_len * NODE_WIDTH + (max_row_len - 1) * H_GAP;
    let canvas_h = MARGIN * 2 + num_layers as i32 * NODE_HEIGHT + (num_layers as i32 - 1) * V_GAP;

    let mut positions = vec![(0, 0); n];
    for (row_idx, row) in rows.iter().enumerate() {
        let row_len = row.len() as i32;
        let row_width = row_len * NODE_WIDTH + (row_len - 1) * H_GAP;
        let start_x = MARGIN + (canvas_w - MARGIN * 2 - row_width) / 2;
        let y = MARGIN + row_idx as i32 * (NODE_HEIGHT + V_GAP) + NODE_HEIGHT / 2;
        for (col_idx, &node_i) in row.iter().enumerate() {
            let x = start_x + col_idx as i32 * (NODE_WIDTH + H_GAP) + NODE_WIDTH / 2;
            positions[node_i] = (x, y);
        }
    }
    (positions, canvas_w, canvas_h)
}

/// Where an edge should visually start/end: the box boundary, not the
/// center, chosen from whichever axis (vertical or horizontal) separates
/// the two nodes more.
fn edge_endpoints(from: (i32, i32), to: (i32, i32)) -> ((i32, i32), (i32, i32)) {
    let half_w = NODE_WIDTH / 2;
    let half_h = NODE_HEIGHT / 2;
    match to.1.cmp(&from.1) {
        std::cmp::Ordering::Greater => ((from.0, from.1 + half_h), (to.0, to.1 - half_h)),
        std::cmp::Ordering::Less => ((from.0, from.1 - half_h), (to.0, to.1 + half_h)),
        std::cmp::Ordering::Equal if to.0 >= from.0 => {
            ((from.0 + half_w, from.1), (to.0 - half_w, to.1))
        }
        std::cmp::Ordering::Equal => ((from.0 - half_w, from.1), (to.0 + half_w, to.1)),
    }
}

fn truncate_label(label: &str) -> String {
    if label.chars().count() <= MAX_LABEL_CHARS {
        return label.to_string();
    }
    let mut truncated: String = label.chars().take(MAX_LABEL_CHARS - 1).collect();
    truncated.push('…');
    truncated
}

fn centered_style(size: u32) -> TextStyle<'static> {
    TextStyle::from(("sans-serif", size).into_font())
        .color(&BLACK)
        .pos(Pos::new(HPos::Center, VPos::Center))
}

fn draw_edge<DB: DrawingBackend>(
    area: &DrawingArea<DB, Shift>,
    from: (i32, i32),
    to: (i32, i32),
) -> Result<(), String> {
    let (start, end) = edge_endpoints(from, to);
    area.draw(&PathElement::new(
        vec![start, end],
        ShapeStyle::from(&EDGE_COLOR).stroke_width(2),
    ))
    .map_err(|e| e.to_string())?;

    let dx = (end.0 - start.0) as f64;
    let dy = (end.1 - start.1) as f64;
    let len = dx.hypot(dy);
    if len < 1.0 {
        return Ok(());
    }
    let (ux, uy) = (dx / len, dy / len);
    let (px, py) = (-uy, ux);
    let size = 9.0;
    let base = (
        end.0 as f64 - ux * size * 1.6,
        end.1 as f64 - uy * size * 1.6,
    );
    let left = (
        (base.0 + px * size * 0.6) as i32,
        (base.1 + py * size * 0.6) as i32,
    );
    let right = (
        (base.0 - px * size * 0.6) as i32,
        (base.1 - py * size * 0.6) as i32,
    );
    area.draw(&Polygon::new(
        vec![end, left, right],
        ShapeStyle::from(&EDGE_COLOR).filled(),
    ))
    .map_err(|e| e.to_string())
}

fn draw_node<DB: DrawingBackend>(
    area: &DrawingArea<DB, Shift>,
    center: (i32, i32),
    label: &str,
) -> Result<(), String> {
    let half_w = NODE_WIDTH / 2;
    let half_h = NODE_HEIGHT / 2;
    let top_left = (center.0 - half_w, center.1 - half_h);
    let bottom_right = (center.0 + half_w, center.1 + half_h);
    area.draw(&Rectangle::new(
        [top_left, bottom_right],
        ShapeStyle::from(&NODE_FILL).filled(),
    ))
    .map_err(|e| e.to_string())?;
    area.draw(&Rectangle::new(
        [top_left, bottom_right],
        ShapeStyle::from(&NODE_BORDER).stroke_width(2),
    ))
    .map_err(|e| e.to_string())?;
    area.draw(&Text::new(
        truncate_label(label),
        center,
        centered_style(16),
    ))
    .map_err(|e| e.to_string())
}

/// Deletes its wrapped path on drop, so the scratch PNG file used during
/// rendering is removed on every exit path — including a panic unwinding
/// out of `render_to_path` (e.g. a plotters-internal panic on pathological
/// input) — not just the ordinary success/error returns.
struct TempFileGuard(PathBuf);

impl Drop for TempFileGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Render the accumulated graph to PNG bytes. Renders through a scratch file
/// (plotters' bitmap backend has no in-memory PNG target) that is always
/// cleaned up, success, error, or panic.
pub fn render_png(graph: &GraphBuilder) -> Result<Vec<u8>, String> {
    if graph.is_empty() {
        return Err("graph has no nodes".to_string());
    }
    ensure_font_registered();

    let n = graph.labels.len();
    let layers = compute_layers(n, &graph.edges);
    let (positions, content_w, content_h) = layout_positions(n, &layers);
    let title_offset = if graph.title.is_some() {
        TITLE_HEIGHT
    } else {
        0
    };
    let width = content_w.max(NODE_WIDTH + MARGIN * 2) as u32;
    let height = (content_h + title_offset).max(NODE_HEIGHT + MARGIN * 2) as u32;

    let path = std::env::temp_dir().join(format!("{TEMP_FILE_PREFIX}{}.png", uuid::Uuid::new_v4()));
    let _cleanup = TempFileGuard(path.clone());
    render_to_path(&path, width, height, graph, &positions, title_offset).and_then(|()| {
        std::fs::read(&path).map_err(|e| format!("failed to read rendered image: {e}"))
    })
}

/// Removes stray scratch PNGs left under `dir` by `render_png` — normally
/// nothing accumulates (see `TempFileGuard`), but a hard crash (OOM-kill,
/// `SIGKILL`) skips `Drop`, and older builds could leak one on an internal
/// panic. Only touches files matching our own naming prefix, and only those
/// past `max_age`, so it won't collide with a render that's still in
/// flight. Returns the number of files removed.
pub fn sweep_stale_temp_files(dir: &Path, max_age: Duration) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    let now = SystemTime::now();
    let mut removed = 0;
    for entry in entries.flatten() {
        let is_ours = entry
            .file_name()
            .to_str()
            .is_some_and(|name| name.starts_with(TEMP_FILE_PREFIX) && name.ends_with(".png"));
        if !is_ours {
            continue;
        }
        let age = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|modified| now.duration_since(modified).ok());
        if age.is_some_and(|age| age >= max_age) && std::fs::remove_file(entry.path()).is_ok() {
            removed += 1;
        }
    }
    removed
}

fn render_to_path(
    path: &std::path::Path,
    width: u32,
    height: u32,
    graph: &GraphBuilder,
    positions: &[(i32, i32)],
    title_offset: i32,
) -> Result<(), String> {
    let root = BitMapBackend::new(path, (width, height)).into_drawing_area();
    root.fill(&WHITE).map_err(|e| e.to_string())?;

    if let Some(title) = &graph.title {
        root.draw(&Text::new(
            title.as_str(),
            (width as i32 / 2, title_offset / 2),
            centered_style(20),
        ))
        .map_err(|e| e.to_string())?;
    }

    let shifted: Vec<(i32, i32)> = positions
        .iter()
        .map(|(x, y)| (*x, *y + title_offset))
        .collect();
    for &(from, to) in &graph.edges {
        if from != to {
            draw_edge(&root, shifted[from], shifted[to])?;
        }
    }
    for (i, label) in graph.labels.iter().enumerate() {
        draw_node(&root, shifted[i], label)?;
    }

    root.present().map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
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
}
