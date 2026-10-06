//! Render-ready evaluated scene (Task 1.3, MES §13 input side).
//!
//! [`EvaluatedScene`] is the flat handoff between the parametric engine and
//! the renderer: every [`vectra_core::Parameter`] is resolved, every path is a
//! built [`lyon::path::Path`], and draw order is explicit. The renderer
//! traverses this — never the [`vectra_core::Document`] graph.
//!
//! ```text
//! Document ──[GeometryEvaluator]──▶ EvaluatedScene ──[Tessellator]──▶ RenderScene ──▶ Screen
//!   (parametric)      (this crate)      (concrete)        (vectra-render)
//! ```

use crate::evaluator::DirtySet;
use crate::paint::{EvaluatedAppearance, EvaluatedAppearanceKind, EvaluatedPaint};
use std::collections::HashMap;
use vectra_core::{Color, Document, NodeId};

/// A fully-resolved drawable primitive. All values are finite `f64`s inside
/// the GPU-representable range (see [`crate::evaluator::is_renderable`]).
#[derive(Debug, Clone)]
pub enum EvaluatedPrimitive {
    Rect {
        x: f64,
        y: f64,
        w: f64,
        h: f64,
        /// Rounded-corner radius, clamped to `[0, min(w,h)/2]`.
        ///
        /// Deliberate extension of the MES sketch: `NodeKind::Rectangle`
        /// carries `corner_radius`, and dropping it here would silently lose
        /// author intent one tessellation step later.
        corner_radius: f64,
    },
    Circle {
        cx: f64,
        cy: f64,
        r: f64,
    },
    Arc {
        cx: f64,
        cy: f64,
        r: f64,
        /// Arc start, normalized to `[0, TAU)`.
        start_angle: f64,
        /// Arc end, expressed as `start_angle + sweep` with
        /// `sweep ∈ [0, TAU]`. `end_angle == start_angle` is an empty sweep;
        /// `end_angle == start_angle + TAU` is a full circle (see
        /// [`crate::angles::normalize_arc_angles`]).
        end_angle: f64,
    },
    /// A built lyon path: fully-resolved points, chained through
    /// [`lyon::path::Builder`]. Iterate with `path.iter()`.
    Path(lyon::path::Path),
}

impl EvaluatedPrimitive {
    /// Stable tag for diagnostics and snapshot diffing.
    pub fn tag(&self) -> &'static str {
        match self {
            Self::Rect { .. } => "Rect",
            Self::Circle { .. } => "Circle",
            Self::Arc { .. } => "Arc",
            Self::Path(_) => "Path",
        }
    }

    /// Sweep of an arc in `[0, TAU]`, or `None` for non-arcs.
    pub fn arc_sweep(&self) -> Option<f64> {
        match self {
            Self::Arc {
                start_angle,
                end_angle,
                ..
            } => Some(end_angle - start_angle),
            _ => None,
        }
    }
}

/// Fully-resolved style: the node's **paint stack** plus its global opacity.
///
/// Unresolvable channels fall back to the [`vectra_core::StyleProperties`]
/// defaults (never skip the node for style) — a gradient whose frame reads an
/// unresolvable expression becomes a solid of the fallback colour with a
/// diagnostic, rather than a hole in the drawing.
///
/// The stack is never empty: a node always paints *something*, even if that
/// something is a fully transparent fill, because "no paint at all" would make a
/// selectable-but-invisible shape whose click behaviour nobody can explain.
#[derive(Debug, Clone, PartialEq)]
pub struct EvaluatedStyle {
    pub appearances: Vec<EvaluatedAppearance>,
    /// Clamped to `[0, 1]`. The node's global opacity, applied *on top of* each
    /// layer's own — that is the design-tool meaning of the two controls, and it
    /// is why hiding is not the same as setting this to zero.
    pub opacity: f64,
}

impl Default for EvaluatedStyle {
    fn default() -> Self {
        Self {
            appearances: vec![EvaluatedAppearance {
                kind: EvaluatedAppearanceKind::Fill,
                paint: EvaluatedPaint::Solid(Color::rgb(0x22, 0x66, 0xee)),
                opacity: 1.0,
                blend: vectra_core::BlendMode::Normal,
                visible: true,
            }],
            opacity: 1.0,
        }
    }
}

