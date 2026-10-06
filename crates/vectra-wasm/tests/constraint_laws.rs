//! Task 3.1 laws: constraints are *hard mathematical rules* enforced by the
//! engine, and they behave like every other mutation.
//!
//! Everything here runs through the real composition root
//! (`VectraEngine::dispatch_command` / `undo` / `redo` + `get_snapshot`), i.e.
//! the exact path the React remote control drives — never a private solver call.
//!
//! * [`law_satisfaction_via_vertical_constraint`] — MES §9/§18: make two
//!   rectangles vertical, then move one; the other follows to the *exact* value.
//! * [`law_overconstrained_drops_the_weakest`] — the weaker rule is dropped,
//!   visibly disabled, and reported as a `Diagnostic::ConstraintDropped`;
//!   two `Required` rules that contradict are a typed error, not a drop.
//! * [`law_undo_reverts_geometry_and_releases_variables`] — undo restores the
//!   unconstrained geometry *and* the Cassowary variable count decreases.
//! * [`law_parametric_link_is_broken_and_restorable`] — the solver never
//!   writes `Document.variables`: it pins the slot and reports
//!   `ParametricLinkBroken`; undo re-links the node to its source.
//! * [`law_deleting_a_node_withdraws_its_constraints`] — no dangling rules, and
//!   undo brings both node and rules back.
//! * [`prop_solving_is_idempotent_and_deterministic`] — re-solving changes
//!   nothing; the same command stream yields byte-identical snapshots.

use std::collections::BTreeSet;

use vectra_core::{new_node_id, VectraError};
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

fn undo(engine: &mut VectraEngine) -> serde_json::Value {
    serde_json::from_str(&engine.undo()).expect("parsed undo")
}

fn redo(engine: &mut VectraEngine) -> serde_json::Value {
    serde_json::from_str(&engine.redo()).expect("parsed redo")
}

fn dependencies(engine: &VectraEngine) -> serde_json::Value {
    serde_json::from_str(&engine.dependencies()).expect("parsed graph")
}

fn create_rect(engine: &mut VectraEngine, name: &str, x: f64) -> String {
    let node = id();
    let response = dispatch(
        engine,
        serde_json::json!({
            "type": "CreateNode",
            "id": node,
            "name": name,
            "kind": { "Rectangle": {
                "x": { "Literal": x },
                "y": { "Literal": 0.0 },
                "width": { "Literal": 10.0 },
                "height": { "Literal": 10.0 },
                "corner_radius": { "Literal": 0.0 },
            }},
        }),
    );
    assert_eq!(response["status"], "ok", "create {name}: {response}");
    node
}

/// Add a constraint and return `(constraint_id, response)`.
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

fn rect_x(view: &serde_json::Value, node: &str) -> f64 {
    view["scene"]["nodes"][node]["primitive"]["x"]
        .as_f64()
        .unwrap_or_else(|| panic!("no rect {node} in scene: {view}"))
}

fn diagnostics_with_code(view: &serde_json::Value, code: &str) -> Vec<serde_json::Value> {
    view["diagnostics"]
        .as_array()
        .expect("diagnostics array")
        .iter()
        .filter(|d| d["code"] == code)
        .cloned()
        .collect()
}

fn solver(view: &serde_json::Value) -> &serde_json::Value {
    &view["solver"]
}

fn dirty_ids(response: &serde_json::Value) -> Vec<String> {
    response["events"]
        .as_array()
        .expect("events")
        .iter()
        .find(|e| e["type"] == "Dirty")
        .expect("every mutation reports a Dirty event")["ids"]
        .as_array()
        .expect("dirty ids")
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect()
}

// ── L1: satisfaction ──────────────────────────────────────────────────────

