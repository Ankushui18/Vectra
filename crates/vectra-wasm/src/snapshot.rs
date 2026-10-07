//! Serializable snapshot projection (Tasks 1.4 + 2.2).
//!
//! [`SnapshotResponse`] is the UI's entire world: evaluated drawables (paths
//! as SVG data so any host can preview without a tessellator), variable
//! values, expression sources, diagnostics, dependency-graph summary,
//! evaluation statistics, undo state, and engine time. The React remote
//! control renders *only* this — it holds no document, resolves no parameters.
//!
//! Since Task 2.2 the scene is not evaluated here: the engine keeps an
//! [`IncrementalScene`](vectra_dependency::IncrementalScene) that is patched at
//! mutation time, and this projection simply reads it. That is what makes
//! `get_snapshot` a cheap, side-effect-free read and what makes the `eval`
//! block meaningful (`full_evals` stays at 1 while edits stay incremental).
//!
//! The projection is **canonical**: every map crosses the wire sorted (scene
//! keys by id, variables by name, expressions by id) and diagnostics are
//! ordered by (severity, code, node, message). Evaluation order must never be
//! observable across the boundary — a patched cache and a rebuilt one hold the
//! same scene but visit it in different orders, and hash-map iteration order
//! would otherwise leak that difference into the JSON. Canonicalization is what
//! lets the UI (and `scripts/smoke.mjs`) assert `patch ≡ rebuild` byte for
//! byte.

use serde::Serialize;
use std::collections::BTreeMap;
use vectra_constraints::SolverStats;
use vectra_core::layers::ArtboardRecord;
use vectra_core::{ids::LayerId, Constraint, Engine, NodeId};
#[cfg(test)]
use vectra_core::{NodeKind, Resolvable};
use vectra_dependency::GraphSummary;
use vectra_geometry::{
    path_to_svg_data, Diagnostic, EvaluatedAppearance, EvaluatedPaint, EvaluatedPrimitive,
    EvaluatedScene, EvaluatedStyle,
};

/// Wire shape of one evaluated primitive. Structurally mirrors
/// [`EvaluatedPrimitive`], except `Path`, which crosses the boundary as SVG
/// path data (document space, y-up) instead of a lyon object.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum SnapshotPrimitive {
    Rect {
        x: f64,
        y: f64,
        w: f64,
        h: f64,
        corner_radius: f64,
    },
    Circle {
        cx: f64,
        cy: f64,
        r: f64,
    },
    Arc {
        cx: f64,
        cy: f64,
        r: f64,
        start_angle: f64,
        end_angle: f64,
    },
    Path {
        d: String,
    },
    /// **A laid-out run** (Task 11.0). Geometry crosses as `d` — the glyph
    /// contours, exactly what the renderer fills — plus the run's own numbers,
    /// because a text node is the one primitive whose *box* is not derivable
    /// from its shape (the box is the advance area a selection rectangle and a
    /// "slide along the path" gutter are drawn from).
    Text {
        d: String,
        glyphs: usize,
        width: f64,
        height: f64,
        lines: usize,
    },
}

impl From<&EvaluatedPrimitive> for SnapshotPrimitive {
    fn from(p: &EvaluatedPrimitive) -> Self {
        match p {
            EvaluatedPrimitive::Rect {
                x,
                y,
                w,
                h,
                corner_radius,
            } => Self::Rect {
                x: *x,
                y: *y,
                w: *w,
                h: *h,
                corner_radius: *corner_radius,
            },
            EvaluatedPrimitive::Circle { cx, cy, r } => Self::Circle {
                cx: *cx,
                cy: *cy,
                r: *r,
            },
            EvaluatedPrimitive::Arc {
                cx,
                cy,
                r,
                start_angle,
                end_angle,
            } => Self::Arc {
                cx: *cx,
                cy: *cy,
                r: *r,
                start_angle: *start_angle,
                end_angle: *end_angle,
            },
            EvaluatedPrimitive::Path(path) => Self::Path {
                d: path_to_svg_data(path),
            },
            EvaluatedPrimitive::Text(text) => Self::Text {
                d: path_to_svg_data(&text.outline),
                glyphs: text.glyphs.len(),
                width: text.metrics.width,
                height: text.metrics.height,
                lines: text.metrics.lines,
            },
        }
    }
}

/// Wire shape of one gradient stop: an offset and a `#rrggbb[aa]` colour.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SnapshotStop {
    pub offset: f64,
    pub color: String,
}

/// Wire shape of a resolved paint (Task 10.2 RULE 3). Gradients carry their
/// **document-space** frame and their stops, which is everything the gradient bar
/// and the SVG preview need; the sampler stays in the engine.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum SnapshotPaint {
    Solid {
        color: String,
    },
    Linear {
        start: [f64; 2],
        end: [f64; 2],
        stops: Vec<SnapshotStop>,
    },
    Radial {
        center: [f64; 2],
        radius: f64,
        stops: Vec<SnapshotStop>,
    },
}

impl From<&EvaluatedPaint> for SnapshotPaint {
    fn from(paint: &EvaluatedPaint) -> Self {
        let stops = |stops: &[vectra_core::GradientStop]| -> Vec<SnapshotStop> {
            stops
                .iter()
                .map(|stop| SnapshotStop {
                    offset: stop.offset,
                    color: stop.color.to_hex(),
                })
                .collect()
        };
        match paint {
            EvaluatedPaint::Solid(color) => Self::Solid {
                color: color.to_hex(),
            },
            EvaluatedPaint::Linear {
                start,
                end,
                gradient,
            } => Self::Linear {
                start: [start.x, start.y],
                end: [end.x, end.y],
                stops: stops(&gradient.stops),
            },
            EvaluatedPaint::Radial {
                center,
                radius,
                gradient,
            } => Self::Radial {
                center: [center.x, center.y],
                radius: *radius,
                stops: stops(&gradient.stops),
            },
        }
    }
}

