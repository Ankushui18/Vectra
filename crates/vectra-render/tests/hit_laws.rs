//! **Hit-Test Law** (Task 5.0 §5, RULE 3).
//!
//! > Given non-overlapping shapes: a click on a shape's centre returns exactly
//! > that shape's `NodeId`; a click outside every shape returns `None`.
//!
//! This is the law that makes the canvas a *remote control*: the pointer
//! produces a `NodeId`, and the engine's `BeginDrag` takes it from there. The
//! UI never decides what was clicked.
//!
//! The generated scenes matter more than the count of the assertions. `proptest`
//! lays out **non-overlapping axis-aligned rectangles** by rejection sampling on
//! a jittered grid, so the "centre" and "outside" queries have unambiguous
//! answers by construction — the law tests the index, not a coincidence of
//! layout. Shapes are then replayed through the *real* path a pointer takes:
//! `RenderScene::sync` → `hit_test`, so the index under test is the index a
//! click would meet, including the local-frame rebasing done during sync.
//!
//! Circles, arcs and holes are covered by the hand-written cases at the end,
//! where the expected answer can be stated analytically instead of sampled.

use proptest::prelude::*;
use vectra_core::{Color, NodeId};
use vectra_geometry::{
    DirtySet, EvaluatedNode, EvaluatedPrimitive, EvaluatedScene, EvaluatedStyle,
};
use vectra_render::RenderScene;

const GRID: u32 = 6;
const CELL: f64 = 40.0;
const MARGIN: f64 = 6.0;

fn style() -> EvaluatedStyle {
    EvaluatedStyle::solid(Color::rgb(0x30, 0x30, 0x30), Color::TRANSPARENT, 0.0, 1.0)
}

/// One generated shape: which grid cell it sits in, plus its inset box.
#[derive(Debug, Clone)]
struct Cell {
    index: u32,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
}

fn cell_strategy() -> impl Strategy<Value = Cell> {
    (0u32..(GRID * GRID), 0.0f64..1.0, 0.0f64..1.0).prop_map(|(index, fx, fy)| {
        let col = (index % GRID) as f64;
        let row = (index / GRID) as f64;
        let full = CELL - 2.0 * MARGIN;
        let w = MARGIN + fx * (full - MARGIN);
        let h = MARGIN + fy * (full - MARGIN);
        Cell {
            index,
            x: col * CELL + MARGIN,
            y: row * CELL + MARGIN,
            w,
            h,
        }
    })
}

/// A scene of disjoint rectangles. Two shapes can only overlap if they share a
/// grid cell, and per-cell insets keep even those disjoint (each stays inside
/// its own `MARGIN`-inset quadrant of the cell).
fn scene_strategy() -> impl Strategy<Value = Vec<Cell>> {
    proptest::collection::vec(cell_strategy(), 1..12).prop_map(|cells| {
        // Deduplicate by cell: one shape per cell ⇒ guaranteed disjointness.
        let mut seen = std::collections::HashSet::new();
        cells
            .into_iter()
            .filter(|cell| seen.insert(cell.index))
            .collect::<Vec<_>>()
    })
}

fn build(cells: &[Cell]) -> (EvaluatedScene, RenderScene, Vec<(NodeId, Cell)>) {
    let mut scene = EvaluatedScene::empty();
    let mut placed = Vec::new();
    for (n, cell) in cells.iter().enumerate() {
        // Deterministic ids keep a failing case readable and reproducible.
        let id = NodeId::from_u128(0x5eed_0000_0000_0000_0000_0000_0000_0000 + n as u128);
        scene.nodes.insert(
            id,
            EvaluatedNode::new(
                id,
                EvaluatedPrimitive::Rect {
                    x: cell.x,
                    y: cell.y,
                    w: cell.w,
                    h: cell.h,
                    corner_radius: 0.0,
                },
                style(),
            ),
        );
        scene.z_order.push(id);
        placed.push((id, cell.clone()));
    }
    let mut render = RenderScene::new();
    render.sync(&scene, &DirtySet::all());
    (scene, render, placed)
}

