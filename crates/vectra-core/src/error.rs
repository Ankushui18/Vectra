//! Unified error types for `vectra-core`.
//!
//! MES §14: commands return `Result<_, VectraError>` so the WASM boundary
//! can serialize failures back to the React remote-control UI.

use crate::ids::{ArtboardId, ConstraintId, ExpressionId, LayerId, NodeId, OperationId};
use thiserror::Error;

/// Errors raised while resolving a [`crate::param::Parameter<T>`] to a concrete value.
#[derive(Debug, Clone, PartialEq, Error)]
pub enum ResolveError {
    #[error("undefined variable '{0}'")]
    UndefinedVariable(String),

    #[error("variable '{0}' is scalar (f64) and cannot resolve to this type; bind a scalar property instead")]
    VariableTypeMismatch(String),

    #[error("expression '{0}' needs an ExpressionEvaluator, but none is wired into the EvaluationContext")]
    MissingExpressionEvaluator(ExpressionId),

    #[error("expression result is scalar (f64) and cannot resolve to this type")]
    ExpressionTypeMismatch,

    #[error(
        "motion binding needs a MotionEvaluator, but none is wired into the EvaluationContext"
    )]
    MissingMotionEvaluator,

    #[error("animated binding cannot resolve to this type in Phase 1 (motion is f64-only)")]
    UnsupportedAnimatedTarget,

    #[error("procedural output needs a ProceduralEvaluator, but none is wired into the EvaluationContext")]
    MissingProceduralEvaluator,

    /// **RULE 1**: a wire or a slot read that pairs mismatched port types.
    /// Reported before anything is stored; never a coercion.
    #[error("procedural port '{port}' carries {found}, but {expected} was required")]
    ProceduralPortType {
        port: String,
        expected: &'static str,
        found: &'static str,
    },

    /// The addressed node or port does not exist, or has not published a value
    /// (parked, failed, or never evaluated).
    #[error("procedural output {node} • {port} is not available (unknown node or port, parked, or not yet evaluated)")]
    ProceduralPortUnavailable { node: NodeId, port: String },

    #[error("interaction binding needs an InteractionProvider, but none is wired into the EvaluationContext")]
    MissingInteractionProvider,

    #[error("color parameters only support Literal and Procedural sources in Phase 1")]
    UnsupportedColorSource,

    #[error("evaluator failed: {0}")]
    Evaluator(String),
}

/// Errors raised by [`crate::command::Command`] execution and document mutation.
#[derive(Debug, Clone, PartialEq, Error)]
pub enum VectraError {
    #[error("node not found: {0}")]
    NodeNotFound(NodeId),

    #[error("node already exists: {0}")]
    NodeAlreadyExists(NodeId),

    #[error("layer not found: {0}")]
    LayerNotFound(LayerId),

    #[error("layer already exists: {0}")]
    LayerAlreadyExists(LayerId),

    #[error("node is not on any layer: {0}")]
    NodeNotInAnyLayer(NodeId),

    #[error("artboard not found: {0}")]
    ArtboardNotFound(ArtboardId),

    #[error("artboard already exists: {0}")]
    ArtboardAlreadyExists(ArtboardId),

    #[error("appearance layer {index} does not exist on node {node_id}")]
    AppearanceNotFound { node_id: NodeId, index: usize },

    /// A node cannot be a descendant of itself: the drop that would nest a group
    /// inside one of its own children is refused **before** anything moves, so a
    /// document can never contain a cycle (Task 10.4 RULE 1).
    #[error("cannot move {node_id} into {ancestor}: that would make the group its own descendant")]
    GroupCycle { node_id: NodeId, ancestor: NodeId },

    /// Only a group holds children. The refusal is typed rather than silent
    /// because "make this rectangle a parent" is a bug in the caller, and a
    /// rectangle that quietly accepted children would be a second, wrong kind of
    /// container.
    #[error("node {0} is not a group and cannot hold children")]
    NotAGroup(NodeId),

    #[error("a path segment's appearance stack must hold at least one layer")]
    EmptyAppearanceStack,

    #[error("unknown property '{property}' for node {node_id} ({node_kind})")]
    UnknownProperty {
        node_id: NodeId,
        node_kind: String,
        property: String,
    },

    #[error("type mismatch for property '{property}': expected {expected}, got {got}")]
    PropertyTypeMismatch {
        property: String,
        expected: &'static str,
        got: &'static str,
    },

    #[error("component path '{property}' requires the base parameter to be a Literal; bind the whole point instead")]
    ComponentRequiresLiteral { property: String },

    #[error("variable not found: '{0}'")]
    VariableNotFound(String),

    #[error("expression not found: {0}")]
    ExpressionNotFound(ExpressionId),

    /// A command would have introduced a cycle into the dependency graph
    /// (MES §8, Task 2.2). Detection happens *before* mutation: the document,
    /// the history stack, and the graph are all left exactly as they were, so
    /// a cycle never exists — not even for one frame.
    #[error("cyclic dependency rejected: {message}")]
    CyclicDependency { message: String },

