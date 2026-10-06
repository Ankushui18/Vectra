//! Task 9.0's laws through the **wasm boundary** and the real dispatch path.
//!
//! `vectra-ai`'s own tests prove the loop over `vectra-core`'s engine. This file
//! proves the half that only exists here: the AI's commands meet the *boundary*
//! gates — the dependency-graph cycle check, the expression pre-compile, the
//! procedural port gate, the drag guard — and the scene/dirty bookkeeping that
//! follows them. It also pins the wire shape the React panel consumes.

use serde_json::{json, Value};
use vectra_wasm::VectraEngine;

fn dispatch(engine: &mut VectraEngine, cmd: Value) -> Value {
    serde_json::from_str(&engine.dispatch_command(&serde_json::to_string(&cmd).unwrap())).unwrap()
}

fn rect(engine: &mut VectraEngine, name: &str, x: f64, y: f64, w: f64, h: f64) -> String {
    let id = vectra_core::new_node_id().to_string();
    let response = dispatch(
        engine,
        json!({"type": "CreateNode", "id": id, "name": name,
               "kind": {"Rectangle": {
                   "x": {"Literal": x}, "y": {"Literal": y},
                   "width": {"Literal": w}, "height": {"Literal": h},
                   "corner_radius": {"Literal": 0.0}}}}),
    );
    assert_eq!(response["status"], "ok", "{response}");
    id
}

/// Every UUID in a JSON tree replaced by a placeholder — plans differ by minted
/// ids alone, and this is how that claim is tested.
fn scrub(value: &Value) -> Value {
    match value {
        Value::String(text) if text.len() == 36 && text.matches('-').count() == 4 => json!("<id>"),
        Value::Array(items) => Value::Array(items.iter().map(scrub).collect()),
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(key, item)| (key.clone(), scrub(item)))
                .collect(),
        ),
        other => other.clone(),
    }
}

/// The status of one undo, as a string.
fn undo_status(engine: &mut VectraEngine) -> String {
    let response: Value = serde_json::from_str(&engine.undo()).unwrap();
    response["status"].as_str().unwrap_or("?").to_string()
}

fn preview(engine: &VectraEngine, prompt: &str) -> Value {
    serde_json::from_str(&engine.ai_generate_commands(prompt, "")).unwrap()
}

fn execute(engine: &mut VectraEngine, prompt: &str) -> Value {
    serde_json::from_str(&engine.ai_execute_with_retry(prompt, "")).unwrap()
}

// ── RULE 2: the summary at the boundary ────────────────────────────────

#[test]
fn the_boundary_publishes_the_summary_the_prompt_and_the_phrasings() {
    let mut engine = VectraEngine::new();
    let card = rect(&mut engine, "card", 0.0, 0.0, 80.0, 60.0);
    dispatch(
        &mut engine,
        json!({"type": "SetVariable", "name": "base", "value": 40.0}),
    );
    dispatch(
        &mut engine,
        json!({"type": "SetParameter", "node_id": card, "property": "width",
               "value": {"Float": {"Variable": "base"}}}),
    );

    let summary: Value = serde_json::from_str(&engine.document_summary()).unwrap();
    assert_eq!(summary["nodes"][0]["label"], "Rectangle 'card'");
    assert_eq!(summary["nodes"][0]["id"], card);
    assert_eq!(summary["variables"][0]["name"], "base");
    assert_eq!(summary["variables"][0]["value"], 40.0);
    // The *parametric* source, not the number: RULE 2's whole point.
    let width = summary["nodes"][0]["slots"]
        .as_array()
        .unwrap()
        .iter()
        .find(|slot| slot["property"] == "width")
        .unwrap();
    assert_eq!(width["source"], "$base");
    assert_eq!(width["value"], 40.0);

    // The system prompt carries the document block.
    let prompt = engine.ai_prompt();
    assert!(prompt.contains("Rectangle 'card'"), "{prompt}");
    assert!(prompt.contains("$base = 40"));
    assert!(prompt.contains("THE COMMAND API"));
    assert!(!prompt.contains("{{DOCUMENT}}"));

    // The hint line is the planner's own list.
    let phrasings: Vec<String> = serde_json::from_str(&engine.ai_phrasings()).unwrap();
    assert!(phrasings.iter().any(|line| line.contains("rectangle")));
}

