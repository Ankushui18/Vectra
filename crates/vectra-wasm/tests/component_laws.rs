//! Task 10.6's laws at the **protocol boundary** (RULE 1 and RULE 3).
//!
//! `vectra-core`'s unit tests prove the bind plan and the icon planner in
//! isolation, and `vectra-ai`'s tests prove what the prompts emit. This file
//! proves the half that only exists here — the numbers a designer actually sees
//! after the engine has evaluated the graph:
//!
//! * **The Component Prop Law** — setting a prop on one instance writes *that
//!   instance's* variable and nothing else: the master keeps its design values,
//!   the other instances keep theirs, and the written instance re-evaluates in
//!   the same `settle` (its clones' geometry, stroke and corner radius are all
//!   functions of the one number).
//! * **The Icon Scaling Law** — for every size in an icon set, the ratio
//!   `stroke ÷ size` is the master's ratio at *every* size, so a 2px stroke on a
//!   24px icon is 1.33px at 16px rather than a hairline; corner radius scales
//!   with the same law, and every artboard is exactly the size it claims.
//! * **RULE 4 at the seam** — every panel endpoint answers with prose a designer
//!   can read; no endpoint hands the UI raw JSON to render as JSON.

use proptest::prelude::*;
use serde_json::{json, Value};
use vectra_core::Document;
use vectra_wasm::VectraEngine;

// ── fixtures ───────────────────────────────────────────────────────────

fn dispatch(engine: &mut VectraEngine, cmd: Value) -> Value {
    serde_json::from_str(&engine.dispatch_command(&serde_json::to_string(&cmd).unwrap())).unwrap()
}

fn ok(engine: &mut VectraEngine, cmd: Value) -> Value {
    let response = dispatch(engine, cmd);
    assert_eq!(response["status"], "ok", "{response}");
    response
}

/// A square rectangle of `size` units with `radius` corners and a `stroke`-wide
/// outline — the shape of an icon master, in one call.
fn rect(engine: &mut VectraEngine, name: &str, size: f64, radius: f64, stroke: f64) -> String {
    let id = vectra_core::new_node_id().to_string();
    ok(
        engine,
        json!({"type": "CreateNode", "id": id, "name": name,
               "kind": {"Rectangle": {
                   "x": {"Literal": 0.0}, "y": {"Literal": 0.0},
                   "width": {"Literal": size}, "height": {"Literal": size},
                   "corner_radius": {"Literal": radius}}}}),
    );
    // A visible stroke needs a colour *and* a width: the legacy projection drops
    // a stroke layer whose colour is transparent, which is the engine's own rule
    // (Task 10.2) and not something a test should work around.
    ok(
        engine,
        json!({"type": "SetParameter", "node_id": id, "property": "style.stroke",
               "value": {"Color": {"Literal": {"r": 16, "g": 16, "b": 16, "a": 255}}}}),
    );
    if stroke > 0.0 {
        ok(
            engine,
            json!({"type": "SetParameter", "node_id": id, "property": "style.stroke_width",
                   "value": {"Float": {"Literal": stroke}}}),
        );
    }
    id
}

fn snapshot(engine: &mut VectraEngine) -> Value {
    serde_json::from_str(&engine.get_snapshot()).unwrap()
}

/// The document, round-tripped through the wire the container already uses —
/// no test-only accessor on the boundary, and the same JSON the file format
/// owns (Task 10.0).
fn document(engine: &VectraEngine) -> Document {
    serde_json::from_str(&engine.document_json()).expect("document json")
}

fn num(value: &Value, path: &[&str]) -> f64 {
    let mut cursor = value;
    for step in path {
        cursor = &cursor[*step];
    }
    cursor
        .as_f64()
        .unwrap_or_else(|| panic!("no number at {path:?}: {cursor:?}"))
}

