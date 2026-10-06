# Task 7.0 — The Procedural Graph: reconnaissance (pre-authorization)

**Status: nothing implemented.** This document exists so the authorization prompt can be written
against the code as it *is*, not against a guess about it. It is the inventory of what is already
soldered, the forks the prompt must settle, the laws I would hold the work to, and the plan I
recommend.

Companion documents: `TASK-6.0-REPORT.md` (the motion engine this follows), `TASK-4.0-DESIGN.md`
(the non-destructive-operation pattern the graph should mirror), `MES.md` §11 (Procedural Graph
API).

---

## 0. The verdict, and why

`vectra-motion` in Task 6.0 was a *plug*: the socket existed and the crate was a stub. Task 7.0 is
the same shape of job, one size larger — and unlike Task 6.0, the socket is only **half** built.

What exists:

| piece | state |
| --- | --- |
| `Parameter::Procedural(NodeOutputId)` | ✅ in core since Task 1.2 (`param.rs:225`) |
| `NodeOutputId { node: NodeId, port: PortId }` | ✅ `param.rs:30`; `PortId = String` (`ids.rs:40`) |
| `ProceduralEvaluator` trait | ✅ two methods, `evaluate_float` / `evaluate_point` (`eval.rs:62–72`) |
| `EvaluationContext.procedural` + `with_procedural` | ✅ `eval.rs:100`, `eval.rs:126` |
| typed failure when nothing is wired | ✅ `ResolveError::MissingProceduralEvaluator` (`error.rs:33`) |
| `vectra-procedural` crate | ✅ member, manifest already declares `vectra-geometry` + `petgraph` — and **nothing in the workspace depends on it** (`Cargo.toml:74` is a declaration, not an edge) |
| a registry in `Document` | ❌ no `procedural` field (`document.rs:658–677`) |
| commands, undo, graph vertices | ❌ none |
| wasm surface, snapshot, UI | ❌ none (`grep procedural crates/vectra-wasm` → zero hits) |

And the two decisive facts that make this the right next task:

1. **Nothing can regress.** Every `Parameter::Procedural` in existence today resolves to the same
   typed error. There is no live behaviour to protect — the work is additive by construction.
2. **The MES puts it in Phase 2, and it is the last Phase-2 model change.** Export (Task 8.0) is
   downstream of it: an export IR written before the node graph exists would have to be rewritten
   after. This is the same argument the Task 6.0 recon made about motion, and it is unchanged.

**Why not 8.0 first (honest case).** Export is cheap and ships a tangible artifact, and it would
touch no engine semantics. But the RECON for Task 6.0 already recorded the reason to wait — the
document model is still growing — and Task 7.0 is precisely that growth. After it, the model is
stable and export is a *read* of a finished thing. Doing 8.0 now means writing the IR twice.

**Why not 7.0 in pieces (scope).** The forks below are not independent: F1 (what a port carries)
determines whether the graph draws at all, and F2 (value-only vs. geometry) decides whether this
is a value library or a geometry engine. Piecemeal authorization would produce a value-only graph
that has to be re-opened for the geometry half — the thing the RECON process exists to prevent.

---

## 1. What is already soldered, precisely

### 1.1 Core: the address without the thing addressed

```rust
// crates/vectra-core/src/param.rs:225
Parameter::Procedural(NodeOutputId)

// crates/vectra-core/src/param.rs:30
pub struct NodeOutputId { pub node: NodeId, pub port: PortId }
```

An address of `(node, port)` in the **same id space as primitives** — which is the Task 4.0 trick
(RULE 3: a virtual result is keyed, ordered and styled like an authored node). A procedural node's
id can therefore become a scene node with no new identity machinery.

### 1.2 Core: the trait, and the hole in it

```rust
// crates/vectra-core/src/eval.rs:62
pub trait ProceduralEvaluator: Send + Sync {
    fn evaluate_float(&self, output: &NodeOutputId, ctx: &EvaluationContext) -> Result<f64, ResolveError>;
    fn evaluate_point(&self, output: &NodeOutputId, ctx: &EvaluationContext) -> Result<Point2, ResolveError>;
}
```