// ── RULE 1 + the preview path ──────────────────────────────────────────

#[test]
fn a_preview_shows_resolved_commands_and_applies_nothing() {
    let mut engine = VectraEngine::new();
    let card = rect(&mut engine, "card", 0.0, 0.0, 80.0, 60.0);
    dispatch(
        &mut engine,
        json!({"type": "SetVariable", "name": "marker", "value": 1.0}),
    );
    let before = engine.get_snapshot();

    let preview = preview(&engine, "set the width of card to 120");
    assert_eq!(preview["status"], "ok");
    assert_eq!(preview["attempt"], 1);
    assert_eq!(preview["applies"], false);
    let plan = preview["plan"].as_array().unwrap();
    assert_eq!(plan.len(), 1);
    assert_eq!(plan[0]["type"], "SetParameter");
    assert_eq!(plan[0]["node_id"], card, "the summary's id, verbatim");
    assert_eq!(plan[0]["value"]["Float"]["Literal"], 120.0);

    // A preview mutates nothing — not the document, not the scene, not history.
    assert_eq!(
        engine.get_snapshot(),
        before,
        "a preview changed the document"
    );
    // The top of the history is still the `SetVariable` above — a preview pushes
    // no entry of its own.
    assert_eq!(undo_status(&mut engine), "ok");
    let summary: Value = serde_json::from_str(&engine.document_summary()).unwrap();
    assert!(
        summary["variables"].as_array().unwrap().is_empty(),
        "the preview pushed a history entry: {summary}"
    );
}

#[test]
fn an_approved_plan_executes_and_the_engine_marks_the_nodes_dirty() {
    let mut engine = VectraEngine::new();
    let card = rect(&mut engine, "card", 0.0, 0.0, 80.0, 60.0);
    let _ = card;
    let preview = preview(&engine, "set the width of card to 120");
    let plan = serde_json::to_string(&preview["plan"]).unwrap();

    let response: Value =
        serde_json::from_str(&engine.ai_execute_commands("set the width of card to 120", &plan))
            .unwrap();
    assert_eq!(response["status"], "ok");
    let report = &response["report"];
    assert_eq!(report["attempts"], 1);
    assert_eq!(report["plan"].as_array().unwrap().len(), 1);
    assert_eq!(report["dirty"].as_array().unwrap().len(), 1, "{report}");
    assert_eq!(
        report["summary"]["nodes"][0]["slots"]
            .as_array()
            .unwrap()
            .iter()
            .find(|slot| slot["property"] == "width")
            .unwrap()["value"],
        120.0
    );
    // One history entry for the whole plan, and the headline says what happened.
    assert!(response["headline"]
        .as_str()
        .unwrap()
        .contains("1 command(s)"));
    assert_eq!(undo_status(&mut engine), "ok");
}

// ── RULE 3: the same gates a user's command meets ──────────────────────

