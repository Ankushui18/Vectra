# Task 4.0 — Non-Destructive Operations: design

The requirement, in the user's terms: operations must be **virtual nodes** that read
source geometry and never bake it, the boolean math must come from the **`geo` crate**,
and an `OperationNode` must be an ordinary scene node that can be styled, constrained and
animated like a primitive.

Those three statements are RULE 1, RULE 2 and RULE 3 below. Everything in this document
either implements one of them or is bookkeeping that keeps them true.

---

## 0. Process note — this document is written *after* the code

Task 4.0 was authorised with "Do not ask clarifying questions. Proceed." The
implementation therefore landed code-first: `vectra-core::operation`, the geometry and
dependency plumbing, `vectra-operations`, the wasm integration and both law suites were
written and made green before this file existed. That is a deliberate deviation from the
MES "design, then build" order, and the honest consequence is that this document is
**descriptive** of shipped code rather than prescriptive of planned code. Every claim
below is a claim about a file that exists and a test that runs; where the design *chose*
something over an alternative, the choice is argued in place. Where the code fell short
of the intent, the gap is in §11 instead of being quietly omitted.

What did *not* deviate: the standing rules. No source node is ever mutated by an
operation, no boolean math is hand-rolled, and no operation result is a special case in
the scene.

---

## 1. Data model (`vectra-core::operation`)

```rust
pub type OperationId = NodeId;                      // RULE 3: one id space

pub struct OperationNode {
    pub id: OperationId,
    pub kind: OperationKind,
    pub inputs: Vec<NodeId>,                        // operand order is meaningful
    pub enabled: bool,
    pub name: String,
    pub style: StyleProperties,
}

pub enum OperationKind {
    Boolean { op: BooleanOp },                      // arity 2
    Offset  { distance: Parameter<f64> },           // arity 1
    Fillet  { radius:   Parameter<f64> },           // arity 1
    Mirror  { axis: MirrorAxis },                   // arity 1
}

pub enum BooleanOp { Union, Subtract, Intersect, Exclude }

pub enum MirrorAxis { Vertical { at: Parameter<f64> }, Horizontal { at: Parameter<f64> } }
```

Design decisions here, each load-bearing:

* **`OperationId = NodeId`, not a parallel id type.** RULE 3 says an operation appears in
  `EvaluatedScene` as a standard node. If the ids were different types, every consumer
  (scene cache, snapshot, z-order, styling, selection) would need a conversion and a
  second code path. Aliasing the type means the id space is literally shared: the scene
  is keyed by `NodeId`, and an operation's id is a legal key. It is also what makes
  `Dirty { ids }` able to mix sources and operation results without a union type.
* **A separate registry, not `Document::nodes`.** The document's node table holds authored
  primitives whose slots are addressable `Parameter<f64>`s, and `NodeKind` is matched
  exhaustively across the workspace (expression, dependency, constraints, geometry,
  export). Adding a synthetic `NodeKind::Operation` variant would ripple through all of
  them and would let `SetParameter` and `BeginDrag` address a shape that has no authored
  slots. The registry (`OperationRegistry { nodes: BTreeMap<OperationId, OperationNode>,
  order: Vec<OperationId> }`) keeps operations in their own table while sharing the id
  space; `Document::geometry_ids()` enumerates nodes then **enabled** operations in draw
  order, which is the single list the scene layer iterates.
* **`Parameter<f64>` for every operand.** `Offset::distance`, `Fillet::radius` and
  `MirrorAxis::at` are parameters, not floats, so an operation is animatable and
  steerable by exactly the machinery the primitives use: a `Literal`, a `Variable`, a
  compiled `Expression` (so `$time`-driven offsets work), a motion binding, etc. This is
  RULE 3's "animated like a primitive", spent on the operation's *own* numbers.
* **A `name` and a `style` on the record.** An operation is drawn, so it needs a name for the
  layers panel and a style for the canvas; `OperationNode::default_style()` (≈ `#f28c28`)
  keeps it visually distinct from authored nodes without the UI inventing a colour.