#[test]
fn law_satisfaction_via_vertical_constraint() {
    let mut engine = VectraEngine::new();
    let a = create_rect(&mut engine, "a", 100.0);
    let b = create_rect(&mut engine, "b", 300.0);
    assert_eq!(rect_x(&snapshot(&mut engine), &b), 300.0);

    // "Make B vertical to A": one command, and the engine solves immediately.
    let (_constraint, response) =
        add_constraint(&mut engine, "vertical", &[(&a, "x"), (&b, "x")], None, None);
    assert_eq!(response["status"], "ok", "{response}");

    let view = snapshot(&mut engine);
    assert_eq!(
        rect_x(&view, &b),
        100.0,
        "the constraint moved B onto A's column"
    );
    assert_eq!(rect_x(&view, &a), 100.0, "the anchor did not move");
    assert_eq!(
        view["solver"]["constraints"], 1,
        "one row is in the tableau"
    );
    assert_eq!(
        view["solver"]["variables"], 2,
        "one Cassowary variable per slot"
    );
    assert_eq!(view["solver"]["writes"], 1, "exactly one slot moved");
    let before_full_evals = view["eval"]["full_evals"].as_u64().unwrap();

    // The user drags A to 150 — a plain SetParameter, and the only hint the
    // solver gets is "this slot is what the user just did".
    let response = dispatch(
        &mut engine,
        serde_json::json!({
            "type": "SetParameter", "node_id": a, "property": "x",
            "value": { "Float": { "Literal": 150.0 } },
        }),
    );
    assert_eq!(response["status"], "ok", "{response}");
    let mut dirty = dirty_ids(&response);
    dirty.sort();
    let mut expected = vec![a.clone(), b.clone()];
    expected.sort();
    assert_eq!(
        dirty, expected,
        "exactly the dragged node and the node the constraint moved: {response}"
    );
    assert!(
        response["events"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|e| e["type"] == "NodesUpdated")
            .count()
            >= 2,
        "the solver's write is reported as its own NodesUpdated: {response}"
    );

    let view = snapshot(&mut engine);
    assert_eq!(rect_x(&view, &a), 150.0);
    assert_eq!(
        rect_x(&view, &b),
        150.0,
        "B matches A exactly, not approximately"
    );
    assert_eq!(
        view["eval"]["full_evals"].as_u64().unwrap(),
        before_full_evals,
        "a constraint solve never falls back to a full re-evaluation"
    );
    assert_eq!(
        view["eval"]["last_evaluated"], 2,
        "the dragged node and its constrained partner are re-evaluated — and nothing else"
    );
}

// ── L2: over-constrained ⇒ weakest dropped, and it says so ────────────────

#[test]
fn law_overconstrained_drops_the_weakest() {
    let mut engine = VectraEngine::new();
    let a = create_rect(&mut engine, "a", 100.0);
    let b = create_rect(&mut engine, "b", 0.0);

    // `Distance(a, b) = v` is the signed separation `a - b = v`, so this rule
    // already holds and must not move anything.
    let (medium, response) = add_constraint(
        &mut engine,
        "distance",
        &[(&a, "x"), (&b, "x")],
        Some("medium"),
        Some(100.0),
    );
    assert_eq!(response["status"], "ok", "{response}");
    assert_eq!(
        solver(&snapshot(&mut engine))["writes"],
        0,
        "a rule that already holds moves nothing"
    );

    // …then ask for 200 at Weak. The two rows describe the same line with
    // different offsets, so the weaker one is dropped.
    let (weak, response) = add_constraint(
        &mut engine,
        "distance",
        &[(&a, "x"), (&b, "x")],
        Some("weak"),
        Some(200.0),
    );
    assert_eq!(
        response["status"], "ok",
        "a soft contradiction is not an error: {response}"
    );

    let view = snapshot(&mut engine);
    let dropped = diagnostics_with_code(&view, "constraint-dropped");
    assert_eq!(dropped.len(), 1, "exactly one drop is reported: {view}");
    let short = &weak[..8];
    assert!(
        dropped[0]["message"].as_str().unwrap().contains(short),
        "the diagnostic names the dropped constraint: {dropped:?}"
    );
    assert_eq!(dropped[0]["severity"], "warning");
    assert_eq!(
        view["constraints"][&weak]["enabled"], false,
        "the loser left the active set"
    );
    assert_eq!(view["constraints"][&medium]["enabled"], true);
    assert_eq!(solver(&view)["dropped"], 1);
    assert_eq!(
        solver(&view)["constraints"],
        1,
        "only the winner is in the tableau"
    );
    assert_eq!(
        rect_x(&view, &a) - rect_x(&view, &b),
        100.0,
        "the surviving rule still holds exactly"
    );

    // Required beats Medium: the medium rule is the one that goes.
    let mut engine = VectraEngine::new();
    let a = create_rect(&mut engine, "a", 100.0);
    let b = create_rect(&mut engine, "b", 0.0);
    let (_required, response) = add_constraint(
        &mut engine,
        "distance",
        &[(&a, "x"), (&b, "x")],
        Some("required"),
        Some(100.0),
    );
    assert_eq!(response["status"], "ok");
    let (medium, response) = add_constraint(
        &mut engine,
        "distance",
        &[(&a, "x"), (&b, "x")],
        Some("medium"),
        Some(200.0),
    );
    assert_eq!(
        response["status"], "ok",
        "required never yields: {response}"
    );
    let view = snapshot(&mut engine);
    assert_eq!(view["constraints"][&medium]["enabled"], false);
    assert_eq!(
        rect_x(&view, &b) - rect_x(&view, &a),
        -100.0,
        "the required 100 separation is untouched by the dropped 200"
    );

    // Required vs Required: not droppable — the command is rejected outright.
    let (_first, response) = add_constraint(
        &mut engine,
        "distance",
        &[(&a, "x"), (&b, "x")],
        Some("required"),
        Some(100.0),
    );
    assert_eq!(
        response["status"], "ok",
        "the same row at the same offset is redundant"
    );
    let (second, response) = add_constraint(
        &mut engine,
        "distance",
        &[(&a, "x"), (&b, "x")],
        Some("required"),
        Some(777.0),
    );
    assert_eq!(response["status"], "error", "{response}");
    let message = response["message"].as_str().unwrap();
    assert!(
        message.contains("unsatisfiable"),
        "typed as a required contradiction: {message}"
    );
    let view = snapshot(&mut engine);
    assert!(
        view["constraints"].get(&second).is_none(),
        "the rejected constraint was never registered"
    );
    assert_eq!(rect_x(&view, &a), 100.0, "the document is untouched");
    assert_eq!(solver(&view)["dropped"], 0, "a rejection is not a drop");
}

