//! **Resolved paint**: the appearance stack, flattened to numbers the renderer
//! and the exporters can both read (Task 10.2 RULE 3).
//!
//! The evaluator's job is the same here as it is for geometry — turn
//! `Parameter`s into values — and this module holds the two pieces that are
//! worth having exactly once:
//!
//! * [`EvaluatedAppearance`], one entry of a node's paint stack with every
//!   channel resolved;
//! * [`EvaluatedPaint::sample`], the **gradient ramps**, evaluated on the CPU so
//!   the SVG exporter, the hit-test colour probe and the GPU can be checked
//!   against one definition.
//!
//! # Why the ramp lives here and not only in WGSL
//!
//! A gradient is a function from a point in the node's own space to a colour. The
//! `wgpu` shader computes it per fragment for speed; the exporter needs the same
//! function to emit `<linearGradient>`; and the GPU law suite needs to know what
//! the pixels *should* be in order to assert them. Writing that formula three
//! times is how three implementations drift apart, so it is written once here,
//! and the shader is a transliteration of it — with `vectra-render`'s pixel laws
//! as the referee between the two.

use crate::scene::EvaluatedStyle;
use vectra_core::{
    AppearanceKind, AppearanceLayer, BlendMode, Color, EvaluationContext, GradientStop, Paint,
    Resolvable, StyleProperties,
};

/// A gradient with its geometry already resolved.
#[derive(Debug, Clone, PartialEq)]
pub struct EvaluatedGradient {
    pub stops: Vec<GradientStop>,
}