fn centre(cell: &Cell) -> (f32, f32) {
    (
        (cell.x + cell.w / 2.0) as f32,
        (cell.y + cell.h / 2.0) as f32,
    )
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 96, ..ProptestConfig::default() })]

    // A click on a shape's centre returns exactly that shape.
    #[test]
    fn a_click_on_a_centre_returns_that_node(cells in scene_strategy()) {
        let (scene, mut render, placed) = build(&cells);
        prop_assume!(!placed.is_empty());
        for (id, cell) in &placed {
            let (x, y) = centre(cell);
            prop_assert_eq!(
                render.hit_test(x, y),
                Some(*id),
                "centre of {:?} at ({}, {})",
                cell,
                x,
                y
            );
        }
        // And the index agrees with the scene it was built from.
        prop_assert_eq!(scene.nodes.len(), render.len());
    }

    // A click outside every shape returns None.
    #[test]
    fn a_click_outside_every_shape_returns_none(cells in scene_strategy()) {
        let (_scene, mut render, placed) = build(&cells);
        // The annulus around the whole layout, plus the inter-cell gutters: the
        // grid pitch is CELL and every shape keeps a MARGIN from its cell edge,
        // so any point on a gutter line is inside no shape.
        let far = (GRID as f64 + 2.0) * CELL;
        for (x, y) in [(far, far), (-far, -far), (far, 0.0), (0.0, -far)] {
            prop_assert_eq!(render.hit_test(x as f32, y as f32), None, "far point ({}, {})", x, y);
        }
        for (_, cell) in &placed {
            // Gutters: one MARGIN/2 outside each edge of the shape's own cell.
            let left = (cell.x - MARGIN / 2.0) as f32;
            let top = (cell.y + cell.h / 2.0) as f32;
            prop_assert_eq!(render.hit_test(left, top), None, "gutter left of {:?}", cell);
        }
    }

    // The answer names a node that really contains the point: no phantom hits.
    #[test]
    fn a_hit_is_always_a_shape_that_contains_the_point(cells in scene_strategy()) {
        let (_scene, mut render, placed) = build(&cells);
        for (_, cell) in &placed {
            // Probe a jittered interior point (centre pulled toward a corner).
            for (fx, fy) in [(0.25, 0.25), (0.75, 0.75), (0.5, 0.1)] {
                let x = (cell.x + fx * cell.w) as f32;
                let y = (cell.y + fy * cell.h) as f32;
                let hit = render.hit_test(x, y);
                prop_assert!(hit.is_some(), "interior point ({}, {}) of {:?} missed", x, y, cell);
                let (id, _) = placed
                    .iter()
                    .find(|(id, _)| Some(*id) == hit)
                    .expect("hit names a placed node");
                let c = &placed.iter().find(|(hid, _)| hid == id).unwrap().1;
                prop_assert!(
                    x as f64 >= c.x && (x as f64) <= c.x + c.w
                        && y as f64 >= c.y && (y as f64) <= c.y + c.h,
                    "phantom hit: ({}, {}) is not inside {:?}",
                    x, y, c
                );
            }
        }
    }

    // Ordering: a click inside a later-drawn shape returns *that* shape.
    #[test]
    fn z_order_decides_between_stacked_shapes(n in 1usize..6) {
        // N identical rectangles stacked exactly on top of each other: every
        // candidate is under the pointer, so only draw order can answer.
        let mut scene = EvaluatedScene::empty();
        let mut ids = Vec::new();
        for i in 0..n {
            // The ids **descend** as the stack rises, so UUID order is the exact
            // reverse of draw order: only `z_order` can produce the answer the
            // law demands. (With ascending ids this passed by luck — the index
            // used to be built in slot order, which is UUID order.)
            let id = NodeId::from_u128(0xd00d_0000_0000_0000_0000_0000_0000_0000 + (n - i) as u128);
            scene.nodes.insert(
                id,
                EvaluatedNode::new(id, EvaluatedPrimitive::Rect {
                        x: 10.0,
                        y: 10.0,
                        w: 50.0,
                        h: 50.0,
                        corner_radius: 0.0,
                    }, style()),
            );
            scene.z_order.push(id);
            ids.push(id);
        }
        let mut render = RenderScene::new();
        render.sync(&scene, &DirtySet::all());
        prop_assert_eq!(render.hit_test(35.0, 35.0), Some(ids[n - 1]), "the last drawn wins");
    }

    // The index follows the scene: after edits, hits reflect the new geometry.
    #[test]
    fn the_index_tracks_geometry_edits(dx in -20.0f64..20.0, extra_w in 0.0f64..40.0) {
        let id = NodeId::from_u128(0xbeef);
        let mut scene = EvaluatedScene::empty();
        scene.nodes.insert(id, EvaluatedNode::new(id, EvaluatedPrimitive::Rect { x: 0.0, y: 0.0, w: 10.0 + extra_w, h: 10.0, corner_radius: 0.0 }, style()));
        scene.z_order.push(id);
        let mut render = RenderScene::new();
        render.sync(&scene, &DirtySet::all());
        prop_assert_eq!(render.hit_test(5.0, 5.0), Some(id));

        // Move it: the old centre must stop hitting, the new one must start.
        if let EvaluatedPrimitive::Rect { x, .. } = &mut scene.nodes.get_mut(&id).unwrap().primitive {
            *x = dx;
        }
        render.sync(&scene, &DirtySet::single(id));
        let new_centre = (dx + (10.0 + extra_w) / 2.0) as f32;
        prop_assert_eq!(render.hit_test(new_centre, 5.0), Some(id));
        if dx > 15.0 {
            prop_assert_eq!(render.hit_test(5.0, 5.0), None, "the old centre no longer hits");
        }
    }
}

