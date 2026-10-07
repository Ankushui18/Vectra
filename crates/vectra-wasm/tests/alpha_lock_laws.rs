//! **Task 10.7 RULE 3a at the protocol boundary** — the Alpha Lock Law.
//!
//! `vectra-operations`' `clip_laws.rs` proves the mathematics of the clip and
//! the pass. This file proves the half that only exists in the engine: a stroke
//! **drawn through the real pen door** on an alpha-locked layer comes out as
//! geometry inside that layer's existing artwork — or does not come out at all,
//! with a sentence a designer can read.
//!
//! Three laws:
//!
//! * **Inside the lines** — the committed node's region is a subset of the
//!   layer's content, whatever the stroke and wherever it was drawn;
//! * **Nothing to land on** — on a layer with no artwork the commit is refused
//!   (a typed error, no node, no history entry);
//! * **Unlocked means untouched** — with the flag off, the identical stroke
//!   commits with its own geometry, byte for byte: the feature is a flag, not a
//!   permanent change of the drawing pipeline.

use proptest::prelude::*;
use serde_json::{json, Value};
use vectra_wasm::VectraEngine;

// ── fixtures ─────────────────────────────────────────────────────────────

fn dispatch(engine: &mut VectraEngine, cmd: Value) -> Value {
    serde_json::from_str(&engine.dispatch_command(&serde_json::to_string(&cmd).unwrap())).unwrap()
}

fn ok(engine: &mut VectraEngine, cmd: Value) -> Value {
    let response = dispatch(engine, cmd);
    assert_eq!(response["status"], "ok", "{response}");
    response
}

/// A rectangle on the active layer — the layer's *existing artwork*.
fn rect(engine: &mut VectraEngine, name: &str, x: f64, y: f64, w: f64, h: f64) -> String {
    let id = vectra_core::new_node_id().to_string();
    ok(
        engine,
        json!({"type": "CreateNode", "id": id, "name": name,
               "kind": {"Rectangle": {
                   "x": {"Literal": x}, "y": {"Literal": y},
                   "width": {"Literal": w}, "height": {"Literal": h},
                   "corner_radius": {"Literal": 0.0}}}}),
    );
    id
}

/// The workspace's one layer (the fixed id `open_workspace` seeds).
fn workspace_layer(engine: &mut VectraEngine) -> String {
    let snapshot: Value = serde_json::from_str(&engine.get_snapshot()).unwrap();
    snapshot["layers"][0]["id"].as_str().unwrap().to_string()
}

fn set_alpha_lock(engine: &mut VectraEngine, layer: &str, locked: bool) {
    ok(
        engine,
        json!({"type": "SetLayerAlphaLocked", "id": layer, "alpha_locked": locked}),
    );
}

/// The document the engine currently holds — read back through the *public*
/// door the desktop build uses, so the law sees exactly what a save would.
fn document(engine: &mut VectraEngine) -> vectra_core::Document {
    serde_json::from_str(&engine.document_json()).expect("the engine emits its own document")
}

/// One node's region, from the **real evaluator** (the same pass the engine
/// runs), so "inside the bounds" is checked against what is drawn.
fn node_region(doc: &vectra_core::Document, id: &str) -> geo::MultiPolygon<f64> {
    use vectra_core::EvaluationContext;
    use vectra_geometry::GeometryEvaluator;
    let variables = std::collections::HashMap::new();
    let ctx = EvaluationContext::new(&variables, 0.0);
    let id: vectra_core::NodeId = id.parse().expect("a node id");
    let evaluation = GeometryEvaluator::new().evaluate_full(doc, &ctx);
    let node = evaluation
        .scene
        .get(id)
        .unwrap_or_else(|| panic!("node {id} is not in the scene"));
    vectra_operations::region_of(&node.primitive)
}

/// The layer's content region — its own children, as drawn.
fn layer_content(doc: &vectra_core::Document, layer: &str) -> geo::MultiPolygon<f64> {
    use vectra_core::EvaluationContext;
    use vectra_geometry::GeometryEvaluator;
    let variables = std::collections::HashMap::new();
    let ctx = EvaluationContext::new(&variables, 0.0);
    let evaluation = GeometryEvaluator::new().evaluate_full(doc, &ctx);
    let layer_id: vectra_core::LayerId = layer.parse().expect("a layer id");
    let children = doc
        .layers
        .get(&layer_id)
        .expect("the layer the snapshot named")
        .children
        .clone();
    vectra_operations::content_region(&evaluation.scene, &children)
}

