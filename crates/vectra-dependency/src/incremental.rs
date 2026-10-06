//! Incremental evaluation: patch the cached scene with exactly the dirty ids
//! (Task 2.2 §3, MES §18).
//!
//! [`IncrementalScene`] owns the engine's scene cache. A mutation hands it the
//! ids the dependency graph marked dirty; it re-evaluates *only* those and
//! patches the cache through the geometry crate's
//! [`EvaluatedScene::apply_partial`] (which also rebuilds z-order from live
//! document order, so structural changes need no special case).
//!
//! ```text
//! SetVariable $base ──▶ graph.dirty_ids() ──▶ [rect] ──▶ evaluate(dirty) ──▶ apply_partial
//!                                                    (1 node, not the document)
//! ```
//!
//! # Why this is safe
//! The dirty set is by construction the *downstream closure* of whatever
//! changed (plus the changed nodes themselves), so anything whose resolved
//! value can differ is in it. [`IncrementalScene::refresh`] therefore produces
//! a scene identical to a full rebuild — pinned as a property test
//! (`tests/incremental_laws.rs`, "patch ≡ rebuild") the same way MES §18's
//! incrementality boundary was pinned in Task 1.3.
//!
//! # Diagnostics
//! Diagnostics are per-node by construction (`Diagnostic.node_id`), so the
//! incremental path drops the notes of re-evaluated nodes and re-collects
//! them, keeping the rest. A `node_id: None` note would be dropped (none exist
//! today; a full pass re-derives them) — documented rather than guessed.

use std::collections::BTreeSet;
use vectra_core::{Document, EvalMode, EvaluationContext, NodeId};
use vectra_geometry::{Diagnostic, DirtySet, EvaluatedScene, Evaluator, GeometryEvaluator};

/// Counters for the eval panel: proof that the loop is incremental.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EvalStats {
    /// Passes that evaluated the whole document (cold start or explicit rebuild).
    pub full_evals: u32,
    /// Passes that patched a subset.
    pub incremental_evals: u32,
    /// Mutations that required no evaluation at all (nothing depends on them).
    pub no_ops: u32,
    pub last_dirty: usize,
    pub last_evaluated: usize,
    pub last_mode: EvalMode,
}

impl Default for EvalStats {
    fn default() -> Self {
        Self {
            full_evals: 0,
            incremental_evals: 0,
            no_ops: 0,
            last_dirty: 0,
            last_evaluated: 0,
            last_mode: EvalMode::Full,
        }
    }
}

/// What one [`IncrementalScene::refresh`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvalReport {
    pub mode: EvalMode,
    /// The ids that were (or would have been) re-evaluated, sorted.
    pub dirty: Vec<NodeId>,
    /// Nodes the evaluator actually produced for the dirty set.
    pub evaluated: usize,
}

impl EvalReport {
    /// True ⟺ nothing was re-evaluated (the mutation had no dependents).
    pub fn is_noop(&self) -> bool {
        self.dirty.is_empty() && self.mode == EvalMode::Incremental
    }
}

/// Evaluated scene + diagnostics cache with an incremental patch path.
#[derive(Debug, Default)]
pub struct IncrementalScene {
    scene: EvaluatedScene,
    diagnostics: Vec<Diagnostic>,
    warm: bool,
    stats: EvalStats,
}

impl IncrementalScene {
    pub fn new() -> Self {
        Self::default()
    }

    /// Throw the cache away: the next `refresh` is a full pass.
    ///
    /// Used when the document is replaced wholesale (load/import) — *not* on
    /// undo/redo, which are ordinary mutations and stay incremental.
    pub fn invalidate(&mut self) {
        self.scene = EvaluatedScene::empty();
        self.diagnostics.clear();
        self.warm = false;
    }

    pub fn is_warm(&self) -> bool {
        self.warm
    }

    pub fn scene(&self) -> &EvaluatedScene {
        &self.scene
    }

