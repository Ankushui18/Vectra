//! Bézier math for the drawing tools (Task 10.1 RULE 2).
//!
//! Two jobs, both of them about *handles*:
//!
//! * **Building** a segment from a drag. In every vector tool the pen's drag is
//!   the same gesture: the pointer leaves the anchor and the vector to where it
//!   is becomes the outgoing handle.
//! * **Fitting** a curve to a sampled stroke (the brush, [`crate::brush`]) — and
//!   the one function that makes the brush's fitting *provable*:
//!   [`interpolate`], the value of a Bézier at `t`.
//!
//! ## Placing the handles a third of the way
//!
//! Given four points, the handle pulled out when starting at `a` and the one
//! pulled back when arriving at `d`, the classic construction puts each handle at
//! **one third of the chord** along its tangent:
//!
//! ```text
//!          c1 = a + (b - a)/3                 c2 = d + (c - d)/3
//!   a ●──────────────▶ c1              c2 ◀──────────────● d
//!     \                                                   /
//!      \                     the chord a→d               /
//!       ●─────────────────────────────────────────────●
//! ```
//!
//! A third, not a half: at a third the curve through the *whole* chord is the
//! closest cubic approximation of the circular arc of the same sweep, which is
//! why every CAD kernel snaps to it. Expressed in the engine's own vocabulary the
//! result is a [`PathSegment::Cubic`] whose four slots are
//! `Parameter<Point2>` — RULE 1's requirement that the tool produce engine types,
//! not a parallel geometry model.

use vectra_core::{Parameter, PathSegment, Point2};

/// The arc-approximation constant: a handle reaches one third of the chord.
///
/// Kept as a named constant because it is the single number that decides how
/// "round" a dragged segment looks, and because the tests assert against it by
/// name rather than by a magic literal.
pub const HANDLE_FRACTION: f64 = 1.0 / 3.0;

/// A point with the handles attached to it — one **anchor** of a path.
///
/// This is the pen's working vocabulary: the user places anchors, and a drag
/// decides whether an anchor is a corner (no handle), smooth (mirrored handles)
/// or broken (independent handles). It is *transient* — the document stores
/// [`PathSegment`]s — but it is what a pen state machine can hold between two
/// pointer events without re-reading the path.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Anchor {
    /// The point on the path itself.
    pub point: Point2,
    /// Handle controlling the curve *arriving* at this anchor (`None` = corner).
    pub handle_in: Option<Point2>,
    /// Handle controlling the curve *leaving* this anchor (`None` = corner).
    pub handle_out: Option<Point2>,
}

impl Anchor {
    /// A sharp corner: the point, no handles.
    pub fn corner(point: Point2) -> Self {
        Self {
            point,
            handle_in: None,
            handle_out: None,
        }
    }

    /// A **smooth** anchor: the drag vector mirrored, so the two handles are
    /// opposite and (by construction) collinear through the point.
    ///
    /// `drag` is the pointer vector from the anchor while the mouse button is
    /// held: it becomes `handle_out`, and `handle_in` is its negation. Mirroring
    /// rather than scaling means the *tension* the user draws with is exactly the
    /// tension they see on both sides — the Procreate feel, and the rule the
    /// Handle Symmetry Law asserts.
    pub fn smooth(point: Point2, drag: Point2) -> Self {
        Self {
            point,
            handle_in: Some(Point2::new(point.x - drag.x, point.y - drag.y)),
            handle_out: Some(Point2::new(point.x + drag.x, point.y + drag.y)),
        }
    }

    /// A **broken** anchor: the out handle is wherever the user dragged, the in
    /// handle stays where it was (Alt/Option-drag — RULE 2's third bullet).
    ///
    /// `keep_in` is the existing in-handle, which is what "independent" means: the
    /// gesture touches one handle and nothing else.
    pub fn broken(point: Point2, drag: Point2, keep_in: Option<Point2>) -> Self {
        Self {
            point,
            handle_in: keep_in,
            handle_out: Some(Point2::new(point.x + drag.x, point.y + drag.y)),
        }
    }

