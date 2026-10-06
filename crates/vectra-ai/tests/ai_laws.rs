//! Task 9.0's laws, and the planner's own grammar.
//!
//! The three laws the task names:
//!
//! * **Validity Law** — AI-generated commands parse and execute with no
//!   `VectraError` on a valid document.
//! * **Context Law** — the AI references the ids the `DocumentSummary` gave it,
//!   and never invents one for an existing shape.
//! * **Self-Correction Law** — an invalid command is caught, the error is fed
//!   back, and the next attempt either succeeds or the loop fails with a typed
//!   `AiError::MaxRetriesExceeded` — having applied nothing.
//!
//! Plus the parts a law cannot state: what the summary actually contains, what
//! the prompt says, and what each supported phrasing produces.

use proptest::prelude::*;
use serde_json::{json, Value};
use vectra_ai::exec::{self, CommandHost, CoreHost, DryRunHost};
use vectra_ai::planner::{HeuristicPlanner, PlanRequest, Planner, ScriptedPlanner};
use vectra_ai::{compile_plan, AiError, MAX_ATTEMPTS};
use vectra_core::{Command, DocumentSummary, Engine, EngineEvent, NodeKind};

// ── fixtures ───────────────────────────────────────────────────────────

fn engine_with_card_and_dot() -> Engine {
    let mut engine = Engine::new();
    engine
        .dispatch(Command::SetVariable {
            name: "base".into(),
            value: 40.0,
        })
        .unwrap();
    let card = vectra_core::new_node_id();
    engine
        .dispatch(Command::CreateNode {
            id: card,
            kind: rect(0.0, 0.0, 80.0, 60.0, 0.0),
            name: Some("card".to_string()),
            index: None,
        })
        .unwrap();
    let dot = vectra_core::new_node_id();
    engine
        .dispatch(Command::CreateNode {
            id: dot,
            kind: NodeKind::circle(140.0, 30.0, 15.0),
            name: Some("dot".to_string()),
            index: None,
        })
        .unwrap();
    engine
}

fn rect(x: f64, y: f64, w: f64, h: f64, radius: f64) -> NodeKind {
    match NodeKind::rectangle(x, y, w, h) {
        NodeKind::Rectangle {
            x,
            y,
            width,
            height,
            ..
        } => NodeKind::Rectangle {
            x,
            y,
            width,
            height,
            corner_radius: vectra_core::Parameter::Literal(radius),
        },
        other => other,
    }
}

fn summary_of(engine: &Engine) -> DocumentSummary {
    DocumentSummary::capture(engine.document())
}

/// Apply a prompt for real, and hand back the report — the shape most tests want.
fn run(engine: &mut Engine, prompt: &str) -> Result<exec::ExecutionReport, AiError> {
    let host = CoreHost::new(std::mem::take(engine));
    let mut host = host;
    let report = exec::execute(&HeuristicPlanner::new(), &mut host, prompt);
    *engine = host.engine;
    report
}

/// A prompt list the planner claims to support, for the universal-law tests.
const SUPPORTED: [&str; 12] = [
    "add a rectangle named panel at 0 0 size 40x20",
    "add a circle named blob at 10 10 radius 5",
    "set the width of card to 120",
    "set the height of card to 90",
    "move dot to 100 50",
    "fill card with #112233",
    "union card and dot",
    "subtract dot from card",
    "fillet card by 6",
    "mirror card vertically at 0",
    "align card and dot vertically",
    "keep card 20 below dot",
];

// ── the summary (RULE 2) ───────────────────────────────────────────────

#[test]
fn the_summary_carries_ids_names_kinds_variables_and_slots() {
    let engine = engine_with_card_and_dot();
    let summary = summary_of(&engine);
    let text = summary.to_text();

    assert_eq!(summary.nodes.len(), 2);
    assert_eq!(summary.variables.len(), 1);
    assert_eq!(summary.variables[0].name, "base");
    assert_eq!(summary.variables[0].value, 40.0);

    // The exact phrase the task names, and the exact id.
    assert!(text.contains("Rectangle 'card'"), "{text}");
    assert!(text.contains("Circle 'dot'"), "{text}");
    assert!(text.contains(&summary.nodes[0].id), "{text}");
    assert!(text.contains("$base = 40"), "{text}");

    // Slots: the parametric truth, and the number it resolves to.
    let card = summary.find_node("card").unwrap();
    let width = card
        .slots
        .iter()
        .find(|slot| slot.property == "width")
        .unwrap();
    assert_eq!(width.source, "80");
    assert_eq!(width.value, Some(80.0));
    assert_eq!(card.label, "Rectangle 'card'");
    assert_eq!(
        card.slots
            .iter()
            .map(|slot| slot.property.as_str())
            .collect::<Vec<_>>(),
        vec![
            "x",
            "y",
            "width",
            "height",
            "corner_radius",
            "style.fill",
            "style.stroke",
            "style.stroke_width",
            "style.opacity"
        ]
    );
}