* **Arity is a property of the kind** (`OperationKind::arity()`), used by
  `OperationNode::validate` *before* anything is stored. `Boolean` takes exactly two
  inputs; the modifiers take exactly one. `validate` also rejects an operand listed
  twice — an operation cannot use the same node as its own clip.

The registry also exposes `affected_by(&[NodeId]) -> Vec<OperationId>`: the operations
that read any of the given ids. That is the whole of the dependency story in Phase 1 —
see §5.

---

## 2. Commands and undo (`vectra-core::command`)

```rust
ApplyOperation { id: OperationId, kind: OperationKind, inputs: Vec<NodeId> }
RemoveOperation { id: OperationId }
SetOperationEnabled { id: OperationId, enabled: bool }
```

* **`ApplyOperation` carries its own id** (like `CreateNode`), so undo and redo address
  the *same* virtual node rather than minting a new one on redo — the same rule the
  primitives follow.
* **The command mutates only the registry.** It does not touch, rewrite, or even visit the
  inputs' parameters: that is RULE 1 expressed as a code-level claim, and
  `law_sources_are_never_mutated_by_an_operation` asserts it on the sources' parameters
  directly.
* **Exact inverses.** `ApplyOperation` → `RemoveOperation{id}`; `RemoveOperation` →
  `ApplyOperation` with the removed record's own id/kind/inputs; `SetOperationEnabled` →
  the same command with the previous flag. `RemoveOperation` on an unknown id is
  `VectraError::OperationNotFound` (typed, refused, nothing recorded).
* **`DeleteNode` cascades.** Deleting a source withdraws the operations that read it and
  returns them in the *same* batch, so one undo restores node + operations together.
  An operation's *other* inputs are untouched — the batch contains exactly the orphaned
  operations, nothing more. This is the undo half of "a virtual node is not a destructive
  edit": removing a source cannot leave a dangling operation, and undoing the deletion
  cannot leave the operation behind.
* **Events.** `EngineEvent::OperationsUpdated { ids }` is emitted by all three commands
  (plus `OrderChanged`/`NodesRemoved` where the draw order changed). It is deliberately
  *registry* news; the *geometry* news is the `Dirty` event that follows, carrying the
  operation id — the UI can therefore distinguish "the registry changed" from "the shape
  changed" without inferring anything.

### Why there is no `SetOperationParam`

`OperationKind`'s parameters are `Parameter<f64>`, i.e. fully steerable — but there is no
command that edits them in place. That is a deliberate scope decision, not an omission:

1. **Phase-1 steering already exists through the parameter source.** To animate or bind an
   operation operand you bind the source: `SetParameter{node: source, property, value}`
   for a source's slot, `SetVariable`/`DefineExpression` for a value, and the operation
   picks the change up on the next pass because its input is dirty. The
   `law_the_clock_reaches_operations` protocol law drives a `$time`-bound source and
   asserts the *operation* re-evaluates on `set_time` — the animatable case is covered
   without a new command.
2. **The document has no `OperationNode` mutator to expose.** `OperationRegistry` stores
   `OperationNode`s by value; `RemoveOperation` + `ApplyOperation` (a two-command batch)
   is already an exact, undoable way to change a kind's operand, and it keeps the inverse
   story trivial (both halves are already reversible).
3. **The UI does not need it.** Requirement 5 asks for Union/Subtract between two selected
   nodes; the modifier buttons mint a fresh operation with the entered value.

Adding `SetOperationParam { id, operand, value: Parameter<f64> }` is a *small* change
(one registry accessor, one command arm, one inverse) and is the natural first extension
in Task 5.x if the renderer's time scrubbing wants to retune an offset in place. It is
listed in §11 as an open item rather than pretended to be a design choice.

---

## 3. The `geo` pipeline (RULE 2)

