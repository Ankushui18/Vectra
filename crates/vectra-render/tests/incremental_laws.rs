//! **Incremental Update Law** (Task 5.0 §2/§5, RULE 2).
//!
//! > Change the colour of Node A. Assert that the renderer's `update` only
//! > touches the uniform buffer for Node A, and the vertex buffers for all other
//! > nodes remain untouched.
//!
//! The assertion is made against a [`MockSink`] that keeps the GPU's buffers as
//! bytes and records every write. So "untouched" is not a claim about call
//! counts — it is **byte equality** of what the buffer held before and after.
//!
//! The brief's own case is the first test. The suite then goes further, because
//! the same machinery makes a stronger statement available: a *move* must not
//! touch vertex data either. That is the node-local-frame design in
//! `crates/vectra-render/src/geometry.rs` paying for itself — a drag writes one
//! 80-byte instance row per moved node and nothing else, no matter how large the
//! shape is.

use vectra_core::{Color, NodeId};
use vectra_geometry::{
    DirtySet, EvaluatedNode, EvaluatedPaint, EvaluatedPrimitive, EvaluatedScene, EvaluatedStyle,
};
use vectra_render::{MeshKind, MockSink, RenderScene, WriteKind};

fn rect(x: f64, y: f64, w: f64, h: f64) -> EvaluatedPrimitive {
    EvaluatedPrimitive::Rect {
        x,
        y,
        w,
        h,
        corner_radius: 0.0,
    }
}

fn style(fill: Color) -> EvaluatedStyle {
    EvaluatedStyle::solid(fill, Color::TRANSPARENT, 0.0, 1.0)
}

/// A two-node scene the individual laws mutate in place.
struct Fixture {
    scene: EvaluatedScene,
    render: RenderScene,
    sink: MockSink,
    a: NodeId,
    b: NodeId,
}

impl Fixture {
    fn new() -> Self {
        let a = NodeId::from_u128(1);
        let b = NodeId::from_u128(2);
        let mut scene = EvaluatedScene::empty();
        scene.nodes.insert(
            a,
            EvaluatedNode::new(
                a,
                rect(0.0, 0.0, 100.0, 50.0),
                style(Color::rgb(0x22, 0x66, 0xee)),
            ),
        );
        scene.nodes.insert(
            b,
            EvaluatedNode::new(
                b,
                rect(300.0, 0.0, 20.0, 20.0),
                style(Color::rgb(0xee, 0x66, 0x22)),
            ),
        );
        scene.z_order = vec![a, b];
        let mut render = RenderScene::new();
        render.sync(&scene, &DirtySet::all());
        let mut sink = MockSink::new();
        render.flush(&mut sink).expect("mock sink");
        // The fixture starts settled: the laws below measure the *next* sync.
        Self {
            scene,
            render,
            sink,
            a,
            b,
        }
    }

    /// Sync with exactly these dirty ids, flush, and return the sink calls made.
    fn sync(&mut self, dirty: &[NodeId]) -> Vec<vectra_render::SinkCall> {
        self.sink.calls.clear();
        self.render
            .sync(&self.scene, &DirtySet::nodes(dirty.to_vec()));
        self.render.flush(&mut self.sink).expect("mock sink");
        self.sink.calls.clone()
    }

    fn set_fill(&mut self, id: NodeId, fill: Color) {
        // The canonical style is a *stack*: a restyle reaches into the first fill
        // layer, which is the row the draw list draws first.
        let style = &mut self.scene.nodes.get_mut(&id).expect("node").style;
        for layer in style.appearances.iter_mut() {
            let is_stroke = layer.is_stroke();
            if let EvaluatedPaint::Solid(existing) = &mut layer.paint {
                if !is_stroke {
                    *existing = fill;
                    return;
                }
            }
        }
        panic!("the fixture node has a fill layer");
    }

    /// Give a node a stroke of `width` in `color`, keeping its fill.
    fn set_stroke(&mut self, id: NodeId, color: Color, width: f64) {
        let style = &mut self.scene.nodes.get_mut(&id).expect("node").style;
        let fill = style.first_fill().unwrap().paint.preview_color();
        *style = EvaluatedStyle::solid(fill, color, width, 1.0);
    }