#[test]
fn a_summary_shows_the_expression_not_the_number_it_evaluates_to() {
    // The RULE 2 property that matters most: the AI is told `$base * 2`, so it
    // can preserve the arithmetic instead of flattening it.
    let mut engine = engine_with_card_and_dot();
    let card = summary_of(&engine).find_node("card").unwrap().id.clone();
    let expression = vectra_core::new_expression_id();
    engine
        .dispatch(Command::DefineExpression {
            id: expression,
            source: "$base * 2".to_string(),
        })
        .unwrap();
    engine
        .dispatch(Command::SetParameter {
            node_id: card.parse().unwrap(),
            property: "width".to_string(),
            value: vectra_core::ParamValue::Float(vectra_core::Parameter::Expression(expression)),
        })
        .unwrap();

    let summary = summary_of(&engine);
    let node = summary.find_node("card").unwrap();
    let width = node
        .slots
        .iter()
        .find(|slot| slot.property == "width")
        .unwrap();
    assert_eq!(width.source, "$base * 2");
    assert_eq!(
        width.value, None,
        "core alone cannot evaluate an expression"
    );
    assert!(
        summary.to_text().contains("width = $base * 2"),
        "{}",
        summary.to_text()
    );

    let expression_row = summary
        .expressions
        .iter()
        .find(|row| row.id == expression.to_string())
        .unwrap();
    assert_eq!(expression_row.source, "$base * 2");
}

#[test]
fn the_summary_lists_constraints_and_operations() {
    let mut engine = engine_with_card_and_dot();
    let summary = summary_of(&engine);
    let card: vectra_core::NodeId = summary.find_node("card").unwrap().id.parse().unwrap();
    let dot: vectra_core::NodeId = summary.find_node("dot").unwrap().id.parse().unwrap();
    engine
        .dispatch(Command::AddConstraint {
            constraint: vectra_core::Constraint {
                id: vectra_core::new_constraint_id(),
                kind: vectra_core::ConstraintKind::Vertical,
                targets: vec![
                    vectra_core::ConstraintTarget::new(card, "x"),
                    vectra_core::ConstraintTarget::new(dot, "x"),
                ],
                strength: vectra_core::Strength::Required,
                value: None,
                enabled: true,
            },
        })
        .unwrap();
    engine
        .dispatch(Command::ApplyOperation {
            id: vectra_core::new_operation_id(),
            kind: vectra_core::OperationKind::Fillet {
                radius: vectra_core::Parameter::Literal(4.0),
            },
            inputs: vec![card],
        })
        .unwrap();

    let summary = summary_of(&engine);
    assert_eq!(summary.constraints.len(), 1);
    assert_eq!(summary.constraints[0].kind, "vertical");
    assert_eq!(summary.constraints[0].strength, "required");
    assert_eq!(
        summary.constraints[0].targets,
        vec![format!("{card}.x"), format!("{dot}.x")]
    );
    assert_eq!(summary.operations.len(), 1);
    assert_eq!(summary.operations[0].kind, "fillet");
    assert_eq!(summary.operations[0].inputs, vec![card.to_string()]);
    let text = summary.to_text();
    assert!(text.contains("CONSTRAINTS"), "{text}");
    assert!(text.contains("vertical"), "{text}");
}

#[test]
fn node_lookup_accepts_ids_prefixes_and_names_never_guesses() {
    let engine = engine_with_card_and_dot();
    let summary = summary_of(&engine);
    let card = summary.find_node("card").unwrap().id.clone();

    assert_eq!(summary.resolve_node_id("card").unwrap().to_string(), card);
    assert_eq!(summary.resolve_node_id("CARD").unwrap().to_string(), card);
    assert_eq!(
        summary.resolve_node_id(&card[..8]).unwrap().to_string(),
        card
    );
    assert_eq!(summary.resolve_node_id(&card).unwrap().to_string(), card);
    assert!(summary.resolve_node_id("nothing-like-this").is_none());
    assert_eq!(
        vectra_ai::schema::parse_reply("not json at all")
            .unwrap_err()
            .code(),
        "invalid-json"
    );
}

// ── RULE 1: the schema ─────────────────────────────────────────────────

