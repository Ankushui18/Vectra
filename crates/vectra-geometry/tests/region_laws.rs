//! **Task 12.0's region laws, on the geometry** (MES §17's idiom: a law holds
//! for *every* shape, not for the one in the fixture).
//!
//! Two of the three named laws live here, stated on the region graph itself:
//!
//! ```text
//!   Region Detection   two overlapping circles  →  exactly three regions
//!   Span Break          breaking a span          →  closed paths, no gaps
//! ```
//!
//! Both are asserted against *independent* arithmetic — `geo`'s own boolean
//! overlay, and the closed-form lens area — so a law cannot be satisfied by the
//! graph agreeing with itself. The parametric half (a boundary move reaching the
//! fill's geometry through the document) needs the engine and the operations
//! pass, and lives in `vectra-operations/tests/smart_fill_laws.rs`.
//!
//! Run: `cargo test -p vectra-geometry --test region_laws`.

use geo::BooleanOps;
use lyon::path::iterator::PathIterator;
use proptest::prelude::*;
use vectra_core::{new_node_id, NodeId};
use vectra_geometry::{
    multi_polygon_to_path, path_area, path_rings, path_to_multi_polygon, primitive_to_path,
    region_area, ring_pieces_between, EvaluatedPrimitive, RegionGraph, SourceSpec,
    FLATTEN_TOLERANCE,
};

// ── fixtures ────────────────────────────────────────────────────────────────

/// A circle as a region: the same outline the renderer fills (through the
/// engine's own flattening tolerance), so the graph's numbers are the picture's
/// numbers and not a second approximation of the same circle.
fn circle_region(cx: f64, cy: f64, r: f64) -> geo::MultiPolygon<f64> {
    let path = primitive_to_path(&EvaluatedPrimitive::Circle { cx, cy, r });
    path_to_multi_polygon(&path).expect("a circle covers area")
}

fn source(cx: f64, cy: f64, r: f64) -> SourceSpec {
    SourceSpec {
        id: new_node_id(),
        region: circle_region(cx, cy, r),
    }
}

/// The analytic area of the lens of two radius-`r` circles `d` apart:
/// `2r²cos⁻¹(d/2r) − (d/2)√(4r²−d²)`.
fn lens_area(r1: f64, r2: f64, d: f64) -> f64 {
    // The lens is two circular segments: the common chord is at `x1` from the
    // first centre and `x2` from the second, and a segment of radius `r` cut at
    // distance `x` measures `r²·acos(x/r) − x·√(r² − x²)`.
    let x1 = (d * d + r1 * r1 - r2 * r2) / (2.0 * d);
    let x2 = d - x1;
    let segment = |r: f64, x: f64| -> f64 {
        let x = x.clamp(-r, r);
        r * r * (x / r).clamp(-1.0, 1.0).acos() - x * (r * r - x * x).max(0.0).sqrt()
    };
    segment(r1, x1) + segment(r2, x2)
}

/// Two circles that overlap *properly*: neither contains the other, neither is
/// disjoint. `t` moves between the two degenerate ends, and the margin keeps the
/// crossing a real, measurable one rather than a tangent.
fn overlapping(r1: f64, r2: f64, t: f64) -> f64 {
    let inner = (r1 - r2).abs();
    let outer = r1 + r2;
    inner + (0.05 + 0.90 * t) * (outer - inner)
}

/// `t` for the *closed-form* comparisons: away from tangency, where a lens is
/// thick enough that the polygon chords' budget is a few tenths of a percent
/// and the closed form is a meaningful yardstick. The overlay cross-check holds
/// across the whole range — it compares the same polygons with the same rule.
fn proper(t: f64) -> f64 {
    0.15 + 0.7 * t
}

/// A ring's total arc length — the splitter's own measure, computed here
/// independently of the graph.
fn ring_length(ring: &[geo::Coord<f64>]) -> f64 {
    (0..ring.len())
        .map(|index| {
            let p = ring[index];
            let q = ring[(index + 1) % ring.len()];
            (q.x - p.x).hypot(q.y - p.y)
        })
        .sum()
}