// ── Analytic cases the sampler cannot state ────────────────────────────────

#[test]
fn a_click_inside_a_circle_but_outside_its_bbox_corner_is_a_miss() {
    // The grid is coarse on purpose: this point is inside the circle's *bbox*
    // (so it is a candidate) but outside the circle, so the exact test must
    // refuse it. This is the reason `hit_test` does containment, not bbox tests.
    let id = NodeId::from_u128(0xc1);
    let mut scene = EvaluatedScene::empty();
    scene.nodes.insert(
        id,
        EvaluatedNode::new(
            id,
            EvaluatedPrimitive::Circle {
                cx: 100.0,
                cy: 100.0,
                r: 50.0,
            },
            style(),
        ),
    );
    scene.z_order.push(id);
    let mut render = RenderScene::new();
    render.sync(&scene, &DirtySet::all());

    assert_eq!(render.hit_test(100.0, 100.0), Some(id), "dead centre");
    assert_eq!(
        render.hit_test(100.0 + 49.0, 100.0),
        Some(id),
        "just inside the rim"
    );
    assert_eq!(
        render.hit_test(100.0 + 49.0, 100.0 + 49.0),
        None,
        "bbox corner, outside the circle"
    );
    assert_eq!(
        render.hit_test(100.0 + 51.0, 100.0),
        None,
        "just outside the rim"
    );
}

#[test]
fn a_hole_in_a_path_is_not_a_hit() {
    // Even-odd parity is what makes a hole a hole — for the pointer too.
    let id = NodeId::from_u128(0x1101);
    let mut builder = lyon::path::Path::builder();
    // Outer square, 0..100.
    let _ = builder.begin(lyon::math::point(0.0, 0.0));
    let _ = builder.line_to(lyon::math::point(100.0, 0.0));
    let _ = builder.line_to(lyon::math::point(100.0, 100.0));
    let _ = builder.line_to(lyon::math::point(0.0, 100.0));
    builder.close();
    // Inner square (the hole), 40..60 — reversed winding on purpose: even-odd
    // parity must not care which way a hole is wound.
    let _ = builder.begin(lyon::math::point(40.0, 40.0));
    let _ = builder.line_to(lyon::math::point(40.0, 60.0));
    let _ = builder.line_to(lyon::math::point(60.0, 60.0));
    let _ = builder.line_to(lyon::math::point(60.0, 40.0));
    builder.close();
    let outer = builder.build();
    let mut scene = EvaluatedScene::empty();
    scene.nodes.insert(
        id,
        EvaluatedNode::new(id, EvaluatedPrimitive::Path(outer), style()),
    );
    scene.z_order.push(id);
    let mut render = RenderScene::new();
    render.sync(&scene, &DirtySet::all());

    assert_eq!(render.hit_test(10.0, 10.0), Some(id), "inside the ring");
    assert_eq!(render.hit_test(50.0, 50.0), None, "inside the hole");
}

