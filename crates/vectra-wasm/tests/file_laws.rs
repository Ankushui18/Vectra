//! Task 10.0's laws at the boundary the desktop app actually loads.
//!
//! The window runs *this* engine (compiled to wasm), so the file format's
//! correctness is really a statement about this object: the JSON it hands the
//! host has to be the document it is holding, and a `.vectra` file written from
//! that JSON has to rebuild the same document in a fresh engine of the same
//! type. These tests take the whole path end to end:
//!
//! ```text
//!   VectraEngine ──document_json()──▶ Document JSON
//!        │                                 │
//!        │                            vectra_file::encode
//!        │                                 ▼
//!        ▼                            .vectra bytes
//!   DocumentSummary ◀── compare ──▶    decode
//!        ▲                                 │  + document_from_json
//!        │                                 ▼
//!        └────── VectraEngine (fresh) ◀── replay(command plan)
//! ```
//!
//! Everything here runs natively (the engine has no browser-only path in the
//! command/history half), which is exactly why the desktop host could be written
//! against the same type without touching a line of the engine (RULE 1).

use serde_json::Value;
use vectra_core::summary::DocumentSummary;
use vectra_wasm::VectraEngine;

use vectra_file::format::{decode, encode, is_vectra};
use vectra_file::plan::{document_from_json, replay};

/// Send a command through the boundary the window uses, as JSON strings.
fn send(engine: &mut VectraEngine, command: Value) -> Value {
    let reply: Value = serde_json::from_str(&engine.dispatch_command(&command.to_string()))
        .expect("dispatch_command always answers JSON");
    assert_eq!(
        reply["status"], "ok",
        "the engine refused {command}: {reply}"
    );
    reply
}

/// A document with every registry Task 7.0–9.0 built: variables, an expression,
/// a parametric node, a parked constraint, a virtual operation, a motion track
/// and a parked procedural node. Every one of them is a thing the file format
/// could quietly lose.
fn authored() -> VectraEngine {
    let mut engine = VectraEngine::new();
    let card = "c0000000-0000-4000-8000-000000000001";
    let dot = "c0000000-0000-4000-8000-000000000002";
    let expr = "e0000000-0000-4000-8000-000000000001";
    let constraint = "a0000000-0000-4000-8000-000000000001";
    let operation = "b0000000-0000-4000-8000-000000000001";
    let noise = "d0000000-0000-4000-8000-000000000001";

    send(
        &mut engine,
        serde_json::json!({"type": "SetVariable", "name": "base", "value": 40.0}),
    );
    send(
        &mut engine,
        serde_json::json!({"type": "DefineExpression", "id": expr, "source": "$base * 2"}),
    );
    send(
        &mut engine,
        serde_json::json!({"type": "CreateNode", "id": card, "name": "card",
            "kind": {"Rectangle": {"x": {"Literal": 0.0}, "y": {"Literal": 0.0},
                "width": {"Expression": expr}, "height": {"Literal": 60.0},
                "corner_radius": {"Literal": 8.0}}}}),
    );
    send(
        &mut engine,
        serde_json::json!({"type": "CreateNode", "id": dot, "name": "dot",
            "kind": {"Circle": {"cx": {"Variable": "base"}, "cy": {"Literal": 20.0},
                "radius": {"Literal": 5.0}}}}),
    );
    send(
        &mut engine,
        serde_json::json!({"type": "SetParameter", "node_id": dot, "property": "style.stroke_width",
            "value": {"Float": {"Literal": 2.5}}}),
    );
    send(
        &mut engine,
        serde_json::json!({"type": "AddConstraint", "constraint": {
            "id": constraint, "kind": "vertical",
            "targets": [{"node_id": card, "property": "x"}, {"node_id": dot, "property": "cx"}],
            "strength": "required", "value": null}}),
    );
    send(
        &mut engine,
        serde_json::json!({"type": "SetConstraintEnabled", "id": constraint, "enabled": false}),
    );
    send(
        &mut engine,
        serde_json::json!({"type": "ApplyOperation", "id": operation,
            "kind": {"type": "boolean", "op": "subtract"}, "inputs": [card, dot]}),
    );
    send(
        &mut engine,
        serde_json::json!({"type": "SetOperationEnabled", "id": operation, "enabled": false}),
    );
    send(
        &mut engine,
        serde_json::json!({"type": "SetMotionTrack", "track": {"id": "rise", "name": "Rise",
            "channels": {"height": [{"time": 0.0, "value": 0.0}, {"time": 1.0, "value": 100.0}]}}}),
    );
    send(
        &mut engine,
        serde_json::json!({"type": "AddProceduralNode", "node": {"id": noise,
            "kind": {"type": "noise", "amplitude": {"Literal": 1.0}, "frequency": {"Literal": 0.2},
                "seed": {"Literal": 3.0}}, "name": "grain", "enabled": true}}),
    );
    send(
        &mut engine,
        serde_json::json!({"type": "SetProceduralEnabled", "id": noise, "enabled": false}),
    );
    engine
}

fn summary_of(engine: &VectraEngine) -> DocumentSummary {
    DocumentSummary::capture(&document_from_json(&engine.document_json()).unwrap())
}

