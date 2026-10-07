//! Scene evaluation: `Document → EvaluatedScene` (Task 1.3).
//!
//! [`GeometryEvaluator`] resolves every [`vectra_core::Parameter`] through the
//! injected [`EvaluationContext`](vectra_core::EvaluationContext) and emits a
//! flat [`EvaluatedScene`](crate::scene::EvaluatedScene).
//!
//! # Totality contract
//! Evaluation never panics and never halts on a bad node:
//!
//! | Failure | Geometry | Style |
//! |---|---|---|
//! | Unresolvable parameter | skip node + [`Severity::Error`](crate::diagnostic::Severity) | default channel + `Warning` |
//! | Non-finite / out-of-GPU-range value | skip node + `Error` | default channel + `Warning` |
//! | Invalid-but-mappable (negative size, `opacity ∉ [0,1]`…) | clamp + `Warning` | clamp + `Warning` |
//!
//! Clamping (rather than skipping) mappable values keeps scene *membership*
//! stable across animation — a spring overshooting through a negative width
//! degrades to a degenerate rect instead of popping out of existence.
//!
//! # Incrementality (Phase-4 prep)
//! [`DirtySet`] restricts evaluation to explicit ids. Today callers pass ids
//! directly; the Phase-4 dependency graph will compute them from
//! variable/expression edits. [`EvaluatedScene::apply_partial`] patches a
//! cached scene with a partial result.

use crate::angles::normalize_arc_angles;
use crate::diagnostic::{Diagnostic, DiagnosticCode};
use crate::paint::{
    EvaluatedAppearance, EvaluatedAppearanceKind, EvaluatedGradient, EvaluatedPaint,
};
use crate::paths::{build_path, ResolvedSegment};
use crate::scene::{
    EvaluatedNode, EvaluatedPrimitive, EvaluatedScene, EvaluatedStyle, EvaluatedText,
};
use crate::text::{layout_text, layout_text_on_path, resolve_face, FaceRef, TextError, TextSpec};
use std::collections::HashSet;
use vectra_core::{
    Color, Document, EvaluationContext, Node, NodeId, NodeKind, Parameter, PathSegment, Point2,
    Resolvable,
};

/// Largest finite magnitude that survives `f64 → f32` for GPU upload.
/// Anything beyond [`f32::MAX`] (or NaN/infinity) is non-renderable.
pub const MAX_RENDERABLE: f64 = f32::MAX as f64;

/// True iff `v` is finite and GPU-representable.
pub fn is_renderable(v: f64) -> bool {
    v.is_finite() && v.abs() <= MAX_RENDERABLE
}

/// True iff both coordinates of `p` are [`is_renderable`].
pub fn is_renderable_point(p: Point2) -> bool {
    is_renderable(p.x) && is_renderable(p.y)
}

/// Restricts evaluation to an explicit id set.
///
/// * [`DirtySet::all`] (empty) — evaluate everything (full rebuild).
/// * [`DirtySet::nodes`] — evaluate only these ids (incremental patch).
///
/// Unknown ids are ignored, never an error: the set is a *hint boundary* for
/// the future dependency graph, not a precondition.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DirtySet {
    ids: HashSet<NodeId>,
}

impl DirtySet {
    /// Full evaluation (no restriction).
    pub fn all() -> Self {
        Self::default()
    }

    /// Incremental evaluation of exactly these ids.
    pub fn nodes(ids: impl IntoIterator<Item = NodeId>) -> Self {
        Self {
            ids: ids.into_iter().collect(),
        }
    }

    /// Incremental evaluation of a single id.
    pub fn single(id: NodeId) -> Self {
        Self::nodes([id])
    }

    /// True ⟺ no restriction ⟺ full evaluation.
    pub fn is_full(&self) -> bool {
        self.ids.is_empty()
    }

    /// Number of explicit ids (`0` ⟺ [`DirtySet::is_full`]).
    pub fn len(&self) -> usize {
        self.ids.len()
    }

    /// True ⟺ no explicit ids ⟺ full evaluation.
    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }

    pub fn contains(&self, id: NodeId) -> bool {
        self.ids.contains(&id)
    }

    pub fn ids(&self) -> &HashSet<NodeId> {
        &self.ids
    }
}

/// The output of one evaluation pass: the scene plus every note about
/// fallbacks, clamps, and skips encountered along the way.
#[derive(Debug, Clone, Default)]
pub struct SceneEvaluation {
    pub scene: EvaluatedScene,
    pub diagnostics: Vec<Diagnostic>,
}

