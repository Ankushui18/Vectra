//! Operation laws at the **protocol boundary** (Task 4.0 §4 and §5) — the
//! `vectra-wasm` half of the operations contract.
//!
//! `crates/vectra-operations/tests/operation_laws.rs` proves the arithmetic and
//! the non-destructiveness at the engine level. This file proves what the UI
//! actually consumes:
//!
//! * the wire shape (`ApplyOperation` parses, snapshots carry `operations`, the
//!   virtual node appears in `scene.z_order` as a `path` primitive),
//! * dirty propagation as an *observable* (`Dirty` names the operation id when a
//!   source moves — the Requirement-3 claim, on the wire),
//! * the undo law through the command protocol (`revert` removes the operation
//!   and leaves the sources byte-identical),
//! * typed rejection of ill-formed operations (arity, unknown input, unknown
//!   operation on remove/enable).

use serde_json::{json, Value};
use vectra_core::new_node_id;
use vectra_wasm::VectraEngine;

/// Ids are minted by the engine's own generator (the same one the UI's
/// `crypto.randomUUID()` stands in for), so the tests never depend on a crate
/// that is not in this target's dependency graph.
fn id() -> String {
    new_node_id().to_string()
}

fn dispatch(engine: &mut VectraEngine, cmd: Value) -> Value {
    serde_json::from_str(&engine.dispatch_command(&serde_json::to_string(&cmd).unwrap())).unwrap()
}

fn snapshot(engine: &mut VectraEngine) -> Value {
    serde_json::from_str(&engine.get_snapshot()).unwrap()
}

fn undo(engine: &mut VectraEngine) -> Value {
    serde_json::from_str(&engine.undo()).unwrap()
}

fn create_rect(engine: &mut VectraEngine, name: &str, x: f64, y: f64, w: f64, h: f64) -> String {
    let node = id();
    let response = dispatch(
        engine,
        json!({
            "type": "CreateNode",
            "id": node,
            "name": name,
            "kind": {
                "Rectangle": {
                    "x": {"Literal": x}, "y": {"Literal": y},
                    "width": {"Literal": w}, "height": {"Literal": h},
                    "corner_radius": {"Literal": 0.0},
                }
            }
        }),
    );
    assert_eq!(response["status"], "ok", "{response}");
    node
}

fn apply(engine: &mut VectraEngine, kind: Value, inputs: Vec<String>) -> (String, Value) {
    let operation = id();
    let response = dispatch(
        engine,
        json!({"type": "ApplyOperation", "id": operation, "kind": kind, "inputs": inputs}),
    );
    (operation, response)
}

fn scene_node<'a>(view: &'a Value, id: &str) -> &'a Value {
    &view["scene"]["nodes"][id]
}

/// The x coordinates of an `M…L…Z` path's vertices (the snapshot's `d` data).
/// Small enough to inline here, and it lets the modifier laws assert on real
/// coordinate values instead of guessing from the string's shape.
fn x_values(d: &str) -> Vec<f64> {
    let mut out = Vec::new();
    for chunk in d.split(['M', 'L', 'Z']) {
        let numbers: Vec<f64> = chunk
            .split_whitespace()
            .filter_map(|token| token.parse().ok())
            .collect();
        for pair in numbers.chunks(2) {
            if let Some(x) = pair.first() {
                out.push(*x);
            }
        }
    }
    out
}

fn dirty_ids(response: &Value) -> Vec<String> {
    response["events"]
        .as_array()
        .expect("events")
        .iter()
        .filter(|event| event["type"] == "Dirty")
        .flat_map(|event| event["ids"].as_array().cloned().unwrap_or_default())
        .map(|id| id.as_str().unwrap().to_string())
        .collect()
}

fn create_circle(engine: &mut VectraEngine, name: &str, cx: f64, cy: f64, r: f64) -> String {
    let node = id();
    let response = dispatch(
        engine,
        json!({
            "type": "CreateNode",
            "id": node,
            "name": name,
            "kind": {
                "Circle": {
                    "cx": {"Literal": cx}, "cy": {"Literal": cy},
                    "radius": {"Literal": r},
                }
            }
        }),
    );
    assert_eq!(response["status"], "ok", "{response}");
    node
}

// ── Laws ─────────────────────────────────────────────────────────────────