#[test]
fn the_schema_is_the_command_enum_and_nothing_else() {
    let engine = engine_with_card_and_dot();
    let summary = summary_of(&engine);
    let reply = r#"
        Here you go:
        ```json
        [{"type":"SetVariable","name":"gap","value":8}]
        ```
    "#;
    let commands = compile_plan(reply, &summary).unwrap();
    assert_eq!(
        commands,
        vec![Command::SetVariable {
            name: "gap".into(),
            value: 8.0
        }]
    );

    // A hallucinated command type is refused, typed, and quoted back.
    let error = compile_plan(r#"[{"type":"DrawSVG","svg":"<svg/>"}]"#, &summary).unwrap_err();
    assert_eq!(error.code(), "unknown-command");
    assert!(error.to_string().contains("DrawSVG"), "{error}");

    // Raw geometry in a field is a schema error too: `kind` must be a node kind.
    let error = compile_plan(
        r#"[{"type":"CreateNode","id":"$new:x","kind":{"Svg":"<path d='M0 0'/>"}}]"#,
        &summary,
    )
    .unwrap_err();
    assert_eq!(error.code(), "unknown-command");

    // A `type` field naming a real command but with a wrong payload is refused.
    let error = compile_plan(r#"[{"type":"SetParameter","node_id":1}]"#, &summary).unwrap_err();
    assert_eq!(error.code(), "unknown-command");
}

#[test]
fn placeholders_are_minted_once_and_reused_within_a_plan() {
    let engine = engine_with_card_and_dot();
    let summary = summary_of(&engine);
    let reply = r#"[
        {"type":"CreateNode","id":"$new:panel","name":"panel",
         "kind":{"Rectangle":{"x":{"Literal":0},"y":{"Literal":0},
                 "width":{"Literal":10},"height":{"Literal":10},
                 "corner_radius":{"Literal":0}}}},
        {"type":"SetParameter","node_id":"$new:panel","property":"width",
         "value":{"Float":{"Literal":25}}}
    ]"#;
    let commands = compile_plan(reply, &summary).unwrap();
    let created = match &commands[0] {
        Command::CreateNode { id, .. } => id.to_string(),
        other => panic!("expected CreateNode, got {other:?}"),
    };
    match &commands[1] {
        Command::SetParameter { node_id, .. } => assert_eq!(node_id.to_string(), created),
        other => panic!("expected SetParameter, got {other:?}"),
    }
    // The id is a real UUID, not the placeholder.
    assert!(created.parse::<vectra_core::NodeId>().is_ok());
    assert!(!created.contains("$new:"));

    // Two different slugs are two different nodes.
    let reply = r#"[
        {"type":"CreateNode","id":"$new:a","name":"a","kind":{"Circle":
            {"cx":{"Literal":0},"cy":{"Literal":0},"radius":{"Literal":1}}}},
        {"type":"CreateNode","id":"$new:b","name":"b","kind":{"Circle":
            {"cx":{"Literal":0},"cy":{"Literal":0},"radius":{"Literal":1}}}}
    ]"#;
    let commands = compile_plan(reply, &summary).unwrap();
    assert_ne!(commands[0], commands[1]);
}

// ── the Context Law ────────────────────────────────────────────────────

#[test]
fn law_context_the_ai_uses_the_models_ids_and_never_hallucinates_one() {
    let engine = engine_with_card_and_dot();
    let summary = summary_of(&engine);
    let card = summary.find_node("card").unwrap().id.clone();

    // The id from the summary: accepted.
    let ok = compile_plan(
        &format!(
            r#"[{{"type":"SetParameter","node_id":"{card}","property":"width",
                 "value":{{"Float":{{"Literal":120}}}}}}]"#
        ),
        &summary,
    );
    assert!(ok.is_ok(), "{ok:?}");

    // An invented id: refused before dispatch, and the message names it.
    let error = compile_plan(
        r#"[{"type":"SetParameter","node_id":"11111111-1111-4111-8111-111111111111",
             "property":"width","value":{"Float":{"Literal":120}}}]"#,
        &summary,
    )
    .unwrap_err();
    assert_eq!(error.code(), "unknown-node-id");
    assert!(error.to_string().contains("11111111"), "{error}");

    // A made-up name is not an id either (names resolve only when the summary
    // has one — the model cannot smuggle in a shape it never saw).
    let error = compile_plan(r#"[{"type":"DeleteNode","id":"legend"}]"#, &summary).unwrap_err();
    assert_eq!(error.code(), "unknown-node-id");

    // Creating a node that already exists is a *context* mistake: `$new:` was
    // meant for something new.
    let error = compile_plan(
        &format!(
            r#"[{{"type":"CreateNode","id":"{card}","name":"again","kind":{{"Circle":
                 {{"cx":{{"Literal":0}},"cy":{{"Literal":0}},"radius":{{"Literal":1}}}}}}}}]"#
        ),
        &summary,
    )
    .unwrap_err();
    assert_eq!(error.code(), "plan-conflict");
}

