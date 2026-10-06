# Task 2.2 — Dependency Graph & Incremental Evaluation

**Status: complete and green.** Crate `vectra-dependency` (+ `vectra-wasm` composition
root + React remote control), 2026-10-05.

---

## 1. Graph implementation & `Engine::dispatch` integration

**Crate** `crates/vectra-dependency`: `graph.rs` (topology + propagation),
`prospective.rs` (edge deltas + gate), `incremental.rs` (patched scene cache),
`wire.rs` (deterministic JSON projection), re-exported from `lib.rs`.

**Data structures** — `petgraph::stable_graph::StableGraph<GraphNode, DependencyEdge>`
(**StableGraph, not `Graph`**: node removal must not renumber survivors, which is
what makes undo/redo of graph elements exact), plus `node_to_index: HashMap<GraphNode,
NodeIndex>` for O(1) lookup and an edge map `HashMap<DagEdge, EdgeIndex>` for O(1)
edge identity/removal. `DependencyEdge` is a marker type — every edge means the
same thing: *from depends on to*.

```rust
enum GraphNode { Variable(VariableId), Expression(ExpressionId),
                 GeometryProperty(NodeId, String) }   // + clock() = Variable("time")
```

**Maintenance protocol** — post-mutation, in the composition root
(`vectra_wasm::VectraEngine::settle`, because `vectra-core` must never depend on
`petgraph`):

```
core.dispatch(cmd)                 // document + history
  → expressions.sync_from_document // compiled registry (Task 2.1)
  → graph.sync(doc)                // dependency graph  (Task 2.2)
  → graph.dirty_ids_for_events(&events)
  → scene.refresh(.., &dirty)      // IncrementalScene → GeometryEvaluator
  → push EngineEvent::Dirty { ids, mode }
```