#[test]
fn an_arc_hit_respects_the_wedge_not_the_circle() {
    // A quarter arc centred at (0,0) from 0° to 90°. In SVG's y-down frame the
    // wedge covers the first quadrant; the opposite quadrant must miss.
    let id = NodeId::from_u128(0xa2c);
    let mut scene = EvaluatedScene::empty();
    scene.nodes.insert(
        id,
        EvaluatedNode::new(
            id,
            EvaluatedPrimitive::Arc {
                cx: 0.0,
                cy: 0.0,
                r: 50.0,
                start_angle: 0.0,
                end_angle: std::f64::consts::FRAC_PI_2,
            },
            style(),
        ),
    );
    scene.z_order.push(id);
    let mut render = RenderScene::new();
    render.sync(&scene, &DirtySet::all());

    assert_eq!(
        render.hit_test(25.0, 25.0),
        Some(id),
        "the pie's own quadrant"
    );
    assert_eq!(
        render.hit_test(-25.0, -25.0),
        None,
        "the remaining three quarters"
    );
}

#[test]
fn an_empty_scene_hits_nothing() {
    let mut render = RenderScene::new();
    let scene = EvaluatedScene::empty();
    render.sync(&scene, &DirtySet::all());
    assert_eq!(render.hit_test(0.0, 0.0), None);
    assert_eq!(render.hit_test(-1e6, 1e6), None);
}

#[test]
fn a_removed_node_stops_hitting() {
    let a = NodeId::from_u128(0xa1);
    let b = NodeId::from_u128(0xb2);
    let mut scene = EvaluatedScene::empty();
    for (id, x) in [(a, 0.0), (b, 100.0)] {
        scene.nodes.insert(
            id,
            EvaluatedNode::new(
                id,
                EvaluatedPrimitive::Rect {
                    x,
                    y: 0.0,
                    w: 20.0,
                    h: 20.0,
                    corner_radius: 0.0,
                },
                style(),
            ),
        );
        scene.z_order.push(id);
    }
    let mut render = RenderScene::new();
    render.sync(&scene, &DirtySet::all());
    assert_eq!(render.hit_test(10.0, 10.0), Some(a));

    scene.nodes.remove(&a);
    scene.z_order.retain(|id| *id != a);
    render.sync(&scene, &DirtySet::single(a));
    assert_eq!(
        render.hit_test(10.0, 10.0),
        None,
        "a dropped node cannot be hit"
    );
    assert_eq!(
        render.hit_test(110.0, 10.0),
        Some(b),
        "and its neighbour still can"
    );
    assert_eq!(render.len(), 1);
}

/// **Stacking.** Where two shapes overlap, the pointer finds the one the canvas
/// paints last — and it finds it *every* time.
///
/// This is the law the index was missing. `HitIndex` resolves overlapping
/// bounding boxes by insertion order, and `RenderScene` used to insert in
/// `slots` order — which is `Uuid` order. A UUID's order has nothing to do with
/// the order a designer stacked two shapes, so the answer was whichever id
/// happened to sort later: two overlapping rectangles could return one shape in
/// one process and the other in the next, for the same document. The two cases
/// below deliberately **disagree** with id order, so neither can pass by luck.
#[test]
fn the_front_most_shape_wins_the_pixel_it_covers() {
    let low = NodeId::from_u128(0x0000_0001);
    let high = NodeId::from_u128(0xffff_ffff);

    // `z_order` is back → front; the two shapes overlap between (50, 50) and
    // (100, 100), and each has a region the other does not reach.
    let stacked = |back: NodeId, front: NodeId| {
        let mut scene = EvaluatedScene::empty();
        for (id, x, y) in [(back, 0.0, 0.0), (front, 50.0, 50.0)] {
            scene.nodes.insert(
                id,
                EvaluatedNode::new(
                    id,
                    EvaluatedPrimitive::Rect {
                        x,
                        y,
                        w: 100.0,
                        h: 100.0,
                        corner_radius: 0.0,
                    },
                    style(),
                ),
            );
        }
        scene.z_order = vec![back, front];
        let mut render = RenderScene::new();
        render.sync(&scene, &DirtySet::all());
        render
    };

    let mut later_id_on_top = stacked(low, high);
    assert_eq!(
        later_id_on_top.hit_test(75.0, 75.0),
        Some(high),
        "the shared pixel belongs to the shape drawn last"
    );
    assert_eq!(later_id_on_top.hit_test(10.0, 10.0), Some(low));
    assert_eq!(later_id_on_top.hit_test(125.0, 125.0), Some(high));

    // The same geometry, the stacking reversed — and it is the *other* shape
    // that owns the shared pixel, even though id order would say otherwise.
    let mut earlier_id_on_top = stacked(high, low);
    assert_eq!(
        earlier_id_on_top.hit_test(75.0, 75.0),
        Some(low),
        "stacking decides, not id order"
    );
    assert_eq!(earlier_id_on_top.hit_test(10.0, 10.0), Some(high));
    assert_eq!(earlier_id_on_top.hit_test(125.0, 125.0), Some(low));

    // Rebuilding the index for a *new* revision must not re-shuffle the answer:
    // the same query, asked after an incremental sync, still finds the top shape.
    let mut again = stacked(high, low);
    again.sync(&EvaluatedScene::empty(), &DirtySet::all());
    let mut scene = EvaluatedScene::empty();
    for (id, x, y) in [(high, 0.0, 0.0), (low, 50.0, 50.0)] {
        scene.nodes.insert(
            id,
            EvaluatedNode::new(
                id,
                EvaluatedPrimitive::Rect {
                    x,
                    y,
                    w: 100.0,
                    h: 100.0,
                    corner_radius: 0.0,
                },
                style(),
            ),
        );
    }
    scene.z_order = vec![high, low];
    again.sync(&scene, &DirtySet::all());
    assert_eq!(
        again.hit_test(75.0, 75.0),
        Some(low),
        "a re-sync answers the same question the same way"
    );
}

