//! `vectra-core` — IDs, [`Parameter<T>`], document model, commands (MES §3–§4, §14).
//!
//! This crate is the **dependency root** of the workspace: every other crate
//! depends on it, and it depends on nothing internal. It owns:
//!
//! * [`ids`] — [`NodeId`], [`VariableId`], [`ExpressionId`]
//! * [`param`] — [`Parameter<T>`], [`MotionBinding`], [`ParamValue`]
//! * [`eval`] — [`EvaluationContext`] + evaluator traits + [`Resolvable`] + [`EvalMode`]
//! * [`geom`] — [`Point2`], [`Color`] value types
//! * [`document`] — [`Document`], [`Node`], [`NodeKind`]
//! * [`command`] — [`Command`], [`CommandStack`], [`EngineEvent`]
//! * [`engine`] — headless [`Engine`] (wrapped by `vectra-wasm`)
//! * [`error`] — [`VectraError`], [`ResolveError`]
//!
//! # The core law
//! ```text
//! The engine only ever asks Resolvable::resolve(ctx, time) -> T.
//! It never cares whether T came from a literal, a variable, an expression,
//! a motion binding, a procedural port, or a live input.
//! ```
//! Geometry, logic, and motion are the same editable system.

pub mod command;
pub mod component;
pub mod constraint;
pub mod document;
pub mod engine;
pub mod error;
pub mod eval;
pub mod geom;
pub mod ids;
pub mod layers;
pub mod operation;
pub mod param;
pub mod procedural;
pub mod style;
pub mod summary;

pub use command::{Command, CommandStack, EngineEvent, HistoryEntry, OutlinePath};
pub use component::{
    bind_plan, design_size, icon_set_plan, infer_props, inspect, is_instance, is_master, master_of,
    members_of, owner_of, spec_of, BindPlan, ComponentProp, ComponentSpec, ComponentView, PropLaw,
    PropTarget, PropType, PropView, ScaledTarget, SlotWrite, DEFAULT_ICON_SIZES, ICON_GUTTER,
};
pub use constraint::{Constraint, ConstraintKind, ConstraintRegistry, ConstraintTarget, Strength};
pub use document::{
    Document, ExpressionRecord, Keyframe, MotionTrack, MotionTrackRegistry, Node, NodeKind,
    OperationRecord, PathSegment, StyleProperties, TextAlign, TextPathBinding, DEFAULT_FONT_FAMILY,
    DEFAULT_LINE_HEIGHT, DOCUMENT_VERSION,
};
pub use engine::{DispatchResult, Engine};
pub use error::{ResolveError, VectraError};
pub use eval::{
    EvalMode, EvaluationContext, ExpressionEvaluator, FontProvider, InteractionProvider,
    MotionEvaluator, ProceduralEvaluator, Resolvable,
};
pub use geom::{Color, Point2};
pub use ids::{
    new_artboard_id, new_constraint_id, new_expression_id, new_layer_id, new_node_id,
    new_operation_id, parse_node_id, ArtboardId, ConstraintId, ExpressionId, LayerId, NodeId,
    OperationId, PortId, TrackId, VariableId,
};
pub use layers::{ArtboardRecord, ArtboardRegistry, LayerRecord, LayerRegistry};
pub use operation::{
    BooleanOp, MirrorAxis, Operation, OperationKind, OperationNode, OperationRegistry,
};
pub use param::{InputBinding, MotionBinding, NodeOutputId, ParamValue, Parameter};
pub use procedural::{
    GeometryData, IncomingWire, Port, PortType, ProceduralKind, ProceduralNode, ProceduralRegistry,
    RemovedProceduralNode,
};
pub use style::{AppearanceKind, AppearanceLayer, BlendMode, GradientStop, Paint};
pub use summary::{
    describe_scalar, trim_number, DocumentSummary, SummaryConstraint, SummaryExpression,
    SummaryNode, SummaryOperation, SummaryProcedural, SummarySlot, SummaryTrack, SummaryVariable,
    STYLE_SLOTS,
};