/// One layer of a node's appearance stack (Task 10.2 RULE 3): the row the
/// Appearance Panel draws, and the row the renderer draws.
///
/// `width` is present exactly when the layer is a stroke, mirroring
/// [`EvaluatedAppearance::stroke_width`] — a fill has no width, and inventing
/// `Some(0.0)` would make the panel offer a control that does nothing.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SnapshotAppearance {
    /// `fill` or `stroke` — the panel's two sections, and nothing else.
    pub kind: String,
    pub paint: SnapshotPaint,
    /// This layer's own opacity, `0..=1`.
    pub opacity: f64,
    /// `normal` | `multiply` | `screen` | `overlay` — icon lookup keys for the
    /// panel, and the same tags the shader's blend code is derived from.
    pub blend: String,
    /// A hidden layer is still in the stack (that is what makes the eye a
    /// *renderer-side* flag — RULE 4).
    pub visible: bool,
    pub width: Option<f64>,
}

impl From<&EvaluatedAppearance> for SnapshotAppearance {
    fn from(layer: &EvaluatedAppearance) -> Self {
        Self {
            kind: if layer.is_stroke() { "stroke" } else { "fill" }.to_string(),
            paint: SnapshotPaint::from(&layer.paint),
            opacity: layer.opacity,
            blend: layer.blend.tag().to_string(),
            visible: layer.visible,
            width: layer.stroke_width(),
        }
    }
}

/// Wire shape of one evaluated style. Colors cross as `#rrggbb[aa]` hex.
///
/// `fill` / `stroke` / `stroke_width` are **derived** from the stack (first fill,
/// first stroke) and kept because they are what the SVG preview, the swatch and
/// every legacy consumer read; `appearances` is the canonical stack, and it is
/// what the Appearance Panel edits.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SnapshotStyle {
    pub fill: String,
    pub stroke: String,
    pub stroke_width: f64,
    pub opacity: f64,
    pub appearances: Vec<SnapshotAppearance>,
}

impl From<&EvaluatedStyle> for SnapshotStyle {
    fn from(s: &EvaluatedStyle) -> Self {
        Self {
            fill: s
                .first_fill()
                .map(|layer| layer.paint.preview_color().to_hex())
                .unwrap_or_else(|| vectra_core::Color::TRANSPARENT.to_hex()),
            stroke: s
                .first_stroke()
                .map(|layer| layer.paint.preview_color().to_hex())
                .unwrap_or_else(|| vectra_core::Color::TRANSPARENT.to_hex()),
            stroke_width: s.max_stroke_width(),
            opacity: s.opacity,
            appearances: s.appearances.iter().map(SnapshotAppearance::from).collect(),
        }
    }
}

/// One drawable row for the Layers panel + SVG preview.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SnapshotNode {
    pub id: String,
    pub name: String,
    pub primitive: SnapshotPrimitive,
    pub style: SnapshotStyle,
    /// The node's **canonical position slots** (Task 3.2), or `None` for kinds
    /// that have none (`Path`, `Group`) — which is also the UI's "is this node
    /// draggable?" test. The `*_source` tags say how each slot is driven
    /// (`literal`, `variable`, `expression`, …), so the UI can warn before a
    /// drag breaks a parametric link.
    pub position: Option<SnapshotPosition>,
    /// Effective visibility: the node's own eye **and** its layer's (RULE 1).
    /// This is the flag the canvas obeys.
    pub visible: bool,
    /// Effective lock: the node's own padlock **or** its layer's.
    pub locked: bool,
    /// The node's *own* flags, so the Layers Panel can show a node that is
    /// visible while its layer is hidden (and vice versa) without another call.
    pub own_visible: bool,
    pub own_locked: bool,
    /// The layer that lists this node, if any. An unassigned node is a legacy
    /// document's node and still draws.
    pub layer: Option<String>,
    /// The layer's name, so the row reads `Layer — Node` without the panel
    /// joining two lists by hand.
    pub layer_name: Option<String>,
    /// The Text panel's row (Task 11.0): present exactly for a text node.
    /// `None` on every other kind, which is also the UI's "show the typography
    /// inspector?" test — the same convention as `position` above.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<SnapshotText>,
}

/// Everything the typography inspector edits, plus how each number is driven.
///
/// The four parametric slots carry their resolved value **and** their
/// [`vectra_core::Parameter::source_tag`], so the panel can warn before a
/// slider drag breaks a `$variable` or a spring — the exact contract
/// `SnapshotPosition` has for the x/y slots, extended to type.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SnapshotText {
    pub text: String,
    pub font_family: String,
    /// `Left` / `Center` / `Right` — [`vectra_core::TextAlign::tag`].
    pub alignment: String,
    pub font_size: f64,
    pub font_size_source: String,
    pub letter_spacing: f64,
    pub letter_spacing_source: String,
    pub line_height: f64,
    pub line_height_source: String,
    /// The bound path's node id when the run follows a curve (RULE 2).
    pub bound_to: Option<String>,
    /// The bound run's arc-length offset (`path_offset`), when bound.
    pub offset: Option<f64>,
    pub offset_source: Option<String>,
}