    /// Recolour a node's first stroke, keeping its width and fill.
    fn recolor_stroke(&mut self, id: NodeId, color: Color) {
        let style = &mut self.scene.nodes.get_mut(&id).expect("node").style;
        let fill = style.first_fill().unwrap().paint.preview_color();
        let width = style.max_stroke_width();
        *style = EvaluatedStyle::solid(fill, color, width, 1.0);
    }

    fn translate(&mut self, id: NodeId, dx: f64, dy: f64) {
        let node = self.scene.nodes.get_mut(&id).expect("node");
        if let EvaluatedPrimitive::Rect { x, y, .. } = &mut node.primitive {
            *x += dx;
            *y += dy;
        } else {
            panic!("fixture nodes are rectangles");
        }
    }

    fn resize(&mut self, id: NodeId, w: f64, h: f64) {
        let node = self.scene.nodes.get_mut(&id).expect("node");
        if let EvaluatedPrimitive::Rect {
            w: width,
            h: height,
            ..
        } = &mut node.primitive
        {
            *width = w;
            *height = h;
        } else {
            panic!("fixture nodes are rectangles");
        }
    }
}

#[test]
fn a_style_change_touches_only_that_nodes_instance() {
    let mut f = Fixture::new();
    let b_vertices_before = f.sink.nodes[&f.b].fill_vertices.clone();
    let b_indices_before = f.sink.nodes[&f.b].fill_indices.clone();
    let b_instance_before = f.sink.nodes[&f.b].instance.clone();
    let a_vertices_before = f.sink.nodes[&f.a].fill_vertices.clone();

    f.set_fill(f.a, Color::rgb(0x00, 0xff, 0x00));
    let calls = f.sync(&[f.a]);

    // Exactly one write, for A's instance row.
    assert_eq!(calls.len(), 1, "{calls:?}");
    assert_eq!(calls[0].id, f.a);
    assert_eq!(calls[0].kind, WriteKind::Instance);
    assert_eq!(calls[0].bytes, 80);

    // A's vertex data is untouched (nothing about the shape moved)…
    assert_eq!(f.sink.nodes[&f.a].fill_vertices, a_vertices_before);
    // …and B is byte-identical in every buffer it owns.
    assert_eq!(f.sink.nodes[&f.b].fill_vertices, b_vertices_before);
    assert_eq!(f.sink.nodes[&f.b].fill_indices, b_indices_before);
    assert_eq!(f.sink.nodes[&f.b].instance, b_instance_before);
    // The instance *array* slice for B's slot was not written either.
    let b_slot = f.render.get(f.b).unwrap().items[0].slot;
    assert!(!f.sink.calls.iter().any(|call| call.slot == b_slot));
    // And the new colour is really there.
    let instance = f.render.get(f.a).unwrap().instances[0];
    assert_eq!(instance.color[1], 1.0);
}

#[test]
fn a_move_touches_only_that_nodes_instance() {
    let mut f = Fixture::new();
    let a_vertices_before = f.sink.nodes[&f.a].fill_vertices.clone();
    let a_indices_before = f.sink.nodes[&f.a].fill_indices.clone();

    f.translate(f.a, 37.5, -12.25);
    let calls = f.sync(&[f.a]);

    assert_eq!(calls.len(), 1, "a drag is one instance write: {calls:?}");
    assert_eq!(calls[0].kind, WriteKind::Instance);
    // The whole point of the local frame: a large shape moving does not
    // re-upload a single vertex.
    assert_eq!(f.sink.nodes[&f.a].fill_vertices, a_vertices_before);
    assert_eq!(f.sink.nodes[&f.a].fill_indices, a_indices_before);
    let slot = f.render.get(f.a).unwrap();
    assert_eq!(slot.key.origin, (37.5, -12.25));
    assert_eq!(slot.instances[0].transform[2..4], [37.5, -12.25]);
}