**The signature is the fork nobody has had to face yet: the evaluator sees an
`EvaluationContext`, not a `Document`.** `EvaluationContext` carries variables, time, and the four
other evaluator hooks (`eval.rs:94–102`) — but no registry and no scene. So a procedural node
cannot look up its inputs from inside resolution. Task 4.0 never hit this because
`OperationsEvaluator` is a *pass* (`evaluate(doc, primitives, ctx, ids)`, `evaluator.rs:90`), called
by the engine with everything in hand; it is not a trait impl.

Whatever shape Task 7.0 takes, something must bridge that gap (fork F4).

Two more honest notes on the trait:

* `Resolvable<Point2>`'s comment (`eval.rs:191–194`) already tells the intended story: *"a point
  must be bound component-wise at the property level or produced by the procedural graph."*
* `Resolvable<Color>` (`eval.rs:215–225`) returns `MissingProceduralEvaluator` **unconditionally**,
  under a comment that says *"typed color ports arrive with the procedural graph."* The colour half
  of the trait is a placeholder waiting for this task.

### 1.3 Dependency: the arm that does nothing

```rust
// crates/vectra-dependency/src/graph.rs:905
Parameter::Procedural(_) | Parameter::Interaction(_) => {}   // no edge, silently
```

A slot fed by a procedural port is currently dirtied by **nothing**. Today that is harmless —
resolution fails loudly — but it is the *same class* of bug as the Task 6.0 stale-target bug
(`dependency_target`, singular): a silent stale reader. It must be closed **in the same commit that
makes procedural resolution succeed**, or the first working procedural value ships with a silent
staleness bug. `GraphNode` (`graph.rs:52`) has five kinds, none procedural; `wire.rs:18` mirrors it.

### 1.4 Geometry: the RULE 3 composition path already exists

`EvaluatedScene::apply_operations(doc, updates)` (`scene.rs:169`) inserts virtual nodes and then
prunes anything failing `doc.is_geometry_id(id)` (`scene.rs:179`), restacking `z_order` from
`doc.geometry_ids()`. **This is the one trap in the whole task**: if a procedural result is drawn
but `is_geometry_id`/`geometry_ids` (`document.rs:697`, `document.rs:715`) is not widened, the next
operations pass silently deletes the procedural geometry from the cache. It is a two-line change
with a scene-destroying failure mode, and it gets a test in the same commit.

### 1.5 The pattern precedent: `vectra-operations` (Task 4.0)

Every seam the procedural graph needs has a working example one crate over:

| seam | operations | procedural (planned) |
| --- | --- | --- |
| registry in core | `OperationRegistry` w/ `order` + `affected_by` (`operation.rs:228`) | `ProceduralRegistry` |
| virtual node → scene | `OperationEvaluation::compose_into` (`evaluator.rs:60`) | same shape |
| incremental re-run | `affected_by(dirty)`, one-level scan (`evaluator.rs`/`lib.rs:1635`) | **not sufficient** — chains nest (§4, F3) |
| failure is local | `Diagnostic` per node, no abort (`evaluator.rs:213`) | same, new codes |
| commands + undo | `ApplyOperation` / `RemoveOperation` / `SetOperationEnabled` (`command.rs:113–129`) | three to five commands |
| wasm plumbing | `pending_operations`, `run_operations`, `prune_disabled_operations` (`lib.rs:131`, `1635`, `1592`) | mirrored |
| snapshot section | `SnapshotOperation` map (`snapshot.rs:244`) | mirrored |
| UI panel | `operations` testids, `op-*` buttons (`App.tsx:1259–1360`) | mirrored |

Where the procedural graph is *harder* than operations, and why this is a real task:

* operations read whole shapes and **never chain**, so there is no cycle to reject and a one-level
  `affected_by` scan is enough;
* a procedural graph **is a graph**: nodes wire to nodes, so cycles are constructible (the Task 2.2
  doctrine demands pre-application rejection with a typed `CyclicDependency`), dirty must propagate
  *along* the chain in topological order, and every port needs a type.

### 1.6 The stub itself

`crates/vectra-procedural/src/lib.rs` is 17 lines: an error enum with one `Other(String)` variant
and `placeholder() -> true`. Its manifest (`Cargo.toml:11–14`) already lists `vectra-geometry` and
`petgraph` — the crate was scaffolded with the intent to own its own graph. Nothing depends on it.

---

## 2. What the MES actually promises (§11)