/// The canonical position of a node plus how each slot is sourced.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SnapshotPosition {
    pub x: f64,
    pub y: f64,
    /// `Parameter::source_tag()` of the x-like slot.
    pub x_source: String,
    /// `Parameter::source_tag()` of the y-like slot.
    pub y_source: String,
}

/// Flat scene: drawables keyed by id + explicit z-order (back → front).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SnapshotScene {
    /// Sorted by id — see the module docs on canonical ordering.
    pub nodes: BTreeMap<String, SnapshotNode>,
    pub z_order: Vec<String>,
    /// The families the host's font library can resolve, sorted — the Text
    /// panel's font picker. The bundled family is always in the list, so the
    /// control is never empty.
    pub fonts: Vec<String>,
}

/// One row of the Layers Panel (Task 10.2 RULE 1).
///
/// **Layers are not nodes** (see `vectra_core::layers`): this is a separate list
/// with its own ids, and a node names its layer. Groups are nodes
/// (`NodeKind::Group`) and appear as rows inside a layer, which is what makes
/// nesting — and "moving a group moves its children" — fall out of the engine's
/// existing group semantics instead of a second tree here.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SnapshotLayer {
    pub id: String,
    pub name: String,
    pub visible: bool,
    pub locked: bool,
    /// **Task 10.7 RULE 3a**: new artwork on this layer is constrained to what
    /// the layer already holds. The panel shows a chip; the engine reads the flag
    /// at the drawing boundary.
    pub alpha_locked: bool,
    /// **Task 10.7 RULE 3b**: this layer shows only where it overlaps the layer
    /// below.
    pub clipping_mask: bool,
    /// The layer this one clips to, when a layer exists below it. `None` for the
    /// bottom layer — nothing to clip to, so the panel disables the toggle rather
    /// than letting the designer switch on a mask that masks everything away.
    pub clipped_to: Option<String>,
    /// The layer's contents, back → front, exactly as the engine draws them.
    pub children: Vec<String>,
    /// Display names parallel to `children` — the panel formats no ids.
    pub child_names: Vec<String>,
    /// `true` for a child that is a group node, so the panel knows which rows
    /// carry a group icon.
    pub child_is_group: Vec<bool>,
    /// `true` for a child that holds something — a group whose subtree is not
    /// empty. The panel's disclosure triangles are exactly these, and a folder
    /// with nothing in it gets none: a triangle that opens onto emptiness is a
    /// lie about the document.
    pub child_can_open: Vec<bool>,
    /// **Which group holds each child**, parallel to `children` and in the same
    /// order (Task 10.4 RULE 1). `None` = the layer lists that row directly.
    ///
    /// This is the whole tree on the wire: a row's contents are the rows that
    /// name it, so nesting to any depth costs one optional id per row rather than
    /// a nested structure per level — which is what removed Task 10.3's
    /// two-level cap (§5.1 of that report). The panel resolves it in a walk that
    /// also yields the indent, so no depth is sent: one fact, one source.
    ///
    /// A parent is only ever another row **in this layer** (a group and its
    /// contents share a layer — `Document::set_parent` moves the subtree), so a
    /// link that crosses layers can only come from a hand-written document; the
    /// projection reports such a row as a root instead, rather than sending a
    /// parent the panel could not draw.
    pub child_parent: Vec<Option<String>>,
    /// The artboard that owns this layer, if any.
    pub artboard: Option<String>,
    /// The active layer, so the panel can mark it (the one `ensure_layer` hands
    /// new artwork to).
    pub active: bool,
}

/// One row of the artboard dropdown (Task 10.2 RULE 2).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SnapshotArtboard {
    pub id: String,
    pub name: String,
    /// Bounds in document space: `(x, y, width, height)` — what the canvas frames
    /// when the designer jumps to this artboard.
    pub bounds: [f64; 4],
    pub background: String,
    /// How many layers call this board home, for the dropdown's subtitle.
    pub layers: usize,
    pub active: bool,
}

/// Wire shape of one evaluation diagnostic.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SnapshotDiagnostic {
    pub severity: String,
    pub code: String,
    pub node_id: Option<String>,
    pub property: Option<String>,
    pub message: String,
}

impl From<&Diagnostic> for SnapshotDiagnostic {
    fn from(d: &Diagnostic) -> Self {
        Self {
            severity: d.severity.tag().to_string(),
            code: d.code.tag().to_string(),
            node_id: d.node_id.as_ref().map(ToString::to_string),
            property: d.property.clone(),
            message: d.message.clone(),
        }
    }
}

/// Wire shape of the engine's evaluation bookkeeping (Task 2.2).
///
/// `full_evals` staying at 1 across a session is the incrementality claim made
/// observable: the document was evaluated once, at cold start, and every edit
/// after that patched only what the dependency graph marked dirty.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct SnapshotEval {
    pub last_mode: &'static str,
    pub last_dirty: usize,
    pub last_evaluated: usize,
    pub full_evals: u32,
    pub incremental_evals: u32,
    pub no_ops: u32,
}

impl From<vectra_dependency::EvalStats> for SnapshotEval {
    fn from(stats: vectra_dependency::EvalStats) -> Self {
        Self {
            last_mode: stats.last_mode.tag(),
            last_dirty: stats.last_dirty,
            last_evaluated: stats.last_evaluated,
            full_evals: stats.full_evals,
            incremental_evals: stats.incremental_evals,
            no_ops: stats.no_ops,
        }
    }
}