#[test]
fn a_resize_rewrites_that_nodes_meshes_and_nothing_of_its_neighbour() {
    let mut f = Fixture::new();
    let b_before = f.sink.nodes[&f.b].clone();

    f.resize(f.a, 250.0, 90.0);
    let calls = f.sync(&[f.a]);

    let kinds: Vec<WriteKind> = calls.iter().map(|call| call.kind).collect();
    assert!(
        kinds.contains(&WriteKind::Vertices(MeshKind::Fill)),
        "{kinds:?}"
    );
    assert!(
        kinds.contains(&WriteKind::Indices(MeshKind::Fill)),
        "{kinds:?}"
    );
    assert!(
        !calls.iter().any(|call| call.id == f.b),
        "the neighbour must not be touched: {calls:?}"
    );
    assert_eq!(f.sink.nodes[&f.b], b_before, "B is byte-identical");
}

#[test]
fn a_dirty_but_unchanged_node_writes_nothing() {
    let mut f = Fixture::new();
    let calls = f.sync(&[f.a, f.b]);
    assert!(calls.is_empty(), "no change, no writes: {calls:?}");
    assert_eq!(f.render.plan().ops.len(), 0);
    let report = f.render.report();
    assert_eq!(report.writes, 0);
    assert_eq!(report.bytes, 0);
    assert!(report.touched.is_empty());
    // The nodes are still live and still drawn.
    assert_eq!(report.nodes, 2);
}

#[test]
fn a_style_change_on_a_stroked_node_does_not_touch_its_stroke_geometry() {
    let mut f = Fixture::new();
    f.set_stroke(f.a, Color::BLACK, 4.0);
    let calls = f.sync(&[f.a]);
    // Adding a stroke *is* a geometry change (the outline did not exist before).
    assert!(
        calls
            .iter()
            .any(|call| call.kind == WriteKind::Vertices(MeshKind::Stroke(0))),
        "{calls:?}"
    );
    let stroke_vertices_after = f.sink.nodes[&f.a].stroke_vertices[&0].clone();
    assert!(!stroke_vertices_after.is_empty());

    // Now change only its colour: the stroke's *buffers* must not be rewritten.
    f.recolor_stroke(f.a, Color::rgb(0xff, 0x00, 0x00));
    let calls = f.sync(&[f.a]);
    assert_eq!(calls.len(), 1, "{calls:?}");
    assert_eq!(calls[0].kind, WriteKind::Instance);
    assert_eq!(
        f.sink.nodes[&f.a].stroke_vertices[&0],
        stroke_vertices_after
    );
}

#[test]
fn adding_and_removing_nodes_touches_only_their_own_buffers() {
    let mut f = Fixture::new();
    let c = NodeId::from_u128(3);
    let a_before = f.sink.nodes[&f.a].clone();
    let b_before = f.sink.nodes[&f.b].clone();

    f.scene.nodes.insert(
        c,
        EvaluatedNode::new(
            c,
            rect(500.0, 500.0, 40.0, 40.0),
            style(Color::rgb(0x88, 0x88, 0x88)),
        ),
    );
    f.scene.z_order.push(c);
    let calls = f.sync(&[c]);
    assert!(calls.iter().all(|call| call.id == c), "{calls:?}");
    assert!(calls.iter().any(|call| call.kind == WriteKind::Create));
    assert_eq!(f.sink.nodes[&f.a], a_before, "A untouched by an insertion");
    assert_eq!(f.sink.nodes[&f.b], b_before, "B untouched by an insertion");

    // Removal drops exactly that node's buffers.
    f.scene.nodes.remove(&c);
    f.scene.z_order.retain(|id| *id != c);
    let calls = f.sync(&[c]);
    assert!(f.sink.dropped.contains(&c));
    assert!(
        calls.is_empty(),
        "dropping is not a buffer write: {calls:?}"
    );
    assert!(!f.sink.nodes.contains_key(&c));
    assert_eq!(f.sink.nodes[&f.a], a_before);
    assert_eq!(f.sink.nodes[&f.b], b_before);
    assert_eq!(f.render.len(), 2);
}

#[test]
fn a_deleted_node_without_a_dirty_hint_is_still_dropped() {
    // The engine's `Dirty` event is the renderer's *hint*, not its contract: a
    // node that vanished from the scene must not keep drawing from a stale
    // buffer, whatever the dirty set says.
    let mut f = Fixture::new();
    f.scene.nodes.remove(&f.b);
    f.scene.z_order.retain(|id| *id != f.b);
    let _ = f.sync(&[f.a]);
    assert!(f.sink.dropped.contains(&f.b));
    assert!(f.render.get(f.b).is_none());
}

