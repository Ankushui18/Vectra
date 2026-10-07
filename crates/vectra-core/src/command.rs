//! Event-sourced commands + undo/redo (MES §14).
//!
//! No full-document cloning: every [`Command::apply`] returns its exact
//! inverse, and [`CommandStack`] stores `(forward, backward)` pairs.
//! The WASM boundary (MES §15) ships these commands as JSON from the React
//! remote control; the engine owns all mutation.

use crate::constraint::Constraint;
use crate::document::StyleProperties;
use crate::document::{Document, MotionTrack, Node, NodeKind, PathSegment, TextAlign};
use crate::error::VectraError;
use crate::geom::{Color, Point2};
use crate::ids::{
    ArtboardId, ConstraintId, ExpressionId, LayerId, NodeId, OperationId, TrackId, VariableId,
};
use crate::layers::ArtboardRecord;
use crate::layers::LayerRecord;
use crate::operation::{OperationKind, OperationNode};
use crate::param::{MotionBinding, NodeOutputId, ParamValue, Parameter};
use crate::procedural::ProceduralNode;
use crate::style::AppearanceLayer;
use serde::{Deserialize, Serialize};

/// All mutations to a [`Document`]. Serialized over the WASM bridge as
/// externally-tagged JSON, e.g.
/// `{"type":"SetParameter","node_id":"…","property":"width","value":{"Float":{"Variable":"base"}}}`.
///
/// Field names use `snake_case` on the wire to match the JS convention.
///
/// `CreateNode` owns a full `NodeKind` by value: commands are user-frequency
/// (never per-frame), undo entries must own their redo payload, and ~320 bytes
/// is irrelevant next to the document they mutate. Boxing would only add
/// indirection without a measurable win.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Command {
    CreateNode {
        id: NodeId,
        kind: NodeKind,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        /// Draw-order slot. `None` = append. `Some(i)` restores an undo slot.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        index: Option<usize>,
    },
    DeleteNode {
        id: NodeId,
    },
    SetParameter {
        node_id: NodeId,
        property: String,
        value: ParamValue,
    },
    SetVariable {
        name: VariableId,
        value: f64,
    },
    RemoveVariable {
        name: VariableId,
    },
    /// Define (or redefine) an expression source. Validation is a boundary
    /// concern (`vectra-wasm` pre-compiles); core stores the source.
    DefineExpression {
        id: ExpressionId,
        source: String,
    },
    RemoveExpression {
        id: ExpressionId,
    },
    /// Register a hard mathematical rule (MES §9, Task 3.1). The engine re-runs
    /// the solver immediately after this applies; the properties it adjusts
    /// belong to *this* history entry, so one undo reverts rule and geometry
    /// together.
    AddConstraint {
        constraint: Constraint,
    },
    RemoveConstraint {
        id: ConstraintId,
    },
    /// Park or re-arm a registered constraint without deleting it.
    SetConstraintEnabled {
        id: ConstraintId,
        enabled: bool,
    },
    /// Start a drag gesture on a node (Task 3.2).
    ///
    /// Purely a *session* command: it changes no document state, it tells the
    /// engine to open a live solver session in which the node's canonical
    /// position slots are Cassowary **edit variables** at `STRONG`. Every
    /// following `UpdateDrag` nudges them with `suggest_value`, and the geometry
    /// that moves (this node plus whatever the constraints pull along) is
    /// applied as ordinary writes, folded into the single history entry that
    /// `EndDrag` records.
    BeginDrag {
        node_id: NodeId,
    },
    /// Move a dragged node to an **absolute** position, resolved by the solver.
    ///
    /// `x`/`y` are the target values of the node's canonical position slots
    /// (`x`/`y` for a rectangle, `cx`/`cy` for a circle or arc). The engine
    /// routes this through the open drag session — never through a bare
    /// `SetParameter` write.
    UpdateDrag {
        node_id: NodeId,
        x: f64,
        y: f64,
    },
    /// Finish a drag gesture: the last solved values stay in the document as
    /// literals, the edit variables are released, and the gesture becomes one
    /// undoable entry.
    EndDrag {
        node_id: NodeId,
    },
    /// Register a non-destructive operation (MES §10, Task 4.0).
    ///
    /// The **inputs are not touched**: the new [`OperationNode`] merely reads
    /// their evaluated geometry, so this command mutates only the operation
    /// registry. The id is carried by the command (like `CreateNode`), so undo
    /// and redo address the same virtual node.
    ApplyOperation {
        id: OperationId,
        kind: OperationKind,
        inputs: Vec<NodeId>,
        /// The record's paint, when the caller has one to restore (Task 12.0).
        ///
        /// `RemoveOperation` is undone by re-applying this command, and a
        /// Smart Fill is *created with a colour* (RULE 4's drop) — so an inverse
        /// that rebuilt the operation from its kind alone would redo a red fill
        /// as the default orange. Omitted by callers that only mean "make this
        /// operation exist", which is why it is `Option` and `#[serde(default)]`
        /// rather than a required field: the wire shape Task 4.0 shipped stays
        /// valid.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        style: Option<StyleProperties>,
        /// The record's display name, on the same terms as `style`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
    },
    /// Withdraw an operation. The inputs live on exactly as they were (RULE 1);
    /// only the virtual result disappears.
    RemoveOperation {
        id: OperationId,
    },
    /// Park or re-arm an operation without deleting it: a parked operation
    /// keeps its inputs and its place in the registry, but contributes no
    /// geometry to the scene.
    SetOperationEnabled {
        id: OperationId,
        enabled: bool,
    },
    /// **Create a Smart Fill** (Task 12.0 RULES 2 and 4): a parametric region
    /// pinned between `boundaries`, at the face under `seed`.
    ///
    /// The boundaries are ordinary node ids and **nothing about them is
    /// edited** — the fill reads their evaluated geometry, exactly as
    /// [`Command::ApplyOperation`]'s boolean reads its operands. That is what
    /// makes it parametric: move a boundary and the dirty set reaches the fill's
    /// `inputs`, so the pass recomputes the region and the fill follows.
    ///
    /// `fill` is RULE 4: a ColorDrop arrives here with the colour that was
    /// dropped, so the drop *creates* paint rather than rewriting the boundary's
    /// style. Omitted (a click with the Smart Fill tool, or an AI request) the
    /// fill takes the operations registry's own default.
    CreateSmartFill {
        id: OperationId,
        boundaries: Vec<NodeId>,
        seed: (f64, f64),
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fill: Option<Color>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
    },
    /// **Break a path at its intersection points** (Task 12.0 RULE 3).
    ///
    /// The pieces travel with the command — exactly like [`Command::OutlineText`]'s
    /// letterforms, and for the same reason: computing them needs *resolved*
    /// geometry (any point slot may be a variable or an expression), and
    /// [`Command::apply`] has no evaluation context. The boundary builds them
    /// from the region graph's spans; core validates, mints and places them.
    ///
    /// Non-destructive, like every other derived geometry in this document: the
    /// source path is **hidden, not deleted**, and one undo restores it.
    BreakPath {
        node_id: NodeId,
        pieces: Vec<OutlinePath>,
    },
    /// Bind a motion source to a numeric slot (MES §12, Task 6.0).
    ///
    /// The slot keeps its identity and its address: this is an ordinary
    /// parameter write whose *source* happens to be a spring, a state branch or
    /// a keyframe track. The inverse is therefore the generic
    /// `SetParameter { value: <what the slot held before> }`, which restores a
    /// literal, a variable reference or a previous binding exactly — binding and
    /// unbinding are both undoable, with no special-case inverse machinery.
    ///
    /// Motion **samples** never write: this command records where the animation
    /// starts, not where it is now.
    BindMotion {
        node_id: NodeId,
        property: String,
        binding: MotionBinding,
    },
    /// Create or replace a keyframe track (validated on insertion). The inverse
    /// is the previous track, or [`Command::RemoveMotionTrack`] if there was
    /// none.
    SetMotionTrack {
        track: MotionTrack,
    },
    /// Remove a keyframe track. The inverse restores it verbatim.
    RemoveMotionTrack {
        track_id: TrackId,
    },
    /// Register a procedural node (MES §11, Task 7.0).
    ///
    /// The record is carried whole — kind, wiring, operands, style — which is
    /// what makes the inverse exact with no reconstruction. Nothing else in the
    /// document is touched: a node's *sources* are read through its `Source`
    /// wiring (Task 4.0 RULE 1), never mutated by adding it.
    AddProceduralNode {
        node: ProceduralNode,
    },
    /// Withdraw a procedural node. The inverse restores the record **and** the
    /// wires other nodes had into it, so undo puts the graph back exactly as it
    /// was (a downstream node is never left dangling by a redo, either).
    RemoveProceduralNode {
        id: NodeId,
    },
    /// Wire an input port to an upstream output port (RULE 1: the types must
    /// match, and the wire must not close a chain cycle — both rejected before
    /// this command touches the document).
    ConnectProcedural {
        node_id: NodeId,
        port: crate::ids::PortId,
        from: NodeOutputId,
    },
    /// Unwire an input port. The inverse re-connects the exact address.
    DisconnectProcedural {
        node_id: NodeId,
        port: crate::ids::PortId,
    },
    /// Write a node's operand (a `Parameter`, so a grid's spacing is as
    /// parametric as any other number in the document).
    ///
    /// **RULE 3**: the value may read a variable, an expression or motion, but
    /// never another node's procedural output — a value reference back into the
    /// graph is a cycle wearing a disguise, and it is rejected here.
    SetProceduralOperand {
        node_id: NodeId,
        port: crate::ids::PortId,
        value: ParamValue,
    },
    /// Park or re-arm a procedural node: a parked node keeps its wiring but
    /// contributes no geometry and publishes no values.
    SetProceduralEnabled {
        id: NodeId,
        enabled: bool,
    },
    /// **Rewrite a path's vertex list** (Task 10.1, the drawing tools).
    ///
    /// This is the authoring command behind the Pen tool, the Vector Brush and
    /// every handle drag, and the reason it exists as a command rather than as a
    /// sequence of point writes is a **type** fact, not a convenience one: a
    /// segment's *kind* is part of its type. A `Line` has one endpoint, a
    /// `Cubic` has an endpoint and two control points, and switching between
    /// them is not a value change `SetParameter` can express — no sequence of
    /// point writes turns a line into a curve.
    ///
    /// `SetParameter` remains the tool for editing a point *within* a segment
    /// (a handle drag, an anchor nudge) — [`crate::document::Node::path_slots`]
    /// enumerates every such slot, and the drawing tools use those writes
    /// wherever they can. This command is for the structural edit.
    ///
    /// ## The inverse: the ambiguity function is the identity
    ///
    /// A path's geometry is exactly two fields (`start` and `segments`), so the
    /// inverse is the same command carrying the values it replaced — no diffing,
    /// no special cases, total and lossless:
    ///
    /// ```text
    ///   forward  = SetPath { id, start: draft,   segments: draft   }
    ///   backward = SetPath { id, start: current, segments: current }
    /// ```
    ///
    /// Undo therefore restores a pen sketch, a brush stroke *and* a snapped
    /// primitive identically. A node whose kind is not a `Path` is refused
    /// rather than silently ignored.
    SetPath {
        id: NodeId,
        start: Parameter<Point2>,
        segments: Vec<PathSegment>,
    },
    /// **Rewrite a node's paint stack** (Task 10.2 RULE 3).
    ///
    /// The stack is *structure*: adding a second stroke, reordering fills and
    /// switching a fill from solid to a linear gradient all change the list's
    /// length or the shape of its entries, which no sequence of `SetParameter`
    /// writes can express — the same argument that gave the drawing tools
    /// [`Command::SetPath`]. Scalars *inside* the stack (an opacity, a stroke
    /// width, a gradient's frame, a solid colour) stay ordinary `SetParameter`
    /// writes, so a slider drag is one write per sample and the panel needs no
    /// special case.
    ///
    /// Two fields, one inverse, exactly like `SetPath`: the command carries the
    /// whole list, so undo is the same command holding what it replaced.
    SetAppearances {
        node_id: NodeId,
        appearances: Vec<AppearanceLayer>,
    },
    /// **Rewrite a text node's string** (Task 11.0 RULE 1).
    ///
    /// A string is not a `ParamValue`, so this is its own command rather than a
    /// `SetParameter` write — the same reason [`Command::RenameNode`] exists.
    /// It is *only* the string: family, alignment and every number stay where
    /// they are, so retyping a word never moves the type it was set in.
    SetText {
        node_id: NodeId,
        text: String,
    },
    /// Choose a text node's font family (a string, like [`Command::RenameNode`]):
    /// the name the boundary's font library resolves. An unknown family is
    /// accepted and *diagnosed* — the run falls back to the bundled face — so a
    /// document written on one machine opens on another with its words intact.
    SetFontFamily {
        node_id: NodeId,
        family: String,
    },
    /// Set a text node's line alignment. Structure, not a number: the layout
    /// branches on it (see [`crate::document::TextAlign::line_start`]).
    SetTextAlignment {
        node_id: NodeId,
        alignment: TextAlign,
    },
    /// **Bind a text node to a path** (RULE 2): the run follows the bound node's
    /// evaluated geometry, sliding `offset` document units along it.
    ///
    /// Validated before anything is stored: the target must exist and must be
    /// *path-shaped* (`Path`, `Arc` or `Circle`). That restriction is what lets
    /// evaluation stay a single pass — a text node can never bind to text, so
    /// the run's geometry is always resolvable without recursion.
    ///
    /// Binding a node that is already bound is a *re-bind*; the inverse carries
    /// the previous binding, so ⌘Z restores the old path **and** the old offset.
    BindTextToPath {
        node_id: NodeId,
        path: NodeId,
        offset: Parameter<f64>,
    },
    /// Unbind a text node: it returns to its `x`/`y` baseline, keeping every
    /// typographic property (and its text) exactly as it was.
    UnbindTextFromPath {
        node_id: NodeId,
    },
    /// **Outline to paths** (RULE 3): replace a text node's *rendering* with real
    /// letterform geometry — one [`NodeKind::Path`] per outlined run, grouped,
    /// with the original text node hidden rather than deleted.
    ///
    /// # Why the command carries the plan
    ///
    /// Shaping is not the core's business: the glyph outlines come out of the
    /// font library, which lives in `vectra-geometry`. The command therefore
    /// carries what that library produced — `group_id` and one
    /// [`OutlinePath`] per letterform — exactly as [`Command::ApplyOperation`]
    /// carries an operation record and [`Command::SetPath`] carries a pen
    /// sketch. Core stays font-free, the boundary stays thin, and the whole
    /// conversion is still **one undoable command**.
    ///
    /// # Non-destructive, and that is a testable claim
    ///
    /// The text node is not deleted and not edited: it is *hidden*
    /// (`visible = false`), so its string, its parameters and its bindings are
    /// all still there — the inverse re-shows it and removes the group. Undo of
    /// an outline is therefore exact, and redo re-applies it with the same ids,
    /// so a boolean built on an outlined letterform survives the round trip.
    OutlineText {
        node_id: NodeId,
        /// The group that holds the outlined letterforms (minted by the caller,
        /// so undo and redo address the same node).
        group_id: NodeId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        /// One entry per outlined letterform, in draw order.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        paths: Vec<OutlinePath>,
    },
    /// Show or hide a node (Task 10.2 RULE 4). One `bool`, one history entry,
    /// **no re-evaluation** — the event it publishes is `NodeFlagsChanged`.
    SetNodeVisible {
        id: NodeId,
        visible: bool,
    },
    /// Lock or unlock a node. A locked node is skipped by hit-testing (so a
    /// click passes through to what is underneath) and by the selection tools,
    /// but it still draws.
    SetNodeLocked {
        id: NodeId,
        locked: bool,
    },
    /// Rename a node. Names are plain strings — not parametric in Phase 1 — so
    /// this is the only writer.
    RenameNode {
        id: NodeId,
        name: String,
    },
    /// **Create a layer** (Task 10.2 RULE 1).
    CreateLayer {
        id: LayerId,
        name: String,
        /// Z-position among layers; `None` = on top.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        index: Option<usize>,
        /// Which artboard owns it; `None` = the active one.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        artboard: Option<ArtboardId>,
    },
    /// Delete a layer. Its nodes are **not** deleted: they become unassigned and
    /// keep drawing (the inverse restores the layer with its children, so undo
    /// puts the arrangement back exactly).
    DeleteLayer {
        id: LayerId,
    },
    RenameLayer {
        id: LayerId,
        name: String,
    },
    /// Toggle a layer's eye. Same contract as [`Command::SetNodeVisible`]: a
    /// flag, one history entry, no re-evaluation.
    SetLayerVisible {
        id: LayerId,
        visible: bool,
    },
    /// Toggle a layer's padlock: its nodes stay visible but stop responding to
    /// the pointer.
    SetLayerLocked {
        id: LayerId,
        locked: bool,
    },
    /// **Alpha Lock** (Task 10.7 RULE 3a): constrain new artwork on this layer
    /// to the layer's existing content.
    ///
    /// A flag like the eye and the padlock — one history entry, no
    /// re-evaluation, no geometry touched — but a flag the *drawing boundary*
    /// reads: while it is on, a committed stroke is intersected with the layer's
    /// content before it is stored, so a designer cannot paint outside the lines
    /// they already have.
    SetLayerAlphaLocked {
        id: LayerId,
        alpha_locked: bool,
    },
    /// **Clipping Mask** (Task 10.7 RULE 3b): show this layer only where it
    /// overlaps the layer below.
    ///
    /// This one *is* geometry: the scene pass reshapes the layer's nodes to
    /// their intersection with the layer below, so the toggle reports the
    /// affected nodes as updated (they must be re-sent to the renderer) while
    /// still resolving no parameter.
    SetLayerClippingMask {
        id: LayerId,
        clipping_mask: bool,
    },
    /// Move a layer in the z-order. Its nodes travel with it — a layer is a unit
    /// in the draw order, which is what makes dragging one in the panel move
    /// everything on it.
    ReorderLayer {
        id: LayerId,
        index: usize,
    },
    /// Move a node to a layer. Membership is the only thing that changes: the
    /// node's identity, geometry and style are untouched, which is what makes
    /// reorganizing a document non-destructive.
    AssignNodeToLayer {
        node_id: NodeId,
        layer: LayerId,
    },
    /// Take a node out of every layer. It keeps drawing — it is unassigned, not
    /// deleted — and it is the exact inverse of assigning a node that had no
    /// layer, which is why it exists as a command rather than as a special case
    /// of [`Command::AssignNodeToLayer`].
    DetachNodeFromLayers {
        node_id: NodeId,
    },
    /// **Create an artboard** (Task 10.2 RULE 2): a named frame with a bounding
    /// box and a background colour.
    /// **Move a node into a group, out of every group, or to another layer's
    /// top level** — and to a position among the destination's siblings
    /// (Task 10.4 RULE 1).
    ///
    /// `parent: None` means "listed directly by the layer", which is not the same
    /// as "unassigned" ([`Command::DetachNodeFromLayers`] is that): the node keeps
    /// its layer and leaves only its group.
    ///
    /// Refused typed before anything moves when the destination is not a group
    /// (`NotAGroup`) or is the node itself or one of its descendants
    /// (`GroupCycle`) — a document can never hold a group inside itself, so the
    /// drag that would make one is answered instead of silently ignored.
    SetNodeParent {
        id: NodeId,
        parent: Option<NodeId>,
        index: usize,
    },
    CreateArtboard {
        id: ArtboardId,
        name: String,
        x: f64,
        y: f64,
        width: f64,
        height: f64,
        #[serde(default = "default_artboard_background")]
        background: Color,
    },
    /// Delete an artboard. Its layers survive (they become unlisted), because
    /// deleting a frame is not deleting the artwork in it.
    DeleteArtboard {
        id: ArtboardId,
    },
    RenameArtboard {
        id: ArtboardId,
        name: String,
    },
    /// Re-frame an artboard (the move/resize handle on its border).
    SetArtboardBounds {
        id: ArtboardId,
        x: f64,
        y: f64,
        width: f64,
        height: f64,
    },
    SetArtboardBackground {
        id: ArtboardId,
        background: Color,
    },
    /// Switch the active artboard. A pointer, not structure — but undoable like
    /// everything else, because a designer who jumped boards by accident
    /// expects ⌘Z to take them back.
    SetActiveArtboard {
        id: ArtboardId,
    },
    /// Make a layer current: the one `CreateNode` fills from now on.
    SetActiveLayer {
        id: LayerId,
    },
    /// **Create a Smart Component** (Task 10.6 RULE 1): a
    /// [`crate::procedural::ProceduralKind::ComponentMaster`] whose members are
    /// the given nodes, plus the bindings that make those members parametric.
    ///
    /// The members keep their identity — a component is a *view* of the artwork
    /// plus a set of variable-backed slots, not a copy of it. Each prop's slots
    /// are rebound to a variable the command creates (scalar props) or to a port
    /// the master publishes (colour props), which is what makes "set a prop"
    /// instant and local: the dependency graph already knows how to propagate a
    /// variable to exactly the slots that read it.
    ///
    /// `props` may be empty, in which case [`crate::component::infer_props`]
    /// decides them — the panel can also send an explicit list, because a
    /// designer who renamed a prop expects the rename to stick.
    CreateComponent {
        id: NodeId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        members: Vec<NodeId>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        props: Vec<crate::component::ComponentProp>,
    },
    /// **Place an instance** of a Smart Component (RULE 1): a
    /// [`crate::procedural::ProceduralKind::Component`] node plus a group
    /// holding its own clones, bound to its own variables.
    ///
    /// The clones are ordinary authored nodes, so they layer, hit-test and
    /// export like anything else; nothing in the renderer has to know that
    /// components exist.
    InstantiateComponent {
        id: NodeId,
        master: NodeId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        index: Option<usize>,
    },
    /// **Copy a node** (Task 10.6 RULE 2): the structural primitive behind "create
    /// 4 colour variations".
    ///
    /// A copy, not a reference: duplicating is how a designer explores, and the
    /// copies are ordinary nodes they can then edit, name and delete. Parameters
    /// come across as they are — a duplicate of a `$base`-wide rectangle is
    /// still `$base` wide, which is the *useful* default and the reason a
    /// variation set stays a family.
    DuplicateNode {
        id: NodeId,
        source: NodeId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        index: Option<usize>,
    },
    /// **Set a prop on one instance** (RULE 1). Only that instance's variable
    /// (or port) is written, so only its clones re-evaluate — the master and
    /// every other instance are untouched.
    SetComponentProp {
        target: NodeId,
        prop: String,
        value: ParamValue,
    },
    /// Replace a component's prop list (used by the panel to add, remove or
    /// rename props without recreating the component).
    SetComponentSpec {
        id: NodeId,
        spec: crate::component::ComponentSpec,
    },
    /// A composite: apply children in order; the inverse is the reversed
    /// inverses. Used for one user action whose document effect is several
    /// mutations (e.g. `DeleteNode` also withdrawing the constraints that
    /// targeted the node, or a solver pass amending what it adjusted).
    Batch {
        commands: Vec<Command>,
    },
}