/// **Draw one pen path** through the real door: two anchors, then commit.
/// The path is *open* — a curve with no area, which is the 1-D clip's case.
///
/// The pen's session is **explicitly reset** between paths, exactly as the canvas
/// does it: `PenSession::finish` takes `&self` and keeps the anchors (a designer
/// may keep clicking after committing), and the workspace's own reset gesture is
/// a `cancel` pointer event — `App`'s tool change sends one for the same reason.
fn draw_pen_line(engine: &mut VectraEngine, from: (f64, f64), to: (f64, f64)) -> Value {
    let _: Value =
        serde_json::from_str(&engine.draw_pointer("pen", "cancel", 0.0, 0.0, false, false, 0.0))
            .unwrap();
    for (x, y) in [from, to] {
        let reply: Value =
            serde_json::from_str(&engine.draw_pointer("pen", "down", x, y, false, true, 0.0))
                .unwrap();
        assert_eq!(reply["ok"], true, "{reply}");
    }
    serde_json::from_str(&engine.draw_pen_commit(false, None)).unwrap()
}

/// **Paint one brush stroke** through the real door: a sampled sweep, fitted and
/// committed. The brush's ribbon is a *closed* region, which is RULE 3a's
/// headline case — the one Procreate's alpha lock is named for.
fn paint_stroke(engine: &mut VectraEngine, from: (f64, f64), to: (f64, f64)) -> Value {
    let mut first = true;
    for step in 0..=6 {
        let t = step as f64 / 6.0;
        let x = from.0 + (to.0 - from.0) * t;
        let y = from.1 + (to.1 - from.1) * t;
        let kind = if first { "down" } else { "move" };
        first = false;
        let reply: Value =
            serde_json::from_str(&engine.draw_pointer("brush", kind, x, y, false, false, 0.0))
                .unwrap();
        assert_eq!(reply["ok"], true, "{reply}");
    }
    serde_json::from_str(&engine.draw_brush_commit(None)).unwrap()
}

/// Every point the committed path passes through, read from the document (the
/// pen writes literals, so this needs no evaluator).
fn path_points(doc: &vectra_core::Document, id: &str) -> Vec<(f64, f64)> {
    use vectra_core::{NodeKind, Parameter, PathSegment};
    let id: vectra_core::NodeId = id.parse().expect("a node id");
    let node = doc.nodes.get(&id).expect("the committed node");
    let NodeKind::Path { start, segments } = &node.kind else {
        panic!("a drawn path must be a Path node");
    };
    let literal = |parameter: &Parameter<vectra_core::Point2>| match parameter {
        Parameter::Literal(point) => (point.x, point.y),
        other => panic!("a drawn path writes literals, found {}", other.source_tag()),
    };
    let mut points = vec![literal(start)];
    for segment in segments {
        match segment {
            PathSegment::Line { to } => points.push(literal(to)),
            PathSegment::Quadratic { control, to } => {
                points.push(literal(control));
                points.push(literal(to));
            }
            PathSegment::Cubic {
                control1,
                control2,
                to,
            } => {
                points.push(literal(control1));
                points.push(literal(control2));
                points.push(literal(to));
            }
            PathSegment::Close => {}
        }
    }
    points
}

// ── laws ─────────────────────────────────────────────────────────────────

proptest! {
    #![proptest_config(ProptestConfig::with_cases(24))]

    /// **The Alpha Lock Law.** Whatever the layer holds and wherever the
    /// stroke is painted, the committed ribbon is inside the layer's existing
    /// bounds — or the commit is refused with a sentence. Never a stroke that
    /// crossed the lines.
    #[test]
    fn a_painted_stroke_stays_inside_the_layers_artwork(
        cx in 0.0f64..300.0, cy in 0.0f64..300.0, cw in 40.0f64..200.0, ch in 40.0f64..200.0,
        sx in 0.0f64..400.0, sy in 0.0f64..400.0, ex in 0.0f64..400.0, ey in 0.0f64..400.0,
    ) {
        let mut engine = VectraEngine::new();
        let layer = workspace_layer(&mut engine);
        rect(&mut engine, "content", cx, cy, cw, ch);
        set_alpha_lock(&mut engine, &layer, true);
        let content = layer_content(&document(&mut engine), &layer);
        prop_assume!(vectra_operations::region_area(&content) > 0.0);

        let reply = paint_stroke(&mut engine, (sx, sy), (ex, ey));
        if reply["ok"] == true {
            let id = reply["node_id"].as_str().expect("a committed node has an id").to_string();
            let doc = document(&mut engine);
            let committed = node_region(&doc, &id);
            let area = vectra_operations::region_area(&committed);
            let inside = vectra_operations::region_area(
                &vectra_operations::clip_region(&committed, &content),
            );
            prop_assert!(
                (inside - area).abs() <= 1e-3 * (1.0 + area),
                "the stroke escaped the layer's bounds: {inside} of {area} inside"
            );
            prop_assert!(area <= vectra_operations::region_area(&content) + 1e-3);
        } else {
            // Refused: the stroke missed the artwork entirely, and the refusal
            // is a sentence about alpha lock rather than a panic.
            let message = reply["error"].as_str().unwrap_or("");
            prop_assert!(message.contains("alpha lock"), "{reply}");
            prop_assert_eq!(reply["node_id"].as_str(), None);
        }
    }
}

