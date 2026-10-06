//! **The appearance system** (Task 10.2 RULE 3): stacked paints, blend modes,
//! per-layer opacity and gradients.
//!
//! # Why this is a list and not two colours
//!
//! Phase 1 gave a node one `fill` colour and one `stroke` colour, which is
//! enough for a diagram and not enough for an illustration: the standard "thick
//! black stroke, thinner white stroke on top" logo is *two strokes*, and no
//! amount of per-node styling can express it. So a node wears an ordered list of
//! [`AppearanceLayer`]s, drawn back → front, each with its own paint, opacity and
//! blend mode. That ordering is the feature: the list *is* the compositing
//! order.
//!
//! # The compatibility decision, stated plainly
//!
//! `StyleProperties` keeps its four original fields (`fill`, `stroke`,
//! `stroke_width`, `opacity`) and documents written before this task keep loading
//! and *drawing identically*. They are not shadowed by the new list; they are its
//! **default**: an empty `appearances` means "the paint is exactly what the four
//! fields say", and the evaluator projects them into a one- or two-entry stack.
//! A non-empty `appearances` wins, because a document that has an explicit stack
//! has said what it wants. One rule, no dual truth: whoever reads a node's paint
//! reads [`StyleProperties::resolved_appearances`].
//!
//! # Parameters, not constants
//!
//! A gradient's *frame* (its start/end points, its centre and radius) is
//! `Parameter<…>`, exactly like every other coordinate in the engine, so a
//! gradient can be driven by a variable, an expression, motion or the
//! constraint solver. Its **stops** are structure — an ordered list whose
//! *length* can change — so they travel in a command
//! ([`crate::command::Command::SetAppearances`], which carries the whole
//! stack: a stop added, removed or recoloured is a new list) the same way a
//! path's vertex list travels in [`crate::command::Command::SetPath`], rather
//! than being addressed one slot at a time.

use serde::{Deserialize, Serialize};

use crate::geom::{Color, Point2};
use crate::param::Parameter;

/// How a layer's colour combines with what is already on the canvas.
///
/// The four modes every designer expects from the W3C compositing spec:
/// `Normal` replaces, `Multiply` darkens, `Screen` lightens, `Overlay`
/// multiplies the dark half of the backdrop and screens the light half.
///
/// They are *not* cosmetic: each is a different function, and the renderer
/// implements them exactly (see `vectra-render`'s backdrop compositing, verified
/// against pixels by `tests/gpu_laws.rs`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum BlendMode {
    #[default]
    Normal,
    Multiply,
    Screen,
    Overlay,
}

impl BlendMode {
    pub fn tag(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::Multiply => "multiply",
            Self::Screen => "screen",
            Self::Overlay => "overlay",
        }
    }

    /// Every mode, in the order the UI's icon row shows them.
    pub fn all() -> [BlendMode; 4] {
        [Self::Normal, Self::Multiply, Self::Screen, Self::Overlay]
    }

    /// Parse the wire spelling (`"multiply"`). Case-insensitive; `None` for an
    /// unknown name so the boundary can refuse it rather than guess.
    pub fn parse(name: &str) -> Option<Self> {
        match name.to_ascii_lowercase().as_str() {
            "normal" => Some(Self::Normal),
            "multiply" => Some(Self::Multiply),
            "screen" => Some(Self::Screen),
            "overlay" => Some(Self::Overlay),
            _ => None,
        }
    }

    /// Does this mode read the canvas underneath it?
    ///
    /// `Normal` composites with plain source-over and needs no backdrop; the
    /// other three are *functions of the backdrop colour*, which is why the
    /// renderer splits the frame and snapshots the target before drawing them.
    /// The UI uses the same predicate to decide whether a layer needs the
    /// isolated-compositing badge.
    pub fn needs_backdrop(self) -> bool {
        !matches!(self, Self::Normal)
    }
}

/// One colour stop of a gradient: where it sits (`0..=1`) and what it is.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct GradientStop {
    pub offset: f64,
    pub color: Color,
}

impl GradientStop {
    pub fn new(offset: f64, color: Color) -> Self {
        Self {
            offset: offset.clamp(0.0, 1.0),
            color,
        }
    }
}