    #[error("invalid variable name '{0}': must be non-empty and contain no whitespace")]
    InvalidVariableName(String),

    #[error("cannot {action} variable '{name}': {reason}")]
    VariableInUse {
        action: String,
        name: String,
        reason: String,
    },

    #[error("constraint not found: {0}")]
    ConstraintNotFound(ConstraintId),

    /// Two or more `Required` constraints contradict each other (MES §9, Task
    /// 3.1). A required row is never dropped, so an inconsistent hard system is
    /// a typed failure: the command is **not** applied and the document, the
    /// history stack and the solver are left exactly as they were.
    #[error("unsatisfiable constraints: {message}")]
    UnsatisfiableConstraints { message: String },

    /// A command arrived while a drag gesture was live (Task 3.2). A drag owns
    /// the tableau for its duration, so interleaving another mutation would
    /// operate on stale solver state: the whole gesture must be finished first.
    #[error("a drag of node {node_id} is in progress; finish the gesture first")]
    DragActive { node_id: NodeId },

    /// `UpdateDrag` / `EndDrag` for a node that is not being dragged — either
    /// no gesture is live or the id does not match the one that began it.
    #[error("no drag in progress for node {node_id}")]
    DragNotActive { node_id: NodeId },

    /// `BeginDrag` on a node with no canonical position slots (`Path`,
    /// `Group`): Phase-1 dragging moves `(x, y)` / `(cx, cy)` only.
    #[error("node {node_id} has no draggable position slots (path and group kinds are not draggable in Phase 1)")]
    NotDraggable { node_id: NodeId },

    /// `RemoveOperation` / `SetOperationEnabled` for an id that is not a
    /// registered operation (Task 4.0).
    #[error("operation not found: {0}")]
    OperationNotFound(OperationId),

    /// An operation was built with the wrong number of inputs for its kind
    /// (booleans take exactly two, modifiers exactly one). Rejected before
    /// anything is stored, so the registry never holds an ill-formed node.
    #[error("operation {id} takes {expected} input(s), got {got}")]
    OperationArity {
        id: OperationId,
        expected: usize,
        got: usize,
        kind: String,
    },

    #[error("nothing to undo")]
    NothingToUndo,

    #[error("nothing to redo")]
    NothingToRedo,

    #[error("command failed: {0}")]
    Command(String),

    #[error("resolve failed: {0}")]
    Resolve(#[from] ResolveError),

    #[error("serialization failed: {0}")]
    Serialization(String),
}

impl VectraError {
    pub fn command(msg: impl Into<String>) -> Self {
        Self::Command(msg.into())
    }

    /// Typed constructor for [`VectraError::CyclicDependency`].
    pub fn cyclic(message: impl Into<String>) -> Self {
        Self::CyclicDependency {
            message: message.into(),
        }
    }

    /// Typed constructor for [`VectraError::UnsatisfiableConstraints`].
    pub fn unsatisfiable(message: impl Into<String>) -> Self {
        Self::UnsatisfiableConstraints {
            message: message.into(),
        }
    }

    /// True ⟺ this error is a contradiction between required constraints.
    pub fn is_constraint_conflict(&self) -> bool {
        matches!(self, Self::UnsatisfiableConstraints { .. })
    }

    /// Typed constructor for [`VectraError::OperationArity`].
    pub fn operation_arity(
        id: OperationId,
        kind: &crate::operation::OperationKind,
        expected: usize,
        got: usize,
    ) -> Self {
        Self::OperationArity {
            id,
            expected,
            got,
            kind: kind.tag().to_string(),
        }
    }

    /// True ⟺ this error is about the operation registry (Task 4.0) rather
    /// than the document's node table.
    pub fn is_operation_error(&self) -> bool {
        matches!(
            self,
            Self::OperationNotFound(_) | Self::OperationArity { .. }
        )
    }

    /// Typed constructor for [`VectraError::DragActive`].
    pub fn drag_active(node_id: NodeId) -> Self {
        Self::DragActive { node_id }
    }

    /// Typed constructor for [`VectraError::DragNotActive`].
    pub fn drag_not_active(node_id: NodeId) -> Self {
        Self::DragNotActive { node_id }
    }

    /// Typed constructor for [`VectraError::NotDraggable`].
    pub fn not_draggable(node_id: NodeId) -> Self {
        Self::NotDraggable { node_id }
    }

    /// True ⟺ this error is about the drag protocol (Task 3.2) rather than the
    /// document. The UI uses it to clear its local gesture state.
    pub fn is_drag_error(&self) -> bool {
        matches!(
            self,
            Self::DragActive { .. } | Self::DragNotActive { .. } | Self::NotDraggable { .. }
        )
    }

    /// True ⟺ this error is a rejected cyclic dependency (UI/logging aid).
    pub fn is_cycle_rejection(&self) -> bool {
        matches!(self, Self::CyclicDependency { .. })
    }
}