    /// Mutable access to the cached scene, for layers that compose *derived*
    /// geometry on top of the primitive pass (Task 4.0 operations). Callers are
    /// responsible for calling [`EvaluatedScene::apply_operations`] so z-order
    /// and the live-id invariant are restored in the same breath.
    pub fn scene_mut(&mut self) -> &mut EvaluatedScene {
        &mut self.scene
    }

    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    pub fn stats(&self) -> EvalStats {
        self.stats
    }

    pub fn node_count(&self) -> usize {
        self.scene.len()
    }

    /// Bring the cache up to date with `doc`.
    ///
    /// * cold cache (or `dirty` cannot be localized) → full pass;
    /// * warm cache + empty `dirty` → no evaluation at all;
    /// * warm cache + ids → partial pass, patched into the cache.
    ///
    /// Evaluates the passed `dirty` ids exactly; duplicates are collapsed and
    /// unknown ids are ignored (they simply evaluate to nothing, which is how
    /// deleted nodes leave the cache).
    pub fn refresh(
        &mut self,
        doc: &Document,
        ctx: &EvaluationContext,
        dirty: &[NodeId],
    ) -> EvalReport {
        if !self.warm {
            return self.refresh_full(doc, ctx);
        }

        let ids: BTreeSet<NodeId> = dirty.iter().copied().collect();
        if ids.is_empty() {
            // **A no-op is still a mutation's worth of bookkeeping.** Reordering
            // layers, moving a group, assigning a node to a layer and toggling an
            // eye all dirty nothing — the dependency graph is right to stay
            // silent, because not one parameter needs resolving — but the *draw
            // order* and the *presentation flags* are not values, so nothing else
            // would notice they moved. The order is re-derived here; the flags
            // are re-derived by the caller that owns the document
            // (`vectra_wasm::VectraEngine::settle` → `refresh_presentation`).
            self.scene.resync_draw_order(doc);
            self.stats.no_ops += 1;
            self.stats.last_dirty = 0;
            self.stats.last_evaluated = 0;
            self.stats.last_mode = EvalMode::Incremental;
            return EvalReport {
                mode: EvalMode::Incremental,
                dirty: Vec::new(),
                evaluated: 0,
            };
        }

        let dirty_set = DirtySet::nodes(ids.iter().copied());
        let partial = GeometryEvaluator.evaluate(doc, ctx, &dirty_set);
        let evaluated = partial.scene.len();

        // Merge diagnostics: forget the notes of the nodes we just
        // re-evaluated, then append the fresh ones.
        self.diagnostics.retain(|d| match d.node_id {
            Some(id) => !ids.contains(&id),
            None => false,
        });
        self.diagnostics.extend(partial.diagnostics);

        self.scene.apply_partial(doc, partial.scene, &dirty_set);
        debug_assert!(self.scene_consistent_with(doc), "cache holds live ids only");

        self.stats.incremental_evals += 1;
        self.stats.last_dirty = ids.len();
        self.stats.last_evaluated = evaluated;
        self.stats.last_mode = EvalMode::Incremental;
        EvalReport {
            mode: EvalMode::Incremental,
            dirty: ids.into_iter().collect(),
            evaluated,
        }
    }

    /// Evaluate the whole document (used for the cold path and for the
    /// "re-evaluate everything" control that demonstrably matches the
    /// incremental result).
    pub fn refresh_full(&mut self, doc: &Document, ctx: &EvaluationContext) -> EvalReport {
        let evaluation = GeometryEvaluator.evaluate_full(doc, ctx);
        let dirty: Vec<NodeId> = doc.order.clone();
        self.scene = evaluation.scene;
        self.diagnostics = evaluation.diagnostics;
        self.warm = true;

        self.stats.full_evals += 1;
        self.stats.last_dirty = dirty.len();
        self.stats.last_evaluated = self.scene.len();
        self.stats.last_mode = EvalMode::Full;
        EvalReport {
            mode: EvalMode::Full,
            dirty,
            evaluated: self.scene.len(),
        }
    }