/// **The index follows the order, not just the geometry.** Where two shapes
/// overlap, the answer is a statement about *stacking* — so a sync that moves
/// nothing but the draw order has to re-point the index. The index is rebuilt
/// lazily, from the scene's cached state, so this is exactly the kind of drift a
/// stale cache produces: the right answer before the reorder, the wrong one
/// after, with a document that never moved.
///
/// The dirty id is deliberately one the renderer does not hold (the engine may
/// name a node that has not been evaluated); nothing is re-synced, so the order
/// is the only input that changed.
#[test]
fn a_reorder_alone_re_points_the_index() {
    let back = NodeId::from_u128(0x0000_0002);
    let front = NodeId::from_u128(0x0000_0003);
    let mut scene = EvaluatedScene::empty();
    for (id, x, y) in [(back, 0.0, 0.0), (front, 50.0, 50.0)] {
        scene.nodes.insert(
            id,
            EvaluatedNode::new(
                id,
                EvaluatedPrimitive::Rect {
                    x,
                    y,
                    w: 100.0,
                    h: 100.0,
                    corner_radius: 0.0,
                },
                style(),
            ),
        );
    }
    scene.z_order = vec![back, front];
    let mut render = RenderScene::new();
    render.sync(&scene, &DirtySet::all());
    assert_eq!(render.hit_test(75.0, 75.0), Some(front));

    // Same geometry, same boxes, the stack turned over.
    scene.z_order = vec![front, back];
    render.sync(&scene, &DirtySet::single(NodeId::from_u128(0xdead)));

    assert_eq!(
        render.hit_test(75.0, 75.0),
        Some(back),
        "the shape drawn last owns the pixel now"
    );
    // Each shape keeps the pixels only it covers: `back` sits at the origin,
    // `front` at (50, 50), and neither inherits the other's region.
    assert_eq!(render.hit_test(10.0, 10.0), Some(back));
    assert_eq!(render.hit_test(125.0, 125.0), Some(front));

    // And the same query twice answers the same way: the index is not rebuilt
    // between them, because nothing changed.
    assert_eq!(render.hit_test(75.0, 75.0), Some(back));
}

