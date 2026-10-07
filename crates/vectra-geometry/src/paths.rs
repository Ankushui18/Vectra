//! lyon path construction (Task 1.3 §3).
//!
//! [`build_path`] is deliberately pure: it takes **already-resolved** points
//! and chains them through [`lyon::path::Builder`]. All fallibility
//! (unresolvable parameters, non-finite values) is handled upstream by the
//! [`crate::evaluator`], which only calls this once every point is verified
//! [`crate::evaluator::is_renderable`]. The renderer therefore receives paths
//! whose every coordinate survives the `f64 → f32` conversion finite.

use crate::angles::TAU;
use lyon::path::Path;
use std::f64::consts::{FRAC_PI_2, PI};
use vectra_core::Point2;

/// A path segment with concrete, renderable endpoints.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ResolvedSegment {
    Line {
        to: Point2,
    },
    Quadratic {
        control: Point2,
        to: Point2,
    },
    Cubic {
        control1: Point2,
        control2: Point2,
        to: Point2,
    },
    Close,
}

/// Build a [`lyon::path::Path`] from **region rings** (Task 7.0).
///
/// `rings[0]` is the exterior; every later ring is a hole. Unlike `geo`'s
/// convention, the rings are given *without* a repeated closing vertex — the
/// procedural model stores a ring as a cycle, not as a list whose first and last
/// entries coincide — and each ring is closed here.
///
/// Holes are emitted reversed, which is what
/// `vectra_operations::convert::multi_polygon_to_path` does with `geo`
/// interiors. The renderer's tessellator fills with `FillRule::EvenOdd`, so a
/// hole is a hole regardless of winding; emitting the established convention
/// anyway keeps one path shape in the workspace rather than two.
///
/// Rings with fewer than three vertices are skipped: a degenerate ring encloses
/// no area and would only add zero-length edges.
pub fn rings_to_path(rings: &[Vec<Point2>]) -> Path {
    let mut builder = Path::builder();
    for (index, ring) in rings.iter().enumerate() {
        add_ring(&mut builder, ring, index > 0);
    }
    builder.build()
}

/// Build a path from a loose vertex list (`closed` ⇒ a ring, else a polyline).
pub fn polyline_to_path(points: &[Point2], closed: bool) -> Path {
    let mut builder = Path::builder();
    if closed {
        add_ring(&mut builder, points, false);
    } else if let Some(first) = points.first() {
        let _ = builder.begin(to_lyon_point(*first));
        for point in &points[1..] {
            builder.line_to(to_lyon_point(*point));
        }
        builder.end(false);
    } else {
        // An empty open path is representable (Begin + End with no draw), which
        // the caller may legitimately hit for an empty point list.
        let _ = builder.begin(lyon::math::point(0.0, 0.0));
        builder.end(false);
    }
    builder.build()
}

/// Append one closed ring to `builder`, dropping a repeated closing vertex.
fn add_ring(builder: &mut lyon::path::Builder, ring: &[Point2], reverse: bool) {
    let mut points: Vec<Point2> = ring.to_vec();
    if points.len() > 1 && points[0] == points[points.len() - 1] {
        points.pop();
    }
    if reverse {
        points.reverse();
    }
    if points.len() < 3 {
        return;
    }
    let _ = builder.begin(to_lyon_point(points[0]));
    for point in &points[1..] {
        builder.line_to(to_lyon_point(*point));
    }
    builder.close();
}

/// Flatten a resolved primitive into **rings** of document-space points
/// (Task 7.0: the procedural graph reads authored shapes as rings).
///
/// One entry per subpath; a closed subpath does **not** repeat its first vertex
/// (the ring model's convention). Curves are approximated with `tolerance`
/// document units, so this is the same picture the renderer draws when it
/// tessellates the very same primitive — the graph sees what the user sees,
/// not a second approximation of it.
pub fn primitive_to_rings(
    primitive: &crate::scene::EvaluatedPrimitive,
    tolerance: f32,
) -> Vec<Vec<Point2>> {
    path_to_rings(&primitive_to_path(primitive), tolerance)
}

