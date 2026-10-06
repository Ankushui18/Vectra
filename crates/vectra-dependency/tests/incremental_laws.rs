//! Task 2.2 §2–§4 laws: the patched scene cache is *exactly* the rebuilt scene,
//! and it gets there without re-evaluating the document.
//!
//! These are the laws that make incrementality safe to ship:
//!
//! 1. [`law_patch_equals_full_rebuild`] / [`prop_patch_equals_full_rebuild`] —
//!    after every step of a random edit stream, the incrementally patched cache
//!    is bit-identical to a scene rebuilt from scratch (same fingerprint, same
//!    diagnostics). This is the law that catches a dirty set that is too
//!    *small*.
//! 2. [`law_the_canvas_never_re_evaluates_the_document`] — the cache evaluates
//!    only the dirty nodes; unrelated layers are neither re-evaluated nor
//!    touched (a dirty set that is too *large*, or an accidental full sweep,
//!    fails here).
//! 3. [`law_scene_stays_coherent_with_the_document`] /
//!    [`law_force_full_equals_incremental`] — the cache tracks document
//!    membership through create/delete/undo, and the engine's own
//!    "re-evaluate everything" control lands on the same pixels.
//! 4. `perf_*` — propagation and patch cost scale with the affected subgraph,
//!    not with document size, and the immediate tier of MES §18 ("simple
//!    parameter changes propagate within 2ms") holds end to end.
//!
//! The cache is driven by the *production* dirty derivation
//! (`DependencyGraph::dirty_ids_for_events`) through
//! [`common::Harness::patch_scene`], so these laws exercise the same code path
//! the WASM engine runs — never a private re-implementation.

mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

use common::{apply_random_op, scene_fingerprint, Harness};
use proptest::prelude::*;
use vectra_core::{
    new_node_id, Command, EngineEvent, EvalMode, Node, NodeId, NodeKind, ParamValue, Parameter,
};
use vectra_dependency::{EvalReport, IncrementalScene};
use vectra_geometry::Diagnostic;

/// §18 "typical icon": the scale the 2ms immediate budget is written for.
const TYPICAL_ICON_NODES: usize = 500;
/// Far beyond §18's scale — used only to show that *incremental* paths do not
/// grow with the document.
const OVERSIZED_NODES: usize = 4_000;
/// MES §18: simple parameter changes propagate within 2ms.
const IMMEDIATE_BUDGET_MS: f64 = 2.0;

// ── Diagnostics bookkeeping ──────────────────────────────────────────────

/// Diagnostics grouped per node so two caches compare field-by-field with a
/// readable diff on failure. Document-level notes (no node) get a sentinel key.
fn notes_of(diagnostics: &[Diagnostic]) -> BTreeMap<String, Vec<String>> {
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for d in diagnostics {
        let key = match &d.node_id {
            Some(id) => id.to_string(),
            None => "<document>".to_string(),
        };
        out.entry(key)
            .or_default()
            .push(format!("{:?}|{:?}|{}", d.severity, d.code, d.message));
    }
    for messages in out.values_mut() {
        messages.sort();
    }
    out
}

fn cached_notes(h: &Harness) -> BTreeMap<String, Vec<String>> {
    notes_of(h.cache.diagnostics())
}

/// The oracle: a cache that has never seen this document, evaluated in one
/// full pass. Returned by value so callers can compare *both* the scene and the
/// diagnostics the patched cache is claiming.
fn cold_rebuild(h: &Harness) -> IncrementalScene {
    let mut oracle = IncrementalScene::new();
    let ctx = h.ctx();
    let report = oracle.refresh_full(h.doc(), &ctx);
    assert_eq!(report.mode, EvalMode::Full);
    // A full pass evaluates every node it can render; nodes skipped for an
    // unresolvable parameter still carry their diagnostic (that is the whole
    // point of total evaluation), so compare against the scene, not the doc.
    assert_eq!(report.evaluated, oracle.node_count());
    oracle
}

/// Refresh the harness cache with an explicit dirty set and hand back the
/// report (the harness's `patch_scene` does this from events and drops the
/// report; the laws need to inspect it).
fn refresh_with(h: &mut Harness, dirty: &[NodeId]) -> EvalReport {
    let Harness {
        engine,
        expressions,
        cache,
        ..
    } = h;
    let ctx = engine.evaluation_context().with_expression(expressions);
    cache.refresh(engine.document(), &ctx, dirty)
}

