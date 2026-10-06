# Task 4.0 — Non-Destructive Operations: report

**Status: complete.** Operations are virtual nodes over untouched sources; booleans come
from `geo`; the results are ordinary scene nodes the UI can select, style and see
re-evaluated. All gates green.

Companion documents: `TASK-4.0-DESIGN.md` (the design, the deviations and the open items),
`TASK-3.2-REPORT.md` / `TASK-3.2-DESIGN.md` (the drag triad this builds on).

---

## 1. The data model

```rust
// vectra-core
pub type OperationId = NodeId;   // RULE 3 — one id space, no conversion layer

pub struct OperationNode {
    pub id: OperationId,
    pub kind: OperationKind,
    pub inputs: Vec<NodeId>,     // operand order is meaningful (Subtract: [subject, clip])
    pub enabled: bool,
    pub name: String,
    pub style: StyleProperties,  // default: #f28c28, so a result reads as derived
}

pub enum OperationKind {
    Boolean { op: BooleanOp },                 // arity 2  → tag "boolean"
    Offset  { distance: Parameter<f64> },      // arity 1  → tag "offset"
    Fillet  { radius:   Parameter<f64> },      // arity 1  → tag "fillet"
    Mirror  { axis: MirrorAxis },              // arity 1  → tag "mirror"
}
pub enum BooleanOp { Union, Subtract, Intersect, Exclude }   // tags + glyphs ∪ − ∩ ⊕
pub enum MirrorAxis { Vertical { at: Parameter<f64> }, Horizontal { at: Parameter<f64> } }
```

`OperationRegistry { nodes: BTreeMap<OperationId, OperationNode>, order: Vec<OperationId> }`
holds the records; `Document::geometry_ids()` enumerates authored nodes then **enabled**
operations in draw order, and `Document::is_geometry_id(id)` is the single liveness
predicate the scene layer agrees on. Every operand is a `Parameter<f64>`, so an operation's
own number is animatable/bindable exactly like a primitive's slot.

Commands (all undoable, exact inverses, integrated in `Engine::dispatch`):

| command | effect | inverse |
| --- | --- | --- |
| `ApplyOperation { id, kind, inputs }` | registers the virtual node; **touches no source** | `RemoveOperation { id }` |
| `RemoveOperation { id }` | withdraws the result, keeps the sources | `ApplyOperation` with the removed record |
| `SetOperationEnabled { id, enabled }` | park / re-arm (keeps row + inputs) | same command, previous flag |

`DeleteNode` of a source cascades: the orphaned operations are withdrawn in the *same*
batch, so one undo restores node + operations together, and no dangling virtual shape can
survive. `EngineEvent::OperationsUpdated { ids }` announces registry changes; the geometry
change arrives as those ids in the following `Dirty` event.

## 2. The `geo` integration pipeline (RULE 2)

```
EvaluatedScene input node (Path|Rect|Circle|Arc)
  → vectra_geometry::primitive_to_path            (lyon Path, f32)
  → convert::path_to_multi_polygon                (flatten @0.05, one ring per subpath,
                                                   parity canonicalised via union(&empty))
  → ops::{boolean | offset | fillet | mirror}     (geo::BooleanOps, Buffer, own fillet)
  → convert::multi_polygon_to_path                (exteriors CCW, interiors CW)
  → EvaluatedNode { primitive: Path }             (RULE 3 — an ordinary scene node)
```

No boolean arithmetic is written by hand: union/difference/intersection/xor come from
`geo::boolean::BooleanOps`, offsetting from `geo::algorithm::buffer::Buffer`, mirroring from
`geo::AffineTransform` + `AffineOps`, all over `geo::MultiPolygon`. `vectra-operations` is
the only crate that depends on `geo`; nothing above it speaks polygons.

Even-odd canonicalisation is load-bearing, not cosmetic: lyon paths have no fill rule and
`geo::boolean` is even-odd, so a nested ring must become an *interior ring* before any
operation or the hole is unioned away as a solid. The failure mode is asserted against:
outer 10×10 + inner 4×4 → 84.0 exactly, at the region level (1e-9) and after the round
trip back to a path (1e-6).

## 3. Dirty propagation

```
dirty source ids ─┬─▶ pending_operations (registry changes in this batch)
                  └─▶ registry.affected_by(&dirty)  ← one-level scan, deterministic order
                                        │
                                        ▼
                      OperationsEvaluator → compose_into(scene) → ids merged into `Dirty`
```

`VectraEngine::settle` runs, in order: **retire** (`prune_disabled_operations` — removed or
parked operations lose their geometry before the cache is validated) → **patch** (the
Task-2.2 primitive pass) → **operations** (the virtual pass, reading this frame's source
geometry) → **merge** the recomputed operation ids into the `Dirty` event. `set_time` now
runs the same sequence, so a `$time`-driven source drags its operation along; the clocks'
witness is `law_the_clock_reaches_operations`. `force_full_evaluation` recomputes the
virtual layer too, so `patch ≡ rebuild` still holds with operations in the scene.