    /// Is this anchor smooth? True when both handles exist and mirror each other
    /// exactly — the property the symmetry law checks.
    pub fn is_smooth(&self, tolerance: f64) -> bool {
        match (self.handle_in, self.handle_out) {
            (Some(a), Some(b)) => {
                let mx = (a.x + b.x) / 2.0;
                let my = (a.y + b.y) / 2.0;
                (mx - self.point.x).abs() <= tolerance && (my - self.point.y).abs() <= tolerance
            }
            _ => false,
        }
    }
}

/// Does a drag count as a handle pull at all?
///
/// A click is not a drag: a pen user who clicks to place a corner often moves the
/// pointer a pixel or two while the button is down, and turning that into a
/// curved segment would make the tool feel broken. The threshold is in document
/// units and deliberately small — it exists to absorb tremor, not intent.
pub const DRAG_THRESHOLD: f64 = 2.0;

/// The segment from `from` to `to`, given the two anchors' handles (Task 10.1
/// RULE 2, the whole of the pen's geometry).
///
/// | `handle_out` of `from` | `handle_in` of `to` | segment |
/// | --- | --- | --- |
/// | `None` | `None` | `Line` — a corner-to-corner click |
/// | either | either | `Cubic` — exactly one handle is enough to curve |
///
/// One handle is enough on purpose: a smooth anchor's *in* handle is what curves
/// the segment arriving at it, and a user who Alt-drags a single handle into a
/// corner should still see the segment bend toward it. Requiring both would
/// silently straighten segments the user can see are pulled.
pub fn segment_between(from: &Anchor, to: &Anchor) -> PathSegment {
    match (from.handle_out, to.handle_in) {
        (None, None) => PathSegment::Line {
            to: literal(to.point),
        },
        (out, incoming) => {
            // A missing handle degenerates to its own anchor, which is what makes
            // the one-handle case exact: the curve leaves `from` along the
            // handle it has and arrives at `to` travelling straight from it.
            let c1 = out.unwrap_or(from.point);
            let c2 = incoming.unwrap_or(to.point);
            PathSegment::Cubic {
                control1: literal(c1),
                control2: literal(c2),
                to: literal(to.point),
            }
        }
    }
}

/// The closing segment back to the first anchor.
///
/// A close can carry handles too — closing a drawn circle with a corner would
/// put a visible kink where the path meets its start, so the first anchor's
/// `handle_in` and the last anchor's `handle_out` are honoured exactly like any
/// other segment.
pub fn closing_segment(from: &Anchor, to: &Anchor) -> PathSegment {
    match (from.handle_out, to.handle_in) {
        (None, None) => PathSegment::Close,
        (out, incoming) => PathSegment::Cubic {
            control1: literal(out.unwrap_or(from.point)),
            control2: literal(incoming.unwrap_or(to.point)),
            to: literal(to.point),
        },
    }
}

/// The handle a drag pulls out, as an **absolute** point.
pub fn handle_from_drag(anchor: Point2, drag: Point2) -> Point2 {
    Point2::new(anchor.x + drag.x, anchor.y + drag.y)
}

/// The drag vector implied by an anchor and one of its handles.
pub fn drag_of(anchor: Point2, handle: Point2) -> Point2 {
    Point2::new(handle.x - anchor.x, handle.y - anchor.y)
}

/// How far a drag has to travel before it is a handle pull rather than a click.
pub fn is_drag(drag: Point2) -> bool {
    (drag.x * drag.x + drag.y * drag.y).sqrt() > DRAG_THRESHOLD
}

/// Wrap a point as a literal parameter — the form every drawn vertex takes.
///
/// A drawn vertex is a **literal**: it is where the user put it. RULE 1's point
/// is that the slot is a `Parameter<Point2>`, so the *engine* can later bind that
/// same slot to a variable, an expression or a procedural port — the tool does
/// not need to know, and does not try to guess.
pub fn literal(point: Point2) -> Parameter<Point2> {
    Parameter::Literal(point)
}