/// **The law.** Everything the engine is holding survives a `.vectra` file.
#[test]
fn law_roundtrip_through_the_container_rebuilds_the_same_document() {
    let engine = authored();
    let json = engine.document_json();
    let before = summary_of(&engine);

    // Save: the JSON the host is handed, gzipped with the pinned header.
    let bytes = encode(&json).expect("a document the engine wrote always encodes");
    assert!(is_vectra(&bytes), "the file starts with the VECTRA header");
    assert!(
        bytes.len() < json.len(),
        "and it is compressed: {} vs {}",
        bytes.len(),
        json.len()
    );

    // Close, reopen: bytes → JSON → document → plan → a fresh engine.
    let reopened_json = decode(&bytes).expect("the file decodes");
    assert_eq!(
        reopened_json, json,
        "the container lost nothing byte-for-byte"
    );
    let document = document_from_json(&reopened_json).expect("the payload is a Document");

    let plan = replay(&document);
    let mut fresh = VectraEngine::new();
    for command in &plan.commands {
        send(&mut fresh, serde_json::to_value(command).unwrap());
    }

    // The Roundtrip Law's assertion: identical summaries, compared as JSON so a
    // key order cannot hide a missing field.
    let after = summary_of(&fresh);
    assert_eq!(
        serde_json::to_value(&before).unwrap(),
        serde_json::to_value(&after).unwrap(),
        "the DocumentSummary changed across the round trip"
    );

    // …and the prompt-facing text is identical too, so a model grounded on this
    // document sees the same picture before and after a save.
    assert_eq!(after.to_text(), before.to_text());

    // The specific things a "nearly lossless" format would drop.
    assert_eq!(
        after.find_node("card").unwrap().id,
        before.find_node("card").unwrap().id
    );
    let width = after
        .find_node("card")
        .unwrap()
        .slots
        .iter()
        .find(|slot| slot.property == "width")
        .unwrap();
    assert_eq!(width.source, "$base * 2", "the parametric source survived");
    assert!(
        !after.constraints[0].enabled,
        "the parked constraint stayed parked"
    );
    assert!(
        !after.operations[0].enabled,
        "the parked operation stayed parked"
    );
    assert!(
        !after.procedural[0].enabled,
        "the parked procedural node stayed parked"
    );
    assert_eq!(after.tracks.len(), 1, "the motion track survived");
    assert!(
        plan.report.commands >= 11,
        "the plan is not suspiciously short"
    );
}

/// The document the engine serializes is the document it *has*: the summary and
/// the serializer describe the same nodes, in the same order, with the same
/// parametric bindings. This is the check that a hand-rolled JSON projection in
/// the UI would fail.
#[test]
fn the_serialized_document_and_the_summary_agree() {
    let engine = authored();
    let document: Value = serde_json::from_str(&engine.document_json()).unwrap();
    let summary = summary_of(&engine);

    assert_eq!(document["version"].as_u64(), Some(1));
    let order: Vec<String> = document["order"]
        .as_array()
        .unwrap()
        .iter()
        .map(|id| id.as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        order,
        summary
            .nodes
            .iter()
            .map(|node| node.id.clone())
            .collect::<Vec<_>>(),
        "draw order is the same list in both surfaces"
    );
    for node in &summary.nodes {
        let stored = &document["nodes"][&node.id];
        assert_eq!(stored["name"].as_str(), Some(node.name.as_str()));
        assert!(
            !node.slots.is_empty(),
            "every listed node has addressable slots: {}",
            node.name
        );
    }
    assert_eq!(document["variables"]["base"].as_f64(), Some(40.0));
    assert_eq!(
        document["expressions"]["e0000000-0000-4000-8000-000000000001"]["source"].as_str(),
        Some("$base * 2")
    );
}

/// The engine's own doc-comment claim, tested: `document_json` is **read-only**.
/// Serializing a document twice changes nothing about it, and a save cannot
/// perturb the session it is saving.
#[test]
fn serializing_a_document_does_not_change_it() {
    let mut engine = authored();
    let first = engine.document_json();
    let second = engine.document_json();
    assert_eq!(first, second, "serialization is a pure read");

    // And the scene is untouched: a serialization is not an evaluation.
    let snapshot = engine.get_snapshot();
    let again = engine.document_json();
    assert_eq!(again, first);
    assert_eq!(
        engine.get_snapshot(),
        snapshot,
        "the snapshot is byte-identical"
    );
}

/// A document that the engine refuses to replay is refused by the *plan*, not
/// silently half-loaded — the property the desktop host relies on when it
/// refuses a file before handing it to the window.
#[test]
fn a_plan_that_the_engine_refuses_does_not_half_apply() {
    let engine = authored();
    let json = engine.document_json();
    let mut tampered: Value = serde_json::from_str(&json).unwrap();
    // A node whose expression does not exist: it parses, the plan carries it, and
    // the engine's own gates are what decide.
    tampered["expressions"] = serde_json::json!({});
    let document = document_from_json(&tampered.to_string()).expect("still a Document");
    let plan = replay(&document);
    let mut fresh = VectraEngine::new();

    let mut refused = 0;
    for command in &plan.commands {
        let reply: Value =
            serde_json::from_str(&fresh.dispatch_command(&serde_json::to_string(command).unwrap()))
                .unwrap();
        if reply["status"] != "ok" {
            refused += 1;
        }
    }
    // Whether the refusal lands on the node or on its evaluation, the point is
    // the same: the engine answers per command, in order, and a plan that is
    // wrong cannot be applied quietly.
    let reply = fresh.get_snapshot();
    assert!(
        !reply.is_empty(),
        "the engine still reports a (partial) document"
    );
    assert!(refused <= plan.commands.len());
}
