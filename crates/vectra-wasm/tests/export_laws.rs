//! Task 8.0's laws, through the **compiled wasm module** and its JSON seam.
//!
//! The crate's own tests prove the exporters; this file proves that the boundary
//! publishes them — and, more importantly, that the two halves of the IR's input
//! meet here: the export is compiled from the document **and the live scene**, so
//! a motion-driven slot exports the number the canvas is drawing, and a
//! procedural result exports the region the graph computed.

use serde_json::{json, Value};
use vectra_wasm::VectraEngine;

fn id() -> String {
    vectra_core::new_node_id().to_string()
}

fn dispatch(engine: &mut VectraEngine, cmd: Value) -> Value {
    serde_json::from_str(&engine.dispatch_command(&serde_json::to_string(&cmd).unwrap())).unwrap()
}

fn export(engine: &mut VectraEngine, format: &str) -> Value {
    let raw = match format {
        "svg" => engine.export_to_svg(),
        "react" => engine.export_to_react(),
        other => panic!("unknown format {other}"),
    };
    serde_json::from_str(&raw).expect("the export envelope is JSON")
}

fn rect(engine: &mut VectraEngine, name: &str, x: f64, y: f64, w: f64, h: f64) -> String {
    let node = id();
    let response = dispatch(
        engine,
        json!({"type": "CreateNode", "id": node, "name": name,
               "kind": {"Rectangle": {
                   "x": {"Literal": x}, "y": {"Literal": y},
                   "width": {"Literal": w}, "height": {"Literal": h},
                   "corner_radius": {"Literal": 0.0}}}}),
    );
    assert_eq!(response["status"], "ok", "{response}");
    node
}

/// The envelope is one shape for both formats, and the code is *the file*.
#[test]
fn law_both_formats_come_out_of_one_envelope() {
    let mut engine = VectraEngine::new();
    rect(&mut engine, "box", 0.0, 0.0, 40.0, 30.0);

    let svg = export(&mut engine, "svg");
    assert_eq!(svg["status"], "ok");
    assert_eq!(svg["format"], "svg");
    let code = svg["code"].as_str().expect("code is a string");
    assert!(code.starts_with("<?xml"), "{}", &code[..40.min(code.len())]);
    assert!(code.contains("<rect"), "{code}");
    assert!(
        code.contains("stroke-width") || code.contains("fill="),
        "{code}"
    );
    assert_eq!(
        svg["warnings"],
        json!([]),
        "a plain rect has nothing to warn about"
    );

    let react = export(&mut engine, "react");
    assert_eq!(react["format"], "react");
    let tsx = react["code"].as_str().unwrap();
    assert!(tsx.contains("export function Scene"), "{tsx}");
    assert!(tsx.contains("<Rect"), "{tsx}");
}