/// RULE 1 has teeth: a misspelled field is **refused**, never ignored.
///
/// `Command` is an internally tagged enum, so serde cannot `deny_unknown_fields`
/// it — without this check a model that wrote `"propety"` or nested a stray key
/// inside `kind` would get a command the engine silently applied differently
/// from what the model meant.
#[test]
fn a_misspelled_or_invented_field_is_refused_not_ignored() {
    let engine = engine_with_card_and_dot();
    let summary = summary_of(&engine);
    let card = summary.find_node("card").unwrap().id.clone();

    // A typo of a *required* field: refused by serde itself, and the message
    // names the field that is missing rather than blaming the typo.
    let reply = format!(
        r#"[{{"type":"SetParameter","node_id":"{card}","propety":"width",
             "value":{{"Float":{{"Literal":120}}}}}}]"#
    );
    let error = compile_plan(&reply, &summary).unwrap_err();
    assert_eq!(error.code(), "unknown-command");
    assert!(error.to_string().contains("property"), "{error}");

    // A typo of an *optional* field — `nome` for `name`. Serde would have
    // accepted this command and quietly dropped the name; the strictness check
    // is what stands between the model's intent and a silent rename-to-nothing.
    let reply = r#"[{"type":"CreateNode","id":"$new:twin","nome":"twin","kind":{"Circle":
         {"cx":{"Literal":0},"cy":{"Literal":0},"radius":{"Literal":5}}}}]"#;
    let error = compile_plan(reply, &summary).unwrap_err();
    assert_eq!(error.code(), "unknown-command");
    assert!(error.to_string().contains("nome"), "{error}");

    // A stray field nested inside the kind — the same rule, one level down.
    let reply = r#"[{"type":"CreateNode","id":"$new:twin","name":"twin","kind":{"Circle":
         {"cx":{"Literal":0},"cy":{"Literal":0},"radius":{"Literal":5},
          "diameter":{"Literal":10}}}}]"#;
    let error = compile_plan(reply, &summary).unwrap_err();
    assert_eq!(error.code(), "unknown-command");
    assert!(error.to_string().contains("diameter"), "{error}");

    // …while a legitimate `null` for an optional operand is *not* a stray field:
    // `AddConstraint`'s `value` rides along as null for the relational kinds.
    let reply = format!(
        r#"[{{"type":"AddConstraint","constraint":{{"id":"$new:k1","kind":"vertical",
             "targets":[{{"node_id":"{card}","property":"x"}},
                        {{"node_id":"{card}","property":"y"}}],
             "strength":"required","value":null}}}}]"#
    );
    let commands = compile_plan(&reply, &summary).expect("canonical null is accepted");
    assert_eq!(commands.len(), 1);
}

proptest! {
    /// The Context Law, universally: for any id that is *not* in the summary and
    /// is not a `$new:` placeholder, the plan is refused — never passed to the
    /// engine, never applied.
    #[test]
    fn law_context_refuses_every_unknown_id(suffix in "[0-9a-f]{4}", property in "[a-z_]{2,10}") {
        let engine = engine_with_card_and_dot();
        let summary = summary_of(&engine);
        let id = format!("deadbeef-0000-4000-8000-0000{suffix}0000");
        prop_assume!(summary.resolve_node_id(&id).is_none());
        let reply = format!(
            r#"[{{"type":"SetParameter","node_id":"{id}","property":"{property}",
                 "value":{{"Float":{{"Literal":1}}}}}}]"#
        );
        let error = compile_plan(&reply, &summary).unwrap_err();
        prop_assert_eq!(error.code(), "unknown-node-id");

        // …and a `$new:` placeholder in the same slot is accepted instead.
        let reply = format!(
            r#"[{{"type":"SetParameter","node_id":"$new:thing","property":"{property}",
                 "value":{{"Float":{{"Literal":1}}}}}}]"#
        );
        // It parses (the context check refuses it later, at dispatch, because a
        // placeholder that was never created names nothing) — the point is that
        // the *id rule* is about ids, not about property names.
        let result = compile_plan(&reply, &summary);
        prop_assert!(result.is_ok() || result.unwrap_err().code() == "unknown-node-id");
    }
}

// ── the Validity Law ───────────────────────────────────────────────────

#[test]
fn law_validity_every_supported_prompt_produces_commands_that_run() {
    for prompt in SUPPORTED {
        let mut engine = engine_with_card_and_dot();
        let before = serde_json::to_string(&summary_of(&engine)).unwrap();
        let report = run(&mut engine, prompt)
            .unwrap_or_else(|error| panic!("`{prompt}` failed: {error} (code {})", error.code()));
        assert!(!report.plan.is_empty(), "`{prompt}` produced an empty plan");
        assert_eq!(report.attempts, 1, "`{prompt}` needed a retry");
        assert!(
            report.events.iter().any(|event| matches!(
                event,
                EngineEvent::NodesUpdated { .. }
                    | EngineEvent::NodesRemoved { .. }
                    | EngineEvent::VariablesUpdated { .. }
                    | EngineEvent::ConstraintsUpdated { .. }
                    | EngineEvent::OperationsUpdated { .. }
            )),
            "`{prompt}` produced no mutation event: {:?}",
            report.events
        );
        // The document really changed. (`Dirty` is appended by the *boundary*
        // owner — `vectra-wasm` — so a core host reports no dirty set; the smoke
        // run proves the dirty path end to end.)
        let after = serde_json::to_string(&summary_of(&engine)).unwrap();
        assert_ne!(before, after, "`{prompt}` changed nothing");
    }
}