```rust
pub trait GeometryNode {
    fn inputs(&self) -> Vec<Port>;
    fn outputs(&self) -> Vec<Port>;
    fn evaluate(&self, inputs: HashMap<PortId, GeometryData>) -> GeometryData;
}
// "Implements vectra_core::ProceduralEvaluator. Nodes: GridGenerator,
//  NoiseModifier, PathSmooth, …"
```

Three things follow, and none of them is negotiable if the task is to implement §11 rather than
replace it:

1. **`GeometryData` is a core type that does not exist yet.** The trait takes and returns it, so it
   has to live where both the trait and `Parameter` can name it — core. Core has no `lyon` and must
   not gain one (§Appendix A.1: core is the dependency root).
2. **The named nodes produce *geometry*.** `GridGenerator` generates shapes, `NoiseModifier` and
   `PathSmooth` modify them. A value-only reading of §11 would not implement the three examples the
   spec itself gives.
3. **`ProceduralEvaluator` is the value door.** `Parameter::Procedural` resolves *through* the trait
   to `f64` / `Point2` (and, per §1.2, the colour placeholder). So the graph must also produce
   values that parameter slots can read — a generator's cell count, a noise sample, a smoothed
   curve's length.

Both halves, then. The design question is only how they meet (F2).

---

## 3. The forks

### F1 — What does a port carry? (`GeometryData`, owned by core)

| option | verdict |
| --- | --- |
| **(a) core enum, dependency-free, serializable** — `Scalar(f64)`, `Point(Point2)`, `Points(Vec<Point2>)`, `Path { points, closed }`, `Region { rings }`, `Color(Color)` | **recommended** |
| (b) `lyon::path::Path` as the unit | rejected: puts a rendering crate in the dependency root, and a `Path` cannot express a scalar or a colour |
| (c) opaque handle / trait object | rejected: unserializable (the document must round-trip), and it makes the type system do nothing |

Recommendation: **(a)**, with the explicit rule that **a port has exactly one type** — the type is
the `GeometryData` variant, declared by the node kind's `outputs()`, and a wire that does not match
is a *typed* rejection (`PortTypeMismatch`), never a coercion and never a zero. MES §11 says "typed
node graph" and gives an untyped `HashMap`; port typing is the deliberate extension that makes the
sentence true, of the same character as Task 4.0's `corner_radius` extension.

`Region { rings }` is point lists, not a geometry library type: region → `geo::MultiPolygon` →
`lyon::path::Path` conversion already exists and is tested (`vectra-operations/src/convert.rs`,
`FLATTEN_TOLERANCE = 0.05` at `convert.rs:38`), and reusing it keeps one tolerance in the workspace.

### F2 — Does the graph draw, or only feed numbers?

| option | consequence |
| --- | --- |
| (a) value-only | plugs straight into `ProceduralEvaluator`; §11's three named nodes are impossible; nothing to look at |
| (b) geometry-only | mirrors Task 4.0 exactly; `Parameter::Procedural` stays a permanent error; the "typed graph" is half a graph |
| **(c) both, separated by port type** | **recommended** |

Under (c):

```text
procedural node ──┬── scalar/point/color ports ──▶ published table ──▶ Parameter::Procedural
                  └── path/region ports ────────▶ virtual scene node  ──▶ renderer, hit-test, layers
```

* a slot may only reference a **value** port; a region port referenced from a scalar slot is a typed
  error naming the port (`ResolveError::ProceduralPortType`, new);
* a **region/path** output becomes an `EvaluatedNode` with the node's own `NodeId` — RULE 3, so it
  is drawn, hit-tested, z-ordered and styled like anything else, and needs no new scene machinery;
* **colour**: with typed ports available, `Parameter<Color>` finally has a real source. This retires
  the placeholder at `eval.rs:215–225` and the odd asymmetry in `UnsupportedColorSource`
  (`error.rs:39` says colour supports "Literal and Procedural", and then the resolver refuses
  procedural). Cost: one added trait method, `evaluate_color`, on a trait with one implementor.

### F3 — Where does the graph's own DAG live, and who rejects cycles?

The node *records and wiring* are authored data: they belong in the document, beside `constraints`,
`operations` and `motion` (`document.rs:667–676`), serialize with it, and are created by undoable
commands. The *topology* (topological order, cycle gate, dirty walk) is derived, exactly like
`vectra-dependency::graph::derive(&doc)` — so it is computed, never stored:

* `ProceduralRegistry { nodes: BTreeMap<NodeId, ProceduralNode>, order: Vec<NodeId> }` in core;
* `ProceduralNode { id, name, kind, wires: BTreeMap<PortId, NodeInput>, enabled, style }` where a
  `NodeInput` is either a `Parameter<f64>` operand *or* a wire from another node's output port;
* the chain DAG is built by `vectra-procedural` (petgraph, as its manifest already declares), in
  topological order, recomputed on demand — not stored, so undo needs no special case (the Task 2.2
  Rule 1 discipline);
* **cycle rejection happens before the document changes**, through the existing gate:
  `prospective_edges` (`prospective.rs:87`) gains an arm for the wire commands, and the candidate
  wiring is dry-run through a pure `would_cycle(&registry, new_wire)` in `vectra-procedural`,
  surfacing the *same* `VectraError::CyclicDependency` the expression and motion graphs use. One
  error type, two graphs.

**And a second, deliberate prohibition** — the composite-cycle rule: a procedural node's
`Parameter<f64>` operand may read variables, expressions or motion, but **not** another node's
procedural output. Wires express dependency inside the graph; a value reference back into it is a
cycle wearing a disguise (geometry ← parameter ← procedural value ← geometry source), and it is
rejected at the command boundary with a typed message. That keeps evaluation order a straight
topological sweep and keeps the value table a pure function.

### F4 — Who evaluates, and how does the evaluator reach the registry? (the F1 hole of §1.2)

| option | verdict |
| --- | --- |
| (a) put a `Document` reference in `EvaluationContext` | rejected — it is the *core* context; threading the document into every resolution inverts the dependency root and re-couples parameters to the document |
| (b) a stateful `ProceduralEngine` that syncs from the document, reads the registry out of its own copy, and **publishes** the value ports' values for resolution to read | **recommended** |

Recommendation: **(b)**, and it is not a compromise — it is the *only* shape that makes the value
path and the geometry path one evaluation instead of two:

```text
engine mutation
   │
   ├─ primitive pass (existing)                document ─▶ EvaluatedScene
   ├─ operations pass (existing, Task 4.0)     + virtual boolean/offset/fillet/mirror nodes
   └─ procedural pass (new, runs last)
         ├─ sync_from_document(&doc)           ← the MotionEngine precedent (motion/lib.rs:127)
         ├─ dirty seeds → topo walk            ← Incrementality Law
         ├─ evaluate each dirty node           ← GeometryData in, GeometryData out
         ├─ publish value ports                ← the table `ProceduralEvaluator` reads
         └─ compose region ports into the scene (RULE 3, apply_operations)
```

* `impl ProceduralEvaluator for ProceduralEngine` becomes a **lookup**, not an evaluation: no
  document, no scene, no ordering hazard — it answers from the table the pass filled. Resolution
  stays pure, which is what the Motion Purity Law bought last task and what makes this testable.
* The pass runs **last** (after operations), so a `Source` node may read a primitive *or* a boolean
  result — a typed diagnostic if the id is not live geometry.
* Consequence to accept explicitly: a procedural value read by a slot is only correct **after** a
  pass. A stale table is exactly the failure mode to test against (a slot's value must be unchanged
  when the pass is a no-op, and must change only after the dirty node re-runs).

### F5 — Which node kinds ship?

The MES names three; a working graph needs a way in and a way out:

| kind | direction | ports | why it is in the first set |
| --- | --- | --- | --- |
| `Source { node }` | adapter | region out | lets the graph consume authored geometry — the RULE 1 reading (sources are read, never written) |
| `Grid { columns, rows, spacing }` | generator | region **and** points out | §11's `GridGenerator`; and the two port types in one node exercise F2 both ways |
| `Repeat { count, offset }` | generator | region out | instancing is the cheapest genuinely useful generator, and it is a *chain* consumer (`Source` → `Repeat`) |
| `Noise { amplitude, frequency, seed }` | modifier | region out, scalar out | §11's `NoiseModifier`; value noise over a seeded integer hash — deterministic, no `rand`, no new dependency |
| `Smooth { iterations, strength }` | modifier | region out | §11's `PathSmooth`; Chaikin-style corner cutting, deterministic |

Out of scope, explicitly: arbitrary/user-defined nodes, per-item parameter arrays, curve-handle
editing, GPU compute, mesh-level operations, and a node-graph *canvas* in the UI (the panel wires by
dropdown; the UI stays a dumb remote per Task 1.4).

