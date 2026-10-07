//! Task 10.7 RULE 3 — Alpha Lock and Clipping Masks, as geometry.
//!
//! Both features are *layer flags*, but both are answered with set arithmetic:
//!
//! * **Alpha Lock** (`LayerRecord::alpha_locked`) constrains **new** artwork to
//!   the layer's existing content. The scene is not reshaped at all — the flag is
//!   read at the drawing boundary, where a committed stroke is intersected with
//!   the layer's region before it is stored ([`content_region`] is the region
//!   that boundary uses; the intersection itself is [`clip_region`], reached
//!   through [`clipped_path`]).
//! * **Clipping Mask** (`LayerRecord::clipping_mask`) makes a layer show only
//!   where it overlaps the layer below. That *is* a scene reshape, and
//!   [`ClipState::apply`] performs it live: every node on a clipping layer keeps
//!   its primitive replaced by `region(node) ∩ region(layer below)`.
//!
//! ```text
//! mask   = ⋃ { region(n) | n ∈ layer below, n visible }        mask_region
//! shown  = region(node) ∩ mask                                 clip_region
//! ```
//!
//! # Why a baseline map
//!
//! A clipped node's primitive is *derived*, and deriving a derived value again is
//! how a clip turns into a shrink: `clip(clip(x, M₁), M₂) ≠ clip(x, M₂)`. The
//! [`ClipState`] therefore remembers, per clipped node, the primitive the
//! **document** produced (the baseline) and recomputes `baseline ∩ mask` from
//! scratch on every pass. A baseline is refreshed only when the evaluator
//! actually re-derived that node (the `reevaluated` slice) — the one moment the
//! document's answer for it changed.
//!
//! Nothing here reads a layer flag twice or guesses: [`ClipState::plan`] walks the
//! layer registry in order (back → front), so a clipped layer is already clipped
//! when the layer above it reads it as a mask — Procreate's stacked-mask
//! behaviour falls out of the loop order instead of out of a special case.

use std::collections::{BTreeSet, HashMap};

use geo::{Area, MultiPolygon};
use vectra_core::{BooleanOp, Document, NodeId};
use vectra_geometry::paths::{path_to_svg_data, primitive_to_path};
use vectra_geometry::scene::{EvaluatedPrimitive, EvaluatedScene};

use crate::convert::{multi_polygon_to_path, path_to_multi_polygon, region_area};
use crate::ops::boolean;

/// An empty region — the answer when there is nothing to show, and the identity
/// element for union.
pub fn empty_region() -> MultiPolygon<f64> {
    MultiPolygon::new(Vec::new())
}

/// The region a single evaluated primitive covers.
///
/// Flattening a curve to a polygon is the only lossy step in the whole file, and
/// it is the same step the boolean operations already take, at the same
/// tolerance: one tessellation policy for the operations and the clipping masks
/// both.
pub fn region_of(primitive: &EvaluatedPrimitive) -> MultiPolygon<f64> {
    path_to_multi_polygon(&primitive_to_path(primitive)).unwrap_or_else(empty_region)
}

/// The union of several regions.
fn union_all(regions: impl Iterator<Item = MultiPolygon<f64>>) -> MultiPolygon<f64> {
    regions
        .reduce(|a, b| boolean(&a, &b, BooleanOp::Union))
        .unwrap_or_else(empty_region)
}

/// **The mask of a layer**: the union of its *visible* scene nodes' regions.
///
/// `visible` here is the effective flag the evaluator derived (the node's own eye
/// **and** its layer's), so a hidden shape is not part of the alpha it lends to
/// the layer above it — the mask is what is actually on the canvas, which is the
/// only thing a designer can see to clip against.
pub fn mask_region(scene: &EvaluatedScene, ids: &[NodeId]) -> MultiPolygon<f64> {
    union_all(
        ids.iter()
            .filter_map(|id| scene.get(*id))
            .filter(|node| node.visible)
            .map(|node| region_of(&node.primitive)),
    )
}

/// **Alpha Lock's content region**: what a layer already holds.
///
/// Deliberately the same union as [`mask_region`] (visible nodes only): "the
/// layer's existing `EvaluatedScene` bounds" is the alpha a designer sees, and
/// drawing on an alpha-locked layer is drawing inside exactly that.
pub fn content_region(scene: &EvaluatedScene, ids: &[NodeId]) -> MultiPolygon<f64> {
    mask_region(scene, ids)
}

/// The region a set of scene nodes covers, visible or not — for tests and
/// callers that want the raw footprint.
pub fn region_of_nodes(scene: &EvaluatedScene, ids: &[NodeId]) -> MultiPolygon<f64> {
    union_all(
        ids.iter()
            .filter_map(|id| scene.get(*id))
            .map(|node| region_of(&node.primitive)),
    )
}

