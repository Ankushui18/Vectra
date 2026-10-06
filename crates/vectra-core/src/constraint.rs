//! Constraint model (MES §9) — the *declarative* half of the constraint solver.
//!
//! A constraint is a hard mathematical rule the engine enforces on property
//! slots: "these two nodes share an x", "this distance is 100", "these edges
//! are perpendicular". It is **not** a UI guideline — the solver rewrites the
//! document's geometry so the rule holds *exactly*, and the dependency graph
//! re-derives over the result like any other mutation.
//!
//! The types here are pure data (serde-friendly, no solver dependency) so the
//! document, the command layer, the WASM boundary and the TypeScript mirror all
//! speak one vocabulary. Translation into linear rows is `vectra-constraints`'
//! job; see [`crate::command::Command::AddConstraint`] for the entry point.
//!
//! # Vocabulary
//!
//! A [`ConstraintTarget`] is a `(node, property)` pair naming one *float slot*
//! in the document — `(node_a, "x")`, `(node_a, "width")`, `(node_b, "cx")`.
//! Property names are exactly the ones [`Node::get_param`]/[`Node::set_param`]
//! and the dependency graph already use, so a constraint and a dependency edge
//! address the same slot.
//!
//! [`Node::get_param`]: crate::document::Node::get_param
//! [`Node::set_param`]: crate::document::Node::set_param

use serde::{Deserialize, Serialize};

use crate::ids::{new_constraint_id, ConstraintId, NodeId};

/// Strength of a constraint, in Cassowary terms (MES §9).
///
/// The solver minimizes weighted error; when two constraints disagree, the
/// weakest one loses and the engine emits a
/// `Diagnostic::ConstraintDropped`. `Required` is *not* droppable — two
/// `Required` rows that contradict each other are a hard error.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Strength {
    /// Must hold exactly; never dropped.
    Required,
    /// Dropped only when nothing weaker can give.
    Strong,
    /// Default for user-authored constraints.
    Medium,
    /// First to go when the system is over-constrained.
    Weak,
}

impl Strength {
    /// Stable wire tag (`"required"` / `"strong"` / `"medium"` / `"weak"`).
    pub fn tag(&self) -> &'static str {
        match self {
            Self::Required => "required",
            Self::Strong => "strong",
            Self::Medium => "medium",
            Self::Weak => "weak",
        }
    }

    /// True for [`Strength::Required`].
    pub fn is_required(&self) -> bool {
        matches!(self, Self::Required)
    }
}

/// The kinds of constraint Phase 1 understands (MES §9).
///
/// Every kind is expressed on float slots only, which is what keeps the system
/// linear: rotation-relative rules (a true "parallel to that edge") arrive with
/// the non-linear pass (MES §9 `solve_nonlinear`, Phase 2+).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConstraintKind {
    /// `a == b`, component-wise, for each target pair.
    Coincident,
    /// `a.y == b.y`: the two targets share a row (or a constant row).
    Horizontal,
    /// `a.x == b.x`: the two targets share a column.
    Vertical,
    /// Axis-aligned edges keep a constant separation: `a.y - b.y == value`.
    Parallel,
    /// Axis-aligned edges stay orthogonal: `a.x - a.y == b.x - b.y`.
    Perpendicular,
    /// `|a| == |b|` on a single magnitude slot (width/height/radius).
    EqualLength,
    /// Signed separation of two slots: `a - b == value`.
    Distance,
    /// A single slot pinned to `value` (degrees or radians, caller's choice).
    Angle,
}