/// Flatten a path into rings (see [`primitive_to_rings`]).
pub fn path_to_rings(path: &Path, tolerance: f32) -> Vec<Vec<Point2>> {
    use lyon::path::iterator::PathIterator;
    let mut rings: Vec<Vec<Point2>> = Vec::new();
    let mut current: Vec<Point2> = Vec::new();
    for event in path.iter().flattened(tolerance) {
        match event {
            lyon::path::PathEvent::Begin { at } => {
                flush_ring(&mut rings, &mut current);
                current.push(Point2::new(at.x as f64, at.y as f64));
            }
            lyon::path::PathEvent::Line { to, .. } => {
                current.push(Point2::new(to.x as f64, to.y as f64));
            }
            // `End`'s `last` was already emitted by the preceding `Line`, and a
            // closed subpath's return segment is implied by the ring model.
            lyon::path::PathEvent::End { .. } => {
                flush_ring(&mut rings, &mut current);
            }
            _ => {}
        }
    }
    flush_ring(&mut rings, &mut current);
    rings
}

fn flush_ring(rings: &mut Vec<Vec<Point2>>, current: &mut Vec<Point2>) {
    if current.is_empty() {
        return;
    }
    // Consecutive duplicates (a repeated vertex, a zero-length segment) carry no
    // information and would show up as degenerate edges downstream.
    let mut cleaned: Vec<Point2> = Vec::with_capacity(current.len());
    for point in current.iter() {
        if cleaned.last() != Some(point) {
            cleaned.push(*point);
        }
    }
    // A ring given as a closed cycle (`first == last`) keeps one copy of it.
    if cleaned.len() > 1 && cleaned[0] == cleaned[cleaned.len() - 1] {
        cleaned.pop();
    }
    if cleaned.len() >= 2 {
        rings.push(cleaned);
    }
    current.clear();
}

/// Build a [`lyon::path::Path`] from a resolved start point + segments.
///
/// * `start` becomes the initial `Begin` (the `MoveTo`).
/// * Segments map 1:1 to `line_to` / `quadratic_bezier_to` /
///   `cubic_bezier_to` / `close`.
/// * An empty segment list yields a valid open path (`Begin` + `End`, a move
///   with no draw — representable and iterable, tessellates to nothing).
/// * A trailing open subpath is explicitly ended: lyon's `build()` requires
///   `end()`/`close()` first and panics otherwise.
/// * Subpath state is tracked explicitly because lyon does **not** auto-begin
///   after `close()`: a draw following a `Close` re-begins at the close point
///   (the closed subpath's start), and a `Close` with no open subpath is
///   skipped as a geometric no-op. Arbitrary segment streams are valid input.
pub fn build_path(start: Point2, segments: &[ResolvedSegment]) -> Path {
    debug_assert!(
        crate::evaluator::is_renderable(start.x) && crate::evaluator::is_renderable(start.y),
        "build_path requires renderable points; resolve upstream"
    );
    let mut builder = Path::builder();
    let start_pt = to_lyon_point(start);
    let _ = builder.begin(start_pt);
    let mut subpath_open = true;
    let mut subpath_start = start_pt;
    let mut current = start_pt;
    for segment in segments {
        match *segment {
            ResolvedSegment::Line { to } => {
                let to = to_lyon_point(to);
                if !subpath_open {
                    let _ = builder.begin(current);
                    subpath_start = current;
                    subpath_open = true;
                }
                let _ = builder.line_to(to);
                current = to;
            }
            ResolvedSegment::Quadratic { control, to } => {
                let (control, to) = (to_lyon_point(control), to_lyon_point(to));
                if !subpath_open {
                    let _ = builder.begin(current);
                    subpath_start = current;
                    subpath_open = true;
                }
                let _ = builder.quadratic_bezier_to(control, to);
                current = to;
            }
            ResolvedSegment::Cubic {
                control1,
                control2,
                to,
            } => {
                let (c1, c2, to) = (
                    to_lyon_point(control1),
                    to_lyon_point(control2),
                    to_lyon_point(to),
                );
                if !subpath_open {
                    let _ = builder.begin(current);
                    subpath_start = current;
                    subpath_open = true;
                }
                let _ = builder.cubic_bezier_to(c1, c2, to);
                current = to;
            }
            ResolvedSegment::Close => {
                if subpath_open {
                    builder.close();
                    subpath_open = false;
                    current = subpath_start;
                }
            }
        }
    }
    if subpath_open {
        builder.end(false);
    }
    builder.build()
}

