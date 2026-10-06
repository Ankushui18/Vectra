//! The drawing-tools boundary (Task 10.1): the four tools, as engine operations.
//!
//! Every function here exists for the same reason, and it is the reason the Task
//! 1.4 rule is still in force after nine tasks:
//!
//! > **the React UI is a dumb remote control — no geometry logic, no parameter
//! > resolution, no state duplication; only JSON commands in, events and
//! > snapshots out.**
//!
//! A pen tool is not an exception to that rule, it is the hardest test of it. The
//! rest of this module is therefore a set of *narrow* doors: the UI reports where
//! the pointer is and which modifiers are held; the engine decides what that
//! means, produces the geometry with the mathematics in `vectra-draw`, and writes
//! it through the ordinary `Command` path (so a drawn stroke is undoable, is
//! recorded in history, dirties exactly the right nodes and animates the same way
//! as anything else).
//!
//! ```text
//!   pointer event ──▶ draw_pointer(kind, x, y, alt)
//!                        │
//!                        ├─ pen:   PenSession  (vectra-draw) → draft of engine types
//!                        └─ brush: Sample + BrushProfile → brush_stroke → draft
//!                        ▼
//!                 draft JSON (document space, engine vocabulary)
//!                        │
//!   commit ────────▶ CreateNode / SetPath  ──▶ history, dependencies, render
//! ```
//!
//! ## The three doors
//!
//! 1. **[`VectraEngine::draw_pointer`]** — a pen gesture. Returns the draft as it
//!    stands (including the rubber band to the cursor), which the UI draws.
//! 2. **[`VectraEngine::draw_brush`]** — a brush sample, and at the end the
//!    committed stroke. The fitting happens once, on release, against the whole
//!    stroke (the reasoning is in `vectra_draw::stroke`).
//! 3. **[`VectraEngine::draw_quick_shape`]** — the snap. It builds the ideal path
//!    *and* the constraint rows that hold it to the primitive, then dispatches
//!    them through the ordinary command path, where the Task 3.1 solver picks
//!    them up. The UI never sees a solver.
//!
//! ## Why the draft is JSON and not a `Path`
//!
//! The frontend has to *draw* the draft — anchors, handles, the rubber band — on
//! a canvas that is not the WebGPU scene. So the draft crosses as plain data:
//! absolute points in document space, which the UI maps to the screen with the
//! camera it already has. No geometry, just coordinates.

use serde::{Deserialize, Serialize};
use vectra_core::{
    new_node_id, Command, Constraint, EngineEvent, NodeId, NodeKind, ParamValue, Parameter,
    PathSegment, Point2,
};
use vectra_draw::{
    brush_stroke, plan_quick_shape, Anchor, BrushOptions, HandleSide, PathDraft, PenSession, Sample,
};

use wasm_bindgen::prelude::*;

use crate::{CommandResponse, VectraEngine};

/// What the pointer did. The UI's whole vocabulary for drawing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PointerKind {
    /// Button down at `(x, y)` — places an anchor (pen) or starts a stroke.
    Down,
    /// Pointer moved to `(x, y)`. With the button down it drags a handle; with it
    /// up it moves the rubber band.
    Move,
    /// Button released.
    Up,
    /// Escape: abandon the draft.
    Cancel,
}

/// The drawing session: which tool, what it has collected so far.
///
/// One field per tool rather than an enum of states, because the tools share the
/// `Idle`/`Drawing` distinction and a session switch must not silently keep a
/// half-drawn pen path alive: [`ToolSession::begin`] clears both.
#[derive(Default)]
pub struct DrawTools {
    pen: PenSession,
    stroke: Vec<Sample>,
    /// The tolerance the brush fits with, in document units.
    tolerance: f64,
    /// The last committed drawn node, so the UI can address it (the Inspector's
    /// selection, the direct-selection tool's target).
    last_node: Option<NodeId>,
    /// The stroke's pointer-down time, for velocity.
    started_at: Option<f64>,
    /// The live direct-selection gesture, if one is open (RULE 4).
    edit: Option<EditSession>,
}

/// **An open direct-selection gesture** (Task 10.1 RULE 4).
///
/// Dragging an anchor or a Bézier handle is the same shape of interaction as
/// Task 3.2's node drag — a pointer moves something for a while, and the user
/// expects *one* undo to put it back — so it gets the same treatment: every
/// pointer sample is applied **untracked** (the document and the scene move
/// immediately, the overlay follows), and the gesture records a single history
/// entry when it ends.
///
/// The entry is a `SetPath` pair rather than a list of parameter writes, and that
/// is the whole trick: the path *before* the gesture is captured here, the path
/// *after* it is read off the document at the end, and those two are exact
/// inverses — so one undo restores the pre-drag geometry no matter how many
/// components the drag touched, how many samples the pointer produced, or what
/// the constraint solver did in between.
#[derive(Debug, Clone)]
pub struct EditSession {
    pub node_id: NodeId,
    /// The anchor slot being dragged, or (for a handle) the anchor that owns it.
    pub slot: String,
    /// `"in"` or `"out"` for a handle; ignored for an anchor.
    pub side: String,
    /// `true` when the anchor itself moves, `false` when a control point does.
    pub anchor: bool,
    /// The path as it was when the gesture began.
    before_start: Parameter<Point2>,
    before_segments: Vec<PathSegment>,
    /// Inverses the constraint pass applied during the gesture, to be folded into
    /// the recorded entry so undo reverts them too.
    inverses: Vec<Command>,
    /// How many pointer samples the gesture has taken.
    pub updates: usize,
}

impl DrawTools {
    pub fn new() -> Self {
        Self {
            tolerance: 0.35,
            ..Self::default()
        }
    }

    /// Clear both tools.
    //
    // The edit session is deliberately *not* cleared here: `begin` is called when
    // a tool session starts, and an open direct-selection gesture is closed by its
    // own `draw_edit_end` (which is what records the undo step). A gesture that
    // was abandoned mid-drag is cancelled explicitly, by `draw_edit_cancel`.
    pub fn begin(&mut self) {
        self.pen = PenSession::new();
        self.stroke.clear();
        self.started_at = None;
    }

    pub fn pen(&self) -> &PenSession {
        &self.pen
    }

    pub fn pen_mut(&mut self) -> &mut PenSession {
        &mut self.pen
    }

    pub fn stroke(&self) -> &[Sample] {
        &self.stroke
    }

    pub fn stroke_mut(&mut self) -> &mut Vec<Sample> {
        &mut self.stroke
    }

    pub fn set_last_node(&mut self, id: NodeId) {
        self.last_node = Some(id);
    }

    pub fn started_at(&self) -> Option<f64> {
        self.started_at
    }

    pub fn set_started_at(&mut self, time: f64) {
        self.started_at = Some(time);
    }

    /// The live direct-selection gesture, if any.
    pub fn edit(&self) -> Option<&EditSession> {
        self.edit.as_ref()
    }

    pub fn set_edit(&mut self, session: EditSession) {
        self.edit = Some(session);
    }

    pub fn take_edit(&mut self) -> Option<EditSession> {
        self.edit.take()
    }

    pub fn note_edit_update(&mut self) {
        if let Some(session) = &mut self.edit {
            session.updates += 1;
        }
    }

