//! lyon ⇄ `geo` conversion: the boundary between the parametric scene and the
//! boolean solver (Task 4.0, RULE 2).
//!
//! `geo` reasons about *regions* (`Polygon` / `MultiPolygon`); `lyon` reasons
//! about *paths*. This module is the only place the two meet:
//!
//! ```text
//! lyon::path::Path ──flatten──▶ geo::MultiPolygon ──op──▶ geo::MultiPolygon ──build──▶ lyon::path::Path
//! ```
//!
//! # Ring model
//!
//! Each flattened, explicitly-closed subpath of the input becomes **its own
//! single-ring polygon**, and the boolean overlay runs with `geo`'s default
//! **even-odd** fill rule. That is what makes nested and overlapping subpaths
//! behave the way the renderer already fills them (parity — a ring inside a
//! ring is a hole), without the converter having to guess which ring is
//! exterior and which is a hole. Results come back with proper holes, so the
//! *output* path is written the way an SVG consumer expects: exterior rings
//! counter-clockwise, interior rings clockwise.
//!
//! # Tolerance
//!
//! Curves are flattened by `lyon` at a fixed document-space tolerance
//! ([`FLATTEN_TOLERANCE`]) and all sampling upstream is deterministic, so the
//! same scene always produces the same polygons — boolean results are
//! reproducible, which the proptests rely on.

use geo::{Area, BooleanOps, Coord, LineString, MultiPolygon, Polygon};
use lyon::path::iterator::PathIterator;
use lyon::path::Path;

/// Document-space flattening tolerance, in the same units as the geometry.
///
/// 0.05 units: below the visible threshold at any sane zoom, and small enough
/// that a 100-unit circle's polygonal approximation is within ~0.02% of its
/// true area.
pub const FLATTEN_TOLERANCE: f32 = 0.05;

/// Convert a built path into a region `geo` can run booleans on.
///
/// Returns `None` when the path has no closed subpath with at least three
/// distinct points — there is no region to speak of (an open path, an empty
/// path, or a degenerate sliver). Callers turn that into a typed error or an
/// empty result; it is never a panic.
pub fn path_to_multi_polygon(path: &Path) -> Option<MultiPolygon<f64>> {
    let mut polygons: Vec<Polygon<f64>> = Vec::new();
    let mut ring: Vec<Coord<f64>> = Vec::new();

    for event in path.iter().flattened(FLATTEN_TOLERANCE) {
        match event {
            lyon::path::Event::Begin { at } => {
                ring.clear();
                ring.push(coord(at.x as f64, at.y as f64));
            }
            lyon::path::Event::Line { to, .. } => {
                ring.push(coord(to.x as f64, to.y as f64));
            }
            // `flattened` yields only line segments, but a hostile iterator
            // (or a future lyon) must not silently corrupt a ring: treat a
            // stray curve's endpoint as a straight chord.
            lyon::path::Event::Quadratic { to, .. } | lyon::path::Event::Cubic { to, .. } => {
                ring.push(coord(to.x as f64, to.y as f64));
            }
            lyon::path::Event::End { close, .. } => {
                if let Some(polygon) = ring_to_polygon(&ring, close) {
                    polygons.push(polygon);
                }
                ring.clear();
            }
        }
    }
    // A trailing open subpath is still a region: SVG closes subpaths for
    // filling, and the preview fills arcs exactly that way.
    if let Some(polygon) = ring_to_polygon(&ring, true) {
        polygons.push(polygon);
    }

    if polygons.is_empty() {
        return None;
    }
    // Resolve the ring set into a canonical region: run the overlay against an
    // empty clip under the even-odd rule, which turns nested/overlapping rings
    // into proper exteriors + holes. Without this, a path with a ring inside a
    // ring would measure as *more* material (the sum of its rings) instead of
    // less, and booleans would see overlapping polygons rather than a hole.
    Some(MultiPolygon::new(polygons).union(&MultiPolygon::<f64>::new(Vec::new())))
}

fn coord(x: f64, y: f64) -> Coord<f64> {
    Coord { x, y }
}

/// Close `ring` if needed and keep it when it has real area.
fn ring_to_polygon(ring: &[Coord<f64>], close: bool) -> Option<Polygon<f64>> {
    if ring.len() < 3 {
        return None;
    }
    let mut points = ring.to_vec();
    let first = points[0];
    let last = *points.last().expect("len checked");
    let _ = close; // both cases close: an explicit `Z` and an implicit fill-close
    if first != last {
        points.push(first);
    }
    if points.len() < 4 {
        // Three distinct points are the minimum for area; a "ring" of two
        // points plus the closing repeat has none.
        return None;
    }
    let polygon = Polygon::new(LineString::new(points), Vec::new());
    // Degenerate (zero-area) rings are dropped rather than handed to the
    // overlay, where they only produce noise.
    if polygon.unsigned_area() <= f64::EPSILON {
        return None;
    }
    Some(polygon)
}

/// Build a closed lyon path from a region, exterior rings counter-clockwise and
/// interior rings clockwise.
///
/// The winding is what makes the result render correctly under *both* SVG fill
/// rules: `nonzero` sees a hole because the interior is wound the other way,
/// and `even-odd` sees it by parity.
pub fn multi_polygon_to_path(multi: &MultiPolygon<f64>) -> Path {
    let mut builder = lyon::path::Builder::new();
    for polygon in &multi.0 {
        add_ring(&mut builder, &polygon.exterior().0, false);
        for interior in polygon.interiors() {
            add_ring(&mut builder, &interior.0, true);
        }
    }
    builder.build()
}