/// The bounding box of a built path: `(min_x, min_y, max_x, max_y)` in document
/// units.
///
/// Measured with lyon's own iterator rather than from a cached box, so it is
/// exact for whatever the path holds — including the multi-subpath outline a
/// text run produces. An empty path yields the degenerate box `(0, 0, 0, 0)`,
/// which is the box a caller drawing "nothing" should get.
pub fn path_bounds(path: &Path) -> (f64, f64, f64, f64) {
    use lyon::path::iterator::PathIterator;
    let mut min = (f64::INFINITY, f64::INFINITY);
    let mut max = (f64::NEG_INFINITY, f64::NEG_INFINITY);
    for event in path.iter().flattened(0.05) {
        let point = match event {
            lyon::path::PathEvent::Begin { at } => at,
            lyon::path::PathEvent::Line { to, .. } => to,
            lyon::path::PathEvent::End { last, .. } => last,
            _ => continue,
        };
        min.0 = min.0.min(point.x as f64);
        min.1 = min.1.min(point.y as f64);
        max.0 = max.0.max(point.x as f64);
        max.1 = max.1.max(point.y as f64);
    }
    if !min.0.is_finite() || !min.1.is_finite() {
        return (0.0, 0.0, 0.0, 0.0);
    }
    (min.0, min.1, max.0, max.1)
}

#[inline]
fn to_lyon_point(p: Point2) -> lyon::math::Point {
    // Safe: callers guarantee `is_renderable`, so `as f32` stays finite.
    lyon::math::point(p.x as f32, p.y as f32)
}

/// Build a [`lyon::path::Path`] from an **evaluated** primitive (Task 4.0).
///
/// This is the region view of a drawable: what the renderer fills, expressed as
/// a path the operations layer can convert to a polygon. It exists because
/// operations take *geometry in*, not `NodeId`s — an offset or a boolean reads
/// the shape the user sees, whichever primitive kind produced it:
///
/// * `Rect` — four `line_to`s (rounded corners stay exact: each corner is a
///   quarter-circle sampled at the same tolerance policy as arcs). A zero-radius
///   rectangle is four exact corners.
/// * `Circle` — a closed path sampled from the arc, starting at angle 0.
/// * `Arc` — the swept wedge, closed back to the centre so it has an interior
///   (this matches the fill rule the preview already renders arcs with).
/// * `Path` — returned as-is; it is already a built path.
///
/// Sampling is deterministic (`ARC_SEGMENTS_PER_TAU` segments per full turn, at
/// least one per sweep), so the same scene always produces byte-identical
/// geometry — a property the operations proptests rely on.
pub fn primitive_to_path(primitive: &crate::scene::EvaluatedPrimitive) -> Path {
    use crate::scene::EvaluatedPrimitive as P;
    match primitive {
        P::Path(path) => path.clone(),
        // A run *is* a path once it is laid out: the glyph contours were built
        // in document space by the shaping pass (`crate::text`), so every
        // consumer of this function — the renderer's flattener, the region
        // engine, the exporters — gets a run's letterforms with no special case
        // of its own. That is what makes RULE 4 hold: the renderer tessellates
        // text by tessellating a path.
        P::Text(text) => text.outline.clone(),
        P::Rect {
            x,
            y,
            w,
            h,
            corner_radius,
        } => {
            let mut builder = Path::builder();
            let r = corner_radius.clamp(0.0, 0.5 * w.min(*h));
            let (x0, y0, x1, y1) = (*x, *y, x + w, y + h);
            if r <= 0.0 {
                let _ = builder.begin(lyon::math::point(x0 as f32, y0 as f32));
                builder.line_to(lyon::math::point(x1 as f32, y0 as f32));
                builder.line_to(lyon::math::point(x1 as f32, y1 as f32));
                builder.line_to(lyon::math::point(x0 as f32, y1 as f32));
                builder.close();
            } else {
                // Counter-clockwise from the bottom-left corner's tangent point.
                let _ = builder.begin(lyon::math::point((x0 + r) as f32, y0 as f32));
                builder.line_to(lyon::math::point((x1 - r) as f32, y0 as f32));
                for p in sample_arc(x1 - r, y0 + r, r, -FRAC_PI_2, 0.0) {
                    builder.line_to(p);
                }
                builder.line_to(lyon::math::point(x1 as f32, (y1 - r) as f32));
                for p in sample_arc(x1 - r, y1 - r, r, 0.0, FRAC_PI_2) {
                    builder.line_to(p);
                }
                builder.line_to(lyon::math::point((x0 + r) as f32, y1 as f32));
                for p in sample_arc(x0 + r, y1 - r, r, FRAC_PI_2, PI) {
                    builder.line_to(p);
                }
                builder.line_to(lyon::math::point(x0 as f32, (y0 + r) as f32));
                for p in sample_arc(x0 + r, y0 + r, r, PI, 3.0 * FRAC_PI_2) {
                    builder.line_to(p);
                }
                builder.close();
            }
            builder.build()
        }
        P::Circle { cx, cy, r } => {
            let mut builder = Path::builder();
            let steps = arc_steps(TAU, *r);
            let _ = builder.begin(lyon::math::point((cx + r) as f32, *cy as f32));
            for step in 1..steps {
                let angle = TAU * (step as f64) / (steps as f64);
                builder.line_to(lyon::math::point(
                    (cx + r * angle.cos()) as f32,
                    (cy + r * angle.sin()) as f32,
                ));
            }
            builder.close();
            builder.build()
        }
        P::Arc {
            cx,
            cy,
            r,
            start_angle,
            end_angle,
        } => {
            let sweep = end_angle - start_angle;
            let mut builder = Path::builder();
            let steps = arc_steps(sweep, *r);
            let _ = builder.begin(lyon::math::point(*cx as f32, *cy as f32));
            for step in 0..=steps {
                let angle = start_angle + sweep * (step as f64) / (steps as f64);
                builder.line_to(lyon::math::point(
                    (cx + r * angle.cos()) as f32,
                    (cy + r * angle.sin()) as f32,
                ));
            }
            builder.close();
            builder.build()
        }
    }
}