    /// Fold a constraint-pass inverse into the gesture's own entry.
    pub fn note_edit_inverse(&mut self, inverse: Command) {
        if let Some(session) = &mut self.edit {
            if !matches!(inverse, Command::Batch { ref commands } if commands.is_empty()) {
                session.inverses.push(inverse);
            }
        }
    }

    pub fn tolerance(&self) -> f64 {
        self.tolerance
    }

    pub fn set_tolerance(&mut self, tolerance: f64) {
        self.tolerance = tolerance.clamp(1e-3, 100.0);
    }
}

/// One anchor as the UI draws it: the point, its handles, and *where* it lives in
/// the document — `slot` is the address a command writes (`segments[2].to`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnchorWire {
    /// `"start"` or `"segments[i].to"`.
    pub slot: String,
    pub x: f64,
    pub y: f64,
    /// Handle arriving at this anchor, absolute, if any.
    pub handle_in: Option<[f64; 2]>,
    /// Handle leaving this anchor, absolute, if any.
    pub handle_out: Option<[f64; 2]>,
    /// Resolution of the anchor: `"literal"`, `"variable"`, `"expression"`, …
    /// The UI greys an anchored (non-literal) vertex, because a designer typing
    /// into an expression owns that number, not the mouse.
    pub source: String,
}

/// A draft, as the UI needs it: absolute points, in document space.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DraftWire {
    pub start: [f64; 2],
    /// Segment kinds in order: `"line"`, `"quadratic"`, `"cubic"`, `"close"`.
    pub kinds: Vec<String>,
    /// Every segment's points in slot order — the UI's flattening-free preview
    /// draws them as a polyline through the anchors plus its Bézier control
    /// points, which is what a pen preview is.
    pub points: Vec<[f64; 2]>,
    pub anchors: Vec<AnchorWire>,
    pub closed: bool,
    /// Is the path a candidate for Quick Shape snapping *right now*? The UI shows
    /// the hint; the hold timer decides whether to ask for the snap.
    pub snap_candidate: bool,
}

/// The reply to a drawing event.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DrawReply {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub draft: Option<DraftWire>,
    /// Samples collected so far (brush only), so the UI can taper its preview.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub samples: Option<Vec<[f64; 3]>>,
    /// Set by a commit: what the engine did.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub segments: Option<usize>,
    /// Fitting statistics, so the status line can say what the brush did.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fit_error: Option<f64>,
    /// Set by a Quick Shape snap: which primitive the stroke turned out to be.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snapped: Option<String>,
    /// How far the *input* stroke was from the primitive, in document units —
    /// the UI can honestly say "snapped a circle (±3.1 units)".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snap_error: Option<f64>,
    /// Whether the stroke drawn so far is a candidate for Quick Shape snapping.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snap_candidate: Option<bool>,
}

impl DrawReply {
    fn ok() -> Self {
        Self {
            ok: true,
            error: None,
            draft: None,
            samples: None,
            node_id: None,
            segments: None,
            fit_error: None,
            snapped: None,
            snap_error: None,
            snap_candidate: None,
        }
    }

    fn err(message: impl Into<String>) -> Self {
        Self {
            ok: false,
            error: Some(message.into()),
            draft: None,
            samples: None,
            node_id: None,
            segments: None,
            fit_error: None,
            snapped: None,
            snap_error: None,
            snap_candidate: None,
        }
    }

    fn to_json(&self) -> String {
        serde_json::to_string(self)
            .unwrap_or_else(|e| format!("{{\"ok\":false,\"error\":\"{e}\"}}"))
    }
}

/// A source tag for an anchor, from the parameter it holds.
fn source_of(param: &Parameter<Point2>) -> String {
    match param {
        Parameter::Literal(_) => "literal",
        Parameter::Variable(_) => "variable",
        Parameter::Expression(_) => "expression",
        Parameter::Animated(_) => "animated",
        Parameter::Procedural(_) => "procedural",
        Parameter::Interaction(_) => "interaction",
    }
    .to_string()
}

/// Absolute point of a literal parameter (non-literals report their raw value if
/// resolvable is not available — the UI only ever *draws* these).
fn point_of(param: &Parameter<Point2>) -> Point2 {
    match param {
        Parameter::Literal(point) => *point,
        // A bound point has no literal value to hand; the caller draws it from
        // the snapshot's resolved geometry instead. Reporting the origin keeps
        // the wire honest about "unknown" rather than inventing a number.
        _ => Point2::ZERO,
    }
}

/// Serialize a drafted path for the UI.
fn draft_wire(draft: &PathDraft, handles: &[Anchor], snap_candidate: bool) -> DraftWire {
    let mut kinds = Vec::with_capacity(draft.segments.len());
    let mut points = Vec::new();
    for segment in &draft.segments {
        match segment {
            PathSegment::Line { to } => {
                kinds.push("line".to_string());
                points.push([point_of(to).x, point_of(to).y]);
            }
            PathSegment::Quadratic { control, to } => {
                kinds.push("quadratic".to_string());
                points.push([point_of(control).x, point_of(control).y]);
                points.push([point_of(to).x, point_of(to).y]);
            }
            PathSegment::Cubic {
                control1,
                control2,
                to,
            } => {
                kinds.push("cubic".to_string());
                points.push([point_of(control1).x, point_of(control1).y]);
                points.push([point_of(control2).x, point_of(control2).y]);
                points.push([point_of(to).x, point_of(to).y]);
            }
            PathSegment::Close => kinds.push("close".to_string()),
        }
    }
    let anchors = handles
        .iter()
        .enumerate()
        .map(|(index, anchor)| AnchorWire {
            slot: if index == 0 {
                "start".to_string()
            } else {
                format!("segments[{}].to", index - 1)
            },
            x: anchor.point.x,
            y: anchor.point.y,
            handle_in: anchor.handle_in.map(|h| [h.x, h.y]),
            handle_out: anchor.handle_out.map(|h| [h.x, h.y]),
            source: "literal".to_string(),
        })
        .collect();
    DraftWire {
        start: [draft.start.x, draft.start.y],
        kinds,
        points,
        anchors,
        closed: draft.closed,
        snap_candidate,
    }
}

/// Turn a draft into the two commands that put it in the document.
///
/// A new path is `CreateNode` + `SetPath`; an existing one is `SetPath` alone.
/// Both are ordinary commands, so this adds no history machinery: `CreateNode`'s
/// inverse is `DeleteNode` and `SetPath`'s inverse is `SetPath` with the previous
/// geometry (`vectra_core::command`).
pub fn path_commands(
    node_id: NodeId,
    name: &str,
    draft: &PathDraft,
    existing: bool,
) -> Vec<Command> {
    let mut commands = Vec::with_capacity(2);
    if !existing {
        commands.push(Command::CreateNode {
            id: node_id,
            kind: NodeKind::Path {
                start: Parameter::Literal(draft.start),
                segments: draft.segments.clone(),
            },
            name: Some(name.to_string()),
            index: None,
        });
    } else {
        commands.push(Command::SetPath {
            id: node_id,
            start: Parameter::Literal(draft.start),
            segments: draft.segments.clone(),
        });
    }
    commands
}

