//! **Quick Shape snapping** (Task 10.1 RULE 3): a rough closed stroke becomes a
//! perfect primitive, through the Task 3.1 constraint solver.
//!
//! RULE 3 asks for something specific and unusual: *"if the user draws a rough
//! closed shape (circle or rectangle) and pauses/holds, the UI must silently
//! trigger the existing Constraint Solver to snap the path to a perfect
//! mathematical primitive."* Not "round the numbers" — **trigger the solver**.
//! So this module is split in two halves, and the split is the design:
//!
//! ```text
//!   ┌─ recognition + fit (pure math, this file) ─────────────────────────┐
//!   │  the stroke says "circle, centre (120, 84), radius 56, ±3 units"  │
//!   │  the stroke says "rectangle, corners (12,20)…(88,96), ±2 units"   │
//!   └───────────────────────────────────────────────────────────────────┘
//!                                  │
//!                                  ▼
//!   ┌─ the plan (this file) ─────────────────────────────────────────────┐
//!   │  geometry:   the ideal path (`SetPath`)                            │
//!   │  primitive:  a real `Circle` / `Rectangle` node                    │
//!   │  rows:       `Constraint`s over the *path's own slots*             │
//!   │  hints:      the anchors, at `STRONG`                              │
//!   └───────────────────────────────────────────────────────────────────┘
//!                                  │
//!                                  ▼
//!   ┌─ the engine (Task 3.1's solver, unchanged) ────────────────────────┐
//!   │  `ConstraintSolver::solve` → `SolveOutcome` → `SetParameter` writes │
//!   └───────────────────────────────────────────────────────────────────┘
//!
//! The point of the third stage is that the snap is **not a one-off numeric
//! substitution**. The resulting document carries real constraints between the
//! path's anchors and the primitive node's own parameters (`cx`/`cy`/`radius`),
//! which is what makes the result alive: change the `radius`, re-solve, and the
//! four anchors move to the new circle exactly. A snapped circle is therefore a
//! *parametric* circle, which is the whole reason this product exists.
//!
//! ## Only linear rows — and therefore only certain shapes
//!
//! Every row the solver can hold is a linear equality. That is not a limitation
//! to work around here, it is the shape of the vocabulary, and it decides what
//! "perfect" can mean:
//!
//! * a circle's cardinals are `(cx ± r, cy)`, `(cx, cy ± r)`: **linear** in the
//!   primitive's own `cx`, `cy`, `radius` slots — eight rows, exactly expressible;
//! * a rectangle's corners share coordinates pairwise: **linear** — four rows,
//!   which is exactly the "exact 90°" RULE 3 promises (a shared `y` and a shared
//!   `x` *is* a right angle);
//! * "all four corners are equidistant from a centre" is quadratic, and is
//!   deliberately **not** attempted — the circle case gets its power from
//!   constraining against the primitive's parameters instead, which is both
//!   linear and more useful (it is the primitive that stays editable).
//!
//! ## Strength: why these rows are `Medium`
//!
//! The engine's default for a user-authored rule. With `Required` rows the four
//! anchors of a snapped circle would be *immovable* — every direct-selection drag
//! would be pulled straight back — which is not a designer's tool. At `Medium` the
//! solver satisfies the snap whenever it is free to (so a `radius` change drives
//! the path exactly), and the last word still belongs to the person holding the
//! mouse. That is the same deal the Inspector offers for any other rule.

use vectra_core::{
    new_constraint_id, Constraint, ConstraintKind, ConstraintTarget, NodeId, NodeKind, PathSegment,
    Point2,
};

use crate::bezier::literal;
use crate::stroke;

/// The magic number behind a four-arc circle: `4/3 · (√2 − 1)`.
///
/// The handle length that makes a cubic Bézier arc of 90° track a circular arc to
/// within ~2.7 × 10⁻⁴ of the radius — the closest a cubic can get, and the number
/// every drawing tool, font engine and CAD kernel uses. The Quick Shape Law
/// asserts the approximation error against this bound rather than pretending the
/// result is an exact circle.
pub const KAPPA: f64 = 0.552_284_749_830_793_4;

/// Which primitive a stroke turned out to be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapKind {
    Circle,
    Rectangle,
}

impl SnapKind {
    pub fn tag(&self) -> &'static str {
        match self {
            Self::Circle => "circle",
            Self::Rectangle => "rectangle",
        }
    }
}

