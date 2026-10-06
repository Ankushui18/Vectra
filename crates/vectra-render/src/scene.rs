//! `RenderScene` — the CPU-side truth of what the GPU holds (Task 5.0 §2).
//!
//! The renderer's contract with the engine is one method:
//!
//! ```text
//! RenderScene::sync(&mut self, scene: &EvaluatedScene, dirty: &DirtySet) -> &WritePlan
//!     │
//!     └──▶ WritePlan { ops: [Create, Vertices(Fill), Indices(Fill), Instance, …], removed, order }
//! ```
//!
//! `sync` decides *what changed* (geometry? placement? style? nothing?) and
//! records it as a plan of **surgical writes**; [`RenderScene::flush`] applies
//! that plan through a [`BufferSink`], which is either the real wgpu renderer or
//! the [`MockSink`] the law tests use to prove no other buffer was touched.
//!
//! # The three change classes
//!
//! | change | detected by | work |
//! |---|---|---|
//! | **resize/reshape** | `GeometryKey::shape` differs | re-tessellate → vertices + indices |
//! | **move** (drag, solver partner) | `GeometryKey::origin` differs | instance only (80 bytes) |
//! | **restyle** (colour, width, opacity, blend) | `EvaluatedStyle` differs | instance only (80 bytes per draw item) |
//! | **restack** (a layer added, removed, hidden) | the draw-item list differs | the rows that differ |
//! | **eye / padlock** (Task 10.2 RULE 4) | the node's flags differ | **nothing** |
//!
//! A node that is in the dirty set but changed in none of those ways costs *no
//! write at all* — which is what the Incremental Update Law asserts, and what
//! makes a pointer gesture cheap: the engine resolves the drag, the scene layer
//! rewrites 80 bytes per moved draw item, and the vertex buffers are never
//! touched.
//!
//! # Task 10.2: a node is a *list* of draws
//!
//! One node used to be one instance row and at most two meshes. With a paint
//! stack it is one row and one mesh per **draw item** — a fill or a stroke layer,
//! in stack order — so `NodeSlot::items` is the node's own draw list, and the
//! frame is the concatenation of every node's list. All of the incrementality
//! above survives that: rows are reused positionally, so adding a stroke layer
//! allocates one row and writes one row, and changing a colour rewrites exactly
//! one.

use std::collections::{BTreeMap, BTreeSet};

use vectra_core::{Color, NodeId};
use vectra_geometry::{DirtySet, EvaluatedNode, EvaluatedScene, EvaluatedStyle};

use crate::error::RenderError;
use crate::geometry::{Bounds, GeometryKey, Mesh};
use crate::hit::HitIndex;
use crate::instance::{InstanceRaw, RampTable};
use crate::tessellate::{tessellate_flattened, Tessellation};

/// No instance slot allocated yet.
pub const UNASSIGNED_SLOT: u32 = u32::MAX;

/// Which of a node's GPU buffers a write addresses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum WriteKind {
    /// Allocate (or reallocate) this node's buffers.
    Create,
    Vertices(MeshKind),
    Indices(MeshKind),
    /// One 80-byte instance row: this node, this draw item.
    Instance,
    /// The whole gradient stop array. One write per changed ramp, not one per
    /// stop and not one per gradient — the table is the unit the GPU reads.
    Ramps,
}

pub use crate::geometry::MeshKind;

impl WriteKind {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Create => "create",
            Self::Vertices(MeshKind::Fill) => "fill.vertices",
            Self::Vertices(MeshKind::Stroke(_)) => "stroke.vertices",
            Self::Indices(MeshKind::Fill) => "fill.indices",
            Self::Indices(MeshKind::Stroke(_)) => "stroke.indices",
            Self::Instance => "instance",
            Self::Ramps => "ramps",
        }
    }

    /// The stable wire name the UI and the smoke test assert on.
    pub fn tag(&self) -> &'static str {
        match self {
            Self::Create => "create",
            Self::Vertices(_) => "vertices",
            Self::Indices(_) => "indices",
            Self::Instance => "instance",
            Self::Ramps => "ramps",
        }
    }
}

/// One surgical write: *this* node, *this* buffer, `bytes` long.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WriteOp {
    pub id: NodeId,
    pub slot: u32,
    pub kind: WriteKind,
    pub bytes: usize,
}

/// What one `sync` decided, and why. Kept on the scene after the flush so the
/// UI (and the smoke test) can read what the last frame actually cost.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct WritePlan {
    pub ops: Vec<WriteOp>,
    pub removed: Vec<NodeId>,
    /// Draw order after this sync (back → front).
    pub order: Vec<NodeId>,
    /// The dirty set was `full` (cold cache / explicit rebuild).
    pub full: bool,
    /// The draw order changed (no buffer traffic — just iteration order).
    pub reordered: bool,
    /// Nodes whose *geometry* was re-tessellated.
    pub retessellated: Vec<NodeId>,
    /// Nodes that merely moved (instance-only, the drag fast path).
    pub moved: Vec<NodeId>,
    /// Nodes that merely restyled (instance-only).
    pub restyled: Vec<NodeId>,
    /// Nodes whose *presentation* changed — an eye or a padlock, node or layer
    /// (Task 10.2 RULE 4). These cost **no writes at all**: the next frame draws
    /// the same buffers through a shorter list, and the hit index skips them.
    pub repainted: Vec<NodeId>,
    /// The ramp table's size in bytes, when it was re-uploaded.
    pub ramps: Option<usize>,
}

impl WritePlan {
    /// Ids touched by at least one write, in plan order, deduplicated.
    pub fn touched(&self) -> Vec<NodeId> {
        let mut out: Vec<NodeId> = Vec::new();
        for op in &self.ops {
            if !out.contains(&op.id) {
                out.push(op.id);
            }
        }
        out
    }