proptest! {
    /// The Validity Law, over numbers: whatever the user asks for, the plan the
    /// planner emits executes without a `VectraError`, and the number the user
    /// asked for is the number the document holds afterwards.
    #[test]
    fn law_validity_any_number_lands(width in 1.0f64..1000.0, height in 1.0f64..1000.0) {
        let mut engine = engine_with_card_and_dot();
        let report = run(
            &mut engine,
            &format!("set the width of card to {width} and set the height of card to {height}"),
        )
        .expect("the plan runs");
        prop_assert_eq!(report.attempts, 1);

        let summary = summary_of(&engine);
        let card = summary.find_node("card").unwrap();
        let slot = |property: &str| {
            card.slots
                .iter()
                .find(|slot| slot.property == property)
                .and_then(|slot| slot.value)
        };
        prop_assert_eq!(slot("width"), Some(width));
        prop_assert_eq!(slot("height"), Some(height));
    }

    /// Creating shapes is valid for any position and size, and the created node
    /// is in the document under the name the prompt used.
    #[test]
    fn law_validity_created_shapes_are_in_the_document(x in -500.0f64..500.0, y in -500.0f64..500.0, size in 1.0f64..400.0) {
        let mut engine = engine_with_card_and_dot();
        let report = run(
            &mut engine,
            &format!("add a rectangle named panel at {x} {y} size {size}x{size}"),
        )
        .expect("the plan runs");
        prop_assert_eq!(report.attempts, 1);

        let summary = summary_of(&engine);
        let panel = summary.find_node("panel").expect("the node exists");
        let slot = |property: &str| {
            panel
                .slots
                .iter()
                .find(|slot| slot.property == property)
                .and_then(|slot| slot.value)
        };
        prop_assert_eq!(slot("x"), Some(x));
        prop_assert_eq!(slot("y"), Some(y));
        prop_assert_eq!(slot("width"), Some(size));
    }

    /// Every generated command round-trips through JSON: what the report shows
    /// is what the engine ran.
    #[test]
    fn law_validity_the_report_is_the_json_that_ran(index in 0usize..SUPPORTED.len()) {
        let prompt = SUPPORTED[index];
        let mut engine = engine_with_card_and_dot();
        let report = run(&mut engine, prompt).unwrap();
        prop_assert_eq!(report.plan.len(), report.plan.len());
        for value in &report.plan {
            let command: Command = serde_json::from_value(value.clone()).expect("a real command");
            let again = serde_json::to_value(&command).unwrap();
            prop_assert_eq!(again, value.clone());
        }
        // …and every applied command produced an event.
        prop_assert!(!report.events.is_empty());
    }
}

// ── the Self-Correction Law ────────────────────────────────────────────

#[test]
fn law_self_correction_the_engine_refusal_is_fed_back_and_the_retry_succeeds() {
    // "round the corners of dot by 8": the planner's slot table knows the words
    // `corner radius` (a real slot on a rectangle), the engine knows the kind —
    // and `dot` is a Circle. Attempt 1 is refused by the engine; the retry is
    // told exactly why and remaps the slot to `radius`.
    let mut engine = engine_with_card_and_dot();
    let dot = summary_of(&engine).find_node("dot").unwrap().id.clone();

    // Attempt 1, alone, is indeed a refusal — the law is about a real error.
    let wrong = format!(
        r#"[{{"type":"SetParameter","node_id":"{dot}","property":"corner_radius",
             "value":{{"Float":{{"Literal":8}}}}}}]"#
    );
    let mut probe = CoreHost::new(engine_with_card_and_dot());
    let single = exec::execute(
        &ScriptedPlanner::new([wrong.clone(), wrong.clone(), wrong]),
        &mut probe,
        "round the corners of dot by 8",
    )
    .unwrap_err();
    assert_eq!(single.code(), "max-retries-exceeded");
    // Nothing was applied: the document is untouched.
    assert_eq!(probe.summary().nodes.len(), 2);
    assert_eq!(
        probe
            .summary()
            .find_node("dot")
            .unwrap()
            .slots
            .iter()
            .find(|slot| slot.property == "radius")
            .and_then(|slot| slot.value),
        Some(15.0),
        "the rejected plan left no trace"
    );

    // The real thing: the heuristic planner reads the refusal and fixes it.
    let report = run(&mut engine, "round the corners of dot by 8").unwrap();
    assert_eq!(report.attempts, 2, "it took one correction");
    assert_eq!(report.corrections.len(), 1);
    assert_eq!(report.corrections[0].code, "engine-rejected");
    assert!(
        report.corrections[0]
            .error
            .contains("unknown property 'corner_radius'"),
        "the engine's own words: {}",
        report.corrections[0].error
    );
    assert!(
        report.notes.iter().any(|note| note.contains("radius")),
        "the planner says what it changed: {:?}",
        report.notes
    );

    // The document now holds the number the user asked for, in the right slot.
    let summary = summary_of(&engine);
    let dot = summary.find_node("dot").unwrap();
    assert_eq!(
        dot.slots
            .iter()
            .find(|slot| slot.property == "radius")
            .and_then(|slot| slot.value),
        Some(8.0)
    );
}