/// A fitted primitive, before any constraint is involved.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FittedPrimitive {
    Circle { center: Point2, radius: f64 },
    Rectangle { min: Point2, max: Point2 },
}

/// What the recognizer decided, with the evidence.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Recognition {
    pub kind: SnapKind,
    /// The primitive that best fits the stroke.
    pub primitive: FittedPrimitive,
    /// Worst distance from a sample to the ideal primitive, in document units.
    pub error: f64,
    /// How circular the stroke was (mean radial deviation / radius, `0` = perfect).
    pub circle_residual: f64,
    /// How rectangular the stroke was (fraction of samples near the box border).
    pub rect_score: f64,
    /// How circular it was (`1 − residual`, clamped).
    pub circle_score: f64,
}

/// **Recognize** a closed stroke: circle or rectangle, and how good the fit is.
///
/// The decision between the two is made on evidence, not on order of testing: both
/// candidates are fitted, both are scored the same way (the fraction of samples
/// that sit within `tolerance` of the candidate's outline), and the better score
/// wins as long as it beats `min_score`. A tie goes to the circle, because a
/// square-ish blob is far more often a drawn circle than a drawn box.
///
/// Returns `None` when neither shape is credible — a scribble must stay a
/// scribble; a tool that squares everything it sees cannot be trusted with a
/// deliberate wobble.
pub fn recognize(points: &[Point2], tolerance: f64, min_score: f64) -> Option<Recognition> {
    if points.len() < 8 {
        return None;
    }
    let rect = fit_rectangle(points);
    let circle = fit_circle(points);

    // ── structural gates, before any scoring ────────────────────────────────
    //
    // A score can be fooled by geometry that is mathematically "close" to a
    // primitive and obviously not one. The clean example, caught by the laws: a
    // **straight line** is within any tolerance of a circle of enormous radius,
    // so a scorer alone reports `circle_score = 1.0` for a stroke that is plainly
    // not a circle. Two facts fix it, and they are facts about drawn shapes
    // rather than about arithmetic:
    //
    //  * a shape the user closed comes back near where it started;
    //  * a drawn circle's radius is comparable to the stroke's own size — the
    //    diagonal of its bounding box is `r√2` for a circle, so anything outside
    //    `[0.15, 1.0] × diagonal` is a fit to a line, not a circle.
    let diagonal = ((rect.1.x - rect.0.x).powi(2) + (rect.1.y - rect.0.y).powi(2)).sqrt();
    if diagonal <= f64::EPSILON {
        return None;
    }
    let (first, last) = (points[0], points[points.len() - 1]);
    if crate::bezier::distance(first, last) > 0.25 * diagonal {
        return None;
    }
    if !(circle.1 >= 0.15 * diagonal && circle.1 <= diagonal) {
        return None;
    }
    let circle_score = outline_score(points, tolerance, |point| {
        crate::bezier::distance(*point, circle.0) - circle.1
    });
    let rect_score = outline_score(points, tolerance, |point| {
        distance_to_box_outline(*point, rect.0, rect.1)
    });
    let residual = circle_residual(points, circle.0, circle.1);

    let (kind, primitive, error, score) = if circle_score >= rect_score && circle_score >= min_score
    {
        (
            SnapKind::Circle,
            FittedPrimitive::Circle {
                center: circle.0,
                radius: circle.1,
            },
            circle.2,
            circle_score,
        )
    } else if rect_score > circle_score && rect_score >= min_score {
        let worst = points
            .iter()
            .map(|point| distance_to_box_outline(*point, rect.0, rect.1))
            .fold(0.0_f64, f64::max);
        (
            SnapKind::Rectangle,
            FittedPrimitive::Rectangle {
                min: rect.0,
                max: rect.1,
            },
            worst,
            rect_score,
        )
    } else {
        return None;
    };

    Some(Recognition {
        kind,
        primitive,
        error,
        circle_residual: residual,
        rect_score,
        circle_score: score,
    })
}

/// Fraction of samples within `tolerance` of an outline function.
fn outline_score(points: &[Point2], tolerance: f64, outline: impl Fn(&Point2) -> f64) -> f64 {
    let near = points
        .iter()
        .filter(|point| outline(point).abs() <= tolerance)
        .count();
    near as f64 / points.len() as f64
}