// ── Fixtures ─────────────────────────────────────────────────────────────

/// A document with `nodes` circles, exactly one of which (the anchor) has its
/// radius bound to `$base`.
///
/// Built through the document API rather than `CreateNode` commands: this
/// fixture is about *size*, and dispatching N commands would run N graph syncs
/// (O(N²)) for no law-relevant reason. The harness then syncs and warms once,
/// which is the state every edit in these laws starts from.
fn scaled_harness(nodes: usize) -> (Harness, NodeId) {
    let mut h = Harness::new();
    let anchor = new_node_id();
    {
        let doc = h.engine.document_mut();
        doc.variables.insert("base".to_string(), 100.0);
        let mut a = Node::new(anchor, "anchor", NodeKind::circle(10.0, 10.0, 1.0));
        a.set_param(
            "radius",
            ParamValue::Float(Parameter::variable("base".to_string())),
        )
        .expect("bind anchor radius to $base");
        doc.insert_node(a, None).expect("insert anchor");
        for i in 0..nodes.saturating_sub(1) {
            let id = new_node_id();
            doc.insert_node(
                Node::new(
                    id,
                    format!("n{i}"),
                    NodeKind::circle((i % 50) as f64, (i / 50) as f64, 4.0),
                ),
                None,
            )
            .expect("insert filler");
        }
    }
    h.settle();
    h.force_full_refresh();
    assert_eq!(h.doc().nodes.len(), nodes);
    assert_eq!(h.cache.node_count(), nodes);
    (h, anchor)
}

/// Median wall time of `runs` calls, with one untimed warm-up call.
/// The **best** of `runs` samples: the estimator that measures *work*.
///
/// A wall-clock sample is bounded below by the true cost and above by the
/// scheduler — and on a shared machine the two can be orders of magnitude apart.
/// Measured here on a loaded box: a one-node patch whose real cost is ~60 µs
/// reported a *median* of 16.3 ms, because a 60 µs measurement on a contended
/// core is mostly "waiting to be scheduled", and the median lands on the
/// scheduler's time slice rather than on the work. The minimum is the one sample
/// that ran without being preempted, so it answers "which of these two does less
/// work" — which is what a comparison between two differently-sized operations
/// is actually claiming. (Absolute, user-facing budgets keep using the median:
/// a user feels the median, not the best case.)
fn timed_best<T>(runs: usize, mut f: impl FnMut() -> T) -> (Duration, T) {
    let mut out = f();
    let mut best = Duration::MAX;
    for _ in 0..runs {
        let start = Instant::now();
        out = f();
        best = best.min(start.elapsed());
    }
    (best, out)
}

fn timed<T>(runs: usize, mut f: impl FnMut() -> T) -> (Duration, T) {
    let mut out = f();
    let mut samples = Vec::with_capacity(runs);
    for _ in 0..runs {
        let start = Instant::now();
        out = f();
        samples.push(start.elapsed());
    }
    samples.sort();
    (samples[samples.len() / 2], out)
}

// ── §2 patch ≡ rebuild ───────────────────────────────────────────────────

/// §2 `law_patch_equals_full_rebuild`: the brief's shape with known arithmetic
/// — 500 nodes, one variable change, one dirty node, both evaluation paths
/// agreeing to the bit.
#[test]
fn law_patch_equals_full_rebuild() {
    let (mut h, anchor) = scaled_harness(TYPICAL_ICON_NODES);
    let patched_before = scene_fingerprint(h.cache.scene());

    let events = h
        .dispatch(Command::SetVariable {
            name: "base".to_string(),
            value: 200.0,
        })
        .expect("SetVariable $base");
    let dirty = h.graph.dirty_ids_for_events(&events);
    assert_eq!(dirty, vec![anchor], "only the bound node is dirty");
    assert_eq!(h.resolve(anchor, "radius").unwrap(), 200.0);

    let oracle = cold_rebuild(&h);
    assert_eq!(
        scene_fingerprint(h.cache.scene()),
        scene_fingerprint(oracle.scene()),
        "a one-node patch must equal a {TYPICAL_ICON_NODES}-node rebuild"
    );
    assert_eq!(cached_notes(&h), notes_of(oracle.diagnostics()));
    assert_ne!(
        scene_fingerprint(h.cache.scene()),
        patched_before,
        "the edit must actually be visible in the scene"
    );
}

