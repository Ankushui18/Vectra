//! Graph-oriented document model (MES §4, §5).
//!
//! No flat JSON blobs: the document is a collection of interconnected
//! registries (`variables`, `nodes`, `expressions`, `constraints`,
//! `operations`) joined by [`crate::ids`] keys. The dependency graph
//! (MES §8, `petgraph`, implemented in `vectra-constraints` / engine)
//! traverses exactly these joins to compute minimal dirty sets.
//!
//! ## Scalar-first geometry
//! Rectangle / Circle / Arc decompose into scalar [`Parameter<f64>`] fields
//! (`x`, `y`, `width`, `radius`, …) because constraints and expressions are
//! scalar systems. `Rectangle.width` is therefore addressable as a
//! first-class [`ParamValue::Float`]. Composite [`Point2`] parameters are
//! reserved for path vertices / control points, where vector binding is
//! geometrically meaningful.

use crate::constraint::ConstraintRegistry;
use crate::error::{ResolveError, VectraError};
use crate::eval::EvaluationContext;
use crate::geom::{Color, Point2};
use crate::ids::{new_node_id, ArtboardId, ExpressionId, LayerId, NodeId, TrackId, VariableId};
use crate::layers::{ArtboardRecord, ArtboardRegistry, LayerRecord, LayerRegistry};
use crate::operation::OperationRegistry;
use crate::param::{NodeOutputId, ParamValue, Parameter};
use crate::style::{AppearanceKind, AppearanceLayer, BlendMode, Paint};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap};

/// Every procedural port this authored node reads, in **slot order**: floats
/// first, then the two style colours, then a path's points.
///
/// The mirror of [`Node::for_each_float_param`], which visits only floats — the
/// cycle gate cannot afford to miss a point or a colour slot, because a slot
/// that reads a port is where a disguised cycle starts.
fn collect_node_procedural_refs(node: &Node, out: &mut Vec<NodeOutputId>) {
    node.for_each_float_param(|_, param| {
        if let Parameter::Procedural(output) = param {
            out.push(output.clone());
        }
    });
    for color in [&node.style.fill, &node.style.stroke] {
        if let Parameter::Procedural(output) = color {
            out.push(output.clone());
        }
    }
    // The appearance stack is addressable, so it can read a port too — and a
    // slot that reads a port is where a disguised cycle starts, which is exactly
    // what this scan exists to catch.
    for layer in &node.style.appearances {
        layer.paint.visit_colors(|_, color| {
            if let Parameter::Procedural(output) = color {
                out.push(output.clone());
            }
        });
        layer.paint.visit_points(|_, point| {
            if let Parameter::Procedural(output) = point {
                out.push(output.clone());
            }
        });
    }
    if let NodeKind::Path { start, segments } = &node.kind {
        let visit_point = |param: &Parameter<Point2>, out: &mut Vec<NodeOutputId>| {
            if let Parameter::Procedural(output) = param {
                out.push(output.clone());
            }
        };
        visit_point(start, out);
        for segment in segments {
            match segment {
                PathSegment::Line { to } => visit_point(to, out),
                PathSegment::Quadratic { control, to } => {
                    visit_point(control, out);
                    visit_point(to, out);
                }
                PathSegment::Cubic {
                    control1,
                    control2,
                    to,
                } => {
                    visit_point(control1, out);
                    visit_point(control2, out);
                    visit_point(to, out);
                }
                PathSegment::Close => {}
            }
        }
    }
}

/// Current on-disk / over-the-wire document version.
pub const DOCUMENT_VERSION: u32 = 1;

/// A single path segment. All endpoints are parametric.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum PathSegment {
    Line {
        to: Parameter<Point2>,
    },
    Quadratic {
        control: Parameter<Point2>,
        to: Parameter<Point2>,
    },
    Cubic {
        control1: Parameter<Point2>,
        control2: Parameter<Point2>,
        to: Parameter<Point2>,
    },
    Close,
}

impl PathSegment {
    /// Every parametric point this segment owns, in slot order.
    ///
    /// The single source for "what points does a segment have": `Node::path_slots`
    /// names them, the evaluator resolves them, the AI's context check scans them
    /// for node references, and the drawing tools read them to find a segment's
    /// handles. `Close` owns none — it is a flag, not a vertex.
    pub fn params(&self) -> Vec<&Parameter<Point2>> {
        match self {
            Self::Line { to } => vec![to],
            Self::Quadratic { control, to } => vec![control, to],
            Self::Cubic {
                control1,
                control2,
                to,
            } => vec![control1, control2, to],
            Self::Close => Vec::new(),
        }
    }
}

/// True mathematical primitive kinds — never bare point arrays (MES §5).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum NodeKind {
    Rectangle {
        x: Parameter<f64>,
        y: Parameter<f64>,
        width: Parameter<f64>,
        height: Parameter<f64>,
        corner_radius: Parameter<f64>,
    },
    Circle {
        cx: Parameter<f64>,
        cy: Parameter<f64>,
        radius: Parameter<f64>,
    },
    Arc {
        cx: Parameter<f64>,
        cy: Parameter<f64>,
        radius: Parameter<f64>,
        start_angle: Parameter<f64>,
        end_angle: Parameter<f64>,
    },
    Path {
        start: Parameter<Point2>,
        segments: Vec<PathSegment>,
    },
    Group {
        children: Vec<NodeId>,
    },
}

impl NodeKind {
    /// Stable kind tag for errors and snapshots.
    pub fn tag(&self) -> &'static str {
        match self {
            Self::Rectangle { .. } => "Rectangle",
            Self::Circle { .. } => "Circle",
            Self::Arc { .. } => "Arc",
            Self::Path { .. } => "Path",
            Self::Group { .. } => "Group",
        }
    }

    /// This kind's addressable slots, in the order every surface shows them.
    ///
    /// The list is the *contract* of [`Node::get_param`]: a caller that wants to
    /// enumerate a node's parameters (a document summary for an AI, an
    /// inspector, a diff) reads it here instead of keeping a second copy that
    /// can drift. Geometry slots only — style slots are uniform across kinds and
    /// live in `crate::summary::STYLE_SLOTS`.
    pub fn scalar_slots(&self) -> &'static [&'static str] {
        match self {
            Self::Rectangle { .. } => &["x", "y", "width", "height", "corner_radius"],
            Self::Circle { .. } => &["cx", "cy", "radius"],
            Self::Arc { .. } => &["cx", "cy", "radius", "start_angle", "end_angle"],
            // A `Path`'s slot space is *dynamic*: `start` plus three point slots
            // for every segment. The static list can only name the one slot that
            // always exists — [`Node::path_slots`] enumerates the rest, and
            // [`Node::get_param`] / [`Node::set_param`] address them.
            Self::Path { .. } => &["start"],
            Self::Group { .. } => &[],
        }
    }

    /// Name-only view of a path's slots (the borrow-friendly twin of
    /// [`Node::path_slots`], which needs a whole `Node`).
    pub(crate) fn path_slot_names(&self) -> Vec<String> {
        let NodeKind::Path { segments, .. } = self else {
            return Vec::new();
        };
        let mut slots = vec!["start".to_string()];
        for (index, segment) in segments.iter().enumerate() {
            match segment {
                PathSegment::Line { .. } => slots.push(format!("segments[{index}].to")),
                PathSegment::Quadratic { .. } => {
                    slots.push(format!("segments[{index}].control"));
                    slots.push(format!("segments[{index}].to"));
                }
                PathSegment::Cubic { .. } => {
                    slots.push(format!("segments[{index}].control1"));
                    slots.push(format!("segments[{index}].control2"));
                    slots.push(format!("segments[{index}].to"));
                }
                PathSegment::Close => {}
            }
        }
        slots
    }

    /// The node's **canonical position slots**: `("x", "y")` for a rectangle,
    /// `("cx", "cy")` for a circle or arc, `None` for kinds with no position
    /// (Phase-1 `Path` / `Group`).
    ///
    /// This is the one place that decides what "the position of a node" means,
    /// so a drag (Task 3.2) and every constraint address the same slots for a
    /// given kind.
    pub fn position_slots(&self) -> Option<(&'static str, &'static str)> {
        match self {
            Self::Rectangle { .. } => Some(("x", "y")),
            Self::Circle { .. } | Self::Arc { .. } => Some(("cx", "cy")),
            Self::Path { .. } | Self::Group { .. } => None,
        }
    }

    /// Canonical spelling of one property path: the same slot must never have
    /// two names, or the solver would intern two variables for it.
    ///
    /// `get_param` accepts aliases (`x` on a circle, `r` for `radius`, …) — this
    /// maps them onto the canonical name and passes everything else through.
    pub fn canonical_slot(&self, property: &str) -> String {
        match self {
            Self::Rectangle { .. } => match property {
                "radius" => "corner_radius".to_string(),
                other => other.to_string(),
            },
            Self::Circle { .. } | Self::Arc { .. } => match property {
                "x" | "center.x" => "cx".to_string(),
                "y" | "center.y" => "cy".to_string(),
                "r" => "radius".to_string(),
                "start" => "start_angle".to_string(),
                "end" => "end_angle".to_string(),
                other => other.to_string(),
            },
            Self::Path { .. } | Self::Group { .. } => property.to_string(),
        }
    }

    pub fn rectangle(x: f64, y: f64, width: f64, height: f64) -> Self {
        Self::Rectangle {
            x: Parameter::Literal(x),
            y: Parameter::Literal(y),
            width: Parameter::Literal(width),
            height: Parameter::Literal(height),
            corner_radius: Parameter::Literal(0.0),
        }
    }

    pub fn circle(cx: f64, cy: f64, radius: f64) -> Self {
        Self::Circle {
            cx: Parameter::Literal(cx),
            cy: Parameter::Literal(cy),
            radius: Parameter::Literal(radius),
        }
    }

    pub fn arc(cx: f64, cy: f64, radius: f64, start_angle: f64, end_angle: f64) -> Self {
        Self::Arc {
            cx: Parameter::Literal(cx),
            cy: Parameter::Literal(cy),
            radius: Parameter::Literal(radius),
            start_angle: Parameter::Literal(start_angle),
            end_angle: Parameter::Literal(end_angle),
        }
    }
}

