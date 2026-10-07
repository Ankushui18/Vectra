//! **Task 10.7 RULE 3's laws** — Alpha Lock and clipping masks, as geometry.
//!
//! These run the **real** pipeline: `Engine::dispatch` → `Document` →
//! `GeometryEvaluator` → `EvaluatedScene` → the clip pass. Nothing is stubbed,
//! and every assertion is arithmetic on areas (`region_area`), which is what
//! makes "constrained to the layer's bounds" something a machine can check:
//!
//! * **Alpha Lock Law** — a region clipped to a layer's content is a *subset* of
//!   that content (`area(clip ∩ content) == area(clip)`), never larger than the
//!   probe, and empty when the content is empty: a stroke with nowhere to land
//!   survives as nothing rather than as a stroke outside the lines.
//! * **Pen Stroke Law** (the 1-D clip) — for artwork with no area, the surviving
//!   runs are exactly the points that were inside:
//!   `Σ len(runs) == #{points inside}`, every run point is inside the content,
//!   and order is preserved.
//! * **Clipping Mask Law** — a clipping layer's node is reshaped to
//!   `region(node) ∩ region(layer below)`; the layer below is untouched; the
//!   bottom layer (nothing below) clips to nothing.
//! * **Non-Destructive Law** — clearing the flag restores the *document's* own
//!   geometry: the geometry comes back from the evaluator, not from a saved copy.
//! * **Non-Compounding Law** — a *live* mask re-clips from the node's baseline,
//!   so a mask that grows lets the artwork back in. This is the law that fails
//!   if the pass ever re-clips its own previous output.

use proptest::prelude::*;
use vectra_core::{new_layer_id, new_node_id, BooleanOp, Command, Engine, LayerId, NodeId};
use vectra_geometry::paths::primitive_to_path;
use vectra_geometry::{path_to_svg_data, DirtySet, EvaluatedScene, Evaluator, GeometryEvaluator};
use vectra_operations::{
    boolean, clip_polyline, clip_region, clipped_path, contains_point, content_region,
    path_to_multi_polygon, region_area, ClipState,
};

// ── Harness ──────────────────────────────────────────────────────────────

struct Harness {
    engine: Engine,
    scene: EvaluatedScene,
    clip: ClipState,
}

impl Harness {
    fn new() -> Self {
        let mut engine = Engine::new();
        engine.document_mut().open_workspace(800.0, 600.0);
        let mut harness = Self {
            engine,
            scene: EvaluatedScene::empty(),
            clip: ClipState::new(),
        };
        harness.refresh();
        harness
    }

    fn doc(&self) -> &vectra_core::Document {
        self.engine.document()
    }

    fn rect(&mut self, name: &str, x: f64, y: f64, w: f64, h: f64) -> NodeId {
        let id = new_node_id();
        self.engine
            .dispatch(Command::CreateNode {
                id,
                kind: vectra_core::NodeKind::rectangle(x, y, w, h),
                name: Some(name.to_string()),
                index: None,
            })
            .expect("create");
        self.refresh();
        id
    }

    fn layer(&mut self, name: &str) -> LayerId {
        let id = new_layer_id();
        self.engine
            .dispatch(Command::CreateLayer {
                id,
                name: name.to_string(),
                index: None,
                artboard: None,
            })
            .expect("create layer");
        self.refresh();
        id
    }

    fn assign(&mut self, node: NodeId, layer: LayerId) {
        self.engine
            .dispatch(Command::AssignNodeToLayer {
                node_id: node,
                layer,
            })
            .expect("assign");
        self.refresh();
    }

    fn set_alpha_lock(&mut self, layer: LayerId, locked: bool) {
        self.engine
            .dispatch(Command::SetLayerAlphaLocked {
                id: layer,
                alpha_locked: locked,
            })
            .expect("alpha lock");
        self.refresh();
    }

    fn set_clipping_mask(&mut self, layer: LayerId, clip: bool) {
        self.engine
            .dispatch(Command::SetLayerClippingMask {
                id: layer,
                clipping_mask: clip,
            })
            .expect("clipping mask");
        self.refresh();
    }

