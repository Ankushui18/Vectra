//! File verification, and the reason this module is short-lived engines rather
//! than a resident one.
//!
//! ## The finding: the engine is thread-confined
//!
//! The obvious design for a desktop host is one engine living in Tauri's managed
//! state, so the window can ask it anything. **That is impossible, and not
//! because of anything Tauri does.** Task 3.1 solved constraints with
//! `cassowary`, whose rows are `Rc<RefCell<Row>>` — so
//! `vectra_constraints::Solver`, and with it `vectra_wasm::VectraEngine`, is
//! `!Send`, and `State<T>` in Tauri requires `T: Send + Sync`. The compiler says
//! it precisely:
//!
//! ```text
//! error[E0277]: `Rc<RefCell<cassowary::Row>>` cannot be sent between threads safely
//! ```
//!
//! Task 10.0 RULE 1 forbids "refactor the engine to accommodate Tauri", so the
//! host does not try to keep one. Instead it does what a *file format* needs and
//! nothing more: every command that touches a document builds a short-lived
//! engine, replays the file through it, and drops it. The engine never crosses a
//! thread, because it never leaves the stack frame that made it — and no state
//! has to be kept in sync between the host and the window, because there is no
//! second document to disagree with.
//!
//! That is why this is *better* than the resident engine would have been, not
//! just possible: a host that verified with its own long-lived document could
//! drift from the window's; one that verifies a string per call cannot.
//!
//! ## What a verification is
//!
//! [`verify`] is Task 10.0's Roundtrip Law, callable on any document JSON:
//!
//! 1. parse it as a `Document` (a payload that is not one is refused here);
//! 2. compile its plan — the commands that rebuild it — with `vectra-file`;
//! 3. replay that plan into a fresh engine through `dispatch_command`, i.e. the
//!    *same* boundary the WebView uses, so the engine's own gates (cycles,
//!    unknown properties, unsatisfiable constraints) are the only authority;
//! 4. compare `DocumentSummary` to `DocumentSummary`.
//!
//! A file that fails any of those steps is refused with a sentence a user can
//! act on, and the rejection is what keeps an opened file from being half-trusted.
//!
//! ## What this proves, and what it does not
//!
//! It proves **the plan loses nothing**: every field the engine wrote into a
//! document survives being turned into commands and replayed. That is the claim
//! Task 10.0's Roundtrip Law is about, and it is the claim that matters, because
//! the engine is the only thing that writes a `.vectra` file.
//!
//! It does **not** prove a file was not edited. Both sides of the comparison come
//! from the payload, so a *self-consistent* hand edit (delete a node's entry from
//! `nodes`, and its id from `order`, and the file is coherent) verifies happily —
//! correctly so: a `.vectra` file is JSON, and JSON is a format people may edit.
//! What catches an incoherent file is the integrity check in [`integrity`]: every
//! registry keeps a map *and* an order vector, and a payload where those two
//! disagree did not come out of this engine.

use serde_json::Value;
use vectra_core::summary::DocumentSummary;
use vectra_wasm::VectraEngine;

use vectra_file::format::DocError;
use vectra_file::plan::{document_from_json, replay};

use crate::file_io::{DocumentCheck, SummaryDiff};

/// Parse, plan, replay, compare. The whole law in one function.
pub fn verify(document_json: &str) -> Result<DocumentCheck, DocError> {
    let document = document_from_json(document_json)?;
    let expected = DocumentSummary::capture(&document);
    let plan = replay(&document);

    let mut engine = VectraEngine::new();
    for (position, command) in plan.commands.iter().enumerate() {
        let command_json = serde_json::to_string(command).map_err(|error| DocError::NotJson {
            detail: error.to_string(),
        })?;
        let raw = engine.dispatch_command(&command_json);
        let reply: Value = serde_json::from_str(&raw).map_err(|error| DocError::NotJson {
            detail: format!("the engine answered something that is not JSON: {error}"),
        })?;
        if reply["status"] != "ok" {
            return Err(DocError::NotJson {
                detail: format!(
                    "the file describes something the engine refuses — step {} of {} ({}) was \
                     rejected: {}",
                    position + 1,
                    plan.commands.len(),
                    label(command),
                    reply["message"].as_str().unwrap_or("no reason given"),
                ),
            });
        }
    }

    // The replay built a document. Is it the one the file described?
    let rebuilt = document_from_json(&engine.document_json())?;
    let found = DocumentSummary::capture(&rebuilt);
    let mut diffs = integrity(&document);
    diffs.extend(compare(&expected, &found));

    Ok(DocumentCheck {
        matches: diffs.is_empty(),
        commands: plan.commands.len(),
        headline: plan.report.headline(),
        plan: plan
            .commands
            .iter()
            .map(|command| serde_json::to_value(command).unwrap_or(Value::Null))
            .collect(),
        nodes: expected.nodes.len(),
        variables: expected.variables.len(),
        expressions: expected.expressions.len(),
        constraints: expected.constraints.len(),
        operations: expected.operations.len(),
        tracks: expected.tracks.len(),
        procedural: expected.procedural.len(),
        summary: expected.to_text(),
        diffs,
    })
}