/// Fill / stroke / opacity, **plus the appearance stack** (Task 10.2 RULE 3).
///
/// Every channel is parametric so themes can bind `style.fill` to a
/// variable-driven procedural output. The four original fields are the *default
/// stack*: when `appearances` is empty the node paints exactly what those fields
/// say (which is what every document written before Task 10.2 does, and why they
/// keep loading and drawing identically), and when it is non-empty the stack is
/// what the designer authored. [`StyleProperties::resolved_appearances`] is the
/// single reader of that rule — see the module docs in [`crate::style`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StyleProperties {
    pub fill: Parameter<Color>,
    pub stroke: Parameter<Color>,
    pub stroke_width: Parameter<f64>,
    pub opacity: Parameter<f64>,
    /// Stacked fills and strokes, drawn back → front.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub appearances: Vec<AppearanceLayer>,
}

impl Default for StyleProperties {
    fn default() -> Self {
        Self {
            fill: Parameter::Literal(Color::rgb(0x22, 0x66, 0xee)),
            stroke: Parameter::Literal(Color::TRANSPARENT),
            stroke_width: Parameter::Literal(0.0),
            opacity: Parameter::Literal(1.0),
            appearances: Vec::new(),
        }
    }
}

impl StyleProperties {
    /// The paint stack this node actually draws, **never empty**.
    ///
    /// An explicit stack is returned as-is. Otherwise the legacy fill/stroke pair
    /// is projected into one or two layers:
    ///
    /// * the fill is always present, because a shape with no paint at all is a
    ///   mis-drawn shape rather than a design choice — a transparent fill is how
    ///   a designer says "no fill", and it costs one skipped draw, not an
    ///   invented one;
    /// * the stroke appears only when it would actually be drawn (`width > 0`
    ///   **and** a non-zero alpha), matching [`crate::document::Node::style`]'s
    ///   existing rule and `vectra-render`'s `stroke_key_of`.
    pub fn resolved_appearances(&self) -> Vec<AppearanceLayer> {
        if !self.appearances.is_empty() {
            return self.appearances.clone();
        }
        let mut stack = vec![AppearanceLayer {
            kind: AppearanceKind::Fill,
            paint: Paint::Solid(self.fill.clone()),
            opacity: Parameter::Literal(1.0),
            blend: BlendMode::Normal,
            visible: true,
        }];
        let width_is_visible = match &self.stroke_width {
            Parameter::Literal(width) => *width > 0.0,
            // A non-literal width is "unknown until evaluated": include the
            // layer and let the evaluator decide, exactly as it does today.
            _ => true,
        };
        let stroke_is_visible = match &self.stroke {
            Parameter::Literal(color) => color.a > 0,
            _ => true,
        };
        if width_is_visible && stroke_is_visible {
            stack.push(AppearanceLayer {
                kind: AppearanceKind::Stroke {
                    width: self.stroke_width.clone(),
                },
                paint: Paint::Solid(self.stroke.clone()),
                opacity: Parameter::Literal(1.0),
                blend: BlendMode::Normal,
                visible: true,
            });
        }
        stack
    }

    /// Addressable appearance slots, in draw order:
    /// `style.appearances[2].paint.end.y`, `style.appearances[0].opacity`, …
    ///
    /// Exposed so the inspector, the AI summary and the boundary all enumerate
    /// the same list, and so a *stack edit* can be narrated to the designer
    /// without either side re-deriving it.
    pub fn appearance_slots(&self) -> Vec<String> {
        let mut slots = Vec::new();
        for (index, layer) in self.appearances.iter().enumerate() {
            let base = format!("style.appearances[{index}]");
            slots.push(format!("{base}.opacity"));
            if layer.kind.is_stroke() {
                slots.push(format!("{base}.width"));
            }
            layer.paint.visit_points(|suffix, _| {
                slots.push(format!("{base}.paint.{suffix}.x"));
                slots.push(format!("{base}.paint.{suffix}.y"));
            });
            layer
                .paint
                .visit_floats(|suffix, _| slots.push(format!("{base}.paint.{suffix}")));
            layer
                .paint
                .visit_colors(|suffix, _| slots.push(format!("{base}.paint.{suffix}")));
        }
        slots
    }
}

/// `"appearances[2].paint.end.y"` → `(2, ";paint.end.y")`.
///
/// The leading `;` is the "there was a base to strip" marker: the reader and the
/// writer both need to know that a slot had an *index* at all, because
/// `paint.end.y` with no appearance at the front is not an appearance slot.
fn parse_appearance_slot(slot: &str) -> Option<(usize, &str)> {
    let rest = slot.strip_prefix("appearances[")?;
    let (index, rest) = rest.split_once(']')?;
    let index: usize = index.parse().ok()?;
    Some((index, rest))
}

/// Read a slot inside one appearance layer: `.paint.end.y`, `.width`, …
fn read_appearance_slot(
    layer: &AppearanceLayer,
    rest: &str,
    property: &str,
) -> Result<ParamValue, VectraError> {
    match rest {
        ".opacity" => Ok(ParamValue::Float(layer.opacity.clone())),
        ".width" => match layer.kind.width() {
            Some(width) => Ok(ParamValue::Float(width.clone())),
            None => Err(VectraError::UnknownProperty {
                node_id: NodeId::nil(),
                node_kind: "fill".to_string(),
                property: property.to_string(),
            }),
        },
        other => {
            if let Some(suffix) = other.strip_prefix(".paint.") {
                if let Some(suffix) = suffix.strip_suffix(".x") {
                    if let Some(point) = layer.paint.point_ref(suffix) {
                        return Ok(ParamValue::Float(Node::component_of(point, property, 0)?));
                    }
                }
                if let Some(suffix) = suffix.strip_suffix(".y") {
                    if let Some(point) = layer.paint.point_ref(suffix) {
                        return Ok(ParamValue::Float(Node::component_of(point, property, 1)?));
                    }
                }
                if let Some(radius) = layer.paint.float_ref(suffix) {
                    return Ok(ParamValue::Float(radius.clone()));
                }
                if let Some(color) = layer.paint.color_ref(suffix) {
                    return Ok(ParamValue::Color(color.clone()));
                }
            }
            Err(VectraError::UnknownProperty {
                node_id: NodeId::nil(),
                node_kind: layer.kind.tag().to_string(),
                property: property.to_string(),
            })
        }
    }
}

/// Write a slot inside one appearance layer, returning the replaced value.
fn write_appearance_slot(
    layer: &mut AppearanceLayer,
    rest: &str,
    property: &str,
    value: ParamValue,
) -> Result<ParamValue, VectraError> {
    let mismatched =
        |expected: &'static str, got: &'static str| VectraError::PropertyTypeMismatch {
            property: property.to_string(),
            expected,
            got,
        };
    match rest {
        ".opacity" => match value {
            ParamValue::Float(v) => Ok(ParamValue::Float(std::mem::replace(&mut layer.opacity, v))),
            other => Err(mismatched("float", other.kind())),
        },
        ".width" => match value {
            ParamValue::Float(v) => match layer.kind.width_mut() {
                Some(width) => Ok(ParamValue::Float(std::mem::replace(width, v))),
                None => Err(VectraError::UnknownProperty {
                    node_id: NodeId::nil(),
                    node_kind: layer.kind.tag().to_string(),
                    property: property.to_string(),
                }),
            },
            other => Err(mismatched("float", other.kind())),
        },
        other => {
            let Some(suffix) = other.strip_prefix(".paint.") else {
                return Err(VectraError::UnknownProperty {
                    node_id: NodeId::nil(),
                    node_kind: layer.kind.tag().to_string(),
                    property: property.to_string(),
                });
            };
            // A point component: `end.y` writes half of a `Parameter<Point2>`,
            // which is how the constraint solver already addresses a path
            // vertex — same convention, same inverses.
            let component = if suffix.ends_with(".x") {
                Some(0usize)
            } else if suffix.ends_with(".y") {
                Some(1usize)
            } else {
                None
            };
            if let (Some(component), Some(name)) = (
                component,
                suffix
                    .strip_suffix(".x")
                    .or_else(|| suffix.strip_suffix(".y")),
            ) {
                if layer.paint.point_ref(name).is_some() {
                    let ParamValue::Float(float) = value else {
                        return Err(mismatched("float", value.kind()));
                    };
                    let Parameter::Literal(float) = float else {
                        return Err(VectraError::ComponentRequiresLiteral {
                            property: property.to_string(),
                        });
                    };
                    let Some(point) = layer.paint.point_mut(name) else {
                        return Err(VectraError::UnknownProperty {
                            node_id: NodeId::nil(),
                            node_kind: layer.kind.tag().to_string(),
                            property: property.to_string(),
                        });
                    };
                    // Literal-only, exactly like `segments[i].to.x`: a component
                    // write is a *scalar* view of a vector slot, and there is no
                    // honest scalar view of a vector whose source is a variable.
                    let Parameter::Literal(point) = point else {
                        return Err(VectraError::ComponentRequiresLiteral {
                            property: property.to_string(),
                        });
                    };
                    let previous = if component == 0 { point.x } else { point.y };
                    if component == 0 {
                        point.x = float;
                    } else {
                        point.y = float;
                    }
                    return Ok(ParamValue::Float(Parameter::Literal(previous)));
                }
            }
            if let Some(radius) = layer.paint.float_mut(suffix) {
                return match value {
                    ParamValue::Float(v) => Ok(ParamValue::Float(std::mem::replace(radius, v))),
                    other => Err(mismatched("float", other.kind())),
                };
            }
            if let Some(color) = layer.paint.color_mut(suffix) {
                return match value {
                    ParamValue::Color(v) => Ok(ParamValue::Color(std::mem::replace(color, v))),
                    other => Err(mismatched("color", other.kind())),
                };
            }
            Err(VectraError::UnknownProperty {
                node_id: NodeId::nil(),
                node_kind: layer.kind.tag().to_string(),
                property: property.to_string(),
            })
        }
    }
}

/// Append a layer's live children, skipping duplicates and dead ids.
///
/// A node can only be listed once, and a layer can name a node that was deleted
/// after it was added; both are normal, so both are handled here rather than at
/// every call site.
fn push_layer_children(flat: &mut Vec<NodeId>, layer: &LayerRecord, nodes: &HashMap<NodeId, Node>) {
    for child in &layer.children {
        if nodes.contains_key(child) && !flat.contains(child) {
            flat.push(*child);
        }
    }
}

/// A scene node: identity + typed geometry + style + presentation flags.
///
/// `visible` and `locked` live here rather than in the layer record because they
/// are *node* properties that survive being moved between layers, and because a
/// document with no layers must still be able to hide one shape.
///
/// Both are `#[serde(default)]`, so a document written before Task 10.2 loads
/// with every node visible and unlocked — the only reading that can be right.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Node {
    pub id: NodeId,
    pub name: String,
    pub kind: NodeKind,
    pub style: StyleProperties,
    #[serde(default = "default_node_visible")]
    pub visible: bool,
    #[serde(default)]
    pub locked: bool,
}