/// The background a new artboard gets when the caller does not say: white, the
/// colour a designer means by "a blank canvas" in nine cases out of ten, and the
/// only default that never makes artwork look wrong.
fn default_artboard_background() -> Color {
    Color::WHITE
}

/// Move a freshly inserted node onto `layer` (Task 11.0).
///
/// Fetch a node that must be a **text** node, or explain why it is not.
///
/// The three text commands and the outline all begin with this question; asking
/// it once is what keeps their refusals identical (`UnknownProperty`-shaped for
/// a non-text node, naming the property the caller meant) instead of drifting
/// into four different messages.
fn text_node_mut<'a>(
    doc: &'a mut Document,
    id: NodeId,
    property: &str,
) -> Result<&'a mut Node, VectraError> {
    let kind_tag = doc
        .nodes
        .get(&id)
        .map(|node| node.kind.tag().to_string())
        .unwrap_or_else(|| "missing".to_string());
    let node = doc
        .nodes
        .get_mut(&id)
        .ok_or(VectraError::NodeNotFound(id))?;
    if !matches!(node.kind, NodeKind::Text { .. }) {
        return Err(VectraError::UnknownProperty {
            node_id: id,
            node_kind: kind_tag,
            property: property.to_string(),
        });
    }
    Ok(node)
}

/// One outlined letterform, ready to be inserted as a [`NodeKind::Path`]
/// (Task 11.0 RULE 3).
///
/// The geometry is the same `start` + `segments` pair a drawn path uses, so an
/// outlined glyph is not a second kind of geometry: it can be grouped,
/// booleaned, constrained, animated and exported by everything that already
/// exists. One entry per *letterform* (a character's contours, outer and
/// counters, are subpaths of the one node).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OutlinePath {
    /// The id the caller minted for this letterform's node.
    pub id: NodeId,
    /// Display name — the character, so the layers panel reads `A`, `b`, `“`.
    pub name: String,
    pub start: Parameter<Point2>,
    pub segments: Vec<PathSegment>,
}

impl Command {
    /// Human-readable label for undo menus / devtools.
    pub fn label(&self) -> String {
        match self {
            Self::CreateNode { kind, .. } => format!("Create {}", kind.tag()),
            Self::DeleteNode { .. } => "Delete node".to_string(),
            Self::SetParameter { property, .. } => format!("Set {property}"),
            Self::SetVariable { name, .. } => format!("Set ${name}"),
            Self::RemoveVariable { name } => format!("Remove ${name}"),
            Self::DefineExpression { .. } => "Define expression".to_string(),
            Self::RemoveExpression { .. } => "Remove expression".to_string(),
            Self::AddConstraint { constraint } => {
                format!("Add {} constraint", constraint.kind.tag())
            }
            Self::RemoveConstraint { .. } => "Remove constraint".to_string(),
            Self::SetConstraintEnabled { enabled, .. } => {
                if *enabled {
                    "Enable constraint".to_string()
                } else {
                    "Disable constraint".to_string()
                }
            }
            Self::ApplyOperation { kind, .. } => match kind {
                OperationKind::Boolean { op } => format!("Apply {}", op.tag()),
                other => format!("Apply {}", other.tag()),
            },
            Self::RemoveOperation { .. } => "Remove operation".to_string(),
            Self::CreateSmartFill { boundaries, .. } => {
                format!("Smart fill ({} boundary/ies)", boundaries.len())
            }
            Self::BreakPath { pieces, .. } => format!("Break path ({} piece(s))", pieces.len()),
            Self::SetOperationEnabled { enabled, .. } => {
                if *enabled {
                    "Enable operation".to_string()
                } else {
                    "Disable operation".to_string()
                }
            }
            Self::BindMotion { binding, .. } => format!("Bind {}", binding.tag()),
            Self::SetMotionTrack { .. } => "Set motion track".to_string(),
            Self::RemoveMotionTrack { .. } => "Remove motion track".to_string(),
            Self::AddProceduralNode { node } => format!("Add {} node", node.kind.tag()),
            Self::RemoveProceduralNode { .. } => "Remove procedural node".to_string(),
            Self::ConnectProcedural { port, .. } => format!("Connect {port}"),
            Self::DisconnectProcedural { port, .. } => format!("Disconnect {port}"),
            Self::SetProceduralOperand { port, .. } => format!("Set {port}"),
            Self::SetProceduralEnabled { enabled, .. } => {
                if *enabled {
                    "Enable procedural node".to_string()
                } else {
                    "Disable procedural node".to_string()
                }
            }
            Self::BeginDrag { .. } => "Begin drag".to_string(),
            Self::UpdateDrag { .. } => "Drag".to_string(),
            Self::EndDrag { .. } => "End drag".to_string(),
            // One label for the whole rewrite: "Set start" would describe the
            // first point of a stroke that has two hundred of them.
            Self::SetPath { segments, .. } => format!("Set path ({} segment(s))", segments.len()),
            Self::SetAppearances { appearances, .. } => {
                format!("Set appearance ({} layer(s))", appearances.len())
            }
            // The label quotes the *edit*, the way a text field's undo entry
            // should: a designer undoing a retype wants to see what they typed.
            Self::SetText { text, .. } => {
                let short: String = text.chars().take(24).collect();
                if text.chars().count() > 24 {
                    format!("Type \"{short}…\"")
                } else {
                    format!("Type \"{short}\"")
                }
            }
            Self::SetFontFamily { family, .. } => format!("Font: {family}"),
            Self::SetTextAlignment { alignment, .. } => format!("Align {}", alignment.tag()),
            Self::BindTextToPath { .. } => "Bind text to path".to_string(),
            Self::UnbindTextFromPath { .. } => "Unbind text from path".to_string(),
            Self::OutlineText { paths, .. } => {
                format!("Outline text ({} letterform(s))", paths.len())
            }
            Self::SetNodeVisible { visible, .. } => {
                if *visible { "Show node" } else { "Hide node" }.to_string()
            }
            Self::SetNodeLocked { locked, .. } => {
                if *locked { "Lock node" } else { "Unlock node" }.to_string()
            }
            Self::RenameNode { name, .. } => format!("Rename to \"{name}\""),
            Self::CreateLayer { name, .. } => format!("New layer \"{name}\""),
            Self::DeleteLayer { .. } => "Delete layer".to_string(),
            Self::RenameLayer { name, .. } => format!("Rename layer to \"{name}\""),
            Self::SetLayerVisible { visible, .. } => {
                if *visible { "Show layer" } else { "Hide layer" }.to_string()
            }
            Self::SetLayerLocked { locked, .. } => if *locked {
                "Lock layer"
            } else {
                "Unlock layer"
            }
            .to_string(),
            Self::SetLayerAlphaLocked { alpha_locked, .. } => if *alpha_locked {
                "Lock alpha"
            } else {
                "Unlock alpha"
            }
            .to_string(),
            Self::SetLayerClippingMask { clipping_mask, .. } => if *clipping_mask {
                "Clip to layer below"
            } else {
                "Remove clipping mask"
            }
            .to_string(),
            Self::ReorderLayer { .. } => "Reorder layer".to_string(),
            Self::AssignNodeToLayer { .. } => "Move to layer".to_string(),
            Self::DetachNodeFromLayers { .. } => "Remove from layer".to_string(),
            Self::SetNodeParent { parent, .. } => match parent {
                Some(_) => "Move into group".to_string(),
                None => "Move out of group".to_string(),
            },
            Self::CreateArtboard { name, .. } => format!("New artboard \"{name}\""),
            Self::DeleteArtboard { .. } => "Delete artboard".to_string(),
            Self::RenameArtboard { name, .. } => format!("Rename artboard to \"{name}\""),
            Self::SetArtboardBounds { .. } => "Frame artboard".to_string(),
            Self::SetArtboardBackground { .. } => "Artboard background".to_string(),
            Self::SetActiveArtboard { .. } => "Switch artboard".to_string(),
            Self::SetActiveLayer { .. } => "Switch layer".to_string(),
            Self::CreateComponent { name, members, .. } => format!(
                "Create component \"{}\" from {} shape(s)",
                name.clone().unwrap_or_else(|| "component".to_string()),
                members.len()
            ),
            Self::InstantiateComponent { .. } => "Place component instance".to_string(),
            Self::SetComponentProp { prop, .. } => format!("Set {prop}"),
            Self::DuplicateNode { name, .. } => format!(
                "Duplicate to \"{}\"",
                name.clone().unwrap_or_else(|| "copy".to_string())
            ),
            Self::SetComponentSpec { .. } => "Edit component props".to_string(),
            Self::Batch { commands } => match commands.len() {
                0 => "No-op".to_string(),
                1 => commands[0].label(),
                n => format!("{n} changes"),
            },
        }
    }