/// **Least-squares circle fit**: the algebraic (Kåsa) fit, then one refinement.
///
/// Solving for the centre directly (rather than averaging) matters because a
/// hand-drawn loop is unevenly sampled — a slow start, a fast finish — and the
/// centroid of the *samples* is not the centre of the *circle* they describe.
/// Returns `(centre, radius, max radial error)`.
pub fn fit_circle(points: &[Point2]) -> (Point2, f64, f64) {
    let n = points.len() as f64;
    let (mut sx, mut sy, mut sxx, mut syy, mut sxy, mut sxz, mut syz, mut sz) =
        (0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0);
    for point in points {
        let z = point.x * point.x + point.y * point.y;
        sx += point.x;
        sy += point.y;
        sxx += point.x * point.x;
        syy += point.y * point.y;
        sxy += point.x * point.y;
        sxz += point.x * z;
        syz += point.y * z;
        sz += z;
    }
    // Kåsa's normal equations, solved by Cramer's rule on the 2×2 system that
    // remains after eliminating the constant term.
    let a11 = 2.0 * (sxx - sx * sx / n);
    let a12 = 2.0 * (sxy - sx * sy / n);
    let a22 = 2.0 * (syy - sy * sy / n);
    let b1 = sxz - sx * sz / n;
    let b2 = syz - sy * sz / n;
    let det = a11 * a22 - a12 * a12;
    let center = if det.abs() > f64::EPSILON {
        Point2::new((b1 * a22 - b2 * a12) / det, (a11 * b2 - a12 * b1) / det)
    } else {
        // Degenerate (all samples collinear): fall back to the centroid, and let
        // the residual test reject the shape.
        stroke::centroid(points)
    };
    let radius = points
        .iter()
        .map(|point| crate::bezier::distance(*point, center))
        .sum::<f64>()
        / n;
    let worst = points
        .iter()
        .map(|point| (crate::bezier::distance(*point, center) - radius).abs())
        .fold(0.0_f64, f64::max);
    (center, radius, worst)
}

/// Mean radial deviation divided by the radius — the circle's honesty score.
pub fn circle_residual(points: &[Point2], center: Point2, radius: f64) -> f64 {
    if radius <= f64::EPSILON {
        return f64::INFINITY;
    }
    let sum: f64 = points
        .iter()
        .map(|point| (crate::bezier::distance(*point, center) - radius).abs())
        .sum();
    sum / points.len() as f64 / radius
}

/// The axis-aligned bounding box — the rectangle fit a stroke can be trusted with.
pub fn fit_rectangle(points: &[Point2]) -> (Point2, Point2) {
    let mut min = Point2::new(f64::INFINITY, f64::INFINITY);
    let mut max = Point2::new(f64::NEG_INFINITY, f64::NEG_INFINITY);
    for point in points {
        min.x = min.x.min(point.x);
        min.y = min.y.min(point.y);
        max.x = max.x.max(point.x);
        max.y = max.y.max(point.y);
    }
    if !min.x.is_finite() || !min.y.is_finite() || !max.x.is_finite() || !max.y.is_finite() {
        return (Point2::ZERO, Point2::ZERO);
    }
    (min, max)
}

/// Distance from `point` to a rectangle's **outline** (not its interior).
pub fn distance_to_box_outline(point: Point2, min: Point2, max: Point2) -> f64 {
    // Standing on the border means one coordinate is on a wall and the other is
    // within the span: take the smaller of "how far outside the box" horizontally
    // and vertically.
    let dx = (min.x - point.x).max(point.x - max.x).max(0.0);
    let dy = (min.y - point.y).max(point.y - max.y).max(0.0);
    let inside_dx = (point.x - min.x).min(max.x - point.x);
    let inside_dy = (point.y - min.y).min(max.y - point.y);
    if dx > 0.0 || dy > 0.0 {
        // Outside: the ordinary point-to-box distance.
        (dx * dx + dy * dy).sqrt()
    } else {
        // Inside: the distance to the nearest wall is what "near the outline"
        // should mean, so a stroke that fills the box is not mistaken for one
        // that traces it.
        inside_dx.min(inside_dy)
    }
}

/// The ideal path for a fitted primitive, plus the constraint plan that keeps it
/// true — everything the engine needs, with no solver types leaking out.
#[derive(Debug, Clone, PartialEq)]
pub struct SnapPlan {
    pub kind: SnapKind,
    /// The primitive node to create (`Circle { cx, cy, radius }` or
    /// `Rectangle { x, y, width, height }`).
    pub primitive: NodeKind,
    /// The path's first point.
    pub start: Point2,
    /// The path's segments — four cardinal arcs for a circle, four lines for a
    /// rectangle. Closed.
    pub segments: Vec<PathSegment>,
    /// The rules that hold the path and the primitive together.
    pub constraints: Vec<Constraint>,
    /// The slots to pin at `STRONG` while solving: the path's anchors.
    pub anchors: Vec<ConstraintTarget>,
    /// What the recognizer thought the stroke was, for the UI's status line.
    pub recognition: Recognition,
}