fn default_node_visible() -> bool {
    true
}

impl Node {
    pub fn new(id: NodeId, name: impl Into<String>, kind: NodeKind) -> Self {
        Self {
            id,
            name: name.into(),
            kind,
            style: StyleProperties::default(),
            visible: true,
            locked: false,
        }
    }

    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }

    /// The canonical position slots of this node, if any (see
    /// [`NodeKind::position_slots`]).
    pub fn position_slots(&self) -> Option<(&'static str, &'static str)> {
        self.kind.position_slots()
    }

    /// Read a property by path. Supported paths:
    ///
    /// * Rectangle: `x y width height corner_radius`
    /// * Circle: `cx cy radius` (+ aliases `x y`)
    /// * Arc: `cx cy radius start_angle end_angle` (+ `start`/`end`, `x`/`y`)
    /// * Path: `start`
    /// * Any node: `style.fill style.stroke style.stroke_width style.opacity`, plus `name` via [`ParamValue`]? (names are plain strings — see [`Node::name`]; not parametric in Phase 1).
    pub fn get_param(&self, property: &str) -> Result<ParamValue, VectraError> {
        if let Some(style_prop) = property.strip_prefix("style.") {
            return match style_prop {
                "fill" => Ok(ParamValue::Color(self.style.fill.clone())),
                "stroke" => Ok(ParamValue::Color(self.style.stroke.clone())),
                "stroke_width" => Ok(ParamValue::Float(self.style.stroke_width.clone())),
                "opacity" => Ok(ParamValue::Float(self.style.opacity.clone())),
                other => {
                    let (index, rest) = parse_appearance_slot(other).ok_or_else(|| {
                        VectraError::UnknownProperty {
                            node_id: self.id,
                            node_kind: self.kind.tag().to_string(),
                            property: property.to_string(),
                        }
                    })?;
                    let layer = self.style.appearances.get(index).ok_or(
                        VectraError::AppearanceNotFound {
                            node_id: self.id,
                            index,
                        },
                    )?;
                    read_appearance_slot(layer, rest, property)
                }
            };
        }

        match &self.kind {
            NodeKind::Rectangle {
                x,
                y,
                width,
                height,
                corner_radius,
            } => match property {
                "x" => Ok(ParamValue::Float(x.clone())),
                "y" => Ok(ParamValue::Float(y.clone())),
                "width" => Ok(ParamValue::Float(width.clone())),
                "height" => Ok(ParamValue::Float(height.clone())),
                "corner_radius" | "radius" => Ok(ParamValue::Float(corner_radius.clone())),
                _ => Err(self.unknown_property(property)),
            },
            NodeKind::Circle { cx, cy, radius } => match property {
                "cx" | "x" | "center.x" => Ok(ParamValue::Float(cx.clone())),
                "cy" | "y" | "center.y" => Ok(ParamValue::Float(cy.clone())),
                "radius" | "r" => Ok(ParamValue::Float(radius.clone())),
                _ => Err(self.unknown_property(property)),
            },
            NodeKind::Arc {
                cx,
                cy,
                radius,
                start_angle,
                end_angle,
            } => match property {
                "cx" | "x" | "center.x" => Ok(ParamValue::Float(cx.clone())),
                "cy" | "y" | "center.y" => Ok(ParamValue::Float(cy.clone())),
                "radius" | "r" => Ok(ParamValue::Float(radius.clone())),
                "start_angle" | "start" => Ok(ParamValue::Float(start_angle.clone())),
                "end_angle" | "end" => Ok(ParamValue::Float(end_angle.clone())),
                _ => Err(self.unknown_property(property)),
            },
            // Paths have a *dynamic* slot space (Task 10.1): `start` plus every
            // segment endpoint, addressed with the evaluator's own spelling.
            // Unknown names inside a valid shape (`segments[0].control1` on a
            // `Line`) are refused by `path_param` returning `None`, so the error
            // is the ordinary `UnknownProperty` a caller already handles.
            NodeKind::Path { .. } => {
                // `segments[2].to.x` — a *scalar* view of one axis of a point
                // slot, which is how the constraint solver can address an anchor
                // (see `Node::split_component`).
                if let Some((base, axis)) = Self::split_component(property) {
                    if let Some(param) = self.path_param(base) {
                        return Ok(ParamValue::Float(Self::component_of(
                            param, property, axis,
                        )?));
                    }
                }
                match self.path_param(property) {
                    Some(param) => Ok(ParamValue::Point(param.clone())),
                    None => Err(self.unknown_property(property)),
                }
            }
            NodeKind::Group { .. } => Err(self.unknown_property(property)),
        }
    }

    /// Write a property by path, returning the previous value (for undo).
    /// Does writing `property` change this node's **geometry**?
    ///
    /// Every slot that is not `style.*` does: `width` shapes the rect, `cx` moves
    /// the circle, `start` moves the path. Style slots (fill, stroke, width,
    /// opacity) paint the shape and nothing else — a `⬡ source` node reads a
    /// node's *primitive*, never its paint.
    ///
    /// The distinction is what keeps the boundary-time cycle gate honest
    /// ([`crate::command`]'s `validate_procedural_cycle`). A style slot reading a
    /// port that the node's own geometry feeds is **not** a loop — the geometry
    /// flows source → node → port → paint, and the paint feeds nothing — while
    /// the same reference from `width` closes a loop that would oscillate once
    /// per settle round. Rejecting both would have made a legal document
    /// unwritable; rejecting neither would have let the loop through.
    pub fn property_feeds_geometry(property: &str) -> bool {
        !property.starts_with("style.")
    }

    pub fn set_param(
        &mut self,
        property: &str,
        value: ParamValue,
    ) -> Result<ParamValue, VectraError> {
        if let Some(style_prop) = property.strip_prefix("style.") {
            return match style_prop {
                "fill" => match value {
                    ParamValue::Color(v) => Ok(ParamValue::Color(std::mem::replace(
                        &mut self.style.fill,
                        v,
                    ))),
                    other => Err(self.type_mismatch(property, "color", other.kind())),
                },
                "stroke" => match value {
                    ParamValue::Color(v) => Ok(ParamValue::Color(std::mem::replace(
                        &mut self.style.stroke,
                        v,
                    ))),
                    other => Err(self.type_mismatch(property, "color", other.kind())),
                },
                "stroke_width" => match value {
                    ParamValue::Float(v) => Ok(ParamValue::Float(std::mem::replace(
                        &mut self.style.stroke_width,
                        v,
                    ))),
                    other => Err(self.type_mismatch(property, "float", other.kind())),
                },
                "opacity" => match value {
                    ParamValue::Float(v) => Ok(ParamValue::Float(std::mem::replace(
                        &mut self.style.opacity,
                        v,
                    ))),
                    other => Err(self.type_mismatch(property, "float", other.kind())),
                },
                other => {
                    let (index, rest) = parse_appearance_slot(other)
                        .ok_or_else(|| self.unknown_property(property))?;
                    let node_id = self.id;
                    let layer = self
                        .style
                        .appearances
                        .get_mut(index)
                        .ok_or(VectraError::AppearanceNotFound { node_id, index })?;
                    write_appearance_slot(layer, rest, property, value)
                }
            };
        }

        // Geometry properties. `expect_float` centralizes the type check.
        let expect_float = |property: &str, value: ParamValue| match value {
            ParamValue::Float(v) => Ok(v),
            other => Err(VectraError::PropertyTypeMismatch {
                property: property.to_string(),
                expected: "float",
                got: other.kind(),
            }),
        };

        match &mut self.kind {
            NodeKind::Rectangle {
                x,
                y,
                width,
                height,
                corner_radius,
            } => {
                let slot = match property {
                    "x" => x,
                    "y" => y,
                    "width" => width,
                    "height" => height,
                    "corner_radius" | "radius" => corner_radius,
                    _ => return Err(self.unknown_property(property)),
                };
                let v = expect_float(property, value)?;
                Ok(ParamValue::Float(std::mem::replace(slot, v)))
            }
            NodeKind::Circle { cx, cy, radius } => {
                let slot = match property {
                    "cx" | "x" | "center.x" => cx,
                    "cy" | "y" | "center.y" => cy,
                    "radius" | "r" => radius,
                    _ => return Err(self.unknown_property(property)),
                };
                let v = expect_float(property, value)?;
                Ok(ParamValue::Float(std::mem::replace(slot, v)))
            }
            NodeKind::Arc {
                cx,
                cy,
                radius,
                start_angle,
                end_angle,
            } => {
                let slot = match property {
                    "cx" | "x" | "center.x" => cx,
                    "cy" | "y" | "center.y" => cy,
                    "radius" | "r" => radius,
                    "start_angle" | "start" => start_angle,
                    "end_angle" | "end" => end_angle,
                    _ => return Err(self.unknown_property(property)),
                };
                let v = expect_float(property, value)?;
                Ok(ParamValue::Float(std::mem::replace(slot, v)))
            }
            NodeKind::Path { .. } => match value {
                // A component write is a write *through* the point slot: the
                // previous value comes back as the scalar it replaced, so the
                // generic `SetParameter` inverse restores it exactly, and the
                // block of the point the caller did not name is untouched.
                ParamValue::Float(scalar)
                    if Self::split_component(property)
                        .map(|(base, _)| self.path_param(base).is_some())
                        .unwrap_or(false) =>
                {
                    let Some((base, axis)) = Self::split_component(property) else {
                        return Err(self.unknown_property(property));
                    };
                    let Parameter::Literal(value) = scalar else {
                        return Err(VectraError::ComponentRequiresLiteral {
                            property: property.to_string(),
                        });
                    };
                    let slot = self.resolve_path_slot_mut(base)?;
                    let Parameter::Literal(point) = slot else {
                        return Err(VectraError::ComponentRequiresLiteral {
                            property: property.to_string(),
                        });
                    };
                    let previous = if axis == 0 { point.x } else { point.y };
                    if axis == 0 {
                        point.x = value;
                    } else {
                        point.y = value;
                    }
                    Ok(ParamValue::Float(Parameter::Literal(previous)))
                }
                ParamValue::Point(v) => {
                    // The slot may still be unknown; check the write against a
                    // borrow of the kind before committing to it (`self.kind`
                    // is already borrowed mutably here).
                    let known = self
                        .kind
                        .path_slot_names()
                        .iter()
                        .any(|slot| slot == property);
                    if !known {
                        return Err(self.unknown_property(property));
                    }
                    Ok(ParamValue::Point(self.set_path_param(property, v)?))
                }
                other => Err(self.type_mismatch(property, "point", other.kind())),
            },
            NodeKind::Group { .. } => Err(self.unknown_property(property)),
        }
    }

    /// Visit every parametric `f64` property of this node, in schema order.
    ///
    /// Property names match [`Node::get_param`] / [`Node::set_param`] exactly
    /// (`"width"`, `"style.opacity"`, …), so dependency edges and commands
    /// always address the same slot — the join the dependency graph (MES §8,
    /// Task 2.2) traverses.
    ///
    /// `Parameter<Point2>` / `Parameter<Color>` slots are intentionally *not*
    /// visited: Phase 1 validates them as literal-only (a variable/expression
    /// in a point or color slot is a typed [`VectraError::PropertyTypeMismatch`]
    /// or [`ResolveError`], never a silent dependency), so they can carry no
    /// graph edges yet.
    pub fn for_each_float_param<'a>(&'a self, mut visit: impl FnMut(&str, &'a Parameter<f64>)) {
        match &self.kind {
            NodeKind::Rectangle {
                x,
                y,
                width,
                height,
                corner_radius,
            } => {
                for (name, param) in [
                    ("x", x),
                    ("y", y),
                    ("width", width),
                    ("height", height),
                    ("corner_radius", corner_radius),
                ] {
                    visit(name, param);
                }
            }
            NodeKind::Circle { cx, cy, radius } => {
                for (name, param) in [("cx", cx), ("cy", cy), ("radius", radius)] {
                    visit(name, param);
                }
            }
            NodeKind::Arc {
                cx,
                cy,
                radius,
                start_angle,
                end_angle,
            } => {
                for (name, param) in [
                    ("cx", cx),
                    ("cy", cy),
                    ("radius", radius),
                    ("start_angle", start_angle),
                    ("end_angle", end_angle),
                ] {
                    visit(name, param);
                }
            }
            // Path geometry is `Parameter<Point2>`-only and groups are
            // structural (no transforms in Phase 1): no float slots.
            NodeKind::Path { .. } | NodeKind::Group { .. } => {}
        }
        visit("style.stroke_width", &self.style.stroke_width);
        visit("style.opacity", &self.style.opacity);
        // The appearance stack is part of the node's parametric surface: a
        // gradient's `end` may be variable-driven, a stroke's width may be an
        // expression, and the dependency graph must see those edges or a
        // variable edit would silently not reach the drawing.
        for (index, layer) in self.style.appearances.iter().enumerate() {
            visit(
                &format!("style.appearances[{index}].opacity"),
                &layer.opacity,
            );
            layer.paint.visit_floats(|suffix, param| {
                visit(&format!("style.appearances[{index}].paint.{suffix}"), param)
            });
            if let Some(width) = layer.kind.width() {
                visit(&format!("style.appearances[{index}].width"), width);
            }
        }
    }

    fn unknown_property(&self, property: &str) -> VectraError {
        VectraError::UnknownProperty {
            node_id: self.id,
            node_kind: self.kind.tag().to_string(),
            property: property.to_string(),
        }
    }

    fn type_mismatch(
        &self,
        property: &str,
        expected: &'static str,
        got: &'static str,
    ) -> VectraError {
        let _ = self;
        VectraError::PropertyTypeMismatch {
            property: property.to_string(),
            expected,
            got,
        }
    }
    /// **Every** addressable slot of a path, in draw order (Task 10.1).
    ///
    /// `["start", "segments[0].to", "segments[1].control1", …]` — the same
    /// spelling the evaluator puts in its diagnostics (`Property::segment_slot`),
    /// so a designer dragging a handle, an error message and a serialized
    /// document all name the same point the same way.
    ///
    /// This is what makes a drawn path *authorable*: every control point is a
    /// slot a command can address, which is what the Pen tool's handles, the
    /// Brush tool's fitting pass and the direct-selection drags all write
    /// through.
    pub fn path_slots(&self) -> Vec<String> {
        let NodeKind::Path { segments, .. } = &self.kind else {
            return Vec::new();
        };
        let mut slots = Vec::with_capacity(1 + segments.len() * 3);
        slots.push("start".to_string());
        for (index, segment) in segments.iter().enumerate() {
            match segment {
                PathSegment::Line { .. } => slots.push(format!("segments[{index}].to")),
                PathSegment::Quadratic { .. } => {
                    slots.push(format!("segments[{index}].control"));
                    slots.push(format!("segments[{index}].to"));
                }
                PathSegment::Cubic { .. } => {
                    slots.push(format!("segments[{index}].control1"));
                    slots.push(format!("segments[{index}].control2"));
                    slots.push(format!("segments[{index}].to"));
                }
                PathSegment::Close => {}
            }
        }
        slots
    }

    /// Split a **component path** — `"segments[2].to.x"` → `("segments[2].to", 0)`.
    ///
    /// Component addressing is what makes a point slot usable by every subsystem
    /// that speaks *scalars*: the constraint solver's rows are linear equalities
    /// over single floats, so "keep these two anchors level" (`y0 - y1 == 0`) is
    /// only expressible if `y` can be named on its own. Without this, a drawn path
    /// could not be constrained at all, and the Quick Shape snap (Task 10.1
    /// RULE 3) would have to do its own arithmetic — the opposite of "silently
    /// trigger the existing Constraint Solver".
    ///
    /// Returns the base slot and `0` for `.x`, `1` for `.y`.
    pub(crate) fn split_component(property: &str) -> Option<(&str, usize)> {
        for (suffix, axis) in [(".x", 0usize), (".y", 1usize)] {
            if let Some(base) = property.strip_suffix(suffix) {
                if !base.is_empty() {
                    return Some((base, axis));
                }
            }
        }
        None
    }

    /// One axis of a point parameter as a *scalar* parameter.
    ///
    /// Only a literal point can be split: a point bound to a variable, an
    /// expression or a procedural port is one value that resolves as a whole, and
    /// there is no honest scalar to hand back for half of it. The error says so
    /// ([`VectraError::ComponentRequiresLiteral`]) instead of inventing a value —
    /// callers that must write a component bind the whole point instead.
    pub(crate) fn component_of(
        param: &Parameter<Point2>,
        property: &str,
        axis: usize,
    ) -> Result<Parameter<f64>, VectraError> {
        match param {
            Parameter::Literal(point) => Ok(Parameter::Literal(if axis == 0 {
                point.x
            } else {
                point.y
            })),
            _ => Err(VectraError::ComponentRequiresLiteral {
                property: property.to_string(),
            }),
        }
    }

    /// Read one path point slot: `start`, or `segments[i].{to,control,control1,control2}`.
    ///
    /// Returns `None` for a property this kind does not own (the caller decides
    /// whether that is an `UnknownProperty` error or a probe); a slot that exists
    /// but is the wrong *shape* for the segment is also `None` — `segments[0].control1`
    /// on a `Line` is a name that does not exist, not a mismatched type.
    pub fn path_param(&self, property: &str) -> Option<&Parameter<Point2>> {
        let NodeKind::Path { start, segments } = &self.kind else {
            return None;
        };
        if property == "start" {
            return Some(start);
        }
        let rest = property.strip_prefix("segments[")?;
        let (index, field) = rest.split_once("].")?;
        let index: usize = index.parse().ok()?;
        let segment = segments.get(index)?;
        match (segment, field) {
            (PathSegment::Line { to }, "to") => Some(to),
            (PathSegment::Quadratic { control, .. }, "control") => Some(control),
            (PathSegment::Quadratic { to, .. }, "to") => Some(to),
            (PathSegment::Cubic { control1, .. }, "control1") => Some(control1),
            (PathSegment::Cubic { control2, .. }, "control2") => Some(control2),
            (PathSegment::Cubic { to, .. }, "to") => Some(to),
            _ => None,
        }
    }

    /// Write one path point slot, returning the value it held.
    ///
    /// The path equivalent of `set_param`'s scalar arms — same contract, same
    /// "previous value, for undo" discipline — because a bezier handle is a
    /// parameter like any other (Task 10.1 RULE 1: the drawing tools manipulate
    /// `Parameter<Point2>`, they do not invent a second geometry model).
    pub fn set_path_param(
        &mut self,
        property: &str,
        value: Parameter<Point2>,
    ) -> Result<Parameter<Point2>, VectraError> {
        let resolved = self.resolve_path_slot_mut(property)?;
        Ok(std::mem::replace(resolved, value))
    }

    /// Resolve a slot name to a mutable reference, or explain why not.
    fn resolve_path_slot_mut(
        &mut self,
        property: &str,
    ) -> Result<&mut Parameter<Point2>, VectraError> {
        let id = self.id;
        let kind_tag = self.kind.tag().to_string();
        let NodeKind::Path { start, segments } = &mut self.kind else {
            return Err(VectraError::UnknownProperty {
                node_id: id,
                node_kind: kind_tag,
                property: property.to_string(),
            });
        };
        if property == "start" {
            return Ok(start);
        }
        let unknown = || VectraError::UnknownProperty {
            node_id: id,
            node_kind: kind_tag.clone(),
            property: property.to_string(),
        };
        let rest = property.strip_prefix("segments[").ok_or_else(unknown)?;
        let (index, field) = rest.split_once("].").ok_or_else(unknown)?;
        let index: usize = index.parse().map_err(|_| unknown())?;
        let segment = segments.get_mut(index).ok_or_else(unknown)?;
        match (segment, field) {
            (PathSegment::Line { to }, "to") => Ok(to),
            (PathSegment::Quadratic { control, .. }, "control") => Ok(control),
            (PathSegment::Quadratic { to, .. }, "to") => Ok(to),
            (PathSegment::Cubic { control1, .. }, "control1") => Ok(control1),
            (PathSegment::Cubic { control2, .. }, "control2") => Ok(control2),
            (PathSegment::Cubic { to, .. }, "to") => Ok(to),
            _ => Err(unknown()),
        }
    }
}