---

## 4. The laws I would hold the work to

| law | statement |
| --- | --- |
| **Port Type Law** | a port carries exactly one `GeometryData` type; a mismatched wire is rejected before it is stored, and a slot that references a non-value port fails with a diagnostic naming the port — never a coercion, never a zero |
| **Acyclicity Law** | any wiring that would close a cycle (including a value reference back into the graph, F3) is rejected **before** the document, history and scene change; typed `CyclicDependency`; document byte-identical afterwards |
| **Determinism Law** | same document + registry + `ctx` ⇒ byte-identical `GeometryData` and byte-identical scene, regardless of dirty history, evaluation order, or undo/redo; noise is seeded integer hashing (no wall clock, no `rand`, no float-order dependence) |
| **Non-Destruction Law** | a procedural pass mutates **no** document slot and adds **zero** history entries: sources stay editable, draggable and constrained, and a full snapshot after evaluation equals one taken before it |
| **Incrementality Law** | editing one node's operand re-runs exactly that node plus its transitive consumers, **in topological order**, and nothing else — witness lists, in the Task 2.2 style; a no-op mutation re-runs nothing |
| **Containment Law** | a node that fails (unresolvable operand, missing source, empty input, degenerate result) contributes no geometry, emits exactly one diagnostic naming it, and leaves every sibling's output byte-identical |
| **Live-Id Law** | a procedural result is a scene node exactly while its node is enabled: parked ⇒ geometry retired, enabled ⇒ recomposed, deleted ⇒ gone; the operation pass that runs before it can never evict it (the §1.4 trap) |

Gates, unchanged: `cargo test --workspace` green, `clippy -D warnings`, `fmt`, typecheck, UI tests,
`npm run build`, smoke **past 31 steps**, plus `TASK-7.0-DESIGN.md` and `TASK-7.0-REPORT.md`.

---

## 5. Scope ladder

**In:** the `GeometryData` type and port typing in core; the registry + 4–5 commands (add, remove,
set-enabled, connect/disconnect a wire, set an operand) with exact inverses; the chain DAG
(topological order, cycle gate, dirty walk); the five node kinds above; the pass with per-node
diagnostics; `impl ProceduralEvaluator` (float, point, **and** colour); region results as virtual
scene nodes; the live-id widening; dependency vertices for value references; the wasm surface
(commands, `procedural_json`, snapshot section, pass wiring after operations); the UI panel (add /
wire / enable / remove / list / diagnostics); the seven laws as tests; smoke steps 32+; both docs.

**Out:** a node-graph canvas; user-authored nodes; arrays/instancing parameters per item;
curve-handle editing (`Path` segments stay as authored); export (Task 8.0, now unblocked);
procedural *colour ramps* beyond a single typed colour port; any GPU-side evaluation.

---

## 6. The touch list

| file | change |
| --- | --- |
| `crates/vectra-core/src/procedural.rs` | **new**: `GeometryData`, `PortType`, `NodeInput`, `ProceduralNode`, `ProceduralRegistry`, validation, `affected_by`-style helpers |
| `crates/vectra-core/src/{param,document,command,engine,error,lib}.rs` | `Parameter::procedural()` helper, `Document.procedural` (`#[serde(default)]`), 4–5 commands + inverses + `validate`, `new_procedural_id`-free (NodeId space), new `ResolveError` variants, `is_geometry_id`/`geometry_ids` widening |
| `crates/vectra-core/src/eval.rs` | `ProceduralEvaluator::evaluate_color`; colour resolution finally honours `Parameter::Procedural` |
| `crates/vectra-procedural/src/*` | **real crate**: `data.rs` (types + wire format), `nodes.rs` (the five kinds), `graph.rs` (topo order, cycle gate, dirty walk), `engine.rs` (`ProceduralEngine`, publish table, `impl ProceduralEvaluator`), `pass.rs` (`evaluate`, diagnostics, `compose_into`), `error.rs` |
| `crates/vectra-dependency/src/{graph,wire,prospective}.rs` | `GraphNode::Procedural { node, port }` + label/key/rank; `collect_targets` procedural arm (**replaces the silent `{}`**); prospective arms for the write commands; the E0004 cascade (graph.rs `label`/`key`, wire.rs view/`From`/`kind_rank`, `prospective.rs`) |
| `crates/vectra-wasm/src/lib.rs`, `snapshot.rs` | `procedural` field + `sync_from_document` + `run_procedural` after `run_operations`; `with_procedural` on every context (4 sites today); `procedural_json`; snapshot section; dirty plumbing |
| `apps/vectra-web/src/engine/{wire,client,commands,view-model}.ts` | wire types + graph kind, client methods, command builders, panel rows |
| `apps/vectra-web/src/App.tsx`, `App.css` | the Procedural panel; layer rows come free via the scene |
| tests | `crates/vectra-procedural/tests/procedural_laws.rs` (7), `crates/vectra-dependency/tests/procedural_graph_laws.rs`, `crates/vectra-wasm/tests/procedural_laws.rs`, UI view-model tests, smoke 32+ |