/// Everything the projection needs, grouped so call sites stay readable.
pub struct SnapshotInputs<'a> {
    pub engine: &'a Engine,
    /// The engine's *cached* evaluated scene (never re-evaluated here).
    pub scene: &'a EvaluatedScene,
    pub diagnostics: &'a [Diagnostic],
    /// Constraint diagnostics from the last solver pass (drops, broken links,
    /// skips) — merged with `diagnostics` in canonical order.
    pub solver_diagnostics: &'a [Diagnostic],
    /// Canonical positions by node id (Task 3.2), precomputed by the engine
    /// owner because resolving them needs the compiled expression registry.
    pub positions: &'a BTreeMap<String, SnapshotPosition>,
    /// Text-panel rows by node id (Task 11.0), precomputed by the engine owner
    /// for the same reason: resolving the four type slots needs the compiled
    /// registries.
    pub texts: &'a BTreeMap<String, SnapshotText>,
    /// Families the host's font library answers to, sorted (the picker).
    pub fonts: Vec<String>,
    /// Counters from the last solver pass, plus the live gesture state.
    pub solver: SnapshotSolver,
    pub stats: vectra_dependency::EvalStats,
    pub graph: GraphSummary,
    /// Last published value per node and port, already formatted
    /// (`region 3 rings`, `scalar 30`) — precomputed by the engine owner for the
    /// same reason `positions` is: formatting a `GeometryData` needs the
    /// procedural layer, and this module stays a projection.
    pub published: &'a BTreeMap<String, BTreeMap<String, String>>,
}

/// The full UI snapshot. Evaluation is total, so this response is
/// infallible — failures surface as `diagnostics`, never as a thrown error.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "lowercase")]
pub enum SnapshotResponse {
    Ok {
        scene: SnapshotScene,
        variables: BTreeMap<String, f64>,
        /// Defined expressions: id → source (inspector + fx panel).
        expressions: BTreeMap<String, String>,
        /// Sorted by (severity, code, node, message) — see the module docs.
        diagnostics: Vec<SnapshotDiagnostic>,
        /// Dependency-graph summary: vertices, edges, acyclicity.
        graph: GraphSummary,
        /// Incrementality bookkeeping (see [`SnapshotEval`]).
        eval: SnapshotEval,
        /// Active + parked constraints, keyed by id (canonical order).
        constraints: BTreeMap<String, SnapshotConstraint>,
        /// Non-destructive operations, keyed by id in draw order (Task 4.0).
        /// Their *geometry* is in `scene`, as a `path` primitive.
        operations: BTreeMap<String, SnapshotOperation>,
        /// Procedural nodes, keyed by id (Task 7.0), with the registry's own
        /// topological order alongside — a `BTreeMap` on the wire would sort by
        /// uuid, and a chain has a direction.
        procedural: BTreeMap<String, SnapshotProceduralNode>,
        procedural_order: Vec<String>,
        /// Constraint-solver counters for the last pass (see [`SolverStats`]).
        solver: SnapshotSolver,
        /// The document's layers, back → front (Task 10.2 RULE 1). The Layers
        /// Panel renders this list and nothing else.
        layers: Vec<SnapshotLayer>,
        /// The document's artboards, in registry order (Task 10.2 RULE 2).
        artboards: Vec<SnapshotArtboard>,
        /// Which layer new artwork lands in.
        active_layer: Option<String>,
        /// The artboard the canvas is on — also what "Export current artboard"
        /// means.
        active_artboard: Option<String>,
        can_undo: bool,
        can_redo: bool,
        /// History depth, not just "is there anything to undo" (Task 6.0): the
        /// Undo Isolation Law is a statement about *counts*, and the UI's
        /// "n steps" label wants the number anyway.
        undo_depth: usize,
        redo_depth: usize,
        time: f64,
    },
}

impl SnapshotResponse {
    pub fn to_json(&self) -> String {
        serde_json::to_string(self)
            .unwrap_or_else(|_| r#"{"status":"ok","scene":{"nodes":{},"z_order":[]},"variables":{},"expressions":{},"diagnostics":[],"graph":{"nodes":0,"edges":0,"acyclic":true},"eval":{"last_mode":"full","last_dirty":0,"last_evaluated":0,"full_evals":0,"incremental_evals":0,"no_ops":0},"can_undo":false,"can_redo":false,"time":0.0}"#.to_string())
    }
}

/// Wire shape of one non-destructive operation (Task 4.0) — the Operations
/// panel's entire view of it.
///
/// The description is engine-rendered (like a constraint's), so the UI formats
/// no numbers of its own; `inputs` are the source ids the operation reads, which
/// is exactly what the panel indents its row under.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SnapshotOperation {
    pub id: String,
    pub name: String,
    pub kind: String,
    /// e.g. `subtract`, `fillet r=4` — the operand, already formatted.
    pub description: String,
    pub inputs: Vec<String>,
    /// The inputs' display names (parallel to `inputs`), for the panel.
    pub input_names: Vec<String>,
    pub enabled: bool,
}

