//! The **export IR** (Task 8.0, RULE 3): one framework-agnostic description of
//! the document, compiled once and consumed by every exporter.
//!
//! ```text
//!                 Document  (+ the live EvaluatedScene, when there is one)
//!                     │
//!                     ▼   compile_to_ir / compile_to_ir_resolved
//!                 ┌─────────┐
//!                 │ ExportIR│   semantic geometry + parametric slots + props
//!                 └────┬────┘
//!         ┌────────────┴────────────┐
//!         ▼                         ▼
//!    export_svg(ir)          export_react(ir)        (…and Vue/Svelte later)
//! ```
//!
//! Nothing below this module writes a string template from a `Document`, and no
//! exporter reaches back into the engine. That is the point of the intermediate
//! representation: a new target is a new consumer of the IR, and the IR is the
//! only place that has to understand what a document *means*.
//!
//! ## Two sources, one IR
//!
//! RULE 3 names both, and they answer different questions:
//!
//! * the **`Document`** answers *what did the user author* — typed slots
//!   (`Parameter<T>`), and therefore the parametric code a code generator wants;
//! * the **`EvaluatedScene`** answers *what is on screen* — motion sampled at
//!   this instant, procedural results, boolean compositions, clamped styles.
//!
//! So a node's geometry is described twice over: semantically, by its authoring
//! kind ([`ExportGeometry::Circle`] is a circle, not a path), and numerically,
//! by the value each slot had when the picture was taken. The parametric text is
//! what RULE 2 exports; the number is what a picture export needs when a slot
//! cannot be written as code (a spring, a procedural read).
//!
//! ## What the IR is *not*
//!
//! It is not a scene and it is not a document: it carries no dependency edges and
//! no history. It is a value — `Serialize`, owned, with no lifetimes — and it is
//! stable: two compilations of the same document and the same clock produce the
//! same bytes, which is what makes determinism a testable statement rather than a
//! hope.

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use vectra_core::document::{Node, NodeKind};
use vectra_core::eval::Resolvable;
use vectra_core::geom::{Color, Point2};
use vectra_core::param::{NodeOutputId, Parameter};
use vectra_core::{Document, EvaluationContext, PathSegment};
use vectra_expression::ExpressionEngine;
use vectra_geometry::{
    path_to_svg_data, primitive_to_path, EvaluatedNode, EvaluatedPrimitive, EvaluatedScene,
    EvaluatedStyle, GeometryEvaluator,
};

/// The compiled picture: nodes in draw order, the props a code generator must
/// declare, and every note about something that could not be exported exactly.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExportIR {
    /// Nodes in draw order (back → front), the order the renderer uses.
    pub nodes: Vec<ExportNode>,
    /// The unique variables the document's slots read, sorted by name — RULE 2's
    /// "extract all unique variables and define them as typed props".
    pub props: Vec<ExportProp>,
    /// Document-space bounds (y **up**, as the document is), when any node had a
    /// resolved box. `None` for an empty picture.
    pub bounds: Option<ExportBounds>,
    /// Human-readable notes: a slot that fell back to a number, an expression that
    /// could not be translated, a name that had to be sanitised.
    pub warnings: Vec<String>,
}

/// One node of the picture.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExportNode {
    /// The engine's node id, as a string. Kept for stable keys
    /// (`data-vectra-node` in SVG, `key` in JSX); it is not a dependency.
    pub id: String,
    /// The node's name, as the inspector shows it.
    pub name: String,
    pub geometry: ExportGeometry,
    pub style: ExportStyle,
}

/// A **semantic** primitive — the shape the user drew, not its tessellation.
///
/// This is RULE 1's spine: a `Circle` is a circle all the way out to the SVG
/// exporter, so the SVG it produces is `<circle cx cy r/>` and a consumer can
/// still scale it perfectly. Only a true path node — or a *computed* region,
/// which has no primitive form to preserve — is a [`ExportGeometry::Path`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ExportGeometry {
    Rect {
        x: ExportParam,
        y: ExportParam,
        width: ExportParam,
        height: ExportParam,
        corner_radius: ExportParam,
    },
    Circle {
        cx: ExportParam,
        cy: ExportParam,
        radius: ExportParam,
    },
    /// A true arc: the SVG exporter emits a single `A` command, never a chain of
    /// beziers (RULE 1).
    Arc {
        cx: ExportParam,
        cy: ExportParam,
        radius: ExportParam,
        /// Radians, `[0, TAU)`, as the document holds them.
        start_angle: ExportParam,
        /// `start_angle + sweep`, as the evaluator canonicalises it.
        end_angle: ExportParam,
    },
    /// A path: `d` is already lyon's serialization (RULE 1: "use lyon's path
    /// serialization"), so every exporter writes the same commands. `None` when
    /// the path had no resolved geometry — a path whose points come from a spring
    /// cannot be exported as a picture until the clock says when.
    Path { d: Option<String> },
    /// A group of child nodes, by id.
    Group { children: Vec<String> },
}