/// Source record for an expression (MES §7).
///
/// The *compiled* artifact lives in `vectra-expression` and is keyed by the
/// same [`ExpressionId`]; core persists the source so documents survive
/// without the compiler wired in.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExpressionRecord {
    pub source: String,
}

impl ExpressionRecord {
    pub fn new(source: impl Into<String>) -> Self {
        Self {
            source: source.into(),
        }
    }
}

/// Backwards-compatible alias: the MES §10 sketch called the placeholder
/// record `OperationRecord`; Task 4.0 replaces it with the real
/// [`crate::operation::OperationNode`] registry.
pub type OperationRecord = crate::operation::OperationNode;

/// One keyframe: a value pinned to a time (seconds).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Keyframe {
    pub time: f64,
    pub value: f64,
}

impl Keyframe {
    pub fn new(time: f64, value: f64) -> Self {
        Self { time, value }
    }
}

/// A keyframe track (MES §12), one of the sources a
/// [`crate::param::MotionBinding::KeyframeTrack`] binding samples.
///
/// A track is a *named set of channels*, each an ordered list of keyframes, so a
/// single track can drive several slots (`x` and `y` of one node, say) and one
/// edit moves all of them. The binding's `property` field selects the channel.
///
/// Tracks are document state: editing one is an undoable command, and the
/// renderer's dirty propagation follows the `property → track` edges in the
/// dependency graph (`set_time` moves the clock, a track edit moves the track).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MotionTrack {
    pub id: TrackId,
    pub name: String,
    /// Channel name → keyframes. Interpolated linearly (Task 6.0); every list is
    /// kept sorted by time and validated on insertion.
    pub channels: BTreeMap<String, Vec<Keyframe>>,
}