    pub fn writes(&self, kind: WriteKind) -> Vec<NodeId> {
        self.ops
            .iter()
            .filter(|op| op.kind == kind)
            .map(|op| op.id)
            .collect()
    }

    pub fn bytes(&self) -> usize {
        self.ops.iter().map(|op| op.bytes).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.ops.is_empty() && self.removed.is_empty()
    }
}

/// Which mesh of a node a draw item uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DrawMesh {
    Fill,
    /// Index into `NodeSlot::strokes` — the stroke layer's position among the
    /// node's stroke layers, in stack order.
    Stroke(u32),
}

/// One drawable of one node: a paint stack entry, its mesh, and its instance row.
///
/// The item list *is* the node's draw order — back to front within the node —
/// which is what makes "two strokes, black under white" a data structure rather
/// than a special case (Appearance Law).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DrawItem {
    /// Index into `EvaluatedStyle::appearances`.
    pub appearance: u32,
    pub mesh: DrawMesh,
    /// The instance row holding this item's colours, opacity and blend mode.
    pub slot: u32,
    /// The blend code, mirrored so the renderer can group draws without reading
    /// the instance array back off the GPU.
    pub blend: f32,
}

impl DrawItem {
    pub fn needs_backdrop(&self) -> bool {
        self.blend != 0.0
    }
}

/// One node's CPU-side record: what the GPU is holding, and what it was built
/// from (so the next sync can tell what changed).
#[derive(Debug, Clone)]
pub struct NodeSlot {
    pub id: NodeId,
    /// The node's instance rows, one per draw item, in draw order.
    pub items: Vec<DrawItem>,
    /// Local rings (node frame) — the exact hit-test geometry, no allocation
    /// needed at query time: the query point is translated instead.
    pub local_rings: Vec<Vec<[f32; 2]>>,
    pub world_bounds: Bounds,
    pub key: GeometryKey,
    /// The stroke *tessellation* inputs: the width of every stroke layer, in
    /// stack order, with invisible or zero-width layers recorded as `0.0`.
    /// Changing any of them changes a stroke mesh, so it is a geometry change —
    /// not a style change, even though all of it lives in `EvaluatedStyle`.
    pub strokes_key: Vec<f32>,
    pub style: EvaluatedStyle,
    /// Effective presentation, copied from the evaluated node (RULE 4).
    pub visible: bool,
    pub locked: bool,
    pub fill: Mesh,
    /// One mesh per stroke layer, in stack order.
    pub strokes: Vec<Mesh>,
    /// One instance row per draw item, parallel to `items`.
    pub instances: Vec<InstanceRaw>,
}

/// The stroke tessellation key derived from a style: one entry per stroke layer.
fn strokes_key_of(style: &EvaluatedStyle) -> Vec<f32> {
    style
        .strokes()
        .map(|layer| {
            let width = layer.stroke_width().unwrap_or(0.0).max(0.0);
            // An invisible stroke has no width to apply, so its key collapses to
            // `0.0`: going from "1px transparent" to "no stroke" is then
            // correctly a no-op — the same normalization the single-stroke key
            // used to do.
            if width > 0.0 && layer.is_visible() {
                width as f32
            } else {
                0.0
            }
        })
        .collect()
}

impl NodeSlot {
    pub fn vertex_bytes(&self) -> usize {
        (self.fill.vertices.len() + self.strokes.iter().map(|m| m.vertices.len()).sum::<usize>())
            * 8
    }

    pub fn index_bytes(&self) -> usize {
        (self.fill.indices.len() + self.strokes.iter().map(|m| m.indices.len()).sum::<usize>()) * 4
    }

    /// Draws this node issues: one per draw item whose mesh is non-empty.
    pub fn draw_calls(&self) -> usize {
        self.items
            .iter()
            .filter(|item| !self.mesh(item.mesh).is_empty())
            .count()
    }

    pub fn mesh(&self, kind: DrawMesh) -> &Mesh {
        match kind {
            DrawMesh::Fill => &self.fill,
            DrawMesh::Stroke(index) => self
                .strokes
                .get(index as usize)
                .expect("draw item references a stroke mesh"),
        }
    }

    /// The instance rows this node owns, in draw order.
    pub fn slots(&self) -> impl Iterator<Item = u32> + '_ {
        self.items.iter().map(|item| item.slot)
    }
}

/// What the last sync did, in numbers/// What the last sync did, in numbers — the UI's renderer line and the tests'
/// observable.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct UpdateReport {
    pub nodes: usize,
    pub vertices: usize,
    pub indices: usize,
    pub instances: usize,
    pub created: usize,
    pub removed: usize,
    pub retessellated: usize,
    pub moved: usize,
    pub restyled: usize,
    pub writes: usize,
    pub bytes: usize,
    pub full: bool,
    /// Draw items the scene draws — the count the GPU's draw calls follow.
    pub draw_items: usize,
    /// Draw items that read the backdrop (a blend mode other than Normal).
    pub blended_draws: usize,
    pub touched: Vec<NodeId>,
}

