//! Procedural laws at the **protocol boundary** (Task 7.0) — the `vectra-wasm`
//! half of the procedural contract.
//!
//! `crates/vectra-procedural/tests/procedural_laws.rs` proves the seven laws at
//! the engine level, driving the production pipeline directly.
//! `crates/vectra-core/tests/…` proves the command algebra and the port gate.
//! This file proves what React actually consumes, over the JSON seam and nothing
//! else:
//!
//! * the wire shape (`AddProceduralNode` parses; `procedural_json` reports the
//!   chain, its effective operands, and what each port last published),
//! * **RULE 2** end to end: a slot that reads a value port follows the port in
//!   the *same* `settle` — the reader round-trip, over the protocol,
//! * **RULE 4** end to end: a procedural result is a real scene node, survives an
//!   unrelated operation pass, and is *nameable* in the snapshot,
//! * **RULE 1/3** over the wire: a typed port rejection and a disguised cycle
//!   both leave the document byte-identical (the snapshot is the witness),
//! * the undo law: a procedural command's inverse restores the document exactly,
//!   and `patch ≡ rebuild` still holds with a procedural graph in the scene.

use serde_json::{json, Value};
use vectra_core::{new_node_id, ProceduralKind, ProceduralNode};
use vectra_wasm::VectraEngine;

fn id() -> String {
    new_node_id().to_string()
}

fn dispatch(engine: &mut VectraEngine, cmd: Value) -> Value {
    serde_json::from_str(&engine.dispatch_command(&serde_json::to_string(&cmd).unwrap())).unwrap()
}

fn snapshot(engine: &mut VectraEngine) -> Value {
    serde_json::from_str(&engine.get_snapshot()).unwrap()
}

fn procedural(engine: &mut VectraEngine) -> Value {
    serde_json::from_str(&engine.procedural_json()).unwrap()
}