/// **The shape view of a primitive**: the same geometry as
/// [`primitive_to_path`], but with the analytic curves left *as curves*.
///
/// The two functions answer different questions, and the difference matters
/// exactly once — for text on a path (Task 11.0 RULE 2):
///
/// * [`primitive_to_path`] answers *"what polygon does this region fill?"* —
///   the boolean/region/tessellation view, deliberately polygonized
///   ([`ARC_SEGMENTS_PER_TAU`]) so `geo` and lyon get a region they can prove
///   things about.
/// * This one answers *"which way is the curve going?"* — the view a run needs
///   when it samples **tangents**. A 64-gon's edge direction jumps by 5.6° at
///   every vertex no matter how large the circle is, and a run riding one is
///   visibly kinked; the same circle as four cubic arcs has a tangent that is
///   continuous and within ~0.02° of the true one.
///
/// Only the analytic primitives differ; a `Path` and a run's own outline are
/// already curves and are cloned as they are.
pub fn primitive_to_curve_path(primitive: &crate::scene::EvaluatedPrimitive) -> Path {
    use crate::scene::EvaluatedPrimitive as P;
    match primitive {
        // Already curves: a path is the authored path, a run is its glyphs.
        P::Path(_) | P::Text(_) => primitive_to_path(primitive),
        P::Circle { cx, cy, r } => {
            let mut builder = Path::builder();
            let _ = builder.begin(lyon::math::point((cx + r) as f32, *cy as f32));
            append_arc(&mut builder, *cx, *cy, *r, 0.0, TAU);
            builder.close();
            builder.build()
        }
        P::Arc {
            cx,
            cy,
            r,
            start_angle,
            end_angle,
        } => {
            // An arc primitive is a **wedge** (a pie slice): the straight edges
            // to the centre are geometry, not scaffolding, so the curve view
            // keeps them.
            let mut builder = Path::builder();
            let _ = builder.begin(lyon::math::point(*cx as f32, *cy as f32));
            builder.line_to(lyon::math::point(
                (cx + r * start_angle.cos()) as f32,
                (cy + r * start_angle.sin()) as f32,
            ));
            append_arc(&mut builder, *cx, *cy, *r, *start_angle, *end_angle);
            builder.close();
            builder.build()
        }
        P::Rect {
            x,
            y,
            w,
            h,
            corner_radius,
        } => {
            let r = corner_radius.clamp(0.0, 0.5 * w.min(*h));
            let (x0, y0, x1, y1) = (*x, *y, x + w, y + h);
            let mut builder = Path::builder();
            let point = |px: f64, py: f64| lyon::math::point(px as f32, py as f32);
            let _ = builder.begin(point(x0 + r, y0));
            builder.line_to(point(x1 - r, y0));
            append_arc(&mut builder, x1 - r, y0 + r, r, -FRAC_PI_2, 0.0);
            builder.line_to(point(x1, y1 - r));
            append_arc(&mut builder, x1 - r, y1 - r, r, 0.0, FRAC_PI_2);
            builder.line_to(point(x0 + r, y1));
            append_arc(&mut builder, x0 + r, y1 - r, r, FRAC_PI_2, PI);
            builder.line_to(point(x0, y0 + r));
            append_arc(&mut builder, x0 + r, y0 + r, r, PI, 3.0 * FRAC_PI_2);
            builder.close();
            builder.build()
        }
    }
}