#[test]
fn the_ai_path_goes_through_the_engines_own_gates() {
    // A cycle: `width` reads a procedural port whose chain sources `card` — the
    // same shape the smoke suite proves is refused for a *user's* command.
    let mut engine = VectraEngine::new();
    let card = rect(&mut engine, "card", 0.0, 0.0, 80.0, 60.0);
    let source = vectra_core::new_node_id().to_string();
    dispatch(
        &mut engine,
        json!({"type": "AddProceduralNode",
               "node": {"id": source, "kind": {"type": "source", "node": card}}}),
    );
    let noise = vectra_core::new_node_id().to_string();
    dispatch(
        &mut engine,
        json!({"type": "AddProceduralNode",
               "node": {"id": noise, "kind": {"type": "noise",
                       "amplitude": {"Literal": 1.0}, "frequency": {"Literal": 0.2},
                       "seed": {"Literal": 7.0}}}}),
    );
    dispatch(
        &mut engine,
        json!({"type": "ConnectProcedural", "node_id": noise, "port": "region",
               "from": {"node": source, "port": "region"}}),
    );

    // An AI plan that closes the loop is refused by the engine's gate, and the
    // document is untouched.
    let before = engine.get_snapshot();
    let plan = serde_json::to_string(&json!([
        {"type": "SetParameter", "node_id": card, "property": "width",
         "value": {"Float": {"Procedural": {"node": noise, "port": "tint"}}}}
    ]))
    .unwrap();
    let response: Value =
        serde_json::from_str(&engine.ai_execute_commands("make the width read the noise", &plan))
            .unwrap();
    assert_eq!(response["status"], "error");
    assert_eq!(response["code"], "engine-rejected");
    assert!(
        response["message"].as_str().unwrap().contains("cyclic"),
        "{response}"
    );
    assert_eq!(
        engine.get_snapshot(),
        before,
        "a refused plan changed something"
    );

    // The same document *without* the loop accepts the very same command.
    let mut clean = VectraEngine::new();
    let dot = rect(&mut clean, "dot", 0.0, 0.0, 10.0, 10.0);
    let solo = vectra_core::new_node_id().to_string();
    dispatch(
        &mut clean,
        json!({"type": "AddProceduralNode",
               "node": {"id": solo, "kind": {"type": "noise",
                       "amplitude": {"Literal": 1.0}, "frequency": {"Literal": 0.2},
                       "seed": {"Literal": 7.0}}}}),
    );
    let plan = serde_json::to_string(&json!([
        {"type": "SetParameter", "node_id": dot, "property": "width",
         "value": {"Float": {"Procedural": {"node": solo, "port": "tint"}}}}
    ]))
    .unwrap();
    let response: Value =
        serde_json::from_str(&clean.ai_execute_commands("read the noise", &plan)).unwrap();
    assert_eq!(response["status"], "ok", "{response}");
}

#[test]
fn a_drag_in_progress_refuses_the_ai_exactly_as_it_refuses_the_user() {
    let mut engine = VectraEngine::new();
    let card = rect(&mut engine, "card", 0.0, 0.0, 80.0, 60.0);
    dispatch(&mut engine, json!({"type": "BeginDrag", "node_id": card}));

    let response = execute(&mut engine, "set the width of card to 120");
    assert_eq!(response["status"], "error");
    assert_eq!(response["code"], "max-retries-exceeded");
    // The engine's own words, three times over — the gesture owns the tableau.
    assert!(
        response["message"].as_str().unwrap().contains("drag"),
        "{response}"
    );
    assert_eq!(
        response["corrections"].as_array().unwrap().len(),
        3,
        "one correction per attempt: {response}"
    );
    assert_eq!(
        dispatch(&mut engine, json!({"type": "EndDrag", "node_id": card}))["status"],
        "ok"
    );
}

// ── RULE 3's ReAct loop, end to end ────────────────────────────────────

#[test]
fn the_loop_feeds_the_engines_refusal_back_and_the_second_attempt_lands() {
    let mut engine = VectraEngine::new();
    let dot = {
        let id = vectra_core::new_node_id().to_string();
        dispatch(
            &mut engine,
            json!({"type": "CreateNode", "id": id, "name": "dot",
                   "kind": {"Circle": {"cx": {"Literal": 40.0}, "cy": {"Literal": 20.0},
                                       "radius": {"Literal": 15.0}}}}),
        );
        id
    };

    let response = execute(&mut engine, "round the corners of dot by 8");
    assert_eq!(response["status"], "ok", "{response}");
    let report = &response["report"];
    assert_eq!(report["attempts"], 2, "one correction was needed");
    assert_eq!(response["corrections"], 1);
    let correction = &report["corrections"][0];
    assert_eq!(correction["code"], "engine-rejected");
    assert!(
        correction["error"]
            .as_str()
            .unwrap()
            .contains("unknown property 'corner_radius'"),
        "{correction}"
    );
    // The planner says what it changed, and the engine's own event names the
    // node it re-evaluated.
    assert!(report["notes"]
        .as_array()
        .unwrap()
        .iter()
        .any(|note| note.as_str().unwrap().contains("radius")));
    assert_eq!(report["dirty"].as_array().unwrap()[0], dot);

    // The document holds the number in the slot the user meant.
    let snapshot: Value = serde_json::from_str(&engine.get_snapshot()).unwrap();
    assert_eq!(snapshot["scene"]["nodes"][&dot]["primitive"]["r"], 8.0);
}