/// The document, as the UI can see it: the volatile counters stripped, so two
/// snapshots can be compared for equality.
fn stable_snapshot(engine: &mut VectraEngine) -> Value {
    let mut view = snapshot(engine);
    let object = view.as_object_mut().unwrap();
    for key in [
        "variables",
        "can_undo",
        "can_redo",
        "undo_depth",
        "redo_depth",
        "eval",
        "solver",
    ] {
        object.remove(key);
    }
    view
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

/// Add a procedural node; returns (id, response).
///
/// The payload is built from a real [`ProceduralNode`] and then serialized, so
/// the test states the *kind* and the command `AddProceduralNode` instead of
/// re-spelling the record's every field (name, style, `enabled`) — those are
/// core's wire, and its own tests own them. What crosses here is still JSON
/// text, through the same seam React uses.
fn add(engine: &mut VectraEngine, kind: Value) -> (String, Value) {
    let kind: ProceduralKind = serde_json::from_value(kind).expect("the kind parses");
    let node = ProceduralNode::new(new_node_id(), kind);
    let node_id = node.id.to_string();
    let payload = serde_json::to_value(&node).expect("the record serializes");
    let response = dispatch(
        engine,
        json!({"type": "AddProceduralNode", "node": payload}),
    );
    (node_id, response)
}

/// Wire `port` of `node` to `from`'s `port_out`.
fn connect(engine: &mut VectraEngine, node: &str, port: &str, from: &str, port_out: &str) -> Value {
    dispatch(
        engine,
        json!({"type": "ConnectProcedural", "node_id": node, "port": port,
               "from": {"node": from, "port": port_out}}),
    )
}

/// Add a node the test expects to exist (the loud version of [`add`]).
fn add_ok(engine: &mut VectraEngine, kind: Value) -> String {
    let (node, response) = add(engine, kind);
    assert_eq!(response["status"], "ok", "{response}");
    node
}

fn grid(columns: f64, rows: f64, spacing: f64, ox: f64, oy: f64) -> Value {
    json!({
        "type": "grid",
        "columns": {"Literal": columns},
        "rows": {"Literal": rows},
        "spacing": {"Literal": spacing},
        "origin": {"Literal": {"x": ox, "y": oy}},
    })
}

fn scene_ids(view: &Value) -> Vec<String> {
    view["scene"]["z_order"]
        .as_array()
        .map(|ids| {
            ids.iter()
                .map(|v| v.as_str().unwrap().to_string())
                .collect()
        })
        .unwrap_or_default()
}

// ── the wire: the chain, its operands, its published values ────────────

#[test]
fn law_the_chain_round_trips_over_the_wire_in_order() {
    let mut engine = VectraEngine::new();
    let grid_id = add_ok(&mut engine, grid(3.0, 2.0, 10.0, 0.0, 0.0));
    let repeat = add_ok(
        &mut engine,
        json!({
        "type": "repeat",
        "count": {"Literal": 2.0},
        "dx": {"Literal": 60.0},
        "dy": {"Literal": 0.0}}),
    );

    let response = connect(&mut engine, &repeat, "region", &grid_id, "region");
    assert_eq!(response["status"], "ok", "{response}");
    assert!(
        response["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|event| event["type"] == "ProceduralUpdated"),
        "the mutation publishes a procedural update: {response}"
    );

    let view = procedural(&mut engine);
    assert_eq!(view["count"], 2, "{view}");
    let order: Vec<String> = view["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|node| node["id"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        order,
        vec![grid_id.clone(), repeat.clone()],
        "the registry order is the evaluation order, upstream first"
    );

    // Every declared port is on the wire, with its type and its published value.
    let grid_view = &view["nodes"][0];
    assert_eq!(grid_view["kind"], "grid");
    let outputs: Vec<&str> = grid_view["outputs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|port| port["port"].as_str().unwrap())
        .collect();
    assert_eq!(outputs, vec!["region", "points", "center", "span"]);
    assert_eq!(grid_view["outputs"][3]["ty"], "scalar");
    assert_eq!(
        grid_view["outputs"][3]["value"], "scalar 30",
        "3 cells, 10 apart: the published span, as the panel shows it"
    );
    assert_eq!(
        grid_view["outputs"][0]["value"], "region 1 rings, 4 pts",
        "the grid's region is the lattice's overall extent"
    );
    assert_eq!(
        grid_view["outputs"][1]["value"], "points ×12",
        "…and its points are the lattice corners: (3+1) × (2+1)"
    );
    assert!(
        grid_view["description"]
            .as_str()
            .unwrap()
            .starts_with("grid n=3 m=2 s=10"),
        "the effective operands, formatted: {grid_view}"
    );
    assert_eq!(
        grid_view["operands"]["columns"]["text"], "3",
        "the panel gets the operands as data, not just as prose"
    );
    assert_eq!(
        grid_view["operands"]["origin"]["ty"], "point",
        "…with the port's type, so the panel knows to offer two numbers"
    );

    // The consumer's view: what it reads, and whether its required input is fed.
    let repeat_view = &view["nodes"][1];
    assert_eq!(repeat_view["kind"], "repeat");
    assert_eq!(repeat_view["wires"]["region"], format!("{grid_id}:region"));
    assert_eq!(repeat_view["upstream"], json!([grid_id]));
    let region = repeat_view["inputs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|port| port["port"] == "region")
        .expect("a modifier declares a region input");
    assert_eq!(region["ty"], "region");
    assert_eq!(region["required"], true);
    assert_eq!(region["wired"], true);
    assert_eq!(repeat_view["outputs"][0]["ty"], "region");
}

#[test]
fn law_effective_operands_are_what_the_panel_shows() {
    let mut engine = VectraEngine::new();
    let grid_id = add_ok(&mut engine, grid(2.0, 2.0, 10.0, 0.0, 0.0));

    // A variable drives the spacing: the panel must say `(variable)`, never the
    // number the kind's template started with — a node's *effective* operands.
    dispatch(
        &mut engine,
        json!({"type": "SetVariable", "name": "gap", "value": 25.0}),
    );
    let response = dispatch(
        &mut engine,
        json!({"type": "SetProceduralOperand", "node_id": grid_id, "port": "spacing",
               "value": {"Float": {"Variable": "gap"}}}),
    );
    assert_eq!(response["status"], "ok", "{response}");

    let view = procedural(&mut engine);
    let node = &view["nodes"][0];
    assert_eq!(node["operands"]["spacing"]["text"], "(variable)");
    assert_eq!(node["operands"]["spacing"]["ty"], "scalar");
    assert_eq!(node["operands"]["columns"]["text"], "2");
    assert!(
        node["description"]
            .as_str()
            .unwrap()
            .contains("s=(variable)"),
        "{node}"
    );
    // …and the *published* value follows the variable, because the pass resolved
    // the operand through the expression registry.
    assert_eq!(node["outputs"][3]["value"], "scalar 50");

    dispatch(
        &mut engine,
        json!({"type": "SetVariable", "name": "gap", "value": 12.0}),
    );
    let view = procedural(&mut engine);
    assert_eq!(view["nodes"][0]["outputs"][3]["value"], "scalar 24");
}

/// The palette is the engine's own table: the panel adds a node by embedding
/// what this returns, so a drifted default is impossible.
#[test]
fn law_the_palette_is_the_engines_own_table() {
    let mut engine = VectraEngine::new();
    let raw = engine.procedural_kinds();
    let kinds: Value = serde_json::from_str(&raw).expect("the palette is JSON");
    let kinds = kinds.as_array().expect("a list of kinds");
    let tags: Vec<&str> = kinds
        .iter()
        .map(|kind| kind["tag"].as_str().unwrap())
        .collect();
    assert_eq!(tags, vec!["source", "grid", "repeat", "noise", "smooth"]);

    let grid = kinds
        .iter()
        .find(|kind| kind["tag"] == "grid")
        .expect("a grid kind");
    assert_eq!(grid["needs_subject"], false);
    let operand_ports: Vec<&str> = grid["operands"]
        .as_object()
        .unwrap()
        .keys()
        .map(|port| port.as_str())
        .collect();
    assert_eq!(operand_ports, vec!["columns", "origin", "rows", "spacing"]);
    // The payloads are **kind-level** (`Parameter<T>`, no `ParamValue` wrapper),
    // so the panel can spread them straight into the kind it is building. A
    // `{"Float": …}` / `{"Point": …}` wrapper here would be a shape the *kind*
    // cannot deserialize — the bug the smoke's step 32 exists to catch.
    assert_eq!(grid["operands"]["spacing"], json!({"Literal": 40.0}));
    assert!(grid["operands"]["spacing"].get("Float").is_none());
    assert_eq!(grid["operands"]["origin"]["Literal"]["x"], 0.0);
    assert!(grid["operands"]["origin"].get("Point").is_none());
    // …and building a node from the palette alone deserializes: the payload is
    // exactly what `AddProceduralNode` accepts.
    let from_palette = add_ok(
        &mut engine,
        json!({"type": "grid",
               "columns": grid["operands"]["columns"],
               "rows": grid["operands"]["rows"],
               "spacing": grid["operands"]["spacing"],
               "origin": grid["operands"]["origin"]}),
    );
    assert_eq!(
        procedural(&mut engine)["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|node| node["id"] == from_palette.as_str())
            .expect("the node added from the palette is registered")["kind"],
        "grid"
    );

    let source = kinds
        .iter()
        .find(|kind| kind["tag"] == "source")
        .expect("a source kind");
    assert_eq!(source["needs_subject"], true);
    assert!(source["operands"].as_object().unwrap().is_empty());

    // Adding a node from the palette alone — no operand, and the *engine* names
    // it (the wire omits `name`, `enabled` and `style` entirely).
    let rect = create_rect(&mut engine, "rect", 0.0, 0.0, 10.0, 10.0);
    let node = id();
    let mut payload = serde_json::Map::new();
    payload.insert("id".to_string(), json!(node));
    payload.insert("kind".to_string(), json!({"type": "source", "node": rect}));
    let response = dispatch(
        &mut engine,
        json!({"type": "AddProceduralNode", "node": Value::Object(payload)}),
    );
    assert_eq!(response["status"], "ok", "{response}");
    let view = procedural(&mut engine);
    let added = view["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|candidate| candidate["id"] == node.as_str())
        .expect("the node added from the palette is registered");
    assert!(
        added["name"].as_str().unwrap().starts_with("source"),
        "the engine named it: {view}"
    );
    assert_eq!(
        added["outputs"][0]["ty"], "region",
        "and the palette's port list is the report's"
    );
}

// ── RULE 2 over the wire: a slot follows a published port ──────────────

#[test]
fn law_a_slot_follows_a_port_in_the_same_settle() {
    let mut engine = VectraEngine::new();
    let rect = create_rect(&mut engine, "rect", 0.0, 0.0, 10.0, 10.0);
    let grid_id = add_ok(&mut engine, grid(2.0, 3.0, 10.0, 0.0, 0.0));

    let response = dispatch(
        &mut engine,
        json!({"type": "SetParameter", "node_id": rect, "property": "width",
               "value": {"Float": {"Procedural": {"node": grid_id, "port": "span"}}}}),
    );
    assert_eq!(response["status"], "ok", "{response}");

    // The first read of a freshly bound port: `get_snapshot` warms the cache, so
    // bind first, then look.
    let view = snapshot(&mut engine);
    let cold = view["scene"]["nodes"][&rect]["primitive"]["w"]
        .as_f64()
        .expect("the rect is a rect");
    assert!((cold - 20.0).abs() < 1e-9, "2 × 10 = 20, got {cold}");

    // Now move the port the slot reads. The *same* dispatch must leave the slot
    // resolved against the new value — no extra settle, no second frame.
    let response = dispatch(
        &mut engine,
        json!({"type": "SetProceduralOperand", "node_id": grid_id, "port": "spacing",
               "value": {"Float": {"Literal": 25.0}}}),
    );
    assert_eq!(response["status"], "ok", "{response}");
    let view = snapshot(&mut engine);
    let width = view["scene"]["nodes"][&rect]["primitive"]["w"]
        .as_f64()
        .expect("the rect is still a rect");
    assert!(
        (width - 50.0).abs() < 1e-9,
        "2 × 25 = 50 — the reader followed in the same settle, got {width}"
    );
    let driven = view["scene"]["nodes"][&rect]["position"]["x_source"]
        .as_str()
        .map(ToString::to_string)
        .unwrap_or_default();
    assert_ne!(driven, "procedural", "width is what the port drives, not x");
}

// ── RULE 4: the result is real scene geometry, and it is nameable ──────

#[test]
fn law_a_procedural_result_survives_an_operation_pass_on_the_wire() {
    let mut engine = VectraEngine::new();
    let a = create_rect(&mut engine, "a", 0.0, 0.0, 40.0, 30.0);
    let b = create_rect(&mut engine, "b", 20.0, 10.0, 40.0, 30.0);
    let grid_id = add_ok(&mut engine, grid(2.0, 2.0, 15.0, 100.0, 100.0));

    let view = snapshot(&mut engine);
    assert!(
        scene_ids(&view).contains(&grid_id),
        "the procedural result draws: {:?}",
        scene_ids(&view)
    );
    let scene_name = view["scene"]["nodes"][&grid_id]["name"]
        .as_str()
        .expect("the scene entry has a name")
        .to_string();
    assert_eq!(
        scene_name, view["procedural"][&grid_id]["name"],
        "…and the inspector names it the way the registry does (RULE 4's other half)"
    );
    assert_ne!(scene_name, grid_id, "never a bare uuid: {scene_name}");
    assert_eq!(
        view["scene"]["nodes"][&grid_id]["primitive"]["type"], "path",
        "a region leaves the pass as a path primitive"
    );

    // An unrelated boolean: `apply_operations` prunes every cached id failing
    // `is_geometry_id`, which is exactly where a procedural result would be
    // silently evicted if the document did not list it.
    let union = id();
    let response = dispatch(
        &mut engine,
        json!({"type": "ApplyOperation", "id": union,
               "kind": {"type": "boolean", "op": "union"}, "inputs": [a, b]}),
    );
    assert_eq!(response["status"], "ok", "{response}");
    let ids = scene_ids(&snapshot(&mut engine));
    assert!(ids.contains(&union), "the union composed: {ids:?}");
    assert!(
        ids.contains(&grid_id),
        "…and the procedural result is still drawn: {ids:?}"
    );
    assert_eq!(
        procedural(&mut engine)["diagnostics"]
            .as_array()
            .unwrap()
            .len(),
        0,
        "no diagnostics from a healthy graph"
    );
    // The operation pass did not disturb the procedural registry either.
    assert_eq!(procedural(&mut engine)["count"], 1);
}

#[test]
fn law_a_parked_node_is_retired_and_re_armed_by_the_protocol() {
    let mut engine = VectraEngine::new();
    let grid_id = add_ok(&mut engine, grid(2.0, 2.0, 15.0, 0.0, 0.0));
    assert!(scene_ids(&snapshot(&mut engine)).contains(&grid_id));

    let response = dispatch(
        &mut engine,
        json!({"type": "SetProceduralEnabled", "id": grid_id, "enabled": false}),
    );
    assert_eq!(response["status"], "ok", "{response}");
    assert!(
        !scene_ids(&snapshot(&mut engine)).contains(&grid_id),
        "parked ⇒ the geometry leaves the scene"
    );
    let view = procedural(&mut engine);
    assert_eq!(view["nodes"][0]["enabled"], false);
    assert!(
        view["nodes"][0]["description"]
            .as_str()
            .unwrap()
            .contains("parked"),
        "…and the description says so: {view}"
    );

    let response = dispatch(
        &mut engine,
        json!({"type": "SetProceduralEnabled", "id": grid_id, "enabled": true}),
    );
    assert_eq!(response["status"], "ok", "{response}");
    assert!(scene_ids(&snapshot(&mut engine)).contains(&grid_id));
}

// ── RULE 1 + RULE 3 over the wire ─────────────────────────────────────

#[test]
fn law_mismatched_ports_and_disguised_cycles_change_nothing() {
    let mut engine = VectraEngine::new();
    let rect = create_rect(&mut engine, "rect", 0.0, 0.0, 10.0, 10.0);
    let source = add_ok(&mut engine, json!({"type": "source", "node": rect}));
    let grid_id = add_ok(&mut engine, grid(3.0, 2.0, 10.0, 0.0, 0.0));
    let smooth = add_ok(
        &mut engine,
        json!({
        "type": "smooth",
        "iterations": {"Literal": 2.0},
        "strength": {"Literal": 0.5}}),
    );
    assert_eq!(procedural(&mut engine)["count"], 3);

    // RULE 1: `span` is a scalar port; a modifier's `region` input is a region.
    let before = stable_snapshot(&mut engine);
    let response = connect(&mut engine, &smooth, "region", &grid_id, "span");
    assert_eq!(response["status"], "error", "{response}");
    assert!(
        response["message"]
            .as_str()
            .unwrap()
            .to_ascii_lowercase()
            .contains("scalar"),
        "the message names the type the port carries: {response}"
    );
    assert_eq!(
        stable_snapshot(&mut engine),
        before,
        "a refused command leaves the document bit-identical"
    );

    // RULE 3: an operand that reads a procedural output is a cycle wearing a
    // disguise — refused at the command boundary, because the graph's gate
    // cannot see it.
    let response = dispatch(
        &mut engine,
        json!({"type": "SetProceduralOperand", "node_id": smooth, "port": "iterations",
               "value": {"Float": {"Procedural": {"node": grid_id, "port": "region"}}}}),
    );
    assert_eq!(response["status"], "error", "{response}");
    assert_eq!(stable_snapshot(&mut engine), before, "still bit-identical");

    // A real wire cycle: grid → smooth → grid.
    let response = connect(&mut engine, &smooth, "region", &grid_id, "region");
    assert_eq!(response["status"], "ok", "{response}");
    let before = stable_snapshot(&mut engine);
    let response = connect(&mut engine, &grid_id, "region", &smooth, "region");
    assert_eq!(response["status"], "error", "{response}");
    assert!(
        response["message"]
            .as_str()
            .unwrap()
            .to_ascii_lowercase()
            .contains("cycl"),
        "the message says cycle: {response}"
    );
    assert_eq!(stable_snapshot(&mut engine), before, "still bit-identical");
    assert_eq!(
        snapshot(&mut engine)["can_undo"],
        true,
        "the history still has the legitimate commands, nothing more"
    );

    // The source node is part of the chain too: deleting the rect it reads
    // withdraws the source, and the consumer says so instead of drawing stale
    // geometry (the `DeleteNode` withdrawal path, over the wire).
    let _ = source;
}

// ── the undo law and patch ≡ rebuild, with a graph in the scene ────────

#[test]
fn law_a_procedural_command_undoes_exactly() {
    let mut engine = VectraEngine::new();
    let rect = create_rect(&mut engine, "rect", 0.0, 0.0, 40.0, 30.0);
    let source = add_ok(&mut engine, json!({"type": "source", "node": rect}));
    let repeat = add_ok(
        &mut engine,
        json!({
        "type": "repeat",
        "count": {"Literal": 3.0},
        "dx": {"Literal": 60.0},
        "dy": {"Literal": 0.0}}),
    );
    let response = connect(&mut engine, &repeat, "region", &source, "region");
    assert_eq!(response["status"], "ok", "{response}");

    let painted = stable_snapshot(&mut engine);
    assert!(
        scene_ids(&painted).contains(&repeat),
        "the chain draws: {:?}",
        scene_ids(&painted)
    );

    // An operand edit, undone: the document and the picture come back exactly.
    dispatch(
        &mut engine,
        json!({"type": "SetProceduralOperand", "node_id": repeat, "port": "dx",
               "value": {"Float": {"Literal": 90.0}}}),
    );
    assert_ne!(stable_snapshot(&mut engine), painted, "the edit changed it");
    assert_eq!(
        serde_json::from_str::<Value>(&engine.undo()).unwrap()["status"],
        "ok"
    );
    assert_eq!(
        stable_snapshot(&mut engine),
        painted,
        "undo restored the document and the picture exactly"
    );

    // A removal, undone — *while wired*: the inverse is a batch of the record
    // plus the wires other nodes had into it, so the graph comes back whole
    // rather than as a node with a hole.
    dispatch(
        &mut engine,
        json!({"type": "RemoveProceduralNode", "id": repeat}),
    );
    let removed = stable_snapshot(&mut engine);
    assert!(
        !scene_ids(&removed).contains(&repeat),
        "…and it stops drawing"
    );
    assert_eq!(procedural(&mut engine)["count"], 1);
    assert_eq!(
        serde_json::from_str::<Value>(&engine.undo()).unwrap()["status"],
        "ok"
    );
    assert_eq!(
        stable_snapshot(&mut engine),
        painted,
        "undo restored the node, its operands and its wire"
    );
    let view = procedural(&mut engine);
    let restored = view["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["id"] == repeat.as_str())
        .expect("the restored node is in the registry");
    assert_eq!(
        restored["wires"]["region"],
        format!("{source}:region"),
        "…and the wire is the one it had"
    );

    // Redo removes it again, with the same consequence: no geometry, no wire.
    assert_eq!(
        serde_json::from_str::<Value>(&engine.redo()).unwrap()["status"],
        "ok"
    );
    assert_eq!(procedural(&mut engine)["count"], 1);
    assert!(!scene_ids(&stable_snapshot(&mut engine)).contains(&repeat));
}

#[test]
fn law_a_full_rebuild_equals_the_incremental_scene() {
    let mut engine = VectraEngine::new();
    let rect = create_rect(&mut engine, "rect", 0.0, 0.0, 40.0, 30.0);
    let source = add_ok(&mut engine, json!({"type": "source", "node": rect}));
    let noise = add_ok(
        &mut engine,
        json!({
        "type": "noise",
        "amplitude": {"Literal": 1.0},
        "frequency": {"Literal": 0.2},
        "seed": {"Literal": 7.0}}),
    );
    dispatch(
        &mut engine,
        json!({"type": "ConnectProcedural", "node_id": noise, "port": "region",
               "from": {"node": source, "port": "region"}}),
    );
    // A slot reading the noise node's `tint` (the colour door).
    let tinted = add_ok(
        &mut engine,
        json!({
        "type": "repeat",
        "count": {"Literal": 2.0},
        "dx": {"Literal": 20.0},
        "dy": {"Literal": 0.0}}),
    );
    dispatch(
        &mut engine,
        json!({"type": "ConnectProcedural", "node_id": tinted, "port": "region",
               "from": {"node": noise, "port": "region"}}),
    );
    dispatch(
        &mut engine,
        json!({"type": "SetParameter", "node_id": rect, "property": "style.fill",
               "value": {"Color": {"Procedural": {"node": noise, "port": "tint"}}}}),
    );

    let incremental = stable_snapshot(&mut engine);
    let response: Value = serde_json::from_str(&engine.force_full_evaluation()).unwrap();
    assert_eq!(response["status"], "ok", "{response}");
    assert_eq!(response["events"][0]["type"], "Dirty");
    assert_eq!(response["events"][0]["mode"], "full");
    let rebuilt = stable_snapshot(&mut engine);
    assert_eq!(
        incremental, rebuilt,
        "a full rebuild is byte-identical to the incremental scene (determinism, over the wire)"
    );

    // …and the noise node's published tint is what the rect's fill resolved to:
    // the colour door is wired into the scene, not just into the table.
    let view = procedural(&mut engine);
    let tint = view["nodes"][1]["outputs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|port| port["port"] == "tint")
        .and_then(|port| port["value"].as_str())
        .expect("the noise node published a tint")
        .to_string();
    let fill = snapshot(&mut engine)["scene"]["nodes"][&rect]["style"]["fill"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(
        fill, tint,
        "the scene's fill is the published tint, formatted the same way"
    );
}

#[test]
fn law_an_unknown_node_or_port_is_a_typed_rejection() {
    let mut engine = VectraEngine::new();
    let grid_id = add_ok(&mut engine, grid(2.0, 2.0, 10.0, 0.0, 0.0));
    let ghost = id();

    // An unknown port on a real node.
    let response = dispatch(
        &mut engine,
        json!({"type": "ConnectProcedural", "node_id": grid_id, "port": "nope",
               "from": {"node": grid_id, "port": "span"}}),
    );
    assert_eq!(response["status"], "error", "{response}");

    // An unknown node.
    let response = dispatch(
        &mut engine,
        json!({"type": "SetProceduralOperand", "node_id": ghost, "port": "columns",
               "value": {"Float": {"Literal": 4.0}}}),
    );
    assert_eq!(response["status"], "error", "{response}");
    assert!(
        response["message"].as_str().unwrap().contains("procedural"),
        "the error names the registry it looked in: {response}"
    );

    // Removing the node that still feeds a wire: `DeleteNode` withdraws the
    // sources that read it, and the consumer says so instead of drawing stale
    // geometry.
    let rect = create_rect(&mut engine, "rect", 0.0, 0.0, 10.0, 10.0);
    let source = add_ok(&mut engine, json!({"type": "source", "node": rect}));
    let smooth = add_ok(
        &mut engine,
        json!({
        "type": "smooth",
        "iterations": {"Literal": 1.0},
        "strength": {"Literal": 0.5}}),
    );
    dispatch(
        &mut engine,
        json!({"type": "ConnectProcedural", "node_id": smooth, "port": "region",
               "from": {"node": source, "port": "region"}}),
    );
    assert!(scene_ids(&snapshot(&mut engine)).contains(&smooth));
    let response = dispatch(&mut engine, json!({"type": "DeleteNode", "id": rect}));
    assert_eq!(response["status"], "ok", "{response}");
    let view = snapshot(&mut engine);
    assert!(
        !scene_ids(&view).contains(&smooth),
        "the consumer stopped drawing when its source vanished"
    );
    assert!(
        view["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d["code"] == "procedural-missing-input"),
        "and it says why: {}",
        view["diagnostics"]
    );
    assert_eq!(
        procedural(&mut engine)["count"],
        2,
        "the graph keeps both records; the source node simply resolves to nothing"
    );
}