/// Sweep an arc as **cubic Béziers** (at most a quarter turn each), with the
/// standard `κ = 4/3 · tan(θ/4)` handle length.
///
/// The joins are tangent-continuous and each piece's maximum radial error is
/// ~0.03% of the radius — for a run that means its tangent comes from the
/// curve's own direction instead of a chord's.
fn append_arc(builder: &mut lyon::path::Builder, cx: f64, cy: f64, r: f64, from: f64, to: f64) {
    let sweep = to - from;
    let pieces = (sweep.abs() / FRAC_PI_2).ceil().max(1.0) as usize;
    let step = sweep / pieces as f64;
    let mut angle = from;
    for _ in 0..pieces {
        let next = angle + step;
        let kappa = 4.0 / 3.0 * (step / 4.0).tan();
        let start = (cx + r * angle.cos(), cy + r * angle.sin());
        let end = (cx + r * next.cos(), cy + r * next.sin());
        // Tangents of the parameterized circle, scaled by κ.
        let c1 = (
            start.0 - kappa * r * angle.sin(),
            start.1 + kappa * r * angle.cos(),
        );
        let c2 = (
            end.0 + kappa * r * next.sin(),
            end.1 - kappa * r * next.cos(),
        );
        builder.cubic_bezier_to(
            lyon::math::point(c1.0 as f32, c1.1 as f32),
            lyon::math::point(c2.0 as f32, c2.1 as f32),
            lyon::math::point(end.0 as f32, end.1 as f32),
        );
        angle = next;
    }
}

/// Segments per full turn for sampled curves. 64 is the same resolution class
/// the arc tessellator uses; it bounds the boolean solver's input size while
/// keeping a full circle's area within ~0.05% of `πr²`.
pub const ARC_SEGMENTS_PER_TAU: usize = 64;

/// Deterministic sample count for a sweep of `sweep` radians on radius `r`:
/// proportional to the swept angle, never fewer than 2 (a chord is not a turn).
fn arc_steps(sweep: f64, _r: f64) -> usize {
    let steps = ((ARC_SEGMENTS_PER_TAU as f64) * (sweep.abs() / TAU)).ceil() as usize;
    steps.max(2)
}

/// The interior points of a sampled arc, exclusive of both ends (a closed
/// region needs chord geometry, and `geo` speaks polygons, not curves).
///
/// Returned as a value rather than pushed into a builder so the call site stays
/// concrete: lyon's ergonomic `line_to` lives on the concrete builder, not on
/// the `PathBuilder` trait.
fn sample_arc(cx: f64, cy: f64, r: f64, from: f64, to: f64) -> Vec<lyon::math::Point> {
    let steps = arc_steps(to - from, r);
    (1..steps)
        .map(|step| {
            let angle = from + (to - from) * (step as f64) / (steps as f64);
            lyon::math::point((cx + r * angle.cos()) as f32, (cy + r * angle.sin()) as f32)
        })
        .collect()
}

/// Serialize a built path to SVG path data (`M…L…Q…C…Z`).
///
/// Pure presentation projection for snapshots and (later) export: coordinates
/// are the path's `f32` values at full precision, in document space (y-up —
/// consumers flip for screen space). Open subpaths emit no trailing verb;
/// closed ones emit `Z`. Multi-subpath paths repeat `M` per subpath.
pub fn path_to_svg_data(path: &Path) -> String {
    use lyon::path::Event;
    use std::fmt::Write as _;
    let mut d = String::new();
    for event in path.iter() {
        match event {
            Event::Begin { at } => {
                let _ = write!(d, "M{} {}", at.x, at.y);
            }
            Event::Line { to, .. } => {
                let _ = write!(d, "L{} {}", to.x, to.y);
            }
            Event::Quadratic { ctrl, to, .. } => {
                let _ = write!(d, "Q{} {} {} {}", ctrl.x, ctrl.y, to.x, to.y);
            }
            Event::Cubic {
                ctrl1, ctrl2, to, ..
            } => {
                let _ = write!(
                    d,
                    "C{} {} {} {} {} {}",
                    ctrl1.x, ctrl1.y, ctrl2.x, ctrl2.y, to.x, to.y
                );
            }
            Event::End { close: true, .. } => d.push('Z'),
            Event::End { close: false, .. } => {}
        }
    }
    d
}