    fn refresh(&mut self) {
        let context = {
            let ctx = self.engine.evaluation_context();
            GeometryEvaluator.evaluate_full(self.engine.document(), &ctx)
        };
        self.scene = context.scene;
    }

    /// **The incremental refresh the engine actually does** (`settle` → `patch`):
    /// only the named ids come back from the evaluator, and everything else keeps
    /// the primitive the *scene* already holds.
    ///
    /// This distinction is the whole point of the Non-Compounding Law: after a
    /// pass, a clipped node's cached primitive is a *derivative*, and a full
    /// re-evaluation would quietly put the document's geometry back before the
    /// pass ever ran — hiding exactly the bug the law is for.
    fn refresh_nodes(&mut self, ids: &[NodeId]) {
        let dirty = DirtySet::nodes(ids.iter().copied());
        let partial = {
            let ctx = self.engine.evaluation_context();
            GeometryEvaluator
                .evaluate(self.engine.document(), &ctx, &dirty)
                .scene
        };
        let doc = self.engine.document().clone();
        self.scene.apply_partial(&doc, partial, &dirty);
    }

    /// What one node currently covers, in the scene.
    fn region(&self, node: NodeId) -> geo::MultiPolygon<f64> {
        vectra_operations::region_of(&self.scene.get(node).expect("node in scene").primitive)
    }

    /// The layer's content, the region alpha lock constrains to.
    fn content(&self, layer: LayerId) -> geo::MultiPolygon<f64> {
        let children = self
            .doc()
            .layers
            .get(&layer)
            .expect("layer")
            .children
            .clone();
        content_region(&self.scene, &children)
    }

    /// One pass of the real clip machinery.
    fn run_clip(&mut self, reevaluated: &[NodeId]) -> Vec<NodeId> {
        let doc = self.engine.document().clone();
        self.clip.apply(&mut self.scene, &doc, reevaluated).changed
    }

    fn node_ids(&self) -> Vec<NodeId> {
        self.doc().nodes.keys().copied().collect()
    }
}

/// Areas are compared with a relative tolerance: every region in this file has
/// been through the lyon → `f32` → `geo` boundary at least once.
///
/// `1e-6` is exact enough for the *integer* fixtures below — a 40×40 rectangle
/// has an f32-exact area, so the laws that use one are compared at the strict
/// bound.
fn close(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-6 * (1.0 + a.abs().max(b.abs()))
}

/// …and this is the bound for geometry that is *not* on nice numbers.
///
/// The generated cases move the rectangles to arbitrary fractional coordinates,
/// and a lyon path stores `f32`: at x ≈ 200 the representation error is already
/// ~1e-5, so a sliver 4 × 1 units across carries an area error of ~4e-5 — four
/// hundred times the strict bound, from the boundary alone. `1e-4` relative is
/// still two orders of magnitude tighter than any real defect (a compounding
/// clip, a wrong mask, an empty result) could hide in.
fn close_geom(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-4 * (1.0 + a.abs().max(b.abs()))
}