/// The master a fresh selection became, via the panel's own view.
fn create_component(engine: &mut VectraEngine, members: &[String]) -> Value {
    engine
        .set_selection(&serde_json::to_string(members).unwrap())
        .to_string();
    let response: Value = serde_json::from_str(
        &engine.create_component(&serde_json::to_string(members).unwrap(), None),
    )
    .unwrap();
    assert_eq!(response["status"], "ok", "{response}");
    let view: Value = serde_json::from_str(&engine.component_view()).unwrap();
    assert_eq!(view["role"], "master", "{view}");
    assert_eq!(view["instances"], 0);
    view
}

fn instantiate(engine: &mut VectraEngine, master: &str, name: &str) -> String {
    let response: Value =
        serde_json::from_str(&engine.instantiate_component(master, Some(name.to_string())))
            .unwrap();
    assert_eq!(response["status"], "ok", "{response}");
    // The envelope names what it made, so the panel can select it (RULE 4: no
    // id arithmetic in the UI, and none in its tests).
    let instance = response["created"].as_str().unwrap_or_default().to_string();
    assert!(!instance.is_empty(), "no instance id in {response}");
    instance
}

fn set_prop(engine: &mut VectraEngine, target: &str, prop: &str, value: f64) -> Value {
    let response: Value = serde_json::from_str(&engine.set_component_prop(
        target,
        prop,
        &serde_json::to_string(&json!({"Float": {"Literal": value}})).unwrap(),
    ))
    .unwrap();
    assert_eq!(response["status"], "ok", "{response}");
    response
}

/// The clone of `member` belonging to `instance`: the index line-up is the
/// master's member order, which is what `members_of` promises.
fn clone_of(engine: &VectraEngine, master: &str, instance: &str, index: usize) -> String {
    let doc = document(engine);
    let master_id = vectra_core::parse_node_id(master).expect("master id");
    let instance_id = vectra_core::parse_node_id(instance).expect("instance id");
    let members = vectra_core::component::members_of(&doc, master_id);
    let clones = vectra_core::component::members_of(&doc, instance_id);
    assert_eq!(members.len(), clones.len(), "clone count");
    clones[index].to_string()
}

/// Every geometry number the snapshot resolves for one node. The wire's
/// primitive is internally tagged (`{"type":"rect","w":…}`), the same shape the
/// canvas reads — the test talks to the engine exactly as the UI does.
fn rect_of(engine: &mut VectraEngine, id: &str) -> (f64, f64, f64) {
    let snap = snapshot(engine);
    let node = &snap["scene"]["nodes"][id];
    assert_eq!(node["primitive"]["type"], "rect", "{node}");
    (
        num(node, &["primitive", "w"]),
        num(node, &["primitive", "h"]),
        num(node, &["primitive", "corner_radius"]),
    )
}

/// The ids of the document's artboards — a fresh document already has one, so
/// every test about a *macro's* boards compares against this (a law test that
/// assumed an empty document would pass for the wrong reason).
fn artboard_ids(engine: &mut VectraEngine) -> Vec<String> {
    snapshot(engine)["artboards"]
        .as_array()
        .unwrap()
        .iter()
        .map(|board| board["id"].as_str().unwrap_or_default().to_string())
        .collect()
}

fn artboard_count(engine: &mut VectraEngine) -> usize {
    snapshot(engine)["artboards"].as_array().unwrap().len()
}

fn stroke_of(engine: &mut VectraEngine, id: &str) -> f64 {
    let snap = snapshot(engine);
    num(&snap["scene"]["nodes"][id], &["style", "stroke_width"])
}

// ── RULE 1: the panel's view ────────────────────────────────────────────

