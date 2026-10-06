//! Evaluation diagnostics (Task 1.3).
//!
//! Scene evaluation is total: it never panics and never halts on a bad node.
//! Every fallback, clamp, and skip is reported here so the inspector can show
//! *why* a node looks wrong — or is missing — instead of failing silently.
//!
//! Severity policy:
//! * [`Severity::Error`] — the node was **skipped** (no meaningful geometry).
//! * [`Severity::Warning`] — the node was evaluated with a **fallback or clamp**.
//! * [`Severity::Info`] — structural notes (reserved; groups flatten silently).

use vectra_core::NodeId;

/// Severity of a [`Diagnostic`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Severity {
    Info,
    Warning,
    Error,
}

impl Severity {
    /// Stable wire tag (`"info"` / `"warning"` / `"error"`).
    pub fn tag(&self) -> &'static str {
        match self {
            Self::Info => "info",
            Self::Warning => "warning",
            Self::Error => "error",
        }
    }
}

/// Machine-readable cause of a [`Diagnostic`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DiagnosticCode {
    /// A parameter could not be resolved (missing variable, missing evaluator…).
    UnresolvableParameter,
    /// A resolved value was NaN, infinite, or outside the GPU-representable range.
    NonFiniteValue,
    /// A value was clamped into its valid domain (negative size, opacity…).
    ClampedValue,
    /// A style channel fell back to its default; geometry is intact.
    StyleFallback,
    /// Two constraints disagreed and the weaker one was dropped (MES §9).
    ConstraintDropped,
    /// The solver had to write a slot that was driven by a variable or an
    /// expression, so the parametric link was broken and the slot pinned to the
    /// solved value (Task 3.1 doctrine: the solver never writes global
    /// variables).
    ParametricLinkBroken,
    /// A constraint could not be enforced this pass (its slot no longer
    /// resolves); it stays registered and is re-tried on the next solve.
    ConstraintSkipped,
    /// A non-destructive operation could not be computed (Task 4.0): a
    /// non-finite operand, an unresolvable parameter, or mismatched arity. The
    /// operation keeps its registry entry and is retried on the next pass.
    OperationFailed,
    /// An operation's source evaluates to something enclosing no area — there is
    /// no region to union, subtract, offset or round (Task 4.0).
    OperationEmptyInput,
    /// A procedural node could not be computed (Task 7.0): a degenerate grid, a
    /// repeated region with nothing to repeat, an unresolvable operand. The node
    /// keeps its registry entry, contributes no geometry, and is retried on the
    /// next pass.
    ProceduralFailed,
    /// A procedural node is missing a required wired input — the normal state
    /// while a graph is being wired up, and a contained failure, never an error
    /// that takes the scene down (Task 7.0).
    ProceduralMissingInput,
    /// A procedural node's *result* encloses no area: the region it builds is
    /// degenerate, so there is nothing to draw or to feed downstream.
    ProceduralEmptyResult,
}

impl DiagnosticCode {
    pub fn tag(&self) -> &'static str {
        match self {
            Self::UnresolvableParameter => "unresolvable-parameter",
            Self::NonFiniteValue => "non-finite-value",
            Self::ClampedValue => "clamped-value",
            Self::StyleFallback => "style-fallback",
            Self::ConstraintDropped => "constraint-dropped",
            Self::ParametricLinkBroken => "parametric-link-broken",
            Self::ConstraintSkipped => "constraint-skipped",
            Self::OperationFailed => "operation-failed",
            Self::OperationEmptyInput => "operation-empty-input",
            Self::ProceduralFailed => "procedural-failed",
            Self::ProceduralMissingInput => "procedural-missing-input",
            Self::ProceduralEmptyResult => "procedural-empty-result",
        }
    }
}

/// One evaluation note: node + property + cause + human message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub node_id: Option<NodeId>,
    pub property: Option<String>,
    pub severity: Severity,
    pub code: DiagnosticCode,
    pub message: String,
}

impl Diagnostic {
    pub fn error(
        node_id: NodeId,
        property: impl Into<String>,
        code: DiagnosticCode,
        message: impl Into<String>,
    ) -> Self {
        Self {
            node_id: Some(node_id),
            property: Some(property.into()),
            severity: Severity::Error,
            code,
            message: message.into(),
        }
    }

    pub fn warning(
        node_id: NodeId,
        property: impl Into<String>,
        code: DiagnosticCode,
        message: impl Into<String>,
    ) -> Self {
        Self {
            node_id: Some(node_id),
            property: Some(property.into()),
            severity: Severity::Warning,
            code,
            message: message.into(),
        }
    }

    /// A dropped constraint: the geometry is still evaluated, so this is a
    /// warning that names the loser and the winner.
    pub fn constraint_dropped(
        node_id: NodeId,
        property: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self::warning(
            node_id,
            property,
            DiagnosticCode::ConstraintDropped,
            message,
        )
    }

    /// A slot that had to be unlinked from its variable/expression source.
    pub fn parametric_link_broken(
        node_id: NodeId,
        property: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self::warning(
            node_id,
            property,
            DiagnosticCode::ParametricLinkBroken,
            message,
        )
    }

    /// A constraint that could not be enforced this pass.
    pub fn constraint_skipped(
        node_id: NodeId,
        property: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self::warning(
            node_id,
            property,
            DiagnosticCode::ConstraintSkipped,
            message,
        )
    }

    pub fn is_error(&self) -> bool {
        self.severity == Severity::Error
    }
}