/// **The drawing suite, across the wasm boundary** (Task 10.1).
///
/// The `#[wasm_bindgen]` attribute is the difference between "a method that
/// exists" and "a method JavaScript can call", and this block is the *only* thing
/// that puts the pen, the brush and the white arrow in front of a browser. The
/// signatures are deliberately plain — strings, numbers, booleans — so the UI
/// passes a pointer position and receives JSON, exactly as every other door into
/// the engine does (Task 1.4).
#[wasm_bindgen]
impl VectraEngine {
    /// **A pen or brush pointer event** (Task 10.1 RULE 2 / RULE 3).
    ///
    /// `tool` is `"pen"` or `"brush"`. For the pen, the gesture vocabulary is
    /// `vectra_draw::PenSession`'s, so `alt` breaks handle symmetry and `close`
    /// asks for the path to be joined to its first point. The reply carries the
    /// draft, so the UI's canvas has exactly the geometry the engine has — no
    /// interpolation, no duplicated maths.
    // The arity here is the JavaScript contract: one positional argument per
    // event field, because the caller is a `pointerdown`/`pointermove` handler
    // that has these values in hand and nothing to build a struct from.
    #[allow(clippy::too_many_arguments)]
    pub fn draw_pointer(
        &mut self,
        tool: &str,
        kind: &str,
        x: f64,
        y: f64,
        alt: bool,
        close: bool,
        time: f64,
    ) -> String {
        let point = Point2::new(x, y);
        let kind: PointerKind = match serde_json::from_value(serde_json::Value::String(kind.into()))
        {
            Ok(kind) => kind,
            Err(_) => {
                return DrawReply::err(format!(
                    "unknown pointer kind '{kind}' (down | move | up | cancel)"
                ))
                .to_json()
            }
        };
        match tool {
            "pen" => self.draw_pen(kind, point, alt, close),
            "brush" => self.draw_brush(kind, point, alt, time),
            other => DrawReply::err(format!("unknown tool '{other}' (pen | brush)")).to_json(),
        }
    }

    fn draw_pen(&mut self, kind: PointerKind, point: Point2, alt: bool, close: bool) -> String {
        // Everything that needs to *read* the session happens before the write,
        // so no borrow outlives the branch that used it.
        let radius = (self.draw_tools().tolerance() * 8.0).max(6.0);
        let closes = close && self.draw_tools().pen().closes_at(point, radius);

        // RULE 2's fourth gesture: a click on the first anchor closes the path
        // instead of stacking a duplicate point on top of it. Only the engine
        // knows where the anchors are, so only the engine can decide this.
        if closes && kind == PointerKind::Down {
            let draft = self.draw_tools().pen().close();
            return self.commit_draft(&draft, "drawn path");
        }
        match kind {
            PointerKind::Down => {
                self.draw_tools_mut().pen_mut().pointer_down(point);
            }
            PointerKind::Move => {
                self.draw_tools_mut().pen_mut().pointer_move(point, alt);
            }
            PointerKind::Up => {
                self.draw_tools_mut().pen_mut().pointer_up();
            }
            PointerKind::Cancel => {
                self.draw_tools_mut().begin();
                return DrawReply::ok().to_json();
            }
        }

        // The rubber band: the segment the user can *see* but has not placed.
        // The same function the commit path uses, so the preview and the result
        // cannot disagree about handle placement.
        let draft = self
            .draw_tools()
            .pen()
            .draft_with_preview(false, Some(point));
        let anchors = self.draw_tools().pen().anchors().to_vec();
        let candidate = self.pen_snap_candidate();
        let mut reply = DrawReply::ok();
        reply.draft = Some(draft_wire(&draft, &anchors, candidate));
        reply.to_json()
    }

    /// Is the pen's current draft a closed-enough loop for a Quick Shape snap?
    ///
    /// Read-only on purpose: the UI asks on every frame of a hold gesture, and a
    /// question must never be able to change the answer's own subject.
    fn pen_snap_candidate(&self) -> bool {
        let tools = self.draw_tools();
        let draft = tools.pen().draft(false);
        let points: Vec<Point2> = draft
            .segments
            .iter()
            .flat_map(|segment| {
                segment
                    .params()
                    .iter()
                    .map(|p| point_of(p))
                    .collect::<Vec<_>>()
            })
            .collect();
        if points.len() < 4 {
            return false;
        }
        vectra_draw::stroke::is_snap_candidate(&points, 40.0, 0.3, 0.9)
    }

    fn draw_brush(&mut self, kind: PointerKind, point: Point2, _alt: bool, time: f64) -> String {
        match kind {
            PointerKind::Down => {
                let tools = self.draw_tools_mut();
                tools.begin();
                tools.set_started_at(time);
                tools.stroke_mut().push(Sample::new(point, 0.0, Some(0.5)));
            }
            PointerKind::Move | PointerKind::Up => {
                let tools = self.draw_tools_mut();
                let started = tools.started_at().unwrap_or(time);
                tools
                    .stroke_mut()
                    .push(Sample::new(point, time - started, None));
            }
            PointerKind::Cancel => {
                self.draw_tools_mut().begin();
                return DrawReply::ok().to_json();
            }
        }
        let samples: Vec<[f64; 3]> = self
            .draw_tools()
            .stroke()
            .iter()
            .map(|sample| {
                [
                    sample.point.x,
                    sample.point.y,
                    sample.pressure.unwrap_or(0.5),
                ]
            })
            .collect();
        let candidate = self.brush_snap_candidate();
        let mut reply = DrawReply::ok();
        reply.samples = Some(samples);
        reply.snap_candidate = Some(candidate);
        reply.to_json()
    }

    /// Is the collected stroke a snap candidate? Asked on every event, answered
    /// from the cleaned polyline (`vectra_draw::stroke`), so the UI's hold timer
    /// has something cheap and honest to watch.
    fn brush_snap_candidate(&self) -> bool {
        let tools = self.draw_tools();
        let points: Vec<Point2> = tools.stroke().iter().map(|sample| sample.point).collect();
        let cleaned = vectra_draw::stroke::clean(&points_as_samples(&points), 0.5, 8.0);
        let cleaned_points: Vec<Point2> = cleaned.iter().map(|sample| sample.point).collect();
        vectra_draw::stroke::is_snap_candidate(&cleaned_points, 40.0, 0.25, 0.9)
    }

    /// **Commit the pen's draft** as a path node.
    ///
    /// `new_node` decides create-versus-rewrite: pass an existing node id to keep
    /// editing a shape (the direct-selection workflow), or `None` for a fresh
    /// drawn path. Returns the engine's own command response, so the UI's event
    /// log, undo stack and selection state behave exactly as for a hand-authored
    /// command.
    pub fn draw_pen_commit(&mut self, close: bool, node_id: Option<String>) -> String {
        let draft = if close {
            self.draw_tools().pen().close()
        } else {
            self.draw_tools().pen().finish()
        };
        match node_id {
            // Editing an existing path: the same draft, written *into* a node the
            // user already has selected — which is how the pen doubles as the
            // path editor (add points to a shape you snapped a minute ago).
            Some(id) => match id.parse::<NodeId>() {
                Ok(id) => self.rewrite_path(id, &draft),
                Err(_) => DrawReply::err("invalid node id").to_json(),
            },
            None => self.commit_draft(&draft, "drawn path"),
        }
    }