/// **Laziness, exactly.** The index is rebuilt when — and only when — its inputs
/// changed: **the boxes, and the order the rows are painted in**. Nothing else.
///
/// This is the law the old `hit_dirty` flag could not state. That flag was a
/// promise every mutation had to keep, and Task 10.4 had to disclose that a path
/// which forgot it would serve a stale answer; the renderer now compares its
/// inputs instead of remembering a decision, and this law is that comparison's
/// contract. It also pins the *cost* side: an eye or a lock is a boolean
/// `hit_test` reads at query time, so toggling visibility must not move the index
/// at all (RULE 4 — a flag toggle is free, down to the spatial index).
#[test]
fn a_hit_query_rebuilds_only_when_the_index_inputs_changed() {
    let a = NodeId::from_u128(0x0a);
    let b = NodeId::from_u128(0x0b);
    let mut scene = EvaluatedScene::empty();
    for (id, x) in [(a, 0.0), (b, 100.0)] {
        scene.nodes.insert(
            id,
            EvaluatedNode::new(
                id,
                EvaluatedPrimitive::Rect {
                    x,
                    y: 0.0,
                    w: 20.0,
                    h: 20.0,
                    corner_radius: 0.0,
                },
                style(),
            ),
        );
        scene.z_order.push(id);
    }
    let mut render = RenderScene::new();
    render.sync(&scene, &DirtySet::all());

    // The first query builds it; asking again does not.
    assert_eq!(render.hit_test(10.0, 10.0), Some(a));
    let after_first = render.hit_rebuilds();
    assert_eq!(after_first, 1, "one query, one build");
    for _ in 0..4 {
        assert_eq!(render.hit_test(10.0, 10.0), Some(a));
        assert_eq!(render.hit_test(110.0, 10.0), Some(b));
        assert_eq!(render.hit_test(5000.0, 5000.0), None, "a miss is a miss");
    }
    assert_eq!(render.hit_rebuilds(), after_first, "four queries, no build");

    // A flag toggle is not an input: the boxes and the order are unchanged, so
    // the index is reused — and the answer still respects the flag, because
    // `hit_test` reads it at query time.
    let mut hidden = scene.clone();
    hidden.nodes.get_mut(&a).unwrap().visible = false;
    render.sync(&hidden, &DirtySet::single(a));
    assert_eq!(
        render.hit_test(10.0, 10.0),
        None,
        "a hidden shape is not there"
    );
    assert_eq!(
        render.hit_rebuilds(),
        after_first,
        "an eye costs no rebuild at all"
    );
    render.sync(&scene, &DirtySet::single(a));
    assert_eq!(render.hit_test(10.0, 10.0), Some(a));

    // Geometry is an input.
    let mut moved = scene.clone();
    if let EvaluatedPrimitive::Rect { x, .. } = &mut moved.nodes.get_mut(&a).unwrap().primitive {
        *x = 40.0;
    }
    render.sync(&moved, &DirtySet::single(a));
    assert_eq!(
        render.hit_test(10.0, 10.0),
        None,
        "the old box is empty now"
    );
    assert_eq!(render.hit_test(50.0, 10.0), Some(a));
    let after_move = render.hit_rebuilds();
    assert_eq!(after_move, after_first + 1, "geometry rebuilds once");

    // …a second sync that changes nothing does not, even though it is the same
    // `DirtySet`: the *inputs* are what decide, so the query is served warm.
    render.sync(&moved, &DirtySet::single(a));
    assert_eq!(render.hit_test(50.0, 10.0), Some(a));
    assert_eq!(render.hit_rebuilds(), after_move, "no change, no build");

    // Stacking order is an input too: the boxes are the same and each shape still
    // owns its own region, but the index is rebuilt — because insertion order is
    // the answer for anything the boxes overlap. (The rebuild is lazy, so the
    // count moves on the query, not on the sync.)
    let mut restacked = moved.clone();
    restacked.z_order = vec![b, a];
    render.sync(&restacked, &DirtySet::single(NodeId::from_u128(0xf00d)));
    assert_eq!(render.hit_test(50.0, 10.0), Some(a));
    assert_eq!(
        render.hit_rebuilds(),
        after_move + 1,
        "a reorder rebuilds once, on the next query"
    );

    // Removing a node is an input: its box leaves the index.
    let mut dropped = restacked.clone();
    dropped.nodes.remove(&b);
    dropped.z_order.retain(|id| *id != b);
    render.sync(&dropped, &DirtySet::single(b));
    assert_eq!(
        render.hit_test(110.0, 10.0),
        None,
        "a dropped shape is gone"
    );
    assert_eq!(
        render.hit_rebuilds(),
        after_move + 2,
        "a removal rebuilds once"
    );
    // …and the query right after the reply is served from the fresh index.
    assert_eq!(render.hit_test(50.0, 10.0), Some(a));
    assert_eq!(render.hit_rebuilds(), after_move + 2);
}
