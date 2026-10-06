//! `vectra-constraints` — the linear constraint solver (MES §9, Task 3.1).
//!
//! Constraints are **hard mathematical rules**. The engine applies one by
//! solving the document's constraint system and writing the solution back into
//! the slots it moved; the dependency graph then propagates that change like
//! any other edit. Nothing here is a drawing guide, and nothing here mutates the
//! document — this crate *proposes*, the engine *commits* through the ordinary
//! inverse-returning command path.
//!
//! ```text
//! Command::AddConstraint ──▶ preflight (pure)   ── hard contradiction? → typed error, nothing applied
//!                        └─▶ apply to registry
//!                            ConstraintSolver::solve ──▶ SolveOutcome { writes, dropped, skipped }
//!                                                        │
//!                                     engine applies writes as SetParameter commands
//!                                     (undoable, graphed, incremental)
//! ```
//!
//! * [`ConstraintSolver`] — Cassowary-backed, one variable per addressed slot,
//!   edit variables for hints (what the user just moved) and stays (everything
//!   else). **No solver is written from scratch**: the tableau is the
//!   `cassowary` crate.
//! * [`plan`] — the drop pre-pass: Cassowary never reports which non-required
//!   row it failed to satisfy, so over-constraint is decided deterministically
//!   before the tableau (weakest loses, ties drop the newer row, two
//!   contradicting `Required` rows are a typed error instead).
//! * [`rows`] — the kind → linear-row table, and the slot vocabulary shared with
//!   [`vectra_core::Document`] and the dependency graph.
//! * [`VariablePool`] — the slot → `cassowary::Variable` mapping, with explicit
//!   garbage collection so the observable variable count tracks the live rules.

pub mod plan;
pub mod pool;
pub mod rows;
pub mod solver;

use thiserror::Error;

pub use plan::{plan, preflight, Conflict, Dropped, Skipped, SolvePlan};
pub use pool::VariablePool;
pub use rows::{capture_value, rows_for, validate, Row};
pub use solver::{
    cassowary_strength, current_value, position_targets, ConstraintSolver, DragSession,
    SolveOutcome, SolveWrite, SolverStats,
};

use vectra_core::{ConstraintId, ConstraintKind, VectraError};

/// Everything that can go wrong while planning or solving a constraint system.
#[derive(Debug, Error)]
pub enum ConstraintError {
    /// A document-level problem surfaced while reading or naming a slot
    /// (unknown property, type mismatch, missing node, unresolvable value).
    #[error(transparent)]
    Document(#[from] VectraError),

    /// Wrong number of targets for the kind.
    #[error("constraint {id} ({kind:?}) needs {expected} targets, got {got}")]
    Arity {
        /// The offending constraint.
        id: ConstraintId,
        /// Its kind.
        kind: ConstraintKind,
        /// Targets the kind requires.
        expected: usize,
        /// Targets supplied.
        got: usize,
    },

    /// An operand-carrying kind was registered without one and it cannot be
    /// captured from the document.
    #[error("constraint {id} ({kind:?}) requires a value")]
    MissingValue {
        /// The offending constraint.
        id: ConstraintId,
        /// Its kind.
        kind: ConstraintKind,
    },

    /// A non-finite operand or solved value.
    #[error("constraint {id} has a non-finite value ({value})")]
    NonFinite {
        /// The offending constraint.
        id: ConstraintId,
        /// The offending value.
        value: f64,
    },

    /// The constraint collapses to `0 == constant` (all coefficients cancel).
    #[error("constraint {id} ({kind:?}) is degenerate: its terms cancel out")]
    Degenerate {
        /// The offending constraint.
        id: ConstraintId,
        /// Its kind.
        kind: ConstraintKind,
    },

    /// `Required` rows contradict each other. Required rows are never dropped,
    /// so this is a typed failure with nothing applied.
    #[error("unsatisfiable constraints: {message}")]
    Unsatisfiable {
        /// Human-readable explanation (which rows, which offsets).
        message: String,
    },

    /// A second `BeginDrag` arrived while a gesture was already live.
    #[error("a drag of node {node_id} is already in progress")]
    DragBusy {
        /// The node the live gesture owns.
        node_id: vectra_core::NodeId,
    },

    /// `UpdateDrag` / `EndDrag` reached the solver with no live session.
    #[error("no drag session is open")]
    NoDragSession,

    /// `BeginDrag` on a node whose kind has no canonical position slots
    /// (`Path`, `Group`): Phase-1 dragging moves `(x, y)` / `(cx, cy)` only.
    #[error("node {node_id} has no draggable position slots")]
    NotDraggable {
        /// The node that cannot be dragged.
        node_id: vectra_core::NodeId,
    },

    /// The tableau refused a row for its own reasons.
    #[error("cassowary: {0}")]
    Solver(String),

    /// Escape hatch for unexpected conditions.
    #[error("constraint: {0}")]
    Other(String),
}

impl ConstraintError {
    /// True ⟺ this is the "required rows contradict" failure.
    pub fn is_unsatisfiable(&self) -> bool {
        matches!(self, Self::Unsatisfiable { .. })
    }
}