impl ConstraintKind {
    /// Stable wire tag (`snake_case`, matches serde).
    pub fn tag(&self) -> &'static str {
        match self {
            Self::Coincident => "coincident",
            Self::Horizontal => "horizontal",
            Self::Vertical => "vertical",
            Self::Parallel => "parallel",
            Self::Perpendicular => "perpendicular",
            Self::EqualLength => "equal_length",
            Self::Distance => "distance",
            Self::Angle => "angle",
        }
    }

    /// Number of targets this kind requires.
    ///
    /// Two for the pairwise rules, four for [`ConstraintKind::Perpendicular`]
    /// (both endpoints of both axis-aligned edges), one for
    /// [`ConstraintKind::Angle`].
    pub fn arity(&self) -> usize {
        match self {
            Self::Coincident
            | Self::Horizontal
            | Self::Vertical
            | Self::Parallel
            | Self::EqualLength
            | Self::Distance => 2,
            Self::Perpendicular => 4,
            Self::Angle => 1,
        }
    }

    /// Whether this kind reads a numeric `value` (see [`Constraint::value`]).
    pub fn takes_value(&self) -> bool {
        matches!(self, Self::Distance | Self::Angle | Self::Parallel)
    }
}

/// One addressable float slot: a node plus a property path from
/// [`Node::get_param`](crate::document::Node::get_param).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ConstraintTarget {
    /// The node owning the slot.
    pub node_id: NodeId,
    /// Property path, e.g. `"x"`, `"cx"`, `"width"`, `"radius"`.
    pub property: String,
}

impl ConstraintTarget {
    /// Build a target from a node and a property path.
    pub fn new(node_id: NodeId, property: impl Into<String>) -> Self {
        Self {
            node_id,
            property: property.into(),
        }
    }

    /// `"<node uuid>.<property>"` — the human/JSON name of this slot, and the
    /// string the solver uses to key its variable map.
    pub fn label(&self) -> String {
        format!("{}.{}", self.node_id, self.property)
    }
}

impl std::fmt::Display for ConstraintTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.label())
    }
}

/// A constraint: data only, interpreted by `vectra-constraints`.
///
/// Inactive constraints (`enabled == false`) stay in the registry and in the
/// undo stack but are not loaded into the solver, so toggling is free.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Constraint {
    /// Stable identity; survives undo/redo and is what `RemoveConstraint`
    /// takes. Never reused.
    pub id: ConstraintId,
    /// The rule.
    pub kind: ConstraintKind,
    /// The slots the rule applies to, in the order documented on
    /// [`ConstraintKind`]: `[a, b]`, `[a₁, a₂, b₁, b₂]` for
    /// [`ConstraintKind::Perpendicular`], `[a]` for [`ConstraintKind::Angle`].
    pub targets: Vec<ConstraintTarget>,
    /// Droppability (Cassowary strength). Defaults to
    /// [`Strength::Medium`] on the wire.
    #[serde(default = "default_strength")]
    pub strength: Strength,
    /// Numeric operand for [`ConstraintKind::Distance`] /
    /// [`ConstraintKind::Angle`] / [`ConstraintKind::Parallel`]; `None` for the
    /// purely relational kinds. When omitted for a kind that can capture one,
    /// the engine resolves the value that holds *as the document currently is*.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<f64>,
    /// `false` parks the constraint without deleting it. Defaults to `true`.
    #[serde(default = "default_enabled")]
    pub enabled: bool,
}

impl Constraint {
    /// Build an enabled, value-less constraint of `kind` over `targets`.
    pub fn new(id: ConstraintId, kind: ConstraintKind, targets: Vec<ConstraintTarget>) -> Self {
        Self {
            id,
            kind,
            targets,
            strength: Strength::Medium,
            value: None,
            enabled: true,
        }
    }

    /// Builder: set the strength.
    pub fn with_strength(mut self, strength: Strength) -> Self {
        self.strength = strength;
        self
    }

    /// Builder: set the numeric operand.
    pub fn with_value(mut self, value: f64) -> Self {
        self.value = Some(value);
        self
    }