    /// Events a UI should expect if this command succeeds. Computed from the
    /// *intent* (not the outcome) so the React layer can optimistically update.
    pub fn preview_events(&self) -> Vec<EngineEvent> {
        match self {
            Self::CreateNode { id, .. } => vec![
                EngineEvent::NodesUpdated { ids: vec![*id] },
                EngineEvent::OrderChanged,
            ],
            // A path rewrite is a geometry change on one node — the same event
            // shape `SetParameter` publishes, so the incremental evaluator
            // dirties exactly this node and nothing else.
            Self::SetPath { id, .. } => vec![EngineEvent::NodesUpdated { ids: vec![*id] }],
            // Text edits are node changes like any other: the string, the
            // family, the alignment and the binding all change *this* node's
            // geometry, and the run re-lays out on the next settle.
            Self::SetText { node_id, .. }
            | Self::SetFontFamily { node_id, .. }
            | Self::SetTextAlignment { node_id, .. }
            | Self::BindTextToPath { node_id, .. }
            | Self::UnbindTextFromPath { node_id } => vec![EngineEvent::NodesUpdated {
                ids: vec![*node_id],
            }],
            // Outlining touches three sets of ids: the group that now holds the
            // letterforms, the letterforms themselves (new geometry the renderer
            // has never seen) and the source text node (which stops drawing).
            Self::OutlineText {
                node_id,
                group_id,
                paths,
                ..
            } => {
                let mut ids = vec![*node_id, *group_id];
                ids.extend(paths.iter().map(|path| path.id));
                vec![EngineEvent::NodesUpdated { ids }, EngineEvent::OrderChanged]
            }
            Self::DeleteNode { id } => vec![
                EngineEvent::NodesRemoved { ids: vec![*id] },
                EngineEvent::OrderChanged,
            ],
            Self::SetParameter { node_id, .. } => vec![EngineEvent::NodesUpdated {
                ids: vec![*node_id],
            }],
            // A bound slot is a node change (it must re-evaluate now). A *track*
            // edit needs no node list: the graph carries `property → track`
            // edges, so `TracksUpdated` dirties exactly the slots that sample it.
            Self::BindMotion { node_id, .. } => vec![EngineEvent::NodesUpdated {
                ids: vec![*node_id],
            }],
            Self::SetMotionTrack { track } => vec![EngineEvent::TracksUpdated {
                ids: vec![track.id.clone()],
            }],
            Self::RemoveMotionTrack { track_id } => vec![EngineEvent::TracksUpdated {
                ids: vec![track_id.clone()],
            }],
            Self::SetVariable { name, .. } | Self::RemoveVariable { name } => {
                vec![EngineEvent::VariablesUpdated {
                    names: vec![name.clone()],
                }]
            }
            Self::DefineExpression { id, .. } | Self::RemoveExpression { id } => {
                vec![EngineEvent::ExpressionsUpdated { ids: vec![*id] }]
            }
            Self::AddConstraint { constraint } => vec![EngineEvent::ConstraintsUpdated {
                ids: vec![constraint.id],
            }],
            Self::RemoveConstraint { id } => {
                vec![EngineEvent::ConstraintsUpdated { ids: vec![*id] }]
            }
            Self::SetConstraintEnabled { id, .. } => {
                vec![EngineEvent::ConstraintsUpdated { ids: vec![*id] }]
            }
            Self::BeginDrag { node_id } => vec![EngineEvent::DragStarted { node_id: *node_id }],
            // The intent of a pointer sample: this node moved. The engine emits
            // the full set (dragged node + everything the constraints pulled)
            // from the solve's own dirty set.
            Self::UpdateDrag { node_id, .. } => vec![EngineEvent::NodesUpdated {
                ids: vec![*node_id],
            }],
            Self::EndDrag { node_id } => vec![EngineEvent::DragEnded { node_id: *node_id }],
            // The operation's *inputs* are announced too: the result is derived
            // geometry, and a UI that already knows the sources can decide how
            // much of the scene it must re-read.
            Self::ApplyOperation { id, inputs, .. } => vec![
                EngineEvent::OperationsUpdated { ids: vec![*id] },
                EngineEvent::NodesUpdated {
                    ids: inputs.clone(),
                },
                EngineEvent::OrderChanged,
            ],
            Self::RemoveOperation { id } => vec![
                EngineEvent::OperationsUpdated { ids: vec![*id] },
                EngineEvent::NodesRemoved { ids: vec![*id] },
                EngineEvent::OrderChanged,
            ],
            Self::SetOperationEnabled { id, .. } => {
                vec![EngineEvent::OperationsUpdated { ids: vec![*id] }]
            }
            // A Smart Fill is a registry entry: creating it touches no source,
            // so the event is the registry one. The fill's *geometry* arrives
            // through the operations pass, exactly like a boolean's.
            Self::CreateSmartFill { id, .. } => {
                vec![EngineEvent::OperationsUpdated { ids: vec![*id] }]
            }
            // Breaking a path is an ordinary geometry edit of the document's own
            // nodes: the pieces are new nodes, the source is hidden, and the
            // dirty ids are exactly those — so the dependency graph re-resolves
            // the pieces' slots and every Smart Fill reading the source
            // re-derives its region through `affected_by`, with no special case.
            Self::BreakPath { node_id, pieces } => {
                let mut ids: Vec<NodeId> = vec![*node_id];
                ids.extend(pieces.iter().map(|piece| piece.id));
                vec![EngineEvent::NodesUpdated { ids }]
            }
            // A procedural change dirties its own node; the pass then
            // propagates along the chain and the readers of the changed ports
            // (Task 7.0), exactly as an operation announces itself before the
            // geometry it produces arrives in `Dirty`.
            Self::AddProceduralNode { node } => vec![
                EngineEvent::ProceduralUpdated { ids: vec![node.id] },
                EngineEvent::OrderChanged,
            ],
            Self::RemoveProceduralNode { id } => vec![
                EngineEvent::ProceduralUpdated { ids: vec![*id] },
                EngineEvent::NodesRemoved { ids: vec![*id] },
                EngineEvent::OrderChanged,
            ],
            Self::ConnectProcedural { node_id, .. }
            | Self::DisconnectProcedural { node_id, .. }
            | Self::SetProceduralOperand { node_id, .. } => vec![EngineEvent::ProceduralUpdated {
                ids: vec![*node_id],
            }],
            Self::SetProceduralEnabled { id, .. } => {
                vec![EngineEvent::ProceduralUpdated { ids: vec![*id] }]
            }
            // A paint rewrite is a *style* change on one node: the geometry is
            // untouched, but the node must re-evaluate (its appearance stack is
            // resolved there) — so the event is the ordinary `NodesUpdated`.
            Self::SetAppearances { node_id, .. } | Self::RenameNode { id: node_id, .. } => {
                vec![EngineEvent::NodesUpdated {
                    ids: vec![*node_id],
                }]
            }
            // **RULE 4.** The eye and the padlock publish a flags event that the
            // dependency graph deliberately ignores: no slot re-resolves, no
            // mesh is rebuilt. The renderer is the listener.
            Self::SetNodeVisible { id, .. } | Self::SetNodeLocked { id, .. } => {
                vec![EngineEvent::NodeFlagsChanged { ids: vec![*id] }]
            }
            Self::CreateLayer { id, .. } => vec![EngineEvent::LayersUpdated { ids: vec![*id] }],
            Self::DeleteLayer { id } => vec![EngineEvent::LayersUpdated { ids: vec![*id] }],
            Self::RenameLayer { id, .. }
            | Self::SetLayerVisible { id, .. }
            | Self::SetLayerLocked { id, .. }
            | Self::SetLayerAlphaLocked { id, .. } => {
                vec![EngineEvent::LayersUpdated { ids: vec![*id] }]
            }
            // A clipping mask *reshapes* the layer's nodes, so those ids are
            // reported as updated: the evaluator resolves no parameter (the
            // nodes' own slots are untouched) but the scene pass rewrites their
            // primitives, and the renderer has to hear about it.
            // The affected nodes' ids are not available here (a command does not
            // read the document), and they do not need to be: the scene pass
            // reports the ids whose primitives it rewrote, and `settle` folds
            // them into the `Dirty` event the renderer reads.
            Self::SetLayerClippingMask { id, .. } => {
                vec![EngineEvent::LayersUpdated { ids: vec![*id] }]
            }
            // Moving a layer, moving a node, or re-homing one all change the
            // **draw order**, which the scene re-reads without re-evaluating
            // anything: `LayerOrderChanged` carries no geometry.
            Self::ReorderLayer { id, .. } => vec![
                EngineEvent::LayersUpdated { ids: vec![*id] },
                EngineEvent::LayerOrderChanged,
            ],
            Self::AssignNodeToLayer { node_id, layer } => vec![
                EngineEvent::LayersUpdated { ids: vec![*layer] },
                EngineEvent::NodeFlagsChanged {
                    ids: vec![*node_id],
                },
                EngineEvent::LayerOrderChanged,
            ],
            Self::DetachNodeFromLayers { node_id } => vec![
                EngineEvent::NodeFlagsChanged {
                    ids: vec![*node_id],
                },
                EngineEvent::LayerOrderChanged,
            ],
            // A move is a *presentation* change: membership moved, no value did.
            // The node is named so the renderer re-reads its row (its origin and
            // selection flag are unchanged, but its sibling differs), and no
            // `Dirty` is emitted — which is RULE 4's bar for "does not
            // re-evaluate", now applied to the tree as well as the eye.
            Self::SetNodeParent { id, .. } => vec![
                EngineEvent::NodeFlagsChanged { ids: vec![*id] },
                EngineEvent::LayerOrderChanged,
            ],
            Self::CreateArtboard { id, .. }
            | Self::RenameArtboard { id, .. }
            | Self::SetArtboardBounds { id, .. }
            | Self::SetArtboardBackground { id, .. }
            | Self::SetActiveArtboard { id } => {
                vec![EngineEvent::ArtboardsUpdated { ids: vec![*id] }]
            }
            Self::DeleteArtboard { id } => vec![
                EngineEvent::ArtboardsUpdated { ids: vec![*id] },
                EngineEvent::LayerOrderChanged,
            ],
            Self::SetActiveLayer { id } => vec![EngineEvent::LayersUpdated { ids: vec![*id] }],
            // A component is a procedural node whose members are ordinary
            // nodes: the UI hears about both, and the dependency graph dirties
            // whatever the bindings touch.
            Self::CreateComponent { id, members, .. } => vec![
                EngineEvent::ProceduralUpdated { ids: vec![*id] },
                EngineEvent::NodesUpdated {
                    ids: members.clone(),
                },
                EngineEvent::VariablesUpdated { names: Vec::new() },
            ],
            Self::InstantiateComponent { id, .. } => vec![
                EngineEvent::ProceduralUpdated { ids: vec![*id] },
                EngineEvent::NodesUpdated { ids: vec![*id] },
                EngineEvent::OrderChanged,
            ],
            Self::SetComponentProp { target, .. } => vec![
                EngineEvent::ProceduralUpdated { ids: vec![*target] },
                EngineEvent::NodesUpdated { ids: vec![*target] },
            ],
            Self::DuplicateNode { id, .. } => vec![
                EngineEvent::NodesUpdated { ids: vec![*id] },
                EngineEvent::OrderChanged,
            ],
            Self::SetComponentSpec { id, .. } => {
                vec![EngineEvent::ProceduralUpdated { ids: vec![*id] }]
            }
            Self::Batch { commands } => {
                let mut events = Vec::new();
                for cmd in commands {
                    for event in cmd.preview_events() {
                        if !events.contains(&event) {
                            events.push(event);
                        }
                    }
                }
                events
            }
        }
    }