#[test]
fn a_master_is_created_from_the_selection_and_shows_designer_props() {
    let mut engine = VectraEngine::new();
    let rect_id = rect(&mut engine, "glyph", 24.0, 4.0, 2.0);
    let view = create_component(&mut engine, std::slice::from_ref(&rect_id));

    let keys: Vec<String> = view["props"]
        .as_array()
        .expect("props")
        .iter()
        .map(|prop| prop["key"].as_str().unwrap_or_default().to_string())
        .collect();
    for expected in ["size", "stroke_width", "corner_radius"] {
        assert!(
            keys.contains(&expected.to_string()),
            "no `{expected}` in {keys:?}"
        );
    }
    // Every prop is panel-shaped: a label, and — for a scalar, which is what the
    // sliders draw — a value and a range to draw it in. A colour prop carries a
    // swatch instead, so it needs no range.
    for prop in view["props"].as_array().unwrap() {
        let label = prop["label"].as_str().unwrap_or_default();
        assert!(!label.is_empty(), "unlabelled prop: {prop}");
        match prop["ty"].as_str().unwrap_or_default() {
            "scalar" => {
                assert!(prop["value"].is_number(), "no value: {prop}");
                assert!(
                    prop["max"].as_f64().unwrap_or(0.0) > 0.0,
                    "no range: {prop}"
                );
                assert!(
                    prop["min"].as_f64().unwrap_or(-1.0) >= 0.0,
                    "bad range: {prop}"
                );
            }
            "color" => {
                let color = prop["color"].as_str().unwrap_or_default();
                assert!(
                    color.starts_with('#') && color.len() == 7,
                    "bad swatch: {prop}"
                );
            }
            other => panic!("unknown prop type {other}: {prop}"),
        }
    }
    // …and the headline is a sentence (RULE 4), not a JSON blob.
    let headline = view["headline"].as_str().unwrap_or_default();
    assert!(headline.contains("Component"), "{headline}");
    assert!(
        !headline.contains('{') && !headline.contains('['),
        "{headline}"
    );
}

#[test]
fn every_component_verb_answers_with_prose() {
    let mut engine = VectraEngine::new();
    let rect_id = rect(&mut engine, "glyph", 24.0, 4.0, 2.0);
    let view = create_component(&mut engine, std::slice::from_ref(&rect_id));
    let master = view["id"].as_str().unwrap().to_string();

    let mut replies = vec![
        engine.create_component(
            &serde_json::to_string(std::slice::from_ref(&rect_id)).unwrap(),
            None,
        ),
        engine.instantiate_component(&master, Some("small".into())),
        engine.icon_set(&master, &serde_json::to_string(&[16.0, 32.0]).unwrap()),
    ];
    // The one that needs an instance id first.
    let instance = instantiate(&mut engine, &master, "card");
    replies.push(engine.set_component_prop(&instance, "size", "{\"Float\":{\"Literal\":40}}"));

    for reply in replies {
        let value: Value = serde_json::from_str(&reply).unwrap();
        let prose = value["prose"].as_str().unwrap_or_default();
        assert!(!prose.is_empty(), "no prose in {reply}");
        assert!(
            !prose.contains('{') && !prose.contains('[') && !prose.contains('"'),
            "prose is JSON-shaped: {prose}"
        );
        assert!(prose.starts_with('✨'), "not a designer sentence: {prose}");
        // …and the label is a verb phrase, not a variant name.
        let label = value["label"].as_str().unwrap_or_default();
        assert!(
            !label.is_empty() && !label.contains("Component {"),
            "{label}"
        );
    }
    assert_eq!(
        serde_json::from_str::<Value>(&engine.structural_macros())
            .unwrap()
            .as_array()
            .unwrap()
            .len(),
        5
    );
}

// ── RULE 1: the Component Prop Law ──────────────────────────────────────