/// §2 `law_patch_equals_full_rebuild` (property form): whatever the edit stream
/// does — including undo/redo, which applies a *different* command than the one
/// that produced the current state — patching must land on the rebuilt scene.
#[test]
fn prop_patch_equals_full_rebuild() {
    let mut runner = proptest::test_runner::TestRunner::default();
    let strategy = proptest::collection::vec((0u8..12, 0u8..64, 0u8..16), 1..48);
    runner
        .run(&strategy, |ops| {
            let mut h = Harness::new();
            for (op, arg, extra) in ops {
                apply_random_op(&mut h, op, arg, extra);

                let oracle = cold_rebuild(&h);
                prop_assert_eq!(
                    scene_fingerprint(h.cache.scene()),
                    scene_fingerprint(oracle.scene()),
                    "patched cache diverged from a cold rebuild"
                );
                prop_assert_eq!(
                    cached_notes(&h),
                    notes_of(oracle.diagnostics()),
                    "diagnostics diverged from a cold rebuild"
                );
                // Structural invariants every step (these live with the graph
                // laws, but the cache must not break them either).
                h.assert_invariants();
            }
            Ok(())
        })
        .expect("patch ≡ rebuild under random edits");
}

// ── §3 minimal evaluation ────────────────────────────────────────────────

/// §3: a variable edit re-evaluates its dependents and *nothing else* — no
/// other row is recomputed, no other value is touched, and the engine never
/// falls back to a full sweep.
#[test]
fn law_the_canvas_never_re_evaluates_the_document() {
    let (mut h, anchor) = scaled_harness(TYPICAL_ICON_NODES);
    let stats_before = h.cache.stats();
    assert_eq!(stats_before.full_evals, 1, "fixture warmed once");
    let others: Vec<(NodeId, f64)> = h
        .doc()
        .order
        .iter()
        .filter(|id| **id != anchor)
        .map(|id| (*id, h.resolve(*id, "radius").unwrap()))
        .collect();

    let events = h
        .dispatch(Command::SetVariable {
            name: "base".to_string(),
            value: 137.0,
        })
        .expect("SetVariable $base");
    let dirty = h.graph.dirty_ids_for_events(&events);
    assert_eq!(dirty, vec![anchor]);

    let report = refresh_with(&mut h, &dirty);
    assert_eq!(report.mode, EvalMode::Incremental);
    assert_eq!(report.evaluated, 1, "exactly one layer was re-evaluated");
    let stats = h.cache.stats();
    assert_eq!(
        stats.full_evals, stats_before.full_evals,
        "an edit must never trigger a full rebuild"
    );
    assert!(stats.incremental_evals > stats_before.incremental_evals);

    // The bound node followed the variable…
    assert_eq!(h.resolve(anchor, "radius").unwrap(), 137.0);
    // …while every unrelated layer kept its exact value and was never dirty.
    for (id, value) in others {
        assert_eq!(
            h.resolve(id, "radius").unwrap(),
            value,
            "unrelated value moved"
        );
        assert!(!dirty.contains(&id), "an unrelated node was dirty");
    }
}

// ── §2/§3 scene coherence + the full-sweep control ───────────────────────