    /// Refresh only if the cache is cold (used by snapshot projection, so a
    /// read never pays for an evaluation twice).
    pub fn refresh_if_cold(
        &mut self,
        doc: &Document,
        ctx: &EvaluationContext,
    ) -> Option<EvalReport> {
        if self.warm {
            None
        } else {
            Some(self.refresh_full(doc, ctx))
        }
    }

    /// True ⟺ every cached id is still live geometry: an authored node, or a
    /// virtual operation result (Task 4.0 composes those into the same scene).
    fn scene_consistent_with(&self, doc: &Document) -> bool {
        self.scene.nodes.keys().all(|id| doc.is_geometry_id(*id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vectra_core::{Command, Engine, NodeKind, ParamValue, Parameter};

    fn engine_with_circle() -> (Engine, NodeId) {
        let mut engine = Engine::new();
        let id = vectra_core::new_node_id();
        engine
            .dispatch(Command::CreateNode {
                id,
                kind: NodeKind::circle(100.0, 100.0, 50.0),
                name: Some("c".to_string()),
                index: None,
            })
            .unwrap();
        engine
            .dispatch(Command::SetVariable {
                name: "base".to_string(),
                value: 30.0,
            })
            .unwrap();
        (engine, id)
    }

    #[test]
    fn cold_cache_evaluates_then_patches_only_dirty_ids() {
        let (mut engine, id) = engine_with_circle();
        let mut cache = IncrementalScene::new();
        let ctx = engine.evaluation_context();

        let first = cache.refresh(engine.document(), &ctx, &[]);
        assert_eq!(first.mode, EvalMode::Full, "cold cache is a full pass");
        assert_eq!(cache.node_count(), 1);
        assert_eq!(cache.stats().full_evals, 1);

        // Bind the radius to a variable, then dirty just that node.
        engine
            .dispatch(Command::SetParameter {
                node_id: id,
                property: "radius".to_string(),
                value: ParamValue::Float(Parameter::variable("base")),
            })
            .unwrap();
        let ctx = engine.evaluation_context();
        let report = cache.refresh(engine.document(), &ctx, &[id]);
        assert_eq!(report.mode, EvalMode::Incremental);
        assert_eq!(report.dirty, vec![id]);
        assert_eq!(report.evaluated, 1);
        let node = cache.scene().get(id).unwrap();
        match node.primitive {
            vectra_geometry::EvaluatedPrimitive::Circle { r, .. } => assert_eq!(r, 30.0),
            ref other => panic!("unexpected primitive {other:?}"),
        }
        assert_eq!(cache.stats().incremental_evals, 1);
        assert_eq!(cache.stats().full_evals, 1);
    }

    #[test]
    fn empty_dirty_set_is_a_no_op_not_a_full_pass() {
        let (engine, _id) = engine_with_circle();
        let mut cache = IncrementalScene::new();
        let ctx = engine.evaluation_context();
        cache.refresh_full(engine.document(), &ctx);
        let before = cache.stats();
        let report = cache.refresh(engine.document(), &ctx, &[]);
        assert!(report.is_noop());
        assert_eq!(report.evaluated, 0);
        assert_eq!(cache.stats().full_evals, before.full_evals);
        assert_eq!(cache.stats().no_ops, 1);
    }

    #[test]
    fn deleted_nodes_leave_the_cache_and_z_order() {
        let (mut engine, id) = engine_with_circle();
        let mut cache = IncrementalScene::new();
        let ctx = engine.evaluation_context();
        cache.refresh_full(engine.document(), &ctx);
        assert_eq!(cache.scene().z_order.len(), 1);

        engine.dispatch(Command::DeleteNode { id }).unwrap();
        let ctx = engine.evaluation_context();
        let report = cache.refresh(engine.document(), &ctx, &[id]);
        assert_eq!(report.mode, EvalMode::Incremental);
        assert_eq!(cache.node_count(), 0);
        assert!(cache.scene().z_order.is_empty());
    }
}