    /// Rewrite an existing path in place (`SetPath`), for the pen-as-editor flow.
    fn rewrite_path(&mut self, node_id: NodeId, draft: &PathDraft) -> String {
        let command = Command::SetPath {
            id: node_id,
            start: Parameter::Literal(draft.start),
            segments: draft.segments.clone(),
        };
        let Ok(json) = command.to_json() else {
            return DrawReply::err("could not serialize the path rewrite").to_json();
        };
        // `dispatch_command` answers in the *command* envelope (`{"status":…}`),
        // not the drawing one. Reading it as a `DrawReply` — as this code first
        // did — silently succeeded on every failure, because the fallback was a
        // bare `ok`. The two envelopes are deliberately different, so the bridge
        // between them is explicit here: a rejected `SetPath` (an id that is not a
        // path, a dead document) has to reach the UI as an error, or the pen looks
        // like it drew something the document never received.
        let response = self.dispatch_command(&json);
        if let Some(error) = dispatch_error(&response) {
            return DrawReply::err(error).to_json();
        }
        self.draw_tools_mut().set_last_node(node_id);
        let mut reply = DrawReply::ok();
        reply.node_id = Some(node_id.to_string());
        reply.segments = Some(draft.segments.len());
        reply.to_json()
    }

    /// **The tools' settings** — the brush's curve-fit tolerance and the pen's
    /// pick radius are *engine* decisions (they are compared against document
    /// geometry), so they are set here rather than in the UI.
    pub fn draw_set_tolerance(&mut self, tolerance: f64) -> String {
        self.draw_tools_mut().set_tolerance(tolerance);
        DrawReply::ok().to_json()
    }

    /// Commit the brush's stroke: fit it, expand it, and put it in the document.
    /// `_node_id` is accepted for symmetry with the pen's commit (rewriting an
    /// existing path); a brush gesture always produces a new node, because the
    /// stroke *is* the gesture — the workflow that edits an existing path is the
    /// direct-selection tool's.
    pub fn draw_brush_commit(&mut self, _node_id: Option<String>) -> String {
        let (samples, options) = {
            let tools = self.draw_tools();
            (
                tools.stroke().to_vec(),
                BrushOptions {
                    tolerance: tools.tolerance(),
                    ..BrushOptions::default()
                },
            )
        };
        if samples.len() < 2 {
            return DrawReply::err("a stroke needs at least two samples").to_json();
        }
        let stroke = brush_stroke(&samples, &options);
        let draft = PathDraft {
            start: stroke.start,
            segments: stroke.segments.clone(),
            closed: true,
        };
        self.draw_tools_mut().begin();
        let reply_json = self.commit_draft(&draft, "brush stroke");
        // Amend the commit reply with the fit's own statistics, so the UI can
        // report what the brush did without measuring anything itself.
        let mut reply: DrawReply =
            serde_json::from_str(&reply_json).unwrap_or_else(|_| DrawReply::ok());
        reply.fit_error = Some(stroke.fit.max_error);
        reply.segments = Some(stroke.fit.segments);
        reply.to_json()
    }

    /// **Quick Shape snapping** (RULE 3).
    ///
    /// The whole operation, in one call, because it must be atomic from the user's
    /// point of view — they held still for a moment and their blob became a
    /// circle:
    ///
    /// 1. `plan_quick_shape` recognizes the primitive and fits it;
    /// 2. the path is rewritten to the ideal geometry (`SetPath`);
    /// 3. the primitive node is created (`CreateNode`), so the artwork carries a
    ///    real `Circle`/`Rectangle` — which the SVG exporter turns into `<circle>`
    ///    (Task 8.0 RULE 1) rather than a path;
    /// 4. the constraint rows are added (`AddConstraint`), which is where the
    ///    Task 3.1 solver runs: every `AddConstraint` goes through
    ///    `dispatch_command`'s constraint pass, so the snaps are enforced by the
    ///    same tableau as any hand-authored rule.
    ///
    /// `stroke` is the rough input — the UI's cleaned sample points for a brush
    /// gesture, or the drawn anchors for a pen path. Returns a reply naming the
    /// primitive and the input's own distance from it.
    pub fn draw_quick_shape(&mut self, node_id: &str, stroke_json: &str) -> String {
        let points: Vec<[f64; 2]> = match serde_json::from_str(stroke_json) {
            Ok(points) => points,
            Err(e) => return DrawReply::err(format!("invalid stroke: {e}")).to_json(),
        };
        let stroke: Vec<Point2> = points.iter().map(|p| Point2::new(p[0], p[1])).collect();
        let node_id = match node_id.parse::<NodeId>() {
            Ok(id) => id,
            Err(_) => return DrawReply::err("invalid node id").to_json(),
        };
        // **How rough may a hand be?** The tolerance is what decides whether a
        // stroke is recognised as a primitive at all (`recognize` scores the
        // fraction of samples within it), so it is the one number that makes
        // Quick Shape feel psychic or useless — and it must be
        // **scale-relative**, because roughness is: an 8-unit wobble on a
        // 100-unit circle is a rough circle, while the same 8 units on a
        // 1000-unit circle is precision work. So the tolerance is the brush's own
        // setting, widened by a fraction of the stroke's own size.
        let tolerance = snap_tolerance(self.draw_tools().tolerance(), &stroke);
        let primitive_id = new_node_id();
        let Some(plan) = plan_quick_shape(&stroke, node_id, primitive_id, tolerance) else {
            return DrawReply::err("the stroke is not a circle or a rectangle").to_json();
        };

        // ── the path itself, then the primitive it is tied to ───────────────
        let node_name = format!("snapped {}", plan.kind.tag());
        let mut response = String::new();
        let commands: Vec<Command> = vec![
            Command::SetPath {
                id: node_id,
                start: Parameter::Literal(plan.start),
                segments: plan.segments.clone(),
            },
            Command::CreateNode {
                id: primitive_id,
                kind: plan.primitive.clone(),
                name: Some(node_name),
                index: None,
            },
        ];
        // The rows last: each `AddConstraint` carries a solver pass, and the
        // anchors are `STRONG` hints (the slots the user just drew), so the
        // tableau solves the system *around* what they drew rather than from
        // scratch.
        let mut commands = commands;
        commands.extend(constraint_commands(&plan.constraints));
        for command in &commands {
            let Ok(json) = command.to_json() else {
                return DrawReply::err("could not serialize the snap command").to_json();
            };
            response = self.dispatch_command(&json);
            if dispatch_failed(&response) {
                return response;
            }
        }

        self.draw_tools_mut().set_last_node(node_id);
        let mut reply: DrawReply =
            serde_json::from_str(&response).unwrap_or_else(|_| DrawReply::ok());
        reply.ok = true;
        reply.error = None;
        reply.node_id = Some(node_id.to_string());
        reply.snapped = Some(plan.kind.tag().to_string());
        reply.snap_error = Some(plan.recognition.error);
        reply.segments = Some(plan.segments.len());
        reply.to_json()
    }

