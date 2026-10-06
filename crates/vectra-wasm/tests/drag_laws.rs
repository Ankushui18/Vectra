//! Task 3.2 laws: **direct manipulation**. The pointer drives Cassowary's own
//! edit-variable API, the constraints pull the partners along in real time, and
//! the whole gesture is one undoable action.
//!
//! Everything here runs through the real composition root
//! (`VectraEngine::dispatch_command` / `undo` / `redo` + `get_snapshot`), i.e.
//! the exact path the React remote control drives.
//!
//! * [`law_drag_partner_follows_the_pointer`] — dragging A moves B, exactly:
//!   `suggest_value` on A's edit variables, no `SetParameter` re-solve loop.
//! * [`law_required_pins_resist_the_drag`] — a `Required` rule outranks the
//!   pointer: the node does not move, nothing is written, nothing crashes.
//! * [`law_weaker_pin_snaps_back_on_release`] — a `Medium` rule yields to the
//!   pointer *during* the gesture and takes the slot back on release.
//! * [`law_end_drag_persists_literals_and_clears_the_registry`] — the last
//!   solved values land in the document as literals (a dragged link is broken
//!   and reported), and the active-edits registry is empty again.
//! * [`law_one_drag_is_one_undo_step`] — undo restores the whole pre-gesture
//!   state in a single step; redo replays it; a click that moves nothing is not
//!   a history entry at all.
//! * [`law_drag_protocol_rejections_are_typed_and_atomic`] — the state machine
//!   refuses nonsense without touching the document.
//! * [`prop_drag_is_deterministic_and_idempotent`] — repeated samples with the
//!   same coordinates write nothing, and two engines fed the same gesture are
//!   byte-identical.

use vectra_core::{new_node_id, ParamValue, Parameter};
use vectra_wasm::VectraEngine;

fn id() -> String {
    new_node_id().to_string()
}

fn dispatch(engine: &mut VectraEngine, cmd: serde_json::Value) -> serde_json::Value {
    serde_json::from_str(&engine.dispatch_command(&cmd.to_string())).expect("parsed response")
}

fn snapshot(engine: &mut VectraEngine) -> serde_json::Value {
    serde_json::from_str(&engine.get_snapshot()).expect("parsed snapshot")
}

fn dependencies(engine: &VectraEngine) -> serde_json::Value {
    serde_json::from_str(&engine.dependencies()).expect("parsed graph")
}

fn undo(engine: &mut VectraEngine) -> serde_json::Value {
    serde_json::from_str(&engine.undo()).expect("parsed undo")
}

fn redo(engine: &mut VectraEngine) -> serde_json::Value {
    serde_json::from_str(&engine.redo()).expect("parsed redo")
}

fn create_rect(engine: &mut VectraEngine, name: &str, x: f64, y: f64) -> String {
    let node = id();
    let response = dispatch(
        engine,
        serde_json::json!({
            "type": "CreateNode",
            "id": node,
            "name": name,
            "kind": { "Rectangle": {
                "x": { "Literal": x },
                "y": { "Literal": y },
                "width": { "Literal": 10.0 },
                "height": { "Literal": 10.0 },
                "corner_radius": { "Literal": 0.0 },
            }},
        }),
    );
    assert_eq!(response["status"], "ok", "create {name}: {response}");
    node
}

fn add_constraint(
    engine: &mut VectraEngine,
    kind: &str,
    targets: &[(&str, &str)],
    strength: Option<&str>,
    value: Option<f64>,
) -> (String, serde_json::Value) {
    let constraint_id = id();
    let mut constraint = serde_json::json!({
        "id": constraint_id,
        "kind": kind,
        "targets": targets
            .iter()
            .map(|(node, property)| serde_json::json!({ "node_id": node, "property": property }))
            .collect::<Vec<_>>(),
    });
    if let Some(strength) = strength {
        constraint["strength"] = serde_json::json!(strength);
    }
    if let Some(value) = value {
        constraint["value"] = serde_json::json!(value);
    }
    let response = dispatch(
        engine,
        serde_json::json!({ "type": "AddConstraint", "constraint": constraint }),
    );
    (constraint_id, response)
}