impl MotionTrack {
    /// A track with one channel, built from `(time, value)` pairs.
    pub fn ramp(
        id: impl Into<TrackId>,
        name: impl Into<String>,
        channel: impl Into<String>,
        samples: impl IntoIterator<Item = (f64, f64)>,
    ) -> Self {
        let mut channels = BTreeMap::new();
        channels.insert(
            channel.into(),
            samples
                .into_iter()
                .map(|(t, v)| Keyframe::new(t, v))
                .collect(),
        );
        Self {
            id: id.into(),
            name: name.into(),
            channels,
        }
    }

    /// Validate: finite numbers, at least one sample per channel, times strictly
    /// increasing (which also forbids duplicates), non-empty channel names.
    ///
    /// A track is a *sampling contract*, and the evaluator promises a value for
    /// every `t`. Rejecting a malformed track at the boundary is what lets
    /// `evaluate` be total and pure rather than defensive.
    pub fn validate(&self) -> Result<(), VectraError> {
        if self.id.trim().is_empty() {
            return Err(VectraError::command("track id must not be empty"));
        }
        if self.channels.is_empty() {
            return Err(VectraError::command(format!(
                "track {} has no channels",
                self.id
            )));
        }
        for (channel, samples) in &self.channels {
            if channel.trim().is_empty() {
                return Err(VectraError::command(format!(
                    "track {} has an unnamed channel",
                    self.id
                )));
            }
            if samples.is_empty() {
                return Err(VectraError::command(format!(
                    "track {} channel {channel} has no keyframes",
                    self.id
                )));
            }
            for (i, sample) in samples.iter().enumerate() {
                if !sample.time.is_finite() || !sample.value.is_finite() {
                    return Err(VectraError::command(format!(
                        "track {} channel {channel} keyframe {i} is not finite",
                        self.id
                    )));
                }
                if let Some(previous) = i.checked_sub(1).and_then(|j| samples.get(j)) {
                    if sample.time <= previous.time {
                        return Err(VectraError::command(format!(
                            "track {} channel {channel} keyframe times must strictly increase \
                             ({} after {})",
                            self.id, sample.time, previous.time
                        )));
                    }
                }
            }
        }
        Ok(())
    }

    /// Last keyframe time across all channels (the useful horizon for
    /// "is this track still animating?").
    pub fn end_time(&self) -> f64 {
        self.channels
            .values()
            .filter_map(|samples| samples.last().map(|k| k.time))
            .fold(f64::NEG_INFINITY, f64::max)
    }
}

/// Track registry, keyed by [`TrackId`], insertion-ordered for stable
/// serialization and a stable inspector.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MotionTrackRegistry {
    tracks: BTreeMap<TrackId, MotionTrack>,
    order: Vec<TrackId>,
}

impl MotionTrackRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert or replace a validated track. Returns the track it replaced.
    pub fn insert(&mut self, track: MotionTrack) -> Result<Option<MotionTrack>, VectraError> {
        track.validate()?;
        if !self.order.contains(&track.id) {
            self.order.push(track.id.clone());
        }
        Ok(self.tracks.insert(track.id.clone(), track))
    }

    pub fn remove(&mut self, id: &str) -> Option<MotionTrack> {
        self.order.retain(|other| other != id);
        self.tracks.remove(id)
    }

    pub fn get(&self, id: &str) -> Option<&MotionTrack> {
        self.tracks.get(id)
    }

    pub fn contains(&self, id: &str) -> bool {
        self.tracks.contains_key(id)
    }

    /// Track ids in insertion order.
    pub fn ids(&self) -> &[TrackId] {
        &self.order
    }

    pub fn len(&self) -> usize {
        self.tracks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tracks.is_empty()
    }
}

/// The document: interconnected registries, not a flat property bag (MES §4).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Document {
    pub version: u32,
    pub variables: HashMap<VariableId, f64>,
    pub nodes: HashMap<NodeId, Node>,
    /// Draw order (back → front). Indexes into [`Document::nodes`].
    pub order: Vec<NodeId>,
    pub expressions: HashMap<ExpressionId, ExpressionRecord>,
    /// Hard mathematical rules enforced by `vectra-constraints` (MES §9).
    /// Insertion-ordered so serialization, the inspector and the solver agree.
    pub constraints: ConstraintRegistry,
    /// Non-destructive operations (MES §10, Task 4.0): virtual shapes composed
    /// from the nodes above. Never a mutation of their inputs.
    pub operations: OperationRegistry,
    /// Motion sources (MES §12, Task 6.0): keyframe tracks that
    /// `Parameter::Animated(KeyframeTrack { .. })` bindings sample. Springs and
    /// state branches carry their own data in the binding, so they need no
    /// registry; a track is shared state, so it has one.
    #[serde(default)]
    pub motion: MotionTrackRegistry,
    /// The procedural graph (MES §11, Task 7.0): typed nodes whose ports feed
    /// each other, `Parameter::Procedural` slots, and — for geometry-producing
    /// kinds — the scene itself. Authored data only; the topology is derived
    /// from the wiring on demand, so undo needs no special case.
    #[serde(default)]
    pub procedural: crate::procedural::ProceduralRegistry,
    /// The layer tree (Task 10.2 RULE 1): membership, names, eye and padlock.
    ///
    /// Empty for a document authored before layers existed, and *kept* empty
    /// until a designer asks for one — see [`Document::ensure_layer`]. Nodes no
    /// layer lists are drawn exactly as they always were, which is what makes
    /// this additive rather than a migration.
    #[serde(default)]
    pub layers: LayerRegistry,
    /// The document's frames (Task 10.2 RULE 2). Empty means "one implicit
    /// canvas", which is what every earlier document is.
    #[serde(default)]
    pub artboards: ArtboardRegistry,
    /// The layer the designer is working in. A pointer, not structure: it is
    /// what `CreateNode` consults so artwork lands where the panel says it will.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_layer: Option<LayerId>,
}

impl Document {
    pub fn new() -> Self {
        Self {
            version: DOCUMENT_VERSION,
            variables: HashMap::new(),
            nodes: HashMap::new(),
            order: Vec::new(),
            expressions: HashMap::new(),
            constraints: ConstraintRegistry::new(),
            operations: OperationRegistry::new(),
            motion: MotionTrackRegistry::new(),
            procedural: crate::procedural::ProceduralRegistry::new(),
            layers: LayerRegistry::new(),
            artboards: ArtboardRegistry::new(),
            active_layer: None,
        }
    }

    /// Every live geometry id: authored nodes first (draw order), then the
    /// **enabled** virtual operation results, then the **enabled**
    /// geometry-producing procedural results (each in its own draw order). This
    /// is the id space the evaluated scene is keyed in, and it is why an
    /// `OperationId` or a procedural node id can be ordered and styled like a
    /// primitive (RULE 3).
    ///
    /// **Task 7.0 RULE 4 (the silent eviction trap).** The operations pass ends
    /// with `EvaluatedScene::apply_operations`, which prunes every cached id
    /// failing [`Document::is_geometry_id`]. A drawn procedural result that is
    /// not listed here would be *deleted from the cache by the next unrelated
    /// operation pass* — silently, long after the frame that drew it. Widening
    /// both functions in the same change that introduces the first drawn
    /// procedural node is what makes that impossible; a law test drives a
    /// procedural result through an unrelated operation pass to prove it.
    pub fn geometry_ids(&self) -> Vec<NodeId> {
        let mut ids: Vec<NodeId> = self.order.clone();
        ids.extend(
            self.operations
                .in_order()
                .filter(|op| op.enabled)
                .map(|op| op.id),
        );
        ids.extend(self.procedural.geometry_ids());
        ids
    }

    /// True ⟺ `id` contributes geometry right now: an authored node, an
    /// **enabled** operation result, or an **enabled** geometry-producing
    /// procedural node.
    ///
    /// A parked operation or procedural node keeps its registry entry, its
    /// inputs and its id — but it is not live geometry, so the evaluated scene
    /// must not hold a node for it. That single predicate is what keeps the
    /// pruning passes and the scene cache's live-id invariant in agreement.
    pub fn is_geometry_id(&self, id: NodeId) -> bool {
        self.nodes.contains_key(&id)
            || self.operations.get(id).is_some_and(|op| op.enabled)
            || self.procedural.is_geometry_id(id)
    }

    /// Every node whose parameters read one of `outputs` — the readers the
    /// engine must re-evaluate when the procedural pass publishes new values
    /// (Task 7.0: the value path is a real dependency, so a slot that reads a
    /// port cannot be left holding last pass's number).
    ///
    /// Covers the float slots plus the point and colour slots
    /// ([`Node::for_each_float_param`] visits only floats; `Path.start` and the
    /// two style colours are checked separately).
    pub fn procedural_readers(&self, outputs: &[NodeOutputId]) -> Vec<NodeId> {
        if outputs.is_empty() {
            return Vec::new();
        }
        let reads = |param: &Parameter<f64>| match param {
            Parameter::Procedural(output) => outputs.contains(output),
            _ => false,
        };
        let reads_point = |param: &Parameter<Point2>| match param {
            Parameter::Procedural(output) => outputs.contains(output),
            _ => false,
        };
        let reads_color = |param: &Parameter<Color>| match param {
            Parameter::Procedural(output) => outputs.contains(output),
            _ => false,
        };
        let mut ids: Vec<NodeId> = self
            .order
            .iter()
            .copied()
            .filter(|id| {
                let Some(node) = self.nodes.get(id) else {
                    return false;
                };
                let mut hit = false;
                node.for_each_float_param(|_, param| {
                    if reads(param) {
                        hit = true;
                    }
                });
                if hit {
                    return true;
                }
                if reads_color(&node.style.fill)
                    || reads_color(&node.style.stroke)
                    || node
                        .style
                        .appearances
                        .iter()
                        .any(|layer| matches!(layer.solid_color_paint(), Some(paint) if reads_color(paint)))
                {
                    return true;
                }
                match &node.kind {
                    NodeKind::Path { start, segments } => {
                        if reads_point(start) {
                            return true;
                        }
                        segments.iter().any(|segment| match segment {
                            PathSegment::Line { to } => reads_point(to),
                            PathSegment::Quadratic { control, to } => {
                                reads_point(control) || reads_point(to)
                            }
                            PathSegment::Cubic {
                                control1,
                                control2,
                                to,
                            } => reads_point(control1) || reads_point(control2) || reads_point(to),
                            PathSegment::Close => false,
                        })
                    }
                    _ => false,
                }
            })
            .collect();
        ids.sort();
        ids.dedup();
        ids
    }