    /// The shared commit path: dispatch the commands a draft implies, then answer
    /// with the engine's response plus the node's id.
    fn commit_draft(&mut self, draft: &PathDraft, name: &str) -> String {
        if draft.segments.is_empty() {
            return DrawReply::err("nothing to commit: the path has no segments").to_json();
        }
        let node_id = new_node_id();
        let mut response = String::new();
        for command in path_commands(node_id, name, draft, false) {
            let Ok(json) = command.to_json() else {
                return DrawReply::err("could not serialize the path command").to_json();
            };
            response = self.dispatch_command(&json);
            if dispatch_failed(&response) {
                return response;
            }
        }
        self.draw_tools_mut().set_last_node(node_id);
        let mut reply: DrawReply =
            serde_json::from_str(&response).unwrap_or_else(|_| DrawReply::ok());
        reply.ok = true;
        reply.error = None;
        reply.node_id = Some(node_id.to_string());
        reply.segments = Some(draft.segments.len());
        reply.to_json()
    }

    /// **The direct-selection hit test** (RULE 4).
    ///
    /// Given a pointer position *in client space* and the renderer that owns the
    /// camera, returns the nearest anchor or handle of the addressed path, with
    /// the slot a command would write. The radius is in screen pixels and
    /// converted to document units through the camera, so a handle stays as easy
    /// to grab at 4× zoom as at 1× — which is the whole point of hitting in screen
    /// space rather than in the document.
    ///
    /// Anchors win over handles at equal distance: the anchor is what a designer
    /// means when they click "here".
    pub fn draw_hit(
        &self,
        renderer: &crate::render::Renderer,
        node_id: &str,
        x: f64,
        y: f64,
    ) -> String {
        let Ok(node_id) = node_id.parse::<NodeId>() else {
            return DrawReply::err("invalid node id").to_json();
        };
        let Some((doc_x, doc_y)) = renderer.client_to_document(x, y) else {
            return DrawReply::err("the canvas has no drawable area yet").to_json();
        };
        let Ok(node) = self.core.document().get_node(node_id) else {
            return DrawReply::err("no such node").to_json();
        };
        let NodeKind::Path { .. } = &node.kind else {
            return DrawReply::err("direct selection addresses a path").to_json();
        };
        // The grab radius in document units: the camera's *scale* at this
        // viewport, so the grab stays 12 screen pixels at any zoom, pan or
        // devicePixelRatio. Inverted from `Renderer::view`'s own report of the
        // camera rather than recomputed, which is what keeps the overlay and the
        // scene from disagreeing.
        let scale = renderer.pixels_per_unit().unwrap_or(1.0);
        let radius = 12.0 / scale.max(f64::MIN_POSITIVE);
        let target = Point2::new(doc_x, doc_y);

        #[derive(Serialize)]
        struct Hit {
            kind: &'static str,
            slot: String,
            side: Option<&'static str>,
            index: usize,
            distance: f64,
            x: f64,
            y: f64,
        }

        let mut best: Option<Hit> = None;
        let mut consider = |kind: &'static str,
                            slot: String,
                            side: Option<&'static str>,
                            index: usize,
                            point: Point2,
                            priority: f64| {
            let distance = vectra_draw::bezier::distance(point, target) * priority;
            if distance > radius {
                return;
            }
            match &best {
                Some(current) if current.distance <= distance => {}
                _ => {
                    best = Some(Hit {
                        kind,
                        slot,
                        side,
                        index,
                        distance,
                        x: point.x,
                        y: point.y,
                    })
                }
            }
        };

        if let Some(start) = node.path_param("start") {
            consider("anchor", "start".into(), None, 0, point_of(start), 1.0);
        }
        for (index, segment) in segment_anchor_slots(&node.kind).iter().enumerate() {
            let _ = index;
            let (slot, _priority) = segment;
            if let Some(point) = node.path_param(slot) {
                consider("anchor", slot.clone(), None, index, point_of(point), 1.0);
            }
            if let Some(control) = control_slot_of(node, slot) {
                if let Some(point) = node.path_param(&control) {
                    consider("handle", control, Some("out"), index, point_of(point), 1.2);
                }
            }
        }

        match best {
            Some(hit) => serde_json::to_string(&hit).unwrap_or_else(|_| "{}".into()),
            // An empty object is the honest answer for "nothing under the pointer".
            None => "{\"kind\":\"miss\"}".to_string(),
        }
    }

    /// **The command that moves an anchor or handle** (RULE 4).
    ///
    /// Returned rather than dispatched so the UI can put it through the *same*
    /// `dispatch_command` door as everything else (undo, events, dirty nodes,
    /// renderer updates all come for free) — and so the command log in the UI
    /// shows the drag, which is the Task 1.4 rule taken seriously.
    ///
    /// ## Symmetry, in the engine
    ///
    /// Dragging a *smooth* anchor's handle must move the opposite handle too, and
    /// an Alt-drag must not (RULE 2). That decision is `vectra_draw::pen::drag_handle`
    /// — the very function the Handle Symmetry and Handle Independence laws
    /// exercise — and the result is one `Batch`, so the whole symmetric edit is a
    /// single undo entry.
    /// **The command that moves an anchor or handle** (RULE 4).
    ///
    /// Returned rather than dispatched so the UI can put it through the *same*
    /// `dispatch_command` door as everything else (undo, events, dirty nodes,
    /// renderer updates all come for free) — and so the command log in the UI
    /// shows the drag, which is the Task 1.4 rule taken seriously.
    ///
    /// It is the stateless form of the same edit `draw_edit_update` applies during
    /// a gesture: both go through [`edit_batch`], so a handle drag the gesture
    /// previews and one the UI dispatches by hand cannot differ.
    ///
    /// ## Symmetry, in the engine
    ///
    /// Dragging a *smooth* anchor's handle must move the opposite handle too, and
    /// an Alt-drag must not (RULE 2). That decision is `vectra_draw::pen::drag_handle`
    /// — the very function the Handle Symmetry and Handle Independence laws
    /// exercise — and the result is one `Batch`, so the whole symmetric edit is a
    /// single undo entry.
    #[allow(clippy::too_many_arguments)]
    pub fn draw_edit_command(
        &self,
        node_id: &str,
        slot: &str,
        side: &str,
        x: f64,
        y: f64,
        alt: bool,
        anchor: bool,
    ) -> String {
        let Ok(node_id) = node_id.parse::<NodeId>() else {
            return CommandResponse::err_json("invalid node id");
        };
        let Ok(node) = self.core.document().get_node(node_id) else {
            return CommandResponse::err_json("no such node");
        };
        match edit_batch(node, node_id, slot, side, Point2::new(x, y), alt, anchor) {
            Ok(command) => command
                .to_json()
                .unwrap_or_else(|e| CommandResponse::err_json(e.to_string())),
            Err(message) => CommandResponse::err_json(message),
        }
    }