proptest! {
    #![proptest_config(ProptestConfig::with_cases(24))]

    /// **The Component Prop Law.** For any master — any design size, corner
    /// radius and stroke width — and any two instance sizes: writing `size` on
    /// one instance scales *that instance's* clones by exactly `size ÷ design`,
    /// leaves the master's members and the other instance's clones byte-identical,
    /// and moves stroke and corner radius by the same factor, because both are
    /// derived from the one variable through the graph.
    #[test]
    fn law_the_component_prop_law(
        design in 12.0f64..=64.0,
        stroke in 0.5f64..=4.0,
        radius in 0.0f64..=10.0,
        first_size in 8.0f64..=96.0,
        second_size in 8.0f64..=96.0,
    ) {
        let mut engine = VectraEngine::new();
        let member = rect(&mut engine, "glyph", design, radius.min(design / 2.0), stroke);
        let view = create_component(&mut engine, std::slice::from_ref(&member));
        let master = view["id"].as_str().unwrap().to_string();

        let first = instantiate(&mut engine, &master, "first");
        let second = instantiate(&mut engine, &master, "second");
        set_prop(&mut engine, &first, "size", first_size);
        set_prop(&mut engine, &second, "size", second_size);

        let first_clone = clone_of(&engine, &master, &first, 0);
        let second_clone = clone_of(&engine, &master, &second, 0);

        // The master keeps its design values: a component's members are its
        // *reference*, and only an instance's own props move it.
        let (master_w, _, master_radius) = rect_of(&mut engine, &member);
        prop_assert!((master_w - design).abs() < 1e-6, "master width {master_w} != {design}");
        prop_assert!((master_radius - radius.min(design / 2.0)).abs() < 1e-6);
        let master_stroke = stroke_of(&mut engine, &member);
        prop_assert!((master_stroke - stroke).abs() < 1e-6, "master stroke {master_stroke}");

        // Each instance is its own scaling of the master, exactly.
        for (clone, size) in [(&first_clone, first_size), (&second_clone, second_size)] {
            let (w, h, r) = rect_of(&mut engine, clone);
            let ratio = size / design;
            prop_assert!((w - design * ratio).abs() < 1e-6, "{w} != {}*{ratio}", design);
            prop_assert!((h - w).abs() < 1e-6, "not square: {w}x{h}");
            prop_assert!((r - radius.min(design / 2.0) * ratio).abs() < 1e-6);
            let s = stroke_of(&mut engine, clone);
            prop_assert!((s - stroke * ratio).abs() < 1e-6, "{s} != {stroke}*{ratio}");
        }

        // Writing the *second* instance's prop left the first one's numbers
        // alone — the isolation half of the law.
        let (w_before, _, _) = rect_of(&mut engine, &first_clone);
        set_prop(&mut engine, &second, "size", second_size.min(design));
        let (w_after, _, _) = rect_of(&mut engine, &first_clone);
        prop_assert_eq!(w_before, w_after);
    }
}

#[test]
fn an_instances_prop_is_its_own_variable() {
    let mut engine = VectraEngine::new();
    let member = rect(&mut engine, "glyph", 24.0, 4.0, 2.0);
    let view = create_component(&mut engine, std::slice::from_ref(&member));
    let master = view["id"].as_str().unwrap().to_string();
    let instance = instantiate(&mut engine, &master, "small");

    set_prop(&mut engine, &instance, "size", 16.0);
    let snap = snapshot(&mut engine);
    let doc = document(&engine);
    let instance_id = vectra_core::parse_node_id(&instance).unwrap();
    // The instance's own variable is 16 …
    let name = vectra_core::component::spec_of(&doc, instance_id)
        .and_then(|spec| spec.variable("size"))
        .cloned()
        .expect("size variable");
    assert_eq!(snap["variables"][&name], 16.0);
    // … and the master's is still the design size it was created with.
    let master_id = vectra_core::parse_node_id(&master).unwrap();
    let master_name = vectra_core::component::spec_of(&doc, master_id)
        .and_then(|spec| spec.variable("size"))
        .cloned()
        .expect("master size variable");
    assert_eq!(snap["variables"][&master_name], 24.0);
    assert_ne!(name, master_name);
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(24))]

    /// **The Instance Seed Law.** Placing an instance places a *copy*: before a
    /// single prop is written, its clone carries the master's design numbers and
    /// the panel's Size slider reads them — not the `1.0` floor a literal-only
    /// reading of expression-bound clones would leave behind (which shipped a
    /// 1×1 speck with a slider spanning `0..4`).
    #[test]
    fn law_a_fresh_instance_is_the_master_at_its_design_size(
        design in 12.0f64..=64.0,
        stroke in 0.5f64..=4.0,
        radius in 0.0f64..=10.0,
    ) {
        let mut engine = VectraEngine::new();
        let member = rect(&mut engine, "glyph", design, radius.min(design / 2.0), stroke);
        let view = create_component(&mut engine, std::slice::from_ref(&member));
        let master = view["id"].as_str().unwrap().to_string();
        let instance = instantiate(&mut engine, &master, "copy");

        let clone = clone_of(&engine, &master, &instance, 0);
        let (w, h, r) = rect_of(&mut engine, &clone);
        prop_assert!((w - design).abs() < 1e-6, "{w} != {design}");
        prop_assert!((h - design).abs() < 1e-6, "{h} != {design}");
        prop_assert!((r - radius.min(design / 2.0)).abs() < 1e-6, "{r}");
        let s = stroke_of(&mut engine, &clone);
        prop_assert!((s - stroke).abs() < 1e-6, "{s} != {stroke}");

        // …and the panel agrees: the instance's own Size slider reads the design
        // size, and spans a range a designer can use.
        engine
            .set_selection(&serde_json::to_string(&[instance]).unwrap())
            .to_string();
        let panel: Value = serde_json::from_str(&engine.component_view()).unwrap();
        let size = panel["props"]
            .as_array()
            .unwrap()
            .iter()
            .find(|prop| prop["key"] == "size")
            .cloned()
            .expect("size prop");
        prop_assert!(
            (size["value"].as_f64().unwrap() - design).abs() < 1e-6,
            "size value is not the design size: {size}"
        );
        prop_assert!(
            size["max"].as_f64().unwrap() >= design * 2.0,
            "the slider cannot even double the icon: {size}"
        );
    }
}