/// **The Smart Fill plan** (Task 12.0 RULES 1, 3 and 4): the region graph of a
/// set of nodes, as the UI reads it.
///
/// One shape for three consumers, because they are all asking the same
/// question — *which faces do these paths make?*
///
/// * the **Smart Fill tool** hovers a face (`regions[].path` is the highlight);
/// * **ColorDrop** drops into a face and creates a fill with the dropped paint;
/// * the **Region panel** lists the crossings and each boundary's spans, and
///   offers "break this span".
///
/// The numbers are the engine's: areas, arc lengths and bounds are computed in
/// `vectra-geometry` and formatted nowhere in the UI, the same rule every other
/// panel in this workspace follows.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RegionPlanWire {
    /// The shapes that took part, in the order the caller named them — the
    /// order every face signature below is stated in. A group in the caller's
    /// list is *expanded* to the shapes it holds (a group is a selection, not a
    /// boundary).
    pub sources: Vec<PlanSourceWire>,
    /// The faces, smallest-last within equal signatures: each one is a closed
    /// region path the tool can highlight or a fill can adopt.
    pub regions: Vec<PlanRegionWire>,
    /// The intersection points themselves, for the overlay's markers.
    pub crossings: Vec<[f64; 2]>,
    /// The face the caller's probe point fell in, when it passed one: the
    /// tool's hover target, and RULE 4's "which region was this dropped in".
    pub hit: Option<usize>,
}

/// One boundary path in a plan.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PlanSourceWire {
    pub id: String,
    pub name: String,
    /// The shape's own area, in document units².
    pub area: f64,
    /// `[min_x, min_y, max_x, max_y]`, for framing and for the overlay's bbox.
    pub bounds: [f64; 4],
    /// The arcs between this path's intersection points. Empty when nothing
    /// crosses it: there is no point to break at, so there is no span.
    pub spans: Vec<PlanSpanWire>,
}

/// One span: an arc of a boundary's outline between two crossings.
///
/// `from`/`to` are arc lengths along the **whole outline** (all the path's
/// rings, in path order), which is exactly what `break_path` takes back — the
/// UI echoes the numbers it was given rather than computing cut positions.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PlanSpanWire {
    /// Position in this source's `spans` — the panel's row key.
    pub index: usize,
    pub from: f64,
    pub to: f64,
    /// Which subpath (ring) of the source the span is on.
    pub ring: usize,
    pub start: [f64; 2],
    pub end: [f64; 2],
    pub length: f64,
    /// The source's total outline length, so the panel can draw the span's
    /// position as a fraction without a second call.
    pub total: f64,
}

/// One face of the arrangement.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PlanRegionWire {
    pub index: usize,
    pub area: f64,
    /// How many holes the face has (a Smart Fill inside an "O" has one).
    pub holes: usize,
    /// The ids of the sources this face is inside, in plan order — its
    /// signature. A face is inside at least one source, by construction.
    pub members: Vec<String>,
    /// The face as SVG path data: the canonical "clean closed Path" of RULE 1,
    /// and what the geometry is when the fill is finally created.
    pub path: String,
    /// The same face as document-space rings (exterior first, then holes), for
    /// an overlay that maps coordinates through the renderer's camera rather
    /// than parsing path data of its own.
    pub rings: Vec<Vec<[f64; 2]>>,
}

/// A procedural node (Task 7.0), keyed by id. Its *geometry* is in `scene` as a
/// standard evaluated node (RULE 4 — the same id space, so the inspector, the
/// dependency view and the renderer need no special case); this is the panel's
/// view of the graph itself: what the node is, what it reads, and what it last
/// published.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SnapshotProceduralNode {
    pub id: String,
    pub name: String,
    pub kind: String,
    /// `ProceduralNode::describe()` — the node's **effective** operands, so a
    /// panel built on this cannot show numbers the node no longer has.
    pub description: String,
    pub enabled: bool,
    /// Input port → the `node:port` it is wired to.
    pub wires: BTreeMap<String, String>,
    /// Declared output ports, with the value the last pass published (if any).
    pub outputs: Vec<SnapshotProceduralPort>,
    /// Ids this node reads through wires, for the panel's chain line.
    pub upstream: Vec<String>,
}

/// One output port and what it last published.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SnapshotProceduralPort {
    pub port: String,
    pub ty: String,
    /// `None` until the pass has published this port — a parked node, or one
    /// that failed. The panel draws a dash rather than a stale number.
    pub value: Option<String>,
}

impl SnapshotOperation {
    fn from_operation(op: &vectra_core::OperationNode, doc: &vectra_core::Document) -> Self {
        // The panel lists the shapes the operation **reads**. For a Smart Fill
        // that is the kind's boundaries, not `inputs[0]` — `inputs[0]` is the
        // fill's own id (the seed carrier `arity()` counts), and a row printing
        // a node its own id reads as a bug rather than as a design.
        let shapes: Vec<NodeId> = if op.kind.boundaries().is_empty() {
            op.inputs.clone()
        } else {
            op.kind.boundaries().to_vec()
        };
        Self {
            id: op.id.to_string(),
            name: op.name.clone(),
            kind: op.kind.tag().to_string(),
            description: op.kind.describe(),
            inputs: shapes.iter().map(ToString::to_string).collect(),
            input_names: shapes
                .iter()
                .map(|id| {
                    doc.nodes
                        .get(id)
                        .map(|node| node.name.clone())
                        .unwrap_or_else(|| id.to_string())
                })
                .collect(),
            enabled: op.enabled,
        }
    }
}

/// Wire shape of one constraint (Task 3.1) — the Inspector's entire view of it,
/// including the human description so the UI formats nothing itself.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SnapshotConstraint {
    pub id: String,
    pub kind: String,
    pub targets: Vec<SnapshotConstraintTarget>,
    pub strength: String,
    pub value: Option<f64>,
    pub enabled: bool,
    pub description: String,
}

/// One end of a constraint: the node and the float slot it addresses.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SnapshotConstraintTarget {
    pub node_id: String,
    pub property: String,
    /// The node's UI name, when it still exists.
    pub name: Option<String>,
}