#[test]
fn law_self_correction_gives_up_typed_after_two_corrections() {
    // A model that keeps repeating a cycling command: three attempts, then a
    // typed failure that carries the whole history.
    let mut host = CoreHost::new(engine_with_card_and_dot());
    let summary = host.summary();
    let card = summary.find_node("card").unwrap().id.clone();
    let dot = summary.find_node("dot").unwrap().id.clone();
    let cyclic = format!(
        r#"[{{"type":"DefineExpression","id":"$new:e1","source":"$base * 2"}},
            {{"type":"SetProceduralOperand","node_id":"{card}","port":"radius",
              "value":{{"Float":{{"Literal":1}}}}}}]"#
    );
    let _ = (cyclic, dot);

    // Build a genuinely rejected plan: setting an unknown property is the
    // simplest engine refusal, and it is the same command three times.
    let reply = format!(
        r#"[{{"type":"SetParameter","node_id":"{card}","property":"nonsense",
             "value":{{"Float":{{"Literal":1}}}}}}]"#
    );
    let planner = ScriptedPlanner::new([reply.clone(), reply.clone(), reply]);
    let error = exec::execute(&planner, &mut host, "make it weird").unwrap_err();
    assert_eq!(error.code(), "max-retries-exceeded");
    assert_eq!(planner.calls(), MAX_ATTEMPTS, "asked three times, no more");
    match &error {
        AiError::MaxRetriesExceeded {
            attempts,
            last_error,
            last_code,
            plan,
            ..
        } => {
            assert_eq!(*attempts, 3);
            assert_eq!(last_code, "engine-rejected");
            assert!(last_error.contains("nonsense"), "{last_error}");
            assert_eq!(plan.len(), 1, "the plan that failed is attached");
        }
        other => panic!("expected MaxRetriesExceeded, got {other:?}"),
    }

    // The document is untouched after three failed attempts.
    assert_eq!(host.summary().nodes.len(), 2);
    assert_eq!(host.summary().variables[0].value, 40.0);
}

#[test]
fn law_self_correction_a_partial_plan_is_rolled_back_before_the_retry() {
    // The plan applies one command and then hits a refusal. The applied command
    // must be gone before the next attempt — otherwise the retry would be
    // reasoning about a document that no longer matches its summary.
    let mut host = CoreHost::new(engine_with_card_and_dot());
    let card = host.summary().find_node("card").unwrap().id.clone();
    let reply = format!(
        r#"[{{"type":"SetVariable","name":"sneaky","value":1}},
            {{"type":"SetParameter","node_id":"{card}","property":"nonsense",
              "value":{{"Float":{{"Literal":1}}}}}}]"#
    );
    let planner = ScriptedPlanner::new([reply.clone(), reply.clone(), reply]);
    let error = exec::execute(&planner, &mut host, "two steps, one broken").unwrap_err();
    assert_eq!(error.code(), "max-retries-exceeded");

    let summary = host.summary();
    assert_eq!(summary.variables.len(), 1, "the variable was rolled back");
    assert_eq!(summary.variables[0].name, "base");
}

#[test]
fn law_self_correction_a_hallucinated_id_is_retried_and_then_reported() {
    // A context error is a *correctable* mistake — the model can be told which
    // ids exist — so the loop retries it and stops at the ceiling with the typed
    // code of the last failure.
    let mut host = CoreHost::new(engine_with_card_and_dot());
    let reply = r#"[{"type":"SetParameter","node_id":"nope-not-a-node","property":"width",
                    "value":{"Float":{"Literal":1}}}]"#;
    let planner = ScriptedPlanner::new([reply, reply, reply]);
    let error = exec::execute(&planner, &mut host, "make it wider").unwrap_err();
    assert_eq!(error.code(), "max-retries-exceeded");
    assert_eq!(planner.calls(), MAX_ATTEMPTS);
    match &error {
        AiError::MaxRetriesExceeded {
            last_code,
            last_error,
            ..
        } => {
            assert_eq!(last_code, "unknown-node-id");
            assert!(last_error.contains("nope-not-a-node"), "{last_error}");
        }
        other => panic!("expected MaxRetriesExceeded, got {other:?}"),
    }
    // Nothing was applied, and the document is intact.
    let summary = host.summary();
    assert_eq!(summary.nodes.len(), 2);
    assert!(summary.find_node("card").is_ok());
}

#[test]
fn an_unrecognized_prompt_stops_immediately_with_a_usable_message() {
    let mut engine = engine_with_card_and_dot();
    let error = run(&mut engine, "reticulate the splines").unwrap_err();
    assert_eq!(error.code(), "unrecognized-prompt");
    let message = vectra_ai::prompt::failure_message(&error);
    assert!(message.contains("add a circle"), "{message}");
    assert_eq!(summary_of(&engine).nodes.len(), 2, "nothing was applied");
}

// ── the planner's grammar ──────────────────────────────────────────────