/// **The original group is the master's *bound* artwork** (RULE 1's linkage).
///
/// `Create Component` does not move the artwork: the nodes the designer selected
/// stay where they are, and what changes is *what their numbers are* — the
/// master is a `ProceduralNode` that names them, and every slot the props cover
/// is rebound from a literal to an expression over the master's own variable. So
/// the original group references the master through the graph from that moment
/// on, which is what makes a prop write cheap (the edges already exist) and what
/// keeps the design values in exactly one place: the artwork itself.
///
/// This is the codebase's reading of RULE 1's sentence — the selected group is
/// the prototype the master is cut from, not a copy of it, and "becoming an
/// instance" is what *placing* an instance does. The test pins the parts that
/// make the reading true rather than merely stated:
///
/// * the master **names** the selection, in draw order, and the artwork answers
///   `owner_of` with that master;
/// * the binding is real: the member's `width` is a `Parameter::Expression` whose
///   source reads the master's `size` variable;
/// * and it is **value-preserving** — the artwork does not move, resize or
///   restyle when it becomes a component.
#[test]
fn the_original_group_becomes_the_masters_bound_artwork() {
    let mut engine = VectraEngine::new();
    let glyph = rect(&mut engine, "glyph", 24.0, 4.0, 2.0);
    let badge = rect(&mut engine, "badge", 12.0, 0.0, 0.0);
    let before = snapshot(&mut engine);

    let view = create_component(&mut engine, &[glyph.clone(), badge.clone()]);
    let master = vectra_core::parse_node_id(view["id"].as_str().expect("master id")).expect("id");
    let doc = document(&engine);

    // 1. The master names the selection, in draw order — membership is a
    //    *listing* (a wire carries a value; membership is not a value).
    let members: Vec<String> = vectra_core::component::members_of(&doc, master)
        .iter()
        .map(|id| id.to_string())
        .collect();
    assert_eq!(
        members,
        vec![glyph.clone(), badge.clone()],
        "members, in order"
    );

    for member in [&glyph, &badge] {
        let id = vectra_core::parse_node_id(member).expect("member id");
        assert_eq!(
            vectra_core::component::owner_of(&doc, id),
            Some(master),
            "{member} does not answer with its master"
        );
    }

    // 2. The binding is real: the member's width reads the master's `size`
    //    variable through a compiled expression, seeded at the design size.
    let spec = vectra_core::component::spec_of(&doc, master)
        .cloned()
        .expect("master spec");
    let size = spec.variable("size").cloned().expect("size variable");
    assert_eq!(
        doc.variables.get(&size).copied(),
        Some(24.0),
        "the master's own variable is the design size"
    );

    let glyph_id = vectra_core::parse_node_id(&glyph).expect("glyph id");
    let member = doc.nodes.get(&glyph_id).expect("member node");
    match member.get_param("width").expect("width slot") {
        vectra_core::ParamValue::Float(vectra_core::Parameter::Expression(expression)) => {
            let source = doc
                .expressions
                .get(&expression)
                .expect("the expression is registered")
                .source
                .clone();
            assert!(
                source.contains(&format!("${size}")),
                "the member's width must read the master's variable: {source}"
            );
        }
        other => panic!("the member's width was not rebound: {other:?}"),
    }

    // The scaled props bind the same way: the stroke is an expression over the
    // one variable, which is the law the whole feature rests on.
    match member.get_param("style.stroke_width").expect("stroke slot") {
        vectra_core::ParamValue::Float(vectra_core::Parameter::Expression(expression)) => {
            let source = doc
                .expressions
                .get(&expression)
                .expect("stroke expression")
                .source
                .clone();
            assert!(
                source.contains(&format!("${size}")),
                "stroke follows size: {source}"
            );
        }
        other => panic!("the member's stroke was not rebound: {other:?}"),
    }

    // 3. Value-preserving: the artwork the designer selected did not move.
    let after = snapshot(&mut engine);
    let geometry = |snap: &Value, id: &str| {
        let node = &snap["scene"]["nodes"][id];
        (
            num(node, &["primitive", "w"]),
            num(node, &["primitive", "h"]),
            num(node, &["primitive", "corner_radius"]),
            num(node, &["style", "stroke_width"]),
        )
    };
    for id in [&glyph, &badge] {
        assert_eq!(
            geometry(&before, id),
            geometry(&after, id),
            "becoming a component changed {id}'s numbers"
        );
    }
}