impl EvaluatedStyle {
    /// A style from a flat fill/stroke pair — the shorthand every test and every
    /// legacy caller wants, now that the canonical shape is a stack.
    pub fn solid(fill: Color, stroke: Color, stroke_width: f64, opacity: f64) -> Self {
        let mut appearances = vec![EvaluatedAppearance {
            kind: EvaluatedAppearanceKind::Fill,
            paint: EvaluatedPaint::Solid(fill),
            opacity: 1.0,
            blend: vectra_core::BlendMode::Normal,
            visible: true,
        }];
        if stroke_width > 0.0 && stroke.a > 0 {
            appearances.push(EvaluatedAppearance {
                kind: EvaluatedAppearanceKind::Stroke {
                    width: stroke_width,
                },
                paint: EvaluatedPaint::Solid(stroke),
                opacity: 1.0,
                blend: vectra_core::BlendMode::Normal,
                visible: true,
            });
        }
        Self {
            appearances,
            opacity,
        }
    }

    /// The first fill layer's paint, if the node has one. The compatibility door
    /// for consumers that predate stacked paint (the SVG exporter's fast path, the
    /// inspector's swatch, the AI's colour question).
    pub fn first_fill(&self) -> Option<&EvaluatedAppearance> {
        self.appearances.iter().find(|layer| !layer.is_stroke())
    }

    /// Every stroke layer, in draw order.
    pub fn strokes(&self) -> impl Iterator<Item = &EvaluatedAppearance> {
        self.appearances.iter().filter(|layer| layer.is_stroke())
    }

    /// The first stroke layer, if any.
    pub fn first_stroke(&self) -> Option<&EvaluatedAppearance> {
        self.strokes().next()
    }

    /// The widest stroke's width — what the geometry-key cache uses when a node's
    /// stroke set changes shape.
    pub fn max_stroke_width(&self) -> f64 {
        self.strokes()
            .filter_map(|layer| layer.stroke_width())
            .fold(0.0, f64::max)
    }

    /// How many draw items this node produces. The renderer allocates exactly
    /// this many instance rows, so it is also the cheap "did the stack change
    /// shape" fingerprint the incremental cache compares.
    pub fn drawable_count(&self) -> usize {
        self.appearances
            .iter()
            .filter(|layer| layer.is_visible())
            .count()
    }
}

impl EvaluatedNode {
    /// A node with default presentation: visible and unlocked. The shorthand the
    /// renderer's law tests use to build fixtures by hand.
    pub fn new(id: NodeId, primitive: EvaluatedPrimitive, style: EvaluatedStyle) -> Self {
        Self {
            id,
            primitive,
            style,
            visible: true,
            locked: false,
        }
    }
}

/// One drawable: identity + resolved geometry + resolved style + presentation.
#[derive(Debug, Clone)]
pub struct EvaluatedNode {
    pub id: NodeId,
    pub primitive: EvaluatedPrimitive,
    pub style: EvaluatedStyle,
    /// Effective visibility: the node's own eye **and** its layer's (Task 10.2
    /// RULE 4). A hidden node stays in the scene — it is a flag, not a deletion —
    /// and the renderer simply does not draw it.
    pub visible: bool,
    /// Effective lock: its own padlock **or** its layer's. A locked node draws
    /// normally and refuses to be picked (see `HitIndex`).
    pub locked: bool,
}

/// Flat, render-ready scene.
///
/// Invariants (maintained by [`crate::evaluator::GeometryEvaluator`] and
/// [`EvaluatedScene::apply_partial`]):
/// * every id in `z_order` is present in `nodes`;
/// * `z_order` follows [`Document::order`] (back → front);
/// * every stored scalar is [`crate::evaluator::is_renderable`].
#[derive(Debug, Clone, Default)]
pub struct EvaluatedScene {
    pub nodes: HashMap<NodeId, EvaluatedNode>,
    pub z_order: Vec<NodeId>,
}

