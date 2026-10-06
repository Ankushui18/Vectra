//! Non-destructive operations: virtual geometry composed from source nodes
//! (MES §10, Task 4.0).
//!
//! An [`OperationNode`] is **not** a destructive edit. It is a record that
//! says "the union of these two nodes", "this node offset outward by `d`", and
//! so on. The sources stay in the document — editable, draggable, constrained —
//! and the result is recomputed whenever one of them changes (RULE 1).
//!
//! # Where an operation node lives
//!
//! Operations are a *registry of their own* ([`OperationRegistry`]), not
//! entries in [`crate::document::Document::nodes`]: the document's node table
//! holds authored primitives with `f64` slots, and bolting a synthetic kind
//! into [`crate::document::NodeKind`] would ripple through every exhaustive
//! match in the workspace. The two registries share the **same id space** —
//! [`OperationId`] *is* [`NodeId`] — so an operation's result can be keyed,
//! ordered and drawn exactly like a primitive (RULE 3), and
//! [`crate::document::Document::geometry_ids`] enumerates both.
//!
//! # Arity
//!
//! [`OperationKind::arity`] is the contract the commands validate:
//! `Boolean` takes exactly two inputs (the subject and the clip); the
//! modifiers take exactly one.

use crate::error::VectraError;
use crate::ids::{NodeId, OperationId};
use crate::param::Parameter;
use serde::{Deserialize, Serialize};

/// The four boolean set operations (`geo::OpType` on the wire).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BooleanOp {
    /// `A ∪ B` — everything either shape covers.
    Union,
    /// `A \ B` — the subject with the clip removed.
    Subtract,
    /// `A ∩ B` — only the overlap.
    Intersect,
    /// `A ⊕ B` — the symmetric difference (everything covered exactly once).
    Exclude,
}

impl BooleanOp {
    pub fn tag(self) -> &'static str {
        match self {
            Self::Union => "union",
            Self::Subtract => "subtract",
            Self::Intersect => "intersect",
            Self::Exclude => "exclude",
        }
    }

    /// The operator glyph the UI shows in the layers panel.
    pub fn glyph(self) -> &'static str {
        match self {
            Self::Union => "∪",
            Self::Subtract => "−",
            Self::Intersect => "∩",
            Self::Exclude => "⊕",
        }
    }
}

/// The axis a [`OperationKind::Mirror`] reflects across.
///
/// Both are *lines*, not directions: `Vertical { at: c }` reflects across
/// `x = c`, `Horizontal { at: c }` across `y = c`. `at` is a
/// [`Parameter<f64>`], so the mirror plane is animatable and steerable like
/// every other number in the document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "axis", rename_all = "lowercase")]
pub enum MirrorAxis {
    /// Reflect across the vertical line `x = at`.
    Vertical { at: Parameter<f64> },
    /// Reflect across the horizontal line `y = at`.
    Horizontal { at: Parameter<f64> },
}

impl MirrorAxis {
    /// Visit the axis's parameters — the mirror plane is the one place an
    /// operation reads a plain number, so it is the one place a cycle check has
    /// to look.
    pub fn for_each_float_param<'a>(&'a self, mut visit: impl FnMut(&'a Parameter<f64>)) {
        match self {
            Self::Vertical { at } | Self::Horizontal { at } => visit(at),
        }
    }

    pub fn tag(&self) -> &'static str {
        match self {
            Self::Vertical { .. } => "vertical",
            Self::Horizontal { .. } => "horizontal",
        }
    }
}

/// What an operation node computes (MES §10, extended for Task 4.0).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum OperationKind {
    /// A boolean set operation over exactly two inputs.
    Boolean { op: BooleanOp },
    /// Offset the input's region by `distance` (negative insets).
    Offset { distance: Parameter<f64> },
    /// Round the input's convex corners to `radius`.
    Fillet { radius: Parameter<f64> },
    /// Reflect the input across an axis.
    Mirror { axis: MirrorAxis },
}

impl OperationKind {
    /// Number of inputs this kind consumes.
    pub fn arity(&self) -> usize {
        match self {
            Self::Boolean { .. } => 2,
            Self::Offset { .. } | Self::Fillet { .. } | Self::Mirror { .. } => 1,
        }
    }

    /// Visit every float parameter this kind reads (none for `Boolean`).
    ///
    /// Used by the boundary-time cycle gate: a `Parameter<f64>` may read a
    /// procedural port, and a reference that closes a loop through geometry has
    /// to be refused before it is stored (RULE 3).
    pub fn for_each_float_param<'a>(&'a self, mut visit: impl FnMut(&'a Parameter<f64>)) {
        match self {
            Self::Boolean { .. } => {}
            Self::Offset { distance } => visit(distance),
            Self::Fillet { radius } => visit(radius),
            Self::Mirror { axis } => axis.for_each_float_param(visit),
        }
    }