/// **The clip itself**: `region ∩ mask`.
pub fn clip_region(region: &MultiPolygon<f64>, mask: &MultiPolygon<f64>) -> MultiPolygon<f64> {
    boolean(region, mask, BooleanOp::Intersect)
}

/// The clipped primitive for a node: its baseline region intersected with the
/// mask, as a path.
///
/// A node clipped to nothing keeps its place in the scene with an **empty** path
/// rather than being deleted: the scene cache's invariant is that every node the
/// document owns is present (`EvaluatedScene::debug_assert_invariants`), and a
/// hidden-by-mask node must still be *there* for the day the mask grows.
pub fn clipped_primitive(
    primitive: &EvaluatedPrimitive,
    mask: &MultiPolygon<f64>,
) -> EvaluatedPrimitive {
    let region = clip_region(&region_of(primitive), mask);
    EvaluatedPrimitive::Path(multi_polygon_to_path(&region))
}

/// **The alpha-lock boundary, for a drawn path.**
///
/// Returns the largest surviving piece of `path ∩ content` as a new path, or
/// `None` when nothing of the path lies inside the existing content — the honest
/// "this stroke has nowhere to land" the caller reports to the designer, instead
/// of a silently empty stroke that looks like a bug.
///
/// One piece rather than several, and exterior rings only: a node's geometry is a
/// single chain (`NodeKind::Path { start, segments }`), so an intersection that
/// falls into two islands has to be represented by the piece that carries the
/// stroke. This is a **fidelity limit of the path model**, not of the clipping
/// math — [`clip_region`] returns every ring.
pub fn clipped_path(
    path: &lyon::path::Path,
    content: &MultiPolygon<f64>,
) -> Option<lyon::path::Path> {
    let region = path_to_multi_polygon(path)?;
    let clipped = clip_region(&region, content);
    let best = clipped
        .0
        .iter()
        .max_by(|a, b| {
            let (a, b) = (a.unsigned_area(), b.unsigned_area());
            a.partial_cmp(&b).unwrap_or(std::cmp::Ordering::Equal)
        })
        .filter(|polygon| polygon.unsigned_area() > 0.0)?;
    let mut single = MultiPolygon::new(Vec::new());
    single.0.push(best.clone());
    Some(multi_polygon_to_path(&single))
}

/// The area a path encloses — the caller's way of asking "is anything left?"
/// without re-running the clip.
pub fn enclosed_area(path: &lyon::path::Path) -> f64 {
    path_to_multi_polygon(path).map_or(0.0, |region| region_area(&region))
}

/// Is this point inside the region?
///
/// The point-in-polygon test the *stroke* clip needs: an open path has no area,
/// so a boolean intersection cannot describe it — what a hand draws with the pen
/// is a **curve**, and the curve is inside or outside the layer's artwork at each
/// of its points.
pub fn contains_point(region: &MultiPolygon<f64>, x: f64, y: f64) -> bool {
    use geo::Contains;
    region.contains(&geo::Point::new(x, y))
}

/// **The 1-D clip**: the runs of a polyline that lie inside a region.
///
/// Alpha lock's answer for artwork with **no area** — an open pen path, a line.
/// The alternative (intersecting nothing with something) would erase every open
/// stroke drawn on a locked layer, which is not what "constrained to the layer's
/// bounds" means to the person holding the pen: the ink is where the curve is,
/// and the curve's inside-the-artwork stretches are the ink that survives.
///
/// Each run is returned in stroke order; a run of a single point cannot describe
/// a segment and is dropped by the caller's own "nothing to commit" rule.
pub fn clip_polyline(points: &[(f64, f64)], content: &MultiPolygon<f64>) -> Vec<Vec<(f64, f64)>> {
    let mut runs: Vec<Vec<(f64, f64)>> = Vec::new();
    let mut run: Vec<(f64, f64)> = Vec::new();
    for point in points {
        if contains_point(content, point.0, point.1) {
            run.push(*point);
        } else if !run.is_empty() {
            runs.push(std::mem::take(&mut run));
        }
    }
    if !run.is_empty() {
        runs.push(run);
    }
    runs
}

/// The live clipping state of one engine: baselines, the plan, and what the scene
/// currently shows.
///
/// Held by the engine (one per document) rather than by the scene cache, because
/// it is *bookkeeping about a derivation*, not part of the evaluated scene.
#[derive(Debug, Default, Clone)]
pub struct ClipState {
    /// What the document said the last time each clipped node was evaluated.
    baseline: HashMap<NodeId, EvaluatedPrimitive>,
    /// The ids the previous pass clipped — the plan, remembered so a node that
    /// leaves it can be restored.
    live: BTreeSet<NodeId>,
    /// A canonical rendering of what was last written, so a pass reports only
    /// real changes to the renderer's ledger.
    shown: HashMap<NodeId, String>,
}