`vectra-operations` is the only crate that knows `geo` exists. Everything above it
(`vectra-geometry`, `vectra-dependency`, `vectra-wasm`, the UI) speaks `lyon` paths and
`Parameter`s.

### 3.1 `convert.rs` — the bridge

* `path_to_multi_polygon(&Path) -> Option<MultiPolygon<f64>>`
  * flattens the path with `PathIterator::flattened(FLATTEN_TOLERANCE)` (0.05 doc units),
  * emits **one single-ring polygon per flattened subpath**,
  * then canonicalises fill parity with `union(&MultiPolygon::new(vec![]))`.

  The last step is the important one and it is not cosmetic. `lyon` paths carry no fill
  rule, and `geo::boolean` defaults to **even-odd** semantics. A path with a nested ring
  (an outer square and an inner square) is *not* two polygons; under even-odd it is a
  square with a hole. Feeding the raw subpath list into a boolean would union the rings as
  solids and lose the hole. Canonicalising through `union` with an empty region makes the
  parity the *region's* property (a `Polygon` exterior + `interiors`), so the hole
  survives every subsequent operation. `convert.rs`'s test asserts exactly this case:
  100 − 16 = 84.0 both at the region level (1e-9) and after the round trip back to a path
  (1e-6).
* `multi_polygon_to_path(&MultiPolygon) -> Path` writes exteriors **CCW** and interiors
  **CW**, i.e. an orientation convention of its own, so the result is a well-formed
  lyon path that renders identically for any nonzero-or-even-odd consumer. It is what
  `path_to_svg_data` renders, which is why an operation result draws on the SVG canvas
  with no renderer changes.
* `arc_samples()` = **64 segments per full turn** for circular arcs introduced by offsets
  and fillets — the same resolution class as `vectra-geometry`'s `ARC_SEGMENTS_PER_TAU`,
  and deliberately independent of radius or sweep so the same input always yields the same
  polygon (determinism the law suite depends on).
* `region_area` / `path_area`: signed-area helpers used by the ops and by the tests.

### 3.2 `ops.rs` — region → region

| op | implementation | note |
| --- | --- | --- |
| boolean | `geo::BooleanOps::{union, difference, intersection, xor}` | no hand-written clipping geometry anywhere |
| offset | `geo::algorithm::buffer::Buffer::buffer(d)` | round joins; negative `d` insets |
| mirror | `AffineTransform::scale(±1, 1)` + `AffineOps::affine_transform` + `reorient` | area preserved, winding normalised |
| fillet | own corner rounding over the region's rings | the one place with arithmetic this crate owns |

**The fillet rule (worth recording, because the first implementation was wrong).** Rounding
a convex corner replaces the wedge beyond the tangent points with an arc whose centre lies
on the *material* bisector. Two facts decide the geometry, and neither may be inferred
from a cross product:

1. **Which way is material?** Determined by ring orientation: `signed_area > 0` ⇒ CCW ⇒
   material to the left. The material wedge angle is the ring's interior angle `θ` for CCW
   and `2π − θ` for CW (holes, mirrored shapes). The single failing implementation used
   `if cross < 0.0 { sweep -= TAU }`, which is inverted for clockwise rings and produced
   46.54 instead of 96.57 for a side-10 square at `r = 2`.
2. **Which arc?** The **short** one: tangent length `t = r / tan(θ/2)`, centre at
   `r / sin(θ/2)` along the material bisector, sweep `= wrap_to_pi(end − start)` with
   magnitude `π − θ`. The short-arc form is valid for convex material corners only
   (θ < π); reflex corners stay sharp via the `θ ≥ π − 1e-9` early return.
3. **Oversized radii clamp, they do not error.** `t` is capped at
   `0.5 · min(edge_a, edge_b)`, which shrinks the effective radius. `r = 0` is an exact
   no-op (area 100.0), `r = 50` on a side-10 square returns 78.088904 rather than an
   error, and a non-finite radius is a typed `Err`. The clamp is a design choice: a
   fillet that cannot fit its edge is a user intention to round as much as possible, not a
   malformed document.