// ── Alpha Lock ───────────────────────────────────────────────────────────

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]

    /// **The Alpha Lock Law.** For any layer content and any probe region:
    /// the clip is inside the content, no bigger than the probe, and it is
    /// exactly the intersection of the two.
    #[test]
    fn the_clip_is_inside_the_layers_content(
        cx in 0.0f64..200.0, cy in 0.0f64..200.0, cw in 1.0f64..100.0, ch in 1.0f64..100.0,
        px in 0.0f64..200.0, py in 0.0f64..200.0, pw in 1.0f64..100.0, ph in 1.0f64..100.0,
    ) {
        let mut h = Harness::new();
        let content_id = h.rect("content", cx, cy, cw, ch);
        let layer = h.doc().active_layer().expect("the workspace has a layer");
        h.assign(content_id, layer);
        // The probe is a rectangle too, but it is clipped as a *region* — the
        // same call the commit path makes for a filled draft.
        let content = h.content(layer);
        let probe = clipped_path(&rect_path(px, py, pw, ph), &content).map(|path| {
            path_to_multi_polygon(&path).expect("a clipped path is a region")
        });
        let clipped = probe.unwrap_or_else(vectra_operations::empty_region);
        let area = region_area(&clipped);
        let inner = region_area(&clip_region(&clipped, &content));
        let overlap = region_area(&boolean(&rect_region(px, py, pw, ph), &content, BooleanOp::Intersect));
        prop_assert!(
            area <= region_area(&content) * (1.0 + 1e-4) + 1e-4,
            "clip grew past the content: {area} vs {}",
            region_area(&content)
        );
        prop_assert!(
            area <= region_area(&rect_region(px, py, pw, ph)) * (1.0 + 1e-4) + 1e-4,
            "clip grew past the probe"
        );
        prop_assert!(close_geom(inner, area), "the clip escaped the layer's content");
        prop_assert!(close_geom(area, overlap), "the clip is not the intersection");
    }
}

/// **An empty layer has nothing to lock to**: the region form refuses the
/// stroke outright (the commit path turns this `None` into a sentence the
/// designer reads, never a silently empty node).
#[test]
fn an_empty_locked_layer_refuses_the_stroke() {
    let mut h = Harness::new();
    let layer = h.doc().active_layer().expect("layer");
    h.set_alpha_lock(layer, true);
    let content = h.content(layer);
    assert!(
        region_area(&content) == 0.0,
        "the fresh layer must be empty"
    );
    assert!(clipped_path(&rect_path(10.0, 10.0, 40.0, 40.0), &content).is_none());
    // And the 1-D form agrees: no run survives.
    let runs = clip_polyline(&[(0.0, 0.0), (50.0, 50.0)], &content);
    assert!(runs.is_empty());
}

/// **The Pen Stroke Law.** A curve with no area is clipped by *points*: the
/// surviving runs are exactly the points that were inside, in order — nothing
/// inside the artwork is thrown away and nothing outside it survives.
#[test]
fn an_open_path_keeps_exactly_the_stretch_inside_the_artwork() {
    let mut h = Harness::new();
    let content_id = h.rect("content", 50.0, 0.0, 50.0, 100.0);
    let layer = h.doc().active_layer().expect("layer");
    h.assign(content_id, layer);
    let content = h.content(layer);
    // A diagonal line from the empty half into the covered half and back out.
    let samples: Vec<(f64, f64)> = (0..21)
        .map(|step| {
            let t = step as f64 / 20.0;
            (t * 100.0, 50.0 - (t - 0.5).abs() * 80.0)
        })
        .collect();
    let inside: Vec<(f64, f64)> = samples
        .iter()
        .copied()
        .filter(|(x, y)| contains_point(&content, *x, *y))
        .collect();
    assert!(
        !inside.is_empty() && inside.len() < samples.len(),
        "a real crossing"
    );
    let runs = clip_polyline(&samples, &content);
    let kept: usize = runs.iter().map(Vec::len).sum();
    assert_eq!(kept, inside.len(), "the 1-D clip lost or invented a point");
    for run in &runs {
        for point in run {
            assert!(contains_point(&content, point.0, point.1));
        }
    }
    // Order is the stroke's, and every kept point was in the input.
    let flattened: Vec<(f64, f64)> = runs.iter().flatten().copied().collect();
    let mut cursor = 0usize;
    for point in &flattened {
        let found = samples[cursor..]
            .iter()
            .position(|candidate| candidate == point)
            .expect("a kept point must be an input point");
        cursor += found + 1;
    }
}

// ── Clipping Mask ────────────────────────────────────────────────────────

