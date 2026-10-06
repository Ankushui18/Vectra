//! **Tessellation Validity Law** (Task 5.0 §5): a randomly generated *valid*
//! scene must tessellate without `lyon` panicking, and the triangles it produces
//! must be usable — finite positions, in-bounds indices, and a total area that
//! agrees with the primitive's own analytic area.
//!
//! The area clause is the strong one. A tessellation can fail in ways that leave
//! no NaN and no panic: a wrongly wound ring that renders as a hole, a dropped
//! subpath, a fill rule that swallows an inner contour. All of those change the
//! covered area, so comparing the triangle soup's area against the shape's own
//! area catches them, while `is_finite`/`indices_in_bounds` catch the cruder
//! failures.
//!
//! The closing law drives a whole `EvaluatedScene` through `RenderScene::sync`
//! and a `MockSink`: the *renderer's* entry point, on a random scene, must not
//! panic either — and must leave a self-consistent mirror behind.

use proptest::prelude::*;
use vectra_core::{Color, NodeId};
use vectra_geometry::{
    DirtySet, EvaluatedNode, EvaluatedPrimitive, EvaluatedScene, EvaluatedStyle,
    ARC_SEGMENTS_PER_TAU,
};
use vectra_render::{tessellate, MeshKind, MockSink, RenderScene, UpdateReport, WriteKind};

fn style() -> EvaluatedStyle {
    EvaluatedStyle::solid(
        Color::rgb(0x22, 0x66, 0xee),
        Color::rgba(0x11, 0x11, 0x11, 200),
        1.5,
        1.0,
    )
}

/// A simple polygon's area, by the shoelace formula. (Test-side arithmetic used
/// only to build an expectation — the renderer never computes it.)
fn shoelace(points: &[(f64, f64)]) -> f64 {
    let mut sum = 0.0;
    for i in 0..points.len() {
        let a = points[i];
        let b = points[(i + 1) % points.len()];
        sum += a.0 * b.1 - b.0 * a.1;
    }
    (sum / 2.0).abs()
}

/// The area a regular `n`-gon inscribed in radius `r` covers — which is exactly
/// what `primitive_to_path` samples a circle/arc into.
fn inscribed_area(r: f64, segments: usize) -> f64 {
    let n = segments as f64;
    0.5 * n * r * r * (std::f64::consts::TAU / n).sin()
}

fn rect(x: f64, y: f64, w: f64, h: f64) -> EvaluatedPrimitive {
    EvaluatedPrimitive::Rect {
        x,
        y,
        w,
        h,
        corner_radius: 0.0,
    }
}

fn circle(cx: f64, cy: f64, r: f64) -> EvaluatedPrimitive {
    EvaluatedPrimitive::Circle { cx, cy, r }
}

