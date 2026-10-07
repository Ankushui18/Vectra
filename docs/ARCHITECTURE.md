# ARCHITECTURE — the Unified Document Graph, as-built and as-intended

> **Living document.** Owns exactly two things: (1) the **crate dependency map**
> and (2) the **Unified Document Graph (UDG) data flow** — first as the code
> stands today, then as the target the roadmap builds toward, with the diff
> between them stated explicitly. `MES.md` is the historical design brief; this
> file is the as-built truth plus the intended direction, and where the two
> disagree, this file wins because it cites code.
>
> Diagrams are ASCII on purpose: they survive `git diff`, they render in every
> editor and on GitHub, and they can be checked into a test that greps for the
> edges they claim (§9).

---

## 1. The one law everything else is shaped by

```text
The engine only ever asks          Resolvable::resolve(ctx, time) -> T.
It never cares whether T came from a literal, a variable, an expression,
a motion binding, a procedural port, or a live input.
```

Written at the top of `crates/vectra-core/src/lib.rs` and made literal by
`enum Parameter<T>`:

```rust
pub enum Parameter<T> {
    Literal(T),                  // an authored number
    Variable(VariableId),        // $base
    Expression(ExpressionId),    // "1.5 * $base"
    Animated(MotionBinding),     // a keyframe track, a spring, a state branch
    Procedural(NodeOutputId),    // another node's port
    Interaction(InputBinding),   // live pointer input
}
```

Five of those six variants are **links**, not values. This is why the document
can stay small and scalar while the artwork is fully parametric, and it is the
reason a single dirty-set mechanism can serve variables, expressions, motion,
procedural graphs, components and constraints without a special case for any of
them. Every architectural decision below follows from this one.

---

## 2. Layers of state — authored, derived, evaluated, drawn

The single most important property of this codebase: **only one layer is
authored; everything else is a pure function of it.**

| Layer | Type | Lives in | Persisted? | Rebuilt how |
|---|---|---|---|---|
| **Authored** | `Document` (nodes, parameters, variables, expressions, constraints, operations, motion, procedural, layers, artboards) | `vectra-core` | **yes** — `.vectra` | loaded |
| **Derived** | `DependencyGraph` (vertices/edges, dirty sets) | `vectra-dependency` | **no** | `derive(doc)` then `sync(doc)` |
| **Derived** | `RegionGraph` (faces, crossings, spans) | `vectra-geometry` (`regions.rs`) | **no** | re-derived from resolved boundaries |
| **Evaluated** | `EvaluatedScene` (resolved primitives + z-order) | `vectra-geometry` (`scene.rs`) | **no** | full pass or incremental patch |
| **Drawn** | `RenderScene` (tessellated buffers, instances, hit index) | `vectra-render` | **no** | derived from `EvaluatedScene` |
| **Exported** | Export IR (SVG / React / Vue) | `vectra-export` | **no** | derived from resolved scene |

Deriving rather than storing is not a style preference — it is what makes undo,
redo, and command replay need **zero special cases**. `vectra-dependency`'s
first rule says it plainly: *"State is derived, never maintained twice.
`derive(doc)` reads the document; `sync` reconciles the live graph with that
derivation."* A graph that is remembered can drift; a graph that is derived
cannot.

---

## 3. Crate dependency map (measured from `Cargo.toml`, 14 crates)

The rule: **every internal arrow points at `vectra-core`.** No crate depends on
anything that could depend on it, so there are no cycles, and `vectra-core`
depends on no internal crate at all.