/// The polygon area of a point list, by the shoelace formula — a *second*
/// implementation, so "the pieces tile the ring" is not `path_area` agreeing
/// with itself.
fn shoelace(points: &[(f64, f64)]) -> f64 {
    let mut twice = 0.0;
    for index in 0..points.len() {
        let p = points[index];
        let q = points[(index + 1) % points.len()];
        twice += p.0 * q.1 - q.0 * p.1;
    }
    twice.abs() / 2.0
}

/// The arc length of a point list, as an **open** polyline.
///
/// A piece is drawn as `M … Z`: the closing segment is the chord the two pieces
/// share, so the ring's arc length is the sum over the drawn segments only —
/// the closing step would count that chord twice and is not part of either arc.
fn arc_length(points: &[(f64, f64)]) -> f64 {
    points
        .windows(2)
        .map(|pair| (pair[1].0 - pair[0].0).hypot(pair[1].1 - pair[0].1))
        .sum()
}

// ── 1. Region Detection ─────────────────────────────────────────────────────

proptest! {
    #![proptest_config(ProptestConfig { cases: 64, max_shrink_iters: 400, ..ProptestConfig::default() })]

    /// **The Region Detection Law.** Two circles that overlap properly create
    /// **exactly three** distinct regions: A only, B only, and the lens A∩B.
    ///
    /// Every area is cross-checked against `geo`'s own overlay over the same two
    /// regions and against the closed form for the lens, so "exactly three" is a
    /// statement about the arrangement rather than about one implementation's
    /// arithmetic.
    #[test]
    fn prop_two_overlapping_circles_make_exactly_three_regions(
        r1 in 20.0f64..100.0,
        r2 in 20.0f64..100.0,
        t in 0.0f64..1.0,
    ) {
        let d = overlapping(r1, r2, proper(t));
        let graph = RegionGraph::build(vec![source(0.0, 0.0, r1), source(d, 0.0, r2)]);

        // 1. **Exactly three faces, one per signature.** The background (outside
        //    both) is explicitly *not* a face.
        prop_assert_eq!(
            graph.faces.len(),
            3,
            "faces: {:?}",
            graph.faces.iter().map(|f| f.members.clone()).collect::<Vec<_>>()
        );
        let only_a = graph.faces.iter().find(|f| f.members == [true, false]).expect("A only");
        let only_b = graph.faces.iter().find(|f| f.members == [false, true]).expect("B only");
        let lens = graph.faces.iter().find(|f| f.members == [true, true]).expect("A ∩ B");
        prop_assert!(graph.faces.iter().all(|f| f.members.iter().any(|inside| *inside)));

        // 2. **The areas are the arrangement's**, twice checked: `geo`'s overlay
        //    over the same regions, and the closed form for the lens.
        let ra = &graph.sources[0].region;
        let rb = &graph.sources[1].region;
        let overlap = region_area(&ra.intersection(rb));
        let union = region_area(&ra.union(rb));
        prop_assert!(
            (lens.area - overlap).abs() <= 1e-6 + 1e-8 * overlap,
            "lens {} vs the overlay's overlap {}",
            lens.area,
            overlap
        );
        // The circles in the scene are the picture's polygons, not the ideal
        // curves: their chords cut the lens's corners, so the graph's lens is
        // *inside* the closed form by the flattening's own budget (a few parts
        // in a thousand) and never outside it.
        let closed_form = lens_area(r1, r2, d);
        prop_assert!(
            (closed_form - lens.area).abs() / closed_form < 2e-2,
            "lens {} vs the closed form {}",
            lens.area,
            closed_form
        );
        prop_assert!(lens.area <= closed_form + 1e-6, "flattening shrinks, never grows");
        prop_assert!(
            (only_a.area + only_b.area + lens.area - union).abs() <= 1e-6 + 1e-8 * union,
            "the faces {} + {} + {} partition the union {}",
            only_a.area,
            only_b.area,
            lens.area,
            union
        );
        // The pieces tile each source: A only + the lens *is* A. Again two
        // boolean runs added up, so the budget is parts in a hundred million
        // rather than a bit-for-bit match.
        let area_a = region_area(ra);
        let area_b = region_area(rb);
        prop_assert!(
            (only_a.area + lens.area - area_a).abs() <= 1e-6 + 1e-8 * area_a,
            "A only {} + lens {} vs A {}",
            only_a.area,
            lens.area,
            area_a
        );
        prop_assert!(
            (only_b.area + lens.area - area_b).abs() <= 1e-6 + 1e-8 * area_b,
            "B only {} + lens {} vs B {}",
            only_b.area,
            lens.area,
            area_b
        );

        // 3. **The drop test.** The midpoint of the lens's common chord — where
        //    the chord meets the line of centres — is inside both circles and
        //    nowhere else. (Not `d/2`: with different radii the midpoint of the
        //    centres can sit outside the smaller circle, which is exactly the
        //    case a drop test has to get right.) A point beyond both centres is
        //    in no face at all.
        let chord = (d * d + r1 * r1 - r2 * r2) / (2.0 * d);
        prop_assert_eq!(
            graph.face_at((chord, 0.0)),
            Some(graph.faces.iter().position(|f| f.members == [true, true]).unwrap())
        );
        prop_assert_eq!(graph.face_at((d / 2.0, r1 + r2)), None);
        prop_assert_eq!(graph.face_at((-r1 - 1.0, 0.0)), None);

        // 4. **The crossings and spans are the outlines'.** Two proper crossings
        //    of two circles, so each ring is cut into two arcs whose lengths add
        //    up to the ring — and the crossings are the same two points seen from
        //    both rings.
        prop_assert_eq!(graph.crossings.len(), 2);
        for id in [graph.sources[0].id, graph.sources[1].id] {
            let spans = graph.spans_of_source(id);
            prop_assert_eq!(spans.len(), 2, "each circle is cut twice");
            let covered: f64 = spans.iter().map(|span| span.length).sum();
            prop_assert!((covered - spans[0].total).abs() < 1e-6, "{covered} vs {}", spans[0].total);
            // One span ends where the other begins: the arcs are consecutive.
            prop_assert!((spans[0].start.0 - spans[1].end.0).abs() < 1e-6);
            prop_assert!((spans[0].start.1 - spans[1].end.1).abs() < 1e-6);
        }
    }

    /// **The Region Detection Law's shape half.** Every face is a *clean closed
    /// path* whose own area is the region's, and the faces partition the union —
    /// "each region must be a clean closed Path, with even-odd winding handled so
    /// holes remain holes", made checkable.
    #[test]
    fn prop_every_face_is_a_closed_path_that_partitions_the_union(
        r1 in 20.0f64..90.0,
        r2 in 20.0f64..90.0,
        t in 0.0f64..1.0,
        dy in -30.0f64..30.0,
    ) {
        let d = overlapping(r1, r2, t);
        let graph = RegionGraph::build(vec![source(0.0, 0.0, r1), source(d, dy, r2)]);
        let union = region_area(&graph.sources[0].region.union(&graph.sources[1].region));
        let mut total = 0.0;
        for face in &graph.faces {
            total += face.area;
            let rings = path_rings(&face.path);
            prop_assert!(!rings.is_empty(), "a face has at least one ring");
            for ring in &rings {
                prop_assert!(ring.len() >= 3, "a ring is a polygon");
            }
            // …and every subpath is *closed*: `path_rings` reports a subpath's
            // points without repeating the first one, and it is the `Close` the
            // fill and the hole-parity both depend on, so read it off the path
            // rather than inferring it from the last point.
            let mut all_closed = true;
            for event in face.path.iter().flattened(FLATTEN_TOLERANCE) {
                if let lyon::path::Event::End { close, .. } = event {
                    all_closed &= close;
                }
            }
            prop_assert!(all_closed, "every subpath of a face closes");
            // Two functions, one area: the path the evaluator publishes and the
            // region the graph measured.
            let from_path = path_area(&face.path);
            // The path is what the renderer draws: `lyon` points are `f32`, so
            // re-entering the region through the path rounds every coordinate.
            // The budget is a relative one — parts in ten million of the area —
            // which is far below a pixel and far above the rounding.
            prop_assert!(
                (from_path - face.area).abs() <= 1e-4 + 1e-6 * face.area,
                "path area {from_path} vs region area {}",
                face.area
            );
        }
        // Parts in a hundred million: the atoms and the union are *different*
        // boolean runs, so the guarantee is that they cut the same plane, not
        // that they round identically.
        prop_assert!(
            (total - union).abs() <= 1e-6 + 1e-8 * union,
            "faces {total} vs union {union}"
        );
    }

    /// **The Region Detection Law, nested.** A source strictly inside another is
    /// its own face, and the outer face has a **hole** where the inner one sits
    /// — the winding rule the rule names, asserted on the rings' orientations.
    #[test]
    fn prop_a_nested_source_is_its_own_face_and_its_host_gets_a_hole(
        outer_r in 60.0f64..100.0,
        inner_frac in 0.1f64..0.6,
        angle in 0.0f64..std::f64::consts::TAU,
    ) {
        let inner_r = outer_r * inner_frac;
        let inset = (outer_r - inner_r) * 0.5;
        let cx = inset * angle.cos();
        let cy = inset * angle.sin();
        let graph = RegionGraph::build(vec![source(0.0, 0.0, outer_r), source(cx, cy, inner_r)]);
        prop_assert_eq!(graph.faces.len(), 2);
        let outer = graph.faces.iter().find(|f| f.members == [true, false]).expect("the frame");
        // The inset is inside the host *and* itself: `[true, false]` would mean
        // "inside the host's index only", which no part of it is.
        let inner = graph.faces.iter().find(|f| f.members == [true, true]).expect("the inset");
        prop_assert_eq!(outer.holes, 1, "the frame has a hole where the inset is");
        prop_assert_eq!(inner.holes, 0);
        // The inset's face is the inset: `outer ∩ inner` over regions that
        // contain one another is the inner region, up to the overlay's own
        // rounding (which is why the budget is relative, not absolute).
        let inset_area = region_area(&circle_region(cx, cy, inner_r));
        prop_assert!(
            (inner.area - inset_area).abs() <= 1e-6 + 1e-8 * inset_area,
            "the inset's face {} vs the inset {}",
            inner.area,
            inset_area
        );

        // A hole is a hole: the frame's path has two rings — the outer one and
        // the inner one — and under the renderer's rule (`EvenOdd`, which is
        // also the rule `path_to_multi_polygon` canonicalises with) the inner
        // one *subtracts*. That is the whole claim: the frame's published path
        // measures as the frame, not as the outer circle.
        let rings = path_rings(&outer.path);
        prop_assert_eq!(rings.len(), 2, "an exterior and a hole");
        let path_area_of_frame = path_area(&outer.path);
        prop_assert!(
            (path_area_of_frame - outer.area).abs() <= 1e-4 + 1e-6 * outer.area,
            "the frame's path is the frame: {path_area_of_frame} vs {}",
            outer.area
        );
        prop_assert!(
            (outer.area - (region_area(&graph.sources[0].region) - inner.area)).abs()
                <= 1e-6 + 1e-8 * outer.area,
            "the frame is the host less the inset: {} vs {} − {}",
            outer.area,
            region_area(&graph.sources[0].region),
            inner.area
        );

        // The drop test answers *inside* the hole with the inner face, not with
        // the frame that surrounds it: the smallest face wins.
        prop_assert_eq!(
            graph.face_at((cx, cy)),
            Some(graph.faces.iter().position(|f| f.members == [true, true]).unwrap())
        );
    }
}