    /// **Begin a direct-selection drag** (RULE 4): the path is about to be edited.
    ///
    /// Opens a gesture that owns the path until `draw_edit_end`. These three
    /// methods are the anchor/handle counterpart of Task 3.2's drag triad, and
    /// they exist for the same reason: *one gesture is one undo step*.
    /// `draw_edit_command` alone would push a history entry per pointer sample, so
    /// dragging an anchor across the canvas would take two hundred undos to take
    /// back.
    pub fn draw_edit_begin(
        &mut self,
        node_id: &str,
        slot: &str,
        side: &str,
        anchor: bool,
    ) -> String {
        if self.draw_tools().edit().is_some() {
            return CommandResponse::err_json("a direct-selection drag is already in progress");
        }
        let Ok(node_id) = node_id.parse::<NodeId>() else {
            return CommandResponse::err_json("invalid node id");
        };
        let Ok(node) = self.core.document().get_node(node_id) else {
            return CommandResponse::err_json("no such node");
        };
        let NodeKind::Path { start, segments } = &node.kind else {
            return CommandResponse::err_json("direct selection addresses a path");
        };
        // Fail *before* the gesture opens: a slot that does not resolve would
        // otherwise be discovered on the first sample, halfway through a drag.
        if node.path_param(slot).is_none() {
            return CommandResponse::err_json(format!("no such anchor: {slot}"));
        }
        let session = EditSession {
            node_id,
            slot: slot.to_string(),
            side: side.to_string(),
            anchor,
            before_start: start.clone(),
            before_segments: segments.clone(),
            inverses: Vec::new(),
            updates: 0,
        };
        self.draw_tools_mut().set_edit(session);
        CommandResponse::ok_json(vec![EngineEvent::NodesUpdated { ids: vec![node_id] }])
    }

    /// **One sample of a direct-selection drag**, applied and settled.
    ///
    /// Nothing is pushed onto the undo stack here — the gesture is one step, and
    /// `draw_edit_end` records it (see [`EditSession`]). What *does* happen here is
    /// everything else a user can see: the parameter writes, the constraint pass
    /// (a `Required` row stronger than the pointer wins, exactly as in Task 3.1),
    /// the dirty set and the scene patch.
    pub fn draw_edit_update(&mut self, x: f64, y: f64, alt: bool) -> String {
        let Some((node_id, slot, side, anchor)) = self.draw_tools().edit().map(|edit| {
            (
                edit.node_id,
                edit.slot.clone(),
                edit.side.clone(),
                edit.anchor,
            )
        }) else {
            return CommandResponse::err_json("no direct-selection drag in progress");
        };
        let command = {
            let Ok(node) = self.core.document().get_node(node_id) else {
                return CommandResponse::err_json("no such node");
            };
            match edit_batch(node, node_id, &slot, &side, Point2::new(x, y), alt, anchor) {
                Ok(command) => command,
                Err(message) => return CommandResponse::err_json(message),
            }
        };
        let inverse = match self.core.apply_untracked(command.clone()) {
            Ok(inverse) => inverse,
            Err(error) => return CommandResponse::err_json(error.to_string()),
        };
        let mut events = vec![EngineEvent::NodesUpdated { ids: vec![node_id] }];
        match self.constraint_pass(&command, false) {
            Ok(pass) => {
                self.solver_diagnostics = pass.diagnostics;
                for inverse in pass.inverses {
                    self.draw_tools_mut().note_edit_inverse(inverse);
                }
                events.extend(pass.events);
            }
            Err(error) => {
                // The rules refuse this position. The sample is dropped rather
                // than applied — the gesture stays open, the geometry stays where
                // it was, and the UI is told why. That is what a hard rule means.
                let _ = self.core.apply_untracked(inverse);
                self.solver_diagnostics.clear();
                return CommandResponse::err_json(crate::constraint_error_message(error));
            }
        }
        self.draw_tools_mut().note_edit_update();
        let settled = self.settle(events);
        CommandResponse::ok_json(settled)
    }

    /// **Finish a direct-selection drag**: one `Set path` entry, exact in both
    /// directions — and it reverts everything the solver did for the gesture too.
    ///
    /// The entry is a `SetPath` pair rather than a list of parameter writes, and
    /// that is the whole trick: the path before the gesture was captured when it
    /// opened, the path after it is read off the document now, and the two are
    /// exact inverses — so **one undo gets the shape back** no matter how many
    /// components the drag touched, how many samples the pointer produced, or what
    /// the constraint solver did in between.
    pub fn draw_edit_end(&mut self) -> String {
        let Some(session) = self.draw_tools_mut().take_edit() else {
            return CommandResponse::err_json("no direct-selection drag in progress");
        };
        let node_id = session.node_id;
        if session.updates == 0 {
            // A click with no movement: there is nothing to undo, so nothing is
            // recorded. (A no-op entry would put a useless step in the user's
            // history for every click of the white arrow.)
            let settled = self.settle(vec![EngineEvent::NodesUpdated { ids: vec![node_id] }]);
            return CommandResponse::ok_json(settled);
        }
        let Ok(node) = self.core.document().get_node(node_id) else {
            return CommandResponse::err_json("the edited node is gone");
        };
        let NodeKind::Path { start, segments } = &node.kind else {
            return CommandResponse::err_json("direct selection addresses a path");
        };
        let (after_start, after_segments) = (start.clone(), segments.clone());
        let events = self.core.record(
            Command::SetPath {
                id: node_id,
                start: after_start,
                segments: after_segments,
            },
            Command::SetPath {
                id: node_id,
                start: session.before_start,
                segments: session.before_segments,
            },
            "Edit anchor",
        );
        // The solver's own adjustments belong to the gesture (Task 3.1: one user
        // action, one undo).
        if !session.inverses.is_empty() {
            // The solver's writes are folded into the gesture's own entry, so
            // undoing "Edit anchor" also takes back what the solver did.
            self.core.amend_top_backward(Command::Batch {
                commands: session.inverses,
            });
        }
        let settled = self.settle(vec![
            events,
            EngineEvent::NodesUpdated { ids: vec![node_id] },
        ]);
        CommandResponse::ok_json(settled)
    }

    /// Abandon a direct-selection drag, restoring the path it started from.
    pub fn draw_edit_cancel(&mut self) -> String {
        let Some(session) = self.draw_tools_mut().take_edit() else {
            return CommandResponse::ok_json(Vec::new());
        };
        let node_id = session.node_id;
        if session.updates == 0 {
            return CommandResponse::ok_json(Vec::new());
        }
        let _ = self.core.apply_untracked(Command::SetPath {
            id: node_id,
            start: session.before_start,
            segments: session.before_segments,
        });
        let settled = self.settle(vec![EngineEvent::NodesUpdated { ids: vec![node_id] }]);
        CommandResponse::ok_json(settled)
    }

    /// The anchors and handles of a path, for the direct-selection overlay.
    ///
    /// Engine data, in document space: the UI hands it to the renderer's own
    /// camera to place it on screen, so an overlay can never disagree with the
    /// geometry it decorates.
    pub fn draw_overlay(&self, node_id: &str) -> String {
        let Ok(node_id) = node_id.parse::<NodeId>() else {
            return DrawReply::err("invalid node id").to_json();
        };
        let Ok(node) = self.core.document().get_node(node_id) else {
            return DrawReply::err("no such node").to_json();
        };
        let NodeKind::Path { .. } = &node.kind else {
            return "{\"ok\":true,\"anchors\":[]}".to_string();
        };
        let mut anchors: Vec<AnchorWire> = Vec::new();
        if let Some(start) = node.path_param("start") {
            anchors.push(anchor_wire("start".into(), start, None, node, false));
        }
        for (index, slot) in segment_anchor_slots(&node.kind).iter().enumerate() {
            let (slot, _) = slot;
            if let Some(param) = node.path_param(slot) {
                let incoming = previous_control_slot(node, slot)
                    .and_then(|control| node.path_param(&control))
                    .map(point_of);
                anchors.push(anchor_wire(slot.clone(), param, incoming, node, true));
            }
            let _ = index;
        }
        serde_json::json!({ "ok": true, "anchors": anchors }).to_string()
    }

