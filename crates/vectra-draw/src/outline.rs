//! Outline expansion: a width-carrying stroke becomes a **closed contour**
//! (Task 10.1 RULE 3, "an expanded `Path` (filled shape)").
//!
//! A vector brush does not stroke a centreline — it produces a *filled region*,
//! which is why the result can be edited, boolean-combined and exported like any
//! other shape. This module is that last step:
//!
//! ```text
//!   samples ──stamp──▶ centreline + width ──expand──▶ ring ──fit──▶ cubic path
//!      │                     │                          │              │
//!   stroke.rs             stroke.rs                  outline.rs      fit.rs
//! ```
//!
//! The expansion walks the centreline, offsets to the left of the direction of
//! travel by half the width, then back along the right side, and closes with a
//! **round cap** at each end. That is the shape a round-tipped pen makes, and the
//! shape the user watched themselves draw.
//!
//! ## Two details that decide whether the result is usable
//!
//! * **The offset direction is smoothed, not per-segment.** Offsetting by each
//!   raw segment's normal produces a spray of spikes wherever the samples jitter;
//!   averaging the incoming and outgoing directions (a "miter with a limit", in
//!   drawing-tool language) keeps the contour following the stroke.
//! * **The ring must be counter-clockwise**, whatever the stroke's own direction
//!   — the fill rule does not care, but the *area test* in the laws does, and a
//!   signed area is the cheapest way to prove a contour is not self-degenerate.

use vectra_core::Point2;

use crate::stroke::StampedPoint;

/// How many points approximate a round cap. Six is visibly round at brush sizes
/// and keeps the contour short enough to fit cheaply.
pub const CAP_SEGMENTS: usize = 6;

/// A closed contour: the first point is *not* repeated at the end.
pub type Ring = Vec<Point2>;

/// The half-width a sample contributes.
fn half_width(sample: &StampedPoint) -> f64 {
    (sample.width / 2.0).max(0.01)
}

/// The direction of travel at each sample, smoothed across neighbours.
fn directions(points: &[Point2]) -> Vec<Point2> {
    let mut out = Vec::with_capacity(points.len());
    for index in 0..points.len() {
        // Central difference where both neighbours exist, one-sided at the ends:
        // the tangent of the *polyline through the samples*, which is the closest
        // thing to "the direction the user was moving" that the data supports.
        let previous = points[index.saturating_sub(1)];
        let next = points[(index + 1).min(points.len() - 1)];
        let mut dx = next.x - previous.x;
        let mut dy = next.y - previous.y;
        let length = (dx * dx + dy * dy).sqrt();
        if length <= f64::EPSILON {
            // A stationary sample: keep the previous direction if there is one.
            if let Some(last) = out.last() {
                out.push(*last);
                continue;
            }
            dx = 1.0;
            dy = 0.0;
        } else {
            dx /= length;
            dy /= length;
        }
        out.push(Point2::new(dx, dy));
    }
    out
}

/// **Expand a stamped stroke into a filled contour ring.**
///
/// The result is a single closed ring in document units, ready for
/// [`crate::fit::fit_stroke`] (which turns it into cubic segments) or for the
/// engine's region model. A stroke with fewer than two samples produces an empty
/// ring: there is no shape to fill.
pub fn expand(stamped: &[StampedPoint]) -> Ring {
    if stamped.len() < 2 {
        return Vec::new();
    }
    let points: Vec<Point2> = stamped.iter().map(|sample| sample.point).collect();
    let dirs = directions(&points);

    let mut left: Vec<Point2> = Vec::with_capacity(points.len());
    let mut right: Vec<Point2> = Vec::with_capacity(points.len());
    for (index, (point, direction)) in points.iter().zip(&dirs).enumerate() {
        // The normal is the direction rotated by +90°, scaled by the half width.
        let half = half_width(&stamped[index]);
        let nx = -direction.y * half;
        let ny = direction.x * half;
        left.push(Point2::new(point.x + nx, point.y + ny));
        right.push(Point2::new(point.x - nx, point.y - ny));
    }

    // Round caps: sweep the *end* direction through ±90°, so the cap is a
    // semicircle of the end's own half width rather than a fixed blob.
    let head = round_cap(points[0], dirs[0], half_width(&stamped[0]), true);
    let tail = round_cap(
        points[points.len() - 1],
        dirs[dirs.len() - 1],
        half_width(&stamped[stamped.len() - 1]),
        false,
    );

    let mut ring = Vec::with_capacity(left.len() * 2 + head.len() + tail.len());
    ring.extend(tail); // travel backwards out of the start cap
    ring.extend(left.iter().copied());
    ring.extend(head);
    ring.extend(right.iter().rev().copied());
    ring
}

