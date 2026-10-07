//! `Document` ⇄ commands: how a saved file becomes a live engine again.
//!
//! The engine has one door — [`vectra_core::Command`] — and this module is how
//! a file walks through it. Nothing here writes engine state directly: the
//! output is a plan the desktop layer sends through `dispatch_command`, exactly
//! as if a user had made each edit, so **the engine's own gates validate a
//! loaded file**. A `.vectra` that a hand-edit contradicted is refused at load
//! time, in the engine's words, instead of being trusted because it came from a
//! file dialog.
//!
//! ## Order
//!
//! A document is a graph, and commands are applied in order, so the plan is
//! emitted in dependency order:
//!
//! 1. **variables** — expressions and parameters read them;
//! 2. **expressions** — parameters reference them by id;
//! 3. **motion tracks** — a binding may name a track;
//! 4. **nodes**, in draw order (back → front), each followed by the style slots
//!    that differ from the default;
//! 5. **constraints** — their targets are `<node id>.<property>`;
//! 6. **operations** — their inputs are node ids;
//! 7. **procedural nodes**, in evaluation order — a `Source` reads the scene,
//!    and a wire points at a node earlier in that order.
//!
//! Determinism: every registry is walked in its own canonical order (draw
//! order, registry order, name order), never in `HashMap` order, so the same
//! document always compiles to the same plan — and the same file always
//! re-saves to the same bytes.

use serde_json::Value;
use vectra_core::{Command, Document, ParamValue, Parameter, StyleProperties};

use crate::format::DocError;

/// What a plan contains, for the status line and the log.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PlanReport {
    pub commands: usize,
    pub nodes: usize,
    pub variables: usize,
    pub expressions: usize,
    pub tracks: usize,
    pub constraints: usize,
    pub operations: usize,
    pub procedural: usize,
}

impl PlanReport {
    /// `42 command(s): 3 node(s), 1 variable(s), …` — one line, for the UI log.
    pub fn headline(&self) -> String {
        format!(
            "{} command(s): {} node(s), {} variable(s), {} expression(s), {} track(s), \
             {} constraint(s), {} operation(s), {} procedural node(s)",
            self.commands,
            self.nodes,
            self.variables,
            self.expressions,
            self.tracks,
            self.constraints,
            self.operations,
            self.procedural,
        )
    }
}

/// A planned replay: the typed commands and how much of each thing they carry.
#[derive(Debug, Clone, PartialEq)]
pub struct Replay {
    pub commands: Vec<Command>,
    pub report: PlanReport,
}

/// Parse a `Document` JSON string. The whole file's payload must be a document
/// this build understands; a partial parse is not a document.
pub fn document_from_json(json: &str) -> Result<Document, DocError> {
    serde_json::from_str::<Document>(json).map_err(|error| DocError::NotJson {
        detail: format!("not a Document: {error}"),
    })
}

/// `Document` → the commands that rebuild it in a fresh engine.
pub fn document_to_commands(document: &Document) -> Vec<Command> {
    replay(document).commands
}