// ── L3: undo reverts geometry AND releases solver variables ───────────────

#[test]
fn law_undo_reverts_geometry_and_releases_variables() {
    let mut engine = VectraEngine::new();
    let a = create_rect(&mut engine, "a", 100.0);
    let b = create_rect(&mut engine, "b", 300.0);

    let (constraint, response) =
        add_constraint(&mut engine, "vertical", &[(&a, "x"), (&b, "x")], None, None);
    assert_eq!(response["status"], "ok", "{response}");
    let view = snapshot(&mut engine);
    assert_eq!(rect_x(&view, &b), 100.0, "B snapped to A");
    assert_eq!(solver(&view)["variables"], 2, "two slots are interned");

    // One undo: the rule *and* the geometry it moved leave together, because the
    // solver's write belongs to the AddConstraint action.
    let response = undo(&mut engine);
    assert_eq!(response["status"], "ok", "{response}");
    let view = snapshot(&mut engine);
    assert!(view["constraints"].get(&constraint).is_none(), "rule gone");
    assert_eq!(rect_x(&view, &a), 100.0);
    assert_eq!(
        rect_x(&view, &b),
        300.0,
        "geometry is back to the unconstrained state"
    );
    assert_eq!(
        solver(&view)["variables"],
        0,
        "Cassowary's internal variable count decreased: {view}"
    );
    assert_eq!(solver(&view)["constraints"], 0);
    assert!(
        diagnostics_with_code(&view, "constraint-dropped").is_empty(),
        "undoing a rule is not a drop"
    );

    // Redo restores both, exactly.
    let response = redo(&mut engine);
    assert_eq!(response["status"], "ok", "{response}");
    let view = snapshot(&mut engine);
    assert_eq!(view["constraints"][&constraint]["enabled"], true);
    assert_eq!(rect_x(&view, &b), 100.0, "constraint enforced again");
    assert_eq!(solver(&view)["variables"], 2, "and its variables are back");
}

// ── L4: the solver never writes variables — it breaks the link, loudly ────

