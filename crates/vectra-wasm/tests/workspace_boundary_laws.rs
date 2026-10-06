//! Task 10.2 laws at the WASM boundary: **the workspace is a document, and the
//! canvas obeys it.**
//!
//! `vectra-dependency`'s `workspace_laws.rs` proves the engine's side of the
//! four rules — an eye produces no dirty ids, a reorder re-derives the draw
//! order, a stack draws in order and blends. These laws prove the *wired*
//! version: the same claims made against the JSON the React remote actually
//! sends and receives, through the renderer the canvas actually drives.
//!
//! ```text
//! SetLayerVisible ──▶ dispatch_command ──▶ events (no dirty ids)
//!                                            │
//!                          get_snapshot() ───┤ the layer list survives every path
//!                                            │
//!                          render_frame() ───┴─▶ zero writes, fewer draw items
//! ```
//!
//! * [`law_an_eye_across_the_wire_is_a_flag_not_an_evaluation`] — RULE 4 as the
//!   designer experiences it: the eye works, the picture updates, and the frame
//!   that shows it writes *zero bytes*.
//! * [`law_the_workspace_survives_every_mutation_and_undo`] — the snapshot's
//!   layer/artboard projection is not a one-shot: it is there after a geometry
//!   edit, after a restyle, after a reorder, and after an undo.
//! * [`law_the_appearance_stack_reaches_the_canvas_in_order`] — RULE 3's whole
//!   point: a second stroke is a second draw item, and re-blending one layer of
//!   the stack costs one 80-byte instance row.
//! * [`law_artboards_frame_and_export_what_they_own`] — RULE 2: the dropdown's
//!   `frame_document`, the current/all exports, and the background colour.
//!
//! No GPU and no DOM: `Renderer::present` runs the full tessellate-and-diff
//! path and simply records that the device is absent.

use vectra_core::ids::{new_artboard_id, new_layer_id};
use vectra_core::new_node_id;
use vectra_wasm::{Renderer, VectraEngine};

// ── The JSON wire, exactly as `client.ts` drives it ────────────────────────

fn dispatch(engine: &mut VectraEngine, cmd: serde_json::Value) -> serde_json::Value {
    serde_json::from_str(&engine.dispatch_command(&cmd.to_string())).expect("parsed response")
}

/// Dispatch and insist on success — the remote's own `expectOk`.
fn ok(engine: &mut VectraEngine, cmd: serde_json::Value) -> serde_json::Value {
    let response = dispatch(engine, cmd);
    assert_eq!(response["status"], "ok", "{response}");
    response
}

fn snapshot(engine: &mut VectraEngine) -> serde_json::Value {
    serde_json::from_str(&engine.get_snapshot()).expect("parsed snapshot")
}

fn frame(engine: &mut VectraEngine, renderer: &mut Renderer) -> serde_json::Value {
    serde_json::from_str(&engine.render_frame(renderer)).expect("parsed frame")
}

fn number(value: &serde_json::Value, key: &str) -> f64 {
    value[key]
        .as_f64()
        .unwrap_or_else(|| panic!("{key}: {value}"))
}

fn renderer_at(css_w: f64, css_h: f64) -> Renderer {
    let mut renderer = Renderer::new();
    renderer.viewport(0.0, 0.0, css_w, css_h, 1.0);
    renderer
}

/// A rectangle with literal geometry, so `SetParameter` can move it.
fn create_rect(engine: &mut VectraEngine, name: &str, x: f64, y: f64, w: f64, h: f64) -> String {
    let node = new_node_id().to_string();
    ok(
        engine,
        serde_json::json!({
            "type": "CreateNode",
            "id": node,
            "name": name,
            "kind": { "Rectangle": {
                "x": { "Literal": x },
                "y": { "Literal": y },
                "width": { "Literal": w },
                "height": { "Literal": h },
                "corner_radius": { "Literal": 0.0 },
            }},
        }),
    );
    node
}

fn create_layer(engine: &mut VectraEngine, name: &str) -> String {
    let layer = new_layer_id().to_string();
    ok(
        engine,
        serde_json::json!({ "type": "CreateLayer", "id": layer, "name": name }),
    );
    layer
}