```
                          ┌──────────────────────────────────────────┐
                          │  vectra-core        (13,262 src LOC)     │
                          │  IDs · Parameter<T> · Document · Node    │
                          │  Command/undo · Engine · eval traits     │
                          │  components · constraints · operations   │
                          │  procedural · layers · style · summary   │
                          └───▲───▲───▲───▲───▲───▲───▲───▲───▲───▲──┘
        ┌─────────────────────┘   │   │   │   │   │   │   │   │   └───────────────┐
        │        ┌────────────────┘   │   │   │   │   │   │   └───────┐           │
        │        │      ┌─────────────┘   │   │   │   │   └───┐       │           │
        │        │      │      ┌──────────┘   │   │   │       │       │           │
        │        │      │      │      ┌───────┘   │   │       │       │           │
        │        │      │      │      │      ┌────┘   │       │       │           │
        │        │      │      │      │      │        │       │       │           │
   ┌────┴───┐ ┌──┴───┐ ┌┴─────┐ ┌────┴──┐ ┌─┴────┐ ┌─┴────┐ ┌┴─────┐ ┌┴───────┐ ┌─┴────┐
   │geom-   │ │expr- │ │motion│ │const- │ │oper- │ │draw  │ │depen-│ │export  │ │ai    │
   │etry    │ │ession│ │      │ │raints │ │ations│ │      │ │dency │ │        │ │      │
   │6,537   │ │1,303 │ │1,208 │ │2,250  │ │1,309 │ │3,458 │ │2,656 │ │1,818   │ │3,616 │
   └───┬────┘ └──────┘ └──────┘ └───────┘ └──────┘ └──────┘ └──┬───┘ └────────┘ └──────┘
       │  lyon · geo · rustybuzz · ttf-parser                  │ deps: core, expression,
       │                                                       │       geometry
       │                                    ┌──────────────────┘
       │                            ┌───────┴────────┐
       └───────────────────────────▶│  vectra-render │  wgpu 22 · lyon · geo(dev: naga)
                                    │  4,234         │
                                    └───────┬────────┘
                                            │  ▲ dev-only (tests)
   ┌──────────────┐   ┌──────────────┐      │  │
   │ vectra-file  │   │vectra-proced.│      │  │
   │ 485 (flate2) │   │ 1,515        │      │  │
   └──────┬───────┘   └──────┬───────┘      │  │
          │                  │              │  │
          │   deps: core, geometry          │  │
          │   dev: dependency, operations,  │  │
          │        expression               │  │
          └──────────┬───────┴──────────────┴──┘
                     ▼
        ┌────────────────────────────────────────────────────────────┐
        │  vectra-wasm    (8,370 src LOC, 8,073 test LOC)            │
        │  the ONLY public API: 2 wasm-bindgen classes,              │
        │  VectraEngine (54 methods) + Renderer (16 methods)         │
        │  deps: draw, constraints, core, expression, geometry,      │
        │        motion, dependency, operations, procedural,         │
        │        export, ai, render   (dev: draw, file)              │
        └───────────────────────┬────────────────────────────────────┘
                                │  JSON strings over a command bus
        ┌───────────────────────┴────────────────────────────────────┐
        │  apps/vectra-web  (React 18 + Vite 6, ~14.9k TS/TSX)       │
        │  "a dumb remote control": no geometry, no state, no logic  │
        └────────────────────────────────────────────────────────────┘
        ┌────────────────────────────────────────────────────────────┐
        │  apps/vectra-desktop  (Tauri v2) — file host only:         │
        │  open_document · save_document(_as) · verify_document ·    │
        │  forget_path · current_path · host_info; state = one Path  │
        └────────────────────────────────────────────────────────────┘
```

**Two edges deserve a comment.**

- `vectra-dependency → vectra-render` is **dev-only**. The 10.2 laws make claims
  about both halves of the pipeline (the scene the evaluator produces *and* the
  draw list the renderer derives), so the test needs the renderer — but no
  shipping crate gains an edge, and `vectra-render` therefore stays free to not
  depend on the graph. If this edge ever becomes a real dependency, a cycle
  becomes possible and the layer rule is broken.
- `vectra-procedural`'s deps on `dependency`, `operations`, `expression` are
  likewise **dev-only**, for the same reason.

The layering is a *checkable* property, not a convention: `tools/check_docs.py`
(Phase 1, workstream 1.0) is specified to compare the diagram in this file
against `cargo metadata` and fail on drift. Until that script exists, this
diagram is maintained by hand and the "no cycles" claim is verified by the fact
that `cargo build` succeeds — which is real but weaker.

---

## 4. The as-built data flow (today)