impl SceneEvaluation {
    /// True if any node was skipped.
    pub fn has_errors(&self) -> bool {
        self.diagnostics.iter().any(Diagnostic::is_error)
    }

    pub fn errors(&self) -> impl Iterator<Item = &Diagnostic> {
        self.diagnostics.iter().filter(|d| d.is_error())
    }

    pub fn warnings(&self) -> impl Iterator<Item = &Diagnostic> {
        self.diagnostics
            .iter()
            .filter(|d| !d.is_error() && d.severity == crate::diagnostic::Severity::Warning)
    }
}

/// Document → scene evaluation.
pub trait Evaluator {
    fn evaluate(
        &self,
        doc: &Document,
        ctx: &EvaluationContext,
        dirty: &DirtySet,
    ) -> SceneEvaluation;
}

/// The canonical [`Evaluator`]: resolves parameters, canonicalizes arcs,
/// builds lyon paths, and reports diagnostics. Stateless — all inputs are
/// passed per call so cached scenes stay a caller concern.
#[derive(Debug, Clone, Copy, Default)]
pub struct GeometryEvaluator;

impl GeometryEvaluator {
    pub fn new() -> Self {
        Self
    }

    /// Convenience: full evaluation of every node in document order.
    pub fn evaluate_full(&self, doc: &Document, ctx: &EvaluationContext) -> SceneEvaluation {
        self.evaluate(doc, ctx, &DirtySet::all())
    }
}

impl Evaluator for GeometryEvaluator {
    fn evaluate(
        &self,
        doc: &Document,
        ctx: &EvaluationContext,
        dirty: &DirtySet,
    ) -> SceneEvaluation {
        let mut scene = EvaluatedScene::empty();
        let mut diagnostics = Vec::new();

        let targets: Vec<NodeId> = if dirty.is_full() {
            doc.order.clone()
        } else {
            doc.order
                .iter()
                .copied()
                .filter(|id| dirty.contains(*id))
                .collect()
        };

        for id in targets {
            // Defensive: core keeps `order` in sync, but evaluation must stay
            // total even against a hand-built or migrated document.
            let Some(node) = doc.nodes.get(&id) else {
                continue;
            };
            // **Effective** presentation flags: the node's own eye/padlock
            // combined with its layer's (Task 10.2). Resolved here, at evaluation
            // time, into the scene — which is why the renderer only ever copies
            // two booleans and never walks the layer registry (RULE 4).
            let visible = doc.visible(id);
            let locked = doc.locked(id);
            if let Some(evaluated) =
                evaluate_node(node, doc, ctx, visible, locked, &mut diagnostics)
            {
                scene.nodes.insert(id, evaluated);
            }
        }

        // Partial scenes carry a partial z-order (only evaluated ids, still in
        // document order); `apply_partial` rebuilds the global order.
        scene.z_order = doc
            .order
            .iter()
            .copied()
            .filter(|id| scene.nodes.contains_key(id))
            .collect();
        scene.debug_assert_invariants();

        SceneEvaluation { scene, diagnostics }
    }
}