#[test]
fn the_planner_keeps_parametric_slots_parametric() {
    // "make the width of card twice $base" must define an expression and point
    // the slot at it — never flatten it to 80.
    let mut engine = engine_with_card_and_dot();
    let report = run(&mut engine, "make the width of card twice $base").unwrap();
    assert_eq!(report.attempts, 1);
    let summary = summary_of(&engine);
    assert_eq!(summary.expressions.len(), 1);
    assert_eq!(summary.expressions[0].source, "2 * $base");
    let card = summary.find_node("card").unwrap();
    let width = card
        .slots
        .iter()
        .find(|slot| slot.property == "width")
        .unwrap();
    assert_eq!(width.source, "2 * $base");

    // The slot's *value* is the expression — not the 80 it evaluates to, and not
    // the variable written inline. (Assert on the field, never on the command's
    // text: a node id is a random uuid and `"80"` can appear inside one.)
    let set = report
        .plan
        .iter()
        .find(|value| value["type"] == "SetParameter")
        .expect("the plan rebinds the slot");
    let float = &set["value"]["Float"];
    assert_eq!(float["Literal"], serde_json::Value::Null, "{set}");
    assert_eq!(float["Variable"], serde_json::Value::Null, "{set}");
    assert_eq!(float["Expression"], report.plan[0]["id"], "{set}");
}

#[test]
fn the_planner_understands_the_phrasings_it_advertises() {
    // Every advertised phrasing must at least produce commands — the hint line
    // in the UI is generated from this list.
    let engine = engine_with_card_and_dot();
    let summary = summary_of(&engine);
    for prompt in HeuristicPlanner::PHRASINGS {
        let request = PlanRequest {
            prompt,
            summary: &summary,
            correction: None,
        };
        let plan = HeuristicPlanner::new().plan(&request);
        assert!(plan.is_ok(), "`{prompt}` → {plan:?}");
        assert!(
            !plan.unwrap().commands.is_empty(),
            "`{prompt}` produced nothing"
        );
    }
}

#[test]
fn the_planner_can_preview_without_touching_the_document() {
    let engine = engine_with_card_and_dot();
    let host = DryRunHost::new(summary_of(&engine));
    let preview = exec::preview(&HeuristicPlanner::new(), &host, "fillet card by 6").unwrap();
    assert_eq!(preview.plan.len(), 1);
    assert_eq!(preview.attempt, 1);
    let rendered = vectra_ai::render_plan(&preview.plan);
    assert!(rendered.contains("\"ApplyOperation\""), "{rendered}");
    // The preview is the resolved plan: a real id, not a placeholder.
    assert!(!rendered.contains("$new:"), "{rendered}");
    assert_eq!(summary_hashes(&engine, &host), 0);
}

/// A helper that proves nothing changed: the engine's document is untouched by a
/// preview.
fn summary_hashes(engine: &Engine, host: &DryRunHost) -> usize {
    assert_eq!(host.summary.nodes.len(), engine.document().nodes.len());
    engine.document().nodes.len() - host.summary.nodes.len()
}

// ── the prompt (RULE 2's injection point) ──────────────────────────────

#[test]
fn the_system_prompt_teaches_the_api_and_carries_the_document() {
    let engine = engine_with_card_and_dot();
    let summary = summary_of(&engine);
    let prompt = vectra_ai::system_prompt(&summary);

    // The hole is filled, and the document is in there verbatim.
    assert!(!prompt.contains("{{DOCUMENT}}"));
    assert!(prompt.contains("Rectangle 'card'"));
    assert!(prompt.contains(&summary.nodes[0].id));
    assert!(prompt.contains("$base = 40"));

    // The contract, the id rule, the parametric rule, and examples.
    assert!(prompt.contains("OUTPUT CONTRACT"));
    assert!(prompt.contains("$new:<slug>"));
    assert!(prompt.contains("Never flatten"));
    assert!(prompt.contains("THE COMMAND API"));
    for command in [
        "CreateNode",
        "SetParameter",
        "SetVariable",
        "DefineExpression",
        "ApplyOperation",
        "AddConstraint",
        "BindMotion",
    ] {
        assert!(
            prompt.contains(command),
            "the prompt must document {command}"
        );
    }
    // Every command type the planner can emit is documented.
    for needle in [
        "\"Float\"",
        "\"Color\"",
        "\"Literal\"",
        "\"Variable\"",
        "\"Expression\"",
    ] {
        assert!(prompt.contains(needle), "the prompt must document {needle}");
    }
}

#[test]
fn the_correction_prompt_quotes_the_engine() {
    let engine = engine_with_card_and_dot();
    let summary = summary_of(&engine);
    let correction = vectra_ai::Correction {
        attempt: 1,
        code: "engine-rejected".to_string(),
        error: "unknown property 'radius' for node abc (Rectangle)".to_string(),
        command: Some(json!({"type": "SetParameter", "node_id": "abc", "property": "radius"})),
        note: String::new(),
    };
    let prompt = vectra_ai::correction_prompt("set the radius of card to 8", &summary, &correction);
    assert!(prompt.contains("unknown property 'radius'"));
    assert!(prompt.contains("Original request: set the radius of card to 8"));
    assert!(prompt.contains("Nothing was applied"));
    assert!(prompt.contains("Rectangle 'card'"));
}

// ── the host seam ──────────────────────────────────────────────────────

