//! Task 10.0's laws, natively — no Tauri, no GUI, no browser.
//!
//! The desktop app is a *host*: a dialog, a file, and a window around the engine
//! it already has. Everything that could be wrong about it is testable here, and
//! this file proves the three things the task names:
//!
//! * **the container** — a `.vectra` file round-trips byte-for-byte, is small
//!   because it is compressed, and refuses to pretend to be a document when it
//!   is not one;
//! * **the Replay Law** — a plan compiled from a saved document, applied to a
//!   *fresh* engine through `dispatch_json` (the very serialization the WebView
//!   uses), produces the identical document: same order, same parameters, same
//!   parametric sources, same styles, constraints, operations, tracks and
//!   procedural chain;
//! * **the Roundtrip Law** — save → close → reopen → the `DocumentSummary`
//!   matches the original *exactly*, compared as `serde_json::Value` so a key
//!   order can never hide a missing field.
//!
//! Every command goes through `dispatch_json` rather than `dispatch`, because
//! that is the path a loaded file actually takes on the user's machine: a String
//! crosses the FFI boundary, and if that path dropped a field this file is where
//! it would show up.

use serde_json::Value;
use vectra_core::summary::DocumentSummary;
use vectra_core::{Engine, NodeId, NodeKind};

use vectra_file::format::{decode, encode, is_vectra, version_of, DocError};
use vectra_file::plan::{commands_to_json, document_from_json, document_to_commands, PlanReport};

/// The engine side of a round trip: a fresh document, authored through commands
/// so it looks like a real session rather than a fixture.
fn authored() -> Engine {
    let mut engine = Engine::new();
    apply(
        &mut engine,
        r##"[
        {"type":"SetVariable","name":"base","value":40.0},
        {"type":"SetVariable","name":"accent","value":1.0},
        {"type":"DefineExpression","id":"e0000000-0000-4000-8000-000000000001","source":"$base * 2"},
        {"type":"CreateNode","id":"c0000000-0000-4000-8000-000000000001","name":"card",
         "kind":{"Rectangle":{"x":{"Literal":0},"y":{"Literal":0},
                              "width":{"Expression":"e0000000-0000-4000-8000-000000000001"},
                              "height":{"Literal":60},"corner_radius":{"Literal":8}}}},
        {"type":"CreateNode","id":"c0000000-0000-4000-8000-000000000002","name":"dot",
         "kind":{"Circle":{"cx":{"Literal":10},"cy":{"Literal":20},"radius":{"Literal":5}}}},
        {"type":"SetParameter","node_id":"c0000000-0000-4000-8000-000000000002",
         "property":"style.fill","value":{"Color":{"Literal":{"r":255,"g":0,"b":0,"a":255}}}},
        {"type":"SetParameter","node_id":"c0000000-0000-4000-8000-000000000002",
         "property":"x","value":{"Float":{"Variable":"base"}}},
        {"type":"SetParameter","node_id":"c0000000-0000-4000-8000-000000000001",
         "property":"style.stroke_width","value":{"Float":{"Literal":2.5}}},
        {"type":"AddConstraint","constraint":{"id":"a0000000-0000-4000-8000-000000000001",
         "kind":"vertical","targets":[{"node_id":"c0000000-0000-4000-8000-000000000001","property":"x"},
                                      {"node_id":"c0000000-0000-4000-8000-000000000002","property":"cx"}],
         "strength":"required","value":null}},
        {"type":"ApplyOperation","id":"b0000000-0000-4000-8000-000000000001",
         "kind":{"type":"boolean","op":"subtract"},
         "inputs":["c0000000-0000-4000-8000-000000000001","c0000000-0000-4000-8000-000000000002"]},
        {"type":"SetMotionTrack","track":{"id":"rise","name":"Rise","channels":{"height":[
            {"time":0.0,"value":0.0},{"time":1.0,"value":100.0}]}}},
        {"type":"AddProceduralNode","node":{"id":"d0000000-0000-4000-8000-000000000001",
         "kind":{"type":"noise","amplitude":{"Literal":1.0},"frequency":{"Literal":0.2},
                 "seed":{"Literal":3.0}},"name":"grain","enabled":true}},
        {"type":"SetProceduralEnabled","id":"d0000000-0000-4000-8000-000000000001","enabled":false}
    ]"##,
    );
    engine
}

/// Send one command through the engine's JSON door and insist it was accepted.
///
/// `Engine::dispatch_json` answers `{"ok":true,"events":[…]}` — the *core*
/// envelope, which `vectra-wasm::dispatch_command` wraps as
/// `{"status":"ok","events":[…]}` for the WebView. A loaded file takes the wasm
/// door in the app and this one natively; the plan logic has to be right under
/// both, so these tests use the core door and speak its shape.
fn send(engine: &mut Engine, command_json: &str) -> Value {
    let reply: Value =
        serde_json::from_str(&engine.dispatch_json(command_json)).expect("always JSON");
    assert_eq!(
        reply["ok"], true,
        "command refused: {command_json} → {reply}"
    );
    reply
}

