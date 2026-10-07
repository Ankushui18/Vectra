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

use geo::{Area, BooleanOps};
use proptest::prelude::*;
use vectra_core::{new_node_id, NodeId};
use vectra_geometry::{
    path_area, path_to_multi_polygon, path_rings, primitive_to_path, region_area,
    ring_pieces_between, EvaluatedPrimitive, RegionGraph, SourceSpec,
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
    // r1 ≠ r2 general form, via the two circular segments.
    let (r, other) = if r1 >= r2 { (r1, r2) } else { (r2, r1) };
    let a = ((d * d + r * r - other * other) / (2.0 * d * r)).clamp(-1.0, 1.0);
    let b = ((d * d + other * other - r * r) / (2.0 * d * other)).clamp(-1.0, 1.0);
    r * r * a.acos() - (d * d + r * r - other * other) / (2.0 * d) * (1.0 - a * a).max(0.0).sqrt()
        + other * other * b.acos()
        - (d * d + other * other - r * r) / (2.0 * d) * (1.0 - b * b).max(0.0).sqrt()
}

/// Two circles that overlap *properly*: neither contains the other, neither is
/// disjoint. `t` moves between the two degenerate ends, and the margin keeps the
/// crossing a real, measurable one rather than a tangent.
fn overlapping(r1: f64, r2: f64, t: f64) -> f64 {
    let inner = (r1 - r2).abs();
    let outer = r1 + r2;
    inner + (0.05 + 0.90 * t) * (outer - inner)
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

/// The arc length of a point list.
fn arc_length(points: &[(f64, f64)]) -> f64 {
    (0..points.len())
        .map(|index| {
            let p = points[index];
            let q = points[(index + 1) % points.len()];
            (q.0 - p.0).hypot(q.1 - p.1)
        })
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
        let d = overlapping(r1, r2, t);
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
            (lens.area - overlap).abs() < 1e-6,
            "lens {} vs the overlay's overlap {}",
            lens.area,
            overlap
        );
        prop_assert!(
            (lens.area - lens_area(r1, r2, d)).abs() / lens.area < 1e-3,
            "lens {} vs the closed form {}",
            lens.area,
            lens_area(r1, r2, d)
        );
        prop_assert!(
            (only_a.area + only_b.area + lens.area - union).abs() < 1e-6,
            "the faces {} + {} + {} partition the union {}",
            only_a.area,
            only_b.area,
            lens.area,
            union
        );
        // The pieces tile each source: A only + the lens *is* A.
        prop_assert!((only_a.area + lens.area - ra.area()).abs() < 1e-6);
        prop_assert!((only_b.area + lens.area - rb.area()).abs() < 1e-6);

        // 3. **The drop test.** The midpoint of the two centres is in the lens
        //    and nowhere else; a point beyond both centres is in no face at all.
        let midline = (d / 2.0, 0.0);
        prop_assert_eq!(
            graph.face_at(midline),
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
                let first = ring[0];
                let last = ring[ring.len() - 1];
                prop_assert!(
                    (first.x - last.x).abs() < 1e-6 && (first.y - last.y).abs() < 1e-6,
                    "the ring closes: {first:?} … {last:?}"
                );
            }
            // Two functions, one area: the path the evaluator publishes and the
            // region the graph measured.
            let from_path = path_area(&face.path);
            prop_assert!(
                (from_path - face.area).abs() < 1e-6,
                "path area {from_path} vs region area {}",
                face.area
            );
        }
        prop_assert!((total - union).abs() < 1e-6, "faces {total} vs union {union}");
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
        let inner = graph.faces.iter().find(|f| f.members == [false, true]).expect("the inset");
        prop_assert_eq!(outer.holes, 1, "the frame has a hole where the inset is");
        prop_assert_eq!(inner.holes, 0);
        prop_assert!((inner.area - circle_region(cx, cy, inner_r).area()).abs() < 1e-6);

        // A hole is a hole: the frame's outer ring and its hole wind opposite
        // ways, which is what makes the region subtract rather than add under
        // both fill rules.
        let rings = path_rings(&outer.path);
        prop_assert_eq!(rings.len(), 2);
        let signed = |ring: &[geo::Coord<f64>]| -> f64 {
            let mut twice = 0.0;
            for index in 0..ring.len() {
                let p = ring[index];
                let q = ring[(index + 1) % ring.len()];
                twice += p.x * q.y - q.x * p.y;
            }
            twice / 2.0
        };
        prop_assert!(
            signed(&rings[0]) * signed(&rings[1]) < 0.0,
            "exterior {:?} vs hole {:?}",
            signed(&rings[0]),
            signed(&rings[1])
        );

        // The drop test answers *inside* the hole with the inner face, not with
        // the frame that surrounds it: the smallest face wins.
        prop_assert_eq!(
            graph.face_at((cx, cy)),
            Some(graph.faces.iter().position(|f| f.members == [false, true]).unwrap())
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

        // The ring the span lives on, as the graph reports it.
        let rings = path_rings(&graph.faces[0].path);
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
            (shoelace(&piece_a) + shoelace(&piece_b) - circle_region(0.0, 0.0, r1).area()).abs()
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

        // The wrapped case: the same span named the other way round.
        let (a2, b2) = ring_pieces_between(&ring, to, from + perimeter);
        prop_assert!((arc_length(&a2) - side * 1.5).abs() < 1e-6);
        prop_assert!((arc_length(&b2) - (perimeter - side * 1.5)).abs() < 1e-6);
    }
}

/// A `NodeId` is a UUID; the tests only need distinct ones, so the engine's own
/// allocator is used rather than a literal.
#[allow(dead_code)]
fn distinct(n: usize) -> Vec<NodeId> {
    (0..n).map(|_| new_node_id()).collect()
}