impl EvaluatedScene {
    pub fn empty() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    pub fn get(&self, id: NodeId) -> Option<&EvaluatedNode> {
        self.nodes.get(&id)
    }

    /// Nodes in draw order (back → front).
    pub fn in_z_order(&self) -> impl Iterator<Item = &EvaluatedNode> {
        self.z_order.iter().filter_map(|id| self.nodes.get(id))
    }

    /// Debug assertion for the scene invariants.
    pub fn debug_assert_invariants(&self) {
        #[cfg(debug_assertions)]
        {
            for id in &self.z_order {
                debug_assert!(
                    self.nodes.contains_key(id),
                    "z_order references missing node {id}"
                );
            }
        }
    }

    /// Patch the **virtual operation layer** into the scene (Task 4.0).
    ///
    /// Operations are composed on top of the primitive scene: each id in
    /// `updates` replaces (or inserts) that virtual node, then every cached id
    /// that is no longer live geometry — a removed operation, a disabled one, a
    /// deleted node — is dropped, and `z_order` is rebuilt as the document's
    /// node order followed by the **operation order**, so a result always draws
    /// above the sources it reads.
    ///
    /// Ids that are live both before and after and were not in `updates` are
    /// left exactly as they were: this is an incremental patch, not a rebuild.
    pub fn apply_operations(
        &mut self,
        doc: &Document,
        updates: impl IntoIterator<Item = EvaluatedNode>,
    ) {
        for node in updates {
            self.nodes.insert(node.id, node);
        }
        // Forget everything that is no longer live geometry: deleted nodes and
        // removed/disabled operations both leave the cache this way.
        self.nodes.retain(|id, _| doc.is_geometry_id(*id));
        self.z_order = doc
            .geometry_ids()
            .into_iter()
            .filter(|id| self.nodes.contains_key(id))
            .collect();
        self.debug_assert_invariants();
    }

    /// Drop these ids from the scene (Task 7.0).
    ///
    /// Needed because a *live* id can still have no geometry this pass: a
    /// procedural node that is registered and enabled but whose evaluation
    /// failed (a missing input, an unresolvable operand) must stop drawing the
    /// last shape it managed, or the failure would be invisible. The operations
    /// composer cannot do this — it prunes by *liveness*, and this node is live.
    pub fn retire(&mut self, ids: &[NodeId]) {
        if ids.is_empty() {
            return;
        }
        for id in ids {
            self.nodes.remove(id);
        }
        self.z_order.retain(|id| !ids.contains(id));
        self.debug_assert_invariants();
    }

    /// Incrementally patch `self` with a partial evaluation (Phase-4 path).
    ///
    /// For every id in `dirty`: ids present in `partial` are inserted/updated,
    /// ids absent are removed (deleted from the document, or newly skipped
    /// with a diagnostic). `z_order` is rebuilt from the live document order
    /// filtered to surviving nodes, so moves/reorders are picked up even
    /// though they are not "dirty values".
    pub fn apply_partial(&mut self, doc: &Document, partial: EvaluatedScene, dirty: &DirtySet) {
        if dirty.is_full() {
            *self = partial;
            return;
        }
        for id in dirty.ids() {
            match partial.nodes.get(id) {
                Some(node) => {
                    self.nodes.insert(*id, node.clone());
                }
                None => {
                    self.nodes.remove(id);
                }
            }
        }
        // The draw order is re-derived from the document. See
        // [`EvaluatedScene::resync_draw_order`] for why it is `geometry_ids()` and
        // not `order`, and why this is *unconditional* rather than keyed on the
        // dirty set.
        self.resync_draw_order(doc);
        self.debug_assert_invariants();
    }