```
  AUTHORED INPUT — UI gestures, AI commands, or a loaded .vectra file
        │  everything becomes a Command (62 of them), nothing mutates directly
        ▼
  ┌──────────────────────────────────────────────────────────────────────┐
  │ dispatch_command(json)          — the mutation protocol, in order    │
  │  1. parse                          → typed error on garbage           │
  │  2. pre-validate expression sources → InvalidExpression, no mutation  │
  │  3. dry-run the graph delta         → CyclicDependency, no mutation   │
  │  4. core.dispatch                   → the ONLY document mutation      │
  │  5. settle: registry sync → graph sync → dirty propagation → scene    │
  │             patch                                                     │
  │  6. events + Dirty{ids, mode}       → the UI sees what it cost        │
  └────────────────────────────────┬─────────────────────────────────────┘
                                   │ steps 2–3 are why "a refused command
                                   │ changes nothing" is a promise, not a hope
                                   ▼
  ┌──────────────────────────────────────────────────────────────────────┐
  │ DOCUMENT (authored, persisted)                                       │
  │  nodes: HashMap<NodeId, Node>  ·  order: Vec<NodeId>  (back → front)  │
  │  variables: HashMap<VariableId, f64>   expressions: HashMap<_,_>      │
  │  constraints · operations · motion · procedural · layers · artboards  │
  │  Node = { id, name, kind, style, visible, locked }                    │
  │  NodeKind = Rectangle · Circle · Arc · Path · Text · Group            │
  └───────┬───────────────────────────┬──────────────────────────────────┘
          │ derive(doc)               │ boundaries resolve
          ▼                           ▼
  ┌────────────────────┐     ┌────────────────────────┐
  │ DependencyGraph    │     │ RegionGraph            │
  │ petgraph Stable-   │     │ planar arrangement:    │
  │ Graph: vertices =  │     │ faces, crossings,      │
  │ nodes/params/exprs │     │ spans, membership      │
  │ + cycle gate +     │     │ signatures — derived   │
  │ dirty closure      │     │ on demand, never saved │
  └─────────┬──────────┘     └───────────┬────────────┘
            │  dirty ids                 │  seed points
            ▼                            ▼
  ┌──────────────────────────────────────────────────────────────────────┐
  │ EVALUATION — Resolvable::resolve(ctx, time)                          │
  │   ExpressionEngine   → f64         (logos + chumsky, compiled once)   │
  │   MotionEngine       → f64         (tracks sampled at ctx.time)       │
  │   ProceduralEngine   → f64 / Point2 (typed ports, on-demand topology) │
  │   ConstraintSolver   → proposes writes; the ENGINE commits them       │
  │   OperationsEvaluator→ geometry     (boolean/offset/fillet/mirror/    │
  │                                      clip/smart-fill, non-destructive)│
  │   FontLibrary        → glyph runs   (rustybuzz shaping + ttf-parser)  │
  └────────────────────────────────┬─────────────────────────────────────┘
                                   │  EvaluatedScene {nodes, z_order}
                                   │  EvalMode::Full | EvalMode::Incremental
                                   │  law: patch ≡ rebuild, byte-identical
                                   ▼
  ┌──────────────────────────────────────────────────────────────────────┐
  │ Renderer: tessellate → plan → instances → hit index → wgpu buffers    │
  │ 16 exported methods: attach · view · stats · gpu_ready · set_viewport │
  │ pointer_hit · pointer_doc · document_to_client · measure · navigate · │
  │ frame_document                                                        │
  └──────────────────────────────┬───────────────────────────────────────┘
                                 │
      ┌──────────────────────────┴───────────────────────────┐
      ▼                                                      ▼
  ┌──────────────────────────┐                  ┌────────────────────────┐
  │ Export IR → SVG (semantic│                  │ Snapshot → the React   │
  │ data-vectra-node ids,    │                  │ host: scene · variables│
  │ deterministic numbers) · │                  │ expressions · solver · │
  │ React/Vue codegen ·      │                  │ diagnostics · graph ·  │
  │ artboard-scoped SVG      │                  │ eval mode · can_undo   │
  └──────────────────────────┘                  └────────────────────────┘
```

### 4.1 What the document does **not** yet have

Four absences define Phase 1–3, and they are absences of *model*, not of code:

| Absent | Today | Consequence |
|---|---|---|
| **`Node.transform`** | a node's numbers are its world coordinates | groups are structural/flattening; nesting cannot compose placement; hit-testing, export and the region graph each assume identity |
| **Raster as a node kind** | `NodeKind` has 6 variants, none of them raster | `EvaluatedPrimitive` has no raster arm; the scene cannot be one pass over vector + raster + text |
| **A resource table** | fonts live in an in-memory library; nothing else does | no images, no embedded profiles, no binary payloads in `.vectra` |
| **Masks as node references** | two layer booleans (alpha lock, clipping mask) | no mask node, no invert, no feather, no luma masks, no group masks |