/// A payload is internally consistent iff every registry's map and its order
/// vector agree.
///
/// This is not a style preference: `nodes` is keyed storage, `order` is draw
/// order, and the plan follows the **order** — so a file that lists a node the
/// order does not mention is a file where one of the two is wrong. The engine
/// maintains both in lockstep; a file that does not is not a document this build
/// wrote, and the honest answer is to say so rather than to silently draw the
/// intersection.
fn integrity(document: &vectra_core::Document) -> Vec<SummaryDiff> {
    let mut diffs = Vec::new();
    let checks = [
        ("nodes/order", document.nodes.len(), document.order.len()),
        (
            "operations/order",
            document.operations.len(),
            document.operations.order.len(),
        ),
        (
            "procedural/order",
            document.procedural.len(),
            document.procedural.in_order().count(),
        ),
        (
            "motion/order",
            document.motion.len(),
            document.motion.ids().len(),
        ),
    ];
    for (field, in_map, in_order) in checks {
        if in_map != in_order {
            diffs.push(SummaryDiff {
                field: field.to_string(),
                expected: format!("{in_map} entry(ies) keyed"),
                found: format!("{in_order} entry(ies) in the order"),
            });
        }
    }
    diffs
}

/// Compare two summaries section by section, naming what disagrees.
///
/// Comparing JSON per section makes the check exhaustive: a style slot, a
/// parametric source, a parked flag or a procedural wire that changed shows up
/// as a named difference with a path, instead of as "not equal".
fn compare(expected: &DocumentSummary, found: &DocumentSummary) -> Vec<SummaryDiff> {
    let mut diffs = Vec::new();
    // The document's own shape version, before any section: a payload written by
    // a newer engine parses but is not this engine's document.
    if expected.version != found.version {
        diffs.push(SummaryDiff {
            field: "version".to_string(),
            expected: expected.version.to_string(),
            found: found.version.to_string(),
        });
    }
    let sections: [(&str, Vec<u8>, Vec<u8>); 7] = [
        ("nodes", json(&expected.nodes), json(&found.nodes)),
        (
            "variables",
            json(&expected.variables),
            json(&found.variables),
        ),
        (
            "expressions",
            json(&expected.expressions),
            json(&found.expressions),
        ),
        (
            "constraints",
            json(&expected.constraints),
            json(&found.constraints),
        ),
        (
            "operations",
            json(&expected.operations),
            json(&found.operations),
        ),
        ("tracks", json(&expected.tracks), json(&found.tracks)),
        (
            "procedural",
            json(&expected.procedural),
            json(&found.procedural),
        ),
    ];
    for (name, left, right) in sections {
        if left != right {
            diffs.push(SummaryDiff {
                field: name.to_string(),
                expected: excerpt(&left),
                found: excerpt(&right),
            });
        }
    }
    diffs
}

fn json<T: serde::Serialize>(value: &T) -> Vec<u8> {
    serde_json::to_vec(value).unwrap_or_default()
}

/// A bounded, character-safe excerpt of a section, for a status message.
fn excerpt(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    if text.chars().count() <= 240 {
        return text.into_owned();
    }
    let cut: String = text.chars().take(240).collect();
    format!("{cut}… ({} bytes)", bytes.len())
}