    /// Re-derive `z_order` from the live document, returning whether it moved.
    ///
    /// Two things make this a function of its own rather than three lines inside
    /// `apply_partial`:
    ///
    /// * **It is not a value change.** Reordering layers, assigning a node to a
    ///   layer and moving a group all rearrange the picture while dirtying
    ///   nothing — the dependency graph is right to stay silent (a reorder
    ///   resolves no parameter), so the *order* has to be re-derived by whoever
    ///   notices the mutation, not by whoever re-evaluates a value.
    /// * **The order comes from `geometry_ids()`, exactly as the composer's does**
    ///   — NOT from the authored `order`. Virtual geometry (operations,
    ///   procedural results) has an id in that wider list and no entry in
    ///   `doc.order`, so deriving the order from `doc.order` would drop a boolean
    ///   or a procedural result out of `z_order` on the next patch: still cached,
    ///   still live, no longer drawn. Two functions that both claim to produce
    ///   the scene's order must ask the same question.
    pub fn resync_draw_order(&mut self, doc: &Document) -> bool {
        let desired: Vec<NodeId> = doc
            .geometry_ids()
            .into_iter()
            .filter(|id| self.nodes.contains_key(id))
            .collect();
        if self.z_order == desired {
            return false;
        }
        self.z_order = desired;
        true
    }

    /// **RULE 4**: re-derive every node's presentation flags from the document,
    /// touching no geometry, no parameter and no cache key.
    ///
    /// An eye or a padlock never reaches the evaluator (`dirty_ids_for_events`
    /// ignores `NodeFlagsChanged` and `LayersUpdated`), so a toggle would
    /// otherwise leave the *cached* scene claiming a hidden node is visible. The
    /// boundary calls this instead — one `bool` copy per node, and the returned
    /// ids are exactly the nodes whose picture changes, which is the whole
    /// message the renderer needs:
    ///
    /// ```text
    ///   eye / padlock ──▶ refresh_presentation ──▶ renderer repaint, 0 bytes written
    ///   any value edit ──▶ Dirty ──▶ evaluate ──▶ patch ──▶ buffer writes
    /// ```
    pub fn refresh_presentation(&mut self, doc: &Document) -> Vec<NodeId> {
        let mut changed = Vec::new();
        for (id, node) in self.nodes.iter_mut() {
            // **A virtual node is not the document's to flag.** An operation
            // result, a procedural result and a `Source` node are *scene* nodes
            // with no entry in `doc.nodes` (Tasks 4.0 and 7.0: one id space,
            // their own registries), and `Document::visible` answers `false` for
            // an id it does not own — so re-deriving their flags here would
            // hide every computed shape on the next settle. They are skipped,
            // and their presentation stays what the pass that composed them
            // decided: a disabled operation is *pruned*, not hidden.
            if !doc.nodes.contains_key(id) {
                continue;
            }
            let visible = doc.visible(*id);
            let locked = doc.locked(*id);
            if node.visible != visible || node.locked != locked {
                node.visible = visible;
                node.locked = locked;
                changed.push(*id);
            }
        }
        changed.sort();
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect_node(id: NodeId, w: f64) -> EvaluatedNode {
        EvaluatedNode::new(
            id,
            EvaluatedPrimitive::Rect {
                x: 0.0,
                y: 0.0,
                w,
                h: 10.0,
                corner_radius: 0.0,
            },
            EvaluatedStyle::default(),
        )
    }

    #[test]
    fn z_order_iteration_follows_scene_order() {
        let a = vectra_core::new_node_id();
        let b = vectra_core::new_node_id();
        let scene = EvaluatedScene {
            nodes: [(a, rect_node(a, 1.0)), (b, rect_node(b, 2.0))]
                .into_iter()
                .collect(),
            z_order: vec![b, a],
        };
        let widths: Vec<f64> = scene
            .in_z_order()
            .map(|n| match n.primitive {
                EvaluatedPrimitive::Rect { w, .. } => w,
                _ => unreachable!(),
            })
            .collect();
        assert_eq!(widths, vec![2.0, 1.0]);
    }

    #[test]
    fn arc_sweep_is_end_minus_start() {
        let p = EvaluatedPrimitive::Arc {
            cx: 0.0,
            cy: 0.0,
            r: 5.0,
            start_angle: 1.0,
            end_angle: 2.5,
        };
        assert_eq!(p.arc_sweep(), Some(1.5));
        assert_eq!(
            rect_node(vectra_core::new_node_id(), 1.0)
                .primitive
                .arc_sweep(),
            None
        );
    }
}