/// RULE 1 at the boundary: a circle is a `<circle>`, and **nothing** in the file
/// is a bezier approximation of it.
#[test]
fn law_a_circle_is_never_approximated() {
    let mut engine = VectraEngine::new();
    let node = id();
    dispatch(
        &mut engine,
        json!({"type": "CreateNode", "id": node, "name": "dot",
               "kind": {"Circle": {
                   "cx": {"Literal": 10.0}, "cy": {"Literal": 20.0},
                   "radius": {"Literal": 5.0}}}}),
    );

    let code = export(&mut engine, "svg")["code"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(code.contains("<circle"), "{code}");
    assert!(
        !code.contains("<path"),
        "no path stands in for a circle: {code}"
    );
    assert!(code.contains("cx=\"10\""), "{code}");
    assert!(code.contains("r=\"5\""), "{code}");
}

/// RULE 2 at the boundary: the variable is a prop, and the code is the arithmetic.
#[test]
fn law_a_variable_exports_as_a_required_prop() {
    let mut engine = VectraEngine::new();
    dispatch(
        &mut engine,
        json!({"type": "SetVariable", "name": "base", "value": 40.0}),
    );
    let expression = vectra_core::new_expression_id().to_string();
    dispatch(
        &mut engine,
        json!({"type": "DefineExpression", "id": expression, "source": "$base * 2"}),
    );
    let node = rect(&mut engine, "box", 0.0, 0.0, 1.0, 30.0);
    dispatch(
        &mut engine,
        json!({"type": "SetParameter", "node_id": node, "property": "width",
               "value": {"Float": {"Expression": expression}}}),
    );

    let envelope = export(&mut engine, "react");
    let tsx = envelope["code"].as_str().unwrap();
    assert!(tsx.contains("base: number; // 40"), "{tsx}");
    assert!(tsx.contains("width={base * 2}"), "{tsx}");
    assert!(
        !tsx.contains("width={80}"),
        "the number is not the program: {tsx}"
    );

    // …and the *picture* export is the number, because a picture is a picture.
    let svg = export(&mut engine, "svg")["code"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(svg.contains("width=\"80\""), "{svg}");
}

/// The colour door, end to end: a slot that reads a procedural port exports the
/// value the pass published — the scene reaches the exporters, not just the
/// document.
#[test]
fn law_the_export_sees_the_live_scene() {
    let mut engine = VectraEngine::new();
    let base = rect(&mut engine, "base", 0.0, 0.0, 40.0, 30.0);
    let source = id();
    dispatch(
        &mut engine,
        json!({"type": "AddProceduralNode",
               "node": {"id": source, "kind": {"type": "source", "node": base}}}),
    );
    let noise = id();
    dispatch(
        &mut engine,
        json!({"type": "AddProceduralNode",
               "node": {"id": noise, "kind": {"type": "noise",
                       "amplitude": {"Literal": 1.0},
                       "frequency": {"Literal": 0.2},
                       "seed": {"Literal": 7.0}}}}),
    );
    dispatch(
        &mut engine,
        json!({"type": "ConnectProcedural", "node_id": noise, "port": "region",
               "from": {"node": source, "port": "region"}}),
    );
    dispatch(
        &mut engine,
        json!({"type": "SetParameter", "node_id": base, "property": "style.fill",
               "value": {"Color": {"Procedural": {"node": noise, "port": "tint"}}}}),
    );

    // The tint the pass published…
    let published = serde_json::from_str::<Value>(&engine.procedural_json()).unwrap();
    let tint = published["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["kind"] == "noise")
        .and_then(|node| node["outputs"].as_array())
        .and_then(|ports| ports.iter().find(|port| port["port"] == "tint"))
        .and_then(|port| port["value"].as_str())
        .expect("the noise node published a tint")
        .to_string();

    // …is the fill the SVG carries, and the code comment says why it is a number.
    let envelope = export(&mut engine, "svg");
    let svg = envelope["code"].as_str().unwrap();
    assert!(svg.contains(&format!("fill=\"{tint}\"")), "{svg}");
    let warnings = envelope["warnings"].as_array().unwrap();
    assert!(
        warnings
            .iter()
            .any(|warning| warning.as_str().unwrap().contains("colour reading")),
        "a procedural colour is reported, not silently substituted: {warnings:?}"
    );

    // …and the React export declares no prop for it: there is no variable, so
    // there is no prop — the colour is the drawn value with a warning above it.
    let react = export(&mut engine, "react");
    let tsx = react["code"].as_str().unwrap();
    assert!(tsx.contains(&format!("fill=\"{tint}\"")), "{tsx}");
    assert!(!tsx.contains("interface SceneProps"), "{tsx}");
}

/// A picture export of a motion-driven slot carries the number the canvas has at
/// the exported instant — the clock is part of what is exported, and the file says
/// so rather than pretending the slot was static.
#[test]
fn law_a_motion_slot_exports_the_drawn_number() {
    let mut engine = VectraEngine::new();
    let node = rect(&mut engine, "box", 0.0, 0.0, 40.0, 30.0);
    dispatch(
        &mut engine,
        json!({"type": "BindMotion", "node_id": node, "property": "height",
               "binding": {"Spring": {
                   "target": {"Literal": 100.0},
                   "stiffness": 120.0, "damping": 14.0, "from": 0.0, "at": 0.0}}}),
    );
    engine.set_time(0.05);

    let envelope = export(&mut engine, "svg");
    let svg = envelope["code"].as_str().unwrap().to_string();
    let drawn = serde_json::from_str::<Value>(&engine.get_snapshot()).unwrap()["scene"]["nodes"]
        [&node]["primitive"]["h"]
        .as_f64()
        .expect("the rect is drawn");

    // The number in the file is the number on the canvas (to text precision).
    let drawn_text = format!("{drawn:.6}");
    let drawn_text = drawn_text.trim_end_matches('0').trim_end_matches('.');
    assert!(
        svg.contains(&format!("height=\"{drawn_text}\"")),
        "expected {drawn_text} in {svg}"
    );
    assert!(drawn > 0.0, "the spring has moved off its anchor: {drawn}");
    assert!(
        envelope["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|warning| warning.as_str().unwrap().contains("animated")),
        "the file says the number is a sample: {}",
        envelope["warnings"]
    );
}

/// Determinism over the wire: the same engine, exported twice, is byte-identical
/// — and so are two engines that ran the same script.
#[test]
fn law_the_export_is_deterministic() {
    let script = |engine: &mut VectraEngine| {
        rect(engine, "box", 0.0, 0.0, 40.0, 30.0);
        let node = id();
        dispatch(
            engine,
            json!({"type": "CreateNode", "id": node, "name": "dot",
                   "kind": {"Circle": {
                       "cx": {"Literal": 60.0}, "cy": {"Literal": 0.0},
                       "radius": {"Literal": 10.0}}}}),
        );
    };
    let mut first = VectraEngine::new();
    script(&mut first);
    assert_eq!(first.export_to_svg(), first.export_to_svg());

    let mut second = VectraEngine::new();
    script(&mut second);
    // The ids differ (they are uuids), so compare the *pictures*: strip the
    // `data-vectra-node` attributes, which are the only identity in the file.
    let strip = |text: String| -> String {
        text.split_whitespace()
            .filter(|word| !word.starts_with("data-vectra-node="))
            .collect::<Vec<_>>()
            .join(" ")
    };
    assert_eq!(
        strip(first.export_to_svg()),
        strip(second.export_to_svg()),
        "two engines, one script, one picture"
    );
}

/// An empty engine is not an error: the user gets a valid, empty document.
#[test]
fn law_an_empty_engine_still_exports() {
    let mut engine = VectraEngine::new();
    let envelope = export(&mut engine, "svg");
    assert_eq!(envelope["status"], "ok");
    let code = envelope["code"].as_str().unwrap();
    assert!(code.starts_with("<?xml"), "{code}");
    assert!(code.contains("0 node(s)"), "{code}");
    let react = export(&mut engine, "react");
    assert!(react["code"]
        .as_str()
        .unwrap()
        .contains("export function Scene"));
}