fn set_active_layer(engine: &mut VectraEngine, layer: &str) {
    ok(
        engine,
        serde_json::json!({ "type": "SetActiveLayer", "id": layer }),
    );
}

fn create_artboard(
    engine: &mut VectraEngine,
    name: &str,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    background: &str,
) -> String {
    let board = new_artboard_id().to_string();
    let color = vectra_core::geom::Color::from_hex(background).expect("a hex colour");
    ok(
        engine,
        serde_json::json!({
            "type": "CreateArtboard",
            "id": board, "name": name,
            "x": x, "y": y, "width": w, "height": h,
            "background": { "r": color.r, "g": color.g, "b": color.b, "a": color.a },
        }),
    );
    board
}

/// The canvas as the host drives it: one renderer, opaque to these laws except
/// through its frame reports.
struct Canvas {
    renderer: Renderer,
}

impl Canvas {
    fn new() -> Self {
        Self {
            renderer: renderer_at(800.0, 600.0),
        }
    }

    fn frame(&mut self, engine: &mut VectraEngine) -> serde_json::Value {
        frame(engine, &mut self.renderer)
    }
}

/// A solid fill row for `SetAppearances`, in the document's own shape.
fn fill(r: u8, g: u8, b: u8, blend: &str) -> serde_json::Value {
    serde_json::json!({
        "kind": "Fill",
        "paint": { "Solid": { "Literal": { "r": r, "g": g, "b": b, "a": 255 } } },
        "opacity": { "Literal": 1.0 },
        "blend": blend,
        "visible": true,
    })
}

/// A stroke row — the reason RULE 3 exists (thick black under thin white).
fn stroke(r: u8, g: u8, b: u8, width: f64, blend: &str) -> serde_json::Value {
    serde_json::json!({
        "kind": { "Stroke": { "width": { "Literal": width } } },
        "paint": { "Solid": { "Literal": { "r": r, "g": g, "b": b, "a": 255 } } },
        "opacity": { "Literal": 1.0 },
        "blend": blend,
        "visible": true,
    })
}

// ── RULE 4 ────────────────────────────────────────────────────────────────