/// `CreateNode "card"` — enough for a refusal message to point at the step.
fn label(command: &vectra_core::Command) -> String {
    match command {
        vectra_core::Command::CreateNode { name, .. } => {
            format!("CreateNode {}", name.clone().unwrap_or_default())
        }
        vectra_core::Command::SetParameter { property, .. } => format!("SetParameter {property}"),
        vectra_core::Command::SetVariable { name, .. } => format!("SetVariable ${name}"),
        vectra_core::Command::DefineExpression { id, .. } => format!("DefineExpression {id}"),
        vectra_core::Command::AddConstraint { constraint } => {
            format!("AddConstraint {}", constraint.kind.tag())
        }
        vectra_core::Command::ApplyOperation { id, .. } => format!("ApplyOperation {id}"),
        vectra_core::Command::SetMotionTrack { track } => format!("SetMotionTrack {}", track.id),
        vectra_core::Command::AddProceduralNode { node } => {
            format!("AddProceduralNode {}", node.name)
        }
        other => format!("{other:?}").chars().take(48).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A document built by the engine, verified by a different engine.
    fn sample() -> String {
        let mut engine = VectraEngine::new();
        let node = vectra_core::new_node_id();
        let commands = [
            serde_json::json!({"type": "SetVariable", "name": "base", "value": 40.0}),
            serde_json::json!({
                "type": "CreateNode", "id": node.to_string(), "name": "card",
                "kind": {"Rectangle": {
                    "x": {"Literal": 0.0}, "y": {"Literal": 0.0},
                    "width": {"Variable": "base"}, "height": {"Literal": 60.0},
                    "corner_radius": {"Literal": 8.0}}}
            }),
            serde_json::json!({
                "type": "SetParameter", "node_id": node.to_string(),
                "property": "style.stroke_width", "value": {"Float": {"Literal": 2.5}}
            }),
        ];
        for command in commands {
            let reply: Value =
                serde_json::from_str(&engine.dispatch_command(&command.to_string())).unwrap();
            assert_eq!(reply["status"], "ok", "{reply}");
        }
        engine.document_json()
    }

    #[test]
    fn a_document_verifies_against_a_fresh_engine() {
        let check = verify(&sample()).expect("a document the engine wrote verifies");
        assert!(
            check.matches,
            "a document did not round-trip: {:?}",
            check.diffs
        );
        assert_eq!(check.nodes, 1);
        assert_eq!(check.variables, 1);
        assert!(
            check.commands >= 3,
            "the plan carries the variable and the node"
        );
        assert!(
            check.summary.contains("Rectangle 'card'"),
            "{}",
            check.summary
        );
        assert!(!check.headline.is_empty());
    }

    /// A payload whose registry maps and order vectors disagree is refused, and
    /// the diff names the registry — this is what a truncated or hand-edited
    /// file looks like.
    #[test]
    fn an_inconsistent_file_is_caught_before_it_is_trusted() {
        let json = sample();

        // Its node is keyed but not in the draw order: one of the two is wrong.
        let mut orphaned: Value = serde_json::from_str(&json).unwrap();
        orphaned["order"] = serde_json::json!([]);
        let check = verify(&orphaned.to_string()).expect("it is still a Document");
        assert!(
            !check.matches,
            "an orphaned node verified: {:?}",
            check.diffs
        );
        assert!(
            check.diffs.iter().any(|diff| diff.field == "nodes/order"),
            "the diff should name the registry: {:?}",
            check.diffs
        );

        // …and the reverse edit (a node in `nodes`, its id gone from `order` is
        // the same check seen from the other side).
        let mut extra: Value = serde_json::from_str(&json).unwrap();
        extra["nodes"][vectra_core::new_node_id().to_string()] = serde_json::json!({
            "id": vectra_core::new_node_id().to_string(),
            "name": "ghost",
            "kind": {"Circle": {"cx": {"Literal": 0.0}, "cy": {"Literal": 0.0}, "radius": {"Literal": 1.0}}},
            "style": {"fill": {"Literal": {"r": 0, "g": 0, "b": 0, "a": 255}},
                      "stroke": {"Literal": {"r": 0, "g": 0, "b": 0, "a": 0}},
                      "stroke_width": {"Literal": 0.0},
                      "opacity": {"Literal": 1.0}}
        });
        let check = verify(&extra.to_string()).expect("it is still a Document");
        assert!(!check.matches);
        assert!(
            check.diffs.iter().any(|diff| diff.field == "nodes/order"),
            "{:?}",
            check.diffs
        );
    }

    /// A field the *plan* cannot carry is reported. `version` is the honest
    /// example: the replay always rebuilds a document at this engine's version,
    /// so a file written by a newer engine is caught here rather than silently
    /// downgraded.
    #[test]
    fn a_version_the_replay_cannot_reproduce_is_reported() {
        let mut value: Value = serde_json::from_str(&sample()).unwrap();
        value["version"] = serde_json::json!(2);
        let check = verify(&value.to_string()).expect("a Document");
        assert!(!check.matches);
        assert!(
            check.diffs.iter().any(|diff| diff.field == "version"),
            "{:?}",
            check.diffs
        );
    }

    /// A file can parse as a `Document` and still describe something the engine
    /// refuses. The replay is what catches it, which is why loading is commands
    /// and not state injection.
    #[test]
    fn a_payload_the_engine_refuses_is_refused_here() {
        let mut value: Value = serde_json::from_str(&sample()).unwrap();
        value["expressions"] = serde_json::json!({
            "e0000000-0000-4000-8000-000000000001": {"source": "2 * * $base"}
        });
        let error = verify(&value.to_string()).expect_err("a broken expression is refused");
        let message = error.to_string();
        assert!(
            message.contains("the engine refuses") || message.contains("not a Document"),
            "unexpected refusal text: {message}"
        );
    }

    #[test]
    fn nonsense_is_refused_before_any_replay() {
        assert!(verify("not a document").is_err());
        assert!(verify("{\"version\": 1}").is_err());
    }
}