impl From<&Constraint> for SnapshotConstraint {
    fn from(constraint: &Constraint) -> Self {
        Self {
            id: constraint.id.to_string(),
            kind: constraint.kind.tag().to_string(),
            targets: constraint
                .targets
                .iter()
                .map(|target| SnapshotConstraintTarget {
                    node_id: target.node_id.to_string(),
                    property: target.property.clone(),
                    name: None,
                })
                .collect(),
            strength: constraint.strength.tag().to_string(),
            value: constraint.value,
            enabled: constraint.enabled,
            description: constraint.description(),
        }
    }
}

/// Wire shape of the constraint-solver counters for the last pass.
///
/// `variables` is the live Cassowary variable count: it grows when a rule
/// introduces slots and shrinks when the rule goes away — which is how the
/// undo law's *"internal variable count decreases"* becomes visible to the UI.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SnapshotSolver {
    pub variables: usize,
    pub constraints: usize,
    /// Edit variables in the tableau (stays + hints + pointer edits).
    pub edit_variables: usize,
    /// Pointer-driven edit variables currently registered — 0 unless a drag
    /// gesture is live (Task 3.2). The persistence law asserts this is 0 when
    /// the gesture ends.
    pub active_edits: usize,
    /// The node a live gesture is dragging, if any.
    pub drag_node: Option<String>,
    pub writes: usize,
    pub dropped: usize,
    pub skipped: usize,
}

impl SnapshotSolver {
    /// Counters plus the live gesture state (the only part the stats cannot
    /// know about).
    pub fn from_stats(stats: SolverStats, drag_node: Option<String>) -> Self {
        Self {
            variables: stats.variables,
            constraints: stats.constraints,
            edit_variables: stats.edit_variables,
            active_edits: stats.active_edits,
            drag_node,
            writes: stats.writes,
            dropped: stats.dropped,
            skipped: stats.skipped,
        }
    }
}

/// Project the live engine into a [`SnapshotResponse`].
///
/// Pure read: the scene comes from the engine's cache (see [`SnapshotInputs`]),
/// so this never triggers evaluation and never fails.
pub fn build_snapshot(inputs: SnapshotInputs<'_>) -> SnapshotResponse {
    let doc = inputs.engine.document();

    let mut nodes = BTreeMap::new();
    for (id, evaluated) in &inputs.scene.nodes {
        let key = id.to_string();
        // A virtual operation node has no entry in the node table: its name
        // comes from the registry (RULE 3 — same id space, own registry).
        let name = doc
            .nodes
            .get(id)
            .map(|n| n.name.clone())
            .or_else(|| doc.operations.get(*id).map(|op| op.name.clone()))
            // RULE 4's other half: a drawn procedural result must also be
            // *nameable*, or the inspector shows a bare uuid.
            .or_else(|| doc.procedural.get(*id).map(|node| node.name.clone()))
            .unwrap_or_else(|| key.clone());
        let position = inputs.positions.get(&key).cloned();
        let own = doc.nodes.get(id);
        let listed = doc.layers.layer_of(*id).map(|(layer, _)| layer);
        nodes.insert(
            key.clone(),
            SnapshotNode {
                id: id.to_string(),
                name,
                primitive: SnapshotPrimitive::from(&evaluated.primitive),
                style: SnapshotStyle::from(&evaluated.style),
                position,
                // Effective flags come from the evaluated node (the boundary
                // computed them once); the own- flags come from the document.
                visible: evaluated.visible,
                locked: evaluated.locked,
                own_visible: own.map(|node| node.visible).unwrap_or(false),
                own_locked: own.map(|node| node.locked).unwrap_or(false),
                layer: listed.map(|layer| layer.id.to_string()),
                layer_name: listed.map(|layer| layer.name.clone()),
                text: inputs.texts.get(&key).cloned(),
            },
        );
    }

    // ── Layers and artboards (Task 10.2 RULEs 1 and 2) ──────────────────────
    let active_layer = doc.active_layer;
    let active_artboard = doc.artboards.active_id();
    let node_name = |node: &vectra_core::NodeId| -> String {
        doc.nodes
            .get(node)
            .map(|entry| entry.name.clone())
            .or_else(|| doc.operations.get(*node).map(|op| op.name.clone()))
            .or_else(|| doc.procedural.get(*node).map(|entry| entry.name.clone()))
            .unwrap_or_else(|| node.to_string())
    };
    let is_group = |node: &vectra_core::NodeId| -> bool {
        matches!(
            doc.nodes.get(node).map(|entry| &entry.kind),
            Some(vectra_core::NodeKind::Group { .. })
        )
    };
    /// The ids inside a group node, in the engine's own order — **nothing else**
    /// answers here, which is what keeps the panel's tree a projection of
    /// `NodeKind::Group`'s contents rather than a second tree with its own rules.
    fn group_children(
        doc: &vectra_core::Document,
        node: &vectra_core::NodeId,
    ) -> Vec<vectra_core::NodeId> {
        match doc.nodes.get(node).map(|entry| &entry.kind) {
            Some(vectra_core::NodeKind::Group { children }) => children.clone(),
            _ => Vec::new(),
        }
    }
    let layers: Vec<SnapshotLayer> = doc
        .layers
        .iter()
        .map(|layer| SnapshotLayer {
            id: layer.id.to_string(),
            name: layer.name.clone(),
            visible: layer.visible,
            locked: layer.locked,
            alpha_locked: layer.alpha_locked,
            clipping_mask: layer.clipping_mask,
            clipped_to: doc
                .layers
                .below(&layer.id)
                .map(|below| below.id.to_string()),
            children: layer.children.iter().map(ToString::to_string).collect(),
            child_names: layer.children.iter().map(node_name).collect(),
            child_is_group: layer.children.iter().map(is_group).collect(),
            child_can_open: layer
                .children
                .iter()
                .map(|child| is_group(child) && !group_children(doc, child).is_empty())
                .collect(),
            child_parent: layer
                .children
                .iter()
                .map(|child| {
                    // A parent the panel cannot see is not a parent: a link
                    // outside this layer is reported as a root, so the panel never
                    // has to invent a row to hang a node under.
                    doc.parent_of(*child)
                        .filter(|parent| layer.children.contains(parent))
                        .map(|parent| parent.to_string())
                })
                .collect(),
            artboard: doc
                .artboards
                .iter()
                .find(|board| board.has_layer(&layer.id))
                .map(|board| board.id.to_string()),
            active: active_layer.as_ref() == Some(&layer.id),
        })
        .collect();
    let artboards: Vec<SnapshotArtboard> = doc
        .artboards
        .iter()
        .map(|board: &ArtboardRecord| SnapshotArtboard {
            id: board.id.to_string(),
            name: board.name.clone(),
            bounds: [board.x, board.y, board.width, board.height],
            background: board.background.to_hex(),
            layers: board.layers.len(),
            active: active_artboard.as_ref() == Some(&board.id),
        })
        .collect();
    let scene = SnapshotScene {
        nodes,
        z_order: inputs
            .scene
            .z_order
            .iter()
            .map(ToString::to_string)
            .collect(),
        fonts: inputs.fonts,
    };

    SnapshotResponse::Ok {
        scene,
        variables: doc.variables.iter().map(|(k, v)| (k.clone(), *v)).collect(),
        expressions: doc
            .expressions
            .iter()
            .map(|(id, record)| (id.to_string(), record.source.clone()))
            .collect(),
        diagnostics: sorted_diagnostics(inputs.diagnostics, inputs.solver_diagnostics),
        constraints: doc
            .constraints
            .iter()
            .map(|constraint| {
                let mut view = SnapshotConstraint::from(constraint);
                for target in &mut view.targets {
                    target.name = doc
                        .nodes
                        .get(&constraint_target_node(target))
                        .map(|node| node.name.clone());
                }
                (constraint.id.to_string(), view)
            })
            .collect(),
        operations: doc
            .operations
            .in_order()
            .map(|op| {
                (
                    op.id.to_string(),
                    SnapshotOperation::from_operation(op, doc),
                )
            })
            .collect(),
        procedural: doc
            .procedural
            .in_order()
            .map(|node| {
                (
                    node.id.to_string(),
                    SnapshotProceduralNode::from_node(node, inputs.published),
                )
            })
            .collect(),
        procedural_order: doc
            .procedural
            .in_order()
            .map(|node| node.id.to_string())
            .collect(),
        layers,
        artboards,
        active_layer: active_layer.map(|id: LayerId| id.to_string()),
        active_artboard: active_artboard.map(|id| id.to_string()),
        solver: inputs.solver,
        graph: inputs.graph,
        eval: SnapshotEval::from(inputs.stats),
        can_undo: inputs.engine.can_undo(),
        can_redo: inputs.engine.can_redo(),
        undo_depth: inputs.engine.history_depth(),
        redo_depth: inputs.engine.redo_depth(),
        time: inputs.engine.time(),
    }
}