/// A simple polygon, plus the vertex order the shoelace formula needs.
///
/// Points sorted by angle around their centroid never self-intersect, so
/// even-odd and non-zero fills agree and the shoelace area of *that* order is
/// the area the tessellator must produce. (The unsorted input order is not a
/// polygon traversal, so its shoelace value would be meaningless.)
fn sorted_polygon(points: &[(f64, f64)]) -> (EvaluatedPrimitive, Vec<(f64, f64)>) {
    let n = points.len() as f64;
    let cx = points.iter().map(|p| p.0).sum::<f64>() / n;
    let cy = points.iter().map(|p| p.1).sum::<f64>() / n;
    let mut sorted = points.to_vec();
    sorted.sort_by(|a, b| {
        let angle = |p: &(f64, f64)| (p.1 - cy).atan2(p.0 - cx);
        angle(a)
            .partial_cmp(&angle(b))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let mut builder = lyon::path::Path::builder();
    builder.begin(lyon::math::point(sorted[0].0 as f32, sorted[0].1 as f32));
    for point in &sorted[1..] {
        builder.line_to(lyon::math::point(point.0 as f32, point.1 as f32));
    }
    builder.end(true);
    (EvaluatedPrimitive::Path(builder.build()), sorted)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(96))]

    /// Rectangles: the tessellated area is `w × h`, exactly (they are straight
    /// edges, so there is no flattening error at all).
    #[test]
    fn rectangles_tessellate_to_their_area(
        x in -500.0f64..500.0,
        y in -500.0f64..500.0,
        w in 0.5f64..400.0,
        h in 0.5f64..400.0,
    ) {
        let primitive = rect(x, y, w, h);
        let t = tessellate(NodeId::nil(), &primitive, &style()).unwrap();
        prop_assert!(t.fill.is_finite(), "no NaN/inf may reach a buffer");
        prop_assert!(t.fill.indices_in_bounds());
        prop_assert_eq!(t.fill.indices.len() % 3, 0, "whole triangles");
        let area = t.fill.area();
        prop_assert!(
            (area - w * h).abs() <= 1e-3 * (w * h).max(1.0),
            "area {} vs {} for {}x{}", area, w * h, w, h
        );
    }

    /// Circles: the tessellated area is the inscribed `ARC_SEGMENTS_PER_TAU`-gon.
    #[test]
    fn circles_tessellate_to_their_polygon(
        cx in -500.0f64..500.0,
        cy in -500.0f64..500.0,
        r in 0.5f64..300.0,
    ) {
        let primitive = circle(cx, cy, r);
        let t = tessellate(NodeId::nil(), &primitive, &style()).unwrap();
        prop_assert!(t.fill.is_finite());
        prop_assert!(t.fill.indices_in_bounds());
        let expected = inscribed_area(r, ARC_SEGMENTS_PER_TAU);
        let area = t.fill.area();
        prop_assert!(
            (area - expected).abs() <= 1e-3 * expected,
            "area {} vs {} for r={}", area, expected, r
        );
    }

    /// Simple polygons: area equality survives an arbitrary ring, and the
    /// bounds the renderer derives from the flattened rings contain every
    /// tessellated vertex (the hit index trusts those bounds).
    #[test]
    fn polygons_tessellate_to_their_area(
        points in proptest::collection::vec(
            (0.0f64..300.0, 0.0f64..300.0),
            3..7,
        )
    ) {
        let (primitive, order) = sorted_polygon(&points);
        let t = tessellate(NodeId::nil(), &primitive, &style()).unwrap();
        prop_assert!(t.fill.is_finite());
        prop_assert!(t.fill.indices_in_bounds());
        let expected = shoelace(&order);
        let area = t.fill.area();
        // Relative for real shapes, with a floor so a degenerate (zero-area)
        // polygon is allowed its flattening noise rather than a division.
        prop_assert!(
            (area - expected).abs() <= 1e-2 * expected.max(50.0),
            "area {} vs shoelace {}", area, expected
        );
        // The hit test's prefilter must not reject a point the fill covers.
        for vertex in &t.fill.vertices {
            let world_x = vertex[0] + t.key.origin.0;
            let world_y = vertex[1] + t.key.origin.1;
            prop_assert!(
                t.world_bounds.expanded(1e-3).contains(world_x, world_y),
                "vertex ({world_x}, {world_y}) escapes {:?}", t.world_bounds
            );
        }
    }

    /// Arcs: no analytic statement (a filled arc is a circular segment, and its
    /// closure depends on the sweep), but validity must hold for every sweep the
    /// engine can produce — including a full turn.
    #[test]
    fn arcs_tessellate_without_panicking(
        r in 0.5f64..200.0,
        start in 0.0f64..std::f64::consts::TAU,
        sweep in 0.0f64..std::f64::consts::TAU,
    ) {
        let primitive = EvaluatedPrimitive::Arc {
            cx: 100.0,
            cy: 100.0,
            r,
            start_angle: start,
            end_angle: start + sweep,
        };
        let t = tessellate(NodeId::nil(), &primitive, &style()).unwrap();
        prop_assert!(t.fill.is_finite());
        prop_assert!(t.fill.indices_in_bounds());
        // Every stroke layer gets its own outline (Task 10.2 RULE 3): `style()`
        // wears exactly one stroke, so there is exactly one mesh here.
        prop_assert_eq!(t.strokes.len(), 1);
        prop_assert!(t.strokes[0].is_finite());
        prop_assert!(t.strokes[0].indices_in_bounds());
        let area = t.fill.area();
        prop_assert!(area.is_finite() && area >= 0.0);
        prop_assert!(area <= std::f64::consts::PI * r * r + 1e-6);
    }
}