/// What one clip pass did.
#[derive(Debug, Default, Clone)]
pub struct ClipReport {
    /// Ids whose primitive this pass rewrote — the renderer has to hear about
    /// them (they are otherwise not in the pass's `Dirty` event).
    pub changed: Vec<NodeId>,
    /// Ids now being clipped (the plan), in document order.
    pub masked: Vec<NodeId>,
}

impl ClipState {
    pub fn new() -> Self {
        Self::default()
    }

    /// **The plan**: for every clipping layer (back → front), the nodes to clip
    /// and the region they clip to.
    ///
    /// The mask is read from the *scene*, so it is the geometry with every
    /// effect applied — including a lower clipping layer's own mask.
    pub fn plan(&self, doc: &Document, scene: &EvaluatedScene) -> Vec<(NodeId, MultiPolygon<f64>)> {
        let mut plan = Vec::new();
        for layer in doc.layers.iter() {
            if !layer.clipping_mask {
                continue;
            }
            let mask = match doc.layers.below(&layer.id) {
                // A layer is asked for its *own* eye before it lends an alpha: a
                // hidden layer is not on the canvas, so it masks nothing away.
                Some(below) if below.visible => mask_region(scene, &below.children),
                _ => empty_region(),
            };
            for child in &layer.children {
                plan.push((*child, mask.clone()));
            }
        }
        plan
    }

    /// The layer whose alpha governs new artwork (the active layer), when it is
    /// alpha-locked.
    pub fn locked_layer(doc: &Document) -> Option<&vectra_core::LayerRecord> {
        let active = doc.active_layer()?;
        doc.layers.get(&active).filter(|layer| layer.alpha_locked)
    }

    /// Ids that must be re-evaluated before the next pass: the ones that were
    /// clipped and no longer are.
    ///
    /// This is the restore path, and it is what makes a clip *non-destructive*:
    /// the flag goes off, the node's derivative is dropped, the evaluator is asked
    /// for the document's own answer again, and the original geometry is back —
    /// byte for byte, because it is the same evaluation that produced it.
    pub fn pending_restores(&self, doc: &Document) -> Vec<NodeId> {
        // The ids alone are enough here: this runs *before* the pass, and asking
        // for regions would mean asking the scene for geometry the pass is about
        // to replace.
        let now: BTreeSet<NodeId> = doc
            .layers
            .iter()
            .filter(|layer| layer.clipping_mask)
            .flat_map(|layer| layer.children.iter().copied())
            .collect();
        self.live.difference(&now).copied().collect::<Vec<NodeId>>()
    }

    /// **Run the clip pass.**
    ///
    /// `reevaluated` is the set the evaluator just re-derived from the document;
    /// those baselines are refreshed. Every other clipped node keeps the baseline
    /// it had — which is what keeps a live mask from compounding.
    pub fn apply(
        &mut self,
        scene: &mut EvaluatedScene,
        doc: &Document,
        reevaluated: &[NodeId],
    ) -> ClipReport {
        let plan = self.plan(doc, scene);
        let live: BTreeSet<NodeId> = plan.iter().map(|(id, _)| *id).collect();

        let mut changed = Vec::new();
        for (id, mask) in &plan {
            let Some(node) = scene.nodes.get_mut(id) else {
                continue;
            };
            if reevaluated.contains(id) || !self.baseline.contains_key(id) {
                self.baseline.insert(*id, node.primitive.clone());
            }
            let Some(baseline) = self.baseline.get(id) else {
                continue;
            };
            let clipped = clipped_primitive(baseline, mask);
            let rendered = match &clipped {
                EvaluatedPrimitive::Path(path) => path_to_svg_data(path),
                other => other.tag().to_string(),
            };
            let rewritten = self.shown.get(id) != Some(&rendered);
            node.primitive = clipped;
            if rewritten {
                self.shown.insert(*id, rendered);
                changed.push(*id);
            }
        }

        // Bookkeeping: a node that left the plan is no longer derived, so it
        // keeps neither a baseline nor a rendering of ours.
        for stale in self.live.difference(&live).collect::<Vec<_>>() {
            self.baseline.remove(stale);
            self.shown.remove(stale);
        }
        self.live = live;
        changed.sort();
        changed.dedup();
        ClipReport {
            changed,
            masked: plan.into_iter().map(|(id, _)| id).collect(),
        }
    }

    /// Is anything clipped at all? (The engine skips the pass's work when not.)
    pub fn is_idle(&self, doc: &Document) -> bool {
        self.live.is_empty() && !doc.layers.iter().any(|layer| layer.clipping_mask)
    }

    /// The plan's ids, for a caller that only wants to know *who* is clipped.
    pub fn masked_ids(&self) -> Vec<NodeId> {
        self.live.iter().copied().collect()
    }

    /// Forget everything — a document swap: the ids mean nothing any more.
    pub fn reset(&mut self) {
        self.baseline.clear();
        self.live.clear();
        self.shown.clear();
    }
}
