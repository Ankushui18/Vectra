//! Task 5.0 laws, at the WASM boundary: **the canvas is a consumer**.
//!
//! `vectra-render`'s own suites prove the renderer's internals. These laws prove
//! the *composition* — that the engine, the ledger and the canvas agree once
//! they are wired together the way React wires them:
//!
//! ```text
//! dispatch_command ──▶ settle ──▶ DirtyLedger ──▶ render_frame ──▶ GPU plan
//!                                                     │
//!                              pointer_hit(client x, y) ──▶ NodeId
//! ```
//!
//! Nothing here needs a GPU or a DOM: `Renderer::present` runs the full
//! tessellate-and-diff path and simply skips the upload when the device is
//! absent, and the pointer path is plain Rust. That is deliberate — an
//! interactive renderer whose *decisions* can only be tested in a browser would
//! be a renderer whose decisions are never tested.
//!
//! * [`law_the_first_frame_builds_the_whole_scene_once`] — a cold canvas pays
//!   for everything, exactly once.
//! * [`law_a_mutation_uploads_only_what_changed`] — one node moves ⇒ one 80-byte
//!   instance write, through the real command bus.
//! * [`law_a_gesture_is_one_frame_and_one_write_per_node`] — 40 pointer samples,
//!   one drawn frame, one write per moved node. This is the drag fast path.
//! * [`law_a_full_rebuild_of_an_unchanged_scene_costs_nothing`] — "reconcile,
//!   never rebuild" survives the explicit full-evaluation control.
//! * [`law_pointer_hit_follows_the_engine`] — click → `NodeId` → `BeginDrag`: the
//!   whole RULE 3 chain, including that the index tracks edits.
//! * [`law_the_viewport_maps_screen_pixels_to_document_units`] — the canvas box,
//!   the device pixel ratio, and the y-up document convention.
//! * [`law_without_a_gpu_the_pointer_still_works`] — no WebGPU ⇒ no pixels, but
//!   no panic and no lying frame report either.
//! * [`law_the_ledger_coalesces_and_a_full_evaluation_wins`] — the ledger's own
//!   algebra.

use vectra_core::new_node_id;
use vectra_wasm::{DirtyLedger, Renderer, VectraEngine};

// ── Engine driving (the JSON wire the React remote uses) ───────────────────

fn dispatch(engine: &mut VectraEngine, cmd: serde_json::Value) -> serde_json::Value {
    serde_json::from_str(&engine.dispatch_command(&cmd.to_string())).expect("parsed response")
}

fn frame(engine: &mut VectraEngine, renderer: &mut Renderer) -> serde_json::Value {
    serde_json::from_str(&engine.render_frame(renderer)).expect("parsed frame")
}

fn number(frame: &serde_json::Value, key: &str) -> f64 {
    frame[key]
        .as_f64()
        .unwrap_or_else(|| panic!("frame.{key}: {frame}"))
}