// ── RULE 3: the Icon Scaling Law ────────────────────────────────────────

proptest! {
    #![proptest_config(ProptestConfig::with_cases(24))]

    /// **The Icon Scaling Law.** For any master (design size, stroke, radius) and
    /// any ladder of sizes, the generated set is optically consistent: at every
    /// size the stroke ÷ size ratio is the master's, corner radius follows the
    /// same ratio, each artboard is exactly the size it claims, and a stroke that
    /// is at least 1px per 16 design units never falls below 1px at 16px — the
    /// hairline the rule exists to prevent.
    #[test]
    fn law_the_icon_scaling_law(
        design in 12.0f64..=48.0,
        stroke in 1.0f64..=4.0,
        radius in 0.0f64..=10.0,
        ladder in prop::collection::vec(8.0f64..=96.0, 2..4),
    ) {
        let mut engine = VectraEngine::new();
        let member = rect(&mut engine, "glyph", design, radius.min(design / 2.0), stroke);
        let view = create_component(&mut engine, std::slice::from_ref(&member));
        let master = view["id"].as_str().unwrap().to_string();

        let mut sizes = ladder.clone();
        sizes.sort_by(|a, b| a.partial_cmp(b).unwrap());
        sizes.dedup();
        let existing: Vec<String> = artboard_ids(&mut engine);
        let response: Value = serde_json::from_str(
            &engine.icon_set(&master, &serde_json::to_string(&sizes).unwrap()),
        )
        .unwrap();
        prop_assert_eq!(response["status"].as_str(), Some("ok"), "{}", response);

        // One artboard per size, exactly that size (RULE 3's first half) — the
        // boards the macro *made*, not the document's starting board.
        let snap = snapshot(&mut engine);
        let mut boards: Vec<f64> = snap["artboards"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|board| {
                let id = board["id"].as_str().unwrap_or_default();
                !existing.iter().any(|seen| seen == id)
            })
            .map(|board| board["bounds"][2].as_f64().unwrap_or(f64::NAN))
            .collect();
        boards.sort_by(|a, b| a.partial_cmp(b).unwrap());
        prop_assert_eq!(boards.len(), sizes.len(), "{}", snap["artboards"]);
        for (got, want) in boards.iter().zip(sizes.iter()) {
            prop_assert!((got - want).abs() < 1e-9, "{got} != {want}");
        }

        // … and one instance per size, scaled by the master's own ratios.
        let doc = document(&engine);
        let instances = vectra_core::component::instances_of(
            &doc,
            vectra_core::parse_node_id(&master).unwrap(),
        );
        prop_assert_eq!(instances.len(), sizes.len());

        let ratio = stroke / design;
        for (instance, size) in instances.iter().zip(sizes.iter()) {
            let clone = vectra_core::component::members_of(&doc, *instance)[0];
            let clone = clone.to_string();
            let (w, h, r) = rect_of(&mut engine, &clone);
            let scale = size / design;
            prop_assert!((w - design * scale).abs() < 1e-6, "{w} at {size}");
            prop_assert!((h - w).abs() < 1e-6);
            prop_assert!((r - radius.min(design / 2.0) * scale).abs() < 1e-6);
            let s = stroke_of(&mut engine, &clone);
            prop_assert!((s - stroke * scale).abs() < 1e-6, "{s} at {size}");
            // The law's own statement: the ratio is invariant.
            prop_assert!((s / size - ratio).abs() < 1e-9, "ratio drifted at {size}");
            // Nothing thins into a hairline.
            if *size >= 16.0 && ratio >= 1.0 / 16.0 {
                prop_assert!(s >= 1.0, "{s}px stroke at {size}px");
            }
        }
    }
}