impl SnapshotProceduralNode {
    fn from_node(
        node: &vectra_core::ProceduralNode,
        published: &BTreeMap<String, BTreeMap<String, String>>,
    ) -> Self {
        let values = published.get(&node.id.to_string());
        Self {
            id: node.id.to_string(),
            name: node.name.clone(),
            kind: node.kind.tag().to_string(),
            description: node.describe(),
            enabled: node.enabled,
            wires: node
                .wires
                .iter()
                .map(|(port, from)| (port.clone(), format!("{}:{}", from.node, from.port)))
                .collect(),
            outputs: node
                .kind
                .outputs()
                .iter()
                .map(|output| SnapshotProceduralPort {
                    port: output.name.clone(),
                    ty: output.ty.tag().to_string(),
                    value: values.and_then(|ports| ports.get(&output.name)).cloned(),
                })
                .collect(),
            upstream: node.upstream().iter().map(ToString::to_string).collect(),
        }
    }
}

/// The node a wire target refers to (parsed back from the string id).
fn constraint_target_node(target: &SnapshotConstraintTarget) -> vectra_core::NodeId {
    target
        .node_id
        .parse()
        .unwrap_or_else(|_| vectra_core::NodeId::nil())
}

/// Canonical diagnostic order: unstable in the cache (a patch merges only the
/// re-evaluated nodes' diagnostics), stable on the wire. Constraint diagnostics
/// ride in the same channel as evaluation diagnostics — one log for the UI.
fn sorted_diagnostics(
    diagnostics: &[Diagnostic],
    solver_diagnostics: &[Diagnostic],
) -> Vec<SnapshotDiagnostic> {
    let mut out: Vec<SnapshotDiagnostic> = diagnostics
        .iter()
        .chain(solver_diagnostics.iter())
        .map(SnapshotDiagnostic::from)
        .collect();
    out.sort_by(|a, b| {
        (&a.severity, &a.code, &a.node_id, &a.property, &a.message).cmp(&(
            &b.severity,
            &b.code,
            &b.node_id,
            &b.property,
            &b.message,
        ))
    });
    out
}