#[test]
fn law_parametric_link_is_broken_and_restorable() {
    let mut engine = VectraEngine::new();
    let a = create_rect(&mut engine, "a", 100.0);
    let b = create_rect(&mut engine, "b", 0.0);
    dispatch(
        &mut engine,
        serde_json::json!({ "type": "SetVariable", "name": "base", "value": 300.0 }),
    );
    let response = dispatch(
        &mut engine,
        serde_json::json!({
            "type": "SetParameter", "node_id": b, "property": "x",
            "value": { "Float": { "Variable": "base" } },
        }),
    );
    assert_eq!(response["status"], "ok", "{response}");
    let edges_before = dependencies(&engine)["edges"].as_array().unwrap().len();
    assert_eq!(edges_before, 1, "B.x depends on $base");

    // The constraint has to move B.x, which is driven by a variable. Doctrine:
    // break the link, pin the slot, and say so.
    let (constraint, response) =
        add_constraint(&mut engine, "vertical", &[(&a, "x"), (&b, "x")], None, None);
    assert_eq!(response["status"], "ok", "{response}");

    let view = snapshot(&mut engine);
    let broken = diagnostics_with_code(&view, "parametric-link-broken");
    assert_eq!(broken.len(), 1, "the break is reported: {view}");
    assert_eq!(broken[0]["property"], "x");
    assert_eq!(broken[0]["node_id"].as_str().unwrap(), b);
    assert_eq!(
        rect_x(&view, &b),
        100.0,
        "B.x was pinned to the solved value"
    );
    assert_eq!(
        view["variables"]["base"], 300.0,
        "the solver never writes Document.variables"
    );
    assert_eq!(
        dependencies(&engine)["edges"].as_array().unwrap().len(),
        0,
        "the parametric link is gone — B is a literal slot now"
    );
    assert_eq!(solver(&view)["variables"], 2);

    // The break is part of the action: undo restores geometry *and* the link.
    let response = undo(&mut engine);
    assert_eq!(response["status"], "ok", "{response}");
    let view = snapshot(&mut engine);
    assert!(view["constraints"].get(&constraint).is_none());
    assert_eq!(rect_x(&view, &b), 300.0, "B.x resolves from $base again");
    assert_eq!(
        dependencies(&engine)["edges"].as_array().unwrap().len(),
        1,
        "undo re-linked B.x to $base"
    );
    assert!(
        diagnostics_with_code(&view, "parametric-link-broken").is_empty(),
        "the diagnostic is not re-emitted once the link is back"
    );
}

// ── deletion: no dangling rules ───────────────────────────────────────────

#[test]
fn law_deleting_a_node_withdraws_its_constraints() {
    let mut engine = VectraEngine::new();
    let a = create_rect(&mut engine, "a", 100.0);
    let b = create_rect(&mut engine, "b", 300.0);
    let (constraint, response) =
        add_constraint(&mut engine, "vertical", &[(&a, "x"), (&b, "x")], None, None);
    assert_eq!(response["status"], "ok", "{response}");

    let response = dispatch(
        &mut engine,
        serde_json::json!({ "type": "DeleteNode", "id": b }),
    );
    assert_eq!(response["status"], "ok", "{response}");
    let view = snapshot(&mut engine);
    assert!(
        view["constraints"].get(&constraint).is_none(),
        "a deleted node must not leave a dangling rule: {view}"
    );
    assert!(view["scene"]["nodes"].get(&b).is_none());

    // Undo brings the node and its rule back — the rule is enforced again.
    let response = undo(&mut engine);
    assert_eq!(response["status"], "ok", "{response}");
    let view = snapshot(&mut engine);
    assert_eq!(view["constraints"][&constraint]["enabled"], true);
    assert_eq!(rect_x(&view, &b), 100.0, "the restored rule is in force");
    assert_eq!(solver(&view)["variables"], 2);
}

// ── idempotence + determinism ─────────────────────────────────────────────

