//! Stable identity types (MES §3).
//!
//! Every dynamic value in VECTRA flows through [`crate::param::Parameter<T>`]
//! and is addressed by one of these IDs. They are the join keys of the
//! dependency graph (MES §8).

/// Unique identifier for a scene node ([`crate::document::Node`]).
pub type NodeId = uuid::Uuid;

/// Name of a global scalar variable (`Document.variables`).
///
/// Phase 1: variables are `f64`-only. Vector/composite variables arrive with
/// the typed procedural graph (MES §11).
pub type VariableId = String;

/// Unique identifier for a compiled expression (MES §7).
///
/// The compiled artifact lives in `vectra-expression`; `vectra-core` only
/// passes the ID through [`crate::param::Parameter::Expression`] so the
/// dependency direction stays acyclic (`expression → core`, never `core → expression`).
pub type ExpressionId = uuid::Uuid;

/// Unique identifier for a constraint (MES §9).
///
/// A UUID rather than an index so undo/redo and `RemoveConstraint` address the
/// same rule across a session; ids are never reused.
pub type ConstraintId = uuid::Uuid;

/// Identifier for a non-destructive operation node (MES §10, Task 4.0).
///
/// Deliberately the *same* id space as [`NodeId`]: an operation is a virtual
/// shape, and a virtual shape has to be keyable, orderable and drawable in the
/// scene exactly like a primitive (RULE 3). Distinct names, one namespace.
pub type OperationId = NodeId;

/// Identifier for a motion track (keyframe timeline, MES §12).
pub type TrackId = String;

/// Identifier for a **layer** (Task 10.2 RULE 1).
///
/// Its own id space, not a [`NodeId`]: a layer is a record in
/// [`crate::layers::LayerRegistry`], not a node in the dependency graph, so it
/// must be impossible to pass one where a node is expected.
pub type LayerId = uuid::Uuid;

/// Identifier for an **artboard** (Task 10.2 RULE 2). Same reasoning: an
/// artboard is a frame, not geometry, and never enters the graph.
pub type ArtboardId = uuid::Uuid;

/// Identifier for a procedural-graph port (MES §11).
pub type PortId = String;

/// Allocate a fresh [`NodeId`].
pub fn new_node_id() -> NodeId {
    uuid::Uuid::new_v4()
}

/// Allocate a fresh [`ExpressionId`].
pub fn new_expression_id() -> ExpressionId {
    uuid::Uuid::new_v4()
}

/// Allocate a fresh [`OperationId`].
pub fn new_operation_id() -> OperationId {
    uuid::Uuid::new_v4()
}

/// Allocate a fresh [`ConstraintId`].
pub fn new_constraint_id() -> ConstraintId {
    uuid::Uuid::new_v4()
}

/// Allocate a fresh [`LayerId`].
pub fn new_layer_id() -> LayerId {
    uuid::Uuid::new_v4()
}

/// Allocate a fresh [`ArtboardId`].
pub fn new_artboard_id() -> ArtboardId {
    uuid::Uuid::new_v4()
}

/// The artboard a brand-new document opens on (Task 10.2 RULE 2).
///
/// **Fixed, not allocated.** A fresh engine is a *template* — one board, one
/// layer, the same names and the same ids every time — so two engines fed the
/// same command stream stay byte-identical (the determinism laws in
/// `vectra-wasm/tests` compare snapshots across engines) and a diff of two new
/// documents shows the designer's work and nothing else. Ids are per-document
/// keys, and Vectra never merges two documents, so a constant is safe as well
/// as useful.
pub const DEFAULT_ARTBOARD: ArtboardId =
    uuid::Uuid::from_u128(0x1ec7_a000_0000_4000_8000_0000_0000_0001);

/// The layer a brand-new document opens with (Task 10.2 RULE 1). See
/// [`DEFAULT_ARTBOARD`] for why the id is a constant.
pub const DEFAULT_LAYER: LayerId = uuid::Uuid::from_u128(0x1ec7_b000_0000_4000_8000_0000_0000_0001);

/// Parse a [`NodeId`] from the wire (the wasm shell's `&str` boundary).
///
/// Lives here, next to the allocators, so the shell does not have to depend on
/// `uuid` just to read an id back: the type alias stays a private implementation
/// detail of this module.
pub fn parse_node_id(text: &str) -> Option<NodeId> {
    uuid::Uuid::parse_str(text).ok()
}