#[test]
fn a_default_icon_set_is_the_16_32_48_ladder() {
    let mut engine = VectraEngine::new();
    let member = rect(&mut engine, "glyph", 24.0, 4.0, 2.0);
    let view = create_component(&mut engine, std::slice::from_ref(&member));
    let master = view["id"].as_str().unwrap().to_string();

    let response: Value = serde_json::from_str(&engine.icon_set(&master, "[16, 32, 48]")).unwrap();
    assert_eq!(response["status"], "ok", "{response}");

    // The boards are named after the master, so the panel can group them.
    let base = view["name"].as_str().unwrap_or("Component").to_string();
    let snap = snapshot(&mut engine);
    let names: Vec<String> = snap["artboards"]
        .as_array()
        .unwrap()
        .iter()
        .map(|board| board["name"].as_str().unwrap_or_default().to_string())
        .collect();
    for size in [16, 32, 48] {
        let expected = format!("{base} {size}");
        assert!(
            names.iter().any(|name| name.ends_with(&expected)),
            "no artboard for {expected} in {names:?}"
        );
    }
    // The 16px instance is the one the rule is about: 2px × 16 ÷ 24 = 1.33px.
    let doc = document(&engine);
    let instances =
        vectra_core::component::instances_of(&doc, vectra_core::parse_node_id(&master).unwrap());
    let mut strokes: Vec<(f64, f64)> = instances
        .iter()
        .map(|instance| {
            let clone = vectra_core::component::members_of(&doc, *instance)[0].to_string();
            let (w, _, _) = rect_of(&mut engine, &clone);
            (w, stroke_of(&mut engine, &clone))
        })
        .collect();
    strokes.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    assert_eq!(strokes.len(), 3);
    assert!((strokes[0].0 - 16.0).abs() < 1e-6, "{strokes:?}");
    assert!(
        strokes[0].1 >= 1.0,
        "a 2px stroke on a 24px master must not be a hairline at 16px: {strokes:?}"
    );
    for (size, stroke) in &strokes {
        assert!((stroke / size - 2.0 / 24.0).abs() < 1e-9, "{strokes:?}");
    }
}

#[test]
fn an_icon_set_refuses_a_selection_that_is_not_a_master() {
    let mut engine = VectraEngine::new();
    let member = rect(&mut engine, "glyph", 24.0, 4.0, 2.0);
    let before = artboard_count(&mut engine);
    let response: Value = serde_json::from_str(&engine.icon_set(&member, "[16, 32]")).unwrap();
    assert_eq!(response["status"], "error", "{response}");
    let message = response["message"].as_str().unwrap_or_default();
    assert!(message.contains("master"), "{message}");
    // …and the document is untouched: not one artboard was added.
    assert_eq!(artboard_count(&mut engine), before);
}