/// The **shape** of a paint: flat colour, or a gradient.
///
/// A gradient's frame is authored in **document space**, like every other
/// coordinate in the engine: the UI drags a gradient handle at the document
/// point under the pointer, the exporter writes `gradientUnits="userSpaceOnUse"`,
/// and the shader evaluates the ramp at the fragment's document position. One
/// space for every channel means a gradient that is driven by a variable, a
/// constraint or motion behaves exactly like a rectangle's `x`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Paint {
    Solid(Parameter<Color>),
    Linear {
        start: Parameter<Point2>,
        end: Parameter<Point2>,
        stops: Vec<GradientStop>,
    },
    Radial {
        center: Parameter<Point2>,
        radius: Parameter<f64>,
        stops: Vec<GradientStop>,
    },
}

impl Paint {
    pub fn solid(color: Color) -> Self {
        Self::Solid(Parameter::Literal(color))
    }

    /// A two-stop linear ramp — the default a designer gets when they switch a
    /// fill from solid to linear.
    pub fn linear(from: Point2, to: Point2, a: Color, b: Color) -> Self {
        Self::Linear {
            start: Parameter::Literal(from),
            end: Parameter::Literal(to),
            stops: vec![GradientStop::new(0.0, a), GradientStop::new(1.0, b)],
        }
    }

    pub fn radial(center: Point2, radius: f64, a: Color, b: Color) -> Self {
        Self::Radial {
            center: Parameter::Literal(center),
            radius: Parameter::Literal(radius),
            stops: vec![GradientStop::new(0.0, a), GradientStop::new(1.0, b)],
        }
    }

    pub fn tag(&self) -> &'static str {
        match self {
            Self::Solid(_) => "solid",
            Self::Linear { .. } => "linear",
            Self::Radial { .. } => "radial",
        }
    }

    /// Is this paint a gradient? (i.e. does the renderer need the stop buffer?)
    pub fn is_gradient(&self) -> bool {
        !matches!(self, Self::Solid(_))
    }

    pub fn stops(&self) -> &[GradientStop] {
        match self {
            Self::Solid(_) => &[],
            Self::Linear { stops, .. } | Self::Radial { stops, .. } => stops,
        }
    }

    /// Stops in drawing order, first-to-last by offset, with a stable sort so
    /// two stops at the same offset keep the order the designer put them in.
    pub fn ordered_stops(&self) -> Vec<GradientStop> {
        let mut stops = self.stops().to_vec();
        stops.sort_by(|a, b| {
            a.offset
                .partial_cmp(&b.offset)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        stops
    }

    /// Visit every parametric slot this paint owns, in a stable order:
    /// `(suffix, point)` for points, then `(suffix, float)` for numbers.
    ///
    /// One implementation, used by the slot reader, the slot writer, the
    /// dependency-graph visitor and the AI's context scan — so a gradient's
    /// `end.y` is addressable, drivable and undoable in exactly the same way as
    /// a rectangle's `width`.
    pub fn visit_points<'a>(&'a self, mut point: impl FnMut(&'static str, &'a Parameter<Point2>)) {
        match self {
            Self::Solid(_) => {}
            Self::Linear { start, end, .. } => {
                point("start", start);
                point("end", end);
            }
            Self::Radial { center, .. } => point("center", center),
        }
    }

    pub fn visit_floats<'a>(&'a self, mut float: impl FnMut(&'static str, &'a Parameter<f64>)) {
        if let Self::Radial { radius, .. } = self {
            float("radius", radius);
        }
    }

    pub fn visit_colors<'a>(&'a self, mut color: impl FnMut(&'static str, &'a Parameter<Color>)) {
        if let Self::Solid(solid) = self {
            color("color", solid);
        }
    }

    /// Mutable access to the point slot with the given suffix.
    pub fn point_ref(&self, suffix: &str) -> Option<&Parameter<Point2>> {
        match (self, suffix) {
            (Self::Linear { start, .. }, "start") => Some(start),
            (Self::Linear { end, .. }, "end") => Some(end),
            (Self::Radial { center, .. }, "center") => Some(center),
            _ => None,
        }
    }

    pub fn float_ref(&self, suffix: &str) -> Option<&Parameter<f64>> {
        match (self, suffix) {
            (Self::Radial { radius, .. }, "radius") => Some(radius),
            _ => None,
        }
    }

    pub fn color_ref(&self, suffix: &str) -> Option<&Parameter<Color>> {
        match (self, suffix) {
            (Self::Solid(solid), "color") => Some(solid),
            _ => None,
        }
    }

    pub fn point_mut(&mut self, suffix: &str) -> Option<&mut Parameter<Point2>> {
        match (self, suffix) {
            (Self::Linear { start, .. }, "start") => Some(start),
            (Self::Linear { end, .. }, "end") => Some(end),
            (Self::Radial { center, .. }, "center") => Some(center),
            _ => None,
        }
    }

    pub fn float_mut(&mut self, suffix: &str) -> Option<&mut Parameter<f64>> {
        match (self, suffix) {
            (Self::Radial { radius, .. }, "radius") => Some(radius),
            _ => None,
        }
    }

    pub fn color_mut(&mut self, suffix: &str) -> Option<&mut Parameter<Color>> {
        match (self, suffix) {
            (Self::Solid(solid), "color") => Some(solid),
            _ => None,
        }
    }
}