/// **Test-only** twin of the live engine's text rows (`WasmEngine::texts`): the
/// same projection, built from a plain [`Engine`]'s document and context, so a
/// native test can assert what the typography panel would be handed.
///
/// It is a *duplicate* on purpose — the live one resolves through the engine's
/// compiled registries (expressions, motion, procedural, fonts), which a bare
/// `Engine` in a test does not have — and it is small enough that keeping the
/// two readable side by side beats threading a trait through the engine for the
/// benefit of assertions.
#[cfg(test)]
pub fn snapshot_texts(
    doc: &vectra_core::Document,
    ctx: &vectra_core::EvaluationContext,
    scene: &vectra_geometry::EvaluatedScene,
) -> BTreeMap<String, SnapshotText> {
    let _ = scene;
    let mut texts = BTreeMap::new();
    for (id, node) in &doc.nodes {
        let NodeKind::Text {
            text,
            font_family,
            font_size,
            letter_spacing,
            line_height,
            alignment,
            on_path,
            ..
        } = &node.kind
        else {
            continue;
        };
        let (Ok(size), Ok(spacing), Ok(leading)) = (
            font_size.resolve(ctx),
            letter_spacing.resolve(ctx),
            line_height.resolve(ctx),
        ) else {
            continue;
        };
        let (bound_to, offset, offset_source) = match on_path {
            Some(binding) => (
                Some(binding.node.to_string()),
                binding.offset.resolve(ctx).ok(),
                Some(binding.offset.source_tag().to_string()),
            ),
            None => (None, None, None),
        };
        texts.insert(
            id.to_string(),
            SnapshotText {
                text: text.clone(),
                font_family: font_family.clone(),
                alignment: alignment.tag().to_string(),
                font_size: size,
                font_size_source: font_size.source_tag().to_string(),
                letter_spacing: spacing,
                letter_spacing_source: letter_spacing.source_tag().to_string(),
                line_height: leading,
                line_height_source: line_height.source_tag().to_string(),
                bound_to,
                offset,
                offset_source,
            },
        );
    }
    texts
}

/// Test convenience: evaluate a plain [`Engine`] fully, then project. The WASM
/// engine never uses this path — it always projects its incremental cache.
#[cfg(test)]
pub fn build_snapshot_native(
    engine: &Engine,
    ctx: &vectra_core::EvaluationContext,
) -> SnapshotResponse {
    let evaluation = vectra_geometry::GeometryEvaluator.evaluate_full(engine.document(), ctx);
    let positions = BTreeMap::new();
    let texts = snapshot_texts(engine.document(), ctx, &evaluation.scene);
    build_snapshot(SnapshotInputs {
        engine,
        scene: &evaluation.scene,
        diagnostics: &evaluation.diagnostics,
        solver_diagnostics: &[],
        positions: &positions,
        texts: &texts,
        fonts: Vec::new(),
        solver: SnapshotSolver::from_stats(SolverStats::default(), None),
        stats: vectra_dependency::EvalStats::default(),
        published: &BTreeMap::new(),
        graph: GraphSummary {
            nodes: 0,
            edges: 0,
            acyclic: true,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use vectra_core::{Command, NodeKind};

    #[test]
    fn snapshot_projects_scene_variables_and_undo_state() {
        let mut engine = Engine::new();
        let id = vectra_core::new_node_id();
        engine
            .dispatch(Command::CreateNode {
                id,
                kind: NodeKind::circle(100.0, 100.0, 50.0),
                name: Some("hero".to_string()),
                index: None,
            })
            .unwrap();
        engine
            .dispatch(Command::SetVariable {
                name: "base".to_string(),
                value: 200.0,
            })
            .unwrap();

        let json = build_snapshot_native(&engine, &engine.evaluation_context()).to_json();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["status"], "ok");
        assert_eq!(parsed["scene"]["z_order"].as_array().unwrap().len(), 1);
        assert_eq!(parsed["scene"]["nodes"][id.to_string()]["name"], "hero");
        assert_eq!(
            parsed["scene"]["nodes"][id.to_string()]["primitive"]["type"],
            "circle"
        );
        assert_eq!(
            parsed["scene"]["nodes"][id.to_string()]["primitive"]["r"],
            50.0
        );
        assert_eq!(parsed["variables"]["base"], 200.0);
        assert_eq!(parsed["can_undo"], true);
        assert_eq!(parsed["can_redo"], false);
        assert_eq!(parsed["time"], 0.0);
        assert_eq!(parsed["eval"]["last_mode"], "full");
        assert_eq!(parsed["eval"]["full_evals"], 0);
        assert_eq!(parsed["graph"]["acyclic"], true);
    }

    #[test]
    fn snapshot_serializes_path_as_svg_data() {
        use vectra_core::{Parameter, PathSegment, Point2};
        let mut engine = Engine::new();
        let id = vectra_core::new_node_id();
        engine
            .dispatch(Command::CreateNode {
                id,
                kind: NodeKind::Path {
                    start: Parameter::Literal(Point2::new(0.0, 0.0)),
                    segments: vec![
                        PathSegment::Line {
                            to: Parameter::Literal(Point2::new(10.0, 0.0)),
                        },
                        PathSegment::Close,
                    ],
                },
                name: None,
                index: None,
            })
            .unwrap();
        let json = build_snapshot_native(&engine, &engine.evaluation_context()).to_json();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(
            parsed["scene"]["nodes"][id.to_string()]["primitive"],
            serde_json::json!({"type": "path", "d": "M0 0L10 0Z"})
        );
    }
}