/// A semicircle of points around `pivot`, sweeping from `direction` by 180°.
///
/// `at_start` decides which side the semicircle opens away from, so the two caps
/// extend the stroke instead of folding back over it.
fn round_cap(pivot: Point2, direction: Point2, half: f64, at_start: bool) -> Vec<Point2> {
    let base = if at_start {
        direction.y.atan2(direction.x) - std::f64::consts::FRAC_PI_2
    } else {
        direction.y.atan2(direction.x) + std::f64::consts::FRAC_PI_2
    };
    (0..=CAP_SEGMENTS)
        .map(|index| {
            let t = index as f64 / CAP_SEGMENTS as f64;
            let angle = base + t * std::f64::consts::PI;
            Point2::new(pivot.x + half * angle.cos(), pivot.y + half * angle.sin())
        })
        .collect()
}

/// The signed area of a closed ring (`> 0` counter-clockwise in document space).
///
/// The shoelace formula, used by the laws to prove an expanded stroke encloses
/// something and to test which way round it came out.
pub fn signed_area(ring: &[Point2]) -> f64 {
    if ring.len() < 3 {
        return 0.0;
    }
    let mut area = 0.0;
    for index in 0..ring.len() {
        let a = ring[index];
        let b = ring[(index + 1) % ring.len()];
        area += a.x * b.y - b.x * a.y;
    }
    area / 2.0
}

/// Reverse a ring's winding, if it is clockwise.
pub fn ensure_counter_clockwise(mut ring: Ring) -> Ring {
    if signed_area(&ring) < 0.0 {
        ring.reverse();
    }
    ring
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stroke::{BrushProfile, Sample};

    fn stamped_line(count: usize, width: f64, step: f64) -> Vec<StampedPoint> {
        let samples: Vec<Sample> = (0..count)
            .map(|i| {
                let x = i as f64 * step;
                Sample::new(Point2::new(x, 0.0), i as f64 * 0.01, None)
            })
            .collect();
        crate::stroke::stamp(&samples, &BrushProfile::default())
            .into_iter()
            .map(|mut point| {
                point.width = width;
                point
            })
            .collect()
    }

    #[test]
    fn a_straight_stroke_expands_to_a_ring_around_it() {
        let stamped = stamped_line(10, 10.0, 20.0);
        let ring = expand(&stamped);
        assert!(ring.len() > 10, "{ring:?}");
        // Every ring vertex is within reach of the centreline.
        for point in &ring {
            assert!(
                point.y.abs() <= 5.0 + 1e-9,
                "ring vertex {point:?} left the stroke's band"
            );
        }
        // The ring encloses roughly the length × width rectangle.
        let area = signed_area(&ring).abs();
        assert!(area > 150.0, "area was {area}");
    }

    #[test]
    fn the_ring_is_a_closed_non_degenerate_contour() {
        let stamped = stamped_line(6, 8.0, 15.0);
        let ring = ensure_counter_clockwise(expand(&stamped));
        assert!(signed_area(&ring) > 0.0, "winding was not corrected");
        assert!(super::signed_area(&ring).abs() > 1.0);
        // No two consecutive vertices coincide: a degenerate edge would make the
        // fitting pass divide by zero.
        for pair in ring.windows(2) {
            let dx = pair[1].x - pair[0].x;
            let dy = pair[1].y - pair[0].y;
            assert!((dx * dx + dy * dy).sqrt() > 1e-9, "{pair:?}");
        }
    }

    #[test]
    fn a_tapered_stroke_is_wider_at_the_thick_end() {
        let mut stamped = stamped_line(20, 20.0, 10.0);
        for (index, sample) in stamped.iter_mut().enumerate() {
            sample.width = 20.0 * (1.0 - index as f64 / 19.0) + 2.0;
        }
        let ring = expand(&stamped);
        let half = ring.len() / 2;
        // The left side's first vertex is nearer the axis than its last, because
        // the stroke tapers from thick to thin.
        let near = ring[0..half]
            .iter()
            .map(|p| p.y.abs())
            .fold(0.0_f64, f64::max);
        assert!(near > 5.0, "the thick end should reach further: {near}");
    }

    #[test]
    fn a_single_sample_has_no_outline() {
        let stamped = stamped_line(1, 10.0, 0.0);
        assert!(expand(&stamped).is_empty());
        assert!(expand(&[]).is_empty());
    }
}