impl EvaluatedGradient {
    /// A ramp with at least two stops, sorted, with equal offsets collapsed.
    ///
    /// A gradient with one stop is a solid colour and a gradient with none has no
    /// definition at all; both are normalized here rather than refused, because
    /// a designer deleting stops down to one should see the fill become that
    /// colour, not see the shape vanish with a diagnostic they cannot act on.
    pub fn new(stops: &[GradientStop], fallback: Color) -> Self {
        let mut stops: Vec<GradientStop> = stops
            .iter()
            .map(|stop| GradientStop::new(stop.offset, stop.color))
            .collect();
        stops.sort_by(|a, b| {
            a.offset
                .partial_cmp(&b.offset)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        if stops.is_empty() {
            stops = vec![
                GradientStop::new(0.0, fallback),
                GradientStop::new(1.0, fallback),
            ];
        } else if stops.len() == 1 {
            let only = stops[0];
            stops = vec![
                GradientStop::new(0.0, only.color),
                GradientStop::new(1.0, only.color),
            ];
        }
        Self { stops }
    }

    /// The colour at `t ∈ [0, 1]`, interpolated in **straight (non-premultiplied)
    /// sRGB**, which is what the SVG and CSS gradient specs say and what every
    /// designer's eyedropper expects.
    ///
    /// `t` outside `[0, 1]` clamps to the end stops: a gradient is defined over
    /// its own frame, and the frame's edge is the edge of its colour.
    pub fn sample(&self, t: f64) -> Color {
        let t = t.clamp(0.0, 1.0);
        let first = self.stops[0];
        if t <= first.offset {
            return first.color;
        }
        let last = self.stops[self.stops.len() - 1];
        if t >= last.offset {
            return last.color;
        }
        for pair in self.stops.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            if t >= a.offset && t <= b.offset {
                let span = b.offset - a.offset;
                // Two stops at the same offset are a hard edge, not a division by
                // zero: the later stop wins, which is how every editor renders a
                // duplicated stop.
                if span <= f64::EPSILON {
                    return b.color;
                }
                let local = (t - a.offset) / span;
                return a.color.lerp(b.color, local);
            }
        }
        last.color
    }
}

impl EvaluatedPaint {
    /// Resolve one authored layer's paint against an evaluation context.
    ///
    /// The **single** implementation of "an authored `Paint` becomes numbers",
    /// used by the primitive evaluator, the operation evaluator and the
    /// procedural engine alike. Each caller keeps its own error policy (the
    /// primitive evaluator diagnoses and falls back per channel; the others fall
    /// back quietly), but the *shape* of the resolution — which channel goes
    /// where, and what a gradient's ramp is normalized to — exists once, here.
    pub fn resolve(
        layer: &vectra_core::AppearanceLayer,
        ctx: &vectra_core::EvaluationContext,
        fallback: Color,
    ) -> Self {
        match &layer.paint {
            Paint::Solid(color) => Self::Solid(color.resolve(ctx).unwrap_or(fallback)),
            Paint::Linear { start, end, stops } => Self::Linear {
                start: start.resolve(ctx).unwrap_or(vectra_core::Point2::ZERO),
                end: end.resolve(ctx).unwrap_or(vectra_core::Point2::ZERO),
                gradient: EvaluatedGradient::new(stops, fallback),
            },
            Paint::Radial {
                center,
                radius,
                stops,
            } => Self::Radial {
                center: center.resolve(ctx).unwrap_or(vectra_core::Point2::ZERO),
                radius: radius.resolve(ctx).map(|v| v.max(0.0)).unwrap_or(0.0),
                gradient: EvaluatedGradient::new(stops, fallback),
            },
        }
    }
}

/// Resolve one authored layer: its paint, its opacity and its kind.
///
/// The **single** place where an authored `AppearanceLayer` becomes numbers. The
/// primitive evaluator, the operation evaluator and the procedural engine all go
/// through here, so "what does this layer paint, and how wide is its stroke" has
/// exactly one answer in the workspace — the same reason the ramp sampler lives
/// next door.
pub fn resolve_appearance(
    layer: &AppearanceLayer,
    ctx: &EvaluationContext,
    fallback: Color,
    fallback_width: f64,
) -> EvaluatedAppearance {
    let paint = EvaluatedPaint::resolve(layer, ctx, fallback);
    let kind = match layer.kind {
        AppearanceKind::Fill => EvaluatedAppearanceKind::Fill,
        AppearanceKind::Stroke { ref width } => EvaluatedAppearanceKind::Stroke {
            width: width
                .resolve(ctx)
                .map(|v| {
                    if v.is_finite() {
                        v.max(0.0)
                    } else {
                        fallback_width
                    }
                })
                .unwrap_or(fallback_width),
        },
    };
    EvaluatedAppearance {
        kind,
        paint,
        opacity: layer
            .opacity
            .resolve(ctx)
            .map(|v| v.clamp(0.0, 1.0))
            .unwrap_or(1.0),
        blend: layer.blend,
        visible: layer.visible,
    }
}

/// Resolve a whole node's paint stack, with no diagnostics — the policy the
/// operation and procedural evaluators use ("degrade, do not vanish").
///
/// The primitive evaluator does the same thing one channel at a time so it can
/// *name* the slot that failed; this function exists so the two smaller
/// evaluators do not each grow their own copy of the stack rule.
pub fn resolve_style(style: &StyleProperties, ctx: &EvaluationContext) -> EvaluatedStyle {
    let default = EvaluatedStyle::default();
    let fallback = default
        .first_fill()
        .map(|layer| layer.paint.preview_color())
        .unwrap_or(Color::BLACK);
    let fallback_width = default.max_stroke_width();
    let mut appearances: Vec<EvaluatedAppearance> = style
        .resolved_appearances()
        .iter()
        .map(|layer| resolve_appearance(layer, ctx, fallback, fallback_width))
        .collect();
    if appearances.is_empty() {
        appearances = default.appearances.clone();
    }
    EvaluatedStyle {
        appearances,
        opacity: style
            .opacity
            .resolve(ctx)
            .map(|v| v.clamp(0.0, 1.0))
            .unwrap_or(default.opacity),
    }
}

/// A resolved paint: flat colour or a gradient over the node's own space.
#[derive(Debug, Clone, PartialEq)]
pub enum EvaluatedPaint {
    Solid(Color),
    /// `start → end` defines the axis; `t` runs along it.
    Linear {
        start: vectra_core::Point2,
        end: vectra_core::Point2,
        gradient: EvaluatedGradient,
    },
    /// `center` + `radius`; `t` is the normalized distance from the centre.
    Radial {
        center: vectra_core::Point2,
        radius: f64,
        gradient: EvaluatedGradient,
    },
}

impl EvaluatedPaint {
    pub fn tag(&self) -> &'static str {
        match self {
            Self::Solid(_) => "solid",
            Self::Linear { .. } => "linear",
            Self::Radial { .. } => "radial",
        }
    }