#[test]
fn the_panel_view_tracks_the_selection_through_its_roles() {
    let mut engine = VectraEngine::new();
    let member = rect(&mut engine, "glyph", 24.0, 4.0, 2.0);
    let loose = rect(&mut engine, "loose", 10.0, 0.0, 0.0);

    // Nothing selected.
    engine.set_selection("[]").to_string();
    let empty: Value = serde_json::from_str(&engine.component_view()).unwrap();
    assert_eq!(empty["role"], "none");
    assert_eq!(empty["can_create"], false);
    assert!(empty["headline"].as_str().unwrap().contains("Select"));

    // Authored artwork: "this could become a component".
    engine
        .set_selection(&serde_json::to_string(std::slice::from_ref(&loose)).unwrap())
        .to_string();
    let selection: Value = serde_json::from_str(&engine.component_view()).unwrap();
    assert_eq!(selection["role"], "selection");
    assert_eq!(selection["can_create"], true);
    assert_eq!(selection["selection"]["count"], 1);
    assert!(!selection["selection"]["prose"]
        .as_str()
        .unwrap()
        .contains('{'));

    // A clone answers through its instance, which answers through its master.
    let view = create_component(&mut engine, std::slice::from_ref(&member));
    let master = view["id"].as_str().unwrap().to_string();
    let instance = instantiate(&mut engine, &master, "small");
    engine
        .set_selection(&serde_json::to_string(std::slice::from_ref(&instance)).unwrap())
        .to_string();
    let view: Value = serde_json::from_str(&engine.component_view()).unwrap();
    assert_eq!(view["role"], "instance");
    assert_eq!(view["master"], master.as_str());
    assert_eq!(view["instances"], 0);
    assert!(!view["props"].as_array().unwrap().is_empty());
    // The master reports its instance count for the panel's subtitle.
    engine
        .set_selection(&serde_json::to_string(std::slice::from_ref(&master)).unwrap())
        .to_string();
    let view: Value = serde_json::from_str(&engine.component_view()).unwrap();
    assert_eq!(view["instances"], 1);
    assert_eq!(view["masters"].as_array().unwrap().len(), 1);
}

#[test]
fn undo_restores_the_document_after_a_component_and_an_icon_set() {
    let mut engine = VectraEngine::new();
    let member = rect(&mut engine, "glyph", 24.0, 4.0, 2.0);
    let before = snapshot(&mut engine);
    let view = create_component(&mut engine, std::slice::from_ref(&member));
    let master = view["id"].as_str().unwrap().to_string();
    let after_master = snapshot(&mut engine);
    assert_ne!(before["procedural"], after_master["procedural"]);
    assert_eq!(
        serde_json::from_str::<Value>(&engine.undo()).unwrap()["status"],
        "ok"
    );
    let restored = snapshot(&mut engine);
    assert_eq!(restored["procedural"], before["procedural"]);
    assert_eq!(restored["variables"], before["variables"]);
    // Redo brings it back, and the icon set is one undoable step of its own.
    assert_eq!(
        serde_json::from_str::<Value>(&engine.redo()).unwrap()["status"],
        "ok"
    );
    let view: Value = serde_json::from_str(&engine.component_view()).unwrap();
    assert_eq!(
        view["role"], "master",
        "the redo restored the component: {view}"
    );
    assert_eq!(view["instances"], 0);
    let before = artboard_count(&mut engine);
    let response: Value = serde_json::from_str(&engine.icon_set(&master, "[16, 32]")).unwrap();
    assert_eq!(response["status"], "ok", "{response}");
    assert_eq!(artboard_count(&mut engine), before + 2);
    // The whole set is **one** undo step: a macro is one action, not five.
    assert_eq!(
        serde_json::from_str::<Value>(&engine.undo()).unwrap()["status"],
        "ok"
    );
    assert_eq!(artboard_count(&mut engine), before);
}