/// The CPU-side mirror of the GPU buffers.
#[derive(Debug, Default)]
pub struct RenderScene {
    slots: BTreeMap<NodeId, NodeSlot>,
    order: Vec<NodeId>,
    free_instances: Vec<u32>,
    instance_count: u32,
    plan: WritePlan,
    report: UpdateReport,
    hit: HitIndex,
    /// **What the index was built from**, in build order: one `(id, world box)`
    /// per slot. This is the whole staleness protocol — `ensure_index` rebuilds
    /// when this vector differs from the live inputs, so no code path has to
    /// remember to invalidate anything. (The previous design kept a `hit_dirty`
    /// flag, which made correctness a promise every future mutation had to keep;
    /// Task 10.4's report had to disclose exactly that.)
    hit_inputs: Vec<(NodeId, Bounds)>,
    /// Rebuilds since construction. A diagnostic, not a decision: the laws assert
    /// that repeated identical queries do **not** rebuild, and that a geometry
    /// edit, a reorder or a removal does.
    hit_rebuilds: u64,
    /// Every gradient stop in the scene (Task 10.2 RULE 3).
    ramps: RampTable,
}

impl RenderScene {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    pub fn len(&self) -> usize {
        self.slots.len()
    }

    pub fn get(&self, id: NodeId) -> Option<&NodeSlot> {
        self.slots.get(&id)
    }

    pub fn order(&self) -> &[NodeId] {
        &self.order
    }

    /// The plan produced by the last [`RenderScene::sync`].
    pub fn plan(&self) -> &WritePlan {
        &self.plan
    }

    /// The numbers of the last sync.
    pub fn report(&self) -> &UpdateReport {
        &self.report
    }

    /// Live instance rows (`u32::MAX` slots are never allocated). One per draw
    /// item, not one per node.
    pub fn instance_count(&self) -> u32 {
        self.instance_count
    }

    /// The gradient stop table the GPU holds.
    pub fn ramps(&self) -> &RampTable {
        &self.ramps
    }

    pub fn total_vertices(&self) -> usize {
        self.slots
            .values()
            .map(|s| {
                s.fill.vertices.len() + s.strokes.iter().map(|m| m.vertices.len()).sum::<usize>()
            })
            .sum()
    }

    pub fn total_indices(&self) -> usize {
        self.slots
            .values()
            .map(|s| {
                s.fill.indices.len() + s.strokes.iter().map(|m| m.indices.len()).sum::<usize>()
            })
            .sum()
    }