Measured behaviour: side-10 square filleted at `r = 2` → **96.494225** sampled vs
**96.566370** analytic (−0.075 %, the 64-segment arc discretisation); offset `+2` on
10×10 → ≈ 192.57 within 2 % (a 14×14 box with `r = 2` corners, i.e. `196 − 4·(4 − π)`);
mirror preserves area exactly and flips coordinates.

### 3.3 `evaluator.rs` — the engine entry point

* `evaluate_all(doc, scene, ctx) -> OperationEvaluation` — every enabled operation, in
  registry order (so an operation reading a settled result sees it; one-level nesting
  today).
* `evaluate_one(doc, primitives, ctx, id) -> Result<EvaluatedNode, OperationError>` — fetch inputs
  from the evaluated scene (a *virtual* node reads the *scene*, not the document: the
  source may itself be an operation result), `primitive_to_path` each input, convert,
  apply the kind, convert back.
* **A failure is a diagnostic, never a broken pass.** One failing operation produces one
  `Diagnostic` (`OperationFailed` / `OperationEmptyInput`) + no geometry for that node,
  and the rest of the pass continues. The scene stays consistent: the engine never
  half-writes.
* `compose_into(&mut EvaluatedScene, doc)` injects the results as ordinary nodes
  (RULE 3): same `EvaluatedNode` shape, same style path, same z-order mechanism, and it
  prunes any cached virtual id that is no longer live.

---

## 4. Scene injection and the liveness predicate

`EvaluatedScene::apply_operations(doc, updates)` + `compose_into` are the only writers of
virtual geometry. The subtle part is **what "live" means**, and the law suite found it:
`Document::is_geometry_id(id)` must answer "does this id contribute geometry *right now*",
and for an operation that is `enabled`. The first version answered "is it in a registry",
which is true for a *parked* operation — so the scene kept a node for it and the scene
cache's live-id assertion (`incremental.rs`) tripped on a `SetOperationEnabled{false}`
whose dirty set contained no primitives at all. There is now exactly one predicate:

```rust
pub fn is_geometry_id(&self, id: NodeId) -> bool {
    self.nodes.contains_key(&id) || self.operations.get(id).is_some_and(|op| op.enabled)
}
```

`geometry_ids()`, `apply_operations`' pruning and the engine's `prune_disabled_operations`
all read it, so "the registry says enabled", "the scene holds a node" and "the z-order
lists it" cannot disagree. A parked operation keeps its id, its inputs and its row; it
loses only its geometry.

---

## 5. Dirty propagation

```
source node dirty ──▶ operations affected_by([source]) ──▶ re-run ──▶ scene injection
```