/// One scalar slot, in both the forms an exporter can need.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExportParam {
    /// The **parametric** text a code generator writes: `40`, `base`, `base * 2`.
    /// Empty when the slot cannot be written as code (motion, a procedural read,
    /// an interaction binding).
    pub code: String,
    /// The value the slot had. For a code-expressible slot this is the same number
    /// the code evaluates to against the document's current variables; for a
    /// dynamic slot it is the *drawn* number (motion at the sampled instant, the
    /// procedural table's value).
    pub value: Option<f64>,
    /// The variables the `code` reads, sorted. Empty for a literal or a dynamic
    /// slot; for an expression, its free variables.
    pub vars: Vec<String>,
}

impl ExportParam {
    /// A literal: code and value are the same number.
    pub fn literal(value: f64) -> Self {
        Self {
            code: fmt_number(value),
            value: Some(value),
            vars: Vec::new(),
        }
    }

    /// A slot that is only a number: no code, a value. (Motion, procedural.)
    pub fn resolved_only(value: Option<f64>) -> Self {
        Self {
            code: String::new(),
            value,
            vars: Vec::new(),
        }
    }

    /// True when a code generator can write this slot parametrically.
    pub fn is_parametric(&self) -> bool {
        !self.code.is_empty()
    }

    /// The number an exporter must draw with: the value if there is one, else 0 —
    /// and the IR has already warned about the else.
    pub fn number(&self) -> f64 {
        self.value.unwrap_or(0.0)
    }
}

/// A colour slot: same shape as [`ExportParam`], with a colour on the value side
/// (RULE 2's colour door, one layer further out).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExportColor {
    /// A CSS colour literal (`#rrggbb` / `#rrggbbaa`), when the slot is static.
    pub code: String,
    pub value: Option<Color>,
    pub vars: Vec<String>,
}

impl ExportColor {
    pub fn literal(value: Color) -> Self {
        Self {
            code: value.to_hex(),
            value: Some(value),
            vars: Vec::new(),
        }
    }

    pub fn resolved_only(value: Option<Color>) -> Self {
        Self {
            code: String::new(),
            value,
            vars: Vec::new(),
        }
    }

    /// True when the code is a **symbol** (a prop name) rather than a colour
    /// literal: a literal's `code` is still the hex string (SVG uses it), while a
    /// variable's is the name a code generator must declare. So unlike
    /// [`ExportParam::is_parametric`], "has code" is not the question here —
    /// "reads something the caller must supply" is.
    pub fn is_parametric(&self) -> bool {
        !self.vars.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExportStyle {
    pub fill: ExportColor,
    pub stroke: ExportColor,
    pub stroke_width: ExportParam,
    pub opacity: ExportParam,
}

/// A component prop a code generator must declare (RULE 2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExportProp {
    /// The variable's name — and, once sanitised, the prop's name.
    pub name: String,
    /// Its type. Vectra variables are numbers today; the field exists so a future
    /// `Color`/`Point` variable is a data change, not an API change.
    pub ty: ExportPropType,
    /// The document's current value.
    pub value: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExportPropType {
    Number,
}

/// Document-space bounds, y **up**.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ExportBounds {
    pub min_x: f64,
    pub min_y: f64,
    pub max_x: f64,
    pub max_y: f64,
}

impl ExportBounds {
    pub fn width(&self) -> f64 {
        self.max_x - self.min_x
    }

    pub fn height(&self) -> f64 {
        self.max_y - self.min_y
    }