#[test]
fn a_full_resync_of_a_settled_scene_is_free_but_a_cold_renderer_pays_once() {
    let mut f = Fixture::new();
    f.sink.calls.clear();
    f.render.sync(&f.scene, &DirtySet::all());
    f.render.flush(&mut f.sink).unwrap();
    assert!(f.sink.calls.is_empty(), "warm full sync writes nothing");

    // A cold renderer builds everything exactly once.
    let mut cold = RenderScene::new();
    let mut sink = MockSink::new();
    cold.sync(&f.scene, &DirtySet::all());
    cold.flush(&mut sink).unwrap();
    assert_eq!(cold.report().created, 2);
    assert_eq!(cold.report().retessellated, 2);
    assert_eq!(sink.nodes.len(), 2);
}

#[test]
fn draw_order_reorders_without_touching_a_single_buffer() {
    let mut f = Fixture::new();
    let a_before = f.sink.nodes[&f.a].clone();
    let b_before = f.sink.nodes[&f.b].clone();
    f.scene.z_order = vec![f.b, f.a];
    let calls = f.sync(&[]);
    assert!(calls.is_empty(), "{calls:?}");
    assert!(f.render.plan().reordered);
    assert_eq!(f.render.order(), &[f.b, f.a]);
    assert_eq!(f.render.plan().order, vec![f.b, f.a]);
    assert_eq!(f.sink.nodes[&f.a], a_before);
    assert_eq!(f.sink.nodes[&f.b], b_before);
    assert_eq!(
        f.sink.order,
        vec![f.b, f.a],
        "the sink learned the new order"
    );
}

#[test]
fn an_instance_slot_is_reused_rather_than_leaked() {
    let mut f = Fixture::new();
    let a_slot = f.render.get(f.a).unwrap().items[0].slot;
    // Drop A, then add a new node: it must take A's slot, so the instance array
    // does not grow without bound across an editing session.
    f.scene.nodes.remove(&f.a);
    f.scene.z_order.retain(|id| *id != f.a);
    let _ = f.sync(&[f.a]);
    let c = NodeId::from_u128(9);
    f.scene.nodes.insert(
        c,
        EvaluatedNode::new(c, rect(700.0, 700.0, 10.0, 10.0), style(Color::BLACK)),
    );
    f.scene.z_order.push(c);
    let _ = f.sync(&[c]);
    assert_eq!(f.render.get(c).unwrap().items[0].slot, a_slot);
    assert_eq!(f.render.instance_count(), 2, "still two slots, not three");
}

#[test]
fn a_proptest_style_sweep_of_edits_touches_only_what_changed() {
    // A deterministic sweep: 64 edits across two nodes, alternating restyle /
    // move / resize. For every edit, the writes must name the edited node and
    // no other, and the untouched node's bytes must be identical.
    let mut f = Fixture::new();
    for step in 0..64u32 {
        let (target, other) = if step % 2 == 0 {
            (f.a, f.b)
        } else {
            (f.b, f.a)
        };
        let other_before = f.sink.nodes[&other].clone();
        let other_slot = f.render.get(other).unwrap().items[0].slot;
        let kind = step % 3;
        match kind {
            0 => f.set_fill(target, Color::rgb(step as u8, 0x40, 0x80)),
            1 => f.translate(target, 1.5, -0.75),
            _ => f.resize(target, 10.0 + step as f64, 20.0 + step as f64),
        }
        let calls = f.sync(&[target]);
        assert!(
            calls.iter().all(|call| call.id == target),
            "step {step} wrote to a neighbour: {calls:?}"
        );
        assert!(
            !calls.iter().any(|call| call.slot == other_slot),
            "step {step} wrote into the neighbour's instance slot"
        );
        assert_eq!(
            f.sink.nodes[&other], other_before,
            "step {step} changed the untouched node's bytes"
        );
        // A style or move edit is instance-only; a resize is not.
        if kind != 2 {
            assert_eq!(calls.len(), 1, "step {step}: {calls:?}");
            assert_eq!(calls[0].kind, WriteKind::Instance);
        }
    }
}