Also worth stating plainly, because it is a strength that looks like a gap:
**components are not a new node kind.** A master and an instance are both
`ProceduralNode`s (`ComponentMaster` / `Component`); an instance's artwork is an
ordinary authored `Group` whose slots are bound to expressions and published
ports. Nothing is conceptually duplicated, so clones render, layer, hit-test and
export through paths that already existed. Any future "container" concept —
boolean groups, masks, symbols with overrides — should follow this precedent:
**model it as a derived relationship over authored nodes, not as a fourth
parallel hierarchy.**

---

## 5. The target UDG data flow (Phase 1 → Phase 3)

### 5.0 The definition — *"Unified Document Graph"* (ratification proposal D1)

The brief names the target architecture but does not define it, so the
definition lives here explicitly, to be argued with rather than smuggled:

> **UDG.** One document, one graph. Every drawable — vector node, text run,
> raster layer, boolean result, procedural output, component instance, mask —
> is a `Node` in a single arena with: a stable id; a kind owning its data; a
> **transform** (local → parent, invertible, animatable); exactly one parent
> (node / layer / artboard) and an ordered child list; a paint and compositing
> descriptor (appearance stack, blend, masks); parameters that are
> `Parameter<T>` (so every number can be a variable, an expression, a motion
> sample or a port — the same six links as §1); and a place in one z-order.
> Nothing about a drawable is stored anywhere a second time.

Five clauses of that definition are **not yet true** in the code — they are
exactly the absences in §4.1. Ratifying D1 is therefore equal to scheduling
workstreams 1.1, 1.6, 2.1 and 3.1.

### 5.1 The diagram

This is the diagram the roadmap is building toward. Read the changed rows as
"plus"; everything not listed is unchanged from §4.

```
  ┌──────────────────────────────────────────────────────────────────────┐
  │ DOCUMENT — one graph, one id space, one parent per node              │
  │  nodes · transform (local → parent, invertible, animatable)          │
  │  exactly one parent (Node | Layer | Artboard) + ordered children      │
  │  paint/compositing descriptor (appearance stack, blend, masks)        │
  │  parameters · typed variables · resources (fonts, images, tiles)      │
  │  NodeKind += RasterLayer · ComponentInstance · BooleanRef · MaskRef   │
  └───────┬───────────────────────────┬──────────────────────────────────┘
          │ derive                    │ resolve through the SAME transform
          ▼                           ▼      chain (one world_of(node))
  ┌────────────────────┐     ┌────────────────────────┐
  │ DependencyGraph    │     │ RegionGraph            │
  └─────────┬──────────┘     └───────────┬────────────┘
            └──────────────┬─────────────┘
                           ▼
  ┌──────────────────────────────────────────────────────────────────────┐
  │ EVALUATION (unchanged shape) + COLOUR SPACE                          │
  │  … existing evaluators …            linear working space → output    │
  └────────────────────────────────┬─────────────────────────────────────┘
                                   ▼
  ┌──────────────────────────────────────────────────────────────────────┐
  │ ONE SCENE: vector draws · text outlines · RASTER draws               │
  │  EvaluatedPrimitive += Raster { tiles, transform, blend }            │
  └────────────────────────────────┬─────────────────────────────────────┘
                                   ▼
  ┌──────────────────────────────────────────────────────────────────────┐
  │ RENDERER: vector pass + TILED RASTER COMPOSITOR (Phase 2)            │
  │  GPU: path instancing (today) + tile atlas, brush stamping,          │
  │  per-layer blend, alpha lock, selection scratch buffers              │
  └────────────────────────────────┬─────────────────────────────────────┘
                                   ▼
  ┌──────────────────────────────────────────────────────────────────────┐
  │ EXPORT/IMPORT: SVG · PNG (rasterised) · React/Vue · .vectra v2       │
  │ (binary resource chunks, colour space tag) · SVG import              │
  └──────────────────────────────────────────────────────────────────────┘
```

The **three moves that unlock it** are, in order:

1. **Transforms + one parent** (workstream 1.1). Everything downstream — hit
   testing, overlays, export `<g transform>`, snapshots, the raster compositor's
   tile placement — must share one `world_of(node)` implementation. Doing this
   per-feature is how a codebase ends up with four different notions of "where
   is this node".