/// §3: the cache is always a faithful projection of the document — same
/// membership, same z-order — across create, bind, delete, undo and redo.
#[test]
fn law_scene_stays_coherent_with_the_document() {
    let mut h = Harness::new();
    let a = new_node_id();
    let b = new_node_id();

    let expect_coherent = |h: &Harness, label: &str| {
        let scene = h.cache.scene();
        let scene_ids: BTreeSet<NodeId> = scene.nodes.keys().copied().collect();
        let doc_ids: BTreeSet<NodeId> = h.doc().nodes.keys().copied().collect();
        assert_eq!(scene_ids, doc_ids, "{label}: scene membership drifted");
        assert_eq!(scene.z_order, h.doc().order, "{label}: z-order drifted");
        assert_eq!(h.cache.node_count(), h.doc().nodes.len());
    };

    h.dispatch(Command::SetVariable {
        name: "base".to_string(),
        value: 60.0,
    })
    .expect("SetVariable");
    h.dispatch(Command::CreateNode {
        id: a,
        kind: NodeKind::circle(0.0, 0.0, 5.0),
        name: Some("a".to_string()),
        index: None,
    })
    .expect("CreateNode a");
    h.dispatch(Command::SetParameter {
        node_id: a,
        property: "radius".to_string(),
        value: ParamValue::Float(Parameter::variable("base".to_string())),
    })
    .expect("bind a.radius");
    h.dispatch(Command::CreateNode {
        id: b,
        kind: NodeKind::rectangle(0.0, 0.0, 5.0, 5.0),
        name: Some("b".to_string()),
        index: None,
    })
    .expect("CreateNode b");
    expect_coherent(&h, "after three mutations");

    // Deleting the bound node drops it from the scene…
    h.dispatch(Command::DeleteNode { id: a })
        .expect("DeleteNode a");
    expect_coherent(&h, "after delete");
    assert!(!h.cache.scene().nodes.contains_key(&a));
    assert!(h.resolve(b, "width").is_ok());

    // …and undo brings it back, pixels included.
    let before_delete = h
        .dispatch(Command::SetVariable {
            name: "base".to_string(),
            value: 61.0,
        })
        .map(|_| scene_fingerprint(h.cache.scene()))
        .expect("SetVariable");
    h.undo().expect("undo SetVariable");
    h.undo().expect("undo DeleteNode");
    expect_coherent(&h, "after undo");
    assert!(h.cache.scene().nodes.contains_key(&a));
    let rebuilt = cold_rebuild(&h);
    assert_eq!(
        scene_fingerprint(h.cache.scene()),
        scene_fingerprint(rebuilt.scene())
    );
    assert_ne!(before_delete, scene_fingerprint(h.cache.scene()));
}

/// §3: the engine's own "re-evaluate everything" control is not just equivalent
/// to the incremental cache — it *is* the same scene, and the cache stays
/// capable of patching afterwards.
#[test]
fn law_force_full_equals_incremental() {
    let mut h = Harness::new();
    for (op, arg, extra) in [
        (0u8, 3u8, 7u8),
        (2, 1, 4),
        (5, 0, 2),
        (8, 1, 1),
        (1, 2, 9),
        (12, 0, 0),
        (4, 0, 3),
        (9, 0, 3),
    ] {
        apply_random_op(&mut h, op, arg, extra);
    }

    let patched = scene_fingerprint(h.cache.scene());
    let notes = cached_notes(&h);
    let report = h.force_full_refresh();
    assert_eq!(report.mode, EvalMode::Full);
    assert_eq!(report.evaluated, h.cache.node_count());
    assert_eq!(
        scene_fingerprint(h.cache.scene()),
        patched,
        "patch ≡ rebuild"
    );
    assert_eq!(cached_notes(&h), notes, "diagnostics must survive a sweep");
    assert_eq!(h.cache.stats().full_evals, 2, "the sweep is counted");

    // A warm-but-swept cache keeps patching.
    let (mut h, anchor) = scaled_harness(TYPICAL_ICON_NODES);
    let events = h
        .dispatch(Command::SetVariable {
            name: "base".to_string(),
            value: 512.0,
        })
        .expect("SetVariable");
    let dirty = h.graph.dirty_ids_for_events(&events);
    let report = refresh_with(&mut h, &dirty);
    assert_eq!(report.mode, EvalMode::Incremental);
    assert_eq!(report.evaluated, 1);
    assert_eq!(h.resolve(anchor, "radius").unwrap(), 512.0);
}

// ── §4 perf: O(affected), not O(document) ────────────────────────────────

/// §4: dirty propagation is proportional to the affected subgraph. Same single
/// dependent, 8-node document vs 4 000-node document → statistically the same
/// work. This is the law that fails the moment someone makes propagation scan
/// the document.
#[test]
fn perf_propagation_is_independent_of_document_size() {
    let mut timings = Vec::new();
    for nodes in [8usize, OVERSIZED_NODES] {
        let (h, anchor) = scaled_harness(nodes);
        let events = vec![EngineEvent::VariablesUpdated {
            names: vec!["base".to_string()],
        }];
        let (elapsed, dirty) = timed(20, || h.graph.dirty_ids_for_events(&events));
        assert_eq!(dirty, vec![anchor], "{nodes}-node doc: wrong dirty set");
        timings.push((nodes, elapsed));
    }

    let (small_nodes, small) = timings[0];
    let (large_nodes, large) = timings[1];
    let ratio = large.as_secs_f64() / small.as_secs_f64().max(f64::EPSILON);
    println!(
        "propagation: {:.2}µs ({small_nodes} nodes) vs {:.2}µs ({large_nodes} nodes) → ratio {ratio:.2}",
        small.as_secs_f64() * 1e6,
        large.as_secs_f64() * 1e6
    );
    // Both fixtures touch the same single node; a 500× document must not cost
    // meaningfully more. Budget ×3 is generous for timer noise while still
    // failing an O(document) implementation by two orders of magnitude.
    assert!(
        ratio < 3.0,
        "propagation scaled with the document: {small:?} → {large:?} (ratio {ratio:.2})"
    );
}