/// What a layer *is*: a fill (the shape's interior) or a stroke (its outline).
///
/// Only a stroke carries a width, and that asymmetry is real: it is why a stroke
/// layer needs its own tessellation and a fill layer does not.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum AppearanceKind {
    Fill,
    Stroke { width: Parameter<f64> },
}

impl AppearanceKind {
    pub fn is_stroke(&self) -> bool {
        matches!(self, Self::Stroke { .. })
    }

    pub fn tag(&self) -> &'static str {
        match self {
            Self::Fill => "fill",
            Self::Stroke { .. } => "stroke",
        }
    }

    pub fn width(&self) -> Option<&Parameter<f64>> {
        match self {
            Self::Fill => None,
            Self::Stroke { width } => Some(width),
        }
    }

    pub fn width_mut(&mut self) -> Option<&mut Parameter<f64>> {
        match self {
            Self::Fill => None,
            Self::Stroke { width } => Some(width),
        }
    }
}

/// One entry of a node's paint stack: what it paints, how it combines, how
/// transparent it is, and whether it is drawn at all.
///
/// `visible` is per-layer rather than the whole node's business: hiding *one*
/// stroke of a two-stroke logo is a normal design move, and it must be as cheap
/// as hiding the node (RULE 4 — a flag the renderer reads, never a rewrite of the
/// geometry).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AppearanceLayer {
    pub kind: AppearanceKind,
    pub paint: Paint,
    pub opacity: Parameter<f64>,
    #[serde(default)]
    pub blend: BlendMode,
    #[serde(default = "default_true")]
    pub visible: bool,
}

fn default_true() -> bool {
    true
}

impl AppearanceLayer {
    pub fn fill(color: Color) -> Self {
        Self {
            kind: AppearanceKind::Fill,
            paint: Paint::solid(color),
            opacity: Parameter::Literal(1.0),
            blend: BlendMode::Normal,
            visible: true,
        }
    }

    pub fn stroke(color: Color, width: f64) -> Self {
        Self {
            kind: AppearanceKind::Stroke {
                width: Parameter::Literal(width),
            },
            paint: Paint::solid(color),
            opacity: Parameter::Literal(1.0),
            blend: BlendMode::Normal,
            visible: true,
        }
    }

    pub fn with_blend(mut self, blend: BlendMode) -> Self {
        self.blend = blend;
        self
    }

    pub fn with_opacity(mut self, opacity: f64) -> Self {
        self.opacity = Parameter::Literal(opacity);
        self
    }

    pub fn with_paint(mut self, paint: Paint) -> Self {
        self.paint = paint;
        self
    }

    /// The paint's flat colour parameter, when it is a solid. Used by the
    /// engine's "does this node read a source through a paint" scan.
    pub fn solid_color_paint(&self) -> Option<&Parameter<Color>> {
        match &self.paint {
            Paint::Solid(solid) => Some(solid),
            _ => None,
        }
    }

    /// The paint's flat colour, when it has one. Gradients have no single
    /// colour — `None` here is the honest answer that forces callers (the
    /// exporters, the inspector swatch) to handle both cases.
    pub fn solid_color(&self) -> Option<Color> {
        match &self.paint {
            Paint::Solid(Parameter::Literal(color)) => Some(*color),
            _ => None,
        }
    }
}