The fan-out is a scan over the registry rather than a `StableGraph` traversal: Phase 1
operations read authored nodes one level deep, so the dependency is already fully described
by the record's `inputs`. Consequences and the revisit condition are in the design doc §5/§11.

## 4. Test results

| suite | result |
| --- | --- |
| `crates/vectra-operations` unit (convert + ops) | **8 / 8** |
| `crates/vectra-operations/tests/operation_laws.rs` | **12 / 12** (stable over 8 consecutive runs) |
| `crates/vectra-wasm/tests/operation_laws.rs` | **10 / 10** (stable over 5 consecutive runs) |
| `apps/vectra-web/scripts/smoke.mjs` | **24 / 24 steps** on the real wasm module |
| `apps/vectra-web/tests/view-model.test.ts` | **23 / 23** |

The three laws named in the task, each proven at the engine level *and* through the
protocol:

* **Boolean Satisfaction Law** — `area(Subtract(A,B)) == area(A) − area(A ∩ B)`: per-fixture
  assertions across disjoint/overlapping/nested × all four ops, plus a 64-case proptest.
* **Non-Destructive Source Law** — moving B leaves A byte-identical while the operation
  re-runs: engine law, wire law (`Dirty` names the operation, the partner's node is
  unchanged), and smoke step 23.
* **Undo Law** — one undo removes the `OperationNode` and leaves the sources exactly as
  authored: engine law + a 64-case proptest asserting the undo is an exact pre-image, plus
  wire law (`revert`), plus delete-cascade.

Numbers worth keeping (verified, not estimated): side-10 square filleted `r=2` → 96.494225
sampled vs 96.566370 analytic (−0.075 % discretisation); `r=50` clamps to 78.088904 rather
than erroring; `r=0` is an exact no-op (100.0); a non-finite radius is a typed error;
offset `+2` on 10×10 ≈ 192.57 (within 2 %); mirror preserves area exactly and flips
coordinates; 10×10 minus a 4×4 hole is exactly 84.0.

## 5. Workspace gate

* `cargo fmt --check` — clean
* `cargo clippy --workspace --all-targets -- -D warnings` — **0 warnings**
* `cargo test --workspace` — **32 suites, 211 passed, 0 failed** (was 30 / 181 before Task 4.0)
* `npm run typecheck` — clean · `npm run test:ui` — 23 / 23 · `npm run build` — OK
* `node scripts/smoke.mjs` — 24 / 24 through the compiled wasm module

Two bugs the law suites caught during bring-up, both fixed at the source rather than in the
assertion: (1) the scene cache held geometry for a *parked* operation because
`is_geometry_id` ignored `enabled` (§4 of the design doc); (2) the ops pass ran before the
primitive pass, so it read the previous frame's sources.

## 6. UI (requirement 5)

A new **Operations** section sits between Layers and Constraints: **Union / Subtract /
Intersect / Exclude** on a two-source selection (`selected[0]` is the left operand —
Subtract removes `selected[1]` from it) and **Offset / Fillet / Mirror** on a one-source
selection with a value field. The registry list shows each operation's kind tag, its
engine-rendered description, its operands (`plate − hole`), a park/enable checkbox and a
remove button.

The **Layers panel** now renders a tree: operations are indented beneath the source they
read first (`inputs[0]`), tagged amber, with the operand summary. Operation rows are not
selectable as operands — Phase 1 has no operation-on-operation nesting, so the panel never
sends a command the engine would refuse (the engine still validates; smoke step 24 proves a
stray operand is a typed error). The result itself is drawn by the existing canvas with no
special case, because it is a `path` primitive in the snapshot.

Everything the UI does here is a projection: captions come from `input_names`/`description`,
the nesting comes from `inputs`, and the panel adds no arithmetic of its own.

## 7. How to reproduce

```bash
export PATH=/usr/local/cargo/bin:$PATH RUSTUP_HOME=/usr/local/rustup CARGO_HOME=/usr/local/cargo
cd /home/user/vectra

cargo test -p vectra-operations            # unit + laws
cargo test -p vectra-wasm --test operation_laws
cargo test --workspace                     # the gate

cd apps/vectra-web
npm install                                # node_modules is not snapshotted
bash scripts/build-wasm.sh                 # needs wasm-bindgen-cli 0.2.129 installed
node scripts/smoke.mjs
npm run test:ui && npm run typecheck && npm run build
npm run dev                                # vite prints the URL (live preview here: :5175)
```

In the browser: add two shapes, click both Layers rows, press **Subtract** — the result
appears as an amber `path` indented under the first source, the first source stays exactly
where it was, and dragging it re-runs only the operation (the log names the dirty id). One
undo removes the result and leaves both sources intact.

## 8. Known limits (carried into the design doc's open items)

* No operation-on-operation nesting, and therefore no cycle risk to police yet.
* No `SetOperationParam`: an operation's operand is fixed at creation (bind the *source*,
  or remove + re-apply, which is undoable). Argued in the design doc §2.
* `affected_by` is a scan, not a graph edge; `evaluate_all` re-encodes inputs each pass.
  Both are deliberate at Phase-1 scale and both are the first things to change if the
  renderer (Task 5.0) makes the virtual layer hot.
* Even-odd is the only modelled fill rule.