/// Hiding a layer is one `bool` per node: the evaluator is never consulted, and
/// the frame that shows the change writes nothing at all.
#[test]
fn law_an_eye_across_the_wire_is_a_flag_not_an_evaluation() {
    let mut engine = VectraEngine::new();
    let mut canvas = Canvas::new();

    // A new document is already a workspace: one artboard, one layer, and the
    // first shape the designer draws lands on it (RULE 1 without a special case
    // in `insert_node`).
    let opening = snapshot(&mut engine);
    assert_eq!(opening["layers"].as_array().unwrap().len(), 1, "{opening}");
    assert_eq!(
        opening["layers"][0]["name"],
        serde_json::json!("Layer 1"),
        "{opening}"
    );
    assert_eq!(
        opening["artboards"].as_array().unwrap().len(),
        1,
        "{opening}"
    );
    assert_eq!(
        opening["artboards"][0]["bounds"],
        serde_json::json!([0.0, 0.0, 800.0, 600.0]),
        "{opening}"
    );
    let layer_one = opening["layers"][0]["id"].as_str().unwrap().to_string();
    assert_eq!(opening["active_layer"], serde_json::json!(layer_one));

    // Two layers, two nodes each. The second layer is the one we will hide.
    let a = create_rect(&mut engine, "A", 0.0, 0.0, 40.0, 40.0);
    assert_eq!(
        snapshot(&mut engine)["scene"]["nodes"][&a]["layer"],
        serde_json::json!(layer_one),
        "the first node belongs to a layer"
    );
    let b = create_rect(&mut engine, "B", 60.0, 0.0, 40.0, 40.0);
    let guides = create_layer(&mut engine, "Guides");
    set_active_layer(&mut engine, &guides);
    let c = create_rect(&mut engine, "C", 0.0, 60.0, 40.0, 40.0);
    let d = create_rect(&mut engine, "D", 60.0, 60.0, 40.0, 40.0);

    // The first frame draws one item per node.
    let first = canvas.frame(&mut engine);
    assert_eq!(number(&first, "draw_calls"), 4.0, "{first}");
    assert_eq!(number(&first, "created"), 4.0, "{first}");

    // ── The toggle ────────────────────────────────────────────────────────
    let response = ok(
        &mut engine,
        serde_json::json!({ "type": "SetLayerVisible", "id": guides, "visible": false }),
    );
    let events = response["events"].as_array().expect("events");
    assert!(
        events.iter().any(|event| event["type"] == "LayersUpdated"),
        "the panel hears about the layer: {response}"
    );
    let dirty = events
        .iter()
        .find(|event| event["type"] == "Dirty")
        .unwrap_or_else(|| panic!("a Dirty event: {response}"));
    assert_eq!(
        dirty["ids"].as_array().unwrap().len(),
        0,
        "an eye is not a value: {dirty}"
    );

    // The document still holds both nodes, and the layer still lists both.
    let snap = snapshot(&mut engine);
    for node in [&c, &d] {
        let entry = &snap["scene"]["nodes"][node];
        assert_eq!(entry["own_visible"], serde_json::json!(true), "{node}");
        assert_eq!(
            entry["visible"],
            serde_json::json!(false),
            "the layer's eye reaches the node: {entry}"
        );
        assert_eq!(entry["layer"], serde_json::json!(guides), "{entry}");
        assert_eq!(entry["layer_name"], serde_json::json!("Guides"), "{entry}");
    }
    let listed = snap["layers"]
        .as_array()
        .expect("layers")
        .iter()
        .find(|layer| layer["id"] == serde_json::json!(guides))
        .unwrap_or_else(|| panic!("the Guides layer: {snap}"));
    assert_eq!(
        listed["children"].as_array().unwrap().len(),
        2,
        "hiding a layer does not empty it: {listed}"
    );
    assert_eq!(listed["visible"], serde_json::json!(false));
    assert_eq!(snap["active_layer"], serde_json::json!(guides));

    // ── The frame the designer sees ───────────────────────────────────────
    let hidden = canvas.frame(&mut engine);
    assert_eq!(
        number(&hidden, "dirty"),
        2.0,
        "the two hidden nodes repaint: {hidden}"
    );
    assert_eq!(
        number(&hidden, "writes"),
        0.0,
        "an eye writes no bytes: {hidden}"
    );
    assert_eq!(number(&hidden, "bytes"), 0.0, "{hidden}");
    assert_eq!(number(&hidden, "retessellated"), 0.0, "{hidden}");
    assert_eq!(number(&hidden, "moved"), 0.0, "{hidden}");
    assert_eq!(number(&hidden, "restyled"), 0.0, "{hidden}");
    assert_eq!(
        number(&hidden, "removed"),
        0.0,
        "the buffers stay: {hidden}"
    );
    assert_eq!(
        number(&hidden, "nodes"),
        4.0,
        "nothing is released: {hidden}"
    );
    assert_eq!(
        number(&hidden, "draw_calls"),
        2.0,
        "a hidden layer contributes no draw item: {hidden}"
    );

    // Showing it again is the same story in reverse *minus* the rows it released
    // while hidden: the meshes were kept (no retessellation, no buffer rebuild),
    // and each drawn layer needs its instance row back — two nodes × one fill =
    // 2 writes of 80 bytes. That asymmetry is the whole renderer-side cost of
    // RULE 4, and it is stated here rather than hidden.
    ok(
        &mut engine,
        serde_json::json!({ "type": "SetLayerVisible", "id": guides, "visible": true }),
    );
    let shown = canvas.frame(&mut engine);
    assert_eq!(number(&shown, "writes"), 2.0, "{shown}");
    assert_eq!(number(&shown, "bytes"), 160.0, "{shown}");
    assert_eq!(
        number(&shown, "created"),
        0.0,
        "no buffer is rebuilt: {shown}"
    );
    assert_eq!(
        number(&shown, "retessellated"),
        0.0,
        "the shapes never moved: {shown}"
    );
    assert_eq!(number(&shown, "draw_calls"), 4.0, "{shown}");
    // …and an idle frame after it is silent again.
    let idle = canvas.frame(&mut engine);
    assert_eq!(number(&idle, "writes"), 0.0, "{idle}");
    assert_eq!(number(&idle, "bytes"), 0.0, "{idle}");

    // A padlock is the same kind of flag, and it leaves the picture alone: the
    // locked layer still draws, it just stops answering the pointer (which
    // `vectra-render`'s hit laws prove).
    ok(
        &mut engine,
        serde_json::json!({ "type": "SetLayerLocked", "id": guides, "locked": true }),
    );
    let locked = canvas.frame(&mut engine);
    assert_eq!(
        number(&locked, "writes"),
        0.0,
        "a padlock costs nothing at all: {locked}"
    );
    assert_eq!(number(&locked, "draw_calls"), 4.0, "{locked}");
    let snap = snapshot(&mut engine);
    assert_eq!(
        snap["scene"]["nodes"][&c]["locked"],
        serde_json::json!(true)
    );
    assert_eq!(
        snap["scene"]["nodes"][&c]["visible"],
        serde_json::json!(true)
    );
    assert_eq!(
        snap["scene"]["nodes"][&a]["locked"],
        serde_json::json!(false)
    );
    let _ = (a, b);
}