/// **The Clipping Mask Law.** A node on a clipping layer is reshaped to its
/// intersection with the layer below; the layer below is untouched; the bottom
/// layer clips to nothing.
#[test]
fn a_clipping_layer_shows_only_over_the_layer_below() {
    let mut h = Harness::new();
    let below = h.doc().active_layer().expect("the workspace layer");
    let base = h.rect("base", 0.0, 0.0, 100.0, 100.0);
    h.assign(base, below);

    let top = h.layer("top");
    let over = h.rect("over", 50.0, 50.0, 100.0, 100.0);
    h.assign(over, top);
    let before = path_to_svg_data(&primitive_to_path(&h.scene.get(over).unwrap().primitive));

    h.set_clipping_mask(top, true);
    let changed = h.run_clip(&h.node_ids());
    assert!(
        changed.contains(&over),
        "the clipped node must be reported changed"
    );

    let clipped = h.region(over);
    let intersection = boolean(
        &rect_region(50.0, 50.0, 100.0, 100.0),
        &rect_region(0.0, 0.0, 100.0, 100.0),
        BooleanOp::Intersect,
    );
    assert!(
        close(region_area(&clipped), region_area(&intersection)),
        "a clipping layer must show exactly region(node) ∩ region(below)"
    );
    // The mask layer keeps its own geometry: it is the mask, not a victim of it.
    assert!(close(region_area(&h.region(base)), 10_000.0));
    assert_ne!(
        path_to_svg_data(&primitive_to_path(&h.scene.get(over).unwrap().primitive)),
        before
    );

    // A **hidden** layer below is not on the canvas, so it lends no alpha: the
    // clipping layer has nothing to show over.
    h.engine
        .dispatch(Command::SetLayerVisible {
            id: below,
            visible: false,
        })
        .expect("hide");
    h.refresh();
    h.run_clip(&[]);
    assert!(
        region_area(&h.region(over)) < 1e-6,
        "nothing below to clip to means nothing shows"
    );
}

/// **The bottom layer clips to nothing**: `LayerRegistry::below` is `None`, and
/// the eye disabled in the panel is this fact, not an opinion.
#[test]
fn the_bottom_layer_has_nothing_to_clip_to() {
    let mut h = Harness::new();
    let bottom = h.doc().active_layer().expect("layer");
    let only = h.rect("only", 10.0, 10.0, 50.0, 50.0);
    h.assign(only, bottom);
    assert!(h.doc().layers.below(&bottom).is_none());
    h.set_clipping_mask(bottom, true);
    h.run_clip(&h.node_ids());
    assert!(region_area(&h.region(only)) < 1e-6);
}

/// **The Non-Destructive Law.** Clearing the flag asks the evaluator for the
/// document's own geometry again — which is why the artwork comes back exactly,
/// rather than approximately from a saved copy.
#[test]
fn clearing_the_mask_restores_the_document_geometry() {
    let mut h = Harness::new();
    let below = h.doc().active_layer().expect("layer");
    let base = h.rect("base", 0.0, 0.0, 40.0, 40.0);
    h.assign(base, below);

    let top = h.layer("top");
    let over = h.rect("over", 20.0, 20.0, 80.0, 80.0);
    h.assign(over, top);
    let original = path_to_svg_data(&primitive_to_path(&h.scene.get(over).unwrap().primitive));

    h.set_clipping_mask(top, true);
    h.run_clip(&h.node_ids());
    assert!(region_area(&h.region(over)) < region_area(&rect_region(20.0, 20.0, 80.0, 80.0)));

    h.set_clipping_mask(top, false);
    let restored = h.clip.pending_restores(h.doc());
    assert!(
        restored.contains(&over),
        "the pass must know what to restore"
    );
    h.refresh();
    h.run_clip(&restored);
    assert_eq!(
        path_to_svg_data(&primitive_to_path(&h.scene.get(over).unwrap().primitive)),
        original,
        "the document's geometry must come back byte for byte"
    );
}

