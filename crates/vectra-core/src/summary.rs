//! The AI-facing **document summary** (Task 9.0, RULE 2).
//!
//! > "The AI cannot guess what is on the canvas. You must implement a
//! > `DocumentSummary` struct in `vectra-core` that extracts a lightweight,
//! > AI-friendly representation of the current state: Node IDs, names, types
//! > (e.g. `"Rectangle 'card'"`), current variables, and active constraints.
//! > This summary is injected into the AI's system prompt so it can reference
//! > exact `NodeId`s."
//!
//! So this module answers three questions and nothing else:
//!
//! 1. **What exists, and what is it called?** Every node with its exact id, its
//!    name, its kind tag, and a one-line label (`Rectangle 'card'`).
//! 2. **What is each slot driven by?** The *parametric* source of every slot in
//!    Vectra's own syntax (`$base * 2`, not the number 80) plus the number it
//!    currently resolves to. An AI that only saw numbers would rewrite them and
//!    destroy the parametric graph; an AI that only saw sources could not tell
//!    whether the document is sane. Both, per slot.
//! 3. **What else is live?** Variables, expressions, operations, constraints,
//!    motion tracks and the procedural graph — the things a prompt like "align
//!    these two" or "fillet that" has to name.
//!
//! ## It is a *value*, not a view
//!
//! Everything here derives from [`Document`] (plus an [`EvaluationContext`] when
//! the caller has one, so motion samples and procedural reads resolve). It is
//! `Serialize`, it owns its strings, and it has no lifetimes — it can be posted
//! to a model, stored in a test fixture, or round-tripped through JSON with no
//! engine on the other side.
//!
//! ## Two ways to read it
//!
//! * [`DocumentSummary::to_text`] renders the block that goes into a system
//!   prompt (see `vectra-ai`'s prompt template). It is deliberately terse and
//!   stable: every line is `label → id → slots`, so a model can copy an id
//!   verbatim without parsing anything clever.
//! * the struct itself is the machine-readable form, for the boundary and for
//!   `vectra-ai`'s grounding checks.

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fmt::Write as _;

use crate::constraint::Constraint;
use crate::document::{Document, Node};
use crate::eval::{EvaluationContext, Resolvable};
use crate::ids::{NodeId, VariableId};
use crate::operation::OperationNode;
use crate::param::{MotionBinding, NodeOutputId, ParamValue, Parameter};
use crate::procedural::{PortType, ProceduralNode};

/// The style slots every node has, in inspector order.
///
/// Kept beside the summary rather than on `NodeKind` because it is a *view*
/// concern: geoemtry slots come from [`NodeKind::scalar_slots`], style slots are
/// uniform across kinds.
pub const STYLE_SLOTS: [&str; 4] = [
    "style.fill",
    "style.stroke",
    "style.stroke_width",
    "style.opacity",
];

/// How many nodes [`DocumentSummary::to_text_within`] prints before it switches
/// to a count. Models are grounded by the lines they can see; past this the
/// block costs more than it grounds, so the tail is elided *and labelled* (the
/// model is told names still work) rather than silently dropped.
pub const DEFAULT_NODE_BUDGET: usize = 60;

/// A lightweight, serializable, AI-facing view of a document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DocumentSummary {
    pub version: u32,
    /// Nodes in draw order (back → front), the order every other surface uses.
    pub nodes: Vec<SummaryNode>,
    /// Variables, sorted by name.
    pub variables: Vec<SummaryVariable>,
    /// Expression records, sorted by id.
    pub expressions: Vec<SummaryExpression>,
    /// Registered operations, in registry order.
    pub operations: Vec<SummaryOperation>,
    /// Constraints, in registry order.
    pub constraints: Vec<SummaryConstraint>,
    /// The procedural graph, in registry order.
    pub procedural: Vec<SummaryProcedural>,
    /// Motion tracks, sorted by name.
    pub tracks: Vec<SummaryTrack>,
}