/// §18 "immediate" tier: a simple parameter change — gate + dispatch + graph
/// reconciliation + dirty propagation + scene patch — must stay inside 2ms.
/// The fixture is the 500-node "typical icon" the budget is written for; the
/// debug multiplier keeps the law meaningful under `cargo test` while the
/// release number is the one that counts (Task 2.1 precedent).
#[test]
fn perf_immediate_edit_stays_inside_the_mes_18_budget() {
    let (mut h, anchor) = scaled_harness(TYPICAL_ICON_NODES);
    let mut value = 0.0;
    let (measured, dirty) = timed(25, || {
        value += 1.0;
        let events = h
            .dispatch(Command::SetVariable {
                name: "base".to_string(),
                value,
            })
            .expect("SetVariable");
        h.graph.dirty_ids_for_events(&events)
    });
    assert_eq!(dirty, vec![anchor]);

    let budget_ms = if cfg!(debug_assertions) {
        IMMEDIATE_BUDGET_MS * 12.0
    } else {
        IMMEDIATE_BUDGET_MS
    };
    println!(
        "SetVariable end-to-end on {} nodes: {:.2}µs (MES §18 immediate budget {IMMEDIATE_BUDGET_MS} ms)",
        h.doc().nodes.len(),
        measured.as_secs_f64() * 1e6
    );
    assert!(
        measured.as_secs_f64() * 1e3 < budget_ms,
        "immediate edit took {:.3}ms (budget {budget_ms}ms)",
        measured.as_secs_f64() * 1e3
    );
}

/// §4: one node patched vs the whole document rebuilt. Structural assertions
/// (exact evaluation counts) plus the timing as evidence.
#[test]
fn perf_one_node_patch_versus_full_rebuild() {
    let (mut h, _anchor) = scaled_harness(OVERSIZED_NODES);
    let events = vec![EngineEvent::VariablesUpdated {
        names: vec!["base".to_string()],
    }];
    let dirty = h.graph.dirty_ids_for_events(&events);

    // Best-of, not median: the claim is about *work*, and a wall-clock median on
    // a contended box measures the scheduler (see `timed_best`).
    let (patch_time, patch_report) = timed_best(30, || refresh_with(&mut h, &dirty));
    assert_eq!(patch_report.evaluated, 1);
    assert_eq!(patch_report.mode, EvalMode::Incremental);
    let patched = scene_fingerprint(h.cache.scene());
    let stats_after_patch = h.cache.stats();

    let (full_time, full_report) = timed_best(10, || h.force_full_refresh());
    assert_eq!(full_report.evaluated, h.cache.node_count());
    assert_eq!(full_report.mode, EvalMode::Full);

    let nodes = h.doc().nodes.len();
    println!(
        "evaluation (best of N): patch {:.2}µs (1 node) vs full {:.2}µs ({nodes} nodes)",
        patch_time.as_secs_f64() * 1e6,
        full_time.as_secs_f64() * 1e6
    );
    assert!(
        patch_time < full_time,
        "patching one node must beat rebuilding {nodes}: {patch_time:?} vs {full_time:?}"
    );
    assert_eq!(
        scene_fingerprint(h.cache.scene()),
        patched,
        "the last full sweep must land on the patched scene"
    );
    // Both helpers run one untimed warm-up call; every timed call above is a
    // full sweep, and the patches before them added none. (The exact count is
    // asserted through the cache's own statistics, so it cannot drift.)
    assert_eq!(
        h.cache.stats().full_evals,
        stats_after_patch.full_evals + 11,
        "sweeps are counted, patches are not"
    );
    assert_eq!(
        h.cache.stats().incremental_evals,
        stats_after_patch.incremental_evals,
        "a full sweep must not register as a patch"
    );
}