fn apply(engine: &mut Engine, plan_json: &str) {
    let commands: Vec<Value> = serde_json::from_str(plan_json).expect("the fixture plan parses");
    for command in commands {
        send(engine, &command.to_string());
    }
}

/// Rebuild a document from its own commands, the way a load does.
fn reopen(engine: &Engine) -> Engine {
    let plan = document_to_commands(engine.document());
    let mut fresh = Engine::new();
    for command in &plan {
        send(&mut fresh, &serde_json::to_string(command).unwrap());
    }
    fresh
}

fn document_json(engine: &Engine) -> String {
    serde_json::to_string(engine.document()).expect("a document always serializes")
}

/// Two engines hold the same document.
///
/// Compared as *values*, not as strings: `Document`'s collections are
/// `HashMap`s, so the serialization order of `variables` / `nodes` is whatever
/// the hasher felt like and is not document state. Everything that *is* state —
/// ids, order, parameters, styles, registries, enabled flags — shows up here.
fn a_document_value(engine: &Engine) -> Value {
    serde_json::from_str(&document_json(engine)).unwrap()
}

fn assert_same_document(a: &Engine, b: &Engine, what: &str) {
    assert_eq!(a_document_value(a), a_document_value(b), "{what}");
}

fn summary(engine: &Engine) -> DocumentSummary {
    DocumentSummary::capture(engine.document())
}

// ── the container ──────────────────────────────────────────────────────

#[test]
fn a_vectra_file_is_a_header_and_a_gzip_stream() {
    let engine = authored();
    let json = document_json(&engine);
    let bytes = encode(&json).expect("encode");

    assert_eq!(&bytes[..6], b"VECTRA", "the magic is the first six bytes");
    assert_eq!(
        version_of(&bytes).unwrap(),
        1,
        "the version rides in the header"
    );
    assert_eq!(&bytes[8..10], &[0x1f, 0x8b], "then a gzip stream");
    assert!(is_vectra(&bytes));

    // It is *compressed*: JSON of this shape repeats itself a lot.
    assert!(
        bytes.len() < json.len(),
        "gzip made it bigger: {} vs {}",
        bytes.len(),
        json.len()
    );

    // And it round-trips exactly — the JSON the engine wrote is the JSON back.
    assert_eq!(decode(&bytes).expect("decode"), json);
}

#[test]
fn the_same_document_always_encodes_to_the_same_bytes() {
    // Pinned gzip header (mtime 0, OS byte 255): two saves of an unchanged
    // document are byte-identical, which is what lets a test — or a user's
    // diff — reason about them.
    let engine = authored();
    let json = document_json(&engine);
    assert_eq!(encode(&json).unwrap(), encode(&json).unwrap());
}

#[test]
fn a_corrupt_or_foreign_file_is_refused_with_a_typed_error() {
    let engine = authored();
    let mut bytes = encode(&document_json(&engine)).unwrap();

    // Something else entirely.
    let png = b"\x89PNG\r\n\x1a\nthis is not a document".to_vec();
    assert!(matches!(decode(&png), Err(DocError::NotAVectraFile { .. })));

    // Truncated mid-header.
    assert!(matches!(
        decode(&bytes[..4]),
        Err(DocError::Truncated { .. })
    ));

    // A future version: refused by *version*, not mistaken for damage.
    let mut future = bytes.clone();
    let version = 2u16.to_le_bytes();
    future[6] = version[0];
    future[7] = version[1];
    assert!(matches!(
        decode(&future),
        Err(DocError::UnsupportedVersion {
            found: 2,
            supported: 1
        })
    ));

    // Right header, payload that is not gzip at all.
    let mut unzipped = bytes.clone();
    unzipped.truncate(8);
    unzipped.extend_from_slice(b"{\"version\":1}");
    assert!(matches!(
        decode(&unzipped),
        Err(DocError::NotCompressed { .. })
    ));

    // A gzip stream whose payload is not JSON.
    let not_json = encode("{\"version\": 1}").unwrap();
    assert!(decode(&not_json).is_err() || document_from_json("{\"version\": 1}").is_err());

    // Damaged bytes inside the gzip stream: corrupt, not a panic.
    bytes[12] ^= 0xff;
    bytes[20] ^= 0x0f;
    let damaged = decode(&bytes);
    assert!(
        matches!(
            damaged,
            Err(DocError::Corrupt { .. }) | Err(DocError::NotJson { .. })
        ),
        "expected a typed refusal, got {damaged:?}"
    );
}