    /// Builder: set the enabled flag.
    pub fn with_enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// A one-line human description, e.g. `"vertical (node).x ↔ (node).x"`.
    /// Used by the UI inspector and the event log — never by the solver.
    pub fn description(&self) -> String {
        let targets = self
            .targets
            .iter()
            .map(|t| t.label())
            .collect::<Vec<_>>()
            .join(" ↔ ");
        match self.value {
            Some(v) => format!("{} {targets} = {v}", self.kind.tag()),
            None => format!("{} {targets}", self.kind.tag()),
        }
    }

    /// The slots the rule reads, for reverse-indexing a document change back to
    /// the constraints it invalidates.
    pub fn targets_slice(&self) -> &[ConstraintTarget] {
        &self.targets
    }
}

fn default_strength() -> Strength {
    Strength::Medium
}

fn default_enabled() -> bool {
    true
}

/// The document's constraint registry: an ordered, id-keyed set (MES §4/§9).
///
/// Order is insertion order, which makes iteration, JSON and the inspector
/// deterministic. Lookups are by id; the map is small (tens) so a `Vec` scan is
/// the honest data structure.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ConstraintRegistry {
    /// Constraints in insertion order.
    pub constraints: Vec<Constraint>,
}

impl ConstraintRegistry {
    /// An empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of constraints, enabled or not.
    pub fn len(&self) -> usize {
        self.constraints.len()
    }

    /// True when no constraint is registered.
    pub fn is_empty(&self) -> bool {
        self.constraints.is_empty()
    }

    /// All constraints, in insertion order.
    pub fn iter(&self) -> impl Iterator<Item = &Constraint> {
        self.constraints.iter()
    }

    /// The constraint with `id`, if present.
    pub fn get(&self, id: ConstraintId) -> Option<&Constraint> {
        self.constraints.iter().find(|c| c.id == id)
    }

    /// Mutable access for enable/disable (undoable via `SetConstraintEnabled`).
    pub fn get_mut(&mut self, id: ConstraintId) -> Option<&mut Constraint> {
        self.constraints.iter_mut().find(|c| c.id == id)
    }

    /// Whether `id` is registered.
    pub fn contains(&self, id: ConstraintId) -> bool {
        self.get(id).is_some()
    }

    /// Enabled constraints only — the set the solver loads.
    pub fn active(&self) -> impl Iterator<Item = &Constraint> {
        self.constraints.iter().filter(|c| c.enabled)
    }

    /// Add a constraint. Ids are unique; re-inserting a live id is a
    /// [`VectraError::Command`](crate::error::VectraError::Command) because it
    /// would make undo non-invertible.
    pub fn insert(&mut self, constraint: Constraint) -> Result<(), crate::error::VectraError> {
        if self.contains(constraint.id) {
            return Err(crate::error::VectraError::command(format!(
                "constraint {} is already registered",
                constraint.id
            )));
        }
        self.constraints.push(constraint);
        Ok(())
    }

    /// Remove and return the constraint with `id`.
    pub fn remove(&mut self, id: ConstraintId) -> Option<Constraint> {
        let index = self.constraints.iter().position(|c| c.id == id)?;
        Some(self.constraints.remove(index))
    }

    /// Drop every constraint that targets `node_id` — used by `DeleteNode` so a
    /// deleted node cannot leave a dangling rule behind.
    pub fn remove_targeting(&mut self, node_id: NodeId) -> Vec<Constraint> {
        let mut removed = Vec::new();
        self.constraints.retain(|c| {
            let hit = c.targets.iter().any(|t| t.node_id == node_id);
            if hit {
                removed.push(c.clone());
            }
            !hit
        });
        removed
    }
}

impl<'a> IntoIterator for &'a ConstraintRegistry {
    type Item = &'a Constraint;
    type IntoIter = std::slice::Iter<'a, Constraint>;

    fn into_iter(self) -> Self::IntoIter {
        self.constraints.iter()
    }
}

/// Convenience: a fresh id for a user-authored constraint.
pub fn new_user_constraint_id() -> ConstraintId {
    new_constraint_id()
}