    /// Apply the command, returning its exact inverse for the undo stack.
    pub fn apply(&self, doc: &mut Document) -> Result<Command, VectraError> {
        match self {
            Self::CreateNode {
                id,
                kind,
                name,
                index,
            } => {
                let node = Node::new(
                    *id,
                    name.clone().unwrap_or_else(|| kind.tag().to_string()),
                    kind.clone(),
                );
                doc.insert_node(node, *index)?;
                Ok(Self::DeleteNode { id: *id })
            }
            Self::DeleteNode { id } => {
                // Where the node sat on its layer, and in which container,
                // captured *before* the removal: undoing a delete must put it
                // back in the same place on the same layer, not on whichever
                // layer happens to be active later.
                let membership = doc.layers.layer_of(*id).map(|(layer, _)| layer.id);
                let previous_parent = doc.parent_of(*id);
                let previous_index = doc.sibling_index(*id, previous_parent);
                let (node, index) = doc.remove_node(*id)?;
                // A deleted node must not linger in a layer's contents (RULE 1):
                // the id would survive a flatten that filters by existence today
                // and confuse `layer_of` tomorrow.
                doc.layers.detach(*id);
                // A deleted node must not leave a dangling rule behind, and the
                // undo of the delete restores those rules with the node.
                let withdrawn = doc.constraints.remove_targeting(*id);
                let mut restore = vec![Self::CreateNode {
                    id: node.id,
                    kind: node.kind,
                    name: Some(node.name),
                    index: Some(index),
                }];
                // Layer membership first, then the position *in its container*
                // — the pair is what makes undo of a delete invisible in the
                // panel as well as on the canvas. The position is the node's
                // sibling index, not its slot in the layer's list: for a member
                // of a group those are different numbers, and restoring the
                // layer slot would leave the node at the group's top level
                // instead of where it was among its siblings. (`SetNodeParent`
                // then does both jobs: the row's place in its container, and the
                // block's place in the layer's list.)
                if let Some(layer) = membership {
                    restore.push(Self::AssignNodeToLayer {
                        node_id: node.id,
                        layer,
                    });
                    restore.push(Self::SetNodeParent {
                        id: node.id,
                        parent: previous_parent,
                        index: previous_index,
                    });
                }
                restore.extend(
                    withdrawn
                        .into_iter()
                        .map(|constraint| Self::AddConstraint { constraint }),
                );
                // An operation whose input just vanished has no shape to read:
                // it is withdrawn with the node and restored with it, exactly
                // like the constraints above. Its *other* inputs are untouched.
                let orphaned = doc
                    .operations
                    .affected_by(&[*id])
                    .into_iter()
                    .filter_map(|op_id| doc.operations.remove(op_id));
                restore.extend(orphaned.map(|op| Self::ApplyOperation {
                    id: op.id,
                    kind: op.kind,
                    inputs: op.inputs,
                    // The whole record, paint included: a Smart Fill withdrawn
                    // with its boundary comes back wearing what it wore.
                    style: Some(op.style),
                    name: Some(op.name),
                }));
                // The same discipline for the procedural graph: a `Source` node
                // whose subject just vanished has nothing to read, so it is
                // withdrawn with the node (and restored with it) — including
                // the wires other nodes had into it. A *chain* node that merely
                // loses its upstream wire is not withdrawn: it keeps its
                // record and reports a typed diagnostic until it is rewired.
                let orphaned_procedural = doc.procedural.sources_referencing(*id);
                for orphan in orphaned_procedural {
                    let Some(removed) = doc.procedural.remove(orphan) else {
                        continue;
                    };
                    restore.push(Self::AddProceduralNode { node: removed.node });
                    restore.extend(removed.incoming.into_iter().map(|wire| {
                        Self::ConnectProcedural {
                            node_id: wire.node_id,
                            port: wire.port,
                            from: wire.from,
                        }
                    }));
                }
                Ok(Self::batch(restore))
            }
            Self::SetParameter {
                node_id,
                property,
                value,
            } => {
                // Before the mutation, like every other gate: a refused command
                // leaves the document byte-identical. Only a *geometry* slot can
                // close a loop through a shape — see
                // `Document::property_feeds_geometry`.
                if Node::property_feeds_geometry(property) {
                    if let Some(reference) = crate::procedural::param_procedural_ref(value) {
                        validate_procedural_cycle(doc, *node_id, reference)?;
                    }
                } else if let Some(reference) = crate::procedural::param_procedural_ref(value) {
                    // Paint still has to *exist*: a port nobody publishes is a
                    // dangling reference, which the port-type gate below catches
                    // for wires and `ResolveError::ProceduralPortUnavailable`
                    // catches at read time. Nothing to add here.
                    let _ = reference;
                }
                let node = doc.get_node_mut(*node_id)?;
                let old = node.set_param(property, value.clone())?;
                Ok(Self::SetParameter {
                    node_id: *node_id,
                    property: property.clone(),
                    value: old,
                })
            }
            Self::SetPath {
                id,
                start,
                segments,
            } => {
                // Read the previous geometry first: the inverse is the same
                // command carrying it, which is what makes undo of a drawing
                // gesture exact (see the variant's own docs).
                let node = doc.get_node_mut(*id)?;
                let NodeKind::Path {
                    start: old_start,
                    segments: old_segments,
                } = &mut node.kind
                else {
                    return Err(VectraError::UnknownProperty {
                        node_id: *id,
                        node_kind: node.kind.tag().to_string(),
                        property: "segments".to_string(),
                    });
                };
                let previous_start = std::mem::replace(old_start, start.clone());
                let previous_segments = std::mem::replace(old_segments, segments.clone());
                Ok(Self::SetPath {
                    id: *id,
                    start: previous_start,
                    segments: previous_segments,
                })
            }
            Self::SetVariable { name, value } => {
                let old = doc.set_variable(name.clone(), *value)?;
                match old {
                    Some(v) => Ok(Self::SetVariable {
                        name: name.clone(),
                        value: v,
                    }),
                    None => Ok(Self::RemoveVariable { name: name.clone() }),
                }
            }
            Self::RemoveVariable { name } => {
                let old = doc.remove_variable(name)?;
                Ok(Self::SetVariable {
                    name: name.clone(),
                    value: old,
                })
            }
            Self::DefineExpression { id, source } => {
                let old = doc.define_expression(*id, source.clone());
                match old {
                    Some(previous) => Ok(Self::DefineExpression {
                        id: *id,
                        source: previous,
                    }),
                    None => Ok(Self::RemoveExpression { id: *id }),
                }
            }
            Self::RemoveExpression { id } => {
                let old = doc.remove_expression(*id)?;
                Ok(Self::DefineExpression {
                    id: *id,
                    source: old,
                })
            }
            Self::AddConstraint { constraint } => {
                doc.constraints.insert(constraint.clone())?;
                Ok(Self::RemoveConstraint { id: constraint.id })
            }
            Self::RemoveConstraint { id } => {
                let removed = doc
                    .constraints
                    .remove(*id)
                    .ok_or(VectraError::ConstraintNotFound(*id))?;
                Ok(Self::AddConstraint {
                    constraint: removed,
                })
            }
            Self::SetConstraintEnabled { id, enabled } => {
                let constraint = doc
                    .constraints
                    .get_mut(*id)
                    .ok_or(VectraError::ConstraintNotFound(*id))?;
                let previous = constraint.enabled;
                constraint.enabled = *enabled;
                Ok(Self::SetConstraintEnabled {
                    id: *id,
                    enabled: previous,
                })
            }
            Self::BeginDrag { node_id } | Self::EndDrag { node_id } => {
                // Session bookkeeping, not a document mutation: validating here
                // keeps the command meaningful even if it is ever applied
                // outside the engine's drag path.
                let node = doc.get_node(*node_id)?;
                if node.position_slots().is_none() {
                    return Err(VectraError::not_draggable(*node_id));
                }
                Ok(Self::batch_commands(Vec::new()))
            }
            Self::UpdateDrag { node_id, x, y } => {
                let node = doc.get_node_mut(*node_id)?;
                let Some((xs, ys)) = node.position_slots() else {
                    return Err(VectraError::not_draggable(*node_id));
                };
                let old_x = node.set_param(xs, ParamValue::Float(Parameter::Literal(*x)))?;
                let old_y = node.set_param(ys, ParamValue::Float(Parameter::Literal(*y)))?;
                // Reversed: undoing a batch unwinds it last-in-first-out.
                Ok(Self::batch_commands(vec![
                    Self::SetParameter {
                        node_id: *node_id,
                        property: ys.to_string(),
                        value: old_y,
                    },
                    Self::SetParameter {
                        node_id: *node_id,
                        property: xs.to_string(),
                        value: old_x,
                    },
                ]))
            }
            Self::BindMotion {
                node_id,
                property,
                binding,
            } => {
                validate_binding(doc, *node_id, property, binding)?;
                let node = doc.get_node_mut(*node_id)?;
                let old = node.set_param(
                    property,
                    ParamValue::Float(Parameter::Animated(binding.clone())),
                )?;
                Ok(Self::SetParameter {
                    node_id: *node_id,
                    property: property.clone(),
                    value: old,
                })
            }
            Self::SetMotionTrack { track } => {
                let previous = doc.motion.insert(track.clone())?;
                Ok(match previous {
                    Some(previous) => Self::SetMotionTrack { track: previous },
                    None => Self::RemoveMotionTrack {
                        track_id: track.id.clone(),
                    },
                })
            }
            Self::RemoveMotionTrack { track_id } => {
                let removed = doc.motion.remove(track_id).ok_or_else(|| {
                    VectraError::command(format!("motion track {track_id} does not exist"))
                })?;
                Ok(Self::SetMotionTrack { track: removed })
            }
            Self::ApplyOperation {
                id,
                kind,
                inputs,
                style,
                name,
            } => {
                OperationNode::validate(*id, kind, inputs)?;
                // An operation's parameters are slots too: a mirror plane may
                // read a port, and the operation's own output is a shape a
                // `source` node can read — the same disguised loop.
                let mut cycle: Option<VectraError> = None;
                kind.for_each_float_param(|param| {
                    if let Parameter::Procedural(reference) = param {
                        if let Err(error) = validate_procedural_cycle(doc, *id, reference) {
                            cycle.get_or_insert(error);
                        }
                    }
                });
                if let Some(error) = cycle {
                    return Err(error);
                }
                for input in inputs {
                    // The sources must exist — a virtual shape over a missing
                    // node is not a shape. A **Smart Fill names itself** first
                    // among its inputs: that self-reference is how a write that
                    // dirties the fill (`SetAppearances`, a new seed) reaches
                    // the fill's own evaluation, and it is the one id here that
                    // is not an authored node. Reading *another* operation is
                    // still Phase 2 (see the module note in `operation.rs`).
                    if input == id {
                        continue;
                    }
                    doc.get_node(*input)?;
                }
                if doc.operations.contains(*id) {
                    return Err(VectraError::command(format!(
                        "operation {id} already exists"
                    )));
                }
                let mut node = OperationNode::new(*id, kind.clone(), inputs.clone());
                // The full record, when the caller has one: undo of
                // `RemoveOperation` re-applies this command, and a Smart Fill's
                // paint is part of what was removed.
                if let Some(style) = style {
                    node.style = style.clone();
                }
                if let Some(name) = name {
                    node.name = name.clone();
                }
                doc.operations.insert(node);
                Ok(Self::RemoveOperation { id: *id })
            }
            Self::RemoveOperation { id } => {
                let removed = doc
                    .operations
                    .remove(*id)
                    .ok_or(VectraError::OperationNotFound(*id))?;
                // Exact inverse: the whole record — id, kind, inputs, paint and
                // name. The paint is what Task 12.0 added; the inputs are what
                // RULE 1 promised was never touched.
                Ok(Self::ApplyOperation {
                    id: removed.id,
                    kind: removed.kind,
                    inputs: removed.inputs,
                    style: Some(removed.style),
                    name: Some(removed.name),
                })
            }
            Self::SetOperationEnabled { id, enabled } => {
                let node = doc
                    .operations
                    .nodes
                    .get_mut(id)
                    .ok_or(VectraError::OperationNotFound(*id))?;
                let previous = node.enabled;
                node.enabled = *enabled;
                Ok(Self::SetOperationEnabled {
                    id: *id,
                    enabled: previous,
                })
            }
            Self::CreateSmartFill {
                id,
                boundaries,
                seed,
                fill,
                name,
            } => {
                if boundaries.is_empty() {
                    return Err(VectraError::command(
                        "a smart fill needs at least one boundary path".to_string(),
                    ));
                }
                for (index, boundary) in boundaries.iter().enumerate() {
                    if boundaries[..index].contains(boundary) {
                        return Err(VectraError::command(format!(
                            "smart fill {id} lists boundary {boundary} twice"
                        )));
                    }
                    // The boundaries must exist — a region over a missing shape
                    // is not a region. (A *hidden* boundary is fine: hiding is a
                    // presentation flag, not a deletion, and a designer may well
                    // fill between two shapes they are not showing.)
                    doc.get_node(*boundary)?;
                }
                if doc.operations.contains(*id) {
                    return Err(VectraError::command(format!(
                        "operation {id} already exists"
                    )));
                }
                if !seed.0.is_finite() || !seed.1.is_finite() {
                    return Err(VectraError::command(format!(
                        "smart fill {id} has a non-finite seed ({}, {})",
                        seed.0, seed.1
                    )));
                }
                let mut node = OperationNode::smart_fill(*id, boundaries.clone(), *seed);
                if let Some(name) = name {
                    node.name = name.clone();
                }
                if let Some(fill) = fill {
                    // **RULE 4.** The drop *is* the fill: the bottom row of the
                    // default stack is replaced rather than appended to, so a
                    // dropped colour produces a fill with exactly one solid row
                    // of that colour — and every other row (a stroke, a blend)
                    // still comes from the default stack.
                    let mut style = OperationNode::default_style();
                    style.appearances = vec![AppearanceLayer::fill(*fill)];
                    node.style = style;
                }
                doc.operations.insert(node);
                Ok(Self::RemoveOperation { id: *id })
            }
            Self::BreakPath { node_id, pieces } => {
                // 1. Validate everything before anything is written: the source
                //    must be a path, the pieces must be real and free ids.
                let (style, layer, source_index) = {
                    let node = doc.get_node(*node_id)?;
                    if !matches!(node.kind, NodeKind::Path { .. }) {
                        return Err(VectraError::command(format!(
                            "only a Path can be broken at its intersections; {} is a {}",
                            node.name,
                            node.kind.tag()
                        )));
                    }
                    if pieces.is_empty() {
                        return Err(VectraError::command(
                            "there is nothing to break: no spans were given".to_string(),
                        ));
                    }
                    (
                        node.style.clone(),
                        doc.layers.layer_of(*node_id).map(|(layer, _)| layer.id),
                        doc.order_index(*node_id).unwrap_or(0),
                    )
                };
                for (index, piece) in pieces.iter().enumerate() {
                    if doc.nodes.contains_key(&piece.id) || doc.operations.contains(piece.id) {
                        return Err(VectraError::NodeAlreadyExists(piece.id));
                    }
                    if pieces[..index].iter().any(|other| other.id == piece.id) {
                        return Err(VectraError::command(format!(
                            "break of {node_id} lists piece {} twice",
                            piece.id
                        )));
                    }
                }

                // 2. The pieces are ordinary `Path` nodes wearing the source's
                //    paint, so a break does not restyle the artwork.
                for (offset, piece) in pieces.iter().enumerate() {
                    let mut node = Node::new(
                        piece.id,
                        piece.name.clone(),
                        NodeKind::Path {
                            start: piece.start.clone(),
                            segments: piece.segments.clone(),
                        },
                    );
                    node.style = style.clone();
                    doc.insert_node(node, Some(source_index + 1 + offset))?;
                }

                // 3. **Non-destructive**: the source is hidden, not deleted —
                //    its control points are still there, which is what a designer
                //    returning to the curve wants, and what makes the inverse a
                //    one-line restore rather than a saved copy.
                doc.get_node_mut(*node_id)?.visible = false;

                // 4. Place: the pieces take the source's own place in its layer,
                //    directly above it (the source is hidden, so this is purely
                //    so the new geometry draws where the old geometry drew).
                if let Some(layer_id) = layer {
                    let block: Vec<NodeId> = pieces.iter().map(|piece| piece.id).collect();
                    doc.assign_block_above(&block, layer_id, *node_id);
                }

                // The inverse, as one entry: take the pieces out, show the
                // source again. Reversed so undo unwinds last-in-first-out.
                let mut commands: Vec<Command> = pieces
                    .iter()
                    .rev()
                    .map(|piece| Self::DeleteNode { id: piece.id })
                    .collect();
                commands.push(Self::SetNodeVisible {
                    id: *node_id,
                    visible: true,
                });
                Ok(Self::Batch { commands })
            }
            Self::AddProceduralNode { node } => {
                if doc.procedural.contains(node.id) {
                    return Err(VectraError::command(format!(
                        "procedural node {} already exists",
                        node.id
                    )));
                }
                // A record that arrives without a name is named here, from its
                // kind — the same name `ProceduralNode::new` would have given it,
                // so the engine's own default and a remote control's omission
                // produce the same document.
                let mut node = node.clone();
                if node.name.is_empty() {
                    node.name = node.kind.describe();
                }
                node.validate(&doc.procedural)?;
                doc.procedural.insert(node.clone());
                Ok(Self::RemoveProceduralNode { id: node.id })
            }
            Self::RemoveProceduralNode { id } => {
                let removed = doc.procedural.remove(*id).ok_or_else(|| {
                    VectraError::command(format!("procedural node {id} is not registered"))
                })?;
                // Exact inverse: the record, then every wire other nodes had
                // *into* it (the wires out of it travel inside the record).
                let mut restore = vec![Self::AddProceduralNode { node: removed.node }];
                restore.extend(
                    removed
                        .incoming
                        .into_iter()
                        .map(|wire| Self::ConnectProcedural {
                            node_id: wire.node_id,
                            port: wire.port,
                            from: wire.from,
                        }),
                );
                Ok(Self::batch(restore))
            }
            Self::ConnectProcedural {
                node_id,
                port,
                from,
            } => {
                let node = doc
                    .procedural
                    .get(*node_id)
                    .ok_or_else(|| unknown_procedural(*node_id))?;
                let mut candidate = node.clone();
                candidate.wires.insert(port.clone(), from.clone());
                candidate.validate(&doc.procedural)?;
                let previous = doc
                    .procedural
                    .get_mut(*node_id)
                    .expect("checked above")
                    .wires
                    .insert(port.clone(), from.clone());
                Ok(match previous {
                    Some(previous) => Self::ConnectProcedural {
                        node_id: *node_id,
                        port: port.clone(),
                        from: previous,
                    },
                    None => Self::DisconnectProcedural {
                        node_id: *node_id,
                        port: port.clone(),
                    },
                })
            }
            Self::DisconnectProcedural { node_id, port } => {
                let node = doc
                    .procedural
                    .get_mut(*node_id)
                    .ok_or_else(|| unknown_procedural(*node_id))?;
                let removed = node.wires.remove(port).ok_or_else(|| {
                    VectraError::command(format!(
                        "procedural node {node_id} has no wire on port '{port}'"
                    ))
                })?;
                Ok(Self::ConnectProcedural {
                    node_id: *node_id,
                    port: port.clone(),
                    from: removed,
                })
            }
            Self::SetProceduralOperand {
                node_id,
                port,
                value,
            } => {
                let node = doc
                    .procedural
                    .get(*node_id)
                    .ok_or_else(|| unknown_procedural(*node_id))?;
                let mut candidate = node.clone();
                candidate.operands.insert(port.clone(), value.clone());
                candidate.validate(&doc.procedural)?;
                // The inverse writes the *effective* previous value: the stored
                // operand, or the kind's default when the record carried none
                // (which is what evaluation would have used).
                let previous = node.operand(port).unwrap_or_else(|| value.clone());
                doc.procedural
                    .get_mut(*node_id)
                    .expect("checked above")
                    .operands
                    .insert(port.clone(), value.clone());
                Ok(Self::SetProceduralOperand {
                    node_id: *node_id,
                    port: port.clone(),
                    value: previous,
                })
            }
            Self::SetProceduralEnabled { id, enabled } => {
                let node = doc
                    .procedural
                    .get_mut(*id)
                    .ok_or_else(|| unknown_procedural(*id))?;
                let previous = node.enabled;
                node.enabled = *enabled;
                Ok(Self::SetProceduralEnabled {
                    id: *id,
                    enabled: previous,
                })
            }
            // ── appearance (Task 10.2 RULE 3) ────────────────────────────
            Self::SetAppearances {
                node_id,
                appearances,
            } => {
                // An **operation result is a node**, and its paint stack is what
                // Task 12.0 RULE 2 means by "a Smart Fill has its own
                // `Appearance`": the registry entry carries a `StyleProperties`
                // exactly like an authored node, so the same command edits both.
                // (Before Task 12.0 this arm only knew about `doc.nodes`, which
                // would have refused a paint change on a boolean's own id.)
                let previous = if let Some(node) = doc.nodes.get_mut(node_id) {
                    std::mem::replace(&mut node.style.appearances, appearances.clone())
                } else if let Some(operation) = doc.operations.nodes.get_mut(node_id) {
                    std::mem::replace(&mut operation.style.appearances, appearances.clone())
                } else {
                    return Err(VectraError::NodeNotFound(*node_id));
                };
                Ok(Self::SetAppearances {
                    node_id: *node_id,
                    appearances: previous,
                })
            }
            // ── text (Task 11.0 RULES 1–3) ────────────────────────────────
            //
            // Four small commands rather than one setter with a `name`, because
            // each inverse is then exactly what it replaced: a string, a string,
            // an enum. A generic setter would have to invent a value type for
            // three different things, and undo — which is the reason these exist
            // at all — would have to guess which one it held.
            Self::SetText { node_id, text } => {
                let node = text_node_mut(doc, *node_id, "text")?;
                let NodeKind::Text {
                    text: authored_text,
                    ..
                } = &mut node.kind
                else {
                    unreachable!("text_node_mut checked the kind")
                };
                let previous = std::mem::replace(authored_text, text.clone());
                Ok(Self::SetText {
                    node_id: *node_id,
                    text: previous,
                })
            }
            Self::SetFontFamily { node_id, family } => {
                let node = text_node_mut(doc, *node_id, "font_family")?;
                let NodeKind::Text {
                    font_family: authored_family,
                    ..
                } = &mut node.kind
                else {
                    unreachable!("text_node_mut checked the kind")
                };
                let previous = std::mem::replace(authored_family, family.clone());
                Ok(Self::SetFontFamily {
                    node_id: *node_id,
                    family: previous,
                })
            }
            Self::SetTextAlignment { node_id, alignment } => {
                let node = text_node_mut(doc, *node_id, "alignment")?;
                let NodeKind::Text {
                    alignment: authored_alignment,
                    ..
                } = &mut node.kind
                else {
                    unreachable!("text_node_mut checked the kind")
                };
                let previous = std::mem::replace(authored_alignment, *alignment);
                Ok(Self::SetTextAlignment {
                    node_id: *node_id,
                    alignment: previous,
                })
            }
            Self::BindTextToPath {
                node_id,
                path,
                offset,
            } => {
                // Validated **before** anything is stored: a binding to a
                // missing node or a non-path kind is refused whole, so the
                // document never holds a reference that evaluation cannot honor.
                let source = doc.get_node(*path)?;
                if !Document::is_text_path_source(&source.kind) {
                    return Err(VectraError::command(format!(
                        "cannot bind text {node_id} to {path}: {} is not a path, arc or circle",
                        source.kind.tag()
                    )));
                }
                let binding = crate::document::TextPathBinding {
                    node: *path,
                    offset: offset.clone(),
                };
                let node = text_node_mut(doc, *node_id, "path")?;
                let NodeKind::Text { on_path, .. } = &mut node.kind else {
                    unreachable!("text_node_mut checked the kind")
                };
                let previous = on_path.replace(binding);
                Ok(match previous {
                    Some(previous) => Self::BindTextToPath {
                        node_id: *node_id,
                        path: previous.node,
                        offset: previous.offset,
                    },
                    None => Self::UnbindTextFromPath { node_id: *node_id },
                })
            }
            Self::UnbindTextFromPath { node_id } => {
                let node = text_node_mut(doc, *node_id, "path")?;
                let NodeKind::Text { on_path, .. } = &mut node.kind else {
                    unreachable!("text_node_mut checked the kind")
                };
                let previous = on_path.take().ok_or_else(|| {
                    VectraError::command(format!(
                        "text {node_id} is not bound to a path, so there is nothing to unbind"
                    ))
                })?;
                Ok(Self::BindTextToPath {
                    node_id: *node_id,
                    path: previous.node,
                    offset: previous.offset,
                })
            }
            Self::OutlineText {
                node_id,
                group_id,
                name,
                paths,
            } => {
                // The guarantees this command makes, checked before the first
                // mutation: a real text node, a non-empty plan, and an id that
                // is not already taken (the group, the letterforms and the
                // source are four different nodes — a caller that reuses an id
                // is refused rather than half-applied).
                //
                // *Every* check runs before the first write, and the id checks
                // run before the source is borrowed: a refusal must leave the
                // document exactly as it was, and a half-applied outline would
                // be neither the old document nor the new one.
                if !doc.nodes.contains_key(node_id) {
                    return Err(VectraError::NodeNotFound(*node_id));
                }
                if paths.is_empty() {
                    return Err(VectraError::command(format!(
                        "cannot outline text {node_id}: the run produced no letterforms"
                    )));
                }
                if doc.nodes.contains_key(group_id) {
                    return Err(VectraError::NodeAlreadyExists(*group_id));
                }
                if paths.iter().any(|path| doc.nodes.contains_key(&path.id)) {
                    let taken = paths
                        .iter()
                        .find(|path| doc.nodes.contains_key(&path.id))
                        .map(|path| path.id)
                        .expect("just found");
                    return Err(VectraError::NodeAlreadyExists(taken));
                }
                let style = text_node_mut(doc, *node_id, "outline")?.style.clone();
                let layer = doc.layers.layer_of(*node_id).map(|(layer, _)| layer.id);
                let text_index = doc.order_index(*node_id);

                // 1. The group, placed directly above the text it replaces, so
                //    an outline appears where the type was rather than at the
                //    top of the document.
                let children: Vec<NodeId> = paths.iter().map(|path| path.id).collect();
                let group_name = name.clone().unwrap_or_else(|| "Outlined text".to_string());
                doc.insert_node(
                    Node::new(
                        *group_id,
                        group_name,
                        NodeKind::Group {
                            children: children.clone(),
                        },
                    ),
                    text_index.map(|index| index + 1),
                )?;

                // 2. The letterforms: ordinary `Path` nodes carrying the text
                //    node's own paint, so the outline looks exactly like the type
                //    it replaces — a red gradient-filled word outlines to red
                //    gradient-filled letters, with no second styling step.
                for (index, path) in paths.iter().enumerate() {
                    let mut node = Node::new(
                        path.id,
                        path.name.clone(),
                        NodeKind::Path {
                            start: path.start.clone(),
                            segments: path.segments.clone(),
                        },
                    );
                    node.style = style.clone();
                    doc.insert_node(node, None)?;
                    doc.set_parent(path.id, Some(*group_id), index)?;
                }

                // 3. **Non-destructive**: the text node is hidden, not deleted.
                //    Its string, its slots and its bindings are all still there,
                //    which is what makes the inverse exact and what lets a
                //    designer come back to the type after a detour through
                //    boolean operations.
                doc.get_node_mut(*node_id)?.visible = false;

                // 4. Membership and **place**: the group and its letterforms
                //    land on the layer the type was on — so outlining an object
                //    on layer 3 does not move it to whichever layer happens to
                //    be active — and *directly above the type*, not at the top
                //    of the layer: the outline takes the type's place in the
                //    stack, and artwork that was above the type stays above it.
                if let Some(layer_id) = layer {
                    let block = doc.node_block(*group_id);
                    doc.assign_block_above(&block, layer_id, *node_id);
                }

                // The inverse, as one entry: take the letterforms out, take the
                // group out, show the type again. The order matters — the group
                // is removed *last*, so undo recreates it *first* and every
                // letterform can be re-parented into a group that exists again.
                let mut commands: Vec<Command> = children
                    .iter()
                    .rev()
                    .map(|id| Self::DeleteNode { id: *id })
                    .collect();
                commands.push(Self::DeleteNode { id: *group_id });
                commands.push(Self::SetNodeVisible {
                    id: *node_id,
                    visible: true,
                });
                Ok(Self::Batch { commands })
            }
            // ── presentation flags (Task 10.2 RULE 4) ────────────────────
            Self::SetNodeVisible { id, visible } => {
                let node = doc
                    .nodes
                    .get_mut(id)
                    .ok_or(VectraError::NodeNotFound(*id))?;
                let previous = node.visible;
                node.visible = *visible;
                Ok(Self::SetNodeVisible {
                    id: *id,
                    visible: previous,
                })
            }
            Self::SetNodeLocked { id, locked } => {
                let node = doc
                    .nodes
                    .get_mut(id)
                    .ok_or(VectraError::NodeNotFound(*id))?;
                let previous = node.locked;
                node.locked = *locked;
                Ok(Self::SetNodeLocked {
                    id: *id,
                    locked: previous,
                })
            }
            Self::RenameNode { id, name } => {
                let node = doc
                    .nodes
                    .get_mut(id)
                    .ok_or(VectraError::NodeNotFound(*id))?;
                let previous = std::mem::replace(&mut node.name, name.clone());
                Ok(Self::RenameNode {
                    id: *id,
                    name: previous,
                })
            }
            // ── layers (Task 10.2 RULE 1) ─────────────────────────────────
            Self::CreateLayer {
                id,
                name,
                index,
                artboard,
            } => {
                // The layer joins the active board's stack at the top (or at the
                // requested index): the record keeps its own identity, so a later
                // move between boards is a re-listing, never a re-creation.
                let record = LayerRecord::new(*id, name.clone());
                let board = artboard.or_else(|| doc.artboards.active_id());
                if !doc.layers.insert(record, *index) {
                    return Err(VectraError::LayerAlreadyExists(*id));
                }
                if let Some(board) = board {
                    doc.attach_layer_to_artboard(*id, Some(board));
                    if let Some(board_record) = doc.artboards.get_mut(&board) {
                        if let Some(at) = *index {
                            let last = board_record.layers.len().saturating_sub(1);
                            let layer = board_record.layers.remove(last);
                            board_record.layers.insert(at.min(last), layer);
                        }
                    }
                }
                doc.active_layer = Some(*id);
                doc.resync_order_from_layers();
                Ok(Self::DeleteLayer { id: *id })
            }
            Self::DeleteLayer { id } => {
                let record = doc
                    .layers
                    .remove(id)
                    .ok_or(VectraError::LayerNotFound(*id))?;
                for board in &mut doc.artboards.boards {
                    board.layers.retain(|layer| layer != id);
                }
                if doc.active_layer == Some(*id) {
                    doc.active_layer = None;
                }
                // The nodes are *not* deleted: they become unassigned and keep
                // drawing (see the command's docs).
                doc.resync_order_from_layers();
                Ok(Self::CreateLayer {
                    id: record.id,
                    name: record.name,
                    index: None,
                    artboard: None,
                })
            }
            Self::RenameLayer { id, name } => {
                let layer = doc
                    .layers
                    .get_mut(id)
                    .ok_or(VectraError::LayerNotFound(*id))?;
                let previous = std::mem::replace(&mut layer.name, name.clone());
                Ok(Self::RenameLayer {
                    id: *id,
                    name: previous,
                })
            }
            Self::SetLayerVisible { id, visible } => {
                let layer = doc
                    .layers
                    .get_mut(id)
                    .ok_or(VectraError::LayerNotFound(*id))?;
                let previous = layer.visible;
                layer.visible = *visible;
                // The layer's nodes change their *effective* visibility; the
                // renderer re-reads the flags and skips their draws. Nothing
                // reaches the evaluator (RULE 4).
                Ok(Self::SetLayerVisible {
                    id: *id,
                    visible: previous,
                })
            }
            Self::SetLayerLocked { id, locked } => {
                let layer = doc
                    .layers
                    .get_mut(id)
                    .ok_or(VectraError::LayerNotFound(*id))?;
                let previous = layer.locked;
                layer.locked = *locked;
                Ok(Self::SetLayerLocked {
                    id: *id,
                    locked: previous,
                })
            }
            Self::SetLayerAlphaLocked { id, alpha_locked } => {
                let layer = doc
                    .layers
                    .get_mut(id)
                    .ok_or(VectraError::LayerNotFound(*id))?;
                let previous = layer.alpha_locked;
                layer.alpha_locked = *alpha_locked;
                Ok(Self::SetLayerAlphaLocked {
                    id: *id,
                    alpha_locked: previous,
                })
            }
            Self::SetLayerClippingMask { id, clipping_mask } => {
                let layer = doc
                    .layers
                    .get_mut(id)
                    .ok_or(VectraError::LayerNotFound(*id))?;
                let previous = layer.clipping_mask;
                layer.clipping_mask = *clipping_mask;
                Ok(Self::SetLayerClippingMask {
                    id: *id,
                    clipping_mask: previous,
                })
            }
            Self::ReorderLayer { id, index } => {
                // One call, because a layer's position lives in two places: the
                // registry the panel lists and the artboard stack the draw order
                // is flattened from (see `Document::reorder_layer`).
                let previous = doc
                    .reorder_layer(id, *index)
                    .ok_or(VectraError::LayerNotFound(*id))?;
                Ok(Self::ReorderLayer {
                    id: *id,
                    index: previous,
                })
            }
            Self::AssignNodeToLayer { node_id, layer } => {
                if !doc.nodes.contains_key(node_id) {
                    return Err(VectraError::NodeNotFound(*node_id));
                }
                if doc.layers.get(layer).is_none() {
                    return Err(VectraError::LayerNotFound(*layer));
                }
                let previous = doc.layers.layer_of(*node_id).map(|(layer, _)| layer.id);
                doc.layers.detach(*node_id);
                if let Some(target) = doc.layers.get_mut(layer) {
                    target.children.push(*node_id);
                }
                doc.resync_order_from_layers();
                // The inverse returns the node to where it *was*: the same layer
                // (it is a real move within the container) or, when it had no
                // layer at all, "no layer" — which is its own command, because
                // "assign to nothing" is not something an assign can say.
                Ok(match previous {
                    Some(previous) => Self::AssignNodeToLayer {
                        node_id: *node_id,
                        layer: previous,
                    },
                    None => Self::DetachNodeFromLayers { node_id: *node_id },
                })
            }
            Self::DetachNodeFromLayers { node_id } => {
                let previous = doc
                    .layers
                    .detach(*node_id)
                    .ok_or(VectraError::NodeNotInAnyLayer(*node_id))?;
                doc.resync_order_from_layers();
                Ok(Self::AssignNodeToLayer {
                    node_id: *node_id,
                    layer: previous,
                })
            }
            Self::SetNodeParent { id, parent, index } => {
                // Which layer holds the node **before** the move. A move can
                // change that on its own: a layer lists its blocks, so a subtree
                // lives in exactly one layer, and moving a node into a group that
                // belongs to another layer carries the node into *that* layer.
                // The inverse has to put that back too — restoring parent and
                // position alone leaves the artwork in the group's layer, which
                // is the fidelity break the tree laws found on a group made from
                // a selection that spanned two layers.
                let previous_layer = doc.layers.layer_of(*id).map(|(record, _)| record.id);
                match doc.set_parent(*id, *parent, *index)? {
                    // The exact inverse is the same move, back where it came
                    // from: parent, then position — one entry, because one
                    // gesture made it. When the move crossed a layer, that is one
                    // more fact, and it rides in the same entry.
                    Some((previous_parent, previous_index)) => {
                        let restore = Self::SetNodeParent {
                            id: *id,
                            parent: previous_parent,
                            index: previous_index,
                        };
                        let moved_layer = doc.layers.layer_of(*id).map(|(record, _)| record.id);
                        Ok(
                            match previous_layer.filter(|layer| Some(*layer) != moved_layer) {
                                Some(layer) => Self::batch(vec![
                                    Self::AssignNodeToLayer {
                                        node_id: *id,
                                        layer,
                                    },
                                    restore,
                                ]),
                                None => restore,
                            },
                        )
                    }
                    // **Already there**: the move was satisfied before it ran, so
                    // it changed nothing and its inverse is itself. Not an error:
                    // a redo that lands the document where it already is must be as
                    // quiet as the drag that produced it (`NodeNotFound` here was
                    // a real defect, found by the round-trip law).
                    None => Ok(Self::SetNodeParent {
                        id: *id,
                        parent: *parent,
                        index: *index,
                    }),
                }
            }
            // ── artboards (Task 10.2 RULE 2) ─────────────────────────────
            Self::CreateArtboard {
                id,
                name,
                x,
                y,
                width,
                height,
                background,
            } => {
                let mut record = ArtboardRecord::new(*id, name.clone(), *x, *y, *width, *height);
                record.background = *background;
                if !doc.artboards.insert(record, None) {
                    return Err(VectraError::ArtboardAlreadyExists(*id));
                }
                // A new board is the one you are **on** (RULE 2): "+ board" is a
                // move onto the new paper, which is what makes the layer the
                // designer creates next — and the shape after that — land on it
                // rather than on whatever board was active before. Undo falls
                // back through the registry's own "first board" rule
                // ([`ArtboardRegistry::remove`]), so the flag is never dangling.
                doc.artboards.active = Some(*id);
                Ok(Self::DeleteArtboard { id: *id })
            }
            Self::DeleteArtboard { id } => {
                let record = doc
                    .artboards
                    .remove(id)
                    .ok_or(VectraError::ArtboardNotFound(*id))?;
                doc.resync_order_from_layers();
                Ok(Self::CreateArtboard {
                    id: record.id,
                    name: record.name,
                    x: record.x,
                    y: record.y,
                    width: record.width,
                    height: record.height,
                    background: record.background,
                })
            }
            Self::RenameArtboard { id, name } => {
                let board = doc
                    .artboards
                    .get_mut(id)
                    .ok_or(VectraError::ArtboardNotFound(*id))?;
                let previous = std::mem::replace(&mut board.name, name.clone());
                Ok(Self::RenameArtboard {
                    id: *id,
                    name: previous,
                })
            }
            Self::SetArtboardBounds {
                id,
                x,
                y,
                width,
                height,
            } => {
                let board = doc
                    .artboards
                    .get_mut(id)
                    .ok_or(VectraError::ArtboardNotFound(*id))?;
                let previous = (board.x, board.y, board.width, board.height);
                board.x = *x;
                board.y = *y;
                board.width = *width;
                board.height = *height;
                Ok(Self::SetArtboardBounds {
                    id: *id,
                    x: previous.0,
                    y: previous.1,
                    width: previous.2,
                    height: previous.3,
                })
            }
            Self::SetArtboardBackground { id, background } => {
                let board = doc
                    .artboards
                    .get_mut(id)
                    .ok_or(VectraError::ArtboardNotFound(*id))?;
                let previous = std::mem::replace(&mut board.background, *background);
                Ok(Self::SetArtboardBackground {
                    id: *id,
                    background: previous,
                })
            }
            Self::SetActiveArtboard { id } => {
                if doc.artboards.get(id).is_none() {
                    return Err(VectraError::ArtboardNotFound(*id));
                }
                let previous = doc.artboards.active;
                doc.artboards.active = Some(*id);
                // The board the designer just switched to comes forward, so new
                // artwork lands on top of the artwork of the previous board.
                doc.raise_artboard(id);
                let restore = previous
                    .filter(|previous| doc.artboards.get(previous).is_some())
                    .unwrap_or(*id);
                Ok(Self::SetActiveArtboard { id: restore })
            }
            Self::SetActiveLayer { id } => {
                if doc.layers.get(id).is_none() {
                    return Err(VectraError::LayerNotFound(*id));
                }
                let previous = doc.active_layer;
                doc.active_layer = Some(*id);
                Ok(Self::SetActiveLayer {
                    id: previous.unwrap_or(*id),
                })
            }
            Self::DuplicateNode {
                id,
                source,
                name,
                index,
            } => {
                if doc.nodes.contains_key(id) {
                    return Err(VectraError::NodeAlreadyExists(*id));
                }
                let mut copy = doc
                    .nodes
                    .get(source)
                    .ok_or(VectraError::NodeNotFound(*source))?
                    .clone();
                copy.id = *id;
                copy.name = name
                    .clone()
                    .unwrap_or_else(|| format!("{} copy", copy.name));
                doc.insert_node(copy, *index)?;
                Ok(Self::DeleteNode { id: *id })
            }
            // ── Smart Components (Task 10.6 RULE 1) ──────────────────────
            Self::CreateComponent {
                id,
                name,
                members,
                props,
            } => {
                use crate::component::{
                    bind_plan, color_default, infer_props, seed_operands, ComponentProp, PropType,
                };

                if doc.procedural.contains(*id) {
                    return Err(VectraError::command(format!(
                        "procedural node {id} already exists"
                    )));
                }
                if doc.nodes.contains_key(id) {
                    return Err(VectraError::command(format!("node {id} already exists")));
                }
                for member in members {
                    if !doc.nodes.contains_key(member) {
                        return Err(VectraError::NodeNotFound(*member));
                    }
                }
                if members.is_empty() {
                    return Err(VectraError::command(
                        "a component needs at least one member node",
                    ));
                }

                // Props: the caller's list, or the inferred four (RULE 1).
                let props: Vec<ComponentProp> = if props.is_empty() {
                    infer_props(doc, members)
                } else {
                    props.clone()
                };
                let prefix = crate::component::command_prefix(*id);
                let plan = bind_plan(doc, *id, members, &prefix, &props, None);
                let color = color_default(doc, members);

                // Variables first (a scaled source names one), then the
                // expressions, then the slots that read them.
                let mut restore: Vec<Command> = Vec::new();
                for (variable, value) in &plan.variables {
                    let previous = doc.set_variable(variable.clone(), *value)?;
                    restore.push(match previous {
                        Some(previous) => Command::SetVariable {
                            name: variable.clone(),
                            value: previous,
                        },
                        None => Command::RemoveVariable {
                            name: variable.clone(),
                        },
                    });
                }
                for (expression, source) in &plan.expressions {
                    let previous = doc.define_expression(*expression, source.clone());
                    restore.push(match previous {
                        Some(previous) => Command::DefineExpression {
                            id: *expression,
                            source: previous,
                        },
                        None => Command::RemoveExpression { id: *expression },
                    });
                }
                for write in &plan.writes {
                    let member = members[write.member];
                    let node = doc
                        .nodes
                        .get_mut(&member)
                        .ok_or(VectraError::NodeNotFound(member))?;
                    let previous = node.set_param(&write.property, write.value.clone())?;
                    restore.push(Command::SetParameter {
                        node_id: member,
                        property: write.property.clone(),
                        value: previous,
                    });
                }

                // The spec must carry the *effective* variables: a colour prop
                // has none, and a scaled prop is read through the scale prop.
                let mut spec = plan.spec.clone();
                spec.props = props;
                let operands = seed_operands(&spec, color);
                debug_assert!(spec.props.iter().all(|prop| prop.ty == PropType::Color
                    || spec.variables.contains_key(&prop.key)
                    || prop.law.is_scaled()));

                let record = ProceduralNode {
                    id: *id,
                    name: name.clone().unwrap_or_else(|| "Component".to_string()),
                    kind: crate::procedural::ProceduralKind::ComponentMaster {
                        members: members.clone(),
                        spec,
                    },
                    wires: Default::default(),
                    operands,
                    enabled: true,
                    style: Default::default(),
                };
                record.validate(&doc.procedural)?;
                doc.procedural.insert(record);

                restore.reverse();
                restore.push(Command::RemoveProceduralNode { id: *id });
                Ok(Self::batch(restore))
            }
            Self::InstantiateComponent {
                id,
                master,
                name,
                index,
            } => {
                use crate::component::{
                    bind_plan, color_default, scale_key, seed_operands, spec_of, ComponentProp,
                };

                let master_node = doc
                    .procedural
                    .get(*master)
                    .ok_or_else(|| unknown_procedural(*master))?;
                let master_members = match &master_node.kind {
                    crate::procedural::ProceduralKind::ComponentMaster { members, .. } => {
                        members.clone()
                    }
                    _ => {
                        return Err(VectraError::command(format!(
                            "{master} is not a component master"
                        )))
                    }
                };
                if doc.procedural.contains(*id) {
                    return Err(VectraError::command(format!(
                        "procedural node {id} already exists"
                    )));
                }
                let base_name = name
                    .clone()
                    .unwrap_or_else(|| format!("{} instance", master_node.name));

                // 1. The clones: ordinary authored nodes, in an ordinary group.
                let mut restore: Vec<Command> = Vec::new();
                let mut clones: Vec<NodeId> = Vec::new();
                for member in &master_members {
                    let clone_id = crate::ids::new_node_id();
                    let mut clone = doc
                        .nodes
                        .get(member)
                        .ok_or(VectraError::NodeNotFound(*member))?
                        .clone();
                    clone.id = clone_id;
                    clone.name = format!("{} copy", clone.name);
                    doc.insert_node(clone, None)?;
                    clones.push(clone_id);
                    restore.push(Command::DeleteNode { id: clone_id });
                }
                let group = crate::ids::new_node_id();
                let group_node = Node::new(
                    group,
                    base_name.clone(),
                    NodeKind::Group {
                        children: clones.clone(),
                    },
                );
                doc.insert_node(group_node, *index)?;
                restore.push(Command::DeleteNode { id: group });

                // 2. Bind the clones with a plan of their own: same props and
                //    laws, fresh variables **and fresh expressions**, so this
                //    instance is independent of the master and of every other
                //    instance.
                let master_spec = spec_of(doc, *master)
                    .cloned()
                    .ok_or_else(|| unknown_procedural(*master))?;
                let props: Vec<ComponentProp> = master_spec
                    .props
                    .iter()
                    .map(|prop| prop.for_new_owner())
                    .collect();
                let prefix = crate::component::instance_prefix(*id);
                // The instance starts at the **master's** size, not at a fresh
                // reading of its own clones: by the time the clones exist their
                // geometry is expression-bound (`$size × factor`) and
                // `design_size` reads literals, so the reading lands on its 1.0
                // floor — which would place a 1×1 speck whose Size slider spans
                // 0..4 instead of a copy of the artwork. The master's own
                // variable is the number the designer sees on it.
                let seed = scale_key(&master_spec.props)
                    .and_then(|key| master_spec.variable(key))
                    .and_then(|variable| doc.variables.get(variable).copied());
                let plan = bind_plan(doc, *id, &clones, &prefix, &props, seed);
                let color = color_default(doc, &master_members);

                for (variable, value) in &plan.variables {
                    let previous = doc.set_variable(variable.clone(), *value)?;
                    restore.push(match previous {
                        Some(previous) => Command::SetVariable {
                            name: variable.clone(),
                            value: previous,
                        },
                        None => Command::RemoveVariable {
                            name: variable.clone(),
                        },
                    });
                }
                for (expression, source) in &plan.expressions {
                    let previous = doc.define_expression(*expression, source.clone());
                    restore.push(match previous {
                        Some(previous) => Command::DefineExpression {
                            id: *expression,
                            source: previous,
                        },
                        None => Command::RemoveExpression { id: *expression },
                    });
                }
                for write in &plan.writes {
                    let clone = clones[write.member];
                    let node = doc
                        .nodes
                        .get_mut(&clone)
                        .ok_or(VectraError::NodeNotFound(clone))?;
                    let previous = node.set_param(&write.property, write.value.clone())?;
                    restore.push(Command::SetParameter {
                        node_id: clone,
                        property: write.property.clone(),
                        value: previous,
                    });
                }

                // 3. The instance record itself: its *own* spec, so
                //    `SetComponentProp` writes this instance's variables and
                //    re-weights this instance's expressions.
                let mut spec = plan.spec.clone();
                spec.props = props;
                let operands = seed_operands(&spec, color);
                let record = ProceduralNode {
                    id: *id,
                    name: base_name,
                    kind: crate::procedural::ProceduralKind::Component {
                        master: *master,
                        group,
                        spec,
                    },
                    wires: Default::default(),
                    operands,
                    enabled: true,
                    style: Default::default(),
                };
                record.validate(&doc.procedural)?;
                doc.procedural.insert(record);

                restore.reverse();
                restore.push(Command::RemoveProceduralNode { id: *id });
                Ok(Self::batch(restore))
            }
            Self::SetComponentProp {
                target,
                prop,
                value,
            } => {
                use crate::component::{members_of, spec_of, PropLaw, PropType};

                let spec = spec_of(doc, *target)
                    .cloned()
                    .ok_or_else(|| VectraError::command(format!("{target} is not a component")))?;
                let component_prop = spec.get(prop).cloned().ok_or_else(|| {
                    VectraError::command(format!("component {target} has no prop `{prop}`"))
                })?;
                let key = component_prop.key.clone();
                let _members = members_of(doc, *target);

                let previous = match component_prop.ty {
                    PropType::Scalar => {
                        let literal = match &value {
                            ParamValue::Float(Parameter::Literal(v)) => *v,
                            _ => {
                                return Err(VectraError::command(
                                    "a scalar prop takes a literal number",
                                ))
                            }
                        };
                        match &component_prop.law {
                            // A direct prop *is* its variable.
                            PropLaw::Direct => {
                                let variable = spec.variable(&key).cloned().ok_or_else(|| {
                                    VectraError::command(format!(
                                        "prop `{key}` has no variable to write"
                                    ))
                                })?;
                                let previous = doc.set_variable(variable, literal)?;
                                ParamValue::Float(Parameter::Literal(previous.unwrap_or(0.0)))
                            }
                            // A scaled prop is derived: the number typed is the
                            // value at the *current* scale, so the factor is
                            // what actually changes (RULE 3).
                            PropLaw::Scaled { .. } => {
                                let scale_key = component_prop
                                    .law
                                    .scale()
                                    .map(str::to_string)
                                    .unwrap_or_default();
                                let scale_variable =
                                    spec.variable(&scale_key).cloned().ok_or_else(|| {
                                        VectraError::command(format!(
                                            "prop `{key}` scales from `{scale_key}`, \
                                             which has no variable"
                                        ))
                                    })?;
                                let scale =
                                    doc.variables.get(&scale_variable).copied().unwrap_or(0.0);
                                let previous = component_prop.factor().unwrap_or(0.0) * scale;
                                if scale.abs() > f64::EPSILON {
                                    let source = crate::component::scale_source(
                                        literal / scale,
                                        &scale_variable,
                                    );
                                    for scaled in &component_prop.scaled {
                                        doc.define_expression(scaled.expression, source.clone());
                                    }
                                }
                                ParamValue::Float(Parameter::Literal(previous))
                            }
                        }
                    }
                    PropType::Color => {
                        let literal = match &value {
                            ParamValue::Color(Parameter::Literal(color)) => *color,
                            _ => {
                                return Err(VectraError::command(
                                    "a colour prop takes a literal colour",
                                ))
                            }
                        };
                        let node = doc
                            .procedural
                            .get_mut(*target)
                            .ok_or_else(|| unknown_procedural(*target))?;
                        node.operands
                            .insert(key.clone(), value.clone())
                            .unwrap_or(ParamValue::Color(Parameter::Literal(literal)))
                    }
                };

                // A colour prop's value *is* the port an instance publishes, so
                // rewriting it is enough; a scalar prop that drives geometry
                // reaches the document through the dependency graph.
                Ok(Self::SetComponentProp {
                    target: *target,
                    prop: key,
                    value: previous,
                })
            }
            Self::SetComponentSpec { id, spec } => {
                let node = doc
                    .procedural
                    .get(*id)
                    .ok_or_else(|| unknown_procedural(*id))?;
                let mut candidate = node.clone();
                let previous = match &mut candidate.kind {
                    crate::procedural::ProceduralKind::ComponentMaster {
                        spec: current, ..
                    }
                    | crate::procedural::ProceduralKind::Component { spec: current, .. } => {
                        std::mem::replace(current, spec.clone())
                    }
                    _ => return Err(VectraError::command(format!("{id} is not a component"))),
                };
                candidate.validate(&doc.procedural)?;
                doc.procedural.insert(candidate);
                Ok(Self::SetComponentSpec {
                    id: *id,
                    spec: previous,
                })
            }
            Self::Batch { commands } => {
                let mut inverses = Vec::with_capacity(commands.len());
                for cmd in commands {
                    inverses.push(cmd.apply(doc)?);
                }
                // Reversed: undoing a batch must unwind it last-in-first-out.
                inverses.reverse();
                Ok(Self::batch(inverses))
            }
        }
    }