#[test]
fn encoding_refuses_json_that_is_not_json() {
    assert!(matches!(
        encode("not a document"),
        Err(DocError::NotJson { .. })
    ));
    // …so a save can never produce a file a reader would reject as garbage.
    assert!(encode("{\"version\":1}").is_ok());
}

// ── the Replay Law ─────────────────────────────────────────────────────

#[test]
fn commands_rebuild_the_document_into_a_fresh_engine() {
    let engine = authored();
    let reopened = reopen(&engine);

    // The deepest comparison available: the whole document as JSON. A missing
    // style, a flattened expression or a lost wire changes it.
    assert_same_document(
        &reopened,
        &engine,
        "the replay did not reproduce the document",
    );
}

#[test]
fn the_replay_keeps_the_parametric_sources_and_the_registries() {
    let engine = authored();
    let reopened = reopen(&engine);
    let summary = summary(&reopened);

    // The moat: an expression-bound width is still bound to the same expression.
    let card = summary.find_node("card").unwrap();
    let width = card
        .slots
        .iter()
        .find(|slot| slot.property == "width")
        .unwrap();
    assert_eq!(
        width.source, "$base * 2",
        "the width was flattened by the round trip"
    );
    // Its *value* is `None` here on purpose: a summary captured without an
    // evaluator can resolve variables (they live in the document) but not
    // expressions (they live in `vectra-expression`). The desktop host has both
    // — it asks the live `VectraEngine` for `document_summary()` — so this is a
    // property of the fixture, not of the file.
    assert_eq!(
        width.value, None,
        "no expression engine, no expression value"
    );

    // The variable-bound slot next door survives as a *reference* — and it does
    // resolve, because the variable is part of the document that came back.
    // (The fixture set the alias `x`; a circle's canonical slot is `cx`, and the
    // document stores it under the canonical name — the summary lists `cx`.)
    let dot = summary.find_node("dot").unwrap();
    let cx = dot
        .slots
        .iter()
        .find(|slot| slot.property == "cx")
        .expect("the circle's cx slot");
    assert_eq!(cx.source, "$base");
    assert_eq!(cx.value, Some(40.0), "$base came back with its value");

    // Every registry came back, and the style that differed from the default
    // was re-applied (the other three slots were left at their defaults).
    assert_eq!(summary.variables.len(), 2);
    assert_eq!(summary.expressions.len(), 1);
    assert_eq!(summary.constraints.len(), 1);
    assert_eq!(summary.operations.len(), 1);
    assert_eq!(summary.tracks.len(), 1);
    assert_eq!(summary.procedural.len(), 1);
    assert_eq!(
        summary
            .nodes
            .iter()
            .map(|n| n.name.as_str())
            .collect::<Vec<_>>(),
        vec!["card", "dot"],
        "draw order is preserved"
    );

    // A parked procedural node stays parked — that is a *state*, not a default.
    let parked = summary
        .procedural
        .iter()
        .find(|node| node.name == "grain")
        .expect("the noise node came back");
    assert!(
        !parked.enabled,
        "enabled:false did not survive the round trip"
    );

    // The constraint's targets are the same slots.
    assert!(summary.constraints[0]
        .targets
        .iter()
        .any(|target| target.ends_with(".cx")));
}

#[test]
fn the_plan_is_emitted_in_dependency_order() {
    let engine = authored();
    let commands = document_to_commands(engine.document());
    let tags: Vec<&str> = commands
        .iter()
        .map(|command| match command {
            vectra_core::Command::SetVariable { .. } => "variable",
            vectra_core::Command::DefineExpression { .. } => "expression",
            vectra_core::Command::SetMotionTrack { .. } => "track",
            vectra_core::Command::CreateNode { .. } => "node",
            vectra_core::Command::SetParameter { .. } => "parameter",
            vectra_core::Command::AddConstraint { .. } => "constraint",
            vectra_core::Command::ApplyOperation { .. } => "operation",
            vectra_core::Command::AddProceduralNode { .. } => "procedural",
            vectra_core::Command::SetProceduralEnabled { .. } => "park",
            _ => "other",
        })
        .collect();
    let position = |needle: &str| tags.iter().position(|tag| *tag == needle).unwrap();

    assert!(position("variable") < position("expression"), "{tags:?}");
    assert!(position("expression") < position("node"), "{tags:?}");
    assert!(position("track") < position("node"), "{tags:?}");
    // The card is appended first (back), its own style slot travels with it.
    assert!(position("node") < position("parameter"), "{tags:?}");
    assert!(position("node") < position("constraint"), "{tags:?}");
    assert!(position("constraint") < position("operation"), "{tags:?}");
    assert!(position("operation") < position("procedural"), "{tags:?}");
    assert!(position("procedural") < position("park"), "{tags:?}");
}

