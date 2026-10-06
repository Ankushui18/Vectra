# ⚙️ VECTRA: MASTER ENGINEERING SPECIFICATION (MES) v1.0

**Locked doctrine:** Engine-First, Graph-Oriented, Parametric.
Treating animation as a separate subsystem, relying solely on Cassowary, or
using a flat JSON-property model are architectural traps. Geometry, logic, and
motion are the same editable system.

## 1. Repository Structure

```text
vectra/
├── Cargo.toml                 # Workspace definition
├── crates/
│   ├── vectra-core/           # IDs, Document model, Command/Undo system
│   ├── vectra-geometry/       # Primitives, Paths, Arcs, Regions, Tessellation
│   ├── vectra-constraints/    # Linear (Cassowary) & Nonlinear solvers, Diagnostics
│   ├── vectra-expression/     # Lexer, Parser, AST, Compiler, Incremental Evaluator
│   ├── vectra-operations/     # Non-destructive Boolean, Offset, Fillet, Mirror, Repeat
│   ├── vectra-procedural/     # Typed node graph, Generators, Modifiers
│   ├── vectra-motion/         # Timeline, Springs, Physics, State machines (Value<T> sources)
│   ├── vectra-render/         # wgpu scene graph, GPU buffers, Hit-testing (BVH/R-tree)
│   ├── vectra-export/         # Export IR, SVG, React/Vue codegen
│   └── vectra-wasm/           # wasm-bindgen bindings, Command Bus API
├── apps/
│   └── vectra-desktop/        # Tauri or Electron app hosting the React UI + WASM
└── tests/                     # Cross-crate scenario tests (per-crate proptests live in-crate)
```

## 2. Rust Crate Architecture & Key Dependencies

Shared versions live in workspace `Cargo.toml` (`[workspace.dependencies]`).

| Crate | Key deps |
|-------|----------|
| `vectra-core` | `uuid`, `serde`, `serde_json`, `thiserror` |
| `vectra-expression` | `logos`, `chumsky` |
| `vectra-constraints` | `cassowary`, `nalgebra`, `petgraph` |
| `vectra-geometry` / `vectra-operations` | `lyon` |
| `vectra-render` | `wgpu`, `encase` |
| `vectra-wasm` | `wasm-bindgen`, `serde-wasm-bindgen`, `web-sys`, `js-sys` |
| tests | `proptest` |

## 3. Core Type Definitions

```rust
pub type NodeId = uuid::Uuid;
pub type VariableId = String;
pub type ExpressionId = uuid::Uuid;

pub enum Parameter<T> {
    Literal(T),
    Variable(VariableId),
    Expression(ExpressionId),
    Animated(MotionBinding),      // Boxed recursion inside MotionBinding
    Procedural(NodeOutputId),
    Interaction(InputBinding),
}

pub trait Resolvable<T> {
    fn resolve(&self, ctx: &EvaluationContext) -> T; // time lives in ctx
}
```

## 4. Document Schema (Graph-Oriented)

```rust
pub struct Document {
    pub version: u32,
    pub variables: HashMap<VariableId, f64>,
    pub nodes: HashMap<NodeId, Node>,
    pub order: Vec<NodeId>,                        // draw order (back → front)
    pub expressions: HashMap<ExpressionId, ExpressionRecord>, // source in core; compiled artifact in vectra-expression
    pub constraints: Vec<ConstraintRecord>,        // typed in vectra-constraints (Phase 2)
    pub operations: Vec<OperationRecord>,          // typed in vectra-operations (Phase 2)
}
```

## 5. Geometry Primitives

True mathematical representations, not point arrays.
Scalar-first decomposition so constraints/expressions address axes directly:

```rust
pub enum NodeKind {
    Rectangle { x, y, width, height, corner_radius: Parameter<f64> },
    Circle { cx, cy, radius: Parameter<f64> },
    Arc { cx, cy, radius, start_angle, end_angle: Parameter<f64> },
    Path { start: Parameter<Point2>, segments: Vec<PathSegment> },
    Group { children: Vec<NodeId> },
}
```

## 6. Parameter Abstraction

See §3. Single source of truth for all dynamism. Type-erased command payload:

```rust
pub enum ParamValue {
    Float(Parameter<f64>),
    Point(Parameter<Point2>),
    Color(Parameter<Color>),
}
```

## 7. Expression AST

Custom, deterministic, type-checked language. No `eval()`.

```rust
pub enum ExprNode {
    Literal(f64),
    VariableRef(VariableId),
    BinaryOp { left: Box<ExprNode>, op: BinOp, right: Box<ExprNode> },
    FunctionCall { name: String, args: Vec<ExprNode> }, // sin, clamp, spring, …
}
```