/// **Plan a Quick Shape snap.**
///
/// `path_id` is the node the drawn path lives in; `primitive_id` is the id the
/// caller will create the primitive with (the caller owns id generation, as it
/// does for every other command). Returns `None` when the stroke is not a
/// credible primitive — recognition's own refusal, passed through unchanged.
pub fn plan_quick_shape(
    stroke_points: &[Point2],
    path_id: NodeId,
    primitive_id: NodeId,
    tolerance: f64,
) -> Option<SnapPlan> {
    let recognition = recognize(stroke_points, tolerance.max(1.0), 0.6)?;
    Some(match recognition.primitive {
        FittedPrimitive::Circle { center, radius } => {
            plan_circle(center, radius, path_id, primitive_id, recognition)
        }
        FittedPrimitive::Rectangle { min, max } => {
            plan_rectangle(min, max, path_id, primitive_id, recognition)
        }
    })
}

/// A path axis name (`"x"`) → the circle slot it pairs with (`"cx"`).
fn shared_circle_slot(axis: &str) -> &'static str {
    match axis {
        "x" => "cx",
        _ => "cy",
    }
}

/// The four cardinal anchors of a circle, in draw order (right, bottom, left, top).
fn circle_cardinals(center: Point2, radius: f64) -> [Point2; 4] {
    [
        Point2::new(center.x + radius, center.y),
        Point2::new(center.x, center.y + radius),
        Point2::new(center.x - radius, center.y),
        Point2::new(center.x, center.y - radius),
    ]
}

/// Four 90° cubic arcs, the classic kappa construction.
///
/// Each arc's handles sit `κ·r` along the tangent at each end, which is the
/// minimum-error cubic approximation of the quarter circle. The path therefore
/// *is* a circle to the precision the Quick Shape Law asserts, and it is expressed
/// in RULE 1's own vocabulary: `PathSegment::Cubic` with `Parameter<Point2>`
/// control points, nothing else.
///
/// The arcs run **right → bottom → left → top**, so the sequence starts at
/// `(cx + r, cy)` and ends back there: the returned path is closed without a
/// `Close` segment, and its four anchors are the four cardinals. A closed shape
/// with exactly four vertices is what a designer expects to see when they select
/// a circle — and it is what makes the Quick Shape snap's constraint rows total.
pub fn circle_segments(center: Point2, radius: f64) -> Vec<PathSegment> {
    let [right, bottom, left, top] = circle_cardinals(center, radius);
    let k = KAPPA * radius;
    // Tangent directions at each cardinal, counter-clockwise in document space.
    let tangents = [
        Point2::new(0.0, 1.0),  // at right, heading down (+y)
        Point2::new(-1.0, 0.0), // at bottom, heading left
        Point2::new(0.0, -1.0), // at left, heading up
        Point2::new(1.0, 0.0),  // at top, heading right
    ];
    let points = [right, bottom, left, top];
    (0..4)
        .map(|index| {
            let from = points[index];
            let to = points[(index + 1) % 4];
            let (t_from, t_to) = (tangents[index], tangents[(index + 1) % 4]);
            PathSegment::Cubic {
                control1: literal(Point2::new(from.x + t_from.x * k, from.y + t_from.y * k)),
                control2: literal(Point2::new(to.x - t_to.x * k, to.y - t_to.y * k)),
                to: literal(to),
            }
        })
        .collect()
}