// ── 3. Span Break (the geometry half) ───────────────────────────────────────

proptest! {
    #![proptest_config(ProptestConfig { cases: 64, max_shrink_iters: 400, ..ProptestConfig::default() })]

    /// **The Span Break Law.** Cutting a ring at the two crossings that bound
    /// one of its spans yields **two valid, closed paths with no gaps**: they
    /// share the two cut points, their arc lengths add up to the ring's, and
    /// their areas add up to the ring's area.
    #[test]
    fn prop_breaking_a_span_yields_two_closed_paths_with_no_gaps(
        r1 in 40.0f64..100.0,
        r2 in 40.0f64..100.0,
        t in 0.0f64..1.0,
        span_index in 0usize..2,
    ) {
        let d = overlapping(r1, r2, t);
        let graph = RegionGraph::build(vec![source(0.0, 0.0, r1), source(d, 0.0, r2)]);
        let spans = graph.spans_of_source(graph.sources[0].id);
        prop_assert_eq!(spans.len(), 2);
        let span = spans[span_index];

        // The ring the span lives on: the source's own outline, which is what
        // the span's arc positions were measured along.
        let rings = path_rings(&multi_polygon_to_path(&graph.sources[0].region));
        prop_assert!(!rings.is_empty());
        let (piece_a, piece_b) = ring_pieces_between(&rings[span.ring], span.from, span.to);
        prop_assert!(piece_a.len() >= 2 && piece_b.len() >= 2, "both pieces draw");

        // 1. **The cut points are the crossing points** — each piece starts at
        //    one crossing and ends at the other, so the two meet exactly.
        for (piece, first, last) in [
            (&piece_a, span.start, span.end),
            (&piece_b, span.end, span.start),
        ] {
            let head = piece[0];
            let tail = *piece.last().unwrap();
            prop_assert!(
                (head.0 - first.0).hypot(head.1 - first.1) < 1e-6,
                "piece starts at the cut"
            );
            prop_assert!(
                (tail.0 - last.0).hypot(tail.1 - last.1) < 1e-6,
                "piece ends at the other cut"
            );
        }

        // 2. **No gap and no overlap**: the arcs tile the ring's arc length, and
        //    the first piece *is* the span the graph reported.
        let total = ring_length(&rings[span.ring]);
        prop_assert!((arc_length(&piece_a) + arc_length(&piece_b) - total).abs() < 1e-6);
        prop_assert!((arc_length(&piece_a) - span.length).abs() < 1e-6);

        // 3. **Both halves are regions**: their areas add up to the circle's, by
        //    a second area implementation (the shoelace formula).
        prop_assert!(
            (shoelace(&piece_a) + shoelace(&piece_b) - region_area(&circle_region(0.0, 0.0, r1))).abs()
                < 1e-6,
            "the pieces tile the ring"
        );

        // 4. Cutting the *complementary* pair of cuts gives the other two
        //    pieces: the same ring, split at the same two points, the other way.
        let (other_a, other_b) = ring_pieces_between(&rings[span.ring], span.to, span.from);
        prop_assert!((arc_length(&other_a) - (total - span.length)).abs() < 1e-6);
        prop_assert!((arc_length(&other_b) - span.length).abs() < 1e-6);
    }

    /// **The Span Break Law, unwrapped.** A span that crosses the ring's own
    /// start point (from > to) is still one arc: the splitter walks forward
    /// through the wrap rather than backwards past it. Stated on a square, where
    /// the arc lengths are exact integers.
    #[test]
    fn prop_a_span_that_wraps_the_ring_start_still_tiles_it(
        side in 10.0f64..200.0,
        start in 0.0f64..1.0,
    ) {
        // A square ring, counter-clockwise from the origin.
        let ring = vec![
            geo::Coord { x: 0.0, y: 0.0 },
            geo::Coord { x: side, y: 0.0 },
            geo::Coord { x: side, y: side },
            geo::Coord { x: 0.0, y: side },
            geo::Coord { x: 0.0, y: 0.0 },
        ];
        let perimeter = side * 4.0;
        let from = start * perimeter;
        let to = from + side * 1.5; // one and a half sides forward
        let (a, b) = ring_pieces_between(&ring, from, to);
        prop_assert!((arc_length(&a) - side * 1.5).abs() < 1e-6);
        prop_assert!((arc_length(&b) - (perimeter - side * 1.5)).abs() < 1e-6);
        prop_assert!((shoelace(&a) + shoelace(&b) - side * side).abs() < 1e-6);

        // The wrapped case: the same two cuts named the other way round. The
        // first piece is the complementary arc, and `from + perimeter` is the
        // same point as `from` — a `to` past the ring's end is still an arc.
        let (a2, b2) = ring_pieces_between(&ring, to, from + perimeter);
        prop_assert!((arc_length(&a2) - (perimeter - side * 1.5)).abs() < 1e-6);
        prop_assert!((arc_length(&b2) - side * 1.5).abs() < 1e-6);
        prop_assert!((shoelace(&a2) + shoelace(&b2) - side * side).abs() < 1e-6);
    }
}

/// A `NodeId` is a UUID; the tests only need distinct ones, so the engine's own
/// allocator is used rather than a literal.
#[allow(dead_code)]
fn distinct(n: usize) -> Vec<NodeId> {
    (0..n).map(|_| new_node_id()).collect()
}