fn begin(engine: &mut VectraEngine, node: &str) -> serde_json::Value {
    dispatch(
        engine,
        serde_json::json!({ "type": "BeginDrag", "node_id": node }),
    )
}

fn update(engine: &mut VectraEngine, node: &str, x: f64, y: f64) -> serde_json::Value {
    dispatch(
        engine,
        serde_json::json!({ "type": "UpdateDrag", "node_id": node, "x": x, "y": y }),
    )
}

fn end(engine: &mut VectraEngine, node: &str) -> serde_json::Value {
    dispatch(
        engine,
        serde_json::json!({ "type": "EndDrag", "node_id": node }),
    )
}

/// The node's canonical position as the snapshot reports it.
fn position(view: &serde_json::Value, node: &str) -> serde_json::Value {
    view["scene"]["nodes"][node]["position"].clone()
}

fn pos_x(view: &serde_json::Value, node: &str) -> f64 {
    position(view, node)["x"]
        .as_f64()
        .unwrap_or_else(|| panic!("no position for {node} in {view}"))
}

fn pos_y(view: &serde_json::Value, node: &str) -> f64 {
    position(view, node)["y"].as_f64().expect("position y")
}

fn solver(view: &serde_json::Value) -> &serde_json::Value {
    &view["solver"]
}

fn dirty_ids(response: &serde_json::Value) -> Vec<String> {
    response["events"]
        .as_array()
        .expect("events")
        .iter()
        .find(|event| event["type"] == "Dirty")
        .expect("every mutation reports a Dirty event")["ids"]
        .as_array()
        .expect("dirty ids")
        .iter()
        .map(|value| value.as_str().unwrap().to_string())
        .collect()
}

fn diagnostics_with_code(view: &serde_json::Value, code: &str) -> Vec<serde_json::Value> {
    view["diagnostics"]
        .as_array()
        .expect("diagnostics array")
        .iter()
        .filter(|diagnostic| diagnostic["code"] == code)
        .cloned()
        .collect()
}

// ── L1: the partner follows the pointer ───────────────────────────────────