#[test]
fn an_unknown_prompt_fails_gracefully_and_changes_nothing() {
    let mut engine = VectraEngine::new();
    rect(&mut engine, "card", 0.0, 0.0, 80.0, 60.0);
    let before = engine.get_snapshot();

    let response = execute(&mut engine, "reticulate the splines");
    assert_eq!(response["status"], "error");
    assert_eq!(response["code"], "unrecognized-prompt");
    assert!(
        response["message"]
            .as_str()
            .unwrap()
            .contains("add a circle"),
        "the message says what the planner does understand: {response}"
    );
    assert_eq!(engine.get_snapshot(), before, "nothing applied");
}

#[test]
fn a_hallucinated_id_is_caught_before_the_engine_sees_it() {
    let mut engine = VectraEngine::new();
    rect(&mut engine, "card", 0.0, 0.0, 80.0, 60.0);
    let before = engine.get_snapshot();

    // A plan naming a node that is not in the summary: refused by the context
    // check (the AI layer), not by the engine.
    let plan = r#"[{"type":"DeleteNode","id":"99999999-9999-4999-8999-999999999999"}]"#;
    let response: Value =
        serde_json::from_str(&engine.ai_execute_commands("delete the purple one", plan)).unwrap();
    assert_eq!(response["status"], "error");
    assert_eq!(response["code"], "engine-rejected");
    assert!(
        response["message"].as_str().unwrap().contains("99999999"),
        "{response}"
    );
    assert_eq!(engine.get_snapshot(), before);
}

// ── determinism and the empty document ─────────────────────────────────

#[test]
fn the_ai_path_is_deterministic_and_an_empty_document_is_fine() {
    let script = |engine: &mut VectraEngine| {
        rect(engine, "card", 0.0, 0.0, 80.0, 60.0);
        dispatch(
            engine,
            json!({"type": "SetVariable", "name": "base", "value": 40.0}),
        );
    };

    let mut first = VectraEngine::new();
    script(&mut first);
    let a: Value =
        serde_json::from_str(&first.ai_generate_commands("set the width of card to $base * 3", ""))
            .unwrap();
    let b: Value =
        serde_json::from_str(&first.ai_generate_commands("set the width of card to $base * 3", ""))
            .unwrap();
    // A `$new:` placeholder mints a fresh id every call — that is the one part
    // that may differ. Everything else about the plan is deterministic.
    assert_ne!(
        a["plan"][0]["id"], b["plan"][0]["id"],
        "ids are minted per call"
    );
    assert_eq!(
        scrub(&a),
        scrub(&b),
        "the same prompt on the same document plans the same commands"
    );
    assert_eq!(a["plan"][0]["source"], "$base * 3");
    assert_eq!(
        a["plan"][1]["value"]["Float"]["Expression"], a["plan"][0]["id"],
        "the slot points at the expression this very plan defines"
    );

    // An empty document still answers: the summary is empty and the planner
    // still understands a creation.
    let mut empty = VectraEngine::new();
    let response = execute(&mut empty, "add a circle named solo at 0 0 radius 10");
    assert_eq!(response["status"], "ok");
    assert_eq!(response["report"]["plan"].as_array().unwrap().len(), 1);
    let summary: Value = serde_json::from_str(&empty.document_summary()).unwrap();
    assert_eq!(summary["nodes"][0]["label"], "Circle 'solo'");

    // The negative path on an empty document is typed, not a panic.
    let response = execute(&mut empty, "delete everything");
    assert_eq!(response["status"], "error");
    assert_eq!(response["code"], "unrecognized-prompt");
}