    /// Read the drawing session (no side effects — see `pen_snap_candidate`).
    fn draw_tools(&self) -> &DrawTools {
        &self.draw
    }

    /// Mutate the drawing session (only a pointer event may).
    fn draw_tools_mut(&mut self) -> &mut DrawTools {
        &mut self.draw
    }
}

/// **The tolerance a Quick Shape snap recognises with** (RULE 3).
///
/// `brush` is the tools' own setting (a curve-fit tolerance in document units);
/// the result is the larger of it and `SNAP_SCALE_SLOP` × the stroke's diagonal.
/// The diagonal rather than, say, the radius because the recogniser does not know
/// what the stroke *is* yet — it is deciding exactly that — and a bounding-box
/// diagonal is the honest measure of "how big is this thing" for a circle, a box
/// or neither.
///
/// The floor matters as much as the slope: a tiny squiggle must not snap to a
/// micro-circle, so the brush's own minimum (`tolerance × 4`, which is what this
/// used before scale entered the picture) is kept as the base.
fn snap_tolerance(brush: f64, stroke: &[Point2]) -> f64 {
    let base = brush.max(1.0) * 4.0;
    let (mut min_x, mut min_y, mut max_x, mut max_y) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for point in stroke {
        min_x = min_x.min(point.x);
        min_y = min_y.min(point.y);
        max_x = max_x.max(point.x);
        max_y = max_y.max(point.y);
    }
    if !(min_x <= max_x && min_y <= max_y) {
        return base;
    }
    let diagonal = ((max_x - min_x).powi(2) + (max_y - min_y).powi(2)).sqrt();
    base.max(diagonal * SNAP_SCALE_SLOP)
}

/// The share of a stroke's own size a hand may miss by and still be recognised:
/// 12 % of the bounding-box diagonal, which is ≈17 % of a circle's radius.
const SNAP_SCALE_SLOP: f64 = 0.12;

/// Compose the `AnchorWire` for one anchor slot.
fn anchor_wire(
    slot: String,
    param: &Parameter<Point2>,
    handle_in: Option<Point2>,
    node: &vectra_core::Node,
    _interior: bool,
) -> AnchorWire {
    let point = point_of(param);
    let handle_out =
        control_slot_of(node, &slot).and_then(|control| node.path_param(&control).map(point_of));
    AnchorWire {
        slot,
        x: point.x,
        y: point.y,
        handle_in: handle_in.map(|h| [h.x, h.y]),
        handle_out: handle_out.map(|h| [h.x, h.y]),
        source: source_of(param),
    }
}

/// One component write — `SetParameter { property: "segments[2].to.x", … }`.
///
/// The property-string form is the document's own component addressing
/// (`vectra_core::document`), the same one the constraint solver uses, which is
/// why a direct-selection drag is an ordinary parameter write rather than a new
/// command.
fn component_command(node_id: NodeId, slot: &str, axis: &str, value: f64) -> Command {
    Command::SetParameter {
        node_id,
        property: format!("{slot}.{axis}"),
        value: ParamValue::Float(Parameter::Literal(value)),
    }
}

/// The anchor slots of a path, in order (`segments[i].to`).
fn segment_anchor_slots(kind: &NodeKind) -> Vec<(String, f64)> {
    let NodeKind::Path { segments, .. } = kind else {
        return Vec::new();
    };
    (0..segments.len())
        .map(|index| (format!("segments[{index}].to"), 1.0))
        .collect()
}

/// `segments[2].to` → `segments[2].control2` (the handle *arriving* at it).
fn previous_control_slot(node: &vectra_core::Node, slot: &str) -> Option<String> {
    segment_control(node, anchor_index(slot)?, Control::Arriving)
}

/// `segments[2].to` → `segments[3].control1`…
///
/// The outgoing handle of an anchor belongs to the *next* segment, which is the
/// asymmetry in the engine's own model (`PathSegment` stores the handle arriving
/// at its endpoint, plus the one leaving its start). Getting this wrong is
/// invisible in a straight path and wrong in every curve, so it lives in one
/// place with the arithmetic spelled out: for a segment `i`, `control1` curves
/// out of `segments[i-1].to` (or `start`), and `control2` curves into
/// `segments[i].to`.
fn control_slot_of(node: &vectra_core::Node, slot: &str) -> Option<String> {
    if slot == "start" {
        // The start point's outgoing handle is the first segment's leaving control.
        return segment_control(node, 0, Control::Leaving);
    }
    // The route matters more than the number: a single-closed-relative indexing
    // scheme is exactly how an off-by-one turns a smooth join into a kink.
    segment_control(node, anchor_index(slot)? + 1, Control::Leaving)
}

/// `segments[2].to` → `2`.
fn anchor_index(slot: &str) -> Option<usize> {
    let (base, field) = slot.split_once('.')?;
    if field != "to" {
        return None;
    }
    base.strip_prefix("segments[")?
        .strip_suffix(']')?
        .parse()
        .ok()
}

/// Which end of a segment a control point belongs to.
#[derive(Clone, Copy)]
enum Control {
    /// The handle leaving the segment's start point.
    Leaving,
    /// The handle arriving at the segment's end point.
    Arriving,
}

/// The slot of a segment's control point, **by segment kind**.
///
/// This is the whole of RULE 1's "three kinds, three shapes" in one match:
///
/// * `Line` has no control point, so neither side resolves;
/// * `Cubic` has `control1` leaving and `control2` arriving — the distinct pair
///   RULE 1 asks about;
/// * `Quadratic` has a **single** `control` that is *both*, which is not a
///   degenerate cubic but the quadratic's own definition, and is why a
///   quadratic's two handle arms are the same point.
///
/// The pen and the brush only ever emit `Line`/`Cubic`, so a quadratic only
/// reaches the canvas from an AI command or an imported document — precisely the
/// path nobody clicks by hand, and therefore precisely the path where a
/// hard-coded `control1` would have stayed broken.
fn segment_control(node: &vectra_core::Node, index: usize, side: Control) -> Option<String> {
    let NodeKind::Path { segments, .. } = &node.kind else {
        return None;
    };
    let field = match (segments.get(index)?, side) {
        (PathSegment::Cubic { .. }, Control::Leaving) => "control1",
        (PathSegment::Cubic { .. }, Control::Arriving) => "control2",
        (PathSegment::Quadratic { .. }, _) => "control",
        _ => return None,
    };
    Some(format!("segments[{index}].{field}"))
}