    /// Flatten nested batches, so a batch's inverse is always a flat list.
    fn batch(commands: Vec<Command>) -> Command {
        batch_from(commands)
    }

    /// Compose commands into one user action: children apply in order, and the
    /// inverse unwinds them last-in-first-out.
    ///
    /// Used by the engine to fold a solver pass's writes into the history entry
    /// of the command that caused them, so one undo reverts both.
    pub fn batch_commands(commands: Vec<Command>) -> Command {
        batch_from(commands)
    }

    pub fn from_json(json: &str) -> Result<Self, VectraError> {
        serde_json::from_str(json).map_err(|e| VectraError::Serialization(e.to_string()))
    }

    pub fn to_json(&self) -> Result<String, VectraError> {
        serde_json::to_string(self).map_err(|e| VectraError::Serialization(e.to_string()))
    }
}

/// Flatten nested batches into one composite command (1-length batches and
/// empty batches collapse to their content). Inverse-returning, so it is safe
/// to store in a history entry.
pub(crate) fn batch_from(commands: Vec<Command>) -> Command {
    if commands.len() == 1 {
        return commands.into_iter().next().expect("len checked");
    }
    let mut flat = Vec::with_capacity(commands.len());
    for cmd in commands {
        match cmd {
            Command::Batch { commands } => flat.extend(commands),
            other => flat.push(other),
        }
    }
    Command::Batch { commands: flat }
}