/// One node: identity, name, kind, and every addressable slot.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SummaryNode {
    /// The **exact** id a command must carry. Never abbreviated here — an
    /// abbreviation is only useful to a human.
    pub id: String,
    pub name: String,
    /// The kind tag (`Rectangle`, `Circle`, `Arc`, `Path`, `Group`).
    pub kind: String,
    /// `Rectangle 'card'` — the phrase the task names, used as the line label.
    pub label: String,
    /// Every slot, geometry first, then style.
    pub slots: Vec<SummarySlot>,
}

/// One slot: what drives it, and what it currently is.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SummarySlot {
    /// The property path `SetParameter` takes (`width`, `style.fill`).
    pub property: String,
    /// The **parametric source**, in Vectra's syntax: `40`, `$base`,
    /// `$base * 2`, `spring → $target (from 0 @ 0s)`, `proc scatter.region`,
    /// `input pointer.x`. An expression is expanded, so the model sees the
    /// arithmetic it must preserve.
    pub source: String,
    /// The number the slot resolves to *right now*, when it is a numeric slot
    /// and the context can resolve it (a motion sample resolves only when the
    /// caller supplied a motion evaluator; a procedural read only with one too).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SummaryVariable {
    pub name: VariableId,
    pub value: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SummaryExpression {
    pub id: String,
    pub source: String,
    /// The expression's current value, when the context can evaluate it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SummaryOperation {
    pub id: String,
    /// `boolean`, `offset`, `fillet`, `mirror` — the kind tag, plus the boolean
    /// op where it has one (`boolean union`).
    pub kind: String,
    /// Input node ids, in slot order. These are the ids a `RemoveOperation` or a
    /// re-issue must name.
    pub inputs: Vec<String>,
    pub enabled: bool,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SummaryConstraint {
    pub id: String,
    /// `vertical`, `distance`, … — the wire tag.
    pub kind: String,
    /// The addressed slots, `"<node id>.<property>"`, in the order the solver
    /// documents.
    pub targets: Vec<String>,
    pub strength: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<f64>,
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SummaryProcedural {
    pub id: String,
    pub name: String,
    /// `source`, `grid`, `repeat`, `noise`, `smooth`.
    pub kind: String,
    pub enabled: bool,
    /// `"<port> ← <node id>.<port>"`, sorted by port.
    pub wires: Vec<String>,
    /// `"<port> = <value>"` for every published output, when the caller supplied
    /// a context that can read them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub outputs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SummaryTrack {
    pub id: String,
    pub name: String,
    /// Channel name → keyframe count, sorted.
    pub channels: Vec<String>,
}

/// Why a [`DocumentSummary`] could not turn a token into a node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NameLookup {
    /// Nothing matched: no id, no id prefix, no name.
    Unknown,
    /// Several nodes matched — the caller must be more specific.
    Ambiguous(Vec<String>),
}

impl NameLookup {
    pub fn found(&self) -> bool {
        matches!(self, Self::Unknown)
    }
}

impl DocumentSummary {
    /// Capture a document at `t = 0`, with no evaluators wired.
    ///
    /// Literals, variables and (re-rendered) expression *sources* are always
    /// captured; values that need an evaluator come out as `None`. That is a
    /// complete summary for grounding — which is what RULE 2 is for — and it is
    /// the only one a dependency-free `vectra-core` can build by itself.
    pub fn capture(doc: &Document) -> Self {
        Self::capture_in(doc, &doc.evaluation_context(0.0))
    }

    /// Capture a document against a live context — the form the engine uses, so
    /// that a spring exports the number it is drawing and a procedural read its
    /// published value.
    pub fn capture_in(doc: &Document, ctx: &EvaluationContext<'_>) -> Self {
        let nodes = doc
            .order
            .iter()
            .filter_map(|id| doc.nodes.get(id))
            .map(|node| Self::node_summary(doc, node, ctx))
            .collect();

        let mut variables: Vec<SummaryVariable> = doc
            .variables
            .iter()
            .map(|(name, value)| SummaryVariable {
                name: name.clone(),
                value: *value,
            })
            .collect();
        variables.sort_by(|a, b| a.name.cmp(&b.name));

        let mut expressions: Vec<SummaryExpression> = doc
            .expressions
            .iter()
            .map(|(id, record)| SummaryExpression {
                id: id.to_string(),
                source: record.source.clone(),
                value: ctx
                    .expression
                    .and_then(|ev| ev.evaluate(*id, ctx).ok())
                    .filter(|value| value.is_finite()),
            })
            .collect();
        expressions.sort_by(|a, b| a.id.cmp(&b.id));

        let operations = doc
            .operations
            .in_order()
            .map(Self::operation_summary)
            .collect();

        let constraints = doc
            .constraints
            .iter()
            .map(Self::constraint_summary)
            .collect();

        let procedural = doc
            .procedural
            .in_order()
            .map(|node| Self::procedural_summary(node, ctx))
            .collect();

        let mut tracks: Vec<SummaryTrack> = doc
            .motion
            .ids()
            .iter()
            .filter_map(|id| doc.motion.get(id).map(|track| (id, track)))
            .map(|(id, track)| SummaryTrack {
                id: id.to_string(),
                name: track.name.clone(),
                channels: track
                    .channels
                    .iter()
                    .map(|(name, keys)| format!("{name} ×{}", keys.len()))
                    .collect(),
            })
            .collect();
        tracks.sort_by(|a, b| a.name.cmp(&b.name));

        Self {
            version: doc.version,
            nodes,
            variables,
            expressions,
            operations,
            constraints,
            procedural,
            tracks,
        }
    }

    fn node_summary(doc: &Document, node: &Node, ctx: &EvaluationContext<'_>) -> SummaryNode {
        let mut slots = Vec::new();
        for property in node.kind.scalar_slots() {
            if let Ok(value) = node.get_param(property) {
                slots.push(Self::slot(doc, property, &value, ctx));
            }
        }
        for property in STYLE_SLOTS {
            if let Ok(value) = node.get_param(property) {
                slots.push(Self::slot(doc, property, &value, ctx));
            }
        }
        SummaryNode {
            id: node.id.to_string(),
            name: node.name.clone(),
            kind: node.kind.tag().to_string(),
            label: format!("{} '{}'", node.kind.tag(), node.name),
            slots,
        }
    }

    fn slot(
        doc: &Document,
        property: &str,
        value: &ParamValue,
        ctx: &EvaluationContext<'_>,
    ) -> SummarySlot {
        let (source, number) = match value {
            ParamValue::Float(param) => (describe_scalar(doc, param), param.resolve(ctx).ok()),
            ParamValue::Point(param) => (describe_point(doc, param), None),
            ParamValue::Color(param) => (describe_color(doc, param), None),
        };
        SummarySlot {
            property: property.to_string(),
            source,
            value: number.filter(|value| value.is_finite()),
        }
    }

    fn operation_summary(op: &OperationNode) -> SummaryOperation {
        SummaryOperation {
            id: op.id.to_string(),
            kind: op.kind.tag().to_string(),
            inputs: op.inputs.iter().map(|id| id.to_string()).collect(),
            enabled: op.enabled,
            name: op.name.clone(),
        }
    }

    fn constraint_summary(constraint: &Constraint) -> SummaryConstraint {
        SummaryConstraint {
            id: constraint.id.to_string(),
            kind: constraint.kind.tag().to_string(),
            targets: constraint
                .targets
                .iter()
                .map(|target| target.label())
                .collect(),
            strength: constraint.strength.tag().to_string(),
            value: constraint.value,
            enabled: constraint.enabled,
        }
    }

    fn procedural_summary(node: &ProceduralNode, ctx: &EvaluationContext<'_>) -> SummaryProcedural {
        // Only *value* ports are published in the table (`PortType::Scalar` and
        // `Color`); geometry ports compose into the scene instead, and reading
        // one here would be reading a picture, not a number.
        let mut outputs = Vec::new();
        if let Some(evaluator) = ctx.procedural {
            for port in node.kind.outputs() {
                if port.ty != PortType::Scalar {
                    continue;
                }
                let output = NodeOutputId::new(node.id, port.name.clone());
                if let Ok(value) = evaluator.evaluate_float(&output, ctx) {
                    outputs.push(format!("{} = {}", port.name, trim_number(value)));
                }
            }
        }
        SummaryProcedural {
            id: node.id.to_string(),
            name: node.name.clone(),
            kind: node.kind.tag().to_string(),
            enabled: node.enabled,
            wires: node
                .wires
                .iter()
                .map(|(port, from)| format!("{port} ← {}.{}", from.node, from.port))
                .collect(),
            outputs,
        }
    }

    /// The current value of a variable, by name.
    pub fn variable_value(&self, name: &str) -> Option<f64> {
        self.variables
            .iter()
            .find(|variable| variable.name == name)
            .map(|variable| variable.value)
    }

    /// Every node id in the summary — the grounding set an AI's commands are
    /// checked against (`vectra-ai`'s context check).
    pub fn node_ids(&self) -> BTreeSet<&str> {
        self.nodes.iter().map(|node| node.id.as_str()).collect()
    }

    /// Find a node by **exact id**, by **unique id prefix** (≥ 4 characters), or
    /// by **unique name** (case-insensitive).
    ///
    /// This is the one convenience the summary extends to a caller: an id is the
    /// only thing that is guaranteed to be unambiguous, and it is what the prompt
    /// asks for — but a name is what a human (or a heuristic parser) has in hand,
    /// and refusing to resolve it would just push the guessing back to the
    /// model. Ambiguity is reported, never resolved by picking one.
    pub fn find_node(&self, needle: &str) -> Result<&SummaryNode, NameLookup> {
        let needle = needle.trim().trim_matches('"').trim_matches('\'');
        if needle.is_empty() {
            return Err(NameLookup::Unknown);
        }
        if let Some(node) = self.nodes.iter().find(|node| node.id == needle) {
            return Ok(node);
        }
        let lower = needle.to_lowercase();
        let by_name: Vec<&SummaryNode> = self
            .nodes
            .iter()
            .filter(|node| node.name.to_lowercase() == lower)
            .collect();
        match by_name.len() {
            1 => return Ok(by_name[0]),
            0 => {}
            _ => {
                return Err(NameLookup::Ambiguous(
                    by_name.iter().map(|node| node.id.clone()).collect(),
                ))
            }
        }
        if needle.len() >= 4 {
            let by_prefix: Vec<&SummaryNode> = self
                .nodes
                .iter()
                .filter(|node| node.id.starts_with(needle))
                .collect();
            match by_prefix.len() {
                1 => return Ok(by_prefix[0]),
                0 => {}
                _ => {
                    return Err(NameLookup::Ambiguous(
                        by_prefix.iter().map(|node| node.id.clone()).collect(),
                    ))
                }
            }
        }
        Err(NameLookup::Unknown)
    }

    /// Turn a model-supplied token (an id, an id prefix, or a name) into a real
    /// id. `None` when the token names nothing in this document — which is
    /// exactly the hallucination the context check refuses.
    pub fn resolve_node_id(&self, needle: &str) -> Option<NodeId> {
        self.find_node(needle)
            .ok()
            .and_then(|node| node.id.parse::<NodeId>().ok())
    }

    /// Render the summary block for a system prompt.
    pub fn to_text(&self) -> String {
        self.to_text_within(DEFAULT_NODE_BUDGET)
    }

    /// Render, printing at most `budget` node lines.
    ///
    /// A truncated list is *labelled* ("… 12 more"), and the elision note says
    /// names are accepted, so a model working on a huge document is told what it
    /// does not know instead of being silently misled.
    pub fn to_text_within(&self, budget: usize) -> String {
        let mut out = String::new();
        let _ = writeln!(
            out,
            "DOCUMENT v{} — {} node(s), {} variable(s), {} expression(s), \
             {} operation(s), {} constraint(s), {} procedural node(s), {} track(s)",
            self.version,
            self.nodes.len(),
            self.variables.len(),
            self.expressions.len(),
            self.operations.len(),
            self.constraints.len(),
            self.procedural.len(),
            self.tracks.len(),
        );

        if !self.variables.is_empty() {
            let _ = writeln!(out, "VARIABLES");
            for variable in &self.variables {
                let _ = writeln!(
                    out,
                    "  ${} = {}",
                    variable.name,
                    trim_number(variable.value)
                );
            }
        }

        if self.nodes.is_empty() {
            let _ = writeln!(out, "NODES: none (the canvas is empty)");
        } else {
            let _ = writeln!(out, "NODES (draw order, back → front)");
            for node in self.nodes.iter().take(budget) {
                let _ = writeln!(out, "  {}  id={}", node.label, node.id);
                for slot in &node.slots {
                    match slot.value {
                        Some(value) if slot.source != trim_number(value) => {
                            let _ = writeln!(
                                out,
                                "      {} = {}  (now {})",
                                slot.property,
                                slot.source,
                                trim_number(value)
                            );
                        }
                        _ => {
                            let _ = writeln!(out, "      {} = {}", slot.property, slot.source);
                        }
                    }
                }
            }
            if self.nodes.len() > budget {
                let _ = writeln!(
                    out,
                    "  … {} more node(s) not listed; reference those by name (\"the <name>\").",
                    self.nodes.len() - budget
                );
            }
        }

        if !self.expressions.is_empty() {
            let _ = writeln!(out, "EXPRESSIONS");
            for expression in &self.expressions {
                let _ = writeln!(out, "  id={}  {}", expression.id, expression.source);
            }
        }

        if !self.operations.is_empty() {
            let _ = writeln!(out, "OPERATIONS");
            for operation in &self.operations {
                let _ = writeln!(
                    out,
                    "  id={}  {} '{}' inputs=[{}]{}",
                    operation.id,
                    operation.kind,
                    operation.name,
                    operation.inputs.join(", "),
                    if operation.enabled { "" } else { " (parked)" }
                );
            }
        }

        if !self.constraints.is_empty() {
            let _ = writeln!(out, "CONSTRAINTS");
            for constraint in &self.constraints {
                let value = constraint
                    .value
                    .map(|value| format!(" = {}", trim_number(value)))
                    .unwrap_or_default();
                let _ = writeln!(
                    out,
                    "  id={}  [{}] {} [{}]{} {}",
                    constraint.id,
                    constraint.strength,
                    constraint.kind,
                    constraint.targets.join(", "),
                    value,
                    if constraint.enabled { "" } else { "(parked)" }
                );
            }
        }

        if !self.procedural.is_empty() {
            let _ = writeln!(out, "PROCEDURAL");
            for node in &self.procedural {
                let mut line = format!(
                    "  id={}  {} '{}'{}",
                    node.id,
                    node.kind,
                    node.name,
                    if node.enabled { "" } else { " (parked)" }
                );
                if !node.wires.is_empty() {
                    let _ = write!(line, " wires[{}]", node.wires.join("; "));
                }
                if !node.outputs.is_empty() {
                    let _ = write!(line, " outputs[{}]", node.outputs.join(", "));
                }
                let _ = writeln!(out, "{line}");
            }
        }

        if !self.tracks.is_empty() {
            let _ = writeln!(out, "TRACKS");
            for track in &self.tracks {
                let _ = writeln!(
                    out,
                    "  id={}  '{}' channels[{}]",
                    track.id,
                    track.name,
                    track.channels.join(", ")
                );
            }
        }

        out
    }
}

/// How a scalar slot is driven, in Vectra's own syntax.
///
/// Expression sources are **expanded** (the record's text, not its id): a model
/// that sees `$base * 2` can preserve the arithmetic, and a model that sees an
/// opaque id can only replace it with a number — which is the thing RULE 2
/// forbids.
pub fn describe_scalar(doc: &Document, param: &Parameter<f64>) -> String {
    match param {
        Parameter::Literal(value) => trim_number(*value),
        Parameter::Variable(name) => format!("${name}"),
        Parameter::Expression(id) => match doc.expressions.get(id) {
            Some(record) => record.source.clone(),
            None => format!("expr {id} (missing)"),
        },
        Parameter::Animated(binding) => describe_binding(doc, binding),
        Parameter::Procedural(output) => {
            let owner = doc
                .procedural
                .get(output.node)
                .map(|node| node.name.clone())
                .unwrap_or_else(|| output.node.to_string());
            format!("proc {owner}.{}", output.port)
        }
        Parameter::Interaction(binding) => format!("input {binding:?}"),
    }
}

fn describe_point(doc: &Document, param: &Parameter<crate::geom::Point2>) -> String {
    match param {
        Parameter::Literal(point) => {
            format!("({}, {})", trim_number(point.x), trim_number(point.y))
        }
        Parameter::Variable(name) => format!("${name}"),
        Parameter::Expression(id) => match doc.expressions.get(id) {
            Some(record) => record.source.clone(),
            None => format!("expr {id} (missing)"),
        },
        Parameter::Animated(binding) => describe_binding(doc, binding),
        Parameter::Procedural(output) => format!("proc {}.{}", output.node, output.port),
        Parameter::Interaction(binding) => format!("input {binding:?}"),
    }
}

fn describe_color(doc: &Document, param: &Parameter<crate::geom::Color>) -> String {
    match param {
        Parameter::Literal(color) => color.to_hex(),
        Parameter::Variable(name) => format!("${name}"),
        Parameter::Expression(id) => match doc.expressions.get(id) {
            Some(record) => record.source.clone(),
            None => format!("expr {id} (missing)"),
        },
        Parameter::Animated(binding) => describe_binding(doc, binding),
        Parameter::Procedural(output) => format!("proc {}.{}", output.node, output.port),
        Parameter::Interaction(binding) => format!("input {binding:?}"),
    }
}

/// One line for a motion binding — enough that a model knows the slot is
/// *animated* and must not be overwritten with a literal to "fix" it.
fn describe_binding(doc: &Document, binding: &MotionBinding) -> String {
    match binding {
        MotionBinding::Spring {
            target,
            stiffness,
            damping,
            from,
            at,
        } => format!(
            "spring → {} (from {} @ {}s, k={}, c={})",
            describe_scalar(doc, target),
            trim_number(*from),
            trim_number(*at),
            trim_number(*stiffness),
            trim_number(*damping)
        ),
        MotionBinding::StateDriven {
            state,
            true_value,
            false_value,
        } => format!(
            "state ${state} ? {} : {}",
            describe_scalar(doc, true_value),
            describe_scalar(doc, false_value)
        ),
        MotionBinding::KeyframeTrack { track_id, property } => {
            let name = doc
                .motion
                .get(track_id)
                .map(|track| track.name.clone())
                .unwrap_or_else(|| track_id.to_string());
            format!("track '{name}'.{property}")
        }
    }
}

/// Numbers in summaries drop a trailing `.0`: `40`, not `40.0`. Ids and JSON are
/// unaffected — this is for the prose the model reads.
pub fn trim_number(value: f64) -> String {
    if !value.is_finite() {
        return value.to_string();
    }
    let rendered = format!("{value}");
    rendered
}