/// Did a `dispatch_command` reply report an error?
///
/// The boundary's envelope is `{"status":"ok"|"error", …}` (`CommandResponse`),
/// so every multi-command operation here has to check it before continuing —
/// a snap that added six constraints and then failed on the seventh would leave
/// a half-applied shape, which is exactly the state the engine's own gates exist
/// to make impossible.
fn dispatch_failed(response: &str) -> bool {
    dispatch_error(response).is_some()
}

/// The message from a `{"status":"error","message":…}` command envelope, if the
/// response is one — and the reason a response that cannot be parsed counts as a
/// failure (an unreadable reply is not a success).
fn dispatch_error(response: &str) -> Option<String> {
    let parsed: serde_json::Value = serde_json::from_str(response).ok()?;
    if parsed.get("status").and_then(|status| status.as_str()) != Some("error") {
        return None;
    }
    Some(
        parsed
            .get("message")
            .and_then(|message| message.as_str())
            .unwrap_or("the engine rejected the command")
            .to_string(),
    )
}

/// Samples from bare points — the bridge the snap candidate check needs.
fn points_as_samples(points: &[Point2]) -> Vec<Sample> {
    points
        .iter()
        .enumerate()
        .map(|(index, point)| Sample::new(*point, index as f64 * 0.01, None))
        .collect()
}

/// Constraints a plan carries, as commands — split out so the tests can assert
/// "the snap added N rules" without reaching into the plan.
pub fn constraint_commands(constraints: &[Constraint]) -> Vec<Command> {
    constraints
        .iter()
        .cloned()
        .map(|constraint| Command::AddConstraint { constraint })
        .collect()
}

/// **The command for one anchor-or-handle edit** (RULE 4).
///
/// Two things happen here and both are the engine's, not the UI's:
///
/// 1. **What the edit addresses.** An *anchor* drag moves the point and
///    translates the handles attached to it — a translation, not a reshape: the
///    curve through the anchor keeps its tangent and its tension, which is what a
///    designer means by moving a point. A *handle* drag moves one control point.
/// 2. **Symmetry.** For a handle drag the opposite handle is mirrored through
///    `vectra_draw::pen::drag_handle`, unless `alt` is held — RULE 2's
///    "Alt/Option + drag breaks the symmetry" — and the whole edit is one `Batch`,
///    so it is one history entry.
///
/// The two handle slots are the engine's own asymmetry, spelled out once in
/// `control_slot_of`/`previous_control_slot`: for a segment `i`, `control1` curves
/// *out of* `segments[i-1].to` (or `start`) and `control2` curves *into*
/// `segments[i].to`. The UI passes a slot and a side; the arithmetic stays here.
fn edit_batch(
    node: &vectra_core::Node,
    node_id: NodeId,
    slot: &str,
    side: &str,
    target: Point2,
    alt: bool,
    anchor: bool,
) -> Result<Command, String> {
    let NodeKind::Path { .. } = &node.kind else {
        return Err("direct selection addresses a path".to_string());
    };
    // ── an anchor drag: move the point, and the handles travel with it ──
    if anchor {
        let Some(current) = node.path_param(slot) else {
            return Err(format!("no such anchor: {slot}"));
        };
        let from = point_of(current);
        let delta = Point2::new(target.x - from.x, target.y - from.y);
        let mut commands = vec![
            component_command(node_id, slot, "x", target.x),
            component_command(node_id, slot, "y", target.y),
        ];
        // The handles attached to this anchor are *offsets* from it, so they
        // move by the same delta: dragging an anchor must not silently change
        // the shape of the curve through it (that is what handle drags are
        // for), it must translate it.
        if let Some(control) = control_slot_of(node, slot) {
            if let Some(param) = node.path_param(&control) {
                let control_point = point_of(param);
                commands.push(component_command(
                    node_id,
                    &control,
                    "x",
                    control_point.x + delta.x,
                ));
                commands.push(component_command(
                    node_id,
                    &control,
                    "y",
                    control_point.y + delta.y,
                ));
            }
        }
        if let Some(previous) = previous_control_slot(node, slot) {
            if let Some(param) = node.path_param(&previous) {
                let control_point = point_of(param);
                commands.push(component_command(
                    node_id,
                    &previous,
                    "x",
                    control_point.x + delta.x,
                ));
                commands.push(component_command(
                    node_id,
                    &previous,
                    "y",
                    control_point.y + delta.y,
                ));
            }
        }
        return Ok(Command::batch_commands(commands));
    }

    // ── a handle drag: the symmetry algebra, then one batch ─────────────
    let handle_slot = match side {
        "in" => previous_control_slot(node, slot),
        _ => control_slot_of(node, slot),
    };
    let Some(handle_slot) = handle_slot else {
        return Err(format!("{slot} has no {side} handle"));
    };
    let Some(param) = node.path_param(&handle_slot) else {
        return Err(format!("no such handle: {handle_slot}"));
    };
    let anchor_point = node.path_param(slot).map(point_of).unwrap_or(Point2::ZERO);
    let (handle_in, handle_out) = match side {
        "in" => {
            let out = node
                .path_param(&control_slot_of(node, slot).unwrap_or_default())
                .map(point_of);
            (Some(point_of(param)), out)
        }
        _ => {
            let incoming = previous_control_slot(node, slot)
                .and_then(|slot| node.path_param(&slot))
                .map(point_of);
            (incoming, Some(point_of(param)))
        }
    };
    let (new_in, new_out) = vectra_draw::pen::drag_handle(
        anchor_point,
        handle_in,
        handle_out,
        if side == "in" {
            HandleSide::In
        } else {
            HandleSide::Out
        },
        target,
        alt,
    );
    let mut commands = vec![
        component_command(node_id, &handle_slot, "x", target.x),
        component_command(node_id, &handle_slot, "y", target.y),
    ];
    // The mirrored partner: the *other* side of the same anchor.
    // The mirror belongs to the **anchor**, not to the segment kind: the arm
    // leaving `slot` lives on the next segment and the arm arriving at it lives
    // on this one, so they are always two different fields — including when one
    // of them is a quadratic's `control`, which is one point shared by *both of
    // that segment's* anchors rather than both arms of a single anchor. Dragging
    // a quadratic's arm therefore smooths the joint exactly as a cubic's would,
    // and Alt frees it exactly as it frees a cubic's.
    let partner = match side {
        "in" => control_slot_of(node, slot),
        _ => previous_control_slot(node, slot),
    };
    if let Some(partner) = partner {
        let mirrored = match side {
            "in" => new_out,
            _ => new_in,
        };
        // The partner, **only if it actually moves**. With Alt held the
        // symmetry is broken and the partner is already where the edit wants
        // it, so writing it would put a no-op in the user's history — and it
        // is precisely this omission that makes the Handle Independence Law
        // checkable at the boundary: *an Alt-drag writes the dragged handle
        // and nothing else*.
        if let Some(mirrored) = mirrored {
            if let Some(current) = node.path_param(&partner) {
                let current = point_of(current);
                let moved = (current.x - mirrored.x).abs() > f64::EPSILON
                    || (current.y - mirrored.y).abs() > f64::EPSILON;
                if moved {
                    commands.push(component_command(node_id, &partner, "x", mirrored.x));
                    commands.push(component_command(node_id, &partner, "y", mirrored.y));
                }
            }
        }
    }
    Ok(Command::batch_commands(commands))
}