/// [`document_to_commands`], with the report.
pub fn replay(document: &Document) -> Replay {
    let mut commands = Vec::new();

    // 1. Variables, in name order (a `HashMap` has no order worth writing down).
    let mut variables: Vec<(&String, &f64)> = document.variables.iter().collect();
    variables.sort_by(|a, b| a.0.cmp(b.0));
    for (name, value) in &variables {
        commands.push(Command::SetVariable {
            name: (*name).clone(),
            value: **value,
        });
    }

    // 2. Expressions, in id order.
    let mut expressions: Vec<(&vectra_core::ExpressionId, &vectra_core::ExpressionRecord)> =
        document.expressions.iter().collect();
    // Sorted by the id's text form: a `Uuid`'s `Ord` is byte order, which is not
    // what `BTreeMap`-style id order looks like on the wire. The plan is written
    // in the order a person would read it.
    expressions.sort_by_key(|a| a.0.to_string());
    for (id, record) in &expressions {
        commands.push(Command::DefineExpression {
            id: **id,
            source: record.source.clone(),
        });
    }

    // 3. Motion tracks, in registry order (a track is referenced by bindings
    //    inside node parameters, so it has to exist before the nodes land).
    for track_id in document.motion.ids() {
        if let Some(track) = document.motion.get(track_id) {
            commands.push(Command::SetMotionTrack {
                track: track.clone(),
            });
        }
    }

    // 4. Nodes, in draw order, with the style slots that differ from the default.
    for node_id in &document.order {
        let Some(node) = document.nodes.get(node_id) else {
            // A node in `order` but not in `nodes` is a broken document; skip it
            // rather than inventing one, and let the summary comparison catch the
            // difference loudly.
            continue;
        };
        commands.push(Command::CreateNode {
            id: node.id,
            kind: node.kind.clone(),
            name: Some(node.name.clone()),
            index: None,
        });
        for (property, value) in style_slots(&node.style) {
            commands.push(Command::SetParameter {
                node_id: node.id,
                property,
                value,
            });
        }
    }

    // 5. Constraints, in registry order.
    for constraint in document.constraints.iter() {
        commands.push(Command::AddConstraint {
            constraint: constraint.clone(),
        });
        if !constraint.enabled {
            commands.push(Command::SetConstraintEnabled {
                id: constraint.id,
                enabled: false,
            });
        }
    }

    // 6. Operations, in registry order (draw order for virtual nodes).
    for operation in document.operations.in_order() {
        commands.push(Command::ApplyOperation {
            id: operation.id,
            kind: operation.kind.clone(),
            inputs: operation.inputs.clone(),
            style: None,
            name: None,
        });
        if !operation.enabled {
            commands.push(Command::SetOperationEnabled {
                id: operation.id,
                enabled: false,
            });
        }
    }

    // 7. Procedural nodes, in evaluation order, wires and operands verbatim.
    for node in document.procedural.in_order() {
        commands.push(Command::AddProceduralNode { node: node.clone() });
        if !node.enabled {
            commands.push(Command::SetProceduralEnabled {
                id: node.id,
                enabled: false,
            });
        }
    }

    let report = PlanReport {
        commands: commands.len(),
        nodes: document.order.len(),
        variables: document.variables.len(),
        expressions: document.expressions.len(),
        tracks: document.motion.ids().len(),
        constraints: document.constraints.len(),
        operations: document.operations.len(),
        procedural: document.procedural.len(),
    };
    Replay { commands, report }
}

/// The plan as JSON — what crosses the Tauri bridge and what the UI logs.
pub fn commands_to_json(commands: &[Command]) -> Result<Vec<Value>, DocError> {
    commands
        .iter()
        .map(|command| {
            serde_json::to_value(command).map_err(|error| DocError::NotJson {
                detail: error.to_string(),
            })
        })
        .collect()
}

/// The style slots a default node does not already have.
///
/// A node's *geometry* rides inside its [`vectra_core::NodeKind`], so only the
/// four style properties need separate commands — and only when they are not
/// the default, because each one is a history entry and a fresh document should
/// not open with four no-op edits per node.
fn style_slots(style: &StyleProperties) -> Vec<(String, ParamValue)> {
    let default = StyleProperties::default();
    let mut slots = Vec::new();
    let mut push = |property: &str, value: ParamValue, is_default: bool| {
        if !is_default {
            slots.push((property.to_string(), value));
        }
    };
    push(
        "style.fill",
        ParamValue::Color(style.fill.clone()),
        style.fill == default.fill,
    );
    push(
        "style.stroke",
        ParamValue::Color(style.stroke.clone()),
        style.stroke == default.stroke,
    );
    push(
        "style.stroke_width",
        ParamValue::Float(style.stroke_width.clone()),
        style.stroke_width == default.stroke_width,
    );
    push(
        "style.opacity",
        ParamValue::Float(style.opacity.clone()),
        style.opacity == default.opacity,
    );
    slots
}

/// True when a parameter is a plain number. Kept public because the UI's
/// "is this slot parametric?" question is asked *of the file* during a load
/// report, and answering it here keeps the UI out of the document's shape.
pub fn is_literal(param: &Parameter<f64>) -> bool {
    matches!(param, Parameter::Literal(_))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_default_node_needs_no_style_commands() {
        assert!(style_slots(&StyleProperties::default()).is_empty());
    }

    #[test]
    fn a_literal_question_is_answered_by_the_parameter() {
        assert!(is_literal(&Parameter::Literal(1.0)));
        assert!(!is_literal(&Parameter::Variable("base".to_string())));
    }
}