/// Engine → UI notifications (MES §16, step 4).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum EngineEvent {
    NodesUpdated {
        ids: Vec<NodeId>,
    },
    NodesRemoved {
        ids: Vec<NodeId>,
    },
    OrderChanged,
    VariablesUpdated {
        names: Vec<VariableId>,
    },
    ExpressionsUpdated {
        ids: Vec<ExpressionId>,
    },
    /// Constraints were added, removed, enabled or disabled (Task 3.1).
    ConstraintsUpdated {
        ids: Vec<ConstraintId>,
    },
    /// Operations were added, removed, enabled or disabled (Task 4.0). The
    /// virtual node's geometry arrives as its own id in `Dirty`, once the
    /// operations pass has recomputed it.
    OperationsUpdated {
        ids: Vec<OperationId>,
    },
    /// A drag gesture opened (Task 3.2) — one line in the log, not one per
    /// pointer sample.
    /// Procedural-graph changes (Task 7.0): a node was added, removed, wired,
    /// rewired, parked or given a new operand. The port values and drawn
    /// geometry it produces arrive as ids in `Dirty`, once the pass has run.
    ProceduralUpdated {
        ids: Vec<NodeId>,
    },
    /// Motion-track registry changes (Task 6.0). A track is a *graph vertex*
    /// (`GraphNode::Track`), so this event dirties exactly the slots that sample
    /// it — the same discipline variables and expressions already had.
    TracksUpdated {
        ids: Vec<TrackId>,
    },
    DragStarted {
        node_id: NodeId,
    },
    /// A drag gesture finished; the geometry it produced is already in the
    /// document and now lives in exactly one history entry.
    DragEnded {
        node_id: NodeId,
    },
    /// **Presentation flags changed** — a node's eye or padlock (Task 10.2
    /// RULE 4).
    ///
    /// Deliberately *not* a geometry event, and the distinction is the whole
    /// performance rule: the dependency graph ignores this variant (see
    /// `DependencyGraph::dirty_ids_for_events`), so nothing re-resolves, nothing
    /// re-tessellates, and the scene cache is untouched. The **renderer** is what
    /// listens: it re-reads the flags and skips the node's draws. A hidden layer
    /// costs one frame of drawing fewer triangles, not a re-evaluation of the
    /// document.
    NodeFlagsChanged {
        ids: Vec<NodeId>,
    },
    /// Layer records changed: created, deleted, renamed, re-enabled or
    /// re-flagged (Task 10.2 RULE 1).
    LayersUpdated {
        ids: Vec<LayerId>,
    },
    /// A layer or node **moved in the draw order**. Carries no ids: what moved
    /// is the whole stack, and the scene re-reads its order from the document
    /// rather than re-evaluating anything.
    LayerOrderChanged,
    /// Artboards changed: added, removed, renamed, re-framed, re-coloured, or
    /// the active board switched (Task 10.2 RULE 2).
    ArtboardsUpdated {
        ids: Vec<ArtboardId>,
    },
    StackChanged {
        can_undo: bool,
        can_redo: bool,
    },
    /// Exactly which geometry nodes were re-evaluated for this mutation, and
    /// whether that was a full or incremental pass (Task 2.2).
    ///
    /// Unlike the variants above, this one is *derived*, not an intent: it is
    /// appended by the engine owner (`vectra-wasm`) after the dependency graph
    /// has propagated the change, so [`Command::preview_events`] never emits it.
    /// `ids` is empty for a mutation with no dependents — the visible proof
    /// that an edit cost nothing to re-render.
    Dirty {
        ids: Vec<NodeId>,
        mode: crate::eval::EvalMode,
    },
}