fn plan_circle(
    center: Point2,
    radius: f64,
    path_id: NodeId,
    primitive_id: NodeId,
    recognition: Recognition,
) -> SnapPlan {
    // The four cardinals are exactly the four anchors the path has — `start` is
    // the right-hand one and the arcs run right → bottom → left → top, so the
    // path closes on its own first point with no duplicate vertex and no
    // trailing `Close` (see `circle_segments`).
    let [right, _bottom, _left, _top] = circle_cardinals(center, radius);
    let anchors = [
        ConstraintTarget::new(path_id, "start"),
        ConstraintTarget::new(path_id, "segments[0].to"),
        ConstraintTarget::new(path_id, "segments[1].to"),
        ConstraintTarget::new(path_id, "segments[2].to"),
    ];
    // The eight rows that make the cardinals *exactly* the cardinals of the
    // primitive: `right = (cx + r, cy)`, `bottom = (cx, cy + r)`, and so on.
    // Each is linear in the primitive's own slots, which is what lets the
    // primitive stay the source of truth — resize the circle and the path follows.
    //
    // ── who is the source of truth? (the ordering decision) ────────────────
    //
    // A row is a statement about *slots*, and the engine's dispatch is a
    // statement about *the slot the user just touched*: `hints_for` takes each
    // command's **first** target and pins it at its current value, `STRONG`. So
    // the order of a row's targets is not cosmetic — it is the difference between
    // two entirely different tools:
    //
    // | first target | what moves when the system is solved |
    // | --- | --- |
    // | the anchor | the *circle* slides over to the (already correct) anchors |
    // | the primitive | the *anchors* snap onto the circle |
    //
    // The second is what a designer means by "snap to circle", and it is what
    // this plan encodes: **every row names the primitive's slot first**. The
    // snap's own three pinning rows (`Angle(cx)`, `Angle(cy)`, `Angle(radius)`)
    // then hold the primitive at the fitted values, so the only thing a solve can
    // move is the path.
    let mut constraints = Vec::with_capacity(13);
    let circle_slot = |property: &str| ConstraintTarget::new(primitive_id, property);
    let anchor_slot = |index: usize, axis: &str| {
        if index == 0 {
            // The first cardinal is the path's own first point.
            ConstraintTarget::new(path_id, format!("start.{axis}"))
        } else {
            ConstraintTarget::new(path_id, format!("segments[{}].to.{axis}", index - 1))
        }
    };
    // The fitted primitive, pinned. `Angle` is `slot == value` in Phase 1, and for
    // this use it reads as "the circle is where the recognizer says it is".
    for (property, value) in [("cx", center.x), ("cy", center.y), ("radius", radius)] {
        constraints.push(
            Constraint::new(
                new_constraint_id(),
                ConstraintKind::Angle,
                vec![circle_slot(property)],
            )
            .with_value(value),
        );
    }
    // The four cardinals, as a table — because "which axis is shared with the
    // centre and which is one radius away" is the whole content of the geometry
    // and a reader should be able to check it at a glance:
    //
    // | anchor | shares with the centre | one radius away |
    // | --- | --- | --- |
    // | `start` (right) | `y = cy` | `x = cx + r` |
    // | `segments[0].to` (bottom) | `x = cx` | `y = cy + r` |
    // | `segments[1].to` (left) | `y = cy` | `x = cx − r` |
    // | `segments[2].to` (top) | `x = cx` | `y = cy − r` |
    const CARDINALS: [(&str, &str, f64); 4] = [
        ("y", "x", 1.0),  // start   = right:  y == cy,  x == cx + r
        ("x", "y", 1.0),  // segments[0].to = bottom
        ("y", "x", -1.0), // segments[1].to = left
        ("x", "y", -1.0), // segments[2].to = top
    ];
    for (index, (shared, offset, sign)) in CARDINALS.into_iter().enumerate() {
        constraints.push(Constraint::new(
            new_constraint_id(),
            ConstraintKind::Coincident,
            vec![
                circle_slot(shared_circle_slot(shared)),
                anchor_slot(index, shared),
            ],
        ));
        // `centre − anchor == −(±r)`, primitive slot first — so the row's hint
        // pins the *circle*, never the anchor (see the ordering table above).
        constraints.push(
            Constraint::new(
                new_constraint_id(),
                ConstraintKind::Distance,
                vec![
                    circle_slot(shared_circle_slot(offset)),
                    anchor_slot(index, offset),
                ],
            )
            .with_value(-sign * radius),
        );
    }

    // The **seam**, held shut by the solver. The fourth arc ends on the start
    // point — that is what closes the path — so the path carries five slots for
    // four distinct positions, and without these two rows nothing would stop a
    // designer from dragging `start` away from the endpoint sitting on top of it
    // and tearing the circle open at the seam. Two coincident rows make the
    // closure an invariant of the document rather than a coincidence of the
    // numbers the snap wrote.
    for axis in ["x", "y"] {
        constraints.push(Constraint::new(
            new_constraint_id(),
            ConstraintKind::Coincident,
            vec![
                ConstraintTarget::new(path_id, format!("segments[3].to.{axis}")),
                ConstraintTarget::new(path_id, format!("start.{axis}")),
            ],
        ));
    }

    SnapPlan {
        kind: SnapKind::Circle,
        primitive: NodeKind::Circle {
            cx: literal_scalar(center.x),
            cy: literal_scalar(center.y),
            radius: literal_scalar(radius),
        },
        start: right,
        // Four arcs and **no** trailing `Close`: the fourth arc's endpoint *is*
        // the start point, so the path is closed by construction. Appending a
        // `Close` would add a zero-length segment and a fifth anchor sitting
        // exactly on the first — the seam every vector tool avoids, and a vertex
        // the direct-selection tool would let a user drag apart. The fill is
        // unaffected: both the tessellator and SVG close a subpath implicitly.
        segments: circle_segments(center, radius),
        constraints,
        anchors: anchors.to_vec(),
        recognition,
    }
}