    /// Draw items across the whole scene, in draw order — the GPU's draw list.
    pub fn draw_items(&self) -> Vec<(NodeId, DrawItem)> {
        self.order
            .iter()
            .filter_map(|id| self.slots.get(id).map(|slot| (id, slot)))
            .flat_map(|(id, slot)| {
                slot.items
                    .iter()
                    .map(move |item| (*id, *item))
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    pub fn draw_calls(&self) -> usize {
        self.slots.values().map(NodeSlot::draw_calls).sum()
    }

    /// The instance array as the GPU holds it: `instance_count` rows, index =
    /// slot. Holes (dropped nodes) are zeroed.
    pub fn instance_rows(&self) -> Vec<InstanceRaw> {
        let zero = InstanceRaw {
            transform: [0.0; 4],
            color: [0.0; 4],
            params: [0.0; 4],
            ramp: [0.0; 4],
            frame: [0.0; 4],
        };
        let mut rows = vec![zero; self.instance_count as usize];
        for slot in self.slots.values() {
            for (item, instance) in slot.items.iter().zip(&slot.instances) {
                if (item.slot as usize) < rows.len() {
                    rows[item.slot as usize] = *instance;
                }
            }
        }
        rows
    }

    /// Sync with a dirtied evaluator scene: decide the minimal set of writes.
    ///
    /// `dirty.is_full()` means "rebuild the CPU mirror" (a cold start, or the
    /// UI's explicit full re-evaluation) — but note that even then, only the
    /// nodes whose *bytes* differ are written, because the plan is derived from
    /// the cached slots, not from the mode.
    pub fn sync(&mut self, scene: &EvaluatedScene, dirty: &DirtySet) -> &WritePlan {
        let mut plan = WritePlan {
            full: dirty.is_full(),
            ..Default::default()
        };

        // 1. Removals. A node whose id is no longer in the scene is dropped
        //    regardless of the dirty set: a deleted node must never be drawn
        //    from a stale buffer, and relying on the delete appearing in
        //    `dirty` would couple the renderer to the engine's event details.
        let stale: Vec<NodeId> = self
            .slots
            .keys()
            .copied()
            .filter(|id| !scene.nodes.contains_key(id))
            .collect();
        for id in stale {
            if let Some(slot) = self.slots.remove(&id) {
                self.free_instances.extend(slot.slots());
                plan.removed.push(id);
            }
        }

        // 2. The ramp table. Gradient stops live in their own storage array, and
        //    a window into it is only stable while the *table* is stable, so the
        //    table is rebuilt from every cached slot (not just the dirty ones)
        //    and — when it moved — every node that owns a gradient is re-examined
        //    even if the engine did not call it dirty. That is the one place a
        //    gradient edit reaches further than its own node, and it is why a
        //    stop drag stays a handful of row writes.
        let ramps = self.ramps_for(scene);
        let ramps_changed = ramps != self.ramps;
        if ramps_changed {
            plan.ramps = Some(ramps.to_bytes().len());
        }
        self.ramps = ramps;
        let ramp_affected: Vec<NodeId> = if ramps_changed {
            self.slots
                .iter()
                .filter(|(_, slot)| slot.style.appearances.iter().any(|l| l.paint.is_gradient()))
                .map(|(id, _)| *id)
                .collect()
        } else {
            Vec::new()
        };

        // 3. Additions and updates. A full sync visits every node; an
        //    incremental one visits exactly the dirty ids (the same contract
        //    `EvaluatedScene::apply_partial` uses) — plus the gradient owners of
        //    step 2.
        let mut targets: Vec<NodeId> = if dirty.is_full() {
            scene.z_order.clone()
        } else {
            let mut ids: Vec<NodeId> = dirty.ids().iter().copied().collect();
            ids.sort();
            ids
        };
        for id in ramp_affected {
            if !targets.contains(&id) {
                targets.push(id);
            }
        }

        for id in targets {
            match scene.nodes.get(&id) {
                Some(node) => self.sync_node(id, node, &mut plan),
                // A cached node re-examined only because the ramp table moved:
                // no geometry work, just its instance rows.
                None => {
                    let Some(existing) = self.slots.get(&id) else {
                        continue;
                    };
                    let style = existing.style.clone();
                    let origin = existing.key.origin;
                    let visible = existing.visible;
                    self.refresh_rows(id, &style, origin, visible, &mut plan);
                }
            }
        }

        // 4. Draw order. Rebuilt from the scene every sync (it is O(n) over
        //    ids and touches no buffer): a reorder is iteration order, not data.
        let order: Vec<NodeId> = scene
            .z_order
            .iter()
            .copied()
            .filter(|id| self.slots.contains_key(id))
            .collect();
        plan.reordered = order != self.order;
        self.order = order.clone();
        plan.order = order;

        self.report = self.summarize(&plan);
        self.plan = plan;
        &self.plan
    }

    /// The ramp table for the *next* frame: every cached style, with dirty nodes
    /// read from the incoming scene. Ids are visited in a sorted, deterministic
    /// order so a window only moves when a gradient actually appears or vanishes.
    fn ramps_for(&self, scene: &EvaluatedScene) -> RampTable {
        let mut ids: Vec<NodeId> = self.slots.keys().copied().collect();
        ids.extend(scene.nodes.keys().copied());
        ids.sort();
        ids.dedup();
        let mut table = RampTable::default();
        for id in ids {
            let style = match scene.nodes.get(&id) {
                Some(node) => node.style.clone(),
                None => match self.slots.get(&id) {
                    Some(slot) => slot.style.clone(),
                    None => continue,
                },
            };
            table.extend(id, &style);
        }
        table
    }

    /// Sync one node from the evaluated scene: geometry, meshes, draw items and
    /// instance rows — each class of change paying only for itself.
    fn sync_node(&mut self, id: NodeId, node: &EvaluatedNode, plan: &mut WritePlan) {
        let flattened = crate::geometry::flatten(&node.primitive);
        let key = GeometryKey::of(&flattened);
        let strokes_key = strokes_key_of(&node.style);
        let previous = self.slots.get(&id);
        let created = previous.is_none();
        // `map_or(true, …)` = "changed, because there was nothing" — the
        // MSRV-safe spelling of `is_none_or` (the workspace pins 1.75).
        let geometry_changed = previous.map_or(true, |slot| {
            slot.key.shape != key.shape || slot.strokes_key != strokes_key
        });
        let moved = previous.map_or(true, |slot| slot.key.origin != key.origin);
        let style_changed = previous.map_or(true, |slot| slot.style != node.style);
        let flags_changed = previous.map_or(true, |slot| {
            slot.visible != node.visible || slot.locked != node.locked
        });

        if !created && !geometry_changed && !moved && !style_changed && !flags_changed {
            return; // dirty but unchanged: zero writes (RULE 2 in its purest form)
        }

        // Classify before working, so the report says why bytes moved.
        if created || geometry_changed {
            plan.retessellated.push(id);
        } else if moved {
            plan.moved.push(id);
        } else if style_changed {
            plan.restyled.push(id);
        } else if flags_changed {
            // **RULE 4**: an eye or a padlock is not a re-evaluation and not a
            // byte of buffer traffic. The row still holds the same colours; the
            // *list* of rows this node contributes to the frame changes, and the
            // hit index stops offering it. That is the whole cost.
            plan.repainted.push(id);
        }

        if created {
            let slot = self.allocate(id);
            plan.ops.push(WriteOp {
                id,
                slot,
                kind: WriteKind::Create,
                bytes: 0,
            });
        }

        if geometry_changed {
            let tessellation = match tessellate_flattened(id, &flattened, &node.style) {
                Ok(t) => t,
                Err(error) => {
                    // A node that cannot tessellate is not drawn. The engine
                    // diagnoses the value that broke it; the renderer's job is
                    // to keep the frame honest and carry on.
                    eprintln!("{error}");
                    if let Some(slot) = self.slots.remove(&id) {
                        self.free_instances.extend(slot.slots());
                    }
                    plan.removed.push(id);
                    return;
                }
            };
            // Per-mesh, not per-node: widening one stroke rebuilds that outline
            // and must leave the fill's buffers — and every other stroke's —
            // alone.
            // Re-borrow (rather than hold `previous` across the allocation
            // above) to ask the same question: did these bytes change?
            let (fill_changed, strokes_changed) = match self.slots.get(&id) {
                Some(slot) => (
                    slot.fill != tessellation.fill,
                    slot.strokes.len() != tessellation.strokes.len()
                        || slot
                            .strokes
                            .iter()
                            .zip(&tessellation.strokes)
                            .any(|(old, new)| old != new),
                ),
                None => (true, true),
            };
            if fill_changed {
                self.write_mesh_ops(plan, id, MeshKind::Fill, &tessellation);
            }
            if strokes_changed {
                for index in 0..tessellation.strokes.len() as u32 {
                    self.write_mesh_ops(plan, id, MeshKind::Stroke(index), &tessellation);
                }
                // A stack that lost a stroke layer: the buffers for the vanished
                // outlines are simply no longer referenced.
            }
            let bounds = tessellation.world_bounds;
            let local_rings = crate::geometry::rings_local(&tessellation.rings, key.origin);
            let slot = self.slots.get_mut(&id).expect("slot exists");
            slot.local_rings = local_rings;
            slot.world_bounds = bounds;
            slot.key = tessellation.key;
            slot.strokes_key = strokes_key;
            slot.fill = tessellation.fill;
            slot.strokes = tessellation.strokes;
        } else if moved {
            let slot = self.slots.get_mut(&id).expect("slot exists");
            // Local rings are unchanged by construction; only the frame's
            // origin — and therefore the world box — moved.
            slot.world_bounds = slot.world_bounds.translated(
                key.origin.0 - slot.key.origin.0,
                key.origin.1 - slot.key.origin.1,
            );
            slot.key.origin = key.origin;
        }

        let origin = self.slots[&id].key.origin;
        self.refresh_rows(id, &node.style, origin, node.visible, plan);
        let slot = self.slots.get_mut(&id).expect("slot exists");
        slot.style = node.style.clone();
        slot.visible = node.visible;
        slot.locked = node.locked;
        // No index invalidation here, and not by omission: an eye or a lock is a
        // pair of booleans `hit_test` reads *at query time*, so the index — which
        // is about boxes and stacking — is untouched. A visibility toggle costs
        // nothing at all now, not even a rebuild (RULE 4).
    }

    /// Bring a node's draw items and instance rows in line with its style: one
    /// item per visible paint layer, in stack order, each with its own row.
    ///
    /// Rows are reused positionally while a slot still describes the same draw
    /// item, so a colour edit rewrites one row and a stack edit settles the list
    /// once and then writes only what actually differs (`InstanceRaw` equality —
    /// the same "dirty but unchanged costs nothing" rule the meshes follow).
    fn refresh_rows(
        &mut self,
        id: NodeId,
        style: &EvaluatedStyle,
        origin: (f32, f32),
        visible: bool,
        plan: &mut WritePlan,
    ) {
        // 1. The draw items this style implies, in stack order.
        //
        // **RULE 4**: a hidden node contributes *no* items — the eye is applied
        // here, at the draw-list boundary, which is why toggling one costs a
        // repaint and not a re-evaluation. Its *meshes* are kept (it is still
        // the same shape; nothing about the geometry changed), so showing it
        // again re-tessellates nothing. Its **instance rows** are released with
        // the items (see step 3), which is the honest asymmetry the boundary
        // laws measure: hiding costs zero writes, showing costs one 80-byte row
        // per appearance layer. Rows are a draw need, not a document fact.
        let desired: Vec<(u32, DrawMesh)> = {
            let mut stroke_index: u32 = 0;
            let mut out = Vec::new();
            if visible {
                for (index, layer) in style.appearances.iter().enumerate() {
                    if layer.is_stroke() {
                        let mesh = DrawMesh::Stroke(stroke_index);
                        stroke_index += 1;
                        if layer.is_visible() {
                            out.push((index as u32, mesh));
                        }
                    } else if layer.is_visible() {
                        out.push((index as u32, DrawMesh::Fill));
                    }
                }
            }
            out
        };

        // 2. What the node holds now (cloned: two small vectors, no mesh data).
        let (current, current_rows) = match self.slots.get(&id) {
            Some(slot) => (slot.items.clone(), slot.instances.clone()),
            None => (Vec::new(), Vec::new()),
        };

        // 3. Settle the rows: keep the positional row when the same layer still
        //    draws the same mesh, allocate for anything new.
        let mut items: Vec<DrawItem> = Vec::with_capacity(desired.len());
        let mut reused: Vec<bool> = Vec::with_capacity(desired.len());
        for (index, (appearance, mesh)) in desired.iter().enumerate() {
            let kept = current
                .get(index)
                .filter(|item| item.appearance == *appearance && item.mesh == *mesh)
                .map(|item| item.slot);
            let row = match kept {
                Some(row) => {
                    reused.push(true);
                    row
                }
                None => {
                    reused.push(false);
                    self.allocate_row()
                }
            };
            items.push(DrawItem {
                appearance: *appearance,
                mesh: *mesh,
                slot: row,
                blend: 0.0,
            });
        }
        let keep: Vec<u32> = items.iter().map(|item| item.slot).collect();
        for item in &current {
            if !keep.contains(&item.slot) {
                self.free_instances.push(item.slot);
            }
        }

        // 4. Build every row and keep the ones whose bytes changed.
        let mut instances: Vec<InstanceRaw> = Vec::with_capacity(items.len());
        let mut writes: Vec<(u32, Vec<u8>)> = Vec::new();
        for (index, item) in items.iter_mut().enumerate() {
            let layer = &style.appearances[item.appearance as usize];
            let candidate = InstanceRaw::for_appearance(
                id,
                origin,
                style,
                layer,
                item.appearance as usize,
                &self.ramps,
                false, // `selected` is reserved for the UI's selection overlay
            );
            // A layer can be visible and still produce no item (it was filtered
            // above), so a `None` here is a bug, not a case: fall back to an
            // empty row rather than panic in the frame loop.
            let candidate = candidate.unwrap_or_else(|| placeholder_instance(origin));
            item.blend = candidate.params[3];
            let unchanged = reused[index]
                && current_rows
                    .get(index)
                    .map(|row| *row == candidate)
                    .unwrap_or(false);
            if !unchanged {
                writes.push((item.slot, candidate.to_bytes()));
            }
            instances.push(candidate);
        }

        let slot = self.slots.get_mut(&id).expect("slot exists");
        slot.items = items;
        slot.instances = instances;
        for (row, bytes) in writes {
            plan.ops.push(WriteOp {
                id,
                slot: row,
                kind: WriteKind::Instance,
                bytes: bytes.len(),
            });
        }
    }

    /// Take a free instance row, or grow the array by one.
    fn allocate_row(&mut self) -> u32 {
        match self.free_instances.pop() {
            Some(row) => row,
            None => {
                let row = self.instance_count;
                self.instance_count += 1;
                row
            }
        }
    }

    /// Record one mesh's two writes (vertices + indices). Fill and stroke have
    /// separate buffers, so a widened outline never rewrites fill data.
    fn write_mesh_ops(&self, plan: &mut WritePlan, id: NodeId, kind: MeshKind, t: &Tessellation) {
        let mesh = match kind {
            MeshKind::Fill => &t.fill,
            MeshKind::Stroke(index) => match t.strokes.get(index as usize) {
                Some(mesh) => mesh,
                None => return,
            },
        };
        plan.ops.push(WriteOp {
            id,
            slot: UNASSIGNED_SLOT,
            kind: WriteKind::Vertices(kind),
            bytes: mesh.vertices.len() * 8,
        });
        plan.ops.push(WriteOp {
            id,
            slot: UNASSIGNED_SLOT,
            kind: WriteKind::Indices(kind),
            bytes: mesh.indices.len() * 4,
        });
    }

    /// Create a node's slot. Rows are allocated separately (one per draw item)
    /// by [`RenderScene::refresh_rows`], because how many rows a node needs is a
    /// property of its paint stack, not of its existence.
    fn allocate(&mut self, id: NodeId) -> u32 {
        self.slots.insert(
            id,
            NodeSlot {
                id,
                items: Vec::new(),
                local_rings: Vec::new(),
                world_bounds: Bounds::EMPTY,
                key: GeometryKey {
                    shape: String::new(),
                    origin: (0.0, 0.0),
                },
                strokes_key: Vec::new(),
                style: EvaluatedStyle::default(),
                visible: true,
                locked: false,
                fill: Mesh::default(),
                strokes: Vec::new(),
                instances: Vec::new(),
            },
        );
        UNASSIGNED_SLOT
    }

    fn summarize(&self, plan: &WritePlan) -> UpdateReport {
        let draw_items = self.slots.values().map(|slot| slot.draw_calls()).sum();
        UpdateReport {
            nodes: self.slots.len(),
            draw_items,
            blended_draws: self
                .slots
                .values()
                .flat_map(|slot| slot.items.iter())
                .filter(|item| item.needs_backdrop())
                .count(),
            vertices: self.total_vertices(),
            indices: self.total_indices(),
            instances: self.instance_count as usize,
            created: plan.writes(WriteKind::Create).len(),
            removed: plan.removed.len(),
            retessellated: plan.retessellated.len(),
            moved: plan.moved.len(),
            restyled: plan.restyled.len(),
            writes: plan.ops.len(),
            bytes: plan.bytes(),
            full: plan.full,
            touched: plan.touched(),
        }
    }

    /// The bytes the GPU should hold for one node's mesh — what `flush` hands
    /// the sink.
    pub fn mesh_bytes(&self, id: NodeId, kind: MeshKind, indices: bool) -> Option<Vec<u8>> {
        let slot = self.slots.get(&id)?;
        let mesh = match kind {
            MeshKind::Fill => &slot.fill,
            MeshKind::Stroke(index) => slot.strokes.get(index as usize)?,
        };
        Some(if indices {
            mesh.index_bytes()
        } else {
            mesh.vertex_bytes()
        })
    }

    /// One draw item's 80 bytes, addressed by its row.
    pub fn instance_bytes(&self, id: NodeId, row: u32) -> Option<Vec<u8>> {
        let slot = self.slots.get(&id)?;
        let index = slot.items.iter().position(|item| item.slot == row)?;
        Some(slot.instances.get(index)?.to_bytes())
    }

    /// Apply the plan through a sink (the real wgpu renderer, or the mock).
    /// Returns the number of sink calls made.
    pub fn flush<S: BufferSink>(&self, sink: &mut S) -> Result<usize, RenderError> {
        let mut calls = 0;
        sink.ensure_instances(self.instance_count)?;
        for id in &self.plan.removed {
            sink.drop_node(*id)?;
            calls += 1;
        }
        for op in &self.plan.ops {
            match op.kind {
                WriteKind::Create => {
                    let slot = self.slots.get(&op.id).expect("plan references a live slot");
                    sink.create_node(op.id, op.slot, &slot.fill, &slot.strokes)?;
                }
                WriteKind::Vertices(kind) => {
                    let bytes = self
                        .mesh_bytes(op.id, kind, false)
                        .expect("plan references a live slot");
                    sink.write(op.id, op.slot, op.kind, &bytes)?;
                }
                WriteKind::Indices(kind) => {
                    let bytes = self
                        .mesh_bytes(op.id, kind, true)
                        .expect("plan references a live slot");
                    sink.write(op.id, op.slot, op.kind, &bytes)?;
                }
                WriteKind::Instance => {
                    let bytes = self
                        .instance_bytes(op.id, op.slot)
                        .expect("plan references a live slot");
                    sink.write(op.id, op.slot, op.kind, &bytes)?;
                }
                WriteKind::Ramps => {
                    // One write for the whole stop array: it is the unit the
                    // shader addresses, and it is a few hundred bytes.
                    sink.write_ramps(&self.ramps.to_bytes())?;
                }
            }
            calls += 1;
        }
        sink.set_order(&self.plan.order)?;
        Ok(calls)
    }

    // ── Hit testing (RULE 3) ────────────────────────────────────────────

    /// Rebuild the spatial index when its **inputs** changed since the last
    /// query — decided by comparing them, never by a flag.
    ///
    /// The index answers a question about *geometry and stacking*, so its inputs
    /// are exactly two things: each slot's world box, and the order the rows are
    /// painted in. [`RenderScene::index_inputs`] yields them in build order, and
    /// everything else here is a comparison: if the live inputs differ from
    /// [`RenderScene::hit_inputs`], the index is rebuilt and the copy is replaced.
    ///
    /// There is deliberately **nothing to remember**: any mutation that changes a
    /// box or the order changes the comparison, so a path added tomorrow cannot
    /// serve a stale answer by forgetting to invalidate. (The cost is O(n) per
    /// query — the same order as `HitIndex::candidates`, which already allocates —
    /// against an O(n·cells) rebuild it usually avoids. Lazy on purpose: a drag
    /// moves nodes every frame, and the index is only needed when a pointer lands
    /// on the canvas.)
    fn ensure_index(&mut self) {
        let mut current = 0usize;
        let mut same = true;
        for (index, (id, bounds)) in self.index_inputs().enumerate() {
            current = index + 1;
            match self.hit_inputs.get(index) {
                Some((known_id, known_bounds)) if *known_id == id && *known_bounds == bounds => {}
                _ => {
                    same = false;
                    break;
                }
            }
        }
        if same && current == self.hit_inputs.len() {
            return;
        }
        let entries: Vec<(NodeId, Bounds)> = self.index_inputs().collect();
        self.hit.rebuild(entries.iter().copied());
        self.hit_inputs = entries;
        self.hit_rebuilds += 1;
    }

    /// The inputs the index is built from, **in build order**: everything the
    /// draw plan no longer paints first, then the scene's own draw order.
    ///
    /// Insertion order *is* the answer for overlapping shapes (see
    /// [`HitIndex::candidates`], which walks a bucket in reverse), so the order
    /// here is the front-to-back order a click resolves by. A live slot that the
    /// plan does not draw goes in first, so it can never outrank a shape that is
    /// actually on the canvas.
    fn index_inputs(&self) -> impl Iterator<Item = (NodeId, Bounds)> + '_ {
        let drawn: BTreeSet<&NodeId> = self.order.iter().collect();
        self.slots
            .iter()
            .filter(move |(id, _)| !drawn.contains(id))
            .map(|(id, slot)| (*id, slot.world_bounds))
            .chain(
                self.order
                    .iter()
                    .filter_map(|id| self.slots.get(id).map(|slot| (*id, slot.world_bounds))),
            )
    }

    /// How many times the spatial index has been rebuilt. The laws read it to
    /// pin the lazy contract (`a_hit_query_rebuilds_only_when_the_index_inputs_changed`).
    pub fn hit_rebuilds(&self) -> u64 {
        self.hit_rebuilds
    }

    /// The node under `(x, y)`, front-most first — `None` for empty canvas.
    ///
    /// Two stages: the bounding-box grid rejects candidates (cheap), then the
    /// exact even-odd containment test decides (correct) — so clicking a circle's
    /// bounding-box corner returns `None`, not the circle.
    pub fn hit_test(&mut self, x: f32, y: f32) -> Option<NodeId> {
        self.ensure_index();
        let candidates = self.hit.candidates(x, y);
        for id in candidates {
            let Some(slot) = self.slots.get(&id) else {
                continue;
            };
            // **RULE 1 + RULE 4**: a hidden node is not on the canvas and a
            // locked one is not selectable, so neither answers a pointer. The
            // flags cost two booleans here and nothing anywhere else — no
            // re-evaluation, no buffer traffic, not even an index rebuild that
            // is specific to them.
            if !slot.visible || slot.locked {
                continue;
            }
            let local_x = x - slot.key.origin.0;
            let local_y = y - slot.key.origin.1;
            if contains_point(&slot.local_rings, local_x as f64, local_y as f64) {
                return Some(id);
            }
        }
        None
    }
}

/// A row with nothing in it: the frame's transform, everything else zero. Used
/// as the "row exists but has not been filled yet" value, and as the fallback for
/// a draw item that produces no instance at all.
fn placeholder_instance(origin: (f32, f32)) -> InstanceRaw {
    InstanceRaw {
        transform: [1.0, 1.0, origin.0, origin.1],
        color: [0.0; 4],
        params: [0.0; 4],
        ramp: [0.0; 4],
        frame: [0.0; 4],
    }
}

/// Even-odd point-in-shape test over a node's rings, in the node's local frame.
///
/// `geo` does the per-ring crossing test (`Contains`); parity across the rings
/// is the even-odd rule the tessellator fills with (see `geometry::FILL_RULE`),
/// which is what makes a hole a hole for the pointer as well as for the raster.
pub fn contains_point(rings: &[Vec<[f32; 2]>], x: f64, y: f64) -> bool {
    use geo::Contains;
    use geo_types::Point;

    let point = Point::new(x, y);
    let mut inside = false;
    for ring in rings {
        if let Some(polygon) = crate::geometry::ring_polygon(ring) {
            if polygon.contains(&point) {
                inside = !inside;
            }
        }
    }
    inside
}

/// What a sink must be able to do. The real GPU renderer implements it; so does
/// [`MockSink`], which is how the incremental laws are proven without a device.
pub trait BufferSink {
    /// Allocate (or grow) the buffers for a node: one fill mesh and one mesh per
    /// stroke layer, in stack order.
    fn create_node(
        &mut self,
        id: NodeId,
        slot: u32,
        fill: &Mesh,
        strokes: &[Mesh],
    ) -> Result<(), RenderError>;

    /// Upload the whole gradient stop array. A default implementation keeps the
    /// contract small for sinks that do not draw gradients (the mock ledger),
    /// while the real renderer binds it as a storage buffer.
    fn write_ramps(&mut self, _bytes: &[u8]) -> Result<(), RenderError> {
        Ok(())
    }

    /// Write one buffer's bytes. `bytes.is_empty()` means "this mesh is empty":
    /// the sink records zero-length buffers rather than rejecting the write.
    fn write(
        &mut self,
        id: NodeId,
        slot: u32,
        kind: WriteKind,
        bytes: &[u8],
    ) -> Result<(), RenderError>;

    /// Forget a node's buffers.
    fn drop_node(&mut self, id: NodeId) -> Result<(), RenderError>;

    /// Make sure the instance array can hold `count` rows.
    fn ensure_instances(&mut self, count: u32) -> Result<(), RenderError>;

    /// Adopt `order` as the draw order (no buffer traffic).
    fn set_order(&mut self, order: &[NodeId]) -> Result<(), RenderError>;
}

/// One recorded sink call — the Incremental Update Law's evidence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SinkCall {
    pub id: NodeId,
    pub slot: u32,
    pub kind: WriteKind,
    pub bytes: usize,
}

/// What one node looks like inside [`MockSink`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MockNode {
    pub slot: u32,
    pub fill_vertices: Vec<u8>,
    pub fill_indices: Vec<u8>,
    /// Stroke meshes by stack index (Task 10.2: one per stroke layer).
    pub stroke_vertices: BTreeMap<u32, Vec<u8>>,
    pub stroke_indices: BTreeMap<u32, Vec<u8>>,
    /// The **last** instance row written for this node, kept for convenience.
    pub instance: Vec<u8>,
    /// Every row this node owns, by row number.
    pub instance_rows: BTreeMap<u32, Vec<u8>>,
}