/// One typed message for "no such procedural node", used by every command that
/// addresses one.
fn unknown_procedural(id: NodeId) -> VectraError {
    VectraError::command(format!("procedural node {id} is not registered"))
}

/// Every port a binding reads, checked against the node it would steer — when
/// the slot it steers is a geometry slot (`Document::property_feeds_geometry`).
fn validate_binding_references(
    doc: &Document,
    node_id: NodeId,
    property: &str,
    binding: &MotionBinding,
) -> Result<(), VectraError> {
    if !Node::property_feeds_geometry(property) {
        return Ok(());
    }
    for param in binding.inner_parameters() {
        if let Parameter::Procedural(reference) = param {
            validate_procedural_cycle(doc, node_id, reference)?;
        }
    }
    Ok(())
}

/// RULE 3, the half no operand-shaped check can cover.
///
/// The spec's sentence is "a value reference back into the graph is a cycle
/// wearing a disguise", and it names the *operand* form of it: a procedural
/// node's `Parameter<f64>` must not read another node's port, because a wire
/// already expresses dependency inside the graph. But an operand is only one
/// road back. The other one runs through **geometry**:
///
/// ```text
///   rect.width  ←  ⬡ grid • span     the slot reads a port (legal: it is not
///                                    read by the grid)
///   ⬡ source    →  rect              and a source node reads rect's shape
/// ```
///
/// Now flip one slot — let `rect.width` read the output of a node *downstream*
/// of `rect` — and the loop closes with no port-shaped edge anywhere in it: the
/// grid's value depends on `rect`, `rect`'s shape depends on the grid's value.
/// The graph's cycle gate cannot see this (the source→rect relation is a
/// *geometry* read, deliberately not an edge — see `vectra-dependency`'s
/// `procedural_edges`), and an accepted loop is not a static wrong answer: the
/// settle path re-reads a reader and re-runs the chain that reads it, so it
/// would oscillate once per round instead of converging.
///
/// So the boundary asks the question the graph cannot: is `subject` already
/// upstream of the port it wants to read, through shapes rather than ports?
/// If it is, the reference is refused here, typed, with nothing mutated — the
/// same discipline as every other gate in this module.
fn validate_procedural_cycle(
    doc: &Document,
    subject: NodeId,
    reference: &NodeOutputId,
) -> Result<(), VectraError> {
    let closure = doc.procedural_geometry_closure(reference.node);
    if closure.contains(&subject) {
        return Err(VectraError::cyclic(format!(
            "reading {port:?} of procedural node {node} here would close a cycle through geometry: \
             {subject} is upstream of that port",
            port = reference.port,
            node = reference.node,
        )));
    }
    Ok(())
}

/// Reject a binding that could never resolve, so a bound slot is always
/// readable. Cheap, boundary-time validation — the same discipline the
/// expression and constraint gates follow.
fn validate_binding(
    doc: &Document,
    node_id: NodeId,
    property: &str,
    binding: &MotionBinding,
) -> Result<(), VectraError> {
    match binding {
        MotionBinding::Spring {
            stiffness, damping, ..
        } => {
            if !stiffness.is_finite() || *stiffness <= 0.0 {
                return Err(VectraError::command(format!(
                    "spring stiffness must be positive and finite (got {stiffness})"
                )));
            }
            // Damping must be positive: an undamped spring oscillates forever,
            // which would make "is the animation idle?" unanswerable and pin the
            // frame loop at 60 fps for good. See TASK-6.0-DESIGN.md §D2.
            if !damping.is_finite() || *damping <= 0.0 {
                return Err(VectraError::command(format!(
                    "spring damping must be positive and finite (got {damping})"
                )));
            }
            // A binding's inner parameters are slots like any other, so they
            // carry the same ban: a spring aimed by a port the node feeds is a
            // cycle wearing a disguise, one level deeper.
            validate_binding_references(doc, node_id, property, binding)
        }
        MotionBinding::KeyframeTrack { track_id, property } => {
            validate_binding_references(doc, node_id, property, binding)?;
            let track = doc.motion.get(track_id).ok_or_else(|| {
                VectraError::command(format!("motion track {track_id} does not exist"))
            })?;
            if !track.channels.contains_key(property) {
                return Err(VectraError::command(format!(
                    "motion track {track_id} has no channel {property:?} (channels: {})",
                    track
                        .channels
                        .keys()
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(", ")
                )));
            }
            Ok(())
        }
        MotionBinding::StateDriven { state, .. } => {
            if state.trim().is_empty() {
                return Err(VectraError::command("state name must not be empty"));
            }
            Ok(())
        }
    }
}

/// One undoable history entry.
#[derive(Debug, Clone, PartialEq)]
pub struct HistoryEntry {
    pub forward: Command,
    pub backward: Command,
    pub label: String,
}

/// Bounded undo/redo stack over [`Command`] inverses.
#[derive(Debug)]
pub struct CommandStack {
    undo_stack: Vec<HistoryEntry>,
    redo_stack: Vec<HistoryEntry>,
    limit: usize,
}

impl CommandStack {
    pub fn new(limit: usize) -> Self {
        Self {
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            limit: limit.max(1),
        }
    }

    pub fn with_default_limit() -> Self {
        Self::new(200)
    }

    pub fn can_undo(&self) -> bool {
        !self.undo_stack.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo_stack.is_empty()
    }

    pub fn undo_len(&self) -> usize {
        self.undo_stack.len()
    }

    pub fn redo_len(&self) -> usize {
        self.redo_stack.len()
    }

    pub fn clear(&mut self) {
        self.undo_stack.clear();
        self.redo_stack.clear();
    }

    /// The command that [`CommandStack::undo`] would apply next, without
    /// applying it (`None` when the undo stack is empty).
    ///
    /// The undo of a command is its *inverse*, so this returns the inverse:
    /// the exact [`Command`] a caller can dry-run (e.g. the dependency graph's
    /// cycle pre-check in Task 2.2) before deciding to undo.
    pub fn peek_undo(&self) -> Option<&Command> {
        self.undo_stack.last().map(|entry| &entry.backward)
    }

    /// The command that [`CommandStack::redo`] would re-apply next, without
    /// applying it (`None` when the redo stack is empty).
    pub fn peek_redo(&self) -> Option<&Command> {
        self.redo_stack.last().map(|entry| &entry.forward)
    }

    /// Label of the undo entry [`CommandStack::undo`] would apply next.
    pub fn peek_undo_label(&self) -> Option<&str> {
        self.undo_stack.last().map(|entry| entry.label.as_str())
    }

    /// Fold `extra` into the top entry's backward command (Task 3.1).
    ///
    /// The constraint solver runs *after* a command has been applied, so the
    /// properties it adjusts as a consequence belong to that same user action.
    /// Prepending them here means one undo reverts the action **and** everything
    /// the solver did because of it — an undo is a true pre-image, not a
    /// half-reverted state. `extra` must therefore be the *inverse* of what the
    /// solver applied (see `Engine::apply_untracked`).
    pub fn amend_top_backward(&mut self, extra: Command) {
        let Some(entry) = self.undo_stack.last_mut() else {
            return;
        };
        if matches!(extra, Command::Batch { ref commands } if commands.is_empty()) {
            return;
        }
        let existing = std::mem::replace(&mut entry.backward, Command::Batch { commands: vec![] });
        entry.backward = batch_from(vec![extra, existing]);
    }

    /// Pop the top entry and apply its inverse **without** pushing it onto the
    /// redo stack.
    ///
    /// Used to roll a command back when a post-apply stage rejects the result
    /// (the solver finding a deep contradiction between required constraints):
    /// the document returns to its previous state and the stack looks as if the
    /// command had never been dispatched.
    pub fn rollback_last(&mut self, doc: &mut Document) -> Result<(), VectraError> {
        let entry = self.undo_stack.pop().ok_or(VectraError::NothingToUndo)?;
        entry.backward.apply(doc)?;
        Ok(())
    }

    fn stack_event(&self) -> EngineEvent {
        EngineEvent::StackChanged {
            can_undo: self.can_undo(),
            can_redo: self.can_redo(),
        }
    }

    /// Execute and push. Clears the redo stack. Failing commands push nothing.
    pub fn execute(
        &mut self,
        doc: &mut Document,
        cmd: Command,
    ) -> Result<Vec<EngineEvent>, VectraError> {
        let label = cmd.label();
        let mut events = cmd.preview_events();
        let backward = cmd.apply(doc)?;
        self.undo_stack.push(HistoryEntry {
            forward: cmd,
            backward,
            label,
        });
        if self.undo_stack.len() > self.limit {
            self.undo_stack.remove(0);
        }
        self.redo_stack.clear();
        events.push(self.stack_event());
        Ok(events)
    }

    /// Record an action whose effects are **already applied** as one history
    /// entry (Task 3.2).
    ///
    /// [`CommandStack::execute`] applies and pushes; a drag gesture is the
    /// mirror image — the writes land as the pointer moves (through the solver,
    /// see `Engine::apply_untracked`), and the gesture is pushed *once*, at
    /// `EndDrag`, with the net movement as its forward command and the exact
    /// pre-drag state as its backward command. From here on the entry is
    /// indistinguishable from an executed one.
    ///
    /// Does the same bookkeeping as [`CommandStack::execute`]: clears the redo
    /// stack, trims to the limit, and reports the stack change.
    pub fn record(&mut self, forward: Command, backward: Command, label: impl Into<String>) {
        self.undo_stack.push(HistoryEntry {
            forward,
            backward,
            label: label.into(),
        });
        if self.undo_stack.len() > self.limit {
            self.undo_stack.remove(0);
        }
        self.redo_stack.clear();
    }

    pub fn undo(&mut self, doc: &mut Document) -> Result<Vec<EngineEvent>, VectraError> {
        let entry = self.undo_stack.pop().ok_or(VectraError::NothingToUndo)?;
        let mut events = entry.backward.preview_events();
        // Applying the backward command must succeed; if the document changed
        // out-of-band this surfaces as a typed error instead of a corrupt stack.
        let re_forward = entry.backward.apply(doc)?;
        self.redo_stack.push(HistoryEntry {
            forward: entry.forward,
            backward: re_forward,
            label: entry.label,
        });
        events.push(self.stack_event());
        Ok(events)
    }

    pub fn redo(&mut self, doc: &mut Document) -> Result<Vec<EngineEvent>, VectraError> {
        let entry = self.redo_stack.pop().ok_or(VectraError::NothingToRedo)?;
        let mut events = entry.forward.preview_events();
        let backward = entry.forward.apply(doc)?;
        self.undo_stack.push(HistoryEntry {
            forward: entry.forward,
            backward,
            label: entry.label,
        });
        events.push(self.stack_event());
        Ok(events)
    }
}