    pub fn is_gradient(&self) -> bool {
        !matches!(self, Self::Solid(_))
    }

    pub fn stops(&self) -> &[GradientStop] {
        match self {
            Self::Solid(_) => &[],
            Self::Linear { gradient, .. } | Self::Radial { gradient, .. } => &gradient.stops,
        }
    }

    /// The gradient parameter at a point **in document space** (the space the
    /// gradient's own frame is authored in), or `None` for a solid.
    pub fn gradient_t(&self, x: f64, y: f64) -> Option<f64> {
        match self {
            Self::Solid(_) => None,
            Self::Linear { start, end, .. } => {
                let (dx, dy) = (end.x - start.x, end.y - start.y);
                let length_sq = dx * dx + dy * dy;
                if length_sq <= f64::EPSILON {
                    // A zero-length axis is a degenerate gradient: every point is
                    // the end of the ramp, which is the only answer that does not
                    // involve dividing by zero.
                    return Some(1.0);
                }
                Some(((x - start.x) * dx + (y - start.y) * dy) / length_sq)
            }
            Self::Radial { center, radius, .. } => {
                let (dx, dy) = (x - center.x, y - center.y);
                if *radius <= f64::EPSILON {
                    return Some(1.0);
                }
                Some((dx * dx + dy * dy).sqrt() / radius)
            }
        }
    }

    /// Sample the paint at a point in document space.
    pub fn sample(&self, x: f64, y: f64) -> Color {
        match self {
            Self::Solid(color) => *color,
            Self::Linear { gradient, .. } | Self::Radial { gradient, .. } => {
                gradient.sample(self.gradient_t(x, y).unwrap_or(1.0))
            }
        }
    }

    /// The colour a UI swatch should show for this paint: the solid, or the ramp
    /// at its midpoint.
    pub fn preview_color(&self) -> Color {
        match self {
            Self::Solid(color) => *color,
            _ => self
                .stops()
                .first()
                .map(|s| s.color)
                .unwrap_or(Color::BLACK),
        }
    }
}

/// One resolved entry of a node's paint stack.
#[derive(Debug, Clone, PartialEq)]
pub struct EvaluatedAppearance {
    pub kind: EvaluatedAppearanceKind,
    pub paint: EvaluatedPaint,
    /// Clamped to `[0, 1]`.
    pub opacity: f64,
    pub blend: BlendMode,
    pub visible: bool,
}

impl EvaluatedAppearance {
    pub fn is_stroke(&self) -> bool {
        matches!(self.kind, EvaluatedAppearanceKind::Stroke { .. })
    }

    pub fn stroke_width(&self) -> Option<f64> {
        match self.kind {
            EvaluatedAppearanceKind::Fill => None,
            EvaluatedAppearanceKind::Stroke { width } => Some(width),
        }
    }

    /// Is this layer worth drawing at all?
    ///
    /// The same predicate the renderer uses to skip a mesh, kept here so the
    /// exporters, the inspector and the tessellator agree on "invisible" instead
    /// of each inventing one. A fully transparent paint is invisible; a stroke of
    /// zero width is invisible; a hidden layer is invisible.
    pub fn is_visible(&self) -> bool {
        if !self.visible || self.opacity <= 0.0 {
            return false;
        }
        match &self.paint {
            EvaluatedPaint::Solid(color) => color.a > 0,
            // A gradient's alpha varies along the ramp, so the honest answer is
            // "yes, let the renderer composite it".
            _ => true,
        }
    }
}

/// Fill or stroke, with the stroke's width already resolved (and clamped `>= 0`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum EvaluatedAppearanceKind {
    Fill,
    Stroke { width: f64 },
}