* **The registry fan-out is a scan, not a graph traversal.** `affected_by` walks
  `registry.order` and tests `op.inputs` for membership. There is no
  `vectra-dependency` vertex for an operation and no new edge kind in `StableGraph`.
  Justification: Phase 1 operations read only *authored* nodes, one level deep, so the
  dependency is already fully described by the inputs stored on the record. O(#ops) per
  mutation with a handful of operations is cheaper and simpler than a graph visit, and
  the `BTreeMap`/`order` pair makes it deterministic. The moment operations can read
  operations (or motion/procedural nodes can feed them), the scan must be replaced by
  real graph vertices — that replacement is the Phase-2 item in §11, and the code is
  written to make it a local change (`affected_by` is the single call site).
* **The ops pass runs after the primitive pass.** In `VectraEngine::settle` the order is
  fixed: `prune_disabled_operations` (retire geometry of removed/parked operations) →
  `patch(dirty)` (the primitive pass) → `run_operations` (the virtual pass) → merge the
  recomputed operation ids into the `Dirty` event's id list. Running the ops pass first
  would evaluate yesterday's source geometry; running the prune *inside* `run_operations`
  (as the first draft did) left stale virtual nodes in the cache for one edit, because a
  command that only parks an operation may dirty no primitive at all.
* **Pending vs affected.** `pending_operations` collects operation ids mentioned by the
  commands in the current batch (`operation_ids_of`), so a registry change runs even when
  no source is dirty; `affected_by(&report.dirty)` adds the dependents of whatever the
  primitive pass recomputed. A `Full` mode report re-runs everything.
* **The clock is a source.** `set_time` used to patch primitives only, leaving an
  operation stale if a time-bound source moved under it. It now performs the same
  retire → patch → operations sequence and reports the recomputed operation ids as dirty;
  `law_the_clock_reaches_operations` asserts both the dirty id and the changed geometry.
* **Undo/redo re-run the virtual layer too.** The operation ids of the commands being
  reverted/replayed are collected from `peek_undo`/`peek_redo` before applying, so undo
  cannot leave a result drawn for a state that no longer exists.

---

## 6. The `f32` representation contract (what the law tests assert, and why)

An operation's result is a **`lyon` path**: `f32` coordinates. Source primitives are
authored in `f64`. Every law that compares an area computed from the document against an
area computed from the pipeline therefore has three error terms, and conflating them is
what made the first drafts of both suite out of tolerance with no conceptual bug at all:

1. **Lattice rounding (~1e-8 … 1e-12).** A coordinate that does not land exactly on the
   `f32` output lattice comes back slightly off. This is why the boolean *identity* tests
   are stated at 1e-9 against the pipeline's own regions, not against analytic numbers.
2. **Subtracted-rounded-areas (~1e-6).** `area(A \ B)` and `area(A) − area(A ∩ B)` round
   *two* separately measured areas before subtracting; on a small shape the relative error
   is dominated by this term, not by the boolean. The identity is asserted at
   `1e-6 · max(|expected|, 1)` **with the extra rounding called out in a comment**, rather
   than weakened to a sloppy absolute epsilon.
3. **Inexact numeric assertions are still assertions.** The proptests use the same
   pattern: build the expectation from the pipeline's own regions (`path_to_multi_polygon(
   primitive_to_path(node))` re-encoded through the same `f32` path via
   `representable_area`), so a real boolean bug cannot hide inside tolerance.

The rule that came out of this, and applies to future work: **never compare pipeline
output against independently constructed `f64` geometry.** Compare it against the same
geometry pushed through the same representation.

---

## 7. Tests

| suite | count | what it pins |
| --- | --- | --- |
| `crates/vectra-operations/src/{convert,ops}.rs` (unit) | 8 | conversion identities, parity/holes, orientation, each kind's arithmetic and clamps |
| `crates/vectra-operations/tests/operation_laws.rs` | 12 | Boolean Satisfaction, Non-Destructive Source, Undo, modifier scope, parked/failed operations, delete-cascade, refusal |
| `crates/vectra-wasm/tests/operation_laws.rs` | 10 | the same contract on the wire: shape, `Dirty` naming, undo via `revert`, typed refusal, full-rebuild equivalence, the clock |
| `apps/vectra-web/scripts/smoke.mjs` | 24 steps | the MES §16 loop end to end, incl. steps 22–24 for operations |
| `apps/vectra-web/tests/view-model.test.ts` | 23 | the panel's projections: operand captions, the nesting rule, log lines |

The three laws the task named explicitly:

* **Boolean Satisfaction Law** — `Subtract(A, B)` area `== Area(A) − Area(A ∩ B)`; asserted
  per-fixture and as a 64-case proptest, at the tolerances of §6.
* **Non-Destructive Source Law** — moving B leaves A's path byte-identical while the
  operation's path updates (engine law, protocol law, and smoke step 23 on the real wasm
  module).
* **Undo Law** — one undo removes the `OperationNode` and leaves the sources exactly as
  authored; a proptest asserts the undo is an exact pre-image.

---

## 8. UI (requirement 5)

The React app stays a dumb remote (Task 1.4 boundary):

* **Operations section** — Union / Subtract / Intersect / Exclude are enabled by a
  two-source selection (`selected[0]` is the left operand: *Subtract removes
  `selected[1]` from `selected[0]`*), and Offset / Fillet / Mirror by a one-source
  selection plus the value field. Every button sends one `CommandWire` and re-reads the
  snapshot. No geometry, no parameter resolution, no state duplication.
* **Operand eligibility is a UI gate, not a rule.** Operation rows in the Layers panel are
  not selectable (Phase 1 has no operation-on-operation nesting), so the panel never sends
  a command the engine would refuse. The engine still validates: a stray operand is a
  typed error in the log (smoke step 24).
* **The Layers panel nests the result under its source.** `layerRows()` emits authored
  sources in the panel's order and indents each operation directly beneath the source it
  reads *first* (`inputs[0]`). The nesting is presentation of an engine fact — the inputs
  are the engine's — and an operation whose `inputs[0]` is missing is listed at the end
  rather than invented under a neighbour. Operation rows are drawn with an amber kind tag,
  the operand summary (`plate − hole`), the engine's `description`, a park/enable
  checkbox and a remove button.
* **The result is drawn by the existing canvas.** An operation is a `path` primitive in
  the snapshot, so `PreviewNode` renders it with no special case — RULE 3 paying off.

---

## 9. Deliberate deviations from the brief

1. **Only a registry-level `OperationNode`; no `NodeKind::Operation`.** Argued in §1: an
   exhaustive-match blast radius against no behavioural gain, since the id space is
   already shared.
2. **`affected_by` is a scan, not a graph edge.** Argued in §5; revisit with nesting.
3. **No `SetOperationParam`.** Argued in §2.
4. **Fillet is home-grown, not a `geo` call.** RULE 2 forbids hand-written *boolean* math;
   `geo` has no fillet primitive, and the region-level corner rounding here is
   arithmetic on circles and tangents that is fully covered by tests. Offsetting — which
   *does* have a `geo` implementation — uses it.
5. **Even-odd is the only fill rule modelled.** `geo::boolean_op_with_fill_rule` exists,
   but the document has no fill-rule concept, so the pipeline canonicalises parity once
   (§3.1) instead of exposing two semantics through a field the UI cannot set.
6. **`prune_disabled_operations` runs before the primitive pass.** Argued in §5.
7. **MES §2's dependency table is superseded for this crate.** It lists
   `vectra-operations` as `lyon`; RULE 2 adds `geo` + `geo-types` (workspace deps) to it,
   and `vectra-wasm` now depends on `vectra-operations` so the engine can inject the
   virtual layer. `lyon` still owns everything the pipeline *renders from*. The boundary
   rule from Task 1.4 stands: no `vectra-render`, no `wgpu` in `vectra-wasm`.

## 10. Gate status

`cargo fmt --check` clean · `cargo clippy --workspace --all-targets -- -D warnings` clean ·
`cargo test --workspace` **32 suites, 211 passed, 0 failed** · `npm run typecheck` clean ·
`npm run test:ui` **23/23** · `npm run build` OK · `node scripts/smoke.mjs` **24/24**.

## 11. Open items

* **Operation-on-operation nesting.** `affected_by` and `evaluate_all` both assume one
  level. Nesting needs (a) topological ordering of the registry, (b) graph vertices or a
  transitive closure instead of the scan, (c) a cycle check reusing
  `VectraError::CyclicDependency`.
* **`SetOperationParam`** (see §2) — the natural in-place operand editor.
* **Fill rule as document state** (`nonzero` vs `even-odd`) if a source path ever needs
  self-intersecting nonzero semantics.
* **Performance**: `affected_by` is O(#ops) per mutation, and `evaluate_all` re-encodes
  every input on every pass. Fine at Phase-1 scale; measure before caching.