impl Default for CommandStack {
    fn default() -> Self {
        Self::with_default_limit()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::{new_expression_id, new_node_id};
    use crate::param::Parameter;

    fn rect_cmd(id: NodeId) -> Command {
        Command::CreateNode {
            id,
            kind: NodeKind::rectangle(0.0, 0.0, 100.0, 50.0),
            name: Some("rect".to_string()),
            index: None,
        }
    }

    #[test]
    fn create_undo_redo_restores_identity() {
        let mut doc = Document::new();
        let mut stack = CommandStack::with_default_limit();
        let id = new_node_id();
        stack.execute(&mut doc, rect_cmd(id)).unwrap();
        assert!(doc.get_node(id).is_ok());
        stack.undo(&mut doc).unwrap();
        assert!(doc.get_node(id).is_err());
        stack.redo(&mut doc).unwrap();
        let node = doc.get_node(id).unwrap();
        assert_eq!(node.name, "rect");
    }

    #[test]
    fn set_parameter_undo_restores_old_value() {
        let mut doc = Document::new();
        let mut stack = CommandStack::with_default_limit();
        let id = new_node_id();
        stack.execute(&mut doc, rect_cmd(id)).unwrap();
        stack
            .execute(
                &mut doc,
                Command::SetParameter {
                    node_id: id,
                    property: "width".to_string(),
                    value: ParamValue::Float(Parameter::variable("base")),
                },
            )
            .unwrap();
        assert_eq!(
            doc.get_node(id).unwrap().get_param("width").unwrap(),
            ParamValue::Float(Parameter::variable("base"))
        );
        stack.undo(&mut doc).unwrap();
        assert_eq!(
            doc.get_node(id).unwrap().get_param("width").unwrap(),
            ParamValue::float_literal(100.0)
        );
    }

    #[test]
    fn failing_command_pushes_nothing() {
        let mut doc = Document::new();
        let mut stack = CommandStack::with_default_limit();
        let err = stack
            .execute(
                &mut doc,
                Command::SetParameter {
                    node_id: new_node_id(),
                    property: "width".to_string(),
                    value: ParamValue::float_literal(1.0),
                },
            )
            .unwrap_err();
        assert!(matches!(err, VectraError::NodeNotFound(_)));
        assert!(!stack.can_undo());
    }

    #[test]
    fn define_remove_expression_undo_round_trip() {
        let mut doc = Document::new();
        let mut stack = CommandStack::with_default_limit();
        let id = new_expression_id();
        stack
            .execute(
                &mut doc,
                Command::DefineExpression {
                    id,
                    source: "$a * 2".to_string(),
                },
            )
            .unwrap();
        assert_eq!(doc.expressions.get(&id).unwrap().source, "$a * 2");

        // Redefine overwrites; undo restores the previous source.
        stack
            .execute(
                &mut doc,
                Command::DefineExpression {
                    id,
                    source: "$b".to_string(),
                },
            )
            .unwrap();
        stack.undo(&mut doc).unwrap();
        assert_eq!(doc.expressions.get(&id).unwrap().source, "$a * 2");
        stack.redo(&mut doc).unwrap();
        assert_eq!(doc.expressions.get(&id).unwrap().source, "$b");

        // Remove; undo restores.
        stack
            .execute(&mut doc, Command::RemoveExpression { id })
            .unwrap();
        assert!(!doc.expressions.contains_key(&id));
        stack.undo(&mut doc).unwrap();
        assert_eq!(doc.expressions.get(&id).unwrap().source, "$b");

        // Removing an unknown id fails typed and is stack-neutral.
        let depth = stack.undo_len();
        assert!(matches!(
            stack.execute(
                &mut doc,
                Command::RemoveExpression {
                    id: new_expression_id()
                }
            ),
            Err(VectraError::ExpressionNotFound(_))
        ));
        assert_eq!(stack.undo_len(), depth);
    }

    #[test]
    fn stack_peeks_without_mutating() {
        let mut doc = Document::new();
        let mut stack = CommandStack::with_default_limit();
        assert!(stack.peek_undo().is_none());
        assert!(stack.peek_redo().is_none());

        let id = new_node_id();
        stack.execute(&mut doc, rect_cmd(id)).unwrap();
        stack
            .execute(
                &mut doc,
                Command::SetVariable {
                    name: "base".to_string(),
                    value: 9.0,
                },
            )
            .unwrap();

        // peek_undo returns the INVERSE of the newest entry (what undo applies).
        match stack.peek_undo().unwrap() {
            Command::RemoveVariable { name } => assert_eq!(name, "base"),
            other => panic!("expected inverse RemoveVariable, got {other:?}"),
        }
        assert_eq!(stack.peek_undo_label(), Some("Set $base"));
        // Peeking is side-effect free.
        assert_eq!(doc.variables.get("base"), Some(&9.0));
        assert_eq!(stack.undo_len(), 2);

        stack.undo(&mut doc).unwrap();
        // After the undo, redo would re-apply the forward command.
        assert!(matches!(
            stack.peek_redo().unwrap(),
            Command::SetVariable { name, value } if name == "base" && *value == 9.0
        ));
        assert!(stack.peek_undo().is_some());
    }

    #[test]
    fn dirty_event_serializes_with_mode() {
        let event = EngineEvent::Dirty {
            ids: vec![new_node_id()],
            mode: crate::eval::EvalMode::Incremental,
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "Dirty");
        assert_eq!(json["mode"], "incremental");
        assert_eq!(json["ids"].as_array().unwrap().len(), 1);

        // preview_events never claims a Dirty event (it is derived, not intent).
        let cmd = rect_cmd(new_node_id());
        assert!(!cmd
            .preview_events()
            .iter()
            .any(|e| matches!(e, EngineEvent::Dirty { .. })));
    }

    // ── Task 7.0: the procedural graph's commands ──────────────────────

    fn grid_cmd(id: NodeId) -> Command {
        Command::AddProceduralNode {
            node: crate::procedural::ProceduralNode::new(
                id,
                crate::procedural::ProceduralKind::grid(3.0, 2.0, 10.0, crate::geom::Point2::ZERO),
            ),
        }
    }

    #[test]
    fn add_remove_procedural_node_round_trips_with_its_wires() {
        let mut doc = Document::new();
        let grid = crate::ids::new_node_id();
        let smooth = crate::ids::new_node_id();
        grid_cmd(grid).apply(&mut doc).unwrap();
        Command::AddProceduralNode {
            node: crate::procedural::ProceduralNode::new(
                smooth,
                crate::procedural::ProceduralKind::smooth(2.0, 0.5),
            ),
        }
        .apply(&mut doc)
        .unwrap();
        // Wire the modifier to the generator.
        let forward = Command::ConnectProcedural {
            node_id: smooth,
            port: "region".into(),
            from: crate::param::NodeOutputId::new(grid, "region"),
        };
        forward.apply(&mut doc).unwrap();
        assert_eq!(doc.procedural.get(smooth).unwrap().wires.len(), 1, "wired");

        // Removing the *generator* withdraws the consumer's wire and returns
        // both, so undo restores the whole picture.
        let remove = Command::RemoveProceduralNode { id: grid };
        let inverse = remove.apply(&mut doc).unwrap();
        assert!(!doc.procedural.contains(grid));
        assert!(
            doc.procedural.get(smooth).unwrap().wires.is_empty(),
            "the dangling wire left with its upstream"
        );

        inverse.apply(&mut doc).unwrap();
        assert!(doc.procedural.contains(grid));
        assert_eq!(
            doc.procedural.get(smooth).unwrap().wires["region"],
            crate::param::NodeOutputId::new(grid, "region"),
            "undo restored the record *and* the wire into it"
        );
    }

    #[test]
    fn connect_and_disconnect_have_exact_inverses() {
        let mut doc = Document::new();
        let grid = crate::ids::new_node_id();
        let smooth = crate::ids::new_node_id();
        grid_cmd(grid).apply(&mut doc).unwrap();
        Command::AddProceduralNode {
            node: crate::procedural::ProceduralNode::new(
                smooth,
                crate::procedural::ProceduralKind::smooth(2.0, 0.5),
            ),
        }
        .apply(&mut doc)
        .unwrap();

        let connect = Command::ConnectProcedural {
            node_id: smooth,
            port: "region".into(),
            from: crate::param::NodeOutputId::new(grid, "region"),
        };
        let inverse = connect.apply(&mut doc).unwrap();
        assert!(matches!(inverse, Command::DisconnectProcedural { .. }));
        inverse.apply(&mut doc).unwrap();
        assert!(doc.procedural.get(smooth).unwrap().wires.is_empty());

        // Re-connecting the *same* address has a re-connect inverse: the wire
        // is genuinely replaced, so undo must replace it back.
        connect.apply(&mut doc).unwrap();
        let again = connect.apply(&mut doc).unwrap();
        assert_eq!(again, connect, "same wire ⇒ the same wire is restored");
        let disconnect = Command::DisconnectProcedural {
            node_id: smooth,
            port: "region".into(),
        };
        let restore = disconnect.apply(&mut doc).unwrap();
        assert!(matches!(restore, Command::ConnectProcedural { .. }));
        disconnect.apply(&mut doc).unwrap_err(); // already gone: typed refusal
    }

    #[test]
    fn procedural_operands_are_parametric_and_undoable() {
        let mut doc = Document::new();
        let grid = crate::ids::new_node_id();
        grid_cmd(grid).apply(&mut doc).unwrap();

        let forward = Command::SetProceduralOperand {
            node_id: grid,
            port: "spacing".into(),
            value: ParamValue::Float(Parameter::Variable("gap".into())),
        };
        let inverse = forward.apply(&mut doc).unwrap();
        assert_eq!(
            doc.procedural.get(grid).unwrap().operands["spacing"],
            ParamValue::Float(Parameter::Variable("gap".into())),
            "a procedural operand is as parametric as any other slot"
        );
        inverse.apply(&mut doc).unwrap();
        assert_eq!(
            doc.procedural.get(grid).unwrap().operands["spacing"],
            ParamValue::float_literal(10.0),
            "undo restores the effective previous value"
        );
    }

    #[test]
    fn rule_three_and_port_typing_gate_the_commands() {
        let mut doc = Document::new();
        let grid = crate::ids::new_node_id();
        let smooth = crate::ids::new_node_id();
        grid_cmd(grid).apply(&mut doc).unwrap();
        Command::AddProceduralNode {
            node: crate::procedural::ProceduralNode::new(
                smooth,
                crate::procedural::ProceduralKind::smooth(2.0, 0.5),
            ),
        }
        .apply(&mut doc)
        .unwrap();

        // RULE 3: an operand may not read a procedural output.
        let disguised = Command::SetProceduralOperand {
            node_id: smooth,
            port: "iterations".into(),
            value: ParamValue::Float(Parameter::Procedural(crate::param::NodeOutputId::new(
                grid, "span",
            ))),
        };
        let error = disguised.apply(&mut doc).unwrap_err();
        assert!(error.is_cycle_rejection(), "{error}");
        assert_eq!(
            doc.procedural.get(smooth).unwrap().operands["iterations"],
            ParamValue::float_literal(2.0),
            "a rejected command mutates nothing"
        );

        // RULE 1: a Points port cannot feed a Region input.
        let mismatched = Command::ConnectProcedural {
            node_id: smooth,
            port: "region".into(),
            from: crate::param::NodeOutputId::new(grid, "points"),
        };
        let error = mismatched.apply(&mut doc).unwrap_err();
        assert!(matches!(
            error,
            VectraError::Resolve(crate::error::ResolveError::ProceduralPortType { .. })
        ));

        // A wheel: the node cannot consume its own output.
        let self_wire = Command::ConnectProcedural {
            node_id: smooth,
            port: "region".into(),
            from: crate::param::NodeOutputId::new(smooth, "region"),
        };
        assert!(self_wire.apply(&mut doc).unwrap_err().is_cycle_rejection());
    }

    /// The gate the graph cannot run: a slot may read a port, but not one the
    /// slot's own geometry feeds. The graph draws edges for ports only, so this
    /// loop has to be caught by walking shapes at the boundary.
    #[test]
    fn the_cycle_gate_sees_through_geometry_too() {
        let mut doc = Document::new();
        let rect = new_node_id();
        rect_cmd(rect).apply(&mut doc).unwrap();
        let grid = new_node_id();
        grid_cmd(grid).apply(&mut doc).unwrap();
        let source = new_node_id();
        Command::AddProceduralNode {
            node: crate::procedural::ProceduralNode::new(
                source,
                crate::procedural::ProceduralKind::Source { node: rect },
            ),
        }
        .apply(&mut doc)
        .unwrap();
        let smooth = new_node_id();
        Command::AddProceduralNode {
            node: crate::procedural::ProceduralNode::new(
                smooth,
                crate::procedural::ProceduralKind::smooth(2.0, 0.5),
            ),
        }
        .apply(&mut doc)
        .unwrap();
        // ⬡ smooth ← ⬡ source ← ◻ rect — and nothing reads rect's geometry
        // except this chain.
        Command::ConnectProcedural {
            node_id: smooth,
            port: "region".into(),
            from: NodeOutputId::new(source, "region"),
        }
        .apply(&mut doc)
        .unwrap();

        // Legal: the grid is upstream of nothing, so rect reading its span is
        // the value path the whole task exists to carry.
        Command::SetParameter {
            node_id: rect,
            property: "width".into(),
            value: ParamValue::Float(Parameter::Procedural(NodeOutputId::new(grid, "span"))),
        }
        .apply(&mut doc)
        .unwrap();

        // …but reading the *downstream* chain's port closes the loop through
        // rect's own shape. Refused, typed, before anything moved.
        let disguised = Command::SetParameter {
            node_id: rect,
            property: "height".into(),
            value: ParamValue::Float(Parameter::Procedural(NodeOutputId::new(smooth, "region"))),
        };
        let error = disguised.apply(&mut doc).unwrap_err();
        assert!(error.is_cycle_rejection(), "{error}");
        assert!(error.to_string().contains("cycle"), "{error}");
        let NodeKind::Rectangle { height, .. } = &doc.get_node(rect).unwrap().kind else {
            panic!("the rect became something else");
        };
        assert_eq!(
            *height,
            Parameter::Literal(50.0),
            "a rejected command mutates nothing"
        );

        // The same ban reaches a binding: the spring steers the very node whose
        // shape feeds the port it aims at.
        let bound = Command::BindMotion {
            node_id: rect,
            property: "width".into(),
            binding: MotionBinding::spring(
                Parameter::Procedural(NodeOutputId::new(smooth, "region")),
                120.0,
                12.0,
                0.0,
                0.0,
            ),
        };
        assert!(bound.apply(&mut doc).unwrap_err().is_cycle_rejection());
        // Style is **not** geometry, so the paint may read the port the shape
        // feeds: fill flows source → rect → noise → tint → rect, and the tint
        // feeds nothing. Rejecting this would make a legal document unwritable
        // (the wasm colour-door law reads exactly this shape).
        let noise = new_node_id();
        Command::AddProceduralNode {
            node: crate::procedural::ProceduralNode::new(
                noise,
                crate::procedural::ProceduralKind::noise(1.0, 0.2, 7.0),
            ),
        }
        .apply(&mut doc)
        .unwrap();
        Command::ConnectProcedural {
            node_id: noise,
            port: "region".into(),
            from: NodeOutputId::new(source, "region"),
        }
        .apply(&mut doc)
        .unwrap();
        Command::SetParameter {
            node_id: rect,
            property: "style.fill".into(),
            value: ParamValue::Color(Parameter::Procedural(NodeOutputId::new(noise, "tint"))),
        }
        .apply(&mut doc)
        .expect("the colour door is not a cycle: paint feeds no geometry");
        // …but the same reference from a *geometry* slot is still refused.
        let paint_from_geometry = Command::SetParameter {
            node_id: rect,
            property: "width".into(),
            value: ParamValue::Float(Parameter::Procedural(NodeOutputId::new(noise, "scalar"))),
        };
        assert!(paint_from_geometry
            .apply(&mut doc)
            .unwrap_err()
            .is_cycle_rejection());

        // …and a binding aimed at a port nothing feeds from rect is fine.
        Command::BindMotion {
            node_id: rect,
            property: "height".into(),
            binding: MotionBinding::spring(
                Parameter::Procedural(NodeOutputId::new(grid, "span")),
                120.0,
                12.0,
                0.0,
                0.0,
            ),
        }
        .apply(&mut doc)
        .unwrap();
    }

    #[test]
    fn delete_node_withdraws_source_nodes_and_undo_restores_them() {
        let mut doc = Document::new();
        let rect = crate::ids::new_node_id();
        rect_cmd(rect).apply(&mut doc).unwrap();
        let source = crate::ids::new_node_id();
        Command::AddProceduralNode {
            node: crate::procedural::ProceduralNode::new(
                source,
                crate::procedural::ProceduralKind::Source { node: rect },
            ),
        }
        .apply(&mut doc)
        .unwrap();

        let inverse = Command::DeleteNode { id: rect }.apply(&mut doc).unwrap();
        assert!(
            !doc.procedural.contains(source),
            "a Source with nothing to read is withdrawn with its subject"
        );
        inverse.apply(&mut doc).unwrap();
        assert!(doc.procedural.contains(source), "and restored with it");
        assert_eq!(doc.get_node(rect).unwrap().id, rect);
    }

    #[test]
    fn a_procedural_result_is_live_geometry_and_a_parked_one_is_not() {
        // RULE 4: the live-id space includes enabled geometry-producing nodes.
        let mut doc = Document::new();
        let grid = crate::ids::new_node_id();
        grid_cmd(grid).apply(&mut doc).unwrap();
        assert!(doc.is_geometry_id(grid));
        assert_eq!(doc.geometry_ids(), vec![grid]);

        Command::SetProceduralEnabled {
            id: grid,
            enabled: false,
        }
        .apply(&mut doc)
        .unwrap();
        assert!(!doc.is_geometry_id(grid), "parked ⇒ not live geometry");
        assert!(doc.geometry_ids().is_empty());
        let parked = Command::SetProceduralEnabled {
            id: grid,
            enabled: true,
        }
        .apply(&mut doc)
        .unwrap();
        assert!(matches!(
            parked,
            Command::SetProceduralEnabled { enabled: false, .. }
        ));
        assert!(doc.is_geometry_id(grid), "re-armed ⇒ live again");
    }

    #[test]
    fn a_slot_reading_a_port_is_reported_as_a_reader() {
        let mut doc = Document::new();
        let rect = crate::ids::new_node_id();
        rect_cmd(rect).apply(&mut doc).unwrap();
        let port = crate::param::NodeOutputId::new(crate::ids::new_node_id(), "span");
        Command::SetParameter {
            node_id: rect,
            property: "width".into(),
            value: ParamValue::Float(Parameter::Procedural(port.clone())),
        }
        .apply(&mut doc)
        .unwrap();
        assert_eq!(
            doc.procedural_readers(std::slice::from_ref(&port)),
            vec![rect]
        );
        assert!(
            doc.procedural_readers(&[crate::param::NodeOutputId::new(
                crate::ids::new_node_id(),
                "span"
            )])
            .is_empty(),
            "an unrelated port has no readers here"
        );
    }

    #[test]
    fn command_json_roundtrip() {
        let cmd = Command::SetVariable {
            name: "base".to_string(),
            value: 12.0,
        };
        let json = cmd.to_json().unwrap();
        assert_eq!(Command::from_json(&json).unwrap(), cmd);
    }
}