#[test]
fn law_drag_partner_follows_the_pointer() {
    let mut engine = VectraEngine::new();
    let a = create_rect(&mut engine, "a", 100.0, 0.0);
    let b = create_rect(&mut engine, "b", 300.0, 0.0);
    let (_rule, response) =
        add_constraint(&mut engine, "vertical", &[(&a, "x"), (&b, "x")], None, None);
    assert_eq!(response["status"], "ok", "{response}");
    let full_evals = snapshot(&mut engine)["eval"]["full_evals"].clone();

    // BeginDrag: the registry holds the node's two position slots as edits.
    let response = begin(&mut engine, &a);
    assert_eq!(response["status"], "ok", "{response}");
    assert!(
        response["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|event| event["type"] == "DragStarted"),
        "the gesture is announced: {response}"
    );
    let view = snapshot(&mut engine);
    assert_eq!(
        solver(&view)["active_edits"],
        2,
        "x and y are edit variables: {view}"
    );
    assert_eq!(solver(&view)["drag_node"], a.as_str());

    // One pointer sample: the dragged node moves *and* the partner follows.
    let response = update(&mut engine, &a, 200.0, 50.0);
    assert_eq!(response["status"], "ok", "{response}");
    let mut dirty = dirty_ids(&response);
    dirty.sort();
    let mut expected = vec![a.clone(), b.clone()];
    expected.sort();
    assert_eq!(
        dirty, expected,
        "the dragged node and its constrained partner were re-evaluated: {response}"
    );

    let view = snapshot(&mut engine);
    assert_eq!(pos_x(&view, &a), 200.0, "A followed the pointer exactly");
    assert_eq!(
        pos_x(&view, &b),
        200.0,
        "B was pulled by the constraint, exactly"
    );
    assert_eq!(pos_y(&view, &a), 50.0, "A's y is free and follows too");
    assert_eq!(pos_y(&view, &b), 0.0, "nothing constrains B's y");
    assert_eq!(solver(&view)["active_edits"], 2);
    assert_eq!(
        view["eval"]["full_evals"], full_evals,
        "a drag patches the scene, it never rebuilds it"
    );

    // A second sample keeps the gesture alive and keeps the partner in sync.
    let response = update(&mut engine, &a, 40.0, 60.0);
    assert_eq!(response["status"], "ok", "{response}");
    let view = snapshot(&mut engine);
    assert_eq!(pos_x(&view, &a), 40.0);
    assert_eq!(pos_x(&view, &b), 40.0);
    assert_eq!(solver(&view)["active_edits"], 2, "still dragging");
}

// ── L2: resistance under a Required pin ───────────────────────────────────

#[test]
fn law_required_pins_resist_the_drag() {
    let mut engine = VectraEngine::new();
    let a = create_rect(&mut engine, "a", 100.0, 25.0);
    // A "fully pinned" node: both position slots are held by Required rules.
    let (_pin_x, response) = add_constraint(
        &mut engine,
        "angle",
        &[(&a, "x")],
        Some("required"),
        Some(100.0),
    );
    assert_eq!(response["status"], "ok", "{response}");
    let (_pin_y, response) = add_constraint(
        &mut engine,
        "angle",
        &[(&a, "y")],
        Some("required"),
        Some(25.0),
    );
    assert_eq!(response["status"], "ok", "{response}");

    assert_eq!(begin(&mut engine, &a)["status"], "ok");
    let response = update(&mut engine, &a, 500.0, 500.0);
    assert_eq!(
        response["status"], "ok",
        "a pinned drag is resisted, not an error: {response}"
    );

    let view = snapshot(&mut engine);
    assert_eq!(
        pos_x(&view, &a),
        100.0,
        "the Required pin wins over the pointer"
    );
    assert_eq!(pos_y(&view, &a), 25.0);
    assert_eq!(
        solver(&view)["writes"],
        0,
        "nothing moved, so nothing was written: {view}"
    );
    assert_eq!(
        solver(&view)["active_edits"],
        2,
        "the pointer is still registered — the gesture is alive: {view}"
    );

    // Releasing changes nothing either: the pins still hold.
    let response = end(&mut engine, &a);
    assert_eq!(response["status"], "ok", "{response}");
    let view = snapshot(&mut engine);
    assert_eq!(pos_x(&view, &a), 100.0);
    assert_eq!(pos_y(&view, &a), 25.0);
    assert_eq!(solver(&view)["active_edits"], 0);
    assert_eq!(
        view["constraints"]
            .as_object()
            .unwrap()
            .values()
            .filter(|rule| rule["enabled"] == true)
            .count(),
        2,
        "both pins are still armed: {view}"
    );
}

// ── L3: a weaker rule yields, then takes the slot back ───────────────────

#[test]
fn law_weaker_pin_snaps_back_on_release() {
    let mut engine = VectraEngine::new();
    let a = create_rect(&mut engine, "a", 100.0, 0.0);
    let (_rule, response) = add_constraint(
        &mut engine,
        "angle",
        &[(&a, "x")],
        Some("medium"),
        Some(100.0),
    );
    assert_eq!(response["status"], "ok", "{response}");

    assert_eq!(begin(&mut engine, &a)["status"], "ok");
    let response = update(&mut engine, &a, 500.0, 0.0);
    assert_eq!(response["status"], "ok", "{response}");
    assert_eq!(
        pos_x(&snapshot(&mut engine), &a),
        500.0,
        "STRONG edits beat a MEDIUM rule while the pointer is down"
    );

    let response = end(&mut engine, &a);
    assert_eq!(response["status"], "ok", "{response}");
    let view = snapshot(&mut engine);
    assert_eq!(
        pos_x(&view, &a),
        100.0,
        "with no pointer in the tableau the rule re-asserts — the snap-back: {view}"
    );
    assert_eq!(solver(&view)["active_edits"], 0);
    assert_eq!(solver(&view)["drag_node"], serde_json::Value::Null);
}

// ── L4: EndDrag persists literals and releases the edits ──────────────────

#[test]
fn law_end_drag_persists_literals_and_clears_the_registry() {
    let mut engine = VectraEngine::new();
    let a = create_rect(&mut engine, "a", 100.0, 0.0);
    let b = create_rect(&mut engine, "b", 300.0, 0.0);

    // A.x is driven by $base: dragging it has to break that link, loudly.
    dispatch(
        &mut engine,
        serde_json::json!({ "type": "SetVariable", "name": "base", "value": 300.0 }),
    );
    let response = dispatch(
        &mut engine,
        serde_json::json!({
            "type": "SetParameter", "node_id": a, "property": "x",
            "value": { "Float": { "Variable": "base" } },
        }),
    );
    assert_eq!(response["status"], "ok", "{response}");
    assert_eq!(
        dependencies(&engine)["edges"].as_array().unwrap().len(),
        1,
        "A.x depends on $base"
    );
    let (_rule, response) =
        add_constraint(&mut engine, "vertical", &[(&a, "x"), (&b, "x")], None, None);
    assert_eq!(response["status"], "ok", "{response}");

    assert_eq!(begin(&mut engine, &a)["status"], "ok");
    let response = update(&mut engine, &a, 250.0, 0.0);
    assert_eq!(response["status"], "ok", "{response}");

    let view = snapshot(&mut engine);
    let broken = diagnostics_with_code(&view, "parametric-link-broken");
    assert_eq!(
        broken.len(),
        1,
        "the drag broke the link, and says so: {view}"
    );
    assert_eq!(broken[0]["property"], "x");
    assert!(
        broken[0]["message"].as_str().unwrap().contains("$base"),
        "the diagnostic names the link: {broken:?}"
    );
    assert_eq!(
        pos_x(&view, &a),
        250.0,
        "the dragged slot followed the pointer"
    );
    assert_eq!(pos_x(&view, &b), 250.0, "and the partner came along");

    // The gesture ends: the registry is empty, the values are literals.
    let response = end(&mut engine, &a);
    assert_eq!(response["status"], "ok", "{response}");
    assert!(
        response["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|event| event["type"] == "DragEnded"),
        "the gesture is closed: {response}"
    );

    let view = snapshot(&mut engine);
    assert_eq!(
        solver(&view)["active_edits"],
        0,
        "the active-edits registry is empty: {view}"
    );
    assert_eq!(solver(&view)["drag_node"], serde_json::Value::Null);
    assert_eq!(
        solver(&view)["variables"],
        2,
        "the pool kept only the slots the live rule still needs: {view}"
    );

    let a_position = position(&view, &a);
    let b_position = position(&view, &b);
    assert_eq!(a_position["x"], 250.0);
    assert_eq!(
        a_position["x_source"], "literal",
        "the dragged value is a literal now: {a_position}"
    );
    assert_eq!(
        b_position["x_source"], "literal",
        "and so is the partner's: {b_position}"
    );
    assert_eq!(
        view["variables"]["base"], 300.0,
        "the solver never wrote the variable: {view}"
    );
    assert_eq!(
        dependencies(&engine)["edges"].as_array().unwrap().len(),
        0,
        "the parametric link is gone — dragging pins literals"
    );
}

// ── L5: one gesture, one history entry ────────────────────────────────────

#[test]
fn law_one_drag_is_one_undo_step() {
    let mut engine = VectraEngine::new();
    let a = create_rect(&mut engine, "a", 100.0, 0.0);
    let b = create_rect(&mut engine, "b", 300.0, 0.0);
    let (rule, _) = add_constraint(&mut engine, "vertical", &[(&a, "x"), (&b, "x")], None, None);

    // Adding the rule already solved it: B snapped onto A's column. *That* is
    // the state the gesture starts from, and the state undo must restore.
    let before = snapshot(&mut engine);
    assert_eq!(pos_x(&before, &a), 100.0);
    assert_eq!(pos_x(&before, &b), 100.0, "the rule solved on add");

    assert_eq!(begin(&mut engine, &a)["status"], "ok");
    assert_eq!(update(&mut engine, &a, 200.0, 0.0)["status"], "ok");
    assert_eq!(update(&mut engine, &a, 250.0, 0.0)["status"], "ok");
    assert_eq!(end(&mut engine, &a)["status"], "ok");
    let view = snapshot(&mut engine);
    assert_eq!(pos_x(&view, &a), 250.0);
    assert_eq!(pos_x(&view, &b), 250.0);

    // ONE undo: the gesture is a true pre-image — both nodes are back where the
    // gesture found them (A was dragged, B was pulled), and the rule the drag
    // was obeying is still registered, so exactly one entry was recorded.
    let response = undo(&mut engine);
    assert_eq!(response["status"], "ok", "{response}");
    let view = snapshot(&mut engine);
    assert_eq!(pos_x(&view, &a), 100.0, "A is back");
    assert_eq!(
        pos_x(&view, &b),
        100.0,
        "B is back — one entry moved both slots"
    );
    assert_eq!(
        view["scene"], before["scene"],
        "undo restores the whole scene it found"
    );
    assert!(
        view["constraints"].get(&rule).is_some(),
        "the rule the drag was obeying is untouched: {view}"
    );

    // …and a second undo reaches past the gesture, to the rule itself: proof
    // that the drag added exactly one entry.
    let response = undo(&mut engine);
    assert_eq!(response["status"], "ok", "{response}");
    let view = snapshot(&mut engine);
    assert!(
        view["constraints"].get(&rule).is_none(),
        "the second undo removes the rule, not drag geometry: {view}"
    );
    assert_eq!(pos_x(&view, &a), 100.0);
    assert_eq!(pos_x(&view, &b), 300.0, "unconstrained again");

    // Redo walks back up: first the rule, then the gesture's net movement —
    // which replays as literals, with no solver in the loop.
    assert_eq!(redo(&mut engine)["status"], "ok", "the rule comes back");
    let response = redo(&mut engine);
    assert_eq!(response["status"], "ok", "{response}");
    let view = snapshot(&mut engine);
    assert_eq!(
        pos_x(&view, &a),
        250.0,
        "the gesture's net movement is replayed"
    );
    assert_eq!(pos_x(&view, &b), 250.0);

    // A click that moves nothing must not cost an undo step: the only entry in
    // this engine is the CreateNode, so one undo has to reach *past* the gesture
    // (an empty scene) instead of leaving the node where the gesture found it.
    let mut fresh = VectraEngine::new();
    let node = create_rect(&mut fresh, "solo", 10.0, 10.0);
    assert_eq!(begin(&mut fresh, &node)["status"], "ok");
    assert_eq!(end(&mut fresh, &node)["status"], "ok");
    let view = snapshot(&mut fresh);
    assert_eq!(pos_x(&view, &node), 10.0, "a click cannot move geometry");
    assert_eq!(view["can_undo"], true, "the CreateNode is undoable");
    let response = undo(&mut fresh);
    assert_eq!(response["status"], "ok", "{response}");
    let view = snapshot(&mut fresh);
    assert!(
        view["scene"]["nodes"].as_object().unwrap().is_empty(),
        "one undo reached past the gesture, so the gesture added no entry: {view}"
    );
}

// ── L6: the protocol's refusals are typed and atomic ──────────────────────

#[test]
fn law_drag_protocol_rejections_are_typed_and_atomic() {
    let mut engine = VectraEngine::new();
    let a = create_rect(&mut engine, "a", 100.0, 0.0);
    let b = create_rect(&mut engine, "b", 300.0, 0.0);

    // UpdateDrag / EndDrag with no gesture live.
    let response = update(&mut engine, &a, 200.0, 0.0);
    assert_eq!(response["status"], "error");
    assert!(
        response["message"]
            .as_str()
            .unwrap()
            .contains("no drag in progress"),
        "{response}"
    );
    let response = end(&mut engine, &a);
    assert_eq!(response["status"], "error");

    // A drag of a node with no position slots (a Group).
    let group = id();
    let response = dispatch(
        &mut engine,
        serde_json::json!({
            "type": "CreateNode", "id": group, "name": "g",
            "kind": { "Group": { "children": [] } },
        }),
    );
    assert_eq!(response["status"], "ok", "{response}");
    let response = begin(&mut engine, &group);
    assert_eq!(response["status"], "error");
    assert!(
        response["message"]
            .as_str()
            .unwrap()
            .contains("no draggable position slots"),
        "{response}"
    );
    let view = snapshot(&mut engine);
    assert_eq!(
        solver(&view)["active_edits"],
        0,
        "a refused BeginDrag leaves no session behind: {view}"
    );

    // While a gesture is live, everything else is refused.
    assert_eq!(begin(&mut engine, &a)["status"], "ok");
    assert_eq!(update(&mut engine, &a, 150.0, 0.0)["status"], "ok");
    let before = snapshot(&mut engine);

    let response = update(&mut engine, &b, 1.0, 1.0);
    assert_eq!(response["status"], "error", "a different node: {response}");
    assert!(response["message"]
        .as_str()
        .unwrap()
        .contains("no drag in progress"));

    let response = begin(&mut engine, &b);
    assert_eq!(response["status"], "error");
    assert!(
        response["message"]
            .as_str()
            .unwrap()
            .contains("a drag of node")
            && response["message"]
                .as_str()
                .unwrap()
                .contains("is in progress"),
        "a second gesture is refused: {response}"
    );

    let response = undo(&mut engine);
    assert_eq!(response["status"], "error", "undo mid-gesture: {response}");
    assert!(
        response["message"]
            .as_str()
            .unwrap()
            .contains("is in progress"),
        "{response}"
    );

    let response = redo(&mut engine);
    assert_eq!(response["status"], "error");
    let response = dispatch(
        &mut engine,
        serde_json::json!({ "type": "SetVariable", "name": "base", "value": 5.0 }),
    );
    assert_eq!(
        response["status"], "error",
        "any other mutation: {response}"
    );

    // Nothing was applied by any of those refusals.
    let after = snapshot(&mut engine);
    assert_eq!(after["scene"], before["scene"], "geometry untouched");
    assert_eq!(
        after["variables"], before["variables"],
        "variables untouched"
    );
    assert_eq!(after["constraints"], before["constraints"]);
    assert_eq!(
        solver(&after)["active_edits"],
        2,
        "and the gesture is still alive: {after}"
    );

    // Finishing it is still possible, and the document is intact.
    assert_eq!(end(&mut engine, &a)["status"], "ok");
    let view = snapshot(&mut engine);
    assert_eq!(pos_x(&view, &a), 150.0);
    assert_eq!(pos_y(&view, &b), 0.0);
    assert_eq!(solver(&view)["active_edits"], 0);
}

// ── L7: determinism + idempotence ─────────────────────────────────────────

#[test]
fn prop_drag_is_deterministic_and_idempotent() {
    // Fixed ids: "byte-identical for the same gesture" only means something if
    // the gesture itself is identical.
    let fixed = |n: u32| format!("00000000-0000-4000-8000-{n:012}");
    let script = |engine: &mut VectraEngine, samples: &[(f64, f64)]| -> Vec<String> {
        let mut snapshots = Vec::new();
        let a = fixed(1);
        let b = fixed(2);
        let rule = fixed(3);
        for (node, name, x) in [(&a, "a", 100.0), (&b, "b", 300.0)] {
            let response = dispatch(
                engine,
                serde_json::json!({
                    "type": "CreateNode", "id": node, "name": name,
                    "kind": { "Rectangle": {
                        "x": { "Literal": x }, "y": { "Literal": 0.0 },
                        "width": { "Literal": 10.0 }, "height": { "Literal": 10.0 },
                        "corner_radius": { "Literal": 0.0 },
                    }},
                }),
            );
            assert_eq!(response["status"], "ok", "{response}");
        }
        let response = dispatch(
            engine,
            serde_json::json!({ "type": "AddConstraint", "constraint": {
                "id": rule, "kind": "vertical",
                "targets": [ { "node_id": a, "property": "x" }, { "node_id": b, "property": "x" } ] } }),
        );
        assert_eq!(response["status"], "ok", "{response}");
        assert_eq!(begin(engine, &a)["status"], "ok");
        for (x, y) in samples {
            assert_eq!(update(engine, &a, *x, *y)["status"], "ok");
        }
        // Idempotence: repeating the last sample must write nothing at all.
        let Some((x, y)) = samples.last().copied() else {
            return snapshots;
        };
        let response = update(engine, &a, x, y);
        assert_eq!(response["status"], "ok", "{response}");
        assert_eq!(
            response["events"]
                .as_array()
                .unwrap()
                .iter()
                .find(|event| event["type"] == "Dirty")
                .expect("dirty event")["ids"]
                .as_array()
                .unwrap()
                .len(),
            0,
            "re-suggesting the same coordinates moves nothing: {response}"
        );
        snapshots.push(engine.get_snapshot());
        assert_eq!(end(engine, &a)["status"], "ok");
        snapshots.push(engine.get_snapshot());
        snapshots
    };

    let samples = [(180.0, 20.0), (-40.0, 90.0), (250.5, 12.25)];
    let (mut first, mut second) = (VectraEngine::new(), VectraEngine::new());
    let run_one = script(&mut first, &samples);
    let run_two = script(&mut second, &samples);
    assert_eq!(run_one, run_two, "the same gesture must be byte-identical");

    let view: serde_json::Value = serde_json::from_str(run_one.last().unwrap()).unwrap();
    assert_eq!(pos_x(&view, &fixed(1)), 250.5);
    assert_eq!(pos_x(&view, &fixed(2)), 250.5);
    assert_eq!(pos_y(&view, &fixed(1)), 12.25);
    assert_eq!(solver(&view)["active_edits"], 0);
    assert_eq!(
        solver(&view)["writes"],
        0,
        "a settled gesture reports no movement: {view}"
    );
}

// ── the triad is also meaningful outside the engine's drag path ───────────

#[test]
fn law_drag_commands_carry_exact_inverses() {
    // `BeginDrag` / `EndDrag` are session commands with identity document
    // effects; `UpdateDrag` written straight into a document moves the canonical
    // position slots. All three keep the command layer's "apply returns the exact
    // inverse" contract, so a batch containing one still undoes cleanly.
    use vectra_core::{Command, Document, NodeKind};

    let mut document = Document::new();
    let node = new_node_id();
    document
        .insert_node(
            vectra_core::Node::new(node, "a", NodeKind::rectangle(10.0, 20.0, 5.0, 5.0)),
            None,
        )
        .unwrap();

    let begin = Command::BeginDrag { node_id: node };
    let inverse = begin.apply(&mut document).unwrap();
    assert!(
        matches!(inverse, Command::Batch { ref commands } if commands.is_empty()),
        "opening a gesture changes nothing: {inverse:?}"
    );

    let update = Command::UpdateDrag {
        node_id: node,
        x: 42.0,
        y: 84.0,
    };
    let inverse = update.apply(&mut document).unwrap();
    let node_ref = document.get_node(node).unwrap();
    assert_eq!(
        node_ref.get_param("x").unwrap(),
        ParamValue::Float(Parameter::Literal(42.0))
    );
    assert_eq!(
        node_ref.get_param("y").unwrap(),
        ParamValue::Float(Parameter::Literal(84.0))
    );

    inverse.apply(&mut document).unwrap();
    let node_ref = document.get_node(node).unwrap();
    assert_eq!(
        node_ref.get_param("x").unwrap(),
        ParamValue::Float(Parameter::Literal(10.0)),
        "the inverse restores the previous slot values"
    );
    assert_eq!(
        node_ref.get_param("y").unwrap(),
        ParamValue::Float(Parameter::Literal(20.0))
    );

    let end = Command::EndDrag { node_id: node };
    assert!(matches!(
        end.apply(&mut document).unwrap(),
        Command::Batch { ref commands } if commands.is_empty()
    ));

    // A kind with no position slots refuses both.
    let group = new_node_id();
    document
        .insert_node(
            vectra_core::Node::new(group, "g", NodeKind::Group { children: vec![] }),
            None,
        )
        .unwrap();
    assert!(Command::BeginDrag { node_id: group }
        .apply(&mut document)
        .is_err());
    assert!(Command::UpdateDrag {
        node_id: group,
        x: 1.0,
        y: 1.0
    }
    .apply(&mut document)
    .is_err());
}