    /// Every authored node or operation whose **geometry** a procedural node's
    /// output can depend on — the transitive half of RULE 3 that is not a port.
    ///
    /// The graph's cycle gate draws edges for *ports* (a wire, a slot that reads
    /// a port), because those are the references `ProceduralNode` stores. It
    /// cannot see the other path a dependency travels: a `source` node reads a
    /// whole shape (`kind.source_node()`), and the node it reads may in turn
    /// read a port. That is a loop with no port-shaped edge in it:
    ///
    /// ```text
    ///   rect.width ← ⬡ grid • span        (a slot reads a port)
    ///   ⬡ source ← rect                   (the source reads rect's shape)
    /// ```
    ///
    /// Add `grid`'s output back into `rect`'s own geometry and the chain closes
    /// silently — no edge to reject, and the settle loop that re-reads readers
    /// would chase it forever. So the *boundary* asks this question instead: is
    /// the node that wants to read a port already upstream of it? See
    /// [`crate::command`]'s `validate_procedural_cycle`.
    ///
    /// Returns the ids of authored nodes and operations only — the port's own
    /// node is reached through [`Self::procedural_geometry_into`], not listed.
    pub fn procedural_geometry_closure(&self, start: NodeId) -> BTreeSet<NodeId> {
        let mut out: BTreeSet<NodeId> = BTreeSet::new();
        let mut walked: BTreeSet<NodeId> = BTreeSet::new();
        self.procedural_geometry_into(start, &mut out, &mut walked);
        out
    }

    /// Walk one procedural node's geometry inputs into `out` (`walked` guards
    /// against a registry that is mid-edit).
    fn procedural_geometry_into(
        &self,
        id: NodeId,
        out: &mut BTreeSet<NodeId>,
        walked: &mut BTreeSet<NodeId>,
    ) {
        if !walked.insert(id) {
            return;
        }
        let Some(node) = self.procedural.get(id) else {
            return;
        };
        for wire in node.wires.values() {
            self.procedural_geometry_into(wire.node, out, walked);
        }
        if let Some(source) = node.kind.source_node() {
            self.geometry_node_into(source, out, walked);
        }
    }

    /// Walk one scene node or operation's geometry inputs into `out`.
    fn geometry_node_into(
        &self,
        id: NodeId,
        out: &mut BTreeSet<NodeId>,
        walked: &mut BTreeSet<NodeId>,
    ) {
        if !out.insert(id) {
            return;
        }
        if let Some(operation) = self.operations.get(id) {
            // A boolean reads its inputs' *shapes*, so the closure follows the
            // registry rather than stopping at the operation.
            for input in &operation.inputs {
                self.geometry_node_into(*input, out, walked);
            }
            return;
        }
        let Some(node) = self.nodes.get(&id) else {
            return;
        };
        let mut refs: Vec<NodeOutputId> = Vec::new();
        collect_node_procedural_refs(node, &mut refs);
        for reference in refs {
            self.procedural_geometry_into(reference.node, out, walked);
        }
    }

    pub fn get_node(&self, id: NodeId) -> Result<&Node, VectraError> {
        self.nodes.get(&id).ok_or(VectraError::NodeNotFound(id))
    }

    pub fn get_node_mut(&mut self, id: NodeId) -> Result<&mut Node, VectraError> {
        self.nodes.get_mut(&id).ok_or(VectraError::NodeNotFound(id))
    }

    /// Insert a node, appending to draw order (or splicing at `index`).
    pub fn insert_node(&mut self, node: Node, index: Option<usize>) -> Result<(), VectraError> {
        if self.nodes.contains_key(&node.id) {
            return Err(VectraError::NodeAlreadyExists(node.id));
        }
        let id = node.id;
        self.nodes.insert(id, node);
        match index {
            Some(i) if i <= self.order.len() => self.order.insert(i, id),
            _ => self.order.push(id),
        }
        // **RULE 1**: every node belongs to a layer. New artwork lands on the
        // active one (the layer the panel has highlighted), which is also the
        // only way `order == flatten_layers()` can stay true once layers exist —
        // an id that belongs to no layer is unarranged by definition, and the
        // flatten puts those *after* the arranged blocks.
        //
        // A document with no layers is untouched: the node is appended to the
        // flat order exactly as it was before this task, which is what keeps
        // every legacy document (and every non-UI test) byte-identical.
        if let Some(layer) = self.active_layer() {
            self.assign_to_layer(id, layer);
        }
        Ok(())
    }

    /// Remove a node, returning it plus its former draw-order index.
    pub fn remove_node(&mut self, id: NodeId) -> Result<(Node, usize), VectraError> {
        let node = self
            .nodes
            .remove(&id)
            .ok_or(VectraError::NodeNotFound(id))?;
        let index = self.order.iter().position(|o| *o == id).unwrap_or(0);
        self.order.retain(|o| *o != id);
        Ok((node, index))
    }

    pub fn order_index(&self, id: NodeId) -> Option<usize> {
        self.order.iter().position(|o| *o == id)
    }

    /// A node and every node underneath it, in draw order.
    ///
    /// This is what makes "moving a group moves all children" true at the level
    /// of a *flat* draw order: a group is one entry in
    /// [`crate::layers::LayerRecord::children`], so the layer path needs no
    /// traversal at all — but a document with no layers keeps its nodes in
    /// `order`, where a group's children sit beside it and must travel with it.
    pub fn node_block(&self, id: NodeId) -> Vec<NodeId> {
        let mut block: Vec<NodeId> = Vec::new();
        let mut stack = vec![id];
        while let Some(next) = stack.pop() {
            if !self.nodes.contains_key(&next) || block.contains(&next) {
                continue;
            }
            block.push(next);
            if let NodeKind::Group { children } = &self.nodes[&next].kind {
                // Reversed, so the stack yields children in author order.
                stack.extend(children.iter().rev().copied());
            }
        }
        // The block reads in document order, which is what a splice needs.
        let mut ordered: Vec<NodeId> = self
            .order
            .iter()
            .copied()
            .filter(|candidate| block.contains(candidate))
            .collect();
        for id in &block {
            if !ordered.contains(id) {
                ordered.push(*id);
            }
        }
        ordered
    }

    /// **Which group lists this node**, or `None` when a layer lists it directly.
    ///
    /// Parentage lives in exactly one place — `NodeKind::Group { children }` — and
    /// this is the only reader of it. A node listed by two groups (which no
    /// command can produce, but a hand-edited document can) reports the first in
    /// document order, so the answer is deterministic rather than "whichever the
    /// scan reached first".
    pub fn parent_of(&self, id: NodeId) -> Option<NodeId> {
        self.order
            .iter()
            .chain(self.nodes.keys())
            .find_map(
                |candidate| match self.nodes.get(candidate).map(|node| &node.kind) {
                    Some(NodeKind::Group { children }) if children.contains(&id) => {
                        Some(*candidate)
                    }
                    _ => None,
                },
            )
    }

    /// **Move a node into a group, or out of every group, at a z-position.**
    /// (Task 10.4 RULE 1.)
    ///
    /// `parent = Some(id)` puts the node *inside* that group; `None` puts it at
    /// the top level of the layer that lists it (which is what "drag it out of the
    /// group" means). `index` is a position among the destination **container's**
    /// siblings, in the layer's back → front list, counting after the moved block
    /// is taken out. One convention, one placement path (Task 10.5): a drop in
    /// the panel and a command typed by hand mean the same thing.
    ///
    /// Returns the node's previous `(parent, index)`, which is the inverse the
    /// command stack needs; `None` when nothing moved because nothing could.
    ///
    /// Refused, typed, **before any mutation** — a half-applied move would be a
    /// document with a node in two places or a group inside itself:
    ///
    /// * `NodeNotFound` — the node or the parent is not in the document;
    /// * `NotAGroup` — the destination is not a container;
    /// * `GroupCycle` — the destination is the node itself or one of its
    ///   descendants, which would make the group its own ancestor.
    pub fn set_parent(
        &mut self,
        id: NodeId,
        parent: Option<NodeId>,
        index: usize,
    ) -> Result<Option<(Option<NodeId>, usize)>, crate::error::VectraError> {
        use crate::error::VectraError;
        if !self.nodes.contains_key(&id) {
            return Err(VectraError::NodeNotFound(id));
        }
        if let Some(parent) = parent {
            if !self.nodes.contains_key(&parent) {
                return Err(VectraError::NodeNotFound(parent));
            }
            if !matches!(self.nodes[&parent].kind, NodeKind::Group { .. }) {
                return Err(VectraError::NotAGroup(parent));
            }
            // The cycle check is on the *subtree*, not on the pair: moving a group
            // into its own grandchild is the same cycle one level further down.
            if parent == id || self.node_block(id).contains(&parent) {
                return Err(VectraError::GroupCycle {
                    node_id: id,
                    ancestor: parent,
                });
            }
        }

        let previous_parent = self.parent_of(id);
        let previous_index = self.sibling_index(id, previous_parent);
        if previous_parent == parent && previous_index == index {
            return Ok(None); // a drop that changes nothing is not a command
        }

        // 1. Parentage: one list gains the node, one (or none) loses it.
        if let Some(old) = previous_parent {
            if let Some(node) = self.nodes.get_mut(&old) {
                if let NodeKind::Group { children } = &mut node.kind {
                    children.retain(|child| *child != id);
                }
            }
        }
        if let Some(new) = parent {
            if let Some(node) = self.nodes.get_mut(&new) {
                if let NodeKind::Group { children } = &mut node.kind {
                    let at = index.min(children.len());
                    children.insert(at, id);
                }
            }
        }

        // 2. Membership: a group and its contents live in one layer, so the
        //    subtree follows the destination's layer — that is what keeps
        //    `flatten_layers` (which walks layers, not groups) drawing the tree
        //    the panel shows.
        let destination_layer = parent
            .and_then(|parent| self.layers.layer_of(parent).map(|(layer, _)| layer.id))
            .or_else(|| self.layers.layer_of(id).map(|(layer, _)| layer.id));
        if let Some(layer_id) = destination_layer {
            let block = self.node_block(id);
            // Phase 1 — the destination layer takes the block in, and the block
            // leaves the list while its landing spot is computed. One run, so a
            // member can never be spliced into the middle of another container.
            if let Some(layer) = self.layers.get_mut(&layer_id) {
                for member in &block {
                    if !layer.children.contains(member) {
                        layer.children.push(*member);
                    }
                }
                layer.children.retain(|child| !block.contains(child));
            }
            // Phase 2 — where it lands, read from the list as it now stands.
            let at = self.container_index(layer_id, parent, index, &block);
            if let Some(layer) = self.layers.get_mut(&layer_id) {
                for (offset, member) in block.iter().enumerate() {
                    let position = (at + offset).min(layer.children.len());
                    layer.children.insert(position, *member);
                }
            }
            // Phase 3 — a subtree that left its old layer must not linger there.
            let originals: Vec<LayerId> = self
                .layers
                .iter()
                .map(|layer| layer.id)
                .filter(|layer| *layer != layer_id)
                .collect();
            for other in originals {
                let leaves = self
                    .layers
                    .get(&other)
                    .map(|layer| layer.children.iter().any(|child| block.contains(child)))
                    .unwrap_or(false);
                if !leaves {
                    continue;
                }
                if let Some(layer) = self.layers.get_mut(&other) {
                    layer.children.retain(|child| !block.contains(child));
                }
            }
            self.resync_order_from_layers();
        }
        Ok(Some((previous_parent, previous_index)))
    }