#[test]
fn law_an_operation_is_a_virtual_scene_node() {
    let mut engine = VectraEngine::new();
    let a = create_rect(&mut engine, "a", 0.0, 0.0, 100.0, 40.0);
    let b = create_rect(&mut engine, "b", 60.0, 20.0, 100.0, 40.0);

    let (op, response) = apply(
        &mut engine,
        json!({"type": "boolean", "op": "subtract"}),
        vec![a.clone(), b.clone()],
    );
    assert_eq!(response["status"], "ok", "{response}");
    assert!(
        response["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["type"] == "OperationsUpdated" && e["ids"][0] == json!(op)),
        "the registry change is announced: {response}"
    );

    let view = snapshot(&mut engine);
    // RULE 3: the virtual node is a standard scene node with a Path primitive.
    assert_eq!(
        scene_node(&view, &op)["primitive"]["type"],
        "path",
        "the operation evaluates to a path: {view}"
    );
    assert!(
        scene_node(&view, &op)["primitive"]["d"]
            .as_str()
            .is_some_and(|d| d.starts_with('M') && d.len() > 8),
        "with real path data"
    );
    assert!(
        view["scene"]["z_order"]
            .as_array()
            .unwrap()
            .contains(&json!(op)),
        "and it is in the draw order: {view}"
    );
    // …while the sources are untouched primitives, still in the scene.
    assert_eq!(scene_node(&view, &a)["primitive"]["type"], "rect");
    assert_eq!(scene_node(&view, &b)["primitive"]["type"], "rect");
    assert_eq!(view["scene"]["z_order"].as_array().unwrap().len(), 3);

    // The registry view the Operations panel renders.
    let operation = &view["operations"][&op];
    assert_eq!(operation["kind"], "boolean");
    assert_eq!(operation["description"], "subtract");
    assert_eq!(operation["inputs"], json!([a, b]));
    assert_eq!(operation["input_names"], json!(["a", "b"]));
    assert_eq!(operation["enabled"], true);
}

#[test]
fn law_a_source_move_dirties_the_operation_and_only_the_operation() {
    let mut engine = VectraEngine::new();
    let a = create_rect(&mut engine, "a", 0.0, 0.0, 100.0, 40.0);
    let b = create_rect(&mut engine, "b", 60.0, 20.0, 100.0, 40.0);
    let unrelated = create_rect(&mut engine, "unrelated", 500.0, 500.0, 10.0, 10.0);
    let (op, _) = apply(
        &mut engine,
        json!({"type": "boolean", "op": "union"}),
        vec![a.clone(), b.clone()],
    );

    let before = snapshot(&mut engine);
    let op_path_before = scene_node(&before, &op)["primitive"]["d"]
        .as_str()
        .unwrap()
        .to_string();
    let a_before = scene_node(&before, &a)["primitive"].clone();

    // Move B — the operation must re-run, and the Dirty event must say so.
    let response = dispatch(
        &mut engine,
        json!({
            "type": "SetParameter", "node_id": b, "property": "x",
            "value": {"Float": {"Literal": 10.0}}
        }),
    );
    assert_eq!(response["status"], "ok", "{response}");
    let dirty = dirty_ids(&response);
    assert!(
        dirty.contains(&op),
        "the operation is named as re-evaluated: {dirty:?} in {response}"
    );
    assert!(
        dirty.contains(&b),
        "and so is the source that moved: {dirty:?}"
    );
    assert!(
        !dirty.contains(&unrelated),
        "an unrelated layer is never dirtied: {dirty:?}"
    );

    let after = snapshot(&mut engine);
    assert_ne!(
        scene_node(&after, &op)["primitive"]["d"],
        json!(op_path_before),
        "the union tracked the move"
    );
    assert_eq!(
        scene_node(&after, &a)["primitive"],
        a_before,
        "the partner is byte-identical (RULE 1)"
    );
    assert_eq!(
        after["eval"]["full_evals"], before["eval"]["full_evals"],
        "and none of this cost a full re-evaluation"
    );

    // Moving something the operation does not read must not touch it.
    let op_after_move = scene_node(&after, &op)["primitive"]["d"].clone();
    dispatch(
        &mut engine,
        json!({
            "type": "SetParameter", "node_id": unrelated, "property": "x",
            "value": {"Float": {"Literal": 505.0}}
        }),
    );
    let view = snapshot(&mut engine);
    assert_eq!(
        scene_node(&view, &op)["primitive"]["d"],
        op_after_move,
        "an operation is untouched by edits outside its inputs"
    );
}

#[test]
fn law_undo_removes_the_operation_and_leaves_the_sources() {
    let mut engine = VectraEngine::new();
    let a = create_rect(&mut engine, "a", 0.0, 0.0, 100.0, 40.0);
    let b = create_rect(&mut engine, "b", 60.0, 20.0, 100.0, 40.0);
    let before = snapshot(&mut engine);
    let (op, _) = apply(
        &mut engine,
        json!({"type": "boolean", "op": "intersect"}),
        vec![a.clone(), b.clone()],
    );
    assert!(snapshot(&mut engine)["operations"][&op].is_object());

    let response = undo(&mut engine);
    assert_eq!(response["status"], "ok", "{response}");
    let view = snapshot(&mut engine);
    assert!(
        view["operations"].as_object().unwrap().is_empty(),
        "the operation record is gone: {view}"
    );
    assert!(
        view["scene"]["nodes"].get(&op).is_none(),
        "and its geometry left the scene"
    );
    assert!(!view["scene"]["z_order"]
        .as_array()
        .unwrap()
        .contains(&json!(op)));
    // The sources are exactly as they were before the operation existed.
    assert_eq!(view["scene"]["nodes"][&a], before["scene"]["nodes"][&a]);
    assert_eq!(view["scene"]["nodes"][&b], before["scene"]["nodes"][&b]);
    assert_eq!(view["scene"]["z_order"], before["scene"]["z_order"]);

    let response = serde_json::from_str::<Value>(&engine.redo()).unwrap();
    assert_eq!(response["status"], "ok", "{response}");
    let view = snapshot(&mut engine);
    assert!(view["operations"][&op].is_object(), "redo restores it");
    assert_eq!(scene_node(&view, &op)["primitive"]["type"], "path");
}

#[test]
fn law_parking_an_operation_keeps_its_inputs_live() {
    let mut engine = VectraEngine::new();
    let a = create_rect(&mut engine, "a", 0.0, 0.0, 100.0, 40.0);
    let b = create_rect(&mut engine, "b", 60.0, 20.0, 100.0, 40.0);
    let (op, _) = apply(
        &mut engine,
        json!({"type": "boolean", "op": "subtract"}),
        vec![a.clone(), b.clone()],
    );

    let response = dispatch(
        &mut engine,
        json!({"type": "SetOperationEnabled", "id": op, "enabled": false}),
    );
    assert_eq!(response["status"], "ok", "{response}");
    let view = snapshot(&mut engine);
    assert_eq!(view["operations"][&op]["enabled"], false);
    assert!(
        view["scene"]["nodes"].get(&op).is_none(),
        "a parked operation contributes no geometry"
    );
    assert!(
        view["scene"]["nodes"].get(&a).is_some() && view["scene"]["nodes"].get(&b).is_some(),
        "its sources stay in the scene"
    );
    assert_eq!(view["scene"]["z_order"].as_array().unwrap().len(), 2);

    // Re-arming brings it back, with the same id.
    dispatch(
        &mut engine,
        json!({"type": "SetOperationEnabled", "id": op, "enabled": true}),
    );
    let view = snapshot(&mut engine);
    assert_eq!(view["operations"][&op]["enabled"], true);
    assert_eq!(scene_node(&view, &op)["primitive"]["type"], "path");
}

#[test]
fn law_removing_an_operation_leaves_every_source() {
    let mut engine = VectraEngine::new();
    let a = create_rect(&mut engine, "a", 0.0, 0.0, 10.0, 10.0);
    let b = create_rect(&mut engine, "b", 5.0, 5.0, 10.0, 10.0);
    let before = snapshot(&mut engine);
    let (op, _) = apply(
        &mut engine,
        json!({"type": "boolean", "op": "exclude"}),
        vec![a.clone(), b.clone()],
    );

    let response = dispatch(&mut engine, json!({"type": "RemoveOperation", "id": op}));
    assert_eq!(response["status"], "ok", "{response}");
    let view = snapshot(&mut engine);
    assert!(view["operations"].as_object().unwrap().is_empty());
    assert_eq!(view["scene"]["nodes"][&a], before["scene"]["nodes"][&a]);
    assert_eq!(view["scene"]["nodes"][&b], before["scene"]["nodes"][&b]);
    assert_eq!(view["scene"]["z_order"], before["scene"]["z_order"]);

    // …and undoing the removal restores the same virtual node.
    undo(&mut engine);
    let view = snapshot(&mut engine);
    assert!(view["operations"][&op].is_object(), "{view}");
    assert_eq!(view["operations"][&op]["inputs"], json!([a, b]));
}

#[test]
fn modifiers_reach_the_scene_and_keep_their_sources() {
    let mut engine = VectraEngine::new();
    let c = create_rect(&mut engine, "c", 0.0, 0.0, 20.0, 20.0);
    let before = snapshot(&mut engine);

    let fixtures = [
        json!({"type": "offset", "distance": {"Literal": 4.0}}),
        json!({"type": "fillet", "radius": {"Literal": 3.0}}),
        json!({"type": "mirror", "axis": {"axis": "vertical", "at": {"Literal": 0.0}}}),
    ];
    for kind in fixtures {
        let (op, response) = apply(&mut engine, kind.clone(), vec![c.clone()]);
        assert_eq!(response["status"], "ok", "{kind}: {response}");
        let view = snapshot(&mut engine);
        let d = scene_node(&view, &op)["primitive"]["d"]
            .as_str()
            .expect("modifier geometry")
            .to_string();
        let xs = x_values(&d);
        assert!(!xs.is_empty(), "{kind} produced vertices: {d}");

        match kind["type"].as_str().unwrap() {
            "offset" => assert!(
                xs.iter().cloned().fold(f64::INFINITY, f64::min) < -1.0,
                "offsetting 4 units outward reaches x < 0: {d}"
            ),
            "fillet" => assert!(
                xs.iter().cloned().fold(f64::NEG_INFINITY, f64::max) <= 20.0 + 1e-3,
                "filleting never grows past the source bounds: {d}"
            ),
            "mirror" => assert!(
                xs.iter().all(|x| *x <= 1e-6),
                "a mirror across x = 0 puts every vertex at x ≤ 0: {d}"
            ),
            other => panic!("unexpected modifier {other}"),
        }
        // The source keeps its primitive — same type and same numbers.
        assert_eq!(
            scene_node(&view, &c)["primitive"],
            before["scene"]["nodes"][&c]["primitive"],
            "{kind} did not touch its source"
        );
    }
}

#[test]
fn ill_formed_operations_are_refused_with_nothing_applied() {
    let mut engine = VectraEngine::new();
    let a = create_rect(&mut engine, "a", 0.0, 0.0, 10.0, 10.0);
    let b = create_rect(&mut engine, "b", 5.0, 5.0, 10.0, 10.0);
    let view_before = snapshot(&mut engine);

    // A boolean with one input: arity.
    let (_, response) = apply(
        &mut engine,
        json!({"type": "boolean", "op": "union"}),
        vec![a.clone()],
    );
    assert_eq!(response["status"], "error", "{response}");
    assert!(
        response["message"]
            .as_str()
            .unwrap()
            .contains("takes 2 input"),
        "{response}"
    );

    // An operation over a node that does not exist.
    let (_, response) = apply(
        &mut engine,
        json!({"type": "fillet", "radius": {"Literal": 1.0}}),
        vec![id()],
    );
    assert_eq!(response["status"], "error", "{response}");
    assert!(
        response["message"]
            .as_str()
            .unwrap()
            .contains("node not found"),
        "{response}"
    );

    // Removing / enabling something that is not an operation.
    let response = dispatch(&mut engine, json!({"type": "RemoveOperation", "id": a}));
    assert!(
        response["message"]
            .as_str()
            .unwrap()
            .contains("operation not found"),
        "{response}"
    );

    // Nothing was applied by any of the refusals.
    let view = snapshot(&mut engine);
    assert!(view["operations"].as_object().unwrap().is_empty());
    assert_eq!(view["scene"], view_before["scene"]);
    assert_eq!(view["can_undo"], view_before["can_undo"]);

    // The same operation over real inputs still works afterwards.
    let (op, response) = apply(
        &mut engine,
        json!({"type": "boolean", "op": "union"}),
        vec![a, b],
    );
    assert_eq!(response["status"], "ok", "{response}");
    assert!(snapshot(&mut engine)["operations"][&op].is_object());
}

#[test]
fn deleting_a_source_withdraws_the_operation_and_undo_brings_both_back() {
    let mut engine = VectraEngine::new();
    let a = create_rect(&mut engine, "a", 0.0, 0.0, 10.0, 10.0);
    let b = create_rect(&mut engine, "b", 5.0, 5.0, 10.0, 10.0);
    let (op, _) = apply(
        &mut engine,
        json!({"type": "boolean", "op": "union"}),
        vec![a.clone(), b.clone()],
    );

    let response = dispatch(&mut engine, json!({"type": "DeleteNode", "id": a}));
    assert_eq!(response["status"], "ok", "{response}");
    let view = snapshot(&mut engine);
    assert!(
        view["operations"].as_object().unwrap().is_empty(),
        "an operation whose source vanished has nothing to read: {view}"
    );
    assert!(view["scene"]["nodes"].get(&op).is_none());
    assert!(view["scene"]["nodes"].get(&b).is_some(), "b survives");

    undo(&mut engine);
    let view = snapshot(&mut engine);
    assert!(
        view["operations"][&op].is_object(),
        "one undo restores both"
    );
    assert!(view["scene"]["nodes"].get(&a).is_some());
    assert_eq!(scene_node(&view, &op)["primitive"]["type"], "path");
}

#[test]
fn a_full_rebuild_matches_the_incremental_operations_layer() {
    let mut engine = VectraEngine::new();
    let a = create_rect(&mut engine, "a", 0.0, 0.0, 100.0, 40.0);
    let b = create_rect(&mut engine, "b", 60.0, 20.0, 100.0, 40.0);
    let (op, _) = apply(
        &mut engine,
        json!({"type": "boolean", "op": "subtract"}),
        vec![a.clone(), b.clone()],
    );
    // Two more moves, so the virtual node has been re-derived several times.
    for x in [40.0, 25.0] {
        dispatch(
            &mut engine,
            json!({
                "type": "SetParameter", "node_id": b, "property": "x",
                "value": {"Float": {"Literal": x}}
            }),
        );
    }
    let incremental = snapshot(&mut engine);
    let patch_d = scene_node(&incremental, &op)["primitive"]["d"].clone();

    let response: Value = serde_json::from_str(&engine.force_full_evaluation()).unwrap();
    assert_eq!(response["status"], "ok", "{response}");
    assert_eq!(response["events"][0]["mode"], "full");
    let rebuilt = snapshot(&mut engine);
    assert_eq!(
        scene_node(&rebuilt, &op)["primitive"]["d"],
        patch_d,
        "the incrementally derived operation equals a cold rebuild"
    );
    assert!(
        response["events"][0]["ids"]
            .as_array()
            .unwrap()
            .contains(&json!(op)),
        "a full pass names the virtual node too: {response}"
    );
}

/// The clock is a *source* of dirt, and an operation reads sources.
///
/// A time-bound parameter (`$time`) moves the circle's radius; the subtract that
/// reads it must re-run in the same `set_time` call. This is the one path into
/// the scene that does not go through `dispatch`, so it needs its own witness —
/// otherwise a clock-driven animation would silently leave the virtual node
/// stale while its source moved.
#[test]
fn law_the_clock_reaches_operations() {
    let mut engine = VectraEngine::new();
    let disc = create_circle(&mut engine, "clocked", 100.0, 100.0, 10.0);
    let expr = id();
    let response = dispatch(
        &mut engine,
        json!({"type": "DefineExpression", "id": expr, "source": "$time * 10"}),
    );
    assert_eq!(response["status"], "ok", "{response}");
    let response = dispatch(
        &mut engine,
        json!({
            "type": "SetParameter",
            "node_id": disc,
            "property": "radius",
            "value": {"Float": {"Expression": expr}},
        }),
    );
    assert_eq!(response["status"], "ok", "{response}");
    let blade = create_rect(&mut engine, "blade", 100.0, 80.0, 200.0, 40.0);

    // Park the clock at 1.0 → radius 10, then cut the rectangle out of it.
    let settled: Value = serde_json::from_str(&engine.set_time(1.0)).unwrap();
    assert_eq!(settled["status"], "ok", "{settled}");
    let (op, response) = apply(
        &mut engine,
        json!({"type": "boolean", "op": "subtract"}),
        vec![disc.clone(), blade.clone()],
    );
    assert_eq!(response["status"], "ok", "{response}");
    let before = snapshot(&mut engine);
    assert_eq!(scene_node(&before, &disc)["primitive"]["r"], 10.0);
    let d_before = scene_node(&before, &op)["primitive"]["d"].clone();
    assert!(d_before.is_string(), "the operation has geometry: {before}");

    // Advancing the clock re-evaluates the source *and* the virtual node.
    let advanced: Value = serde_json::from_str(&engine.set_time(2.0)).unwrap();
    assert_eq!(advanced["status"], "ok", "{advanced}");
    assert!(
        dirty_ids(&advanced).contains(&op),
        "the clock's dirty set must name the virtual node: {advanced}"
    );
    let after = snapshot(&mut engine);
    assert_eq!(scene_node(&after, &disc)["primitive"]["r"], 20.0);
    assert_ne!(
        scene_node(&after, &op)["primitive"]["d"],
        d_before,
        "the operation re-ran on the clock's edit"
    );
}