fn evaluate_node(
    node: &Node,
    doc: &Document,
    ctx: &EvaluationContext,
    visible: bool,
    locked: bool,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<EvaluatedNode> {
    let primitive = evaluate_primitive(node, doc, ctx, diagnostics)?;
    let style = resolve_style(node, ctx, diagnostics);
    Some(EvaluatedNode {
        id: node.id,
        primitive,
        style,
        // RULE 4: presentation flags travel with the evaluated node, so the
        // renderer and the hit index read them from the scene they already hold
        // rather than reaching back into the document.
        visible,
        locked,
    })
}

/// Resolve one node's geometry, **without** its style or its presentation
/// flags.
///
/// Split out of [`evaluate_node`] for one caller: a text run bound to a path must
/// read that path's geometry, and it must do so *during its own* evaluation,
/// whatever position the path happens to occupy in the document's draw order.
/// Reading the scene instead would make a run's picture depend on evaluation
/// order — the one thing this crate's totality contract exists to prevent.
fn evaluate_primitive(
    node: &Node,
    doc: &Document,
    ctx: &EvaluationContext,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<EvaluatedPrimitive> {
    let id = node.id;
    let primitive = match &node.kind {
        // Structural only in Phase 1 (no transforms): flatten. Children keep
        // their own `order` slots, so no diagnostic — this is exact, not lossy.
        NodeKind::Group { .. } => return None,
        NodeKind::Rectangle {
            x,
            y,
            width,
            height,
            corner_radius,
        } => {
            let x = resolve_scalar(x, ctx, id, "x", diagnostics).ok()?;
            let y = resolve_scalar(y, ctx, id, "y", diagnostics).ok()?;
            let w = clamp_non_negative(
                resolve_scalar(width, ctx, id, "width", diagnostics).ok()?,
                id,
                "width",
                diagnostics,
            );
            let h = clamp_non_negative(
                resolve_scalar(height, ctx, id, "height", diagnostics).ok()?,
                id,
                "height",
                diagnostics,
            );
            let max_r = 0.5 * w.min(h);
            let corner_radius = clamp_range(
                resolve_scalar(corner_radius, ctx, id, "corner_radius", diagnostics).ok()?,
                0.0,
                max_r,
                id,
                "corner_radius",
                diagnostics,
            );
            EvaluatedPrimitive::Rect {
                x,
                y,
                w,
                h,
                corner_radius,
            }
        }
        NodeKind::Circle { cx, cy, radius } => {
            let cx = resolve_scalar(cx, ctx, id, "cx", diagnostics).ok()?;
            let cy = resolve_scalar(cy, ctx, id, "cy", diagnostics).ok()?;
            let r = clamp_non_negative(
                resolve_scalar(radius, ctx, id, "radius", diagnostics).ok()?,
                id,
                "radius",
                diagnostics,
            );
            EvaluatedPrimitive::Circle { cx, cy, r }
        }
        NodeKind::Arc {
            cx,
            cy,
            radius,
            start_angle,
            end_angle,
        } => {
            let cx = resolve_scalar(cx, ctx, id, "cx", diagnostics).ok()?;
            let cy = resolve_scalar(cy, ctx, id, "cy", diagnostics).ok()?;
            let r = clamp_non_negative(
                resolve_scalar(radius, ctx, id, "radius", diagnostics).ok()?,
                id,
                "radius",
                diagnostics,
            );
            let start = resolve_scalar(start_angle, ctx, id, "start_angle", diagnostics).ok()?;
            let end = resolve_scalar(end_angle, ctx, id, "end_angle", diagnostics).ok()?;
            let (start_angle, end_angle) = normalize_arc_angles(start, end);
            EvaluatedPrimitive::Arc {
                cx,
                cy,
                r,
                start_angle,
                end_angle,
            }
        }
        NodeKind::Path { start, segments } => {
            let start = resolve_point(start, ctx, id, "start", diagnostics).ok()?;
            let mut resolved = Vec::with_capacity(segments.len());
            for (i, segment) in segments.iter().enumerate() {
                let property = format!("segments[{i}]");
                match segment {
                    PathSegment::Line { to } => {
                        resolved.push(ResolvedSegment::Line {
                            to: resolve_point(to, ctx, id, &property, diagnostics).ok()?,
                        });
                    }
                    PathSegment::Quadratic { control, to } => {
                        resolved.push(ResolvedSegment::Quadratic {
                            control: resolve_point(control, ctx, id, &property, diagnostics)
                                .ok()?,
                            to: resolve_point(to, ctx, id, &property, diagnostics).ok()?,
                        });
                    }
                    PathSegment::Cubic {
                        control1,
                        control2,
                        to,
                    } => {
                        resolved.push(ResolvedSegment::Cubic {
                            control1: resolve_point(control1, ctx, id, &property, diagnostics)
                                .ok()?,
                            control2: resolve_point(control2, ctx, id, &property, diagnostics)
                                .ok()?,
                            to: resolve_point(to, ctx, id, &property, diagnostics).ok()?,
                        });
                    }
                    PathSegment::Close => resolved.push(ResolvedSegment::Close),
                }
            }
            EvaluatedPrimitive::Path(build_path(start, &resolved))
        }
        // **Text (Task 11.0 RULE 1).** A run's geometry *is* its glyph
        // outlines: they are shaped and laid out, in document space, right here.
        // Everything downstream then treats text like any other primitive —
        // there is no text branch in the tessellator, the hit test or the
        // exporter, because by the time a run leaves this function it is a path.
        //
        // Every number that can drive the layout was resolved through the
        // ordinary parameter door, which is what makes RULE 1's promise true:
        // change a `font_size` variable, an expression or a spring and the very
        // next evaluation lays the glyphs out again, at the new size.
        NodeKind::Text {
            text,
            font_family,
            font_size,
            letter_spacing,
            line_height,
            alignment,
            x,
            y,
            on_path,
        } => {
            let size = clamp_non_negative(
                resolve_scalar(font_size, ctx, id, "font_size", diagnostics).ok()?,
                id,
                "font_size",
                diagnostics,
            );
            let letter_spacing =
                resolve_scalar(letter_spacing, ctx, id, "letter_spacing", diagnostics).ok()?;
            // Leading is a multiple of the size, so its meaningful range is
            // small: a negative multiple would set lines on top of each other in
            // reverse, and a huge one is a typo. Clamped, never fatal — the same
            // policy as a negative width.
            let line_height = clamp_range(
                resolve_scalar(line_height, ctx, id, "line_height", diagnostics).ok()?,
                0.0,
                100.0,
                id,
                "line_height",
                diagnostics,
            );
            let (face, substituted) = resolve_face(ctx.fonts, font_family);
            if substituted {
                diagnostics.push(Diagnostic::warning(
                    id,
                    "font_family",
                    DiagnosticCode::FontFallback,
                    format!(
                        "font family {:?} is not in the host's font library; shaped with the bundled face",
                        font_family
                    ),
                ));
            }
            let spec = TextSpec {
                text,
                family: font_family,
                size,
                letter_spacing,
                line_height,
                alignment: *alignment,
            };
            // Where the run goes is decided **before** the face is asked to
            // shape it: the two are independent, and splitting them here is what
            // makes the fallback retry below a plain second call.
            let placement = match on_path {
                // RULE 2: the run follows a *source* node's geometry, which is
                // resolved on the spot. Reading it from the cached scene instead
                // would make the run's picture depend on which node the
                // evaluator happened to visit first.
                Some(binding) => {
                    let offset =
                        resolve_scalar(&binding.offset, ctx, id, "path_offset", diagnostics)
                            .ok()?;
                    match bound_source_path(binding.node, doc, ctx, diagnostics) {
                        Some(path) => Placement::OnPath(path, offset),
                        // The binding's target is gone (or is not path-shaped):
                        // an empty run, not a failure. Undoing the delete of a
                        // bound path restores the run exactly.
                        None => Placement::Nowhere,
                    }
                }
                None => Placement::Straight(
                    resolve_scalar(x, ctx, id, "x", diagnostics).ok()?,
                    resolve_scalar(y, ctx, id, "y", diagnostics).ok()?,
                ),
            };
            let lay_out = |face: FaceRef<'_>| -> Result<EvaluatedText, TextError> {
                match &placement {
                    Placement::Straight(x, y) => layout_text(face, &spec, (*x, *y)),
                    Placement::OnPath(path, offset) => {
                        layout_text_on_path(face, &spec, path, *offset)
                    }
                    Placement::Nowhere => Ok(EvaluatedText::empty()),
                }
            };
            let mut laid_out = lay_out(face);
            // **A face that cannot be used is not the end of the words.** A host
            // may hand over bytes no shaper can read (a custom provider is not
            // required to validate its faces the way `FontLibrary` does). Rather
            // than skipping the node — which would make a constraint that reads
            // the run's box see it vanish — the run is drawn with the bundled
            // face and the substitution is reported, exactly like a family that
            // is not installed.
            if matches!(
                laid_out,
                Err(TextError::InvalidFace(_)) | Err(TextError::DegenerateFace)
            ) && !face.is_bundled()
            {
                diagnostics.push(Diagnostic::warning(
                    id,
                    "font_family",
                    DiagnosticCode::FontFallback,
                    format!(
                        "font family {:?} could not be used as a font; shaped with the bundled face",
                        font_family
                    ),
                ));
                laid_out = lay_out(FaceRef::bundled());
            }
            match laid_out {
                Ok(run) => {
                    if run.truncated > 0 {
                        diagnostics.push(Diagnostic::warning(
                            id,
                            "path_offset",
                            DiagnosticCode::TextOverflow,
                            format!(
                                "{} glyph(s) of this run do not fit on the bound path; they are left out, the rest keep their spacing",
                                run.truncated
                            ),
                        ));
                    }
                    EvaluatedPrimitive::Text(run)
                }
                Err(error) => {
                    diagnostics.push(Diagnostic::error(
                        id,
                        "text",
                        DiagnosticCode::TextLayoutFailed,
                        format!("{error}; node skipped"),
                    ));
                    return None;
                }
            }
        }
    };
    Some(primitive)
}

/// Where a run goes once it is shaped: the two independent halves of a text
/// node's geometry, split so a face failure never has to be resolved twice.
enum Placement {
    /// On its own baseline, at the node's `(x, y)`.
    Straight(f64, f64),
    /// Along a source's evaluated geometry, slid by `offset` document units.
    OnPath(lyon::path::Path, f64),
    /// Bound to a source that no longer exists (or is not path-shaped): an empty
    /// run, not a failure.
    Nowhere,
}

/// The path a bound run follows: the **evaluated geometry** of the binding's
/// target, resolved on the spot.
///
/// Only path-shaped kinds are followed ([`Document::is_text_path_source`], the
/// same predicate `Command::BindTextToPath` validates against), so a binding can
/// never chain text to text — however a document was authored or hand-edited,
/// this recursion is one level deep.
fn bound_source_path(
    node_id: NodeId,
    doc: &Document,
    ctx: &EvaluationContext,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<lyon::path::Path> {
    let node = doc.nodes.get(&node_id)?;
    if !Document::is_text_path_source(&node.kind) {
        return None;
    }
    let primitive = evaluate_primitive(node, doc, ctx, diagnostics)?;
    // The **curve** view, not the region polygonization: a run's rotation is
    // read from this path's tangents, and a 64-gon's edge direction jumps by
    // 5.6° at every vertex however large the circle is (see
    // [`crate::paths::primitive_to_curve_path`]). The polygonized view is the
    // right answer for fills, bools and hit tests — just not for tangents.
    Some(crate::paths::primitive_to_curve_path(&primitive))
}

/// Resolve a geometry scalar: failure or non-renderable value skips the node.
fn resolve_scalar(
    param: &Parameter<f64>,
    ctx: &EvaluationContext,
    node_id: NodeId,
    property: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<f64, ()> {
    match param.resolve(ctx) {
        Ok(v) if is_renderable(v) => Ok(v),
        Ok(v) => {
            diagnostics.push(Diagnostic::error(
                node_id,
                property,
                DiagnosticCode::NonFiniteValue,
                format!("{property} resolved to non-renderable {v:?}; node skipped"),
            ));
            Err(())
        }
        Err(e) => {
            diagnostics.push(Diagnostic::error(
                node_id,
                property,
                DiagnosticCode::UnresolvableParameter,
                format!("{property} unresolvable ({e}); node skipped"),
            ));
            Err(())
        }
    }
}

/// Resolve a geometry point: failure or non-renderable coordinate skips the node.
fn resolve_point(
    param: &Parameter<Point2>,
    ctx: &EvaluationContext,
    node_id: NodeId,
    property: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<Point2, ()> {
    match param.resolve(ctx) {
        Ok(p) if is_renderable_point(p) => Ok(p),
        Ok(p) => {
            diagnostics.push(Diagnostic::error(
                node_id,
                property,
                DiagnosticCode::NonFiniteValue,
                format!("{property} resolved to non-renderable {p:?}; node skipped"),
            ));
            Err(())
        }
        Err(e) => {
            diagnostics.push(Diagnostic::error(
                node_id,
                property,
                DiagnosticCode::UnresolvableParameter,
                format!("{property} unresolvable ({e}); node skipped"),
            ));
            Err(())
        }
    }
}

fn clamp_non_negative(
    v: f64,
    node_id: NodeId,
    property: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> f64 {
    if v < 0.0 {
        diagnostics.push(Diagnostic::warning(
            node_id,
            property,
            DiagnosticCode::ClampedValue,
            format!("{property} was {v}; clamped to 0"),
        ));
        0.0
    } else {
        v
    }
}

fn clamp_range(
    v: f64,
    lo: f64,
    hi: f64,
    node_id: NodeId,
    property: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> f64 {
    let clamped = v.clamp(lo, hi);
    if clamped != v {
        diagnostics.push(Diagnostic::warning(
            node_id,
            property,
            DiagnosticCode::ClampedValue,
            format!("{property} was {v}; clamped to [{lo}, {hi}]"),
        ));
    }
    clamped
}

/// Style never skips a node: unresolvable channels fall back to defaults.
fn resolve_style(
    node: &Node,
    ctx: &EvaluationContext,
    diagnostics: &mut Vec<Diagnostic>,
) -> EvaluatedStyle {
    let id = node.id;
    let defaults = EvaluatedStyle::default();

    let opacity = clamp_range(
        resolve_style_scalar(
            &node.style.opacity,
            ctx,
            id,
            "style.opacity",
            defaults.opacity,
            diagnostics,
        ),
        0.0,
        1.0,
        id,
        "style.opacity",
        diagnostics,
    );

    // The stack the node actually paints. `resolved_appearances` is the single
    // reader of the "explicit stack wins, else project the legacy fill/stroke
    // pair" rule, so a document written before Task 10.2 and one written after it
    // take the same path from here on.
    let authored = node.style.resolved_appearances();
    let mut appearances = Vec::with_capacity(authored.len());
    for (index, layer) in authored.iter().enumerate() {
        let slot = |suffix: &str| format!("style.appearances[{index}].{suffix}");
        let paint = resolve_paint(layer, ctx, id, &slot, &defaults, diagnostics);
        let layer_opacity = clamp_range(
            resolve_style_scalar(&layer.opacity, ctx, id, &slot("opacity"), 1.0, diagnostics),
            0.0,
            1.0,
            id,
            &slot("opacity"),
            diagnostics,
        );
        let stroke_width = layer.kind.width().map(|width| {
            clamp_non_negative(
                resolve_style_scalar(width, ctx, id, &slot("width"), 0.0, diagnostics),
                id,
                &slot("width"),
                diagnostics,
            )
        });
        appearances.push(EvaluatedAppearance {
            kind: match stroke_width {
                Some(width) => EvaluatedAppearanceKind::Stroke { width },
                None => EvaluatedAppearanceKind::Fill,
            },
            paint,
            opacity: layer_opacity,
            blend: layer.blend,
            visible: layer.visible,
        });
    }
    if appearances.is_empty() {
        // `resolved_appearances` never returns an empty stack; this is the belt
        // to its braces so a future caller cannot produce an unpaintable node.
        appearances = defaults.appearances.clone();
    }

    EvaluatedStyle {
        appearances,
        opacity,
    }
}

/// Resolve one layer's paint, falling back to the first default fill's colour
/// with a diagnostic when a channel will not resolve.
///
/// A gradient's **frame** resolves like any other point or scalar: an
/// unresolvable axis keeps the authored value if it is a literal (so the shape
/// still draws where the designer put it) and otherwise falls back — the same
/// policy the geometry channels already follow.
fn resolve_paint(
    layer: &vectra_core::AppearanceLayer,
    ctx: &EvaluationContext,
    node_id: NodeId,
    slot: &dyn Fn(&str) -> String,
    defaults: &EvaluatedStyle,
    diagnostics: &mut Vec<Diagnostic>,
) -> EvaluatedPaint {
    use vectra_core::Paint;
    let fallback_color = defaults
        .first_fill()
        .map(|layer| layer.paint.preview_color())
        .unwrap_or(Color::BLACK);
    let stop_fallback = fallback_color;
    match &layer.paint {
        Paint::Solid(color) => {
            let property = slot("paint.color");
            let resolved = match color.resolve(ctx) {
                Ok(color) => color,
                Err(error) => {
                    diagnostics.push(Diagnostic::warning(
                        node_id,
                        property.clone(),
                        DiagnosticCode::StyleFallback,
                        format!("{property} unresolvable ({error}); fell back to default"),
                    ));
                    fallback_color
                }
            };
            EvaluatedPaint::Solid(resolved)
        }
        Paint::Linear { start, end, stops } => {
            let start = resolve_paint_point(start, ctx, node_id, &slot("paint.start"), diagnostics);
            let end = resolve_paint_point(end, ctx, node_id, &slot("paint.end"), diagnostics);
            EvaluatedPaint::Linear {
                start,
                end,
                gradient: EvaluatedGradient::new(stops, stop_fallback),
            }
        }
        Paint::Radial {
            center,
            radius,
            stops,
        } => {
            let center =
                resolve_paint_point(center, ctx, node_id, &slot("paint.center"), diagnostics);
            let radius = clamp_non_negative(
                resolve_style_scalar(
                    radius,
                    ctx,
                    node_id,
                    &slot("paint.radius"),
                    0.0,
                    diagnostics,
                ),
                node_id,
                &slot("paint.radius"),
                diagnostics,
            );
            EvaluatedPaint::Radial {
                center,
                radius,
                gradient: EvaluatedGradient::new(stops, stop_fallback),
            }
        }
    }
}

/// Resolve a gradient frame point, falling back to its literal (or the origin)
/// so an unresolvable frame still paints something in the right place.
fn resolve_paint_point(
    param: &Parameter<Point2>,
    ctx: &EvaluationContext,
    node_id: NodeId,
    property: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Point2 {
    match param.resolve(ctx) {
        Ok(point) => point,
        Err(error) => {
            let fallback = match param {
                Parameter::Literal(point) => *point,
                _ => Point2::ZERO,
            };
            diagnostics.push(Diagnostic::warning(
                node_id,
                property.to_string(),
                DiagnosticCode::StyleFallback,
                format!("{property} unresolvable ({error}); fell back to {fallback:?}"),
            ));
            fallback
        }
    }
}

fn resolve_style_scalar(
    param: &Parameter<f64>,
    ctx: &EvaluationContext,
    node_id: NodeId,
    property: &str,
    fallback: f64,
    diagnostics: &mut Vec<Diagnostic>,
) -> f64 {
    match param.resolve(ctx) {
        Ok(v) if is_renderable(v) => v,
        Ok(v) => {
            diagnostics.push(Diagnostic::warning(
                node_id,
                property,
                DiagnosticCode::NonFiniteValue,
                format!("{property} resolved to non-renderable {v:?}; fell back to {fallback}"),
            ));
            fallback
        }
        Err(e) => {
            diagnostics.push(Diagnostic::warning(
                node_id,
                property,
                DiagnosticCode::StyleFallback,
                format!("{property} unresolvable ({e}); fell back to {fallback}"),
            ));
            fallback
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vectra_core::{Color, Engine};

    fn ctx_of(engine: &Engine) -> EvaluationContext<'_> {
        engine.evaluation_context()
    }

    #[test]
    fn literal_document_evaluates_clean() {
        let mut engine = Engine::new();
        let id = vectra_core::new_node_id();
        engine
            .dispatch(vectra_core::Command::CreateNode {
                id,
                kind: NodeKind::rectangle(1.0, 2.0, 100.0, 50.0),
                name: None,
                index: None,
            })
            .unwrap();
        let eval = GeometryEvaluator.evaluate_full(engine.document(), &ctx_of(&engine));
        assert!(eval.diagnostics.is_empty());
        assert!(!eval.has_errors());
        let node = eval.scene.get(id).unwrap();
        assert!(matches!(
            node.primitive,
            EvaluatedPrimitive::Rect {
                x: 1.0,
                y: 2.0,
                w: 100.0,
                h: 50.0,
                corner_radius: 0.0
            }
        ));
    }

    #[test]
    fn missing_variable_skips_node_but_not_scene() {
        let mut engine = Engine::new();
        let bad = vectra_core::new_node_id();
        let good = vectra_core::new_node_id();
        engine
            .dispatch(vectra_core::Command::CreateNode {
                id: bad,
                kind: NodeKind::circle(0.0, 0.0, 10.0),
                name: None,
                index: None,
            })
            .unwrap();
        engine
            .dispatch(vectra_core::Command::CreateNode {
                id: good,
                kind: NodeKind::circle(5.0, 5.0, 3.0),
                name: None,
                index: None,
            })
            .unwrap();
        engine
            .dispatch(vectra_core::Command::SetParameter {
                node_id: bad,
                property: "radius".to_string(),
                value: vectra_core::ParamValue::Float(Parameter::variable("nope")),
            })
            .unwrap();

        let eval = GeometryEvaluator.evaluate_full(engine.document(), &ctx_of(&engine));
        assert!(eval.has_errors());
        assert!(eval.scene.get(bad).is_none(), "bad node must be skipped");
        assert!(eval.scene.get(good).is_some(), "good node must survive");
        assert_eq!(eval.scene.z_order, vec![good]);
        let err: Vec<_> = eval.errors().collect();
        assert_eq!(err.len(), 1);
        assert_eq!(err[0].code, DiagnosticCode::UnresolvableParameter);
        assert_eq!(err[0].property.as_deref(), Some("radius"));
    }

    #[test]
    fn negative_size_clamps_and_warns_rather_than_skipping() {
        let mut engine = Engine::new();
        let id = vectra_core::new_node_id();
        engine
            .dispatch(vectra_core::Command::CreateNode {
                id,
                kind: NodeKind::Rectangle {
                    x: Parameter::Literal(0.0),
                    y: Parameter::Literal(0.0),
                    width: Parameter::Literal(-40.0),
                    height: Parameter::Literal(10.0),
                    corner_radius: Parameter::Literal(0.0),
                },
                name: None,
                index: None,
            })
            .unwrap();
        let eval = GeometryEvaluator.evaluate_full(engine.document(), &ctx_of(&engine));
        assert!(!eval.has_errors());
        assert_eq!(eval.warnings().count(), 1);
        assert!(matches!(
            eval.scene.get(id).unwrap().primitive,
            EvaluatedPrimitive::Rect { w: 0.0, .. }
        ));
    }

    #[test]
    fn groups_flatten_silently() {
        let mut engine = Engine::new();
        let group = vectra_core::new_node_id();
        let child = vectra_core::new_node_id();
        engine
            .dispatch(vectra_core::Command::CreateNode {
                id: child,
                kind: NodeKind::circle(1.0, 2.0, 3.0),
                name: None,
                index: None,
            })
            .unwrap();
        engine
            .dispatch(vectra_core::Command::CreateNode {
                id: group,
                kind: NodeKind::Group {
                    children: vec![child],
                },
                name: None,
                index: None,
            })
            .unwrap();
        let eval = GeometryEvaluator.evaluate_full(engine.document(), &ctx_of(&engine));
        assert!(eval.diagnostics.is_empty());
        assert!(eval.scene.get(group).is_none());
        assert!(eval.scene.get(child).is_some());
    }

    #[test]
    fn partial_evaluation_restricts_to_dirty_ids() {
        let mut engine = Engine::new();
        let a = vectra_core::new_node_id();
        let b = vectra_core::new_node_id();
        for (id, w) in [(a, 1.0), (b, 2.0)] {
            engine
                .dispatch(vectra_core::Command::CreateNode {
                    id,
                    kind: NodeKind::rectangle(0.0, 0.0, w, 1.0),
                    name: None,
                    index: None,
                })
                .unwrap();
        }
        let eval =
            GeometryEvaluator.evaluate(engine.document(), &ctx_of(&engine), &DirtySet::single(a));
        assert_eq!(eval.scene.len(), 1);
        assert!(eval.scene.get(a).is_some());
        assert_eq!(eval.scene.z_order, vec![a]);

        // Unknown ids are ignored, never an error.
        let eval = GeometryEvaluator.evaluate(
            engine.document(),
            &ctx_of(&engine),
            &DirtySet::single(vectra_core::new_node_id()),
        );
        assert!(eval.scene.is_empty());
        assert!(eval.diagnostics.is_empty());
    }

    #[test]
    fn style_failures_fall_back_without_skipping() {
        let mut engine = Engine::new();
        let id = vectra_core::new_node_id();
        engine
            .dispatch(vectra_core::Command::CreateNode {
                id,
                kind: NodeKind::circle(0.0, 0.0, 5.0),
                name: None,
                index: None,
            })
            .unwrap();
        // Opacity bound to a missing variable; fill is a literal (kept).
        engine
            .dispatch(vectra_core::Command::SetParameter {
                node_id: id,
                property: "style.opacity".to_string(),
                value: vectra_core::ParamValue::Float(Parameter::variable("ghost")),
            })
            .unwrap();
        let eval = GeometryEvaluator.evaluate_full(engine.document(), &ctx_of(&engine));
        assert!(!eval.has_errors());
        let node = eval.scene.get(id).unwrap();
        assert_eq!(node.style.opacity, 1.0);
        assert_eq!(
            node.style.first_fill().unwrap().paint.preview_color(),
            Color::rgb(0x22, 0x66, 0xee)
        );
    }

    #[test]
    fn opacity_is_clamped_to_unit_range() {
        let mut engine = Engine::new();
        let id = vectra_core::new_node_id();
        engine
            .dispatch(vectra_core::Command::CreateNode {
                id,
                kind: NodeKind::circle(0.0, 0.0, 5.0),
                name: None,
                index: None,
            })
            .unwrap();
        engine
            .dispatch(vectra_core::Command::SetParameter {
                node_id: id,
                property: "style.opacity".to_string(),
                value: vectra_core::ParamValue::float_literal(7.5),
            })
            .unwrap();
        let eval = GeometryEvaluator.evaluate_full(engine.document(), &ctx_of(&engine));
        assert_eq!(eval.scene.get(id).unwrap().style.opacity, 1.0);
        assert_eq!(eval.warnings().count(), 1);
    }
}