---

## 7. Risks, ranked

1. **The silent eviction (§1.4).** A drawn procedural node whose id is not in `geometry_ids()` is
   deleted from the scene cache by the *next operations pass* — no error, no diagnostic, geometry
   just disappears later. Mitigation: widen `is_geometry_id`/`geometry_ids` in the same commit as
   the first drawn result, and one law test that runs a procedural result **and** an unrelated
   operation afterward.
2. **The stale published table (F4).** A value read by a slot is served from a table filled by the
   pass. If the pass is skipped when it should run, resolution returns the *old* number silently —
   the exact failure the Incrementality Law's witness lists exist to catch. Mitigation: the table is
   written only by the pass, and the law asserts "no-op mutation ⇒ identical published values"
   alongside "dirty node ⇒ new values".
3. **Cycle semantics across two graphs (F3).** The document graph and the chain DAG must not
   disagree about what a cycle is; the composite rule (no value references back into the graph) is
   what keeps them one graph. Mitigation: the rejection is typed, pre-application, and tested from
   both directions (a wire that closes a chain cycle; an operand that references a value port).
4. **Determinism of noise.** Float arithmetic order and hashing must be platform-stable, or the
   Determinism Law cannot hold across wasm and native. Mitigation: integer hashing (splitmix-style)
   → one `f64` in `[0,1)`, no `rand`, no `sin`/`cos` of large arguments, and a test that the same
   seed produces the same scene in native and wasm builds.
5. **Scope creep through the UI.** A node graph wants a graph editor. Mitigation: the panel is a
   dropdown-wired list, and the DESIGN document records the canvas as an explicit Phase-3 item.
6. **The E0004 cascade** (known, documented in the Task 6.0 memory): a new `Command`/`GraphNode`
   variant touches `graph.rs::{label,key}`, `wire.rs::{view,From,kind_rank}`, `prospective.rs`, and
   the test-only `key_of`. Budgeted, not a surprise.

---

## 8. The plan (if this is authorized)

| step | deliverable | gate |
| --- | --- | --- |
| **1** | core: `GeometryData` + port types, `ProceduralRegistry`, commands + inverses + validation, `Document.procedural`, live-id widening | `cargo test -p vectra-core` + fmt/clippy |
| **2** | `vectra-procedural`: types/wire, five node kinds, chain DAG with cycle gate + topo dirty walk, the pass, `impl ProceduralEvaluator` (float/point/colour) | `crates/vectra-procedural/tests/procedural_laws.rs` — the seven laws |
| **3** | dependency: `GraphNode::Procedural{node,port}`, the procedural arm in `collect_targets`, prospective arms for the write commands, wire/rank | `tests/procedural_graph_laws.rs` — including the "value reference back into the graph" rejection |
| **4** | wasm: field + `sync_from_document`, `run_procedural` after `run_operations`, publish, `with_procedural` everywhere, `procedural_json`, snapshot section | `crates/vectra-wasm/tests/procedural_laws.rs` |
| **5** | UI: panel + client/wire/view-model + tests; smoke steps 32+; rebuild wasm | typecheck, `test:ui`, build, smoke |
| **6** | `TASK-7.0-DESIGN.md` + `TASK-7.0-REPORT.md`; final gate sweep | full workspace green, clippy 0 findings |

Recommended authorization, in one line: **proceed with Task 7.0 as specified — typed ports on
`GeometryData` (F1a), both value and geometry outputs (F2c), the chain DAG in `vectra-procedural`
with pre-application cycle rejection and no value references back into the graph (F3), and the
publish model with the pass running last (F4b), across the five node kinds in §F5.**

Awaiting the word.