/// **The Non-Compounding Law.** A live mask re-clips from the node's *baseline*,
/// so growing the mask lets the artwork back in. Deriving from the previous
/// clip would make the second pass `region(B) ∩ A_old ∩ A_new` — this is the
/// test that fails when the baseline map is removed.
#[test]
fn a_growing_mask_lets_the_artwork_back_in() {
    let mut h = Harness::new();
    let below = h.doc().active_layer().expect("layer");
    let base = h.rect("base", 0.0, 0.0, 40.0, 40.0);
    h.assign(base, below);

    let top = h.layer("top");
    let over = h.rect("over", 0.0, 0.0, 100.0, 100.0);
    h.assign(over, top);
    h.set_clipping_mask(top, true);
    h.run_clip(&h.node_ids());
    let first = region_area(&h.region(over));
    assert!(
        close(first, 1_600.0),
        "0..40 ∩ 0..100 is the 40×40 corner, got {first}"
    );

    // The mask grows, on both axes — in the scene, without the clipped node being
    // re-evaluated. A live drag of the layer *below* is exactly this: its nodes
    // re-evaluate, the clipping layer's nodes do not, and the clipped node's
    // cached primitive is still pass 1's 40×40 corner.
    for property in ["width", "height"] {
        h.engine
            .dispatch(Command::SetParameter {
                node_id: base,
                property: property.to_string(),
                value: vectra_core::ParamValue::float_literal(100.0),
            })
            .expect("resize the mask");
    }
    h.refresh_nodes(&[base]);
    assert!(
        close(region_area(&h.region(base)), 10_000.0),
        "the mask node itself must have grown"
    );
    // `settle` hands the pass the ids the evaluator just re-derived — `base`,
    // never `over`. `over`'s baseline therefore survives untouched, which is what
    // makes the second pass `baseline(B) ∩ A_new` rather than `B ∩ A_old ∩ A_new`.
    h.run_clip(&[base]);
    let grown = region_area(&h.region(over));
    assert!(
        close(grown, 10_000.0),
        "the clip must re-derive from the baseline, no bigger and no smaller: the \
         mask is now the whole 100×100, and the clipped node must be too — got {grown}"
    );
    assert!(
        grown > first,
        "a growing mask has to let the artwork back in ({first} → {grown})"
    );
}

/// **Idempotence.** Two passes with nothing re-evaluated leave the scene exactly
/// as it was — the pass reports no changes the second time.
#[test]
fn a_second_pass_without_an_edit_changes_nothing() {
    let mut h = Harness::new();
    let below = h.doc().active_layer().expect("layer");
    let base = h.rect("base", 0.0, 0.0, 60.0, 60.0);
    h.assign(base, below);
    let top = h.layer("top");
    let over = h.rect("over", 30.0, 30.0, 90.0, 90.0);
    h.assign(over, top);
    h.set_clipping_mask(top, true);
    h.run_clip(&h.node_ids());
    let settled = path_to_svg_data(&primitive_to_path(&h.scene.get(over).unwrap().primitive));
    let changed = h.run_clip(&[]);
    assert!(
        changed.is_empty(),
        "a settled mask re-reports a change: {changed:?}"
    );
    assert_eq!(
        path_to_svg_data(&primitive_to_path(&h.scene.get(over).unwrap().primitive)),
        settled
    );
}

// ── helpers ──────────────────────────────────────────────────────────────

/// A rectangle as a lyon path — the probe a drawn draft becomes.
fn rect_path(x: f64, y: f64, w: f64, h: f64) -> lyon::path::Path {
    let mut builder = lyon::path::Builder::new();
    builder.begin(lyon::math::point(x as f32, y as f32));
    builder.line_to(lyon::math::point((x + w) as f32, y as f32));
    builder.line_to(lyon::math::point((x + w) as f32, (y + h) as f32));
    builder.line_to(lyon::math::point(x as f32, (y + h) as f32));
    builder.close();
    builder.build()
}

/// The same rectangle as a region.
fn rect_region(x: f64, y: f64, w: f64, h: f64) -> geo::MultiPolygon<f64> {
    use geo::{Coord, LineString, MultiPolygon, Polygon};
    let ring = LineString::new(vec![
        Coord { x, y },
        Coord { x: x + w, y },
        Coord { x: x + w, y: y + h },
        Coord { x, y: y + h },
        Coord { x, y },
    ]);
    MultiPolygon::new(vec![Polygon::new(ring, Vec::new())])
}