fn plan_rectangle(
    min: Point2,
    max: Point2,
    path_id: NodeId,
    // The rectangle node is authored alongside the snapped path but is **not**
    // parametrically wired to it, and that is a fact about the vocabulary rather
    // than an omission: "the right edge is `x + width` away" is a three-term row
    // (`x + width - p1.x == 0`), and every `ConstraintKind` in Phase 1 is a
    // two-term equality or a single slot against a captured constant. The
    // rectangle's *orthogonality* — the property RULE 3 names — is fully
    // solver-enforced by the four rows below; its *dimensional* link would need a
    // new row shape, which is a Task 3.1 decision, not a drawing-tool one. The
    // circle case needs no such row, which is why a snapped circle comes out
    // parametric and a snapped box comes out exact.
    _primitive_id: NodeId,
    recognition: Recognition,
) -> SnapPlan {
    // Clockwise from the top-left, the order a person draws a box in.
    let corners = [
        min,
        Point2::new(max.x, min.y),
        max,
        Point2::new(min.x, max.y),
    ];
    let anchor = |index: usize, axis: &str| {
        ConstraintTarget::new(path_id, format!("segments[{index}].to.{axis}"))
    };
    // Four rows, and they are exactly "every corner is 90°": sharing a `y` along
    // each horizontal edge and an `x` along each vertical edge *is* the right
    // angle, with no trigonometry and nothing to get wrong.
    let constraints = vec![
        Constraint::new(
            new_constraint_id(),
            ConstraintKind::Horizontal,
            vec![anchor(0, "y"), anchor(1, "y")],
        ),
        Constraint::new(
            new_constraint_id(),
            ConstraintKind::Horizontal,
            vec![anchor(3, "y"), anchor(2, "y")],
        ),
        Constraint::new(
            new_constraint_id(),
            ConstraintKind::Vertical,
            vec![anchor(0, "x"), anchor(3, "x")],
        ),
        Constraint::new(
            new_constraint_id(),
            ConstraintKind::Vertical,
            vec![anchor(1, "x"), anchor(2, "x")],
        ),
    ];
    // Which would fail for a rectangle, and is exactly the evidence the test
    // uses to prove the solver was consulted: the corners are created from the
    // bounding box, so they already satisfy the rows — a solve moves nothing
    // (`writes == 0`), which is the engine's own definition of "already true".
    SnapPlan {
        kind: SnapKind::Rectangle,
        primitive: NodeKind::Rectangle {
            x: literal_scalar(min.x),
            y: literal_scalar(min.y),
            width: literal_scalar((max.x - min.x).max(0.0)),
            height: literal_scalar((max.y - min.y).max(0.0)),
            corner_radius: literal_scalar(0.0),
        },
        start: corners[0],
        segments: vec![
            PathSegment::Line {
                to: literal(corners[1]),
            },
            PathSegment::Line {
                to: literal(corners[2]),
            },
            PathSegment::Line {
                to: literal(corners[3]),
            },
            PathSegment::Close,
        ],
        constraints,
        anchors: (0..4)
            .map(|index| ConstraintTarget::new(path_id, format!("segments[{index}].to")))
            .collect(),
        recognition,
    }
}