#[test]
fn prop_solving_is_idempotent_and_deterministic() {
    // Fixed ids: "byte-identical for the same command stream" can only mean
    // anything if the stream itself is identical.
    let fixed = |n: u32| format!("00000000-0000-4000-8000-{n:012}");
    let script = |engine: &mut VectraEngine, counter: &mut u32| -> Vec<String> {
        let next = |counter: &mut u32| {
            *counter += 1;
            fixed(*counter)
        };
        let mut snapshots = Vec::new();
        let a = next(counter);
        let b = next(counter);
        let c = next(counter);
        for (node, name, x) in [(&a, "a", 10.0), (&b, "b", 40.0), (&c, "c", 90.0)] {
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
        snapshots.push(engine.get_snapshot());

        let mut ops: Vec<serde_json::Value> = vec![
            serde_json::json!({ "type": "SetParameter", "node_id": a, "property": "x",
                "value": { "Float": { "Literal": 12.0 } } }),
            serde_json::json!({ "type": "AddConstraint", "constraint": {
                "id": next(counter), "kind": "vertical",
                "targets": [ { "node_id": a, "property": "x" }, { "node_id": b, "property": "x" } ] } }),
            serde_json::json!({ "type": "AddConstraint", "constraint": {
                "id": next(counter), "kind": "vertical",
                "targets": [ { "node_id": b, "property": "x" }, { "node_id": c, "property": "x" } ],
                "strength": "strong" } }),
            serde_json::json!({ "type": "SetParameter", "node_id": c, "property": "x",
                "value": { "Float": { "Literal": -5.0 } } }),
        ];
        for op in ops.drain(..) {
            let response = dispatch(engine, op);
            assert_eq!(response["status"], "ok", "{response}");
            snapshots.push(engine.get_snapshot());
        }
        // A no-op dispatches: re-solving an already-satisfied system must write
        // nothing and must not perturb the snapshot.
        let before = engine.get_snapshot();
        let response = dispatch(
            engine,
            serde_json::json!({ "type": "SetVariable", "name": "unrelated", "value": 1.0 }),
        );
        assert_eq!(response["status"], "ok");
        let after = engine.get_snapshot();
        let strip = |json: &str| -> serde_json::Value {
            let mut value: serde_json::Value = serde_json::from_str(json).unwrap();
            value["variables"] = serde_json::Value::Null;
            // History bookkeeping is not the projection this law is about: the
            // no-op command between the two snapshots *is* an edit (it defines a
            // variable), so it adds an entry — the geometry is what must not
            // move. `undo_depth`/`redo_depth` joined the wire in Task 6.0.
            value["can_undo"] = serde_json::Value::Null;
            value["undo_depth"] = serde_json::Value::Null;
            value["redo_depth"] = serde_json::Value::Null;
            value["eval"] = serde_json::Value::Null;
            // Counters, not state: `writes` legitimately drops back to zero once
            // the system is settled.
            value["solver"] = serde_json::Value::Null;
            value
        };
        assert_eq!(
            strip(&before),
            strip(&after),
            "a solve that changes nothing must leave the projection alone"
        );
        snapshots.push(after.to_string());
        snapshots
    };

    let (mut first, mut second) = (VectraEngine::new(), VectraEngine::new());
    let run_one = script(&mut first, &mut 0);
    let run_two = script(&mut second, &mut 0);
    assert_eq!(run_one.len(), run_two.len());
    for (one, two) in run_one.iter().zip(run_two.iter()) {
        assert_eq!(one, two, "the same command stream must be byte-identical");
    }
    // The last snapshot: A→B→C vertical, C dragged to −5 pins the chain.
    let view: serde_json::Value = serde_json::from_str(run_one.last().unwrap()).unwrap();
    let ids: BTreeSet<String> = view["scene"]["nodes"]
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect();
    let xs: Vec<f64> = ids
        .iter()
        .map(|node| {
            view["scene"]["nodes"][node]["primitive"]["x"]
                .as_f64()
                .unwrap()
        })
        .collect();
    assert_eq!(
        xs,
        vec![-5.0, -5.0, -5.0],
        "a chain of vertical constraints converges on the dragged slot"
    );
}

// ── the typed error is reachable from the boundary ────────────────────────

#[test]
fn law_required_contradiction_is_a_typed_error() {
    let mut engine = VectraEngine::new();
    let a = create_rect(&mut engine, "a", 0.0);
    let b = create_rect(&mut engine, "b", 100.0);
    assert_eq!(
        add_constraint(
            &mut engine,
            "distance",
            &[(&a, "x"), (&b, "x")],
            Some("required"),
            Some(100.0)
        )
        .1["status"],
        "ok"
    );
    let (_, response) = add_constraint(
        &mut engine,
        "distance",
        &[(&a, "x"), (&b, "x")],
        Some("required"),
        Some(200.0),
    );
    assert_eq!(response["status"], "error");
    assert!(
        response["message"]
            .as_str()
            .unwrap()
            .starts_with("unsatisfiable constraints:"),
        "the boundary carries the typed error: {response}"
    );
    // `VectraError::UnsatisfiableConstraints` is the core-side type of that
    // message, and it is what the UI keys its red log line off.
    let typed = VectraError::unsatisfiable("two required rows disagree");
    assert!(typed.is_constraint_conflict());
    assert_eq!(response["events"], serde_json::Value::Null);
}