#[test]
fn the_report_counts_what_the_plan_carries() {
    let engine = authored();
    let report = PlanReport {
        commands: 0,
        ..Default::default()
    };
    assert!(report.headline().starts_with("0 command(s)"));

    let commands = document_to_commands(engine.document());
    let json = commands_to_json(&commands).expect("a plan is JSON-serializable");
    assert_eq!(json.len(), commands.len());
    // Every planned command round-trips through the wire form the bridge uses.
    for (command, value) in commands.iter().zip(&json) {
        let back: vectra_core::Command = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(&back, command);
    }
}

#[test]
fn saving_an_empty_document_is_a_valid_empty_file() {
    let engine = Engine::new();
    let bytes = encode(&document_json(&engine)).unwrap();
    let json = decode(&bytes).unwrap();
    let document = document_from_json(&json).unwrap();
    assert_eq!(document.nodes.len(), 0);
    assert!(
        document_to_commands(&document).is_empty(),
        "nothing to replay"
    );
    let reopened = reopen(&engine);
    assert_same_document(&reopened, &engine, "an empty document did not round-trip");
}

#[test]
fn a_parked_operation_and_a_disabled_constraint_survive() {
    let mut engine = authored();
    apply(
        &mut engine,
        r##"[{"type":"SetOperationEnabled","id":"b0000000-0000-4000-8000-000000000001","enabled":false},
            {"type":"SetConstraintEnabled","id":"a0000000-0000-4000-8000-000000000001","enabled":false}]"##,
    );
    let reopened = reopen(&engine);
    let summary = summary(&reopened);
    assert!(
        !summary.operations[0].enabled,
        "the parked operation came back parked"
    );
    assert!(
        !summary.constraints[0].enabled,
        "the parked constraint came back parked"
    );
    assert_same_document(&reopened, &engine, "parked state changed the document");
}

// ── the Roundtrip Law ──────────────────────────────────────────────────

#[test]
fn law_roundtrip_save_close_reopen_matches_the_summary_exactly() {
    // Save: the engine's document, through the container the desktop app writes.
    let engine = authored();
    let json = document_json(&engine);
    let bytes = encode(&json).expect("save");
    let before: Value = serde_json::to_value(summary(&engine)).unwrap();
    let stated = summary(&engine).to_text();

    // Close. (The engine is dropped with the app; nothing of it is reused.)
    drop(engine);

    // Reopen: bytes → JSON → document → plan → a *fresh* engine → summary.
    let json = decode(&bytes).expect("open");
    let document = document_from_json(&json).expect("a .vectra payload is a Document");
    let mut reopened = Engine::new();
    for command in document_to_commands(&document) {
        send(&mut reopened, &serde_json::to_string(&command).unwrap());
    }

    let after: Value = serde_json::to_value(summary(&reopened)).unwrap();
    assert_eq!(
        before, after,
        "the DocumentSummary changed across the round trip"
    );

    // The prompt-facing text is identical too — a model grounded on this document
    // sees the same picture it saw before the save.
    assert_eq!(summary(&reopened).to_text(), stated);

    // And the document itself is equivalent — every field, every registry, every
    // parameter — not merely similar. (Compared as a value: two engines may
    // serialize their `HashMap`s in different key orders, which is not a
    // difference in the document. The *file* determinism claim is separate and
    // tested above: the same document encodes to the same bytes.)
    let original: Value = serde_json::from_str(&json).unwrap();
    assert_eq!(
        a_document_value(&reopened),
        original,
        "the document changed across the round trip"
    );
}

#[test]
fn an_id_survives_a_round_trip_so_old_plans_and_ai_prompts_still_address_it() {
    let engine = authored();
    let before = summary(&engine);
    let card_id = before.find_node("card").unwrap().id.clone();
    let reopened = reopen(&engine);
    let after = summary(&reopened);

    assert_eq!(after.find_node("card").unwrap().id, card_id);
    assert_eq!(
        after.resolve_node_id(&card_id),
        Some(card_id.parse::<NodeId>().unwrap()),
        "a stored id is still a valid handle after a save/load cycle"
    );

    // A node created *after* a load is a normal node: the plan is commands, not a
    // frozen document, so the engine's gates still apply.
    let mut live = reopened;
    send(
        &mut live,
        &serde_json::to_string(&vectra_core::Command::CreateNode {
            id: vectra_core::new_node_id(),
            kind: NodeKind::circle(0.0, 0.0, 3.0),
            name: Some("after".to_string()),
            index: None,
        })
        .unwrap(),
    );
    assert_eq!(summary(&live).nodes.len(), 3);
}