`sync` is a **derivation, not bookkeeping**: it computes the topology the document
implies (expression `dependencies()` → `Expression → Variable`; every float
parameter's source → `GeometryProperty(node, prop) → Variable | Expression | clock`)
and applies minimal deltas, so the graph provably cannot drift from the document —
which is also why **undo/redo needs no special path**: edges leave and return with
the document. `sync` returns a report (nodes/edges added/removed + problems) and
`validate()` re-checks every index/edge invariant.

**Propagation** — `affected(&changed)` = downstream closure in *evaluation order*
(Kahn, dependencies first, deterministic tie-breaking); `dirty_set()` returns
`Option<DirtySet>` — `None` means "nothing to do", deliberately distinct from an
empty `DirtySet` (which means "evaluate everything"); `dirty_ids()` is the sorted
id list events and stats use. The three-vertex chain from the brief propagates
exactly: `$base → ƒx → rect • width`.

**Integration is pre-gated**: `dispatch`, `undo` and `redo` all run
`gate_command(&graph, &cmd, doc)` **before** touching the document (`undo`/`redo`
gate the pending *inverse*, so rewind cannot introduce a cycle either). The clock
rides the same path (`set_time` dirties exactly the readers of `$time`).

## 2. Cycle mechanism & safe rejection

* **Pre-application, atomic**: `dry_run(removes, adds)` clones the graph on a scratch
  copy, applies the command's predicted removals, then adds the predicted edges via
  `try_add_edges` — which is itself atomic (tentative add, exact rollback on
  rejection, placeholder nodes pruned). The real graph is *never* mutated, so the
  cycle does not exist "even for a frame": no record, no undo entry, no edge.
* **Typed**: `VectraError::CyclicDependency { .. }` (`is_cycle_rejection()`), message
  names the offending pair and why it closes the loop; the wire returns
  `{status:"error", message}` and the UI logs it in the diagnostics log (red line),
  exactly like any other typed rejection.
* **Prediction fidelity**: `prospective_edges` reproduces the command's true topology
  effect (removals included) and is asserted against the observed delta over random
  command sequences (proptest `...gate_predicts_the_topology_delta_exactly`), so the
  gate inspects exactly the edges `sync` would add.

**Honest scope note (reachability).** With Phase-1 semantics — scalars in
`Document.variables`, expressions reading only variables/`$time`, properties as
leaves — the three vertex layers are strictly ordered, so **no command can currently
construct a cycle**: I probed the real engine with the brief's shapes (`$a = $b + 1`,
`$b = $a + 1`, self-references, redefine loops) and every one is legal or a parse
error, because "a variable defined by an expression" doesn't exist until the Phase-4
procedural graph. That edge type is exactly what the cycle law needs, so the law is
stated against the graph API in that shape
(`Variable(a) → Expression`, `Expression → Variable(b)` + `$b = $a + 1` closing the
loop): rejection is asserted with the graph byte-identical, plus 3-cycle and
self-loop variants, plus an invariant proptest ("after any random command sequence
the graph is acyclic and identical to the derived topology"). When variable-defining
expressions land, the gate fires with zero further changes.

## 3. Test results

| Suite | Result |
|---|---|
| `cargo test --workspace` | **142 passed / 0 failed** (15 binaries) |
| `vectra-dependency` lib | 13/13 — index stability, atomic rollback, dedupe, sync deltas, Kahn order, validate, export determinism |
| `tests/dependency_laws.rs` | 8/8 — **Propagation law**, **Cycle law**, **Undo/Redo integrity law**, dirty-only-dependents, 3 proptests (gate fidelity, random-command invariants, undo/redo graph integrity) |
| `tests/incremental_laws.rs` | 7/7 — patch ≡ full (incl. proptest), minimal evaluation, scene coherence, O(affected) independence, the 2 ms budget law |
| `cargo clippy --workspace --all-targets` | **0 warnings** |
| `cargo fmt --all --check` | clean |

Brief §5 laws, as written: *Propagation* — `$base` change → affected set is exactly
`{Ƒx, rect • width}`, in evaluation order, dirty ids `{rect}`, resolved width correct;
*Cycle* — second definition rejected `Err(CyclicDependency)`, graph unchanged;
*Undo/Redo* — variable + expression + geometry node, undo ×4 → graph back to baseline
(0 vertices / 0 edges), no dangling `NodeIndex`, `validate()` clean, redo restores the
exact edge set.

**Performance (release, MES §18 budget 2 ms).** `SetVariable` end-to-end on 502
nodes **38.18 µs** (52× inside budget) · propagation **1.22 µs @10-node doc vs
1.18 µs @4 002-node doc → ratio 0.96**, i.e. O(affected) not O(document) · one-node
patch **183.98 µs** vs full 4 002-node rebuild **902.26 µs**.

## 4. React UI visibly incremental

`npm run build` (tsc + vite) clean · `npm run test:ui` **8/8** (view-model tests,
Node, no DOM) · dev server live.

* **Event log names the dirty nodes**: every mutation's `Dirty { ids, mode }` event is
  rendered as `⛓ dirty: 2 node(s) re-evaluated — 5ab982df, e18418cf`, a full sweep as
  `⟳ full re-evaluation: N node(s)`, and an edit with no dependents as
  `⛓ dirty: 0 nodes — nothing depends on this edit` (an empty set is never printed as
  `[]` nor conflated with a rebuild).
* **Layers panel marks them**: re-evaluated rows get a ⚡ chip + outline from the same
  ids, so "which nodes moved" is visible against the scene.
* **Canvas updates without re-evaluation**: `get_snapshot` is a pure projection of the
  cached `EvaluatedScene` (patched at mutation time); the header chip shows
  `incremental · 2 dirty → 1 evaluated · full ×1 · inc ×17`, and the panel offers
  `⛓ Rect ← $base` / `⛓ Circle ← ƒx` (create already-bound geometry), `↻ Full re-eval`
  (rebuild ≡ patch — the scene does not change, only `full ×1 → ×2`) and the time
  scrubber (dirty from the clock).
* **Dependency inspector**: new panel rendering the engine's graph export
  (`rect • width → depends on → ƒ 3f2a1b7c → $base`, vertex/edge/acyclic summary).
* UI stayed a dumb remote: the only new UI code is `src/engine/view-model.ts` (pure
  projections) — no geometry, no parameter resolution, no state duplication.

**Bug caught by the smoke test**: the scene JSON was not byte-stable across
patch-vs-rebuild (hash-map iteration order leaks evaluation order). Fixed in the wire
projection — it is now **canonical** (scene/variables/expressions sorted, diagnostics
sorted by severity/code/node/message), which is what lets `scripts/smoke.mjs` assert
`patch ≡ rebuild` byte-for-byte.

## 5. Workspace cleanliness & E2E

* `cargo fmt --all --check` clean · `cargo clippy --workspace --all-targets` **0
  warnings** · `cargo test --workspace` 142/0 · `npm run build` clean · `tsc` clean.
* WASM rebuilt (`scripts/build-wasm.sh`, 5.26 MB debug) — previous artifact was stale
  (no graph, no `Dirty` event).
* `scripts/smoke.mjs`: **13/13 PASS** against the real compiled wasm in Node —
  graph export (4 vertices / 3 edges, acyclic), `$base` edit → `Dirty = [rect, idle]`
  with `full_evals` still 1, edit-with-no-dependents → empty dirty + 0 evaluations,
  undo/redo adds and removes vertices/edges exactly, `force_full_evaluation` ≡
  incremental scene and the cache stays warm afterwards.
* MES §8 updated with a binding implementation note (StableGraph supersedes the
  `Graph` sketch; derived topology; gate semantics; reachability caveat; perf).

**Boundary respected**: no renderer, `wgpu`, tessellation or constraint changes; the
graph is topology + propagation only, and every geometry re-evaluation goes through
the existing `GeometryEvaluator` / `DirtySet` interface.

---

## Addendum — post-acceptance incident & re-verification

**What happened.** After this report was accepted, an edit attempt on
`crates/vectra-dependency/tests/incremental_laws.rs` overwrote the file
(instead of adding to it) and `/home/user/vectra` has **no git history**, so
there was no VCS restore path. The accepted 7 laws had passed as part of the
142-test run above; the clobbered file was a non-compiling draft that was never
the accepted artifact.

**Recovery.** The file was rewritten against the real `tests/common` harness
API (`Harness::{dispatch,undo,redo,settle,patch_scene,ctx,doc,resolve,
force_full_refresh,assert_invariants}`, `scene_fingerprint`,
`apply_random_op`) and re-proved from scratch — not restored from a copy, which
does not exist. It is now **8 tests** (the 7 accepted laws + a targeted property
law). One earlier-baseline claim was corrected while re-proving: a full
evaluation's `report.evaluated` counts the nodes it can *render*, so a document
containing an unresolvable node evaluates one fewer than `doc.nodes.len()`
(the skip is carried as a diagnostic, by design). The assertions now compare
against the cache's own node count instead of the document's.

**Re-verified exit gate (Actions 156–160, all re-run from a clean state):**

| check | result |
| --- | --- |
| `cargo test -p vectra-dependency --test incremental_laws` | 8 passed / 0 failed |
| `cargo test --workspace` | **143 passed / 0 failed** |
| `cargo clippy --workspace --all-targets` | 0 warnings · `cargo fmt --all` clean |
| `node scripts/smoke.mjs` | **13/13 PASS** (same banner) |
| `npm run test:ui` · `typecheck` · `build` | 8/8 · clean · clean |
| release perf | SetVariable e2e **34.06 µs** @500 nodes (budget 2 ms) · propagation **1.28 µs @8 vs 1.18 µs @4000 → ratio 0.92** · patch 1 node **174.97 µs** vs full rebuild **709.83 µs** @4000 |

No other file was touched by the incident. The Task 2.2 deliverable as
described above stands; the test file's *law count* is now 8 rather than 7.