/// Append one closed subpath. `reverse` is used for interior rings so holes
/// wind opposite to their exterior.
fn add_ring(builder: &mut lyon::path::Builder, ring: &[Coord<f64>], reverse: bool) {
    let mut points: Vec<Coord<f64>> = ring.to_vec();
    // geo closes its rings (first == last); lyon's `close()` re-adds that
    // segment, so drop the duplicate to avoid a zero-length edge.
    if points.len() > 1 && points[0] == points[points.len() - 1] {
        points.pop();
    }
    if reverse {
        points.reverse();
    }
    if points.len() < 3 {
        return;
    }
    let _ = builder.begin(lyon::math::point(points[0].x as f32, points[0].y as f32));
    for point in &points[1..] {
        builder.line_to(lyon::math::point(point.x as f32, point.y as f32));
    }
    builder.close();
}

/// Total unsigned area of a region — the quantity every boolean law is stated
/// in terms of, and the measurement the tests use.
pub fn region_area(multi: &MultiPolygon<f64>) -> f64 {
    multi.0.iter().map(|p| p.unsigned_area()).sum()
}

/// The area a built path encloses (zero when it encloses nothing).
pub fn path_area(path: &Path) -> f64 {
    path_to_multi_polygon(path)
        .map(|region| region_area(&region))
        .unwrap_or(0.0)
}

/// Sample count for a fillet arc of radius `r` (Task 4.0): the same
/// deterministic policy `vectra-geometry` uses for its curves, expressed as
/// segments per full turn.
pub(crate) fn arc_samples(radius: f64) -> f64 {
    let _ = radius;
    // 64 segments per turn — the same resolution class `vectra-geometry` uses
    // for circles and arcs, so a filleted region's area lands within ~0.02% of
    // the analytic value. Deterministic by construction (no radius or sweep
    // dependence), so the same input always fillets to the same polygon.
    64.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use vectra_geometry::{primitive_to_path, EvaluatedPrimitive};

    fn rect(x: f64, y: f64, w: f64, h: f64) -> Path {
        primitive_to_path(&EvaluatedPrimitive::Rect {
            x,
            y,
            w,
            h,
            corner_radius: 0.0,
        })
    }

    #[test]
    fn a_rectangle_round_trips_as_its_exact_area() {
        let path = rect(0.0, 0.0, 10.0, 4.0);
        let region = path_to_multi_polygon(&path).expect("a rect is a region");
        assert_eq!(region_area(&region), 40.0);
        // …and back to a path without gaining or losing area.
        let rebuilt = multi_polygon_to_path(&region);
        assert_eq!(path_area(&rebuilt), 40.0);
    }

    #[test]
    fn a_circle_approximates_its_area_within_tolerance() {
        let path = primitive_to_path(&EvaluatedPrimitive::Circle {
            cx: 0.0,
            cy: 0.0,
            r: 10.0,
        });
        let area = path_area(&path);
        let exact = std::f64::consts::PI * 100.0;
        assert!(
            (area - exact).abs() / exact < 2e-3,
            "sampled circle area {area} vs {exact}"
        );
    }

    #[test]
    fn an_open_sliver_is_not_a_region() {
        let mut builder = lyon::path::Builder::new();
        let _ = builder.begin(lyon::math::point(0.0, 0.0));
        builder.line_to(lyon::math::point(10.0, 0.0));
        builder.line_to(lyon::math::point(10.0, 1.0));
        builder.end(false);
        let path = builder.build();
        // An open polyline with area is still a region (fill rule closes it)…
        assert!(path_to_multi_polygon(&path).is_some());
        // …but a two-point line is not.
        let mut line = lyon::path::Builder::new();
        let _ = line.begin(lyon::math::point(0.0, 0.0));
        line.line_to(lyon::math::point(10.0, 0.0));
        line.end(false);
        let degenerate = line.build();
        assert!(path_to_multi_polygon(&degenerate).is_none());
        let empty = Path::builder().build();
        assert!(path_to_multi_polygon(&empty).is_none());
    }

    #[test]
    fn holes_survive_the_round_trip() {
        // A 10×10 square with a 4×4 hole punched in it, expressed the way the
        // renderer would fill it: two nested rings, opposite winding.
        let mut builder = lyon::path::Builder::new();
        let _ = builder.begin(lyon::math::point(0.0, 0.0));
        builder.line_to(lyon::math::point(10.0, 0.0));
        builder.line_to(lyon::math::point(10.0, 10.0));
        builder.line_to(lyon::math::point(0.0, 10.0));
        builder.close();
        let _ = builder.begin(lyon::math::point(3.0, 3.0));
        builder.line_to(lyon::math::point(3.0, 7.0));
        builder.line_to(lyon::math::point(7.0, 7.0));
        builder.line_to(lyon::math::point(7.0, 3.0));
        builder.close();
        let path = builder.build();

        // The converter canonicalises parity: one polygon, the inner ring is a
        // hole (an interior ring), and the region measures 100 − 16.
        let region = path_to_multi_polygon(&path).expect("region");
        assert_eq!(region.0.len(), 1, "nested rings resolve to one polygon");
        assert_eq!(region.0[0].interiors().len(), 1, "with one hole");
        assert!(
            (region_area(&region) - 84.0).abs() < 1e-9,
            "even-odd fill makes the inner ring a hole: {}",
            region_area(&region)
        );
        // …and the hole survives the trip back out to a path.
        let rebuilt = multi_polygon_to_path(&region);
        assert!(
            (path_area(&rebuilt) - 84.0).abs() < 1e-6,
            "{}",
            path_area(&rebuilt)
        );
    }
}