    /// Where the moved block lands in a layer's list: the position, in the
    /// **remaining** list, of the `index`-th sibling of `parent`.
    ///
    /// The destination container's own rows are the anchor, because they are what
    /// the designer pointed at; if the container has fewer than `index` siblings
    /// the block goes after the last of them (never into a foreign container).
    fn container_index(
        &self,
        layer: LayerId,
        parent: Option<NodeId>,
        index: usize,
        moving: &[NodeId],
    ) -> usize {
        let Some(record) = self.layers.get(&layer) else {
            return 0;
        };
        let siblings: Vec<NodeId> = record
            .children
            .iter()
            .copied()
            .filter(|child| !moving.contains(child))
            .filter(|child| self.parent_of(*child) == parent)
            .collect();
        match siblings.get(index) {
            // **At the start of the anchor's run — never inside it.** A row is
            // the *end* of its own run: a group lists its children, and children
            // are drawn behind their container (a group draws nothing), so the
            // list reads `[children…, group]`. Splicing at the anchor row's own
            // position therefore buries the moved block inside the anchor's run
            // whenever the anchor is a group — a document where the panel (which
            // walks parent links) and the canvas (which reads this list) disagree
            // about what is on top. On a valid walk the run's first row is the
            // anchor itself for a leaf, and the anchor's first child for a group;
            // starting there puts the block between two runs, which is what "be
            // the `index`-th sibling" has to mean in a flat list.
            Some(anchor) => self
                .run_start(&record.children, *anchor)
                .unwrap_or_else(|| {
                    record
                        .children
                        .iter()
                        .position(|child| child == anchor)
                        .unwrap_or(0)
                }),
            None => match siblings.last() {
                // Past the end of the container: after its last sibling's run, so
                // a group's new member joins the group rather than escaping it.
                Some(last) => self
                    .run_end(&record.children, *last)
                    .unwrap_or(record.children.len()),
                // An empty container: at the container's own row.
                None => parent
                    .and_then(|parent| record.children.iter().position(|child| *child == parent))
                    .unwrap_or(record.children.len()),
            },
        }
    }

    /// The first row of `node`'s run in a layer's list.
    ///
    /// A node's run is its block — the node and everything it holds, in document
    /// order, which puts a group's children *before* the group. `min` over the
    /// members' positions is the run's start, and it stays honest even for a
    /// document a hand-written command scattered.
    fn run_start(&self, listed: &[NodeId], node: NodeId) -> Option<usize> {
        self.node_block(node)
            .into_iter()
            .filter_map(|member| listed.iter().position(|row| *row == member))
            .min()
    }

    /// One past the last row of `node`'s run in a layer's list.
    fn run_end(&self, listed: &[NodeId], node: NodeId) -> Option<usize> {
        self.node_block(node)
            .into_iter()
            .filter_map(|member| listed.iter().position(|row| *row == member))
            .max()
            .map(|position| position + 1)
    }

    /// A node's position among its container's siblings in the layer list — the
    /// pair [`Document::set_parent`] reports so its inverse can restore the spot.
    pub fn sibling_index(&self, id: NodeId, parent: Option<NodeId>) -> usize {
        let Some(layer) = self.layers.layer_of(id).map(|(layer, _)| layer.id) else {
            // No layer: the container is the flat order, whose siblings are the
            // other unparented nodes.
            return self
                .order
                .iter()
                .filter(|other| self.parent_of(**other) == parent)
                .position(|other| *other == id)
                .unwrap_or(0);
        };
        let Some(record) = self.layers.get(&layer) else {
            return 0;
        };
        record
            .children
            .iter()
            .filter(|child| self.parent_of(**child) == parent)
            .position(|child| *child == id)
            .unwrap_or(0)
    }

    // ── Layers (Task 10.2 RULE 1) ───────────────────────────────────────────

    /// **The invariant that keeps two structures honest.**
    ///
    /// `order` must read as: every layer's children, layer by layer from back to
    /// front, each layer's own order preserved — followed by whatever no layer
    /// lists (a legacy document's nodes, or a node a layer was deleted from).
    ///
    /// A layer command that changes membership or layer order therefore has
    /// exactly one job after mutating the registry: call
    /// [`Document::resync_order_from_layers`]. It is a law in the test suite
    /// (`layers_and_order_agree`) precisely because a violation is invisible
    /// until something draws in the wrong place.
    pub fn order_matches_layers(&self) -> bool {
        self.flatten_layers() == self.order
    }

    /// What `order` must be, given the layer tree.
    pub fn flatten_layers(&self) -> Vec<NodeId> {
        let mut flat: Vec<NodeId> = Vec::with_capacity(self.order.len());
        let mut visited: std::collections::HashSet<LayerId> = std::collections::HashSet::new();
        // Artboards first, back → front: each board contributes its own stack,
        // so raising an artboard raises everything on it.
        for board in self.artboards.iter() {
            for layer_id in &board.layers {
                if !visited.insert(*layer_id) {
                    continue;
                }
                if let Some(layer) = self.layers.get(layer_id) {
                    push_layer_children(&mut flat, layer, &self.nodes);
                }
            }
        }
        for layer in self.layers.iter() {
            if visited.insert(layer.id) {
                push_layer_children(&mut flat, layer, &self.nodes);
            }
        }
        // Unassigned ids keep their relative order and stay *after* the layer
        // blocks: a node with no layer has not been arranged, so the arrangement
        // the designer did make must not be disturbed by it.
        for id in &self.order {
            if !flat.contains(id) {
                flat.push(*id);
            }
        }
        flat
    }

    /// Rewrite `order` so it reads as the layer tree says (see
    /// [`Document::order_matches_layers`]). Idempotent.
    pub fn resync_order_from_layers(&mut self) {
        self.order = self.flatten_layers();
    }

    /// **Reorder a layer everywhere it is listed**, returning where it was.
    ///
    /// A layer's position is expressed twice — in [`LayerRegistry::layers`] (the
    /// list the panel shows) and in its artboard's `layers` stack (the order the
    /// draw order is flattened from, back → front). They are the same arrangement
    /// seen from two sides, so a move has to be written to both or the panel and
    /// the canvas disagree about which layer is on top. That disagreement is
    /// invisible in a screenshot and obvious in a drawing, which is exactly the
    /// kind of bug this method exists to make impossible.
    pub fn reorder_layer(&mut self, id: &LayerId, index: usize) -> Option<usize> {
        let previous = self.layers.position(id)?;
        self.layers.reorder(id, index);
        for board in &mut self.artboards.boards {
            if let Some(position) = board.layers.iter().position(|layer| layer == id) {
                let moved = board.layers.remove(position);
                let at = index.min(board.layers.len());
                board.layers.insert(at, moved);
                break;
            }
        }
        self.resync_order_from_layers();
        Some(previous)
    }

    /// Ensure at least one layer exists, returning its id.
    ///
    /// Called by the *boundary*, not by the engine: an interactive session gets
    /// a layer to draw into ("Layer 1"), while a document built purely from
    /// commands (every law test in this workspace) stays layer-free and behaves
    /// exactly as it did before layers existed.
    pub fn ensure_layer(&mut self) -> LayerId {
        if let Some(first) = self.layers.layers.first() {
            return first.id;
        }
        let id = crate::ids::new_layer_id();
        self.layers.insert(LayerRecord::new(id, "Layer 1"), None);
        if let Some(active) = self.artboards.active_id() {
            if let Some(board) = self.artboards.get_mut(&active) {
                board.layers.push(id);
            }
        }
        self.active_layer = Some(id);
        id
    }

    /// Add a layer to an artboard's stack (or leave it unlisted when the
    /// document has no artboards yet, in which case it still draws).
    pub fn attach_layer_to_artboard(&mut self, layer: LayerId, artboard: Option<ArtboardId>) {
        let target = artboard.or_else(|| self.artboards.active_id());
        if let Some(target) = target {
            for board in &mut self.artboards.boards {
                board.layers.retain(|id| *id != layer);
            }
            if let Some(board) = self.artboards.get_mut(&target) {
                board.layers.push(layer);
            }
        }
    }