/// A rectangle whose *evaluated* bounds are `(x, y) .. (x + w, y + h)` in
/// document units. Position is a literal, so a `SetParameter` moves it.
fn create_rect(engine: &mut VectraEngine, name: &str, x: f64, y: f64, w: f64, h: f64) -> String {
    let node = new_node_id().to_string();
    let response = dispatch(
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
    assert_eq!(response["status"], "ok", "create {name}: {response}");
    node
}

fn set_x(engine: &mut VectraEngine, node: &str, x: f64) -> serde_json::Value {
    dispatch(
        engine,
        serde_json::json!({
            "type": "SetParameter", "node_id": node, "property": "x",
            "value": { "Float": { "Literal": x } },
        }),
    )
}

/// A renderer attached to a canvas of `css_w × css_h` CSS pixels at `scale`.
fn renderer_at(css_w: f64, css_h: f64, scale: f64) -> Renderer {
    let mut renderer = Renderer::new();
    renderer.viewport(0.0, 0.0, css_w, css_h, scale);
    renderer
}

// ── L1: the cold frame ────────────────────────────────────────────────────

#[test]
fn law_the_first_frame_builds_the_whole_scene_once() {
    let mut engine = VectraEngine::new();
    let mut renderer = renderer_at(800.0, 600.0, 1.0);
    create_rect(&mut engine, "A", 10.0, 10.0, 100.0, 50.0);
    create_rect(&mut engine, "B", 300.0, 0.0, 20.0, 20.0);

    let first = frame(&mut engine, &mut renderer);
    assert_eq!(number(&first, "nodes"), 2.0, "{first}");
    assert!(
        first["full"].as_bool().unwrap(),
        "the first frame is a full sync"
    );
    assert_eq!(number(&first, "created"), 2.0, "{first}");
    assert_eq!(number(&first, "touched"), 2.0, "{first}");
    assert!(
        number(&first, "writes") >= 4.0,
        "vertices + indices per node: {first}"
    );
    // No GPU on the host, and the frame report says so rather than pretending.
    assert!(!first["ready"].as_bool().unwrap());
    assert!(first["error"].is_null(), "{first}");

    // Two *more* frames with nothing changed cost nothing at all.
    for _ in 0..2 {
        let idle = frame(&mut engine, &mut renderer);
        assert_eq!(number(&idle, "writes"), 0.0, "idle frame: {idle}");
        assert_eq!(number(&idle, "bytes"), 0.0, "idle frame: {idle}");
        assert_eq!(number(&idle, "dirty"), 0.0, "idle frame: {idle}");
    }
}

// ── L2: one mutation, one slice ───────────────────────────────────────────

#[test]
fn law_a_mutation_uploads_only_what_changed() {
    let mut engine = VectraEngine::new();
    let mut renderer = renderer_at(800.0, 600.0, 1.0);
    let a = create_rect(&mut engine, "A", 10.0, 10.0, 100.0, 50.0);
    let b = create_rect(&mut engine, "B", 300.0, 0.0, 20.0, 20.0);
    let _ = frame(&mut engine, &mut renderer);

    // Move A. The engine's dirty set names exactly one node…
    let response = set_x(&mut engine, &a, 200.0);
    assert_eq!(response["status"], "ok");
    let dirty = response["events"]
        .as_array()
        .expect("events")
        .iter()
        .find(|event| event["type"] == "Dirty")
        .expect("a Dirty event");
    assert_eq!(dirty["ids"].as_array().unwrap().len(), 1, "{dirty}");
    assert_eq!(dirty["ids"][0], serde_json::json!(a));
    assert_eq!(dirty["mode"], "incremental");

    // …and the frame pays for exactly one instance row: 80 bytes (Task 10.2
    // widened the row to carry the appearance layer's blend, ramp window and
    // gradient frame). Same shape,
    // new origin — the node-local frame means no vertex is re-uploaded.
    let moved = frame(&mut engine, &mut renderer);
    assert_eq!(number(&moved, "dirty"), 1.0, "{moved}");
    assert_eq!(number(&moved, "moved"), 1.0, "{moved}");
    assert_eq!(number(&moved, "retessellated"), 0.0, "{moved}");
    assert_eq!(number(&moved, "writes"), 1.0, "{moved}");
    assert_eq!(number(&moved, "bytes"), 80.0, "{moved}");
    assert_eq!(number(&moved, "nodes"), 2.0, "B is still there: {moved}");

    // A style-only change is the same cost, and B is never in the plan.
    let restyle = dispatch(
        &mut engine,
        serde_json::json!({
            "type": "SetStyle", "node_id": b,
            "property": "fill", "value": "#ff0000",
        }),
    );
    if restyle["status"] == "ok" {
        let styled = frame(&mut engine, &mut renderer);
        assert_eq!(number(&styled, "writes"), 1.0, "{styled}");
        assert_eq!(number(&styled, "bytes"), 80.0, "{styled}");
        assert_eq!(number(&styled, "touched"), 1.0, "{styled}");
    }
}

// ── L3: the drag fast path ────────────────────────────────────────────────

#[test]
fn law_a_gesture_is_one_frame_and_one_write_per_node() {
    let mut engine = VectraEngine::new();
    let mut renderer = renderer_at(800.0, 600.0, 1.0);
    let a = create_rect(&mut engine, "A", 10.0, 10.0, 100.0, 50.0);
    let _ = frame(&mut engine, &mut renderer);

    let begin = dispatch(
        &mut engine,
        serde_json::json!({ "type": "BeginDrag", "node_id": a }),
    );
    assert_eq!(begin["status"], "ok", "{begin}");

    // Forty pointer samples, no frame drawn in between: the ledger coalesces
    // them all into one reconciliation.
    for step in 1..=40 {
        let value = dispatch(
            &mut engine,
            serde_json::json!({
                "type": "UpdateDrag", "node_id": a,
                "x": 10.0 + step as f64, "y": 10.0 + step as f64 / 2.0,
            }),
        );
        assert_eq!(value["status"], "ok", "sample {step}: {value}");
    }
    let dragged = frame(&mut engine, &mut renderer);
    assert_eq!(number(&dragged, "dirty"), 1.0, "one node moved: {dragged}");
    assert_eq!(
        number(&dragged, "writes"),
        1.0,
        "one instance write, not forty: {dragged}"
    );
    assert_eq!(number(&dragged, "bytes"), 80.0, "{dragged}");

    let end = dispatch(
        &mut engine,
        serde_json::json!({ "type": "EndDrag", "node_id": a }),
    );
    assert_eq!(end["status"], "ok", "{end}");
    let settled = frame(&mut engine, &mut renderer);
    // EndDrag writes the final literals; if they equal the last sample's value
    // there is nothing left to draw — either way it is never a re-tessellation.
    assert!(number(&settled, "writes") <= 1.0, "{settled}");
    assert_eq!(number(&settled, "retessellated"), 0.0, "{settled}");
}

// ── L4: reconcile, never rebuild ──────────────────────────────────────────

#[test]
fn law_a_full_rebuild_of_an_unchanged_scene_costs_nothing() {
    let mut engine = VectraEngine::new();
    let mut renderer = renderer_at(800.0, 600.0, 1.0);
    create_rect(&mut engine, "A", 10.0, 10.0, 100.0, 50.0);
    create_rect(&mut engine, "B", 300.0, 0.0, 20.0, 20.0);
    let _ = frame(&mut engine, &mut renderer);

    // The full-evaluation control rebuilds the *engine's* cache from scratch…
    let rebuild = serde_json::from_str::<serde_json::Value>(&engine.force_full_evaluation())
        .expect("parsed force_full_evaluation");
    assert_eq!(rebuild["status"], "ok", "{rebuild}");

    // …and the canvas notices: the frame is a full *reconciliation*, which
    // (because nothing actually differs) touches no buffer at all.
    let after = frame(&mut engine, &mut renderer);
    assert!(after["full"].as_bool().unwrap(), "{after}");
    assert_eq!(
        number(&after, "writes"),
        0.0,
        "a rebuild must not re-upload: {after}"
    );
    assert_eq!(number(&after, "nodes"), 2.0, "{after}");
}

// ── L5: RULE 3 end to end ─────────────────────────────────────────────────

#[test]
fn law_pointer_hit_follows_the_engine() {
    let mut engine = VectraEngine::new();
    // A 4:3 canvas at 800×600, so client pixels and document units coincide.
    let mut renderer = renderer_at(800.0, 600.0, 1.0);
    let a = create_rect(&mut engine, "A", 100.0, 100.0, 50.0, 50.0);
    let b = create_rect(&mut engine, "B", 400.0, 300.0, 40.0, 40.0);
    let _ = frame(&mut engine, &mut renderer);

    // Document y is up; screen y is down. Node A spans y 100..150, i.e. screen
    // y 450..500 on a 600-tall canvas.
    assert_eq!(
        renderer.pointer_hit(125.0, 475.0),
        Some(a.clone()),
        "centre of A"
    );
    assert_eq!(
        renderer.pointer_hit(420.0, 280.0),
        Some(b.clone()),
        "centre of B"
    );
    assert_eq!(renderer.pointer_hit(700.0, 100.0), None, "empty space");
    assert_eq!(
        renderer.pointer_hit(125.0, 100.0),
        None,
        "A's corner in screen space"
    );

    // The pointer → BeginDrag chain: what React does with the answer.
    let hit = renderer.pointer_hit(125.0, 475.0).expect("a node");
    let begin = dispatch(
        &mut engine,
        serde_json::json!({ "type": "BeginDrag", "node_id": hit }),
    );
    assert_eq!(begin["status"], "ok", "{begin}");
    dispatch(
        &mut engine,
        serde_json::json!({ "type": "UpdateDrag", "node_id": hit, "x": 300.0, "y": 300.0 }),
    );
    let response = dispatch(
        &mut engine,
        serde_json::json!({ "type": "EndDrag", "node_id": hit }),
    );
    assert_eq!(response["status"], "ok", "{response}");
    let _ = frame(&mut engine, &mut renderer);

    // The index tracked the edit: the node is where the pointer put it, and the
    // place it used to be is now empty.
    assert_eq!(
        renderer.pointer_hit(325.0, 275.0),
        Some(a.clone()),
        "A's new centre"
    );
    assert_eq!(renderer.pointer_hit(125.0, 475.0), None, "A's old centre");
    assert_eq!(renderer.pointer_hit(420.0, 280.0), Some(b), "B never moved");

    // And the document coordinates the canvas reports for a client point agree.
    let doc = serde_json::from_str::<serde_json::Value>(&renderer.pointer_doc(325.0, 275.0))
        .expect("parsed pointer_doc");
    assert!((doc["x"].as_f64().unwrap() - 325.0).abs() < 0.5, "{doc}");
    assert!((doc["y"].as_f64().unwrap() - 325.0).abs() < 0.5, "{doc}");
}

// ── L6: the viewport ──────────────────────────────────────────────────────

#[test]
fn law_the_viewport_maps_screen_pixels_to_document_units() {
    // A canvas that is not the document's aspect ratio: "contain" keeps the
    // document window fully visible and undistorted, adding document around it.
    let renderer = renderer_at(1000.0, 1000.0, 1.0);
    let view = serde_json::from_str::<serde_json::Value>(&renderer.view()).expect("view");
    assert!(view["w"].as_f64().unwrap() >= 800.0, "{view}");
    assert!(view["h"].as_f64().unwrap() >= 600.0, "{view}");

    // The document's centre is the canvas's centre…
    let (x, y) = renderer.client_to_document(500.0, 500.0).expect("mapped");
    let (cx, cy) = (view["x"].as_f64().unwrap(), view["y"].as_f64().unwrap());
    assert!(
        (x - (cx + view["w"].as_f64().unwrap() / 2.0)).abs() < 0.5,
        "{x} vs {view}"
    );
    assert!(
        (y - (cy + view["h"].as_f64().unwrap() / 2.0)).abs() < 0.5,
        "{y} vs {view}"
    );
    // …and +y goes *up* the screen, because document space is y-up.
    let (_, lower) = renderer.client_to_document(500.0, 900.0).expect("mapped");
    let (_, upper) = renderer.client_to_document(500.0, 100.0).expect("mapped");
    assert!(
        lower < upper,
        "screen down is document down: {lower} vs {upper}"
    );

    // A device pixel ratio of 2 does not move the document mapping, only the
    // backing store: the same client point is the same document point.
    let hidpi = renderer_at(1000.0, 1000.0, 2.0);
    let (hx, hy) = hidpi.client_to_document(500.0, 500.0).expect("mapped");
    assert!(
        (hx - x).abs() < 1e-6 && (hy - y).abs() < 1e-6,
        "{hx},{hy} vs {x},{y}"
    );

    // The canvas box is honoured: a canvas offset by (100, 50) in the page
    // shifts every client coordinate by that much.
    let mut offset = renderer_at(1000.0, 1000.0, 1.0);
    offset.viewport(100.0, 50.0, 1000.0, 1000.0, 1.0);
    let (ox, oy) = offset.client_to_document(600.0, 550.0).expect("mapped");
    assert!(
        (ox - x).abs() < 1e-6 && (oy - y).abs() < 1e-6,
        "{ox},{oy} vs {x},{y}"
    );
}

#[test]
fn law_an_unmeasured_canvas_ignores_the_pointer() {
    // Before `measure` runs there is no box: the pointer is refused rather than
    // mapped through a guess.
    let mut renderer = Renderer::new();
    assert_eq!(renderer.client_to_document(10.0, 10.0), None);
    assert_eq!(renderer.pointer_hit(10.0, 10.0), None);
    assert_eq!(renderer.pointer_doc(10.0, 10.0), "null");
    // A zero-size box (hidden panel) is the same story.
    renderer.viewport(0.0, 0.0, 0.0, 0.0, 1.0);
    assert_eq!(renderer.client_to_document(10.0, 10.0), None);
    // A nonsense scale is clamped instead of producing NaN coordinates.
    renderer.viewport(0.0, 0.0, 800.0, 600.0, f64::NAN);
    let (x, y) = renderer.client_to_document(400.0, 300.0).expect("mapped");
    assert!(x.is_finite() && y.is_finite());
}

// ── L7: no GPU, no lies ───────────────────────────────────────────────────

#[test]
fn law_without_a_gpu_the_pointer_still_works() {
    // The host build has no device and never will: that is the point. The
    // spatial index is CPU-side state, so a browser without WebGPU still gets
    // an honest scene graph, and the frame report is honest about the pixels.
    let mut engine = VectraEngine::new();
    let mut renderer = renderer_at(800.0, 600.0, 1.0);
    let a = create_rect(&mut engine, "A", 0.0, 0.0, 100.0, 100.0);
    let drawn = frame(&mut engine, &mut renderer);

    assert!(!renderer.gpu_ready());
    assert!(!drawn["ready"].as_bool().unwrap(), "{drawn}");
    assert_eq!(
        renderer.pointer_hit(50.0, 550.0),
        Some(a),
        "y=550 screen is y=50 document"
    );
    let stats = serde_json::from_str::<serde_json::Value>(&renderer.stats()).expect("stats");
    assert_eq!(stats["ready"], serde_json::json!(false));
    assert_eq!(stats["nodes"], serde_json::json!(1));
    assert_eq!(stats["frames"], serde_json::json!(1));
}

// ── L8: the ledger's own algebra ──────────────────────────────────────────

#[test]
fn law_the_ledger_coalesces_and_a_full_evaluation_wins() {
    use vectra_core::EvalMode;

    let a = new_node_id();
    let b = new_node_id();
    let c = new_node_id();

    let mut ledger = DirtyLedger::new();
    assert!(ledger.is_empty());
    ledger.note([a], EvalMode::Incremental);
    ledger.note([a, b], EvalMode::Incremental);
    assert_eq!(ledger.len(), 2, "duplicates collapse");
    assert!(!ledger.is_empty());
    let dirty = ledger.take();
    assert!(!dirty.is_full());
    assert_eq!(dirty.len(), 2);
    assert!(dirty.contains(a) && dirty.contains(b) && !dirty.contains(c));
    assert!(ledger.is_empty(), "taking drains the ledger");
    assert!(!ledger.is_full(), "…including the full flag");

    // A full evaluation supersedes whatever was pending: individual ids would
    // be a partial truth about a whole-scene rebuild.
    ledger.note([a], EvalMode::Incremental);
    ledger.note([c], EvalMode::Full);
    assert!(ledger.is_full());
    assert_eq!(ledger.len(), 0);
    let dirty = ledger.take();
    assert!(dirty.is_full(), "a full rebuild is handed over as `all`");
    // …and after the full evaluation, incremental notes start a fresh set.
    ledger.note([a], EvalMode::Incremental);
    ledger.note([b], EvalMode::Incremental);
    assert_eq!(ledger.take().len(), 2);
}