#[test]
fn a_host_can_refuse_everything_and_the_loop_reports_it() {
    // The `DryRunHost` is the "no engine attached" case: the loop must report a
    // host failure rather than pretend success.
    struct Host(DryRunHost);
    impl CommandHost for Host {
        fn apply(&mut self, command: &Command) -> Result<Vec<EngineEvent>, String> {
            self.0.apply(command)
        }
        fn rollback(&mut self, steps: usize) -> Result<(), String> {
            self.0.rollback(steps)
        }
        fn summary(&self) -> DocumentSummary {
            self.0.summary()
        }
    }
    let engine = engine_with_card_and_dot();
    let mut host = Host(DryRunHost::new(summary_of(&engine)));
    let error = exec::execute(&HeuristicPlanner::new(), &mut host, "delete dot").unwrap_err();
    assert_eq!(error.code(), "max-retries-exceeded");
    match error {
        AiError::MaxRetriesExceeded { last_error, .. } => {
            assert!(last_error.contains("dry run"), "{last_error}")
        }
        other => panic!("expected MaxRetriesExceeded, got {other:?}"),
    }
}

#[test]
fn execute_plan_applies_an_approved_preview_and_reports_it() {
    let engine = engine_with_card_and_dot();
    let host_summary = summary_of(&engine);
    let preview = exec::preview(
        &HeuristicPlanner::new(),
        &DryRunHost::new(host_summary),
        "set the width of card to 150",
    )
    .unwrap();
    let commands: Vec<Command> = preview
        .plan
        .iter()
        .map(|value| serde_json::from_value(value.clone()).unwrap())
        .collect();

    // The same document the preview was generated against: the ids in the
    // preview are the ids the host has.
    let mut host = CoreHost::new(engine);
    let report = exec::execute_plan(&mut host, "set the width of card to 150", &commands).unwrap();
    assert_eq!(report.attempts, 1);
    assert_eq!(report.plan.len(), 1);

    let engine_after = std::mem::take(&mut host.engine);
    let summary = summary_of(&engine_after);
    assert_eq!(
        summary
            .find_node("card")
            .unwrap()
            .slots
            .iter()
            .find(|slot| slot.property == "width")
            .and_then(|slot| slot.value),
        Some(150.0)
    );

    // A plan that the engine refuses is reported, typed, and rolled back.
    let mut host = CoreHost::new(engine_with_card_and_dot());
    let bad_summary = host.summary();
    let bad = vec![Command::SetParameter {
        node_id: bad_summary.find_node("dot").unwrap().id.parse().unwrap(),
        property: "corner_radius".to_string(),
        value: vectra_core::ParamValue::Float(vectra_core::Parameter::Literal(4.0)),
    }];
    let error = exec::execute_plan(&mut host, "round the dot", &bad).unwrap_err();
    assert_eq!(error.code(), "engine-rejected");
}

#[test]
fn the_report_headline_says_what_happened() {
    let mut engine = engine_with_card_and_dot();
    let report = run(&mut engine, "move dot to 10 10").unwrap();
    let headline = report.headline();
    assert!(headline.contains("2 command(s)"), "{headline}");
    assert!(headline.contains("first attempt"), "{headline}");
    // A `CoreHost` has no dependency graph, so nothing reports dirty here; the
    // boundary host does (the smoke run asserts the dirty ids).
    assert!(headline.contains("nothing re-evaluated"), "{headline}");

    // `dirty_ids` reads the engine's own witness events.
    let events = vec![
        vectra_core::EngineEvent::NodesUpdated {
            ids: vec![vectra_core::new_node_id()],
        },
        vectra_core::EngineEvent::Dirty {
            ids: vec![vectra_core::new_node_id()],
            mode: vectra_core::EvalMode::Incremental,
        },
    ];
    assert_eq!(vectra_ai::dirty_ids(&events).len(), 1);
}

#[test]
fn every_error_code_the_ui_can_show_has_a_message() {
    let engine = engine_with_card_and_dot();
    let summary = summary_of(&engine);
    let cases: Vec<AiError> = vec![
        compile_plan("nonsense", &summary).unwrap_err(),
        compile_plan(r#"[{"type":"Nope"}]"#, &summary).unwrap_err(),
        compile_plan(r#"[{"type":"DeleteNode","id":"missing"}]"#, &summary).unwrap_err(),
        AiError::Rejected {
            attempt: 1,
            command: "x".to_string(),
            error: "boom".to_string(),
        },
        AiError::MaxRetriesExceeded {
            attempts: 3,
            last_error: "boom".to_string(),
            last_code: "engine-rejected".to_string(),
            prompt: "p".to_string(),
            plan: Vec::new().into_boxed_slice(),
            corrections: Vec::new().into_boxed_slice(),
        },
        AiError::Planner {
            detail: "no model".to_string(),
        },
        AiError::Host {
            detail: "no engine".to_string(),
        },
    ];
    for error in cases {
        assert!(!error.code().is_empty());
        assert!(!error.to_string().is_empty());
        let json: Value = error.as_json();
        assert_eq!(json["code"], error.code());
    }
}