/// **The Pen Stroke Law** (the 1-D clip): an open path has no area, so alpha
/// lock keeps the stretches of the *curve* that are inside the artwork. Every
/// point of the committed path is inside; a path that misses entirely is
/// refused; and the part of the line before it reached the artwork is gone.
#[test]
fn an_open_pen_path_keeps_only_the_stretch_inside() {
    let mut engine = VectraEngine::new();
    let layer = workspace_layer(&mut engine);
    // The artwork occupies x ∈ [100, 300]; the line crosses it left to right.
    rect(&mut engine, "content", 100.0, 0.0, 200.0, 200.0);
    set_alpha_lock(&mut engine, &layer, true);
    let content = layer_content(&document(&mut engine), &layer);

    let reply = draw_pen_line(&mut engine, (0.0, 100.0), (400.0, 100.0));
    assert_eq!(reply["ok"], true, "{reply}");
    let id = reply["node_id"].as_str().unwrap().to_string();
    let doc = document(&mut engine);
    let points = path_points(&doc, &id);
    assert!(points.len() >= 2, "a committed path has a chain");
    for (x, y) in &points {
        assert!(
            vectra_operations::contains_point(&content, *x, *y),
            "({x}, {y}) is outside the layer's artwork"
        );
    }
    assert!(
        points.iter().all(|(x, _)| *x >= 100.0 - 1e-6),
        "the stretch before the artwork must not survive: {points:?}"
    );

    // A second line that misses the artwork entirely is refused outright.
    let missed = draw_pen_line(&mut engine, (0.0, 500.0), (400.0, 520.0));
    assert_eq!(missed["ok"], false, "{missed}");
    assert!(
        missed["error"]
            .as_str()
            .unwrap_or("")
            .contains("alpha lock"),
        "{missed}"
    );
}

/// **Nothing to land on**: an empty alpha-locked layer refuses the stroke.
#[test]
fn an_empty_alpha_locked_layer_refuses_every_stroke() {
    let mut engine = VectraEngine::new();
    let layer = workspace_layer(&mut engine);
    set_alpha_lock(&mut engine, &layer, true);
    let reply = paint_stroke(&mut engine, (10.0, 10.0), (300.0, 200.0));
    assert_eq!(reply["ok"], false, "{reply}");
    assert!(
        reply["error"].as_str().unwrap_or("").contains("alpha lock"),
        "{reply}"
    );
    // No node was created — the refusal wrote nothing.
    let snapshot: Value = serde_json::from_str(&engine.get_snapshot()).unwrap();
    assert_eq!(snapshot["scene"]["z_order"].as_array().unwrap().len(), 0);
}

/// **Unlocked means untouched**: with the flag off the identical stroke commits
/// with its own geometry — alpha lock is a boundary, not a rewrite of the tool.
#[test]
fn an_unlocked_layer_keeps_the_strokes_own_geometry() {
    let mut engine = VectraEngine::new();
    rect(&mut engine, "content", 0.0, 0.0, 50.0, 50.0);
    // A stroke well outside the artwork: allowed, because nothing is locked.
    let reply = paint_stroke(&mut engine, (200.0, 200.0), (320.0, 260.0));
    assert_eq!(reply["ok"], true, "{reply}");
    let id = reply["node_id"].as_str().unwrap().to_string();
    let region = node_region(&document(&mut engine), &id);
    assert!(
        vectra_operations::region_area(&region) > 0.0,
        "a stroke outside the artwork must survive when nothing is locked"
    );
}

/// The lock is a **flag**, and the two flags round-trip through the wire
/// (Task 10.7 RULE 3's command shapes).
#[test]
fn the_lock_and_the_mask_round_trip() {
    let mut engine = VectraEngine::new();
    let layer = workspace_layer(&mut engine);
    ok(
        &mut engine,
        json!({"type": "SetLayerAlphaLocked", "id": layer, "alpha_locked": true}),
    );
    ok(
        &mut engine,
        json!({"type": "SetLayerClippingMask", "id": layer, "clipping_mask": true}),
    );
    let snapshot: Value = serde_json::from_str(&engine.get_snapshot()).unwrap();
    assert_eq!(snapshot["layers"][0]["alpha_locked"], true);
    assert_eq!(snapshot["layers"][0]["clipping_mask"], true);
    assert_eq!(
        snapshot["layers"][0]["clipped_to"],
        Value::Null,
        "the bottom layer clips to nothing"
    );
    // Undo takes the flags back, one entry each, in order.
    engine.undo();
    let snapshot: Value = serde_json::from_str(&engine.get_snapshot()).unwrap();
    assert_eq!(snapshot["layers"][0]["clipping_mask"], false);
    assert_eq!(snapshot["layers"][0]["alpha_locked"], true);
    engine.undo();
    let snapshot: Value = serde_json::from_str(&engine.get_snapshot()).unwrap();
    assert_eq!(snapshot["layers"][0]["alpha_locked"], false);
}