    pub fn union(self, other: Self) -> Self {
        Self {
            min_x: self.min_x.min(other.min_x),
            min_y: self.min_y.min(other.min_y),
            max_x: self.max_x.max(other.max_x),
            max_y: self.max_y.max(other.max_y),
        }
    }
}

// ── compilation ────────────────────────────────────────────────────────

/// Compile a document **on its own** — the signature the task names.
///
/// Every statically resolvable slot (literals, variables, expressions) becomes a
/// number; anything that needs a clock or a procedural table is reported in
/// `warnings`. Use [`compile_to_ir_resolved`] when the engine's live scene is at
/// hand — the wasm boundary does — so an exported picture carries motion at the
/// instant it was exported.
pub fn compile_to_ir(doc: &Document) -> ExportIR {
    let evaluation = with_context(doc, |ctx| GeometryEvaluator::new().evaluate_full(doc, ctx));
    let mut ir = with_context(doc, |ctx| build(doc, Some(&evaluation.scene), ctx));
    for diagnostic in &evaluation.diagnostics {
        // `Diagnostic` has a message and a code; nobody wants the Debug dump in a
        // code comment.
        ir.warnings
            .push(format!("evaluator: {}", diagnostic.message));
    }
    ir
}

/// Compile a document **and the scene the engine is drawing**: motion sampled,
/// procedural results published, styles clamped. This is the picture the canvas
/// shows, described semantically.
pub fn compile_to_ir_resolved(doc: &Document, scene: &EvaluatedScene) -> ExportIR {
    with_context(doc, |ctx| build(doc, Some(scene), ctx))
}

/// Run `f` with the context a document can afford on its own: its variables and
/// its expressions. The expression engine is compiled from the document's own
/// sources, so this is the engine's resolution path, not a second implementation
/// of it — but with no clock and no tables, and therefore *no guessing*: a slot
/// that needs more comes back unresolved and is reported.
fn with_context<R>(doc: &Document, f: impl FnOnce(&EvaluationContext) -> R) -> R {
    remember_expressions(doc);
    let mut expressions = ExpressionEngine::new();
    expressions.sync_from_document(doc);
    let ctx = doc.evaluation_context(0.0).with_expression(&expressions);
    f(&ctx)
}

/// The common builder: walk the document's live geometry and describe each node.
fn build(doc: &Document, scene: Option<&EvaluatedScene>, ctx: &EvaluationContext) -> ExportIR {
    // The *document* decides which nodes exist and in what order — the scene only
    // supplies the numbers. That matters for exactly one case, and it is the case
    // RULE 2 is about: a slot driven by an undefined variable makes the evaluator
    // skip the node, and a parametric export must still write the node's code
    // (that is the whole point of exporting code rather than a picture).
    let ids: Vec<vectra_core::NodeId> = doc.geometry_ids();
    let mut nodes: Vec<ExportNode> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();
    let mut bounds: Option<ExportBounds> = None;

    for id in ids {
        let resolved: Option<&EvaluatedNode> = scene.and_then(|scene| scene.get(id));
        match doc.nodes.get(&id) {
            Some(node) => {
                let (geometry, box_) = geometry_of(node, resolved, ctx, &mut warnings);
                if let Some(box_) = box_ {
                    bounds = Some(bounds.map_or(box_, |b| b.union(box_)));
                }
                nodes.push(ExportNode {
                    id: id.to_string(),
                    name: node.name.clone(),
                    geometry,
                    style: style_of(node, resolved, ctx, &mut warnings),
                });
            }
            // A virtual node: an operation's result or a procedural result. It is
            // live geometry (RULE 4) and it draws — but it is *computed*, so it has
            // no primitive form to preserve and no slots a code generator could
            // re-express. It exports as the region it is.
            None => {
                let Some(resolved) = resolved else {
                    continue;
                };
                if let Some(box_) = primitive_bounds(&resolved.primitive) {
                    bounds = Some(bounds.map_or(box_, |b| b.union(box_)));
                }
                nodes.push(ExportNode {
                    id: id.to_string(),
                    name: virtual_name(doc, id),
                    geometry: ExportGeometry::Path {
                        d: Some(path_to_svg_data(&primitive_to_path(&resolved.primitive))),
                    },
                    style: style_of_scene(&resolved.style),
                });
            }
        }
    }

    // RULE 2: every variable any slot reads becomes a prop — once, sorted.
    let mut names: BTreeSet<String> = BTreeSet::new();
    for node in &nodes {
        for param in node.geometry.parameters() {
            names.extend(param.vars.iter().cloned());
        }
        for color in [&node.style.fill, &node.style.stroke] {
            names.extend(color.vars.iter().cloned());
        }
        for param in [&node.style.stroke_width, &node.style.opacity] {
            names.extend(param.vars.iter().cloned());
        }
    }
    let props: Vec<ExportProp> = names
        .into_iter()
        .map(|name| ExportProp {
            ty: ExportPropType::Number,
            value: doc.variables.get(&name).copied().unwrap_or(0.0),
            name,
        })
        .collect();

    ExportIR {
        nodes,
        props,
        bounds,
        warnings,
    }
}

/// The name of a node that has no `Node` record: an operation or a procedural
/// node — the engine's own description, never a guess.
fn virtual_name(doc: &Document, id: vectra_core::NodeId) -> String {
    if let Some(operation) = doc.operations.get(id) {
        return format!("{} (operation)", operation.kind.describe());
    }
    if let Some(node) = doc.procedural.get(id) {
        return node.describe();
    }
    "node".to_string()
}

type GeometryOut = (ExportGeometry, Option<ExportBounds>);

fn geometry_of(
    node: &Node,
    resolved: Option<&EvaluatedNode>,
    ctx: &EvaluationContext,
    warnings: &mut Vec<String>,
) -> GeometryOut {
    let primitive = resolved.map(|node| &node.primitive);
    match &node.kind {
        NodeKind::Rectangle {
            x,
            y,
            width,
            height,
            corner_radius,
        } => {
            let drawn = match primitive {
                Some(EvaluatedPrimitive::Rect {
                    x,
                    y,
                    w,
                    h,
                    corner_radius,
                }) => Some([*x, *y, *w, *h, *corner_radius]),
                _ => None,
            };
            let at = |index: usize| drawn.map(|values| values[index]);
            let geometry = ExportGeometry::Rect {
                x: scalar(x, at(0), ctx, "x", warnings),
                y: scalar(y, at(1), ctx, "y", warnings),
                width: scalar(width, at(2), ctx, "width", warnings),
                height: scalar(height, at(3), ctx, "height", warnings),
                corner_radius: scalar(corner_radius, at(4), ctx, "corner_radius", warnings),
            };
            (geometry, primitive_bounds_of(primitive))
        }
        NodeKind::Circle { cx, cy, radius } => {
            let drawn = match primitive {
                Some(EvaluatedPrimitive::Circle { cx, cy, r }) => Some([*cx, *cy, *r]),
                _ => None,
            };
            let at = |index: usize| drawn.map(|values| values[index]);
            let geometry = ExportGeometry::Circle {
                cx: scalar(cx, at(0), ctx, "cx", warnings),
                cy: scalar(cy, at(1), ctx, "cy", warnings),
                radius: scalar(radius, at(2), ctx, "radius", warnings),
            };
            (geometry, primitive_bounds_of(primitive))
        }
        NodeKind::Arc {
            cx,
            cy,
            radius,
            start_angle,
            end_angle,
        } => {
            let drawn = match primitive {
                Some(EvaluatedPrimitive::Arc {
                    cx,
                    cy,
                    r,
                    start_angle,
                    end_angle,
                }) => Some([*cx, *cy, *r, *start_angle, *end_angle]),
                _ => None,
            };
            let at = |index: usize| drawn.map(|values| values[index]);
            let geometry = ExportGeometry::Arc {
                cx: scalar(cx, at(0), ctx, "cx", warnings),
                cy: scalar(cy, at(1), ctx, "cy", warnings),
                radius: scalar(radius, at(2), ctx, "radius", warnings),
                start_angle: scalar(start_angle, at(3), ctx, "start_angle", warnings),
                end_angle: scalar(end_angle, at(4), ctx, "end_angle", warnings),
            };
            (geometry, primitive_bounds_of(primitive))
        }
        // **Text (Task 11.0).** A run exports as what it is drawn as: its glyph
        // outlines, as one `<path>`. Outlining on the way out — rather than
        // writing `<text>` with a font reference — is what makes an exported
        // file render identically everywhere, with no font installed and no
        // substitution: the letterforms travel inside the SVG. A designer who
        // wants live, editable text in the export runs `OutlineText` first,
        // which is the same non-destructive operation the canvas offers.
        NodeKind::Text { .. } => {
            let d = resolved.and_then(|node| match &node.primitive {
                EvaluatedPrimitive::Path(path) => Some(path_to_svg_data(path)),
                EvaluatedPrimitive::Text(text) => Some(path_to_svg_data(&text.outline)),
                _ => None,
            });
            if d.is_none() {
                warnings.push(format!(
                    "text `{}` has no resolved geometry; exported without `d`",
                    node.name
                ));
            }
            (ExportGeometry::Path { d }, primitive_bounds_of(primitive))
        }
        NodeKind::Path { start, segments } => {
            let d = resolved.and_then(|node| match &node.primitive {
                EvaluatedPrimitive::Path(path) => Some(path_to_svg_data(path)),
                _ => None,
            });
            if d.is_none() {
                warnings.push(format!(
                    "path `{}` has no resolved geometry; exported without `d`",
                    node.name
                ));
            }
            let bounds = path_bounds(start, segments, ctx);
            (ExportGeometry::Path { d }, bounds)
        }
        NodeKind::Group { children } => (
            ExportGeometry::Group {
                children: children.iter().map(|id| id.to_string()).collect(),
            },
            None,
        ),
    }
}

fn primitive_bounds_of(primitive: Option<&EvaluatedPrimitive>) -> Option<ExportBounds> {
    primitive.and_then(primitive_bounds)
}

/// One float slot: parametric code when the author wrote something a code
/// generator can re-express, plus the drawn number as the fallback.
///
/// `drawn` is the evaluator's value for that slot (the pair is positional: the
/// kind's slot order and the primitive's field order are the same list). It is
/// what makes a motion-driven slot exportable as a *picture* even though it has
/// no code.
fn scalar(
    param: &Parameter<f64>,
    drawn: Option<f64>,
    ctx: &EvaluationContext,
    property: &str,
    warnings: &mut Vec<String>,
) -> ExportParam {
    let mut out = param_code(param, warnings);
    if out.value.is_none() {
        out.value = drawn.filter(|value| value.is_finite());
    }
    if out.value.is_none() {
        out.value = param.resolve(ctx).ok();
    }
    if out.value.is_none() {
        warnings.push(format!(
            "slot `{property}` has neither code nor a resolved value; exported as 0"
        ));
    }
    out
}

/// Translate a `Parameter<f64>` into its parametric form — RULE 2's moat.
///
/// The four authoring kinds split in two: `Literal`, `Variable` and `Expression`
/// have code and become props or arithmetic; `Animated`, `Procedural` and
/// `Interaction` are *sampled*, so they keep the drawn number and lose the code
/// (a spring that exports as `{/* animated */}`-less arithmetic would be a lie
/// about what the document is).
fn param_code(param: &Parameter<f64>, warnings: &mut Vec<String>) -> ExportParam {
    match param {
        Parameter::Literal(value) => ExportParam::literal(*value),
        Parameter::Variable(name) => ExportParam {
            code: sanitize_ident(name),
            value: None,
            vars: vec![name.clone()],
        },
        Parameter::Expression(id) => match expression_code(*id, warnings) {
            Some((code, vars)) => ExportParam {
                code,
                value: None,
                vars,
            },
            None => ExportParam::resolved_only(None),
        },
        Parameter::Animated(_) => {
            warnings.push("an animated slot exports as its drawn value (no code)".to_string());
            ExportParam::resolved_only(None)
        }
        Parameter::Procedural(output) => {
            warnings.push(format!(
                "a slot reading {} exports as its drawn value (no code)",
                describe_output(output)
            ));
            ExportParam::resolved_only(None)
        }
        Parameter::Interaction(_) => {
            warnings.push("an interaction slot exports as its drawn value (no code)".to_string());
            ExportParam::resolved_only(None)
        }
    }
}

/// The expression's source, translated to code, plus its free variables.
///
/// The source text comes from the **document** (`ExpressionRecord`), so a slot
/// keeps the author's own arithmetic: `$base * 2` exports as `base * 2`, never as
/// the number it happened to evaluate to.
fn expression_code(
    id: vectra_core::ExpressionId,
    warnings: &mut Vec<String>,
) -> Option<(String, Vec<String>)> {
    let source = CURRENT_EXPRESSIONS.with(|sources| sources.borrow().get(&id).cloned())?;
    let vars = match ExpressionEngine::check_source(&source) {
        Ok(vars) => {
            let mut vars: Vec<String> = vars.into_iter().collect();
            vars.sort();
            vars
        }
        Err(error) => {
            warnings.push(format!(
                "expression {id} does not parse ({error}); exported as a value"
            ));
            return None;
        }
    };
    Some((translate_expression(&source), vars))
}

thread_local! {
    /// The sources of the document currently being compiled.
    ///
    /// A `Parameter::Expression(id)` carries only the id, and a free function
    /// cannot thread the document's expression map through `Parameter::resolve`
    /// — so the compile entry point parks the map here for the duration of one
    /// synchronous `compile_to_ir` call. This is a lookup table for a borrowed
    /// `&Document`, not state: nothing survives the call, and a second document
    /// replaces it.
    static CURRENT_EXPRESSIONS: std::cell::RefCell<std::collections::HashMap<vectra_core::ExpressionId, String>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
}

/// Translate Vectra expression source into a target-language expression.
///
/// The two languages agree on everything except the two things that are syntax:
/// `$name` is a variable reference, and the four functions have their own names.
/// `clamp` is the one that needs a helper — JavaScript has no *total*
/// three-argument clamp — so [`crate::react`] emits one only when the code uses it.
pub fn translate_expression(source: &str) -> String {
    let mut out = String::with_capacity(source.len());
    let mut chars = source.char_indices().peekable();
    while let Some((_, ch)) = chars.next() {
        if ch == '$' {
            // `$name` → `name`; the expression lexer only ever sees an identifier
            // after `$`, so consuming the identifier run is enough.
            while let Some((_, next)) = chars.peek() {
                if next.is_alphanumeric() || *next == '_' {
                    out.push(*next);
                    chars.next();
                } else {
                    break;
                }
            }
        } else if ch.is_alphabetic() || ch == '_' {
            let mut word = String::from(ch);
            while let Some((_, next)) = chars.peek() {
                if next.is_alphanumeric() || *next == '_' {
                    word.push(*next);
                    chars.next();
                } else {
                    break;
                }
            }
            match word.as_str() {
                "sin" => out.push_str("Math.sin"),
                "cos" => out.push_str("Math.cos"),
                "abs" => out.push_str("Math.abs"),
                _ => out.push_str(&word),
            }
        } else {
            out.push(ch);
        }
    }
    out
}

/// One colour channel: authoring-side when the parameter has code (a variable,
/// an expression), drawn-side when it does not.
fn style_color(
    param: &Parameter<Color>,
    fallback: Option<Color>,
    ctx: &EvaluationContext,
    warnings: &mut Vec<String>,
) -> ExportColor {
    match param {
        Parameter::Literal(value) => ExportColor::literal(*value),
        Parameter::Variable(name) => ExportColor {
            code: sanitize_ident(name),
            value: None,
            vars: vec![name.clone()],
        },
        Parameter::Procedural(output) => {
            warnings.push(format!(
                "a colour reading {} exports as its drawn value (no code)",
                describe_output(output)
            ));
            ExportColor::resolved_only(fallback)
        }
        other => ExportColor::resolved_only(other.resolve(ctx).ok().or(fallback)),
    }
}

/// The style block, authoring-side plus drawn-side.
fn style_of(
    node: &Node,
    resolved: Option<&EvaluatedNode>,
    ctx: &EvaluationContext,
    warnings: &mut Vec<String>,
) -> ExportStyle {
    let drawn: Option<&EvaluatedStyle> = resolved.map(|node| &node.style);
    // Task 10.2: a node can wear a *stack* of paints now. The code exporters
    // (React/Vue/Svelte) emit one `fill` and one `stroke` per element, so they
    // take the stack's first fill and first stroke — and say so, because a warning
    // the publisher can read beats a silently flattened logo. The SVG exporter
    // walks the whole stack; see `svg.rs`.
    let first = |stroke: bool| -> Option<&vectra_geometry::EvaluatedAppearance> {
        let style = drawn?;
        if stroke {
            style.first_stroke()
        } else {
            style.first_fill()
        }
    };
    // The warning is *built* here and pushed at the end of the function: the
    // `color` closure holds a mutable borrow of `warnings` for as long as it can
    // be called, and this is not a place to fight the borrow checker for style.
    let stack_note: Option<String> = drawn.and_then(|style| {
        let layers = style.appearances.iter().filter(|l| l.is_visible()).count();
        (layers > 1).then(|| {
            format!(
                "node {} has {layers} stacked appearance layers; code export draws the first fill and first stroke",
                node.id
            )
        })
    });

    let drawn_width = first(true)
        .and_then(|layer| layer.stroke_width())
        .unwrap_or(0.0);
    let stroke_width = scalar(
        &node.style.stroke_width,
        drawn.map(|_| drawn_width),
        ctx,
        "style.stroke_width",
        warnings,
    );
    let opacity = scalar(
        &node.style.opacity,
        drawn.map(|style| style.opacity),
        ctx,
        "style.opacity",
        warnings,
    );
    let drawn_fill = first(false).map(|layer| layer.paint.preview_color());
    let drawn_stroke = first(true).map(|layer| layer.paint.preview_color());
    let fill = style_color(&node.style.fill, drawn_fill, ctx, warnings);
    let stroke = style_color(&node.style.stroke, drawn_stroke, ctx, warnings);
    let style = ExportStyle {
        fill,
        stroke,
        stroke_width,
        opacity,
    };
    if let Some(note) = stack_note {
        warnings.push(note);
    }
    style
}

/// A computed node's style: whatever the evaluator resolved.
fn style_of_scene(style: &EvaluatedStyle) -> ExportStyle {
    ExportStyle {
        fill: ExportColor::resolved_only(style.first_fill().map(|l| l.paint.preview_color())),
        stroke: ExportColor::resolved_only(style.first_stroke().map(|l| l.paint.preview_color())),
        stroke_width: ExportParam::resolved_only(
            style.first_stroke().and_then(|l| l.stroke_width()),
        ),
        opacity: ExportParam::resolved_only(Some(style.opacity)),
    }
}

// ── bounds ─────────────────────────────────────────────────────────────

fn circle_bounds(cx: f64, cy: f64, r: f64) -> Option<ExportBounds> {
    if !(cx.is_finite() && cy.is_finite() && r.is_finite()) {
        return None;
    }
    let r = r.abs();
    Some(ExportBounds {
        min_x: cx - r,
        min_y: cy - r,
        max_x: cx + r,
        max_y: cy + r,
    })
}

/// The exact box of a **partial** arc: the endpoints plus whichever of the four
/// cardinal directions the sweep crosses. A full circle falls back to the whole
/// circle's box, which is then exact.
fn arc_bounds(cx: f64, cy: f64, r: f64, start: f64, end: f64) -> Option<ExportBounds> {
    if !(cx.is_finite() && cy.is_finite() && r.is_finite() && start.is_finite() && end.is_finite())
    {
        return None;
    }
    let r = r.abs();
    let sweep = end - start;
    if sweep >= std::f64::consts::TAU - 1e-12 {
        return circle_bounds(cx, cy, r);
    }
    let mut points: Vec<(f64, f64)> = vec![
        (cx + r * start.cos(), cy + r * start.sin()),
        (cx + r * end.cos(), cy + r * end.sin()),
    ];
    for direction in 0..4 {
        let angle = std::f64::consts::FRAC_PI_2 * direction as f64;
        if (angle - start).rem_euclid(std::f64::consts::TAU) <= sweep {
            points.push((cx + r * angle.cos(), cy + r * angle.sin()));
        }
    }
    Some(ExportBounds {
        min_x: points.iter().map(|p| p.0).fold(f64::INFINITY, f64::min),
        min_y: points.iter().map(|p| p.1).fold(f64::INFINITY, f64::min),
        max_x: points.iter().map(|p| p.0).fold(f64::NEG_INFINITY, f64::max),
        max_y: points.iter().map(|p| p.1).fold(f64::NEG_INFINITY, f64::max),
    })
}

fn primitive_bounds(primitive: &EvaluatedPrimitive) -> Option<ExportBounds> {
    match primitive {
        EvaluatedPrimitive::Rect { x, y, w, h, .. } => Some(ExportBounds {
            min_x: *x,
            min_y: *y,
            max_x: x + w,
            max_y: y + h,
        }),
        EvaluatedPrimitive::Circle { cx, cy, r } => circle_bounds(*cx, *cy, *r),
        EvaluatedPrimitive::Arc {
            cx,
            cy,
            r,
            start_angle,
            end_angle,
        } => arc_bounds(*cx, *cy, *r, *start_angle, *end_angle),
        // A path and a run both measure their letterform/curve points; a run's
        // box is exactly its glyphs' box — the same measurement the renderer's
        // culling and the exporter's viewBox need.
        EvaluatedPrimitive::Path(_) | EvaluatedPrimitive::Text(_) => outline_bounds(primitive),
    }
}

/// The union of every point of a node's outline geometry.
fn outline_bounds(primitive: &EvaluatedPrimitive) -> Option<ExportBounds> {
    let path = match primitive {
        EvaluatedPrimitive::Path(path) => path,
        EvaluatedPrimitive::Text(text) => &text.outline,
        _ => return None,
    };
    let mut bounds: Option<ExportBounds> = None;
    for event in path.iter() {
        for point in event_points(&event) {
            let box_ = ExportBounds {
                min_x: point.x,
                min_y: point.y,
                max_x: point.x,
                max_y: point.y,
            };
            bounds = Some(bounds.map_or(box_, |b| b.union(box_)));
        }
    }
    bounds
}

/// Every point an event contributes to a bounding box. Control points are
/// included, so a curve's box is an over-approximation (the hull's box) — for a
/// picture frame that is the safe direction.
fn event_points(event: &lyon::path::Event<lyon::math::Point, lyon::math::Point>) -> Vec<Point2> {
    use lyon::path::Event;
    match event {
        Event::Begin { at } => vec![Point2::new(f64::from(at.x), f64::from(at.y))],
        Event::Line { to, .. } => vec![Point2::new(f64::from(to.x), f64::from(to.y))],
        Event::Quadratic { ctrl, to, .. } => vec![
            Point2::new(f64::from(ctrl.x), f64::from(ctrl.y)),
            Point2::new(f64::from(to.x), f64::from(to.y)),
        ],
        Event::Cubic {
            ctrl1, ctrl2, to, ..
        } => vec![
            Point2::new(f64::from(ctrl1.x), f64::from(ctrl1.y)),
            Point2::new(f64::from(ctrl2.x), f64::from(ctrl2.y)),
            Point2::new(f64::from(to.x), f64::from(to.y)),
        ],
        Event::End { last, first, .. } => {
            vec![
                Point2::new(f64::from(last.x), f64::from(last.y)),
                Point2::new(f64::from(first.x), f64::from(first.y)),
            ]
        }
    }
}

/// Bounds of an authored path: resolve the points the context can give, and give
/// up politely on the rest (the node's `d` is then missing too, and that is
/// already a warning).
fn path_bounds(
    start: &Parameter<Point2>,
    segments: &[PathSegment],
    ctx: &EvaluationContext,
) -> Option<ExportBounds> {
    let mut points: Vec<Point2> = vec![start.resolve(ctx).ok()?];
    for segment in segments {
        match segment {
            PathSegment::Line { to } => points.push(to.resolve(ctx).ok()?),
            PathSegment::Quadratic { control, to } => {
                points.push(control.resolve(ctx).ok()?);
                points.push(to.resolve(ctx).ok()?);
            }
            PathSegment::Cubic {
                control1,
                control2,
                to,
            } => {
                points.push(control1.resolve(ctx).ok()?);
                points.push(control2.resolve(ctx).ok()?);
                points.push(to.resolve(ctx).ok()?);
            }
            PathSegment::Close => {}
        }
    }
    Some(ExportBounds {
        min_x: points.iter().map(|p| p.x).fold(f64::INFINITY, f64::min),
        min_y: points.iter().map(|p| p.y).fold(f64::INFINITY, f64::min),
        max_x: points.iter().map(|p| p.x).fold(f64::NEG_INFINITY, f64::max),
        max_y: points.iter().map(|p| p.y).fold(f64::NEG_INFINITY, f64::max),
    })
}

// ── small shared helpers ───────────────────────────────────────────────

/// Record the document's expression sources for the duration of a compilation.
/// Called by the entry points; public so tests can drive `build`-adjacent paths.
pub(crate) fn remember_expressions(doc: &Document) {
    CURRENT_EXPRESSIONS.with(|sources| {
        let mut sources = sources.borrow_mut();
        sources.clear();
        for (id, record) in &doc.expressions {
            sources.insert(*id, record.source.clone());
        }
    });
}

/// A number, formatted the way every exporter spells it: no locale, no exponent
/// noise, no trailing `.0`.
pub fn fmt_number(value: f64) -> String {
    if !value.is_finite() {
        return "0".to_string();
    }
    if value == value.trunc() && value.abs() < 1e15 {
        return format!("{}", value as i64);
    }
    let mut text = format!("{value:.6}");
    while text.ends_with('0') {
        text.pop();
    }
    if text.ends_with('.') {
        text.pop();
    }
    text
}

/// Make a variable name safe for a target language, keeping it recognisable.
pub fn sanitize_ident(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for (index, ch) in name.chars().enumerate() {
        let ok = if index == 0 {
            ch.is_alphabetic() || ch == '_' || ch == '$'
        } else {
            ch.is_alphanumeric() || ch == '_' || ch == '$'
        };
        out.push(if ok { ch } else { '_' });
    }
    if out.is_empty() {
        out.push('_');
    }
    out
}

fn describe_output(output: &NodeOutputId) -> String {
    format!(
        "⬡{}•{}",
        output.node.to_string().chars().take(8).collect::<String>(),
        output.port
    )
}

/// The `Parameter` slots of a geometry, in the kind's own field order — the same
/// order the evaluator's primitive uses, which is what makes the drawn numbers
/// line up positionally.
impl ExportGeometry {
    pub fn parameters(&self) -> Vec<&ExportParam> {
        match self {
            ExportGeometry::Rect {
                x,
                y,
                width,
                height,
                corner_radius,
            } => vec![x, y, width, height, corner_radius],
            ExportGeometry::Circle { cx, cy, radius } => vec![cx, cy, radius],
            ExportGeometry::Arc {
                cx,
                cy,
                radius,
                start_angle,
                end_angle,
            } => vec![cx, cy, radius, start_angle, end_angle],
            ExportGeometry::Path { .. } | ExportGeometry::Group { .. } => Vec::new(),
        }
    }
}