/// Evaluate a cubic Bézier at `t ∈ [0, 1]` (the standard Bernstein form).
///
/// The brush's fitting computes each segment's chord error with this, which is
/// what makes the Flatness Law ("the fitted curve stays near its samples")
/// checkable in the same units the tolerance is expressed in.
pub fn cubic_at(p0: Point2, c1: Point2, c2: Point2, p3: Point2, t: f64) -> Point2 {
    let mt = 1.0 - t;
    let a = mt * mt * mt;
    let b = 3.0 * mt * mt * t;
    let c = 3.0 * mt * t * t;
    let d = t * t * t;
    Point2::new(
        a * p0.x + b * c1.x + c * c2.x + d * p3.x,
        a * p0.y + b * c1.y + c * c2.y + d * p3.y,
    )
}

/// The Bézier tangent (first derivative) at `t`, unnormalised.
///
/// Used for two things: the direction a segment *arrives* at its endpoint (which
/// the pen uses to keep a continued stroke smooth), and the aspect of the
/// fitted curve that decides whether a stroke's direction reversed.
pub fn cubic_tangent_at(p0: Point2, c1: Point2, c2: Point2, p3: Point2, t: f64) -> Point2 {
    let mt = 1.0 - t;
    let a = 3.0 * mt * mt;
    let b = 6.0 * mt * t;
    let c = 3.0 * t * t;
    Point2::new(
        a * (c1.x - p0.x) + b * (c2.x - c1.x) + c * (p3.x - c2.x),
        a * (c1.y - p0.y) + b * (c2.y - c1.y) + c * (p3.y - c2.y),
    )
}

/// [`cubic_at`], with the endpoint handed to `t` clamped to `[0, 1]`.
///
/// Pointer-driven code routinely overshoots `t` by a hair; clamping here means no
/// caller has to, and no caller can produce a point outside the curve's hull.
pub fn cubic_at_clamped(p0: Point2, c1: Point2, c2: Point2, p3: Point2, t: f64) -> Point2 {
    cubic_at(p0, c1, c2, p3, t.clamp(0.0, 1.0))
}

/// Distance from `p` to the cubic, approximated by sampling `samples + 1` points.
///
/// The brush uses this to decide "is a straight line good enough?" — comparing
/// the *maximum deviation* of the samples from the chord against a tolerance,
/// which is the same criterion (and the same units) the renderer's flattening
/// uses. `samples` is a caller decision: 8 is plenty for a fitting decision, and
/// more would only make the answer more expensive, not more true.
pub fn cubic_deviation_from_chord(
    p0: Point2,
    c1: Point2,
    c2: Point2,
    p3: Point2,
    samples: usize,
) -> f64 {
    let mut worst: f64 = 0.0;
    for index in 1..samples {
        let t = index as f64 / samples as f64;
        let point = cubic_at(p0, c1, c2, p3, t);
        worst = worst.max(distance_to_segment(point, p0, p3));
    }
    worst
}

/// Perpendicular distance from `p` to the **segment** `a→b` (not the infinite line).
pub fn distance_to_segment(p: Point2, a: Point2, b: Point2) -> f64 {
    let vx = b.x - a.x;
    let vy = b.y - a.y;
    let len2 = vx * vx + vy * vy;
    if len2 <= f64::EPSILON {
        return ((p.x - a.x).powi(2) + (p.y - a.y).powi(2)).sqrt();
    }
    let t = (((p.x - a.x) * vx + (p.y - a.y) * vy) / len2).clamp(0.0, 1.0);
    let cx = a.x + t * vx;
    let cy = a.y + t * vy;
    ((p.x - cx).powi(2) + (p.y - cy).powi(2)).sqrt()
}