#[cfg(test)]
mod tests {
    use super::*;
    use lyon::path::Event;

    #[test]
    fn empty_segments_yield_open_begin_end() {
        let path = build_path(Point2::new(3.0, 4.0), &[]);
        let events: Vec<_> = path.iter().collect();
        assert_eq!(events.len(), 2);
        assert!(matches!(events[0], Event::Begin { at } if at.x == 3.0 && at.y == 4.0));
        assert!(matches!(events[1], Event::End { close: false, .. }));
    }

    #[test]
    fn draw_after_close_rebegins_at_close_point() {
        // Found by proptest: lyon panics on edge-after-close without an
        // explicit begin, so build_path must re-begin (no auto-begin in lyon).
        let path = build_path(
            Point2::new(0.0, 0.0),
            &[
                ResolvedSegment::Line {
                    to: Point2::new(10.0, 0.0),
                },
                ResolvedSegment::Close,
                ResolvedSegment::Line {
                    to: Point2::new(5.0, 5.0),
                },
            ],
        );
        let events: Vec<_> = path.iter().collect();
        // Begin, Line, End(close), Begin(at close point = subpath start),
        // Line, End(open).
        assert_eq!(events.len(), 6);
        assert!(matches!(events[2], Event::End { close: true, .. }));
        assert!(
            matches!(events[3], Event::Begin { at } if at.x == 0.0 && at.y == 0.0),
            "re-begin must resume at the closed subpath's start, got {:?}",
            events[3]
        );
    }

    #[test]
    fn svg_data_covers_all_verbs() {
        let path = build_path(
            Point2::new(0.0, 0.0),
            &[
                ResolvedSegment::Line {
                    to: Point2::new(10.0, 0.0),
                },
                ResolvedSegment::Quadratic {
                    control: Point2::new(15.0, 0.0),
                    to: Point2::new(15.0, 10.0),
                },
                ResolvedSegment::Cubic {
                    control1: Point2::new(15.0, 15.0),
                    control2: Point2::new(10.0, 15.0),
                    to: Point2::new(10.0, 10.0),
                },
            ],
        );
        assert_eq!(
            path_to_svg_data(&path),
            "M0 0L10 0Q15 0 15 10C15 15 10 15 10 10"
        );
    }

    #[test]
    fn svg_data_closes_and_rebegins() {
        let path = build_path(
            Point2::new(0.0, 0.0),
            &[
                ResolvedSegment::Line {
                    to: Point2::new(10.0, 0.0),
                },
                ResolvedSegment::Close,
                ResolvedSegment::Line {
                    to: Point2::new(5.0, 5.0),
                },
            ],
        );
        // Closed subpath emits Z; post-close draw re-begins with M.
        assert_eq!(path_to_svg_data(&path), "M0 0L10 0ZM0 0L5 5");
    }

    #[test]
    fn open_trailing_subpath_is_ended_not_panicking() {
        // `build()` before `end()` panics in lyon — build_path must prevent that.
        let path = build_path(
            Point2::new(0.0, 0.0),
            &[ResolvedSegment::Line {
                to: Point2::new(1.0, 1.0),
            }],
        );
        let events: Vec<_> = path.iter().collect();
        assert_eq!(events.len(), 3);
        assert!(matches!(events[2], Event::End { close: false, .. }));
    }

    #[test]
    fn segments_chain_with_continuity() {
        let path = build_path(
            Point2::new(0.0, 0.0),
            &[
                ResolvedSegment::Line {
                    to: Point2::new(10.0, 0.0),
                },
                ResolvedSegment::Quadratic {
                    control: Point2::new(15.0, 0.0),
                    to: Point2::new(15.0, 10.0),
                },
                ResolvedSegment::Close,
            ],
        );
        let events: Vec<_> = path.iter().collect();
        // Begin + Line + Quadratic + End(close).
        assert_eq!(events.len(), 4);
        assert!(matches!(events[0], Event::Begin { .. }));
        assert!(matches!(events[1], Event::Line { .. }));
        assert!(matches!(events[2], Event::Quadratic { .. }));
        assert!(matches!(events[3], Event::End { close: true, .. }));
    }
}