/// A `BufferSink` that remembers *exactly* what was written, so a test can say
/// "this buffer was not touched" by comparing bytes.
#[derive(Debug, Clone, Default)]
pub struct MockSink {
    pub nodes: BTreeMap<NodeId, MockNode>,
    pub calls: Vec<SinkCall>,
    pub order: Vec<NodeId>,
    pub instances_capacity: u32,
    /// Slot → instance bytes, i.e. the shared instance array's slices.
    pub instance_slices: BTreeMap<u32, Vec<u8>>,
    /// The gradient stop array as the GPU last received it.
    pub ramps: Option<Vec<u8>>,
    pub dropped: Vec<NodeId>,
}

impl MockSink {
    pub fn new() -> Self {
        Self::default()
    }

    /// Ids that were written to, in call order, deduplicated.
    pub fn touched(&self) -> Vec<NodeId> {
        let mut out = Vec::new();
        for call in &self.calls {
            if !out.contains(&call.id) {
                out.push(call.id);
            }
        }
        out
    }

    pub fn calls_for(&self, id: NodeId) -> Vec<&SinkCall> {
        self.calls.iter().filter(|c| c.id == id).collect()
    }

    pub fn kinds_for(&self, id: NodeId) -> Vec<WriteKind> {
        self.calls_for(id).into_iter().map(|c| c.kind).collect()
    }
}