    /// **Open a workspace** (Task 10.2 RULES 1–2): the artboard and the layer a
    /// new document starts on, with the fixed ids from [`crate::ids`].
    ///
    /// Called by the UI's engine constructor, not by [`Document::default`] — a
    /// document type that quietly grew a layer would change what every native
    /// caller (an importer, a library user, a test) sees, and this is a
    /// *workspace* opinion, not a document invariant. The guards make it
    /// idempotent and harmless on a document that already has paper.
    ///
    /// Returns `(artboard, layer)`.
    pub fn open_workspace(&mut self, width: f64, height: f64) -> (ArtboardId, LayerId) {
        if self.artboards.get(&crate::ids::DEFAULT_ARTBOARD).is_none() {
            let board = ArtboardRecord::new(
                crate::ids::DEFAULT_ARTBOARD,
                "Artboard 1",
                0.0,
                0.0,
                width,
                height,
            );
            self.artboards.insert(board, None);
            self.artboards.active = Some(crate::ids::DEFAULT_ARTBOARD);
        }
        if self.layers.layers.is_empty() {
            self.layers
                .insert(LayerRecord::new(crate::ids::DEFAULT_LAYER, "Layer 1"), None);
            self.attach_layer_to_artboard(crate::ids::DEFAULT_LAYER, None);
            self.active_layer = Some(crate::ids::DEFAULT_LAYER);
        }
        (
            self.artboards
                .active_id()
                .unwrap_or(crate::ids::DEFAULT_ARTBOARD),
            self.layers
                .layers
                .first()
                .map(|layer| layer.id)
                .unwrap_or(crate::ids::DEFAULT_LAYER),
        )
    }

    /// The layer new artwork should land in: the **active** one if the document
    /// recorded a choice, else the topmost layer, else `None` (no layers).
    pub fn active_layer(&self) -> Option<LayerId> {
        self.active_layer
            .filter(|id| self.layers.get(id).is_some())
            .or_else(|| self.layers.layers.last().map(|layer| layer.id))
    }

    /// Put a node in a layer, moving it from whichever layer listed it, and
    /// keep `order` in agreement. Returns the layer it left, if any.
    pub fn assign_to_layer(&mut self, node: NodeId, layer: LayerId) -> Option<LayerId> {
        if !self.nodes.contains_key(&node) || self.layers.get(&layer).is_none() {
            return None;
        }
        let previous = self.layers.detach(node);
        if let Some(target) = self.layers.get_mut(&layer) {
            target.children.push(node);
        }
        self.resync_order_from_layers();
        previous
    }

    /// Assign a node to the **active** layer, if the document has one. Used by
    /// `CreateNode` so that artwork made in the UI lands in the layer the panel
    /// says is current; a document with no layers is untouched.
    pub fn assign_to_active_layer(&mut self, node: NodeId) -> Option<LayerId> {
        let layer = self.active_layer()?;
        self.assign_to_layer(node, layer);
        Some(layer)
    }

    /// Detach a node from every layer (used when it is deleted).
    pub fn detach_from_layers(&mut self, node: NodeId) {
        if self.layers.detach(node).is_some() {
            self.resync_order_from_layers();
        }
    }

    /// The effective **visibility** of a node: its own flag *and* its layer's.
    ///
    /// **Document nodes only.** A *virtual* node — an operation result, a
    /// procedural result, a `Source` — has no entry here and therefore answers
    /// `false`; callers that project over a *scene* (which holds both kinds)
    /// must check `nodes.contains_key` first, exactly as the evaluator and
    /// [`vectra_geometry::EvaluatedScene::refresh_presentation`] do.
    pub fn visible(&self, node: NodeId) -> bool {
        self.nodes.get(&node).map(|n| n.visible).unwrap_or(false) && self.layers.visible_for(node)
    }

    /// The effective **lock** of a node: its own flag *or* its layer's.
    pub fn locked(&self, node: NodeId) -> bool {
        self.nodes.get(&node).map(|n| n.locked).unwrap_or(false) || self.layers.locked_for(node)
    }

    // ── Artboards (Task 10.2 RULE 2) ────────────────────────────────────────

    /// Ensure the document has an artboard covering `bounds`, returning its id.
    pub fn ensure_artboard(&mut self, width: f64, height: f64) -> ArtboardId {
        if let Some(board) = self.artboards.boards.first() {
            return board.id;
        }
        let id = crate::ids::new_artboard_id();
        self.artboards.insert(
            ArtboardRecord::new(id, "Artboard 1", 0.0, 0.0, width, height),
            None,
        );
        self.artboards.active = Some(id);
        id
    }

    /// The layer stack of one artboard, back → front.
    pub fn layers_of(&self, artboard: &ArtboardId) -> Vec<&LayerRecord> {
        self.artboards
            .get(artboard)
            .map(|board| board.layer_records(&self.layers))
            .unwrap_or_default()
    }

    /// Move an artboard's whole layer block to the top of the draw order — the
    /// "switch artboard, draw" case, where the board the designer is working on
    /// should be the one artwork lands on top of.
    pub fn raise_artboard(&mut self, artboard: &ArtboardId) {
        let Some(index) = self.artboards.boards.iter().position(|b| &b.id == artboard) else {
            return;
        };
        let board = self.artboards.boards.remove(index);
        self.artboards.boards.push(board);
        self.resync_order_from_layers();
    }

    pub fn validate_variable_name(name: &str) -> Result<(), VectraError> {
        if name.is_empty() || name.chars().any(char::is_whitespace) {
            return Err(VectraError::InvalidVariableName(name.to_string()));
        }
        Ok(())
    }

    pub fn set_variable(
        &mut self,
        name: VariableId,
        value: f64,
    ) -> Result<Option<f64>, VectraError> {
        Self::validate_variable_name(&name)?;
        Ok(self.variables.insert(name, value))
    }

    pub fn remove_variable(&mut self, name: &str) -> Result<f64, VectraError> {
        self.variables
            .remove(name)
            .ok_or_else(|| VectraError::VariableNotFound(name.to_string()))
    }

    /// Define (or redefine) an expression source. Core stores the source
    /// only — compilation lives in `vectra-expression`, and validation
    /// happens at the dispatch boundary (`vectra-wasm`), which pre-validates
    /// before this ever runs. Returns the previous source, if any.
    pub fn define_expression(&mut self, id: ExpressionId, source: String) -> Option<String> {
        self.expressions
            .insert(id, ExpressionRecord::new(source))
            .map(|record| record.source)
    }

    pub fn remove_expression(&mut self, id: ExpressionId) -> Result<String, VectraError> {
        self.expressions
            .remove(&id)
            .map(|record| record.source)
            .ok_or(VectraError::ExpressionNotFound(id))
    }

    /// Borrow an [`EvaluationContext`] over this document's variables.
    ///
    /// Owner crates inject their evaluators via the `with_*` builders before
    /// resolving; see [`crate::engine::Engine`] for the wired path.
    pub fn evaluation_context(&self, time: f64) -> EvaluationContext<'_> {
        EvaluationContext::new(&self.variables, time)
    }

    /// Convenience: create a node with a fresh ID and append it.
    pub fn create_node(&mut self, kind: NodeKind, name: Option<String>) -> NodeId {
        let id = new_node_id();
        let name = name.unwrap_or_else(|| format!("{} {}", kind.tag(), self.order.len() + 1));
        // Fresh UUID: cannot collide.
        let _ = self.insert_node(Node::new(id, name, kind), None);
        id
    }
}

impl Default for Document {
    fn default() -> Self {
        Self::new()
    }
}

/// Re-export for [`crate::eval::Resolvable`] implementors downstream.
#[allow(unused)]
pub(crate) fn _assert_resolve_error_send_sync() {
    fn f<T: Send + Sync>() {}
    f::<ResolveError>();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rectangle_property_roundtrip() {
        let mut node = Node::new(
            new_node_id(),
            "r",
            NodeKind::rectangle(0.0, 0.0, 100.0, 50.0),
        );
        assert_eq!(
            node.get_param("width").unwrap(),
            ParamValue::float_literal(100.0)
        );
        let old = node
            .set_param("width", ParamValue::Float(Parameter::variable("base")))
            .unwrap();
        assert_eq!(old, ParamValue::float_literal(100.0));
        assert_eq!(
            node.get_param("width").unwrap(),
            ParamValue::Float(Parameter::variable("base"))
        );
    }

    #[test]
    fn wrong_type_is_typed_error() {
        let mut node = Node::new(new_node_id(), "c", NodeKind::circle(0.0, 0.0, 10.0));
        let err = node
            .set_param("radius", ParamValue::point_literal(1.0, 2.0))
            .unwrap_err();
        assert!(matches!(err, VectraError::PropertyTypeMismatch { .. }));
    }

    #[test]
    fn unknown_property_names_node_kind() {
        let node = Node::new(new_node_id(), "c", NodeKind::circle(0.0, 0.0, 10.0));
        let err = node.get_param("width").unwrap_err();
        match err {
            VectraError::UnknownProperty { node_kind, .. } => assert_eq!(node_kind, "Circle"),
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn float_param_enumeration_matches_set_param_paths() {
        let mut node = Node::new(
            new_node_id(),
            "r",
            NodeKind::rectangle(0.0, 0.0, 100.0, 50.0),
        );
        node.set_param("width", ParamValue::Float(Parameter::variable("base")))
            .unwrap();
        node.set_param(
            "style.opacity",
            ParamValue::Float(Parameter::variable("fade")),
        )
        .unwrap();

        // The slot names are `String`s since Task 10.2: the appearance stack
        // names its slots by index (`style.appearances[0].width`), and a name
        // that has to be *built* cannot be `&'static str`.
        let mut seen: Vec<(String, &Parameter<f64>)> = Vec::new();
        node.for_each_float_param(|name, param| seen.push((name.to_string(), param)));
        let names: Vec<&str> = seen.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "x",
                "y",
                "width",
                "height",
                "corner_radius",
                "style.stroke_width",
                "style.opacity"
            ]
        );
        // Every enumerated name is addressable through the public accessors.
        for (name, param) in seen {
            let name: &str = &name;
            assert_eq!(
                node.get_param(name).unwrap(),
                ParamValue::Float(param.clone()),
                "{name} must round-trip through get_param"
            );
        }

        // Kinds without float slots still expose their style channels.
        let group = Node::new(new_node_id(), "g", NodeKind::Group { children: vec![] });
        let mut count = 0;
        group.for_each_float_param(|_, _| count += 1);
        assert_eq!(count, 2, "style.stroke_width + style.opacity");
    }

    #[test]
    fn document_insert_remove_preserves_order_index() {
        let mut doc = Document::new();
        let a = doc.create_node(NodeKind::rectangle(0.0, 0.0, 10.0, 10.0), None);
        let b = doc.create_node(NodeKind::circle(0.0, 0.0, 5.0), None);
        assert_eq!(doc.order, vec![a, b]);
        let (removed, index) = doc.remove_node(a).unwrap();
        assert_eq!(removed.id, a);
        assert_eq!(index, 0);
        doc.insert_node(removed, Some(index)).unwrap();
        assert_eq!(doc.order, vec![a, b]);
    }
}