2. **Raster as a first-class kind** (Phase 2). A `RasterLayer` node + a resource
   table + `EvaluatedPrimitive::Raster` + a compositor. This is the largest
   single boundary change the repo will ever make, because `vectra-wasm` — the
   widest, most heavily tested surface (8,073 test LOC) — sits exactly on it.
3. **Typed variables + resources in the container** (Phase 3). `HashMap<String,
   f64>` becomes a typed variable space (scalar/colour/string/alias/modes), and
   `.vectra` v2 gains binary chunks. Both are format changes; both need the
   version field that `vectra-file` already pins.

---

## 6. Invariants — the architecture's immune system

Each of these is a claim you can break. Each is enforced by something concrete;
the "enforced by" column is what makes this document falsifiable rather than
aspirational.

| # | Invariant | Enforced by |
|---|---|---|
| I1 | **Evaluation is total.** No panic on any document the API can produce. | typed `VectraError` / `ResolveError` everywhere; proptest fuzz over documents (`geometry_eval`, `tessellation_laws`) |
| I2 | **Failures never mutate.** A rejected command leaves document, history and graph bit-identical. | protocol steps 2–3 (`vectra-wasm/tests/workspace_boundary_laws.rs`) |
| I3 | **Cycles are impossible, not unlikely.** | `try_add_edge` gate + `dry_run` before dispatch (`dependency_laws.rs`, `tree_laws.rs`) |
| I4 | **Topology is derived, never stored twice.** Undo/redo/replay need no graph special case. | `graph::derive` + `DependencyGraph::sync`; `incremental_laws.rs` |
| I5 | **`patch ≡ rebuild`.** Incremental evaluation is byte-identical to a full pass. | `vectra-render/tests/incremental_laws.rs`, smoke step "patch ≡ rebuild" |
| I6 | **Every mutation has an exact inverse.** | `vectra-core/tests/undo_redo.rs`; undo laws in `workspace_boundary_laws.rs` |
| I7 | **A refused command is typed, not silent.** | error-envelope tests across `vectra-wasm/tests/*` |
| I8 | **Identical documents produce identical bytes.** Pinned `.vectra` header, gzip via a pure-Rust backend, `float_roundtrip` on every JSON hop. | `vectra-file/tests/roundtrip_laws.rs` (12 laws) |
| I9 | **The host is a dumb remote.** No geometry, no resolution, no document state outside the engine. | boundary review; `workspace_boundary_laws.rs` (5 laws) |
| I10 | **The engine proposes; the document commits.** The solver returns writes. | `constraint_laws.rs` (7 laws), drag triad laws |

I5, I6 and I8 are the three that make the *product* possible: they are what let
a UI offer live editing, unlimited undo, and a file that survives a round trip
without a diff.

---

## 7. The boundary — the only public API in the repo

`vectra-wasm` exposes **two `wasm-bindgen` classes** (`apps/vectra-web/src/wasm/vectra_wasm.d.ts`):

| Class | Methods | Covers |
|---|---:|---|
| `VectraEngine` | **54** | the command bus (`dispatch_command`, `get_snapshot`, `undo`, `redo`, `set_time`, `dependencies`, `force_full_evaluation`), document I/O (`document_json`, `document_summary`), export (SVG ×3, React), AI (prompt, phrasings, generate, execute, retry), components (`component_view`, `create_component`, `instantiate_component`, `set_component_prop`, `icon_set`), draw tools (`draw_pointer`, `draw_pen_commit`, `draw_brush_commit`, `draw_quick_shape`, `draw_overlay`, `draw_hit`, `draw_edit_*`), text (`font_families`, `register_font`, `outline_text`), regions (`smart_fill_plan`, `break_path`), motion (`bind_spring`, `bind_hover_spring`, `set_motion_track`, `set_state`, `is_animating`, `motion_json`), procedural (`procedural_json`, `procedural_kinds`, `structural_macros`), `set_selection`, `render_frame` |
| `Renderer` | **16** | `attach`, `view`, `stats`, `gpu_ready`, `set_viewport`, `measure`, `pixels_per_unit`, `document_to_client`, `pointer_doc`, `pointer_hit`, navigation (`nav_pan`, `nav_scale`, `nav_zoom`, `nav_zoom_to`), `frame_document`, `frame_document_default` |

Every method is **JSON string in / JSON string out** on the command bus; the
envelopes are `{"status":"ok",…}` or `{"status":"error","message":…}`, and the
snapshot carries `scene · variables · expressions · diagnostics · graph · eval ·
can_undo · can_redo · time`. Consequences worth naming:

- The boundary is **testable without a browser** — which is why 79 smoke steps
  run in Node against the real wasm.
- The boundary is **wide and flat** (70 methods). Every Phase 2 raster
  capability either rides on existing verbs or adds to this surface; there is no
  plugin seam. Widening it deliberately is cheap (it is all one file's worth of
  glue + laws); widening it *accidentally* is the risk to watch.
- Because it is the only public API, the entire TS host can be rewritten without
  touching Rust. That is the mitigation for the audit's "debug dashboard" UI
  finding: **the UI is not load-bearing.**

---

## 8. Persistence — the `.vectra` container

```
 ┌────────────┬──────────────┬───────────────────────────────────┐
 │ "VECTRA"   │ u16 LE ver   │ gzip( Document JSON )             │
 │ 6 bytes    │ = 1          │ (flate2 rust_backend — deterministic
 └────────────┴──────────────┴───────────────────────────────────┘
```

The pinned magic + version header is a deliberate byte-identity choice: the same
document saves to the same bytes on every platform, because the compression
backend is pure Rust and no system zlib can differ. The document→command replay
plan (`vectra-file/src/plan.rs`) means a loaded file is not "deserialized state"
but a *sequence of ordinary commands* — so loading is undoable, graph-consistent
and path-identical to authoring by hand.

**v2 (Phase 2/3) must add, without breaking v1 readers:** a resource table with
binary chunks (fonts, images, raster tiles), a typed-variable space, an explicit
colour-space tag, and a transform in every node. The version field is already
there for exactly this.

---

## 9. ADR index

Architecture decisions are recorded here, numbered, with a status. **A pending
decision is not an architecture** — the code is. Ratifying an ADR means editing
the as-built diagram in §4 and closing the matching item in `ROADMAP.md` §9.

| ADR | Decision | Status | Owner workstream |
|---|---|---|---|
| **D1** | The UDG definition in §5.0: one graph, one id space, one parent, transform on every node, resources in the document | **PROPOSED** — 5 clauses not yet true (§4.1) | 1.1 (+ 1.6, 2.1, 3.1) |
| **D2** | Transforms live in `vectra-core` (model), are evaluated by `vectra-geometry`/`vectra-render`, and the UI never owns transform maths — one shared `world_of(node)` | **PROPOSED** | 1.1 |
| **D3** | Operations may consume other operations' outputs (nested booleans / a real Pathfinder tree) | **PROPOSED** — today booleans are binary and non-nestable | 1.4 |
| **D4** | Masks are **node references**, not materialised geometry | **PROPOSED** — today they are two layer booleans | 1.6 |
| **D5** | `.vectra` container v2 (resources, typed variables, colour space, transforms) lands in **Phase 2**, not Phase 1 — only Phase 2 needs it, and landing it once avoids a second migration | **PROPOSED** | 2.1 |
| **D6** | Procedural expansion (3.7) may be pulled forward into Phase 2 if parametric-pattern work becomes a product priority | **PROPOSED** | 3.7 ← 2.x |

Accepted decisions are added **above** this table by replacing the row with a
short section: context, decision, consequences, and the commit that made it
true. Nothing is marked ACCEPTED on the strength of intent.

---

## 10. Keeping this file true

1. **§3 must be machine-checked.** `tools/check_docs.py` (Phase 1, workstream
   1.0) compares this diagram's edges against `cargo metadata --no-deps`; a
   crate added, removed or re-pointed without updating §3 fails the check.
2. **§4.1's absence table shrinks only when the model changes** — cite the
   commit that added the field or the `NodeKind` variant, in the same row.
3. **§6's invariant table may only grow with an enforcement column filled in.**
   An invariant with no test is a wish, and wishes do not belong in an
   architecture document.
4. **§5's target diagram is allowed to change; §4's as-built diagram is not
   allowed to be aspirational.** If §4 stops matching the code, that is a bug in
   this file, and the file is expected to be *re-measured* the way
   `ENGINE_STATUS.md` is.
5. **§9's ADRs are ratified in `ROADMAP.md` §9's order, one at a time**, each
   with the commit that made it true. A PROPOSED row surviving two phases is a
   signal that the decision was actually settled by accident — find it and
   record it, or the architecture has drifted.