    pub fn tag(&self) -> &'static str {
        match self {
            Self::Boolean { .. } => "boolean",
            Self::Offset { .. } => "offset",
            Self::Fillet { .. } => "fillet",
            Self::Mirror { .. } => "mirror",
        }
    }

    /// One-line description for the inspector (engine-rendered, like
    /// constraints: the UI never formats numbers of its own).
    pub fn describe(&self) -> String {
        match self {
            Self::Boolean { op } => op.tag().to_string(),
            Self::Offset { distance } => match distance {
                Parameter::Literal(d) => format!("offset {d}"),
                other => format!("offset ({})", other.source_tag()),
            },
            Self::Fillet { radius } => match radius {
                Parameter::Literal(r) => format!("fillet r={r}"),
                other => format!("fillet ({})", other.source_tag()),
            },
            Self::Mirror { axis } => match axis {
                MirrorAxis::Vertical { at } => match at {
                    Parameter::Literal(v) => format!("mirror across x={v}"),
                    other => format!("mirror across x ({})", other.source_tag()),
                },
                MirrorAxis::Horizontal { at } => match at {
                    Parameter::Literal(v) => format!("mirror across y={v}"),
                    other => format!("mirror across y ({})", other.source_tag()),
                },
            },
        }
    }
}

/// A virtual shape: sources in, one evaluated path out (MES §10).
///
/// `id` is a [`NodeId`] in the same id space as primitives, which is what lets
/// the result be styled, ordered, keyed in the scene and (Phase 2) constrained
/// like anything else. `enabled: false` keeps the record and its inputs but
/// drops the result from the scene — the operations analogue of parking a rule.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OperationNode {
    pub id: OperationId,
    pub kind: OperationKind,
    /// The source shapes this operation *reads*. Never mutated by the
    /// operation (RULE 1).
    pub inputs: Vec<NodeId>,
    pub enabled: bool,
    /// Display name; defaults to the kind's tag.
    pub name: String,
    /// Fill/stroke applied to the evaluated result path.
    pub style: crate::document::StyleProperties,
}

impl OperationNode {
    pub fn new(id: OperationId, kind: OperationKind, inputs: Vec<NodeId>) -> Self {
        Self {
            id,
            name: kind.describe(),
            kind,
            inputs,
            enabled: true,
            style: Self::default_style(),
        }
    }

    /// The virtual shape's default look: a warm fill, so a boolean result reads
    /// as *derived* geometry rather than as one of its sources.
    pub fn default_style() -> crate::document::StyleProperties {
        crate::document::StyleProperties {
            fill: Parameter::Literal(crate::geom::Color::rgb(0xf2, 0x8c, 0x28)),
            ..crate::document::StyleProperties::default()
        }
    }

    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }

    pub fn with_enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Validate `inputs` against [`OperationKind::arity`] and the document's
    /// known ids. Called by the command before anything is stored.
    pub fn validate(
        id: OperationId,
        kind: &OperationKind,
        inputs: &[NodeId],
    ) -> Result<(), VectraError> {
        let arity = kind.arity();
        if inputs.len() != arity {
            return Err(VectraError::operation_arity(id, kind, arity, inputs.len()));
        }
        for (index, input) in inputs.iter().enumerate() {
            if inputs[..index].contains(input) {
                return Err(VectraError::command(format!(
                    "operation {id} lists input {input} twice: an operand cannot be its own clip"
                )));
            }
        }
        Ok(())
    }
}

/// Insertion-ordered registry of operation nodes.
///
/// `BTreeMap` for deterministic iteration (serialization, snapshot, export),
/// with `order` mirroring [`crate::document::Document::order`]: the draw order
/// of operation results, back → front.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct OperationRegistry {
    pub nodes: std::collections::BTreeMap<OperationId, OperationNode>,
    pub order: Vec<OperationId>,
}

impl OperationRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    pub fn get(&self, id: OperationId) -> Option<&OperationNode> {
        self.nodes.get(&id)
    }

    pub fn contains(&self, id: OperationId) -> bool {
        self.nodes.contains_key(&id)
    }

    /// Register (or replace) an operation, appending to draw order.
    pub fn insert(&mut self, node: OperationNode) -> Option<OperationNode> {
        let id = node.id;
        if !self.order.contains(&id) {
            self.order.push(id);
        }
        self.nodes.insert(id, node)
    }

    /// Withdraw an operation, returning the record so `RemoveOperation` can
    /// build its exact inverse.
    pub fn remove(&mut self, id: OperationId) -> Option<OperationNode> {
        let removed = self.nodes.remove(&id);
        if removed.is_some() {
            self.order.retain(|existing| *existing != id);
        }
        removed
    }

    /// Every operation that reads at least one id in `ids` — the propagation
    /// step that makes a source move re-run the booleans that depend on it.
    ///
    /// This is a one-level scan of the registry (operations do not feed other
    /// operations in Phase 1), so it is O(operations × inputs) with no graph
    /// traversal and no allocation when nothing matches.
    pub fn affected_by<'a>(&'a self, ids: &'a [NodeId]) -> Vec<OperationId> {
        if ids.is_empty() {
            return Vec::new();
        }
        self.order
            .iter()
            .copied()
            .filter(|op_id| {
                self.nodes
                    .get(op_id)
                    .is_some_and(|op| op.inputs.iter().any(|input| ids.contains(input)))
            })
            .collect()
    }

    /// Iterate operations in draw order.
    pub fn in_order(&self) -> impl Iterator<Item = &OperationNode> {
        self.order.iter().filter_map(|id| self.nodes.get(id))
    }
}

/// Compatibility alias: the MES §10 sketch names the record `OperationNode`
/// and its id type `OperationId`; Task 4.0 keeps both names.
pub type Operation = OperationNode;