fn literal_scalar(value: f64) -> vectra_core::Parameter<f64> {
    vectra_core::Parameter::Literal(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use vectra_core::new_node_id;

    fn p(x: f64, y: f64) -> Point2 {
        Point2::new(x, y)
    }

    /// A jagged circle: `radius` wobble up to `noise`, sampled at `count` points.
    fn jagged_circle(center: Point2, radius: f64, noise: f64, count: usize) -> Vec<Point2> {
        (0..count)
            .map(|i| {
                let t = i as f64 / count as f64 * std::f64::consts::TAU;
                // A deterministic pseudo-noise: integer hashing, no RNG.
                let hash = ((i.wrapping_mul(2654435761)) % 1000) as f64 / 1000.0;
                let r = radius + (hash - 0.5) * 2.0 * noise;
                p(center.x + r * t.cos(), center.y + r * t.sin())
            })
            .collect()
    }

    fn jagged_rectangle(min: Point2, max: Point2, noise: f64, per_edge: usize) -> Vec<Point2> {
        let width = max.x - min.x;
        let height = max.y - min.y;
        let mut points = Vec::new();
        for i in 0..per_edge {
            let t = i as f64 / per_edge as f64;
            points.push(p(min.x + t * width, min.y + noise * ((i % 3) as f64 - 1.0)));
        }
        for i in 0..per_edge {
            let t = i as f64 / per_edge as f64;
            points.push(p(
                max.x + noise * ((i % 3) as f64 - 1.0),
                min.y + t * height,
            ));
        }
        for i in 0..per_edge {
            let t = i as f64 / per_edge as f64;
            points.push(p(max.x - t * width, max.y + noise * ((i % 3) as f64 - 1.0)));
        }
        for i in 0..per_edge {
            let t = i as f64 / per_edge as f64;
            points.push(p(
                min.x + noise * ((i % 3) as f64 - 1.0),
                max.y - t * height,
            ));
        }
        points
    }

    #[test]
    fn a_jagged_circle_is_recognized_and_fitted() {
        let center = p(120.0, 84.0);
        let stroke = jagged_circle(center, 56.0, 3.0, 48);
        let recognition = recognize(&stroke, 6.0, 0.6).expect("a closed loop is a circle");
        assert_eq!(recognition.kind, SnapKind::Circle);
        let FittedPrimitive::Circle { center: c, radius } = recognition.primitive else {
            panic!("expected a circle");
        };
        assert!(crate::bezier::distance(c, center) < 2.0, "centre {c:?}");
        assert!((radius - 56.0).abs() < 3.0, "radius {radius}");
    }

    #[test]
    fn a_jagged_box_is_recognized_as_a_rectangle() {
        let stroke = jagged_rectangle(p(10.0, 20.0), p(90.0, 100.0), 2.0, 12);
        let recognition = recognize(&stroke, 6.0, 0.6).expect("a box is a rectangle");
        assert_eq!(recognition.kind, SnapKind::Rectangle, "{recognition:?}");
    }

    #[test]
    fn a_scribble_is_refused() {
        // Back and forth: not closed, not convex, not any primitive.
        let scribble: Vec<Point2> = (0..60)
            .map(|i| {
                let t = i as f64 * 0.7;
                p(t * 4.0, 40.0 * (t * 3.1).sin())
            })
            .collect();
        assert!(recognize(&scribble, 6.0, 0.6).is_none());
    }

    #[test]
    fn the_snapped_circle_path_is_a_circle_to_the_kappa_bound() {
        use vectra_core::Parameter;
        let center = p(0.0, 0.0);
        let radius = 100.0;
        let segments = circle_segments(center, radius);
        // Sample each arc and measure the radial error.
        let mut worst = 0.0_f64;
        let mut from = p(radius, 0.0); // the first cardinal: `start`
        for segment in &segments {
            let PathSegment::Cubic {
                control1,
                control2,
                to,
            } = segment
            else {
                panic!("expected cubics");
            };
            let (Parameter::Literal(c1), Parameter::Literal(c2), Parameter::Literal(end)) =
                (control1, control2, to)
            else {
                panic!("expected literals");
            };
            for i in 0..=64 {
                let t = i as f64 / 64.0;
                let point = crate::bezier::cubic_at(from, *c1, *c2, *end, t);
                worst = worst.max((crate::bezier::distance(point, center) - radius).abs());
            }
            from = *end;
        }
        // The path really is closed: the last arc ends where the first began.
        assert!(crate::bezier::distance(from, p(radius, 0.0)) < 1e-9);
        assert!(
            worst <= radius * 5e-4,
            "kappa approximation error {worst} exceeds the 5e-4·r bound"
        );
    }

    #[test]
    fn a_circle_plan_pairs_eleven_rows_with_the_primitive() {
        let stroke = jagged_circle(p(50.0, 50.0), 30.0, 3.0, 40);
        let path_id = new_node_id();
        let primitive_id = new_node_id();
        let plan = plan_quick_shape(&stroke, path_id, primitive_id, 6.0).unwrap();
        assert_eq!(plan.kind, SnapKind::Circle);
        // 3 pins + 4 shared-axis rows + 4 one-radius rows + 2 seam rows.
        assert_eq!(plan.constraints.len(), 13, "thirteen rows hold the circle");
        // Four arcs and no `Close`: the fourth arc lands back on `start`.
        assert_eq!(plan.segments.len(), 4, "four arcs, closed by construction");
        assert!(matches!(plan.primitive, NodeKind::Circle { .. }));
        // The three pins name only the primitive…
        for constraint in &plan.constraints[..3] {
            assert_eq!(constraint.targets.len(), 1);
            assert_eq!(constraint.targets[0].node_id, primitive_id);
        }
        // …and every cardinal row touches both the path and the primitive, with
        // the primitive FIRST — the ordering that makes the path follow the
        // circle rather than the other way round.
        for constraint in &plan.constraints[3..11] {
            assert_eq!(constraint.targets.len(), 2);
            assert_eq!(
                constraint.targets[0].node_id, primitive_id,
                "the primitive comes first: {constraint:?}"
            );
            assert_eq!(constraint.targets[1].node_id, path_id);
            assert!(
                constraint
                    .targets
                    .iter()
                    .any(|t| t.property.ends_with(".x") || t.property.ends_with(".y")),
                "{constraint:?}"
            );
        }
        // The four anchors are the path's four *cardinal* slots, `start` included.
        assert_eq!(plan.anchors.len(), 4);
        assert_eq!(plan.anchors[0].property, "start");
        assert_eq!(plan.anchors[1].property, "segments[0].to");
        // The seam rows hold the fourth arc's endpoint on the start point.
        let seam: Vec<&Constraint> = plan.constraints[11..].iter().collect();
        assert_eq!(seam.len(), 2, "two seam rows");
        for constraint in seam {
            assert!(constraint
                .targets
                .iter()
                .all(|target| target.node_id == path_id));
            assert!(constraint
                .targets
                .iter()
                .any(|target| target.property.starts_with("segments[3].to")));
        }
    }

    #[test]
    fn a_rectangle_plan_pairs_four_orthogonality_rows() {
        let stroke = jagged_rectangle(p(10.0, 20.0), p(90.0, 100.0), 2.0, 12);
        let plan = plan_quick_shape(&stroke, new_node_id(), new_node_id(), 6.0).unwrap();
        assert_eq!(plan.kind, SnapKind::Rectangle);
        assert_eq!(plan.constraints.len(), 4);
        let kinds: Vec<ConstraintKind> = plan.constraints.iter().map(|c| c.kind).collect();
        assert_eq!(
            kinds
                .iter()
                .filter(|k| **k == ConstraintKind::Horizontal)
                .count(),
            2
        );
        assert_eq!(
            kinds
                .iter()
                .filter(|k| **k == ConstraintKind::Vertical)
                .count(),
            2
        );
        assert_eq!(plan.segments.len(), 4);
        // The corners are the *fitted* box's — which is the outermost noise of
        // the stroke, not the ideal rectangle the generator aimed at. A snapping
        // tool that ignored a 2-unit wobble would be lying about what the user
        // drew; ±2 is what `jagged_rectangle` asked for, so that is what the fit
        // must return.
        assert!(
            (plan.start.x - 10.0).abs() <= 2.5 && (plan.start.y - 20.0).abs() <= 2.5,
            "start {:?} should hug the noisiest corner",
            plan.start
        );
    }

    #[test]
    fn the_fit_beats_the_bounding_box_for_an_offset_loop() {
        // A loop sampled densely on one side: the centroid of the samples is
        // pulled off-centre, and the least-squares fit is not.
        let true_center = p(0.0, 0.0);
        let mut points = Vec::new();
        for i in 0..90 {
            let t = i as f64 / 90.0 * std::f64::consts::TAU;
            points.push(p(50.0 * t.cos(), 50.0 * t.sin()));
            // …plus a second cluster of samples near angle 0.
            if i % 3 == 0 {
                points.push(p(50.0 * (t * 0.01).cos(), 50.0 * (t * 0.01).sin()));
            }
        }
        let (center, radius, _) = fit_circle(&points);
        let centroid = stroke::centroid(&points);
        assert!(
            crate::bezier::distance(center, true_center)
                < crate::bezier::distance(centroid, true_center),
            "the fit {center:?} should beat the centroid {centroid:?}"
        );
        assert!((radius - 50.0).abs() < 5.0);
    }
}