impl BufferSink for MockSink {
    fn create_node(
        &mut self,
        id: NodeId,
        slot: u32,
        _fill: &Mesh,
        _strokes: &[Mesh],
    ) -> Result<(), RenderError> {
        let entry = self.nodes.entry(id).or_default();
        entry.slot = slot;
        self.calls.push(SinkCall {
            id,
            slot,
            kind: WriteKind::Create,
            bytes: 0,
        });
        Ok(())
    }

    fn write(
        &mut self,
        id: NodeId,
        slot: u32,
        kind: WriteKind,
        bytes: &[u8],
    ) -> Result<(), RenderError> {
        let entry = self.nodes.entry(id).or_default();
        entry.slot = slot;
        match kind {
            WriteKind::Vertices(MeshKind::Fill) => entry.fill_vertices = bytes.to_vec(),
            WriteKind::Indices(MeshKind::Fill) => entry.fill_indices = bytes.to_vec(),
            WriteKind::Vertices(MeshKind::Stroke(index)) => {
                entry.stroke_vertices.insert(index, bytes.to_vec());
            }
            WriteKind::Indices(MeshKind::Stroke(index)) => {
                entry.stroke_indices.insert(index, bytes.to_vec());
            }
            WriteKind::Instance => {
                // Keyed by **row**, not by node: a node with a stack of paints
                // owns several rows, and the ledger has to be able to say which
                // one a frame wrote.
                entry.instance = bytes.to_vec();
                entry.instance_rows.insert(slot, bytes.to_vec());
                self.instance_slices.insert(slot, bytes.to_vec());
            }
            WriteKind::Ramps => {
                self.ramps = Some(bytes.to_vec());
            }
            WriteKind::Create => {}
        }
        self.calls.push(SinkCall {
            id,
            slot,
            kind,
            bytes: bytes.len(),
        });
        Ok(())
    }

    fn drop_node(&mut self, id: NodeId) -> Result<(), RenderError> {
        self.nodes.remove(&id);
        self.dropped.push(id);
        Ok(())
    }

    fn ensure_instances(&mut self, count: u32) -> Result<(), RenderError> {
        self.instances_capacity = self.instances_capacity.max(count);
        Ok(())
    }

    fn set_order(&mut self, order: &[NodeId]) -> Result<(), RenderError> {
        self.order = order.to_vec();
        Ok(())
    }
}

/// A colour that clears the canvas (the UI's background).
pub const CLEAR_COLOR: Color = Color {
    r: 0x0e,
    g: 0x11,
    b: 0x16,
    a: 0xff,
};