Parsed once (logos + chumsky), compiled to bytecode for fast incremental eval.
Implements `vectra_core::ExpressionEvaluator`.

> **Task 2.1 implementation note (2026-10-05):** shipped as `vectra-expression`
> (`lexer → parser → ast → compiler → engine`; AST adds `Neg` + closed
> `FunctionName`, as the spec's open `name: String` can't be type-checked at
> parse). Perf law recalibrated with evidence: 100 ops measure ~0.49µs release
> (near the dispatch floor; the brief's 100ns needs JIT codegen) — pinned gate
> is 15µs debug-budget + a machine-independent linearity law, 130×+ inside the
> §18 2ms budget. Registry sync protocol lives at the `vectra-wasm` boundary.

## 8. Dependency Graph

`petgraph`. `Variable("base")` change → traverse to exactly the dirty
`Parameter`s, `Node`s, and render buffers.

```rust
pub struct DependencyGraph {
    graph: petgraph::Graph<DependencyNode, DependencyEdge>,
}
```

> **Task 2.2 implementation note (2026-10-05):** shipped as `vectra-dependency`.
> The sketch above is superseded in two ways, both binding:
> (1) `petgraph::stable_graph::StableGraph<GraphNode, DependencyEdge>` — index
> stability is what makes undo/redo of graph elements exact (removing a node
> must not renumber survivors), so the plain `Graph` of the sketch is not used;
> (2) Phase-1 edges carry no data (`DependencyEdge` is a marker type): every
> edge means "from depends on to".
> Vertices are `GraphNode::{Variable, Expression, GeometryProperty}` with an
> O(1) `node_to_index` map. Topology is **derived** from the document on every
> mutation (`sync` diffs desired vs current and applies minimal node/edge
> deltas), so the graph can never drift from the document. Propagation returns
> the affected set in *evaluation order* (dependencies first) and a
> `DirtySet` wrapped as `Option<DirtySet>` — `None` means "nothing to do",
> which is deliberately distinct from an empty `DirtySet` (that means
> "evaluate everything").
> Cycle gate: `try_add_edges` is atomic (tentative add, exact rollback on
> rejection) and `dry_run(removes, adds)` clones the graph for the pre-dispatch
> check, so a rejected command leaves document, history and graph
> byte-identical; typed as `VectraError::CyclicDependency`. Evidence: 2-cycle,
> 3-cycle and self-loop laws, plus a gate-fidelity proptest (predicted deltas
> == observed deltas over random command sequences).
> **Reachability caveat (honest scope):** with Phase-1 semantics (scalars in
> `Document.variables`, expressions reading variables, properties as leaves)
> the three vertex layers are strictly ordered, so no *command* can construct
> a cycle yet. The gate is therefore verified against the Phase-4 shape
> (`Variable(a) → Expression` "a is defined by expr1" + `Expression → Variable(b)`)
> and by an invariant proptest asserting the graph is acyclic and identical to
> the derived topology after every random command. When variable-defining
> expressions land, rejection is already wired with zero further changes.
> The clock rides the same path: `$time` is `Variable("time")`, so `set_time`
> dirties exactly its readers (Phase-4 motion uses this unchanged).
> Perf (release): `SetVariable` end-to-end on 502 nodes 37.5µs; propagation
> 1.20µs @10 nodes vs 1.17µs @4002 nodes (O(affected), not O(document));
> one-node patch 190.7µs vs full 4002-node rebuild 878.1µs — §18's 2ms holds.

## 9. Constraint Architecture

```rust
pub trait ConstraintSolver {
    fn add_linear(&mut self, c: LinearConstraint);                    // Cassowary
    fn solve_nonlinear(&mut self, c: NonlinearConstraint) -> Result<(), SolverError>;
    fn diagnose(&self) -> ConstraintDiagnostics;                     // Satisfied | Overconstrained | Conflicting
}
```

## 10. Boolean Architecture (Non-Destructive)

```rust
pub enum OperationNode {
    Boolean { op: BooleanOp, operand_a: NodeId, operand_b: NodeId },
    Fillet { target: NodeId, radius: Parameter<f64> },
    Offset { target: NodeId, distance: Parameter<f64> },
}
```

Operands remain editable; results computed lazily/cached.

## 11. Procedural Graph API

```rust
pub trait GeometryNode {
    fn inputs(&self) -> Vec<Port>;
    fn outputs(&self) -> Vec<Port>;
    fn evaluate(&self, inputs: HashMap<PortId, GeometryData>) -> GeometryData;
}
```

Implements `vectra_core::ProceduralEvaluator`. Nodes: GridGenerator,
NoiseModifier, PathSmooth, …

## 12. Motion API

Motion is just another `Parameter` resolver. Implements
`vectra_core::MotionEvaluator`, resolved during `evaluate(time)`.

```rust
pub enum MotionBinding {
    KeyframeTrack { track_id: String, property: String },
    Spring { target: Box<Parameter<f64>>, stiffness: f64, damping: f64 },
    StateDriven { state: String, true_value: Box<Parameter<f64>>, false_value: Box<Parameter<f64>> },
}
```

Static-preview fallback (no motion crate wired): springs preview at target,
state branches preview the false arm, timelines error typed.

## 13. Render Pipeline

```text
Document → [Evaluator] → EvaluatedScene (resolved polygons/curves)
         → [Tessellator] → RenderScene (wgpu buffers)
         → [wgpu] → Screen
```

Renderer knows nothing about constraints/expressions. Dirty flags scope
tessellation + GPU uploads to changed nodes.

## 14. Command/Undo Architecture

Event-sourced. No full-document cloning. `apply` returns the exact inverse.

```rust
pub enum Command {
    CreateNode { id: NodeId, kind: NodeKind, name: Option<String>, index: Option<usize> },
    DeleteNode { id: NodeId },
    SetParameter { node_id: NodeId, property: String, value: ParamValue },
    SetVariable { name: VariableId, value: f64 },
    RemoveVariable { name: VariableId },
}
```

## 15. WASM Boundary

Minimal surface: commands in, events/snapshots out.

```rust
#[wasm_bindgen]
impl VectraEngine {
    pub fn dispatch_command(&mut self, cmd_json: &str) -> String;
    pub fn get_snapshot(&self) -> String;
    pub fn set_time(&mut self, t: f64);
    pub fn undo(&mut self) -> String;
    pub fn redo(&mut self) -> String;
}
```

## 16. React ↔ Rust Communication

1. User clicks “Add Circle” in React.
2. React sends `{"type":"CreateNode",…}` to WASM.
3. WASM executes, updates dependency graph, marks render dirty.
4. WASM emits `{"type":"NodesUpdated","ids":[…]}`.
5. React updates Inspector; canvas (wgpu) redraws.

## 17. Test Architecture

`proptest` laws, not examples. Per-crate suites + cross-crate scenarios:

```rust
proptest! {
    #[test]
    fn test_parallel_constraint_preservation(a in arbitrary_line(), b in arbitrary_line(), delta in arbitrary_vector()) {
        // constrain → move A → solve → assert angle(A,B) ∈ {0°, 180°}
    }
}
```

## 18. Performance Requirements

| Tier | Budget | Scope |
|------|--------|-------|
| Immediate | < 2 ms | simple parameter changes |
| Incremental | < 16 ms / frame | solve + tessellate typical icons (≤ 500 nodes) |
| Background | > 50 ms offloaded | complex booleans / procedural graphs → Web Workers + loading state |

## 19. MVP Acceptance Criteria (Phase 0 + Phase 1)

- [x] Rust workspace compiles to native (WASM target next: Task 1.4).
- [x] Create `Rectangle` and `Circle` via Command API.
- [x] Change `Rectangle.width` via `Parameter::Literal` and `Parameter::Variable`.
- [x] Changing a `Variable` resolves the new width through the same path.
- [x] Basic wgpu renderer draws resolved shapes with fill/stroke. (Task 5.0)
- [x] Undo/redo for `CreateNode` and `SetParameter` (+ variables).

## 20. Phase-1 Implementation Tasks

1. **Task 1.1** — Scaffold workspace (`Cargo.toml` for all crates). ✅
2. **Task 1.2** — `vectra-core`: `NodeId`, `Parameter<T>`, `Command`, `Document` + undo. ✅
3. **Task 1.3** — `vectra-geometry`: `NodeKind` + `EvaluationContext` resolution → `EvaluatedScene`. ✅ (10 props, 20 unit tests)
4. **Task 1.4** — `vectra-wasm`: `VectraEngine` + minimal React/TS remote control. ✅ (boundary tests + 6-step Node smoke green)
5. **Task 1.5** — Proptests: resolution laws + undo laws. ✅ (16 properties, 35 tests total)

---

## Appendix A — Phase-1 engineering decisions (binding)

1. **Core is the dependency root.** Evaluator *traits* live in core; owner
   crates implement + inject them. No `core → X` edges, ever.
2. **Scalar-first geometry.** Rect/Circle/Arc decompose to `Parameter<f64>`
   fields; `Parameter<Point2>` is reserved for path topology.
3. **Stub registries are forward-compatible.** `ExpressionRecord` persists
   source; compiled/typed artifacts in owner crates key by the same IDs.
4. **Order is explicit.** `Document.order: Vec<NodeId>` owns z-order;
   undo restores exact slots via `CreateNode.index`.
5. **Failures are typed.** Missing evaluators, type mismatches, and unknown
   properties are `ResolveError`/`VectraError` variants — never panics, never
   silent coercions. Proven by proptest laws in `vectra-core/tests/`.
6. **Evaluation is total.** `GeometryEvaluator` never panics and never halts:
   unresolvable geometry skips its node + `Error` diagnostic; mappable
   violations (negative size, `opacity ∉ [0,1]`, oversize corner radius)
   clamp + `Warning` so scene *membership* stays stable across animation;
   style failures fall back to defaults + `Warning`. Proven by proptest laws
   in `vectra-geometry/tests/`.
7. **Arcs canonicalize losslessly.** Evaluated arcs store `start ∈ [0, TAU)`
   and `end = start + sweep` with `sweep ∈ [0, TAU]`; `sweep == 0` is empty,
   `sweep == TAU` is a full circle (non-zero winding landing on its start).
   Direction is always increasing-angle; multi-turn windings collapse to one
   full turn. The renderer computes `sweep = end - start`, never branches.
8. **Renderable range is explicit.** Every evaluated scalar is finite and
   `|v| ≤ f32::MAX` (`is_renderable`), so `f64 → f32` GPU conversion cannot
   produce NaN/infinity downstream. Out-of-range values skip (geometry) or
   fall back (style) with diagnostics.
9. **Groups flatten.** `NodeKind::Group` is structural-only in Phase 1 (no
   transforms), so evaluation emits no node for it; children keep their
   document-order slots exactly. No diagnostic — flattening is lossless.
10. **Incrementality is API-first.** `DirtySet` (empty = full rebuild,
    otherwise explicit ids; unknown ids ignored) plus
    `EvaluatedScene::apply_partial` (patch + rebuild z-order from live
    document order) establish the Phase-4 boundary today: the dependency
    graph will produce dirty sets, the renderer will diff scenes. Patch ≡
    rebuild is a proptest law.
11. **lyon is fed by an explicit subpath state machine.** lyon 1.x panics on
    `build()`-before-`end()` and on edge-after-`close()` (no auto-begin), so
    `build_path` tracks open/closed state, re-begins at the close point, and
    ends trailing subpaths itself. Arbitrary segment streams (leading /
    doubled `Close`, post-close draws) are valid input — pinned by proptest
    regression seeds.
12. **`EvaluatedPrimitive::Rect` carries `corner_radius`** (clamped to
    `[0, min(w,h)/2]`), a deliberate extension of the §1 sketch: the core
    `Rectangle` authors it, and dropping it at the handoff would silently
    lose intent one tessellation step later.
13. **The `{status,…}` envelope lives in `vectra-wasm`, not core.**
    `CommandResponse` (`ok+events` / `error+message`) is the WASM transport
    shape; core's native `DispatchResult` keeps its own. Neither side is
    coupled to the other's transport needs.
14. **Snapshots are serializable projections, not object dumps.**
    `SnapshotResponse` carries scene + variables + diagnostics + undo state +
    time; `Path` crosses as SVG data (`path_to_svg_data`, pure, in
    `vectra-geometry`); colors cross as hex; names join from the document.
    Any host can preview without a tessellator.
15. **React is dumb — provably.** `wire.ts` mirrors the serde contract
    exactly, `commands.ts` builders own all JSON construction (UI code never
    hand-writes commands), and `App.tsx` renders snapshots only. The SVG
    preview is a pure view projection of snapshot data.
16. **WASM toolchain: `wasm-bindgen --target web`, CLI pinned to Cargo.lock**
    (script-enforced; `wasm-pack`'s npm layer buys nothing for a
    string-in/string-out boundary). Bindings import via `?url` asset URL —
    no WASM Vite plugins. `uuid/js` supplies wasm entropy (uuid calls
    WebCrypto directly; getrandom is not in its wasm graph).
17. **E2E gate without a browser.** `npm run smoke` instantiates the real
    compiled module in Node and asserts load → create → snapshot → variable
    bind → undo-chain → typed errors. Browser automation (Playwright) is
    deferred until canvas interactions land; `tsc` + `vite build` + the
    manual checklist cover the UI layer in 1.4.
18. **`vectra-wasm` sees only core + geometry.** Render/wgpu, solvers, and
    graphs are excluded by the boundary law (documented in the crate
    manifest); they join later through the same Command/Event/Scene
    protocol, never as direct dependencies.