/// The distance between two points.
pub fn distance(a: Point2, b: Point2) -> f64 {
    ((a.x - b.x).powi(2) + (a.y - b.y).powi(2)).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(x: f64, y: f64) -> Point2 {
        Point2::new(x, y)
    }

    #[test]
    fn a_click_between_corners_is_a_line() {
        let a = Anchor::corner(p(0.0, 0.0));
        let b = Anchor::corner(p(10.0, 0.0));
        assert!(matches!(segment_between(&a, &b), PathSegment::Line { .. }));
    }

    #[test]
    fn a_drag_between_smooth_anchors_is_a_cubic_with_mirrored_handles() {
        let a = Anchor::smooth(p(0.0, 0.0), p(10.0, 0.0));
        let b = Anchor::smooth(p(30.0, 0.0), p(10.0, 0.0));
        let PathSegment::Cubic {
            control1,
            control2,
            to,
        } = segment_between(&a, &b)
        else {
            panic!("a drag should produce a cubic");
        };
        assert_eq!(control1, literal(p(10.0, 0.0)));
        assert_eq!(control2, literal(p(20.0, 0.0)));
        assert_eq!(to, literal(p(30.0, 0.0)));
        assert!(
            a.is_smooth(1e-9),
            "the smooth constructor mirrors by construction"
        );
    }

    #[test]
    fn a_broken_anchor_keeps_its_other_handle() {
        let keep = p(-5.0, 0.0);
        let anchor = Anchor::broken(p(0.0, 0.0), p(10.0, 0.0), Some(keep));
        assert_eq!(
            anchor.handle_in,
            Some(keep),
            "the untouched handle is untouched"
        );
        assert_eq!(anchor.handle_out, Some(p(10.0, 0.0)));
        assert!(
            !anchor.is_smooth(1e-9),
            "independent handles are not smooth"
        );
    }

    #[test]
    fn the_curve_at_t_zero_and_one_is_the_endpoint() {
        let (a, b, c, d) = (p(0.0, 0.0), p(10.0, 20.0), p(30.0, -20.0), p(40.0, 0.0));
        assert_eq!(cubic_at(a, b, c, d, 0.0), a);
        assert_eq!(cubic_at(a, b, c, d, 1.0), d);
        // …and clamping keeps that promise for overshooting callers.
        assert_eq!(cubic_at_clamped(a, b, c, d, 1.7), d);
        assert_eq!(cubic_at_clamped(a, b, c, d, -3.0), a);
    }

    #[test]
    fn a_straight_cubic_has_no_deviation_from_its_chord() {
        // Control points on the chord: the curve *is* the chord.
        let deviation =
            cubic_deviation_from_chord(p(0.0, 0.0), p(10.0, 0.0), p(20.0, 0.0), p(30.0, 0.0), 8);
        assert!(deviation < 1e-9, "deviation was {deviation}");
    }

    #[test]
    fn a_pulled_cubic_deviates_by_a_known_amount() {
        // Handles pulled 30 units off the chord: the curve must leave it.
        let deviation =
            cubic_deviation_from_chord(p(0.0, 0.0), p(0.0, 30.0), p(30.0, 30.0), p(30.0, 0.0), 16);
        assert!(deviation > 10.0, "deviation was only {deviation}");
    }

    #[test]
    fn distance_to_a_segment_clamps_at_its_ends() {
        let a = p(0.0, 0.0);
        let b = p(10.0, 0.0);
        assert!((distance_to_segment(p(5.0, 3.0), a, b) - 3.0).abs() < 1e-12);
        // Beyond an endpoint the distance is to the endpoint, not the line.
        assert!((distance_to_segment(p(-4.0, 0.0), a, b) - 4.0).abs() < 1e-12);
        assert!((distance_to_segment(p(14.0, 0.0), a, b) - 4.0).abs() < 1e-12);
    }

    #[test]
    fn a_tiny_drag_is_not_a_drag() {
        assert!(!is_drag(p(0.5, 0.5)));
        assert!(!is_drag(p(0.0, 0.0)));
        assert!(is_drag(p(DRAG_THRESHOLD + 1.0, 0.0)));
    }
}