/// **An eye never hides computed geometry.** A virtual node — an operation's
/// result, a procedural result, a `Source` — is a *scene* node with no entry in
/// `doc.nodes`, so `Document::visible` answers `false` for it. Re-deriving the
/// scene's flags from the document without asking who owns each node therefore
/// hides every computed shape on the next settle (a real defect this law now
/// pins: the smoke `patch ≡ rebuild` check caught it). The flags of a virtual
/// node belong to the pass that composed it — a disabled operation is *pruned*,
/// never silently hidden.
#[test]
fn law_a_computed_shape_is_not_hidden_by_a_neighbours_eye() {
    let mut engine = VectraEngine::new();
    let mut canvas = Canvas::new();

    let plate = create_rect(&mut engine, "Plate", 0.0, 0.0, 100.0, 100.0);
    let hole = create_rect(&mut engine, "Hole", 20.0, 20.0, 40.0, 40.0);
    let result = new_node_id().to_string();
    ok(
        &mut engine,
        serde_json::json!({
            "type": "ApplyOperation",
            "id": result,
            "kind": { "type": "boolean", "op": "subtract" },
            "inputs": [plate, hole],
        }),
    );

    let snap = snapshot(&mut engine);
    assert_eq!(
        snap["scene"]["nodes"][&result]["primitive"]["type"],
        serde_json::json!("path"),
        "the operation is a virtual scene node: {snap}"
    );
    assert_eq!(
        snap["scene"]["nodes"][&result]["visible"],
        serde_json::json!(true)
    );
    assert_eq!(
        snap["operations"][&result]["enabled"],
        serde_json::json!(true),
        "{snap}"
    );
    let with_operation = canvas.frame(&mut engine);
    assert_eq!(
        number(&with_operation, "draw_calls"),
        3.0,
        "{with_operation}"
    );

    // Now hide the layer the *sources* live on. Both sources go; the computed
    // shape stays — it is the operation's own geometry, and the operation is
    // still enabled.
    let layer = snapshot(&mut engine)["layers"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    ok(
        &mut engine,
        serde_json::json!({ "type": "SetLayerVisible", "id": layer, "visible": false }),
    );
    let snap = snapshot(&mut engine);
    for node in [&plate, &hole] {
        assert_eq!(
            snap["scene"]["nodes"][node]["visible"],
            serde_json::json!(false),
            "{node} is on the hidden layer: {snap}"
        );
    }
    assert_eq!(
        snap["scene"]["nodes"][&result]["visible"],
        serde_json::json!(true),
        "computed geometry is not the document's to hide: {snap}"
    );
    assert!(
        snap["scene"]["z_order"]
            .as_array()
            .unwrap()
            .contains(&serde_json::json!(result)),
        "and it is still in the draw order: {snap}"
    );
    let hidden = canvas.frame(&mut engine);
    assert_eq!(
        number(&hidden, "draw_calls"),
        1.0,
        "only the computed shape is drawn: {hidden}"
    );

    // Parking the operation is the way to withdraw it — and that is a *prune*,
    // which is a different act from an eye.
    ok(
        &mut engine,
        serde_json::json!({ "type": "SetOperationEnabled", "id": result, "enabled": false }),
    );
    let parked = snapshot(&mut engine);
    assert!(
        parked["scene"]["nodes"][&result].is_null(),
        "a parked operation leaves the scene entirely: {parked}"
    );
}

// ── The layer list is not a one-shot projection ───────────────────────────

/// Every mutation path re-projects the workspace: geometry edits, restyles,
/// reorders and undo all leave `snapshot.layers` intact and accurate.
#[test]
fn law_the_workspace_survives_every_mutation_and_undo() {
    let mut engine = VectraEngine::new();
    let a = create_rect(&mut engine, "A", 0.0, 0.0, 40.0, 40.0);
    let art = create_layer(&mut engine, "Art");
    set_active_layer(&mut engine, &art);
    let _b = create_rect(&mut engine, "B", 90.0, 0.0, 20.0, 20.0);

    let layers = |snap: &serde_json::Value| -> Vec<(String, usize)> {
        snap["layers"]
            .as_array()
            .expect("layers")
            .iter()
            .map(|layer| {
                (
                    layer["name"].as_str().unwrap_or_default().to_string(),
                    layer["children"].as_array().map(Vec::len).unwrap_or(0),
                )
            })
            .collect()
    };

    let snap = snapshot(&mut engine);
    assert_eq!(
        layers(&snap),
        vec![("Layer 1".to_string(), 1), ("Art".to_string(), 1)],
        "back → front: the seeded layer, then the one that was created: {snap}"
    );
    assert_eq!(snap["active_layer"], serde_json::json!(art));
    assert_eq!(
        snap["scene"]["nodes"][&a]["layer_name"],
        serde_json::json!("Layer 1"),
        "{snap}"
    );

    // A geometry edit.
    ok(
        &mut engine,
        serde_json::json!({
            "type": "SetParameter", "node_id": a, "property": "x",
            "value": { "Float": { "Literal": 12.0 } },
        }),
    );
    let snap = snapshot(&mut engine);
    assert_eq!(
        layers(&snap),
        vec![("Layer 1".to_string(), 1), ("Art".to_string(), 1)]
    );

    // A restyle that replaces the appearance stack.
    ok(
        &mut engine,
        serde_json::json!({
            "type": "SetAppearances", "node_id": a, "appearances": [fill(200, 30, 30, "Normal")],
        }),
    );
    let snap = snapshot(&mut engine);
    assert_eq!(
        layers(&snap),
        vec![("Layer 1".to_string(), 1), ("Art".to_string(), 1)]
    );

    // A reorder: the layer list *is* the draw order, so the names swap places.
    ok(
        &mut engine,
        serde_json::json!({ "type": "ReorderLayer", "id": art, "index": 0 }),
    );
    let snap = snapshot(&mut engine);
    assert_eq!(
        layers(&snap),
        vec![("Art".to_string(), 1), ("Layer 1".to_string(), 1)],
        "moving a layer moves its nodes: {snap}"
    );
    assert_eq!(snap["active_layer"], serde_json::json!(art));

    // Undo the reorder: the layer stack and the node bindings come back with it.
    let undone = serde_json::from_str::<serde_json::Value>(&engine.undo()).expect("undo");
    assert_eq!(undone["status"], "ok", "{undone}");
    let snap = snapshot(&mut engine);
    assert_eq!(
        layers(&snap),
        vec![("Layer 1".to_string(), 1), ("Art".to_string(), 1)],
        "undo restores the stack: {snap}"
    );
    assert_eq!(
        snap["scene"]["nodes"][&_b]["layer"],
        serde_json::json!(art),
        "and the node that lives on it: {snap}"
    );
}

// ── RULE 3: the stack, in order, at the boundary ──────────────────────────

/// A node with two strokes and a fill is three draw items; re-blending one of
/// them is one instance row, and a gradient is a ramp.
#[test]
fn law_the_appearance_stack_reaches_the_canvas_in_order() {
    let mut engine = VectraEngine::new();
    let mut canvas = Canvas::new();
    let node = create_rect(&mut engine, "Mark", 0.0, 0.0, 100.0, 60.0);
    let first = canvas.frame(&mut engine);
    assert_eq!(number(&first, "draw_calls"), 1.0, "one fill: {first}");

    // Thick black stroke, thin white stroke on top, fill underneath — the
    // ordering the engine draws in is the list's order.
    ok(
        &mut engine,
        serde_json::json!({
            "type": "SetAppearances", "node_id": node, "appearances": [
                fill(34, 102, 238, "Normal"),
                stroke(0, 0, 0, 6.0, "Normal"),
                stroke(255, 255, 255, 2.0, "Normal"),
            ],
        }),
    );

    let snap = snapshot(&mut engine);
    let rows = snap["scene"]["nodes"][&node]["style"]["appearances"]
        .as_array()
        .unwrap_or_else(|| panic!("appearances: {snap}"))
        .clone();
    assert_eq!(rows.len(), 3, "{snap}");
    let kinds: Vec<String> = rows
        .iter()
        .map(|row| row["kind"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(kinds, ["fill", "stroke", "stroke"], "{rows:?}");
    let widths: Vec<Option<f64>> = rows.iter().map(|row| row["width"].as_f64()).collect();
    assert_eq!(widths, [None, Some(6.0), Some(2.0)], "{rows:?}");
    assert_eq!(rows[2]["blend"], serde_json::json!("normal"));

    let stacked = canvas.frame(&mut engine);
    assert_eq!(
        number(&stacked, "draw_calls"),
        3.0,
        "three appearance layers, three draw items: {stacked}"
    );

    // Re-blending the top stroke is a style change on one row of one node: 80
    // bytes, no retessellation (the outline does not move).
    ok(
        &mut engine,
        serde_json::json!({
            "type": "SetAppearances", "node_id": node, "appearances": [
                fill(34, 102, 238, "Normal"),
                stroke(0, 0, 0, 6.0, "Normal"),
                stroke(255, 255, 255, 2.0, "Multiply"),
            ],
        }),
    );
    let blended = canvas.frame(&mut engine);
    assert_eq!(number(&blended, "bytes"), 80.0, "{blended}");
    assert_eq!(number(&blended, "writes"), 1.0, "{blended}");
    assert_eq!(number(&blended, "retessellated"), 0.0, "{blended}");
    assert_eq!(number(&blended, "draw_calls"), 3.0, "{blended}");
    let snap = snapshot(&mut engine);
    assert_eq!(
        snap["scene"]["nodes"][&node]["style"]["appearances"][2]["blend"],
        serde_json::json!("multiply"),
        "{snap}"
    );

    // A gradient fill: the same node, two stops, and the ramp reaches the
    // renderer (the atlas rows are what a ramp write is).
    ok(
        &mut engine,
        serde_json::json!({
            "type": "SetAppearances", "node_id": node, "appearances": [
                { "kind": "Fill",
                  "paint": { "Linear": {
                      "start": { "Literal": { "x": 0.0, "y": 0.0 } },
                      "end": { "Literal": { "x": 100.0, "y": 0.0 } },
                      "stops": [
                          { "offset": 0.0, "color": { "r": 255, "g": 210, "b": 0, "a": 255 } },
                          { "offset": 0.5, "color": { "r": 240, "g": 90, "b": 20, "a": 255 } },
                          { "offset": 1.0, "color": { "r": 120, "g": 10, "b": 60, "a": 255 } },
                      ],
                  }},
                  "opacity": { "Literal": 1.0 }, "blend": "Normal", "visible": true },
                stroke(0, 0, 0, 6.0, "Normal"),
                stroke(255, 255, 255, 2.0, "Multiply"),
            ],
        }),
    );
    let snap = snapshot(&mut engine);
    let paint = &snap["scene"]["nodes"][&node]["style"]["appearances"][0]["paint"];
    assert_eq!(paint["type"], serde_json::json!("linear"), "{paint}");
    assert_eq!(paint["stops"].as_array().unwrap().len(), 3, "{paint}");
    assert_eq!(
        paint["stops"][1]["offset"],
        serde_json::json!(0.5),
        "{paint}"
    );
    let gradient = canvas.frame(&mut engine);
    assert!(
        number(&gradient, "ramps") > 0.0,
        "a new gradient uploads its ramp: {gradient}"
    );
    assert_eq!(number(&gradient, "draw_calls"), 3.0, "{gradient}");

    // The frame after it is silent: the atlas is in place, the rows already say
    // which ramp window to sample.
    let settled = canvas.frame(&mut engine);
    assert_eq!(number(&settled, "ramps"), 0.0, "{settled}");
    assert_eq!(number(&settled, "writes"), 0.0, "{settled}");

    // Dragging a stop is a *structure* change (the list changes shape), so it
    // travels as one `SetAppearances`, and the ramp is re-uploaded — while the
    // instance rows are not touched, because the paint's frame did not move.
    ok(
        &mut engine,
        serde_json::json!({
            "type": "SetAppearances", "node_id": node, "appearances": [
                { "kind": "Fill",
                  "paint": { "Linear": {
                      "start": { "Literal": { "x": 0.0, "y": 0.0 } },
                      "end": { "Literal": { "x": 100.0, "y": 0.0 } },
                      "stops": [
                          { "offset": 0.0, "color": { "r": 255, "g": 210, "b": 0, "a": 255 } },
                          { "offset": 0.85, "color": { "r": 240, "g": 90, "b": 20, "a": 255 } },
                          { "offset": 1.0, "color": { "r": 120, "g": 10, "b": 60, "a": 255 } },
                      ],
                  }},
                  "opacity": { "Literal": 1.0 }, "blend": "Normal", "visible": true },
                stroke(0, 0, 0, 6.0, "Normal"),
                stroke(255, 255, 255, 2.0, "Multiply"),
            ],
        }),
    );
    let dragged = canvas.frame(&mut engine);
    assert!(number(&dragged, "ramps") > 0.0, "{dragged}");
    assert_eq!(
        number(&dragged, "writes"),
        0.0,
        "a stop moves, a shape does not: {dragged}"
    );
    let snap = snapshot(&mut engine);
    assert_eq!(
        snap["scene"]["nodes"][&node]["style"]["appearances"][0]["paint"]["stops"][1]["offset"],
        serde_json::json!(0.85),
        "{snap}"
    );
}

// ── RULE 2: artboards, framing and export ─────────────────────────────────

/// Two boards, two frames, two exports: the dropdown's jump frames the canvas
/// on the board, and the exports carry exactly the nodes each board owns.
#[test]
fn law_artboards_frame_and_export_what_they_own() {
    let mut engine = VectraEngine::new();
    let mut canvas = Canvas::new();

    // "Icon 16" — a tiny board with one node on it.
    let icon = create_artboard(&mut engine, "Icon 16", 0.0, 0.0, 16.0, 16.0, "#ffffff");
    let layer = create_layer(&mut engine, "Icon");
    set_active_layer(&mut engine, &layer);
    let dot = create_rect(&mut engine, "Dot", 2.0, 2.0, 12.0, 12.0);

    // "Logo" — a 320×200 board with a different background and its own node.
    let logo = create_artboard(&mut engine, "Logo", 400.0, 0.0, 320.0, 200.0, "#101820");
    let ink = create_layer(&mut engine, "Logo ink");
    set_active_layer(&mut engine, &ink);
    let mark = create_rect(&mut engine, "Mark", 440.0, 40.0, 240.0, 120.0);
    ok(
        &mut engine,
        serde_json::json!({ "type": "SetActiveArtboard", "id": logo }),
    );

    let snap = snapshot(&mut engine);
    let board = |name: &str| -> serde_json::Value {
        snap["artboards"]
            .as_array()
            .expect("artboards")
            .iter()
            .find(|candidate| candidate["name"] == serde_json::json!(name))
            .unwrap_or_else(|| panic!("the {name} board: {snap}"))
            .clone()
    };
    assert_eq!(snap["artboards"].as_array().unwrap().len(), 3, "{snap}");
    // The paper the document opened with is still there, empty and framed.
    assert_eq!(
        board("Artboard 1")["bounds"],
        serde_json::json!([0.0, 0.0, 800.0, 600.0]),
        "{snap}"
    );
    assert_eq!(
        board("Icon 16")["bounds"],
        serde_json::json!([0.0, 0.0, 16.0, 16.0])
    );
    assert_eq!(board("Icon 16")["background"], serde_json::json!("#ffffff"));
    assert_eq!(
        board("Icon 16")["layers"],
        serde_json::json!(1),
        "the layer created while standing on it: {snap}"
    );
    assert_eq!(
        board("Logo")["bounds"],
        serde_json::json!([400.0, 0.0, 320.0, 200.0])
    );
    assert_eq!(board("Logo")["background"], serde_json::json!("#101820"));
    // Creating a board puts the designer on it (a `CreateArtboard` is a move),
    // and the layer stacks followed: each board lists exactly its own layer.
    assert_eq!(snap["active_artboard"], serde_json::json!(logo));
    let artboard_of = |layer: &str| -> String {
        snap["layers"]
            .as_array()
            .expect("layers")
            .iter()
            .find(|candidate| candidate["name"] == serde_json::json!(layer))
            .unwrap_or_else(|| panic!("the {layer} layer: {snap}"))["artboard"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    };
    assert_eq!(
        artboard_of("Icon"),
        icon,
        "the Icon layer belongs to its board"
    );
    assert_eq!(artboard_of("Logo ink"), logo);
    assert_eq!(
        artboard_of("Layer 1"),
        board("Artboard 1")["id"].as_str().unwrap()
    );

    // The canvas is on the logo; the dot on the other board is still drawn (it
    // is the same document, the board is a frame) — two items.
    let frame_all = canvas.frame(&mut engine);
    assert_eq!(number(&frame_all, "draw_calls"), 2.0, "{frame_all}");

    // **The jump.** `frame_document` frames the board the designer picked: a
    // 320×200 board on an 800×600 canvas fits by width ×2.5.
    let view: serde_json::Value =
        serde_json::from_str(&canvas.renderer.frame_document(400.0, 0.0, 320.0, 200.0))
            .expect("view");
    assert!(
        (view["w"].as_f64().unwrap_or_default() - 320.0).abs() < 1.0,
        "{view}"
    );
    assert!(
        (view["h"].as_f64().unwrap_or_default() - 240.0).abs() < 1.0,
        "4:3 canvas, 320:200 board ⇒ the board fits by width: {view}"
    );
    assert!(
        view["x"].as_f64().unwrap_or_default() <= 400.0
            && view["x"].as_f64().unwrap_or_default() + view["w"].as_f64().unwrap_or_default()
                >= 720.0,
        "the frame contains the board: {view}"
    );

    // The zoom is real: the same jump on a 16×16 board is a different scale.
    let close: serde_json::Value =
        serde_json::from_str(&canvas.renderer.frame_document(0.0, 0.0, 16.0, 16.0)).expect("view");
    assert!(
        close["w"].as_f64().unwrap_or_default() < view["w"].as_f64().unwrap_or_default(),
        "framing the small board zooms in: {close} vs {view}"
    );
    // Back to the whole picture.
    let wide: serde_json::Value =
        serde_json::from_str(&canvas.renderer.frame_document_default()).expect("view");
    assert!(
        wide["w"].as_f64().unwrap_or_default() > view["w"].as_f64().unwrap_or_default(),
        "the default view is wider than a framed board: {wide}"
    );

    // **Export current**: the active board only, with its own frame and
    // background. The other board's node is not in the file.
    let current: serde_json::Value =
        serde_json::from_str(&engine.export_current_artboard()).expect("export");
    assert_eq!(current["status"], serde_json::json!("ok"), "{current}");
    let code = current["code"].as_str().expect("code");
    assert!(
        code.contains(&format!("data-vectra-artboard=\"{logo}\"")),
        "the active board's frame: {code}"
    );
    assert!(!code.contains(&dot), "the other board's node: {code}");
    assert!(code.contains(&mark), "its own node: {code}");
    assert!(
        code.contains("#101820") || code.contains("16,24,32"),
        "its background: {code}"
    );
    assert!(
        code.contains("width=\"320\"") && code.contains("height=\"200\""),
        "framed to the board, not to the ink: {code}"
    );

    // **Export all**: both boards, each with its own frame group.
    let all: serde_json::Value =
        serde_json::from_str(&engine.export_all_artboards()).expect("export");
    let code = all["code"].as_str().expect("code");
    assert!(
        code.contains(&format!("data-vectra-artboard=\"{logo}\"")),
        "{code}"
    );
    assert!(
        code.contains(&format!("data-vectra-artboard=\"{icon}\"")),
        "{code}"
    );
    assert!(code.contains(&dot) && code.contains(&mark), "{code}");
}