// The renderer's own entry point, on a random scene: sync must not panic, the
// plan must be self-consistent, and the mirror it leaves behind must describe
// exactly the nodes the scene holds — including the "second sync is free" claim.
proptest! {
    #![proptest_config(ProptestConfig::with_cases(32))]

    #[test]
    fn a_random_scene_syncs_and_flushes_without_panicking(
        specs in proptest::collection::vec(
            (
                -200.0f64..200.0,
                -200.0f64..200.0,
                1.0f64..200.0,
                1.0f64..200.0,
                0.0f64..8.0,
            ),
            1..8,
        )
    ) {
        let mut scene = EvaluatedScene::empty();
        for (index, (x, y, w, h, stroke_width)) in specs.iter().enumerate() {
            let id = NodeId::from_u128(index as u128 + 1);
            scene.nodes.insert(
                id,
                EvaluatedNode::new(
                    id,
                    rect(*x, *y, *w, *h),
                    EvaluatedStyle::solid(
                        style().first_fill().unwrap().paint.preview_color(),
                        style().first_stroke().unwrap().paint.preview_color(),
                        *stroke_width,
                        1.0,
                    ),
                ),
            );
            scene.z_order.push(id);
        }

        let mut render = RenderScene::new();
        let mut sink = MockSink::new();
        render.sync(&scene, &DirtySet::all());
        render.flush(&mut sink).expect("the mock sink never fails");

        prop_assert_eq!(render.len(), scene.nodes.len());
        prop_assert_eq!(sink.nodes.len(), scene.nodes.len());
        prop_assert_eq!(render.order(), &scene.z_order[..]);
        // Every node got its mesh buffers plus an 80-byte instance row (the row
        // widened in Task 10.2 RULE 3: blend code, ramp window, gradient frame).
        for id in &scene.z_order {
            let kinds = sink.kinds_for(*id);
            prop_assert!(kinds.contains(&WriteKind::Vertices(MeshKind::Fill)));
            prop_assert!(kinds.contains(&WriteKind::Indices(MeshKind::Fill)));
            prop_assert!(kinds.contains(&WriteKind::Instance));
            prop_assert_eq!(sink.nodes[id].instance.len(), 80);
        }
        // A second, unchanged full sync writes nothing at all.
        let before = sink.calls.len();
        render.sync(&scene, &DirtySet::all());
        render.flush(&mut sink).unwrap();
        prop_assert_eq!(sink.calls.len(), before, "an unchanged scene is free");
    }
}

/// The report the UI reads must describe the work that was done.
#[test]
fn the_update_report_describes_the_plan() {
    let mut scene = vectra_geometry::EvaluatedScene::empty();
    let id = NodeId::from_u128(7);
    scene.nodes.insert(
        id,
        EvaluatedNode::new(id, rect(0.0, 0.0, 10.0, 10.0), style()),
    );
    scene.z_order.push(id);

    let mut render = RenderScene::new();
    let mut sink = MockSink::new();
    let dirty = DirtySet::nodes([id]);
    let report: UpdateReport = {
        render.sync(&scene, &dirty);
        render.flush(&mut sink).unwrap();
        render.report().clone()
    };
    assert_eq!(report.nodes, 1);
    assert_eq!(report.created, 1);
    assert_eq!(report.retessellated, 1);
    assert!(
        report.vertices >= 4,
        "a rectangle has at least four vertices"
    );
    assert!(report.indices >= 6, "…and two triangles");
    // The plan's bytes: one 80-byte instance row per written draw item, plus the
    // vertex and index traffic.
    assert_eq!(
        report.bytes,
        report.instances * 80 + report.vertices * 8 + report.indices * 4
    );
    assert_eq!(report.draw_items, 2, "a fill and a stroke");
    assert_eq!(report.writes, render.plan().ops.len());
    assert!(!report.full);
}
