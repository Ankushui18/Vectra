# VECTRA — REPO-LEVEL ENGINEERING AUDIT & MASTER ROADMAP

**Document type:** strategic audit + phased implementation plan
**Audit date:** 2026-10-07
**Branch audited:** `arena/05758d13-vectra` (base commit `5422a99`, squashed)
**Workspace:** 14 crates · ~52,000 lines of Rust (`src/`) · ~23,800 lines of Rust tests · ~14,000 lines of TypeScript/TSX (excl. checked-in wasm) · ~4,300 lines of UI tests
**Deliverable status:** planning only — **no feature code was written, no engine file was modified**

---

## 0. HOW THIS AUDIT WAS MADE (READ THIS FIRST — IT LIMITS EVERY CLAIM BELOW)

Brutal honesty applies to the auditor as much as to the auditee. This is what the
evidence for this document actually is:

**What was done**
1. Every `Cargo.toml`, every crate's `src/` tree, and every public surface
   (module docs, `pub fn`/`pub enum`/`pub struct` lists) was read.
2. Keyword sweeps were run across the whole tree (`.rs`, `.ts`, `.tsx`) for the
   phenomena the brief asks about: raster/pixel/tile/brush/smudge/texture/tilt,
   OKLab/OKLCH/ICC/P3/color-space/gamma, mask/clip/alpha-lock, import,
   variable-font/`fvar`/GSUB/GPOS/feature-tag, nonlinear/tangent/symmetric,
   timeline/easing. **A subsystem is called `MISSING` here because that sweep
   came back empty and no type in the workspace can represent it — not because a
   filename was absent.**
3. All 28 `TASK-*.md` reports were scanned (headings, gate tables, deviation and
   limitation sections), plus `MES.md` and `README.md` in full, and the eight
   task reports for the subsystems that carry the most risk were read in detail.
4. Test mass was counted **from source**: `#[test]` attributes (650), `proptest!`
   blocks (28), and UI `it(`/`test(` cases (146) — and cross-checked against the
   gate numbers the task reports claim.

**What was executed (and what it produced)**

The repo's own offline tooling was used to build and run the suite:

```text
python3 tools/extract_toolchain.py /tmp/rust      → cargo 1.97.0 (c980f4866)
PYTHONPATH=/tmp/pylibs python3 tools/vendor_deps.py   → 247 crates vendored
cargo test --workspace --lib --tests --no-fail-fast -j 2
    → 650 passed / 0 failed  (54 test targets)
cargo clippy --workspace --all-targets -- -D warnings
    → clean (0 warnings)
cargo fmt --all -- --check
    → clean
cd apps/vectra-web && npm ci
    → npm run typecheck  : clean
    → npm run test:ui    : 146 tests, 146 pass, 0 fail
    → npm run smoke      : 79 steps, SMOKE PASS
    → npm run build      : EXIT 0, 7.72 s
        dist/assets/vectra_wasm_bg-DsKpeYfC.wasm  27,686,250 B  (gzip 3,594.91 kB)
        dist/assets/index-DeeFiym4.js                325,089 B  (gzip 98.39 kB)
        dist/assets/index-Cgrqbkem.css                25,472 B

cargo build --target wasm32-unknown-unknown -p vectra-wasm           (dev)     → 45,634,937 B
cargo build --release --target wasm32-unknown-unknown -p vectra-wasm (release) → 10,066,632 B
    → the committed apps/vectra-web/src/wasm/vectra_wasm_bg.wasm is 27,686,250 B and
      contains no .debug_info/.debug_line sections — consistent with the dev profile
      (which pins debug = "line-tables-only") that build-wasm.sh selects by default
```

Four caveats on those numbers, stated plainly:

1. **Doc-tests did not run.** The bundled toolchain ships `rustc`, `cargo`,
   `rustfmt`, `clippy` and `rust-analyzer` **but not `rustdoc`**, so `--doc`
   targets fail with "No such file or directory". The 650 figure is
   `--lib --tests` only; the reports' "650 passed (56 targets)" therefore
   includes ~2 doc-test targets this run could not execute.
2. The first vendoring pass produced a **broken `wgpu-types@22.0.0`** (a
   reconstructed manifest missing `macro_rules_attribute` while the sources
   imported it) — the full workspace failed to compile until
   `tools/vendor_deps.py --only wgpu-types --force` re-vendored it from a
   different tree. That is a real defect in the offline build tooling, and it is
   filed as such in §3.9.
3. No GPU device, no browser, no display: the renderer's WGSL compiles and its
   laws pass, but nothing rendered pixels here; smoke exercises the wasm module
   in Node, not WebGPU.
4. **The release wasm size is bounded, not measured.** The `wasm-bindgen` CLI is
   absent from this sandbox and the in-repo `apps/vectra-web/scripts/wbgen`
   driver has its own 42-crate closure (not vendored), so what is proven is
   `cargo build --release` = 10,066,632 B *pre-bindgen* versus the committed
   27,686,250 B artifact. The post-bindgen release artifact will be smaller than
   10,066,632 B, by how much is unknown. The finding — the shipped web artifact
   is a dev-profile build — does not depend on that number, and is recorded in
   `docs/ENGINE_STATUS.md` §2.

**What was NOT done**: no perf re-measurement (the reports' microsecond figures
remain *(reported)*); no desktop-app run; no browser session; no history
archaeology (the repository has **one squashed commit**, so "was this ever
broken" cannot be answered from git).

Wherever a number below is quoted rather than measured, it is marked
*(reported)*; measured numbers are stated as such.

**Status vocabulary (used exactly, everywhere in this document)**

| status | meaning |
|---|---|
| `IMPLEMENTED` | fully functional against its declared spec, gated by tests, usable in production |
| `PARTIAL` | core logic exists; edge cases, UI integration or performance work missing |
| `SCAFFOLDED` | crate/file exists with basic structs; no real logic or tests |
| `MISSING` | not present in the repository at all |
| `DEPRECATED` | replaced by a newer approach and should be removed |

---

# PART I — RULE 1: BRUTAL STATUS ASSESSMENT

## 1. The scoreboard

| # | Subsystem | Status | Confidence | Rust src | Tests (counted in source) | Headline truth |
|---|---|---|---|---|---|---|
| 1 | Core Document Graph | `PARTIAL` | high | 13,262 | 79 tests + 2 proptest blocks | A real registry-based document with 62 undoable commands — but **no transforms anywhere**, and layers/artboards/groups are three parallel hierarchies, not one graph |
| 2 | Vector Geometry | `PARTIAL` | high | 6,537 | 89 tests (incl. 22 text) + 4 proptest blocks | Rect/Circle/Arc/Bézier/Path are solid and canonicalised; **no Ellipse, no NURBS, no polygon/star, no path effects, no stroke-to-path** |
| 3 | Constraint Solver | `PARTIAL` | high | 2,250 | 22 tests + 7 constraint + 8 drag wasm laws | Cassowary linear solver with a deterministic drop pass works; **no nonlinear pass → no Tangency, Symmetry, true Parallel/Perpendicular; no on-canvas glyphs; 4 buttons of UI** |
| 4 | Boolean & Operations | `PARTIAL` | high | 1,309 | 31 tests + 3 proptest blocks | Boolean/Offset/Fillet/Mirror/SmartFill are non-destructive and tested; **binary-only booleans, nested unions unsupported, reflex fillets left sharp, results are polylines** |
| 5 | Smart Fills & Region Graph | `IMPLEMENTED` | high | ~1,600 (regions 1,251 + clip 349) | 5 regions + 5 region laws + 7 smart-fill laws | The best-finished subsystem: real planar arrangement, seed-point semantics, spans, Break Path, ColorDrop — with known limits (exact closure, O(n²), flattened faces) |
| 6 | Procedural Graph | `PARTIAL` | high | 1,515 | 23 tests + 12 + 10 laws | 7 typed kinds, deterministic noise, one cycle gate; **no radial repeat, no along-path repeat, no symmetry, no scattering, no graph UI, no node instancing** |
| 7 | Motion & Animation | `PARTIAL` | high | 1,208 | 19 tests + 7 wasm laws | Closed-form springs and total tracks are excellent; **linear interpolation only, no timeline/keyframe UI, no animation export, no state machine graph** |
| 8 | Raster & Brush Engine | `MISSING` | very high | 0 | 0 | Nothing. No pixel, tile, bitmap, texture or image type exists in the workspace; "brush" is vector curve-fitting with a width profile. **No PNG export exists either** |
| 9 | Typography | `PARTIAL` | high | 1,928 | 22 text tests + 21 text laws | Parametric text, HarfBuzz-grade shaping, text-on-path, non-destructive outline — **no bold/italic, no variable fonts, no OpenType feature toggles, no bidi, no area text** |
| 10 | Color Management | `MISSING` | very high | 0 | 0 | `Color { r,g,b,a: u8 }` is the entire colour model. No linear space, no P3, no OKLab/OKLCH, no ICC, no colour variables, no gradient colour-space control |
| 11 | Masks & Compositing | `PARTIAL` | high | (clip.rs 349) | 8 clip + 5 alpha-lock laws | Two layer booleans (alpha lock, clipping mask) + even-odd holes + **4 blend modes**; no mask nodes, no invert/feather, no group masks, no alpha inheritance |
| 12 | Export & Interoperability | `PARTIAL` | high | 1,818 | 17 tests + 7 wasm laws | SVG export is genuinely semantic and React codegen is parametric; **SVG import does not exist, no raster/PDF export, no Vue/Svelte** |
| 13 | AI Command Interface | `PARTIAL` | high | 3,616 | 34 laws + 9 wasm laws | The schema/validate/self-correct architecture is real and tested; **there is no model** — the shipped planner is a keyword matcher and the chat path is an unwired seam |
| 14 | UI/UX Shell | `PARTIAL` | high | 14,016 TS | 146 UI + 79 smoke steps | Dense, functional, dark-only engineering console: canvas + inspectors + a debug strip with the dependency graph and the raw event log in the default layout |

Weighted reading of the scoreboard: **one** subsystem (Smart Fill) is finished to
its declared spec; eleven are in the `PARTIAL` band (most of them deep into it);
two are wholly absent; nothing is `DEPRECATED` — the codebase has never once
thrown something away, which is itself a finding (§3.1).

---

## 2. Subsystem-by-subsystem assessment

### 2.1 Core Document Graph — `PARTIAL`

**Crate:** `vectra-core` (13,262 src LOC, 16 files).

**What is really there.** `Document` is a registry-of-registries, not a flat
property blob — exactly the MES §4 doctrine:

```rust
pub struct Document {
    pub version: u32,
    pub variables: HashMap<VariableId, f64>,
    pub nodes: HashMap<NodeId, Node>,
    pub order: Vec<NodeId>,                  // draw order
    pub expressions: HashMap<ExpressionId, ExpressionRecord>,
    pub constraints: ConstraintRegistry,
    pub operations: OperationRegistry,
    pub motion: MotionTrackRegistry,
    pub procedural: ProceduralRegistry,
    pub layers: LayerRegistry,
    pub artboards: ArtboardRegistry,
    pub active_layer: Option<LayerId>,
}
```

`Command` ships **62 variants** (MES §14 sketches 5), every one returning an
exact inverse; `CommandStack` gives byte-exact undo/redo; `Engine` is headless
and host-agnostic; `geometry_ids()` unifies nodes + enabled operations +
procedural geometry under one id space (`OperationId = NodeId` — a genuinely
good decision); `DocumentSummary` (874 LOC) is a first-class, tested projection
for the AI layer. `vectra-dependency` (2,656 LOC, 3,533 test LOC) derives the
dependency graph from the document on every mutation — "the graph can never
drift from the document" — with an atomic cycle gate and a
predicted-deltas==observed-deltas proptest. This is the strongest single piece
of engineering in the repo after the region graph.

**What is missing, in order of structural damage:**

1. **No transform system.** Every node's geometry is authored in absolute
   document coordinates: `Rectangle { x, y, width, height, corner_radius }`,
   `Circle { cx, cy, radius }`, `Group { children }` — no rotation, no scale,
   no skew, no nested coordinate spaces. MES Appendix A #9 states it outright:
   *"Groups flatten. `NodeKind::Group` is structural-only in Phase 1 (no
   transforms)."* This blocks, directly: group manipulation, component scaling
   beyond the prop-scaling workaround, icon-set transforms, rotated artboards,
   and the placement of anything raster in Phase 2. **This is the #1 blocker in
   the entire product.**
2. **Three parallel hierarchies, not one graph.** Layer membership
   (`AssignNodeToLayer`), parentage (`SetNodeParent` / `child_parent`) and draw
   order (`Document.order` + per-layer lists) are three mechanisms that must be
   kept consistent; Task 10.4 had to write a new `LayerRegistry` walk and special
   cases ("a row whose parent is missing is still emitted as a root") because the
   invariant is not a type. The target architecture wants *one* parent link and
   *one* ordered child list per parent.
3. **No node kinds for half the product.** `NodeKind` ∈ {Rectangle, Circle, Arc,
   Path, Text, Group}. There is no `Ellipse`, `Polygon`, `Star`, `Line`,
   `Frame`, `Image`/`Raster`, `Mesh`, or `Instance`.
4. **No node-level resources.** No styles (paint/text/effect styles), no
   shared palettes, no per-node variables (document `variables` are `f64` only —
   a colour cannot be a variable), no component *library* (components live in the
   same document).
5. **`Document.variables: HashMap<String, f64>` is the whole variable model** —
   no types, no modes (light/dark), no aliasing, no colour or string variables.

**Verdict:** the *spine* is production quality; the *skeleton* is incomplete.
Nothing here needs a rewrite; §7.1 specifies the surgical addition.

---

### 2.2 Vector Geometry — `PARTIAL`

**Crate:** `vectra-geometry` (6,537 src LOC) + `vectra-draw` (3,458) for
authoring-side math.

**Present and correct:** rectangles with clamped corner radius; circles; **true
arcs** with the lossless canonicalisation (`start ∈ [0, TAU)`, `sweep ∈ [0, TAU]`,
`TAU` = full circle, pinned by proptests); paths of `Line | Quadratic | Cubic |
Close` with `Parameter<Point2>` control points; lyon as the single path
representation; deterministic arc sampling (`ARC_SEGMENTS_PER_TAU = 64`); region
conversion with an explicit even-odd ring model; the SVG exporter emits a
**single `A` command** for an arc, never a bezier chain (verified in
`svg.rs::arc_data`).

**Missing / weak:**

| gap | evidence | consequence |
|---|---|---|
| **NURBS / B-splines** | zero hits across the tree for nurbs/bspline/basis spline | the brief's "NURBS" item does not exist |
| **Ellipse** | `NodeKind` has `Circle` only; arcs carry a scalar `radius` | a non-uniform ellipse cannot be authored, only approximated as a path |
| **Polygon / Star / Spiral / Line** | no such kinds | every such shape must be hand-built as a path |
| **Path effects / stroke-to-path** | `vectra-draw/outline.rs` expands a *brush gesture* into a closed path; nothing converts an authored stroke into an outline for booleans | "Outline Stroke" — a daily Illustrator operation — is absent |
| **Boolean-grade curve fidelity** | `multi_polygon_to_path` emits polylines; `FLATTEN_TOLERANCE = 0.05` | boolean/region results are faceted and never refit back to cubics |
| **Elliptical arcs (rx≠ry)** | `Arc { radius: Parameter<f64> }` | no |
| **Conics in the path model** | `PathSegment` is L/Q/C/Close | imported SVGs cannot represent arcs/ellipses natively (relevant to §7.8) |

**Verdict:** what exists is mathematically careful and property-tested. What is
missing are whole *primitive kinds*, and `Path` is the only escape hatch. Phase 1
must at minimum add `Ellipse` and curve refitting for operation results, or the
SVG importer will be second-class on day one.

---

### 2.3 Constraint Solver — `PARTIAL`

**Crates:** `vectra-constraints` (2,250 src LOC), model in
`vectra-core/src/constraint.rs`.

**Present and correct:** 8 constraint kinds (`Coincident`, `Horizontal`,
`Vertical`, `Parallel`, `Perpendicular`, `EqualLength`, `Distance`, `Angle`);
Cassowary-backed tableau (no home-grown solver); a **deterministic drop pre-pass**
(`plan.rs`) that fixes Cassowary's silent-failure problem (weakest loses, ties
drop the newer row, two contradicting `Required` rows are a typed error with
nothing applied); four strengths; `solve` writes back through ordinary
`SetParameter` commands so geometry changes are undoable, graphed and
incrementally evaluated; the drag path is Cassowary's native **edit-variable**
API (`BeginDrag`/`UpdateDrag`/`EndDrag` as one undo entry; re-anchor on release);
diagnostics (`ConstraintDropped`, `ConstraintSkipped`) reach the UI;
22 solver tests + 7 constraint laws + 8 drag laws.

**Missing / weak:**

1. **No nonlinear solver.** `solve_nonlinear` exists in MES §9 and nowhere in
   the code. `ConstraintKind::Parallel`/`Perpendicular` are *axis-offset
   approximations on float slots*, not vector relations —
   `TASK-3.1-OPEN-ITEMS.md` §D5 states this explicitly and defers the true forms
   to "Phase 2+".
2. **No Tangency, Symmetry, Concentric, Collinear, Midpoint, Fix/Anchor,
   Equal-radius, or inequality (≥/≤) constraints.** All six of the brief's named
   constraint families are therefore: Coincident ✓, Parallel ~ (axis form only),
   Perpendicular ~ (axis form only), Tangent ✗, Symmetric ✗, and "etc." = 4 more
   linear kinds.
3. **No UI glyphs, anywhere.** The constraint UI is a *panel* of text rows
   ("Vertical · medium", with pause/remove buttons) and **four buttons**
   (Vertical, Coincident, Hold distance, Over-constrain). Nothing is drawn on the
   canvas: no glyph badges on constrained geometry, no hover highlight of a
   constraint's targets, no click-to-select-a-constraint, no "show constraints"
   mode. The prompt asks for "their UI glyphs" and the answer today is: **there
   are none.** *(Static grep of `App.tsx`, `RegionOverlay`, `DrawOverlay` — no
   constraint rendering path exists.)*
4. **Pair-only, n-ary impossible:** builders always take the *first two* selected
   layers; there is no multi-select constraint, no constraint between geometry
   *features* (an anchor to an edge), and auto-capture only records the current
   value.
5. **No inference/snapping to constraints** (the "smart pen" wires snapping, but
   the solver is not consulted for suggested constraints).

**Verdict:** the mathematics and the transaction model are right; the
*vocabulary* and the *UX* are about 25 % of an Illustrator-class system.

---

### 2.4 Boolean & Operations — `PARTIAL`

**Crates:** `vectra-operations` (1,309 src LOC), `vectra-core/src/operation.rs`.

**Present and correct:** five non-destructive operation kinds —
`Boolean { op }` (union/subtract/intersect/exclude via `geo::BooleanOps`),
`Offset { distance }` (`geo::Buffer`), `Fillet { radius }` (tangent-arc math with
CAD-style clamping to half the shorter edge), `Mirror { axis }`
(`geo::AffineOps` + winding repair), `SmartFill` (§2.5). Sources are never
mutated; the result is a *virtual node with a real id* in the same id space,
carrying its own `StyleProperties`, evaluated lazily in the scene pass with dirty
propagation and a `retired()` path so failures are visible (not silently stale).
Operation glyphs (∪ − ∩ ⊕) exist for the layers panel. 31 operation-crate tests
(incl. 12 boolean laws) + 8 clip laws state their claims as area identities.

**Missing / weak:**

| gap | detail | why it matters |
|---|---|---|
| **Binary booleans only** | `OperationKind::Boolean` reads exactly two inputs | Illustrator's Pathfinder is n-ary; TASK-10.6 documents a real user-facing consequence: *"A nested union is a documented engine limitation"* — an operation cannot take another operation's output |
| **No Divide / Trim / Merge / Crop / Exclude-variants panel** | only the four set ops | the standard Pathfinder panel is not expressible |
| **Reflex fillets left sharp** | documented in `ops.rs` | rounding concave corners is a core CAD/logo operation |
| **No Chamfer** | absent | common alternative to fillet |
| **No Outline Stroke / Expand** | absent | blocks the "shape from a stroked path" workflow and any boolean on a stroke |
| **Polyline results** | boolean/offset/fillet output is flattened | results look worse than their inputs at scale, and get worse with each chained op |
| **Fill-rule freedom** | even-odd is hard-wired (`geo` default + renderer `FillRule::EvenOdd`) | importing SVG with nonzero-fill compound paths is lossy |
| **Booleans need closed regions** | open paths/slivers are skipped | line-vs-line operations impossible |

**Verdict:** what exists is honest, tested, and architecturally right
(non-destructive containers, not destructive edits). It is roughly the
"4 Pathfinder buttons" level; the panel itself, n-ary chaining and
expand/outline are missing.

---

### 2.5 Smart Fills & Region Graph — `IMPLEMENTED`

**Crates:** `vectra-geometry/src/regions.rs` (1,251 LOC),
`vectra-operations` (Smart Fill evaluation), `vectra-wasm` (plan + break), the
web Region UI.

This is the newest work (Task 12.0) and the only subsystem I am willing to call
finished against its declared spec:

- **Region Graph**: pairwise `geo` booleans refined by membership signature →
  planar faces, each a closed lyon path with holes intact; `face_at(point)`
  answers the *smallest* containing face; ring-major **crossings** and **spans**
  (the arcs between consecutive cuts, deduped at `REGION_EPSILON = 1e-6`);
  the reference arrangement (two r=100 circles 120 apart) is pinned by tests to
  the reference areas `22449.251521908423 / 22449.251792811 / 8916.23314174575`.
- **Smart Fill as a parametric node**: `OperationKind::SmartFill { seed,
  boundaries }` — the fill stores *where you clicked*, not a face index, so it
  survives renumbering; it re-derives when boundaries move; when the seed leaves
  every face the geometry is *removed* and a `smart-fill-empty` diagnostic is
  raised (`retired()` → `scene.retire`) so the failure is visible rather than a
  frozen ghost.
- **Spans + Break Path at Intersections**: `BreakPath { node_id, pieces }` sends
  the pieces with the command (one `Batch` inverse = one undo), hides the source,
  and the UI lists spans with the engine's own numbers.
- **ColorDrop into a region** asks `smart_fill_plan` — the same call that
  powers the hover highlight — so what glows is exactly what a click fills.

**Honest limits (not defects, but they belong in FEATURE_MATRIX):**
flattened faces (`FLATTEN_TOLERANCE = 0.05`) mean filled regions are polylines;
the arrangement is O(n²) pairwise and runs on the main thread (MES §18's
">50 ms → Web Worker" offload is **not implemented**); regions require *exact*
closure (no gap tolerance, unlike Illustrator's Smart Fill); and the pairwise
granularity means one pathological source can dominate cost.

---

### 2.6 Procedural Graph — `PARTIAL`

**Crate:** `vectra-procedural` (1,515 src LOC) + core types.

**Present and correct:** seven `ProceduralKind`s — `Source`, `Grid`, `Repeat`,
`Noise` (deterministic splitmix-hashed value noise, byte-identical native/wasm),
`Smooth` (Chaikin-style), `ComponentMaster`, `Component`; strictly typed ports
(`Scalar | Point | Points | Path | Region | Color`); topology *derived*
(`topological_order`), no second graph, the workspace's single cycle gate;
published value table read by `Parameter::Procedural`; a fixpoint law so
evaluation settles; "no silent eviction" (`Document::is_geometry_id`).

**Missing, measured against the brief's four named generators:**

| requested | status | note |
|---|---|---|
| Grid | `PARTIAL` | a grid *of points* and its spanning region — **not** an array of copies of a shape |
| Radial repeat | `MISSING` | `Repeat { count, dx, dy }` is linear only |
| Along-path repeat | `MISSING` | no path sampling in the procedural crate |
| Symmetry | `MISSING` | `Mirror` is an *operation*, not a graph node; no mirror/repeat/rotational symmetry generator |
| Scatter / random placement | `MISSING` | noise displaces vertices; it does not place copies |

Also absent: a **node-graph UI** (Task 7.0 report is explicit: *"the Procedural
panel (dropdown-wired, no canvas graph)"*), subgraphs/node groups, instancing of
*nodes* (Repeat duplicates region geometry, not editable instances), procedural →
texture (there is no raster to shade), and procedural-aware export (React export
writes resolved geometry, not the generator).

---

### 2.7 Motion & Animation — `PARTIAL`

**Crates:** `vectra-motion` (1,208 src LOC), motion registry in core.

**Present and correct:** springs are the **closed-form step response**
(`spring.rs`, 486 LOC, 9 unit tests) with `stiffness`/`damping` and an explicit
anchor `(from, at)` stored *in the document* (so scrubbing, replay and undo are
pure functions of time — no frame integrator, no hidden state); re-anchoring on
drag commit is an ordinary undoable edit; `settle_epsilon`, `is_animating`,
`status`; keyframe tracks are total (defined before/after the span) with a
binary-search sampler and segment diagnostics; state-driven branches with spring
smoothing; State/Track vertices in the dependency graph; 10 wasm methods and 7
end-to-end motion laws through the frame loop.

**Missing / weak:**

1. **Linear interpolation only.** `track.rs` says it plainly: a Catmull-Rom or
   Bézier track "is deferred to Phase 2 with the track-editing UI and its own
   graph vertices." No easing curves, no per-key interpolation mode, no
   hold/step.
2. **No timeline UI.** There is a **time slider** and a Motion panel; there is no
   keyframe editor, no playhead, no dope sheet, no curve editor, no playback
   controls with in/out, no per-track rows. (`SetMotionTrack` exists at the
   boundary; the UI only demonstrates it.)
3. **No animation export** — no GIF/MP4/WebM, no Lottie, no CSS keyframes, no
   SVG SMIL. An animated Vectra document cannot leave Vectra except as a video
   of a screen.
4. **State "machines" are named booleans**, not graphs: no states, transitions,
   guards, or `set_state`-driven multi-state logic beyond true/false.
5. No stagger/sequence/offset helpers, no animation of colours or paths (motion
   bindings are `f64`-only: `Parameter<f64>`), no per-glyph text animation.

---

### 2.8 Raster & Brush Engine — `MISSING`

This is the audit's expected headline, and it is worse than "no brush engine":

- **Nothing raster exists.** A full-tree sweep for
  `raster|pixel|bitmap|tile|texture-map|RGBA buffer` finds only *comments* ("a
  picture cannot invent them", the GPU's backdrop texture) — no pixel type, no
  image node, no decode/encode, no resource table, no readback path.
- **No image format support at all.** No PNG/JPEG/WebP decoder or encoder in any
  `Cargo.toml` (`Cargo.lock` carries no image crate), so **a Vectra document
  cannot export a PNG.** The only outputs are SVG text and React text. For a
  design tool this is not a gap; it is a hole in the floor.
- **The brush is a vector fitter.** `vectra-draw` = pen handle algebra, stroke
  sampling with **pressure if available, else velocity**
  (`Sample { point, time, pressure }`, `BrushProfile { max_width, min_width }`),
  cubic curve fitting at stroke end, and outline expansion into a **closed vector
  path**. What is absent from a Procreate-class brush: texture/grain, tilt and
  azimuth, bristle/dab dynamics, scatter, jitter, spacing control, flow vs
  opacity accumulation, wet/smudge/blend tools, erasers, stamp brushes, dual
  brushes.
- **Blend modes: 4** (`Normal | Multiply | Screen | Overlay`) against Procreate's
  ~27 and Photoshop's 27+. The renderer implements these four *exactly* (with a
  backdrop-snapshot path for non-normal modes) — that part is `IMPLEMENTED`.
- **Alpha lock exists… for vectors.** `LayerRecord::alpha_locked` clips **new
  artwork** to the layer's region — semantically the right idea (Procreate's
  alpha lock), implemented as live vector clipping, not pixel masking. Likewise
  `clipping_mask`. See §2.11.
- **No selection tools** (marquee, lasso, polygon, wand), no transform/selection
  UI, no `select → invert → feather`.

**Verdict:** `MISSING`. Phase 2 is a from-scratch build, and it is the largest
single work item in the roadmap — larger than every Phase-1 workstream combined.

---

### 2.9 Typography — `PARTIAL` (strong core, thin periphery)

**Crates:** `vectra-geometry/src/text.rs` (1,928 src LOC), `NodeKind::Text`.

**Present and correct:** a fully parametric text node — `text`, `font_family`,
`font_size`, `letter_spacing`, `line_height`, `alignment (L/C/R)`, `x`, `y`, and
`on_path { node, offset }`, with **every number a `Parameter<f64>`** (so type is
animatable, constrainable and expression-driven like everything else);
`TextAlign` as structure, not a number; **shaping by `rustybuzz`** with the
face's own GSUB/GPOS (kerning, ligatures, mark positioning) into a glyph run
cached by (face, size, string) with stats and a clear function; **text on a
path** as a *one-way reference* validated at the command boundary (text cannot
bind to text → no dependency cycles), sampled by arc length with analytic
tangents from a curve-ified primitive (a 64-gon would visibly kink), per-glyph
tangent rotation plus a readability flip, parallel lines, an offset `Parameter`,
and a `text-overflow` diagnostic when the run does not fit; **non-destructive
`OutlineText`** where the per-glyph plans travel *with the command* (one undo);
one bundled face (`Vectra Sans` = a 55 KB DejaVu Sans subset, BITSTREAM VERA
licence asset present) plus host `register_font`, with a fallback *diagnostic*
rather than a silent substitution; the renderer treats a run as a path, so
tessellation, hit-testing, booleans and export need no text special-case.

**Missing (this is where "Task 11.0 state" stops):**

| gap | evidence |
|---|---|
| **No bold/italic/weight/style** | a "family" is *one face file*; `NodeKind::Text` has no weight/italic slots; no face-variant matching anywhere |
| **No variable fonts** | zero `fvar`/`gvar`/axis/instance hits; `ttf-parser` is used for outlines only |
| **No OpenType feature control** | no `ss01`/`smcp`/`onum`/`frac` toggles and no UI; only the face's defaults apply (rustybuzz does apply them) |
| **No bidi / RTL layout / vertical text** | layout is a per-line LTR box; no paragraph bidi algorithm |
| **No area text / text frame / wrapping / justification** | only point text on a baseline; no hyphenation, tabs or columns |
| **No text styles** | no shared style resource; every run is authored slot-by-slot |
| **No decorations** | no underline/strikethrough/overline in the model |
| **No font fallback chains** | unknown script → bundled face diagnostic; no per-script stack |
| **No in-canvas caret/IME** | typing happens in the Text panel |

---

### 2.10 Color Management — `MISSING`

The entire colour model is:

```rust
pub struct Color { pub r: u8, pub g: u8, pub b: u8, pub a: u8 }   // sRGB bytes
```

- **No colour spaces.** No linear working space, no Display-P3, no ICC, no
  premultiplied alpha, no gamma-correct blending, no `f32` colour anywhere in
  the document model.
- **No OKLab / OKLCH, no HSL/HSB in the model.** The UI's colour input is the
  browser's literal `<input type="color">` (sRGB pickers only) plus hex parsing.
- **No colour variables.** `Document.variables` is `f64`-only; a named colour
  can only be smuggled through a component colour prop or a procedural port.
  There are no swatches, no palettes, no libraries.
- **Gradients** exist (`Paint::Solid | Linear | Radial`, stops with u8 colours,
  document-space frames, `userSpaceOnUse` export) — but there is **no conic /
  angular gradient, no mesh gradient, no gradient-on-stroke, no per-stop
  position/alpha editor beyond the two-stop bar**, and **no control over the
  interpolation space** (stops are lerped without a defined colour space → muddy
  midpoints in the classic sRGB way).
- **No colour management on export**: no embedded profile, no
  `color-interpolation` hints, no PNG iCCP (there is no PNG).
- **Blend modes are 4** and there is no dithering, no gamut warning, no soft
  proof, no colour-blindness simulation.

**Verdict:** `MISSING` as a subsystem; a colour *type* exists but no colour
*management* does. This is Phase 3 work with a Phase 1 dependency: **gradient
interpolation must at minimum move to linear-light before Phase 1 ships**, or
every gradient in every deliverable is subtly wrong.

---

### 2.11 Masks & Compositing — `PARTIAL`

**What exists:**
- `LayerRecord::clipping_mask` — a layer shows only where the layer *below* it
  covers (live `region ∩ mask` recompute in the scene pass; `vectra-operations/
  src/clip.rs`, 349 LOC).
- `LayerRecord::alpha_locked` — new artwork is clipped to the layer's own
  existing content at the drawing boundary (the vector analogue of Procreate's
  alpha lock).
- Even-odd fill rule → nested subpaths are holes, in render, regions and export.
- **4 blend modes**, implemented in WGSL with two dedicated pipelines plus a
  backdrop snapshot for the non-normal modes (`RenderScene` splits the frame,
  `DrawItem::needs_backdrop()`, isolated-compositing badge in the UI).
- Per-appearance opacity and per-node opacity.
- 8 clip laws + 5 wasm alpha-lock laws.

**What is missing:** masks as **node references** (with invert, feather, and
luminance-vs-alpha semantics); vector mask *layers*; group masks; **alpha
inheritance** (a group's alpha multiplying its children); knockout; pass-through
vs isolated groups as an explicit property; masks on raster content (Phase 2);
mask UI beyond two toggle buttons in the Layers panel; blend modes beyond the
four; and any compositing of a node's *own* appearance stack against itself
beyond the default backdrop rule.

**Verdict:** `PARTIAL`, with the semantics (alpha lock, clip-to-below) proven and
the depth absent.

---

### 2.12 Export & Interoperability — `PARTIAL`

**Present and correct — the export half is genuinely good.**
`vectra-export` (1,818 src LOC, 764 test LOC) compiles a document to an explicit
`ExportIR` that both exporters consume (§RULE 3 of Task 8.0). The SVG exporter is
**semantic** — a circle is `<circle>`, an arc is one `A` command, only a true
path is `<path>` — with artboard scope (`current` / `all`, each in its own `<g>`
with background), gradients in `userSpaceOnUse`, opacity/stroke styling,
`data-vectra-node` identity attributes, deterministic numbers, a y-flip group and
a warnings list in the IR. The React exporter is **parametric**: a slot driven by
`$base * 2` exports as `width={base * 2}` with `base` declared as a required
prop; free variables become props; the IR deliberately carries both the
parametric text and the resolved number. 390 tests at Task 8.0's gate *(reported; the crate's own laws re-ran green in this audit)*.

**Missing / weak:**

| gap | evidence | consequence |
|---|---|---|
| **SVG import** | the word "import" appears in the codebase only in a comment reserving `ExportError` "for a future importer" and in a note that `data-vectra-*` exists "so an importer can line the elements up" | **no round-trip**; you cannot open a file a designer has |
| **Raster export (PNG/JPEG)** | no encoder, no readback | cannot deliver a pixel asset |
| **PDF / EPS / AI / DXF / Figma** | absent | no interop with the wider industry |
| **Vue / Svelte codegen** | MES/README sketch lists them; only React ships | named-but-absent |
| **Text as text** | runs export as `<path>` only | no selectable text, no font embedding/subsetting in SVG |
| **`<clipPath>`/`<mask>` emission** | clipping is resolved geometrically at export | the SVG is a *picture*, not a live editable structure that matches the document |
| **CSS variables / design tokens export** | absent | blocks the React → web-handoff story |

---

### 2.13 AI Command Interface — `PARTIAL` (the honest version: **there is no AI**)

**What is real and tested** (`vectra-ai`, 3,616 src LOC, 1,407 test LOC;
34 laws + 9 wasm laws):
- **RULE 1 — one language.** The AI's schema *is* `vectra_core::Command` with a
  single extension (`$new:<slug>` id tokens so a model never invents a UUID). No
  shim DSL, no SVG, no pixels.
- **RULE 2 — grounding.** The planner sees only `DocumentSummary` (874 LOC,
  its own tests): ids, names, kinds, variables, constraints, motion, procedural
  graph. The system prompt injects it.
- **RULE 3 — validation and self-correction.** Every command goes through the
  engine's own `dispatch`; a typed `VectraError` becomes a `Correction`, which
  becomes the next attempt's prompt, up to `MAX_ATTEMPTS`; exhaustion is a typed
  error with *nothing applied*. Preview (`exec::preview`) never mutates.
- **The boundary** (`ai_prompt`, `ai_phrasings`, `ai_generate_commands`,
  `ai_execute_commands`, `ai_execute_with_retry`), the **⌘K MagicBar**, prose
  receipts in the designer's language, and five one-tap **structural macros**
  ("Make geometric", "Color variations", "Align perfectly", "Create component",
  "Icon set").

**What is not real:**
1. **No model is wired.** `ChatPlanner<F>` wraps a caller-supplied
   `Fn(&str) -> Result<String, AiError>` — a *seam* with no implementation in the
   tree (no HTTP client, no provider, no key handling anywhere). The shipped
   planner is `HeuristicPlanner`, a **deterministic keyword/phrase matcher**
   whose vocabulary is enumerated by `phrasings()`. Say it plainly: today's "AI"
   is a phrase table. Anything phrased outside it fails, and the failure is a
   retry loop over a *different* phrase table.
2. **No AI-assisted constraint detection** (Phase 4's headline item is absent).
3. **No semantic retrieval or document reasoning beyond the summary struct.**
4. **No procedural macro generation** — the macros are five hard-coded prompts
   routed into the phrase matcher.
5. **No plan editing UI** — a plan is shown and executed or rejected wholesale.
6. **No evaluation harness** for AI quality (no corpus, no regression suite of
   prompts → expected plans).

---

### 2.14 UI/UX Shell — `PARTIAL` (and yes: it is a debug dashboard)

**Present:** React 18 + Vite 6 + strict TS; ~14,000 LOC of app code in 12
components + engine modules; a functioning WebGPU canvas driven entirely by
wasm snapshots; tool palette (Select / Direct Selection / Pen / Brush / Text /
Smart Fill, with V/A/P/B/T/F); Direct-Selection anchors and Bézier handles;
QuickShape (hold still to snap to a circle/rectangle); StreamLine smoothing
slider; ColorDrop with a drag ghost; touch gestures with a mode recogniser and a
no-context-menu/no-scroll guarantee; navigation overlay (zoom in/out/reset/fit,
nav window, artboard list); grid; artboard box; diagnostics strip; drag-status
strip; Layers tree with drag-and-drop re-parenting and operation rows; Appearance
panel (stacked fills/strokes, icon-row blend modes, gradient bar with draggable
stops, per-layer opacity); Text panel (family, size, spacing, leading, alignment,
path offset, outline); Component panel; AI panel; Export sheet with copy;
Region/spans UI; a **RULE 4 gate** (`showsMathPanels`) that hides the parametric
panels and shows a designer rail while a direct tool is held. 146 UI tests run in
Node (no browser) and 79 smoke steps drive the real wasm.

**Why the brief's "debug dashboard" verdict is correct, with receipts:**

| evidence | where |
|---|---|
| The header literally reads *"Engine remote · Tasks 3.1–3.2 · constraints + drag"* | `App.tsx` header |
| The canvas panel's subtitle is *"WebGPU · the engine tessellates snapshot.scene, the GPU draws it"* | Canvas panel `<h2>` |
| A **Dependencies** panel (vertices · edges · acyclic ✓) ships in the default layout | `deps-panel` |
| An **Engine events** log with sequence numbers and raw command JSON ships in the default layout | `log-panel` |
| An eval chip reports `last_mode` and "patch ≡ rebuild, every time" | header |
| User-visible copy cites rules and tasks ("RULE 4", "Task 12.0") | multiple panels |
| Diagnostics print raw codes (`[warning] font-fallback: …`) | diag strip |
| Dark mode is the *only* mode ("minimal dark console theme", CSS vars) | `App.css` |
| The tool palette is a row of labelled `<button>`s (with `<kbd>` hints) inside the Canvas panel — **not** a floating toolbar | `ToolPalette.tsx` |
| No icon system: emoji/unicode as UI chrome (⚙ ✓ ⏸ ▶ ✕ ⇩) | throughout |
| No menu bar, no preferences, no command palette (⌘K is the AI bar), no theme switch, no light mode, no resizable panes, no full-screen canvas | — |

Judged as an *engine remote*, it is excellent: dense, honest, and impossible to
misrepresent. Judged as a *designer's tool*, it is a prototype shell — the
canvas is one panel among many, and half the chrome is instrumentation.

---

## 3. Cross-cutting findings (the ones that decide the roadmap)

**3.1 Nothing has ever been deleted.** There is no `DEPRECATED` row in this
audit. `TASK-10.5` retired *one* placement verb, and that is the entire
deprecation history. A product with 28 task reports, 14 crates and no deletions
is carrying its scaffolding as load-bearing structure — expect to pay for it in
Phase 3.

**3.2 There is no CI.** No `.github/`, no workflow files, no CI configuration of
any kind. Nothing runs the gates automatically: a green task report is a
snapshot of one machine on one afternoon. 650 tests that run only when someone
remembers are not a safety net; they are a ritual.

**3.3 License and hygiene gaps.** `Cargo.toml` declares `MIT OR Apache-2.0` and
there is **no LICENSE file** in the repository. There is no `.gitignore`; the
**built wasm artifacts are committed** (`vectra_wasm.js`, `vectra_wasm_bg.wasm`,
`.d.ts`) — a real stale-binary hazard that the client's `STALE_ENGINE` probe
mitigates with a runtime warning rather than preventing.

**3.4 The documentation is drifting at roughly one README per five tasks.**
`README.md` advertises **"70 tests green workspace-wide"** while the source
contains **650 `#[test]` attributes**, 28 proptest blocks and 146 UI tests, and
the latest task report's gate says 650 Rust / 146 UI / 79 smoke. It lists 10
crates; the workspace has 14. It calls `apps/vectra-desktop` a *"Future Tauri
shell"*; Task 10.0 shipped it. Full reconciliation in Part IV.

**3.5 The "minimal WASM boundary" doctrine has quietly inverted.** MES §15
sketches **5** methods. The boundary now exports **48** `#[wasm_bindgen]`
items across `lib.rs`, `draw.rs` and `render.rs` (≈47 methods + constructors),
including three god-objects (`VectraEngine`, `Draw`, `Renderer`) and two
sub-engines (`DrawEngine`, `RenderEngine`). The string-JSON transport is
consistent and the TypeScript side mirrors it by hand in 1,348 lines of
`wire.ts` — a real drift surface. This is not wrong, but it is no longer "thin",
and Phase 2 will add far more surface unless the boundary is structured now.

**3.6 The AI is architecture without intelligence** (§2.13). The seam is right;
the engine behind it is a phrase table. Every "AI" claim in product
communication must carry that caveat until Phase 4.

**3.7 Performance claims live in reports, not in the codebase.** MES §18 sets
budgets (2 ms immediate / 16 ms incremental / >50 ms offloaded); the task reports
record achieved numbers (e.g. `SetVariable` end-to-end 37.5 µs on 502 nodes;
propagation O(affected); one-node patch 190.7 µs vs 878.1 µs full rebuild) — but
there is **no benchmark suite, no perf test, no CI measurement**, and the
"background offload" tier is unimplemented. `ENGINE_STATUS.md` (§14.3) must own
these numbers, and a `criterion` suite must own their regressions.

**3.8 Everything is in absolute document coordinates** (the transform finding,
restated because it recurs): booleans, constraints, masks, text-on-path, the
renderer, the exporters and the region graph are all written against a world
where a node's numbers *are* its position. Introducing transforms is a
**cross-cutting change with a shared invariant** — do it once, early, with the
graph restructuring, not per-feature.

**3.9 The offline build tooling is close but not trustworthy yet.** Three
defects were hit while *using* it (not reading it):

1. `tools/vendor_deps.py`'s first pass produced a `wgpu-types@22.0.0` whose
   reconstructed manifest omitted `macro_rules_attribute` while the sources
   imported it — the workspace does not compile until
   `tools/vendor_deps.py --only wgpu-types --force` re-vendors it from a
   different tree.
2. `tools/extract_toolchain.py` installs a toolchain **without `rustdoc`**, so
   doc-tests cannot run at all in the sandbox the repo's own script creates.
3. `tools/check.sh` appends `--offline` *after* the caller's arguments, so
   `tools/check.sh fmt …` errors with "Unrecognized option: 'offline'" and
   `tools/check.sh clippy … -- -D warnings` passes `--offline` to
   `clippy-driver`. Both gates only work when cargo is called directly with the
   environment variables the script sets.

Each is an hour's work — but together they mean the "green gate" a contributor
reproduces locally is *configuration-dependent*, which is exactly the class of
problem CI exists to remove.

---

# PART II — RULE 2: SUBSYSTEM MAPPING AGAINST THE UNIFIED DOCUMENT GRAPH

## 4. What "Unified Document Graph" (UDG) must mean, concretely

The brief names the target architecture but does not define it. I am stating my
reading explicitly so the roadmap can be argued with (and so the assumption is
recorded rather than smuggled):

> **UDG definition (audit's working model).** One document, one graph. Every
> drawable — vector node, text run, raster layer, boolean result, procedural
> output, component instance, mask — is a `Node` in a single arena with:
> a stable id; a kind owning its data; a **transform** (local → parent,
> invertible, animatable); exactly one parent (node/artboard/layer) and an
> ordered child list; a paint/compositing descriptor (appearance stack, blend,
> opacity, mask/clip references); and, for raster, a handle into a resource
> table. Layers and artboards are node kinds, not parallel registries. Values
> are bound only through `Parameter<T>` (`Resolvable`). Evaluation walks the
> graph once — respecting transforms, inheritance, masks and compositing — and
> produces one scene: vector draws, text draws, raster draws. Every subsystem
> (constraints, regions/booleans, procedural, motion, AI) addresses the same ids
> and slots.

**The delta from today's architecture**, in one table:

| UDG requirement | today | delta |
|---|---|---|
| one parent, one ordered child list | layer list + `child_parent` + `Document.order` (three mechanisms) | unify into `Node { parent, children }`; layers/artboards become kinds |
| per-node transform | none; absolute coordinates everywhere | add `Transform2D` + world-matrix evaluation through geometry/render/hit/UI/export |
| raster as a node kind | absent | Phase 2 (`RasterLayer` node + resource table + tiled payloads) |
| text/raster/vector compositing in one pass | vector-only scene; no raster draws | `EvaluatedPrimitive::Raster` + a compositing order that includes both |
| resource table (fonts, images, raster tiles) | fonts only, in-memory library; no resources | `Document.resources` + `.vectra` v2 container with binary chunks |
| typed variables (scalar/colour/string/alias/modes) | `HashMap<String, f64>` | variables v2 (Phase 3) |
| masks as node references | two layer booleans | `mask: Option<MaskRef>` with invert/feather/luma (Phase 1/3) |
| colour-managed pipeline | `u8` sRGB everywhere | linear working space + output spaces (Phase 3, with the linear-lerp fix in Phase 1) |

## 5. Crate-by-crate map (as-built)

| crate | src LOC | key deps | implements (traits) | status | Phase that touches it |
|---|---|---|---|---|---|
| `vectra-core` | 13,262 | uuid, serde | — (dependency root) | `PARTIAL` | 1, 2, 3, 4 |
| `vectra-geometry` | 6,537 | lyon, geo, rustybuzz, ttf-parser | — | `PARTIAL` | 1, 2, 3 |
| `vectra-dependency` | 2,656 | petgraph, core, expression, geometry | — | `IMPLEMENTED` (for its scope) | 1, 2, 3 |
| `vectra-expression` | 1,303 | logos, chumsky | `ExpressionEvaluator` | `IMPLEMENTED` | 3 (string/colour exprs) |
| `vectra-constraints` | 2,250 | cassowary, nalgebra, petgraph | — (proposes; engine commits) | `PARTIAL` | 1, 4 |
| `vectra-operations` | 1,309 | geo, lyon, geometry | — | `PARTIAL` | 1 |
| `vectra-procedural` | 1,515 | core, geometry | `ProceduralEvaluator` | `PARTIAL` | 1, 3 |
| `vectra-motion` | 1,208 | core | `MotionEvaluator` | `PARTIAL` | 1 (timeline UI), 3 |
| `vectra-render` | 4,234 | wgpu 22, lyon, geo, naga (dev) | `BufferSink` | `PARTIAL` (vector-only) | 1, 2 |
| `vectra-export` | 1,818 | lyon, expression | — | `PARTIAL` | 1 (import = new crate), 4 |
| `vectra-ai` | 3,616 | core, serde_json | `Planner` / `CommandHost` | `PARTIAL` | 4 |
| `vectra-draw` | 3,458 | core, geometry, constraints | — | `PARTIAL` | 1 |
| `vectra-file` | 485 | flate2, serde_json | — | `IMPLEMENTED` (v1 format) | 2 (v2 container) |
| `vectra-wasm` | 8,370 | everything above + wgpu | integration only | `PARTIAL` | 1, 2, 3, 4 |
| `apps/vectra-web` | ~14,000 TS | React 18, Vite 6 | — (dumb remote) | `PARTIAL` | 1, 2, 3, 4 |
| `apps/vectra-desktop` | Tauri v2 (main 9.2 KB, file_io 9.5 KB, verify 15.9 KB) | Tauri 2, vectra-file | document host | `IMPLEMENTED` (for its scope) | 2 (raster files) |

## 6. Target UDG data flow (the diagram `ARCHITECTURE.md` must own)

```
                       ┌──────────────────────────────────────────────┐
  authored input ─────▶│  Document (one graph, one id space)          │
  (UI / AI / file)     │  nodes · transforms · parent/children        │
                       │  parameters · variables · resources          │
                       └───────┬──────────────────────┬───────────────┘
                               │ derived              │ derived
                    ┌──────────▼────────┐   ┌─────────▼──────────────┐
                    │ DependencyGraph   │   │ RegionGraph            │
                    │ (dirty sets,      │   │ (faces, crossings,     │
                    │  cycle gate)      │   │  spans)                │
                    └──────────┬────────┘   └─────────┬──────────────┘
                               │                      │
        ┌──────────────────────▼──────────────────────▼───────────────┐
        │ Evaluation: Expression · Constraints · Motion · Procedural   │
        │             Operations/Boolean · Text shaping · Regions      │
        └───────────────────────────┬─────────────────────────────────┘
                                    │  EvaluatedScene
             (vector draws · text outlines · raster draws)  ← ONE scene
                                    │
                    ┌───────────────▼───────────────┐
                    │ RenderScene (tessellate,      │   ┌────────────────┐
                    │ plan, instances, hit index)   ├──▶│ GPU (vector +  │
                    │ raster tiles → compositor     │   │ tiled raster)  │
                    └───────────────┬───────────────┘   └────────────────┘
                                    │
                    ┌───────────────▼───────────────┐
                    │ Export/Import IR → SVG · PNG  │
                    │ · React/Vue · .vectra v2      │
                    └───────────────────────────────┘
```

**Invariants that must survive every phase** (they are the architecture's immune
system, and today they are enforced by tests, not by types): evaluation is
total; failures are typed and never panic; every mutation returns an exact
inverse; the graph is acyclic by construction with one gate; topology is derived,
never stored twice; `patch ≡ rebuild`; identical documents produce identical
bytes.

---

# PART III — RULE 3: THE PHASED ROADMAP TO VECTRA v1.0

Scope note: phases are **capability milestones, not calendars**. Each workstream
carries a size class (`S` ≤ a few days, `M` ≈ 1–2 weeks, `L` ≈ 3–6 weeks of
focused work) so sequencing can absorb whatever estimate policy the team uses.
Every phase ends with the same gate shape, and **no phase ends without its gate
implemented in code** (phase gates are themselves deliverables).

## Phase 1 — Professional Illustrator Core

**Goal:** a designer can produce a real vector illustration — shapes, booleans,
gradients, clipped layers, text on a path, constraints holding proportions — and
hand off a clean SVG that Vectra can also re-open. Phase 1 is the first release
that can honestly be called a design tool.

### 1.1 Transform & graph foundation (L) — *do this first*

**Crates:** `vectra-core` (model), `vectra-geometry` (evaluation), `vectra-render`
(world matrices, hit test), `vectra-wasm` (commands + wire), `vectra-export`
(`<g transform>`), `apps/vectra-web` (handles, snapping, overlay math).

**New data structures (sketch, to be pinned in a design doc):**

```rust
// vectra-core
pub struct Transform2D {            // affine, row-major, invertible-checked
    pub xx: f64, pub xy: f64, pub tx: f64,
    pub yx: f64, pub yy: f64, pub ty: f64,
}
pub enum TransformSlot { Translation, Rotation, Scale, Skew }  // slot names for Parameter addressing

pub struct Node {
    /* … existing … */
    pub transform: Transform2D,      // local → parent
    pub parent: Option<ParentRef>,   // Node(NodeId) | Layer(LayerId) | Artboard(ArtboardId)
}
// Commands
Command::SetNodeTransform { id: NodeId, transform: Transform2D }
Command::SetNodeTransformSlot { id: NodeId, slot: TransformSlot, value: ParamValue } // animatable/constrainable
```

**Rules to pin:** world matrix = parent world × local (evaluation order fixed);
hit-testing and overlays invert the same chain the renderer uses (one shared
`world_of(node)` implementation); transforms are animatable and constrainable
slots (`rotation`, `scale_x`, `scale_y`, `skew`, `tx`, `ty`); `Group` gains real
semantics (it was "structural-only, flattening" — that decision is hereby
reversed).

**Test gates:** (a) transform-chain proptest — three nested groups, compare world
point against closed form, and against `Document → EvaluatedScene` output;
(b) hit-test-through-transform proptest — a pointer that hits a rotated node's
pixel hits the node; (c) undo law — transform edits are exactly invertible;
(d) export law — `world_matrix` appears as a `<g transform>` and round-trips
through the Phase-1 importer (1.8); (e) constraint law — a rotation slot can be
constrained and solved.

### 1.2 Constraint UX & vocabulary (L)

**Crates:** `vectra-core` (kinds), `vectra-constraints` (rows, nonlinear pass),
`vectra-render` (glyph layer), `vectra-wasm` (commands + solver report),
`apps/vectra-web` (glyphs, panels, n-ary selection), `vectra-export` (warn on
unexportable solver state).

**New data structures:**

```rust
pub enum ConstraintKind {                 // + the new families
    Coincident, Horizontal, Vertical, Parallel, Perpendicular,
    EqualLength, Distance, Angle,
    Tangent,        // circle/arc ↔ line/arc tangency
    Concentric,     // shared centre
    Collinear,      // three points on one line
    Midpoint,       // point is the midpoint of two others
    Symmetric,      // two targets mirror about an axis
    EqualRadius,    // magnitude equality on radii
    Fix,            // pin a slot (a Required Distance with value)
}
pub enum ConstraintOperand { Point(AnchorRef), Edge(EdgeRef), Slot(ConstraintTarget) }
pub struct NonlinearSolve { pub residual: f64, pub iterations: usize, pub jacobian: Sparse } // nalgebra
pub trait ConstraintSolver {
    fn solve_linear(&mut self, system: &RowSystem) -> SolveOutcome;
    fn solve_nonlinear(&mut self, system: &NonlinearSystem) -> Result<SolveOutcome, SolverError>;
}
```

**Design decisions to pin:** the nonlinear pass solves *only* the residuals the
linear pass cannot express (tangency, true parallel/perpendicular, symmetry),
iterating with the linear solution as the initial guess; failure falls back to
the best linear solution **plus a typed diagnostic** — never a silent snap.
Constraint glyphs are engine-computed: the solver already knows each constraint's
targets and satisfaction state, so `RenderScene` gains a glyph draw list keyed to
those points (badge shapes per kind: ⟂ ‖ = ∥ ⌒ ◯ etc.), and the UI gets
"Show constraints", click-to-select, hover-highlight, and drag-a-glyph-to-edit.

**Test gates:** tangency law (circle stays tangent to a moving line to 1e-6 over
a random drag sequence); symmetry law (order of targets irrelevant, residual →
0); nonlinear fallback law (unsolvable ⇒ typed diagnostic + geometry unchanged);
glyph-placement law (every glyph anchor coincides with its target's evaluated
position); n-ary UI law (three selected nodes → n-ary constraint builder);
solver perf gate ≤2 ms for a 200-constraint system *(matches MES §18 tier 1)*.

### 1.3 Smart Pen & Direct Selection polish (M)

**Crates:** `vectra-draw` (fitting, snapping), `vectra-constraints` (snap →
constraint), `vectra-wasm` (draw surface), `apps/vectra-web` (cursor states,
guides).

Add: snapping providers in Rust (grid, pixel, point, edge, tangent, artboard,
guides) with a ranked candidate list crossing the boundary; join/split paths;
simplify path; live fit preview (currently fit happens only at stroke end — keep
that for shape stability, but add an *approximate* live preview from the same
fitter); an eraser (vector) and erase-to-split; constraint-snapping (hold a
modifier → the snap creates a real constraint, not a one-off move); and
"close-path"/"reverse"/"set-start-point" path verbs.

**Test gates:** every snap target law (snapped point lies within ε of the target
for random geometry); fit-quality law (max deviation ≤ tolerance across random
strokes); erase law (erase ⇒ union of remaining pieces ≡ source minus eraser
region); constraint-snap law (a snapped drag leaves the solver satisfied).

### 1.4 Boolean containers & Pathfinder (L)

**Crates:** `vectra-core` (`OperationKind`), `vectra-operations` (n-ary,
chamfer, expand), `vectra-geometry` (curve refit), `vectra-wasm`, UI panel.

```rust
pub enum OperationKind {
    Boolean { op: BooleanOp, inputs: Vec<NodeId> },     // n-ary, order = operand stack
    Offset { distance: Parameter<f64>, joins: JoinStyle },
    Fillet { radius: Parameter<f64>, corners: CornerMask }, // incl. reflex & concave
    Chamfer { size: Parameter<f64>, corners: CornerMask },
    Mirror { axis: MirrorAxis },
    Expand { target: NodeId },                          // stroke → fill (Outline Stroke)
    Divide { inputs: Vec<NodeId> },                     // Pathfinder Divide
    Trim   { inputs: Vec<NodeId> },
    Merge  { inputs: Vec<NodeId> },
    SmartFill { seed: (f64, f64), boundaries: Vec<NodeId> },
}
```

Pin: an **operation may take another operation's output** (removing the
documented nested-union limitation) while the cycle gate keeps the graph
acyclic — the prospective/`dry_run` machinery generalises; results are refit to
cubics at a tolerance with a `max_deviation` field in the diagnostics; fill rule
becomes an explicit per-node property (`EvenOdd | NonZero`).

**Test gates:** n-ary area identities; associativity-law (union over a
permutation of inputs yields equal area within ε); refit-error law (fitted path
stays within tolerance of the polygonised source); chained-op law (an op fed by
an op evaluates, dirties correctly, and undoes as one entry); reflex-fillet law
(concave corner radius preserved); boolean-on-open-path law (typed refusal, not a
panic); fill-rule law (nonzero nested rings do not become holes).

### 1.5 Smart Fill hardening (M)

**Crates:** `vectra-geometry` (regions), `vectra-operations`, `vectra-wasm`
(offload), `apps/vectra-web`.

Add: **gap tolerance** (close near-miss gaps ≤ ε, with a visible "gaps closed"
overlay and a diagnostic); region caching keyed by source revision with a
perf gate; **off-main-thread evaluation** for the >50 ms case (Web Worker or a
`wasm` worker-pool entry point — MES §18's third tier is currently unimplemented);
face geometry preserved as curves (no more polyline faces) so Smart Fill output
is exportable at quality; and a per-face hover chrome that distinguishes
"closed face" from "no face here".

**Test gates:** gap law (two paths with a gap ≤ ε produce the same faces as
exactly-closed paths; gap > ε produces none); cache-invalidation law (moving a
boundary invalidates exactly the affected faces); perf gate (20 sources ≤ 16 ms
incremental on the reference machine); worker law (offloaded and inline results
are byte-identical).

### 1.6 Clipping masks & compositing depth (M)

**Crates:** `vectra-core` (mask refs, blend modes), `vectra-render` (W3C blend
shaders + mask passes), `vectra-operations` (clip), `vectra-export`
(`<clipPath>`/`<mask>`), UI.

```rust
pub struct MaskRef { pub node: NodeId, pub kind: MaskKind, pub invert: bool, pub feather: f32 }
pub enum MaskKind { Alpha, Luminance }
pub enum BlendMode { Normal, Multiply, Screen, Overlay, Darken, Lighten, ColorDodge,
                     ColorBurn, HardLight, SoftLight, Difference, Exclusion,
                     Hue, Saturation, Color, Luminosity }   // W3C compositing set
```

Group semantics: `GroupIsolation { Auto, Isolate, PassThrough }`; per-node mask
references; groups get masks and blend modes; export emits `<mask>`/`<clipPath>`
where the geometry is a clip and `<g style="mix-blend-mode:…">` otherwise with a
warning when SVG cannot express it.

**Test gates:** per-mode blend law (CPU reference implementation vs GPU golden
values, all 16 modes); mask invert/feather law; group-mask law (child outside
the mask is invisible, child inside is unchanged); pass-through law (a
pass-through group's children blend against the backdrop); export-fidelity law
(mask round-trips through the importer).

### 1.7 Gradient system (M)

**Crates:** `vectra-core` (`Paint`), `vectra-geometry` (paint resolution),
`vectra-render` (shader ramps), `vectra-export`, UI.

Add: `Conic { center, angle, stops }`; gradient strokes (a paint applied *along*
the stroke — per-vertex ramp coordinates in the tessellator); a proper gradient
editor (multi-stop, drag position, per-stop colour+alpha, reverse, distribute);
on-canvas gradient handles (like the pen handles), and a gradient tool;
**interpolation in linear-light** with a per-paint `interpolation:
Srgb | Linear | Oklab` field (Oklab lands in Phase 3, but the field must exist
now or every saved file needs migration).

**Test gates:** stop-lerp law (linear-space midpoint ≠ sRGB midpoint for known
pairs — i.e. the bug is provably fixed); gradient-handle law (handles ≡ document
frame); export law (`<linearGradient>`/`<radialGradient>` stop-for-stop;
conic → typed warning + documented fallback); stroke-gradient law (colour at
stroke arc-length t equals the ramp at t).

### 1.8 SVG import/export fidelity (L) — **new crate `vectra-import`**

**New crate:** `vectra-import` (parse SVG XML → `Command` plan → engine), with a
`FidelityReport` (imported / approximated / dropped, per element). Scope: paths
(with `A`/`C`/`Q`/`S`/`T`/`H`/`V` normalisation into `PathSegment`s and a new
`PathSegment::EllipticalArc` so arcs survive), rect/circle/ellipse/line/polygon/
polyline, groups + `transform` (rotate/scale/translate/matrix/skew), styles
(fill/stroke/width/opacity/dash → v1: solid + warn), linear/radial gradients,
`<use>` resolved within the document, `<clipPath>`/`<mask>` → mask refs,
`<text>` → text node with font-fallback diagnostic (or outline-as-path mode),
plus the `data-vectra-*` fast path for Vectra-authored files (a true round trip).
Export gains: optional `<text>` emission with embedded/subset font, `<mask>`/
`<clipPath>` emission, `color-interpolation` hints, and an "editable SVG" mode
vs "picture SVG" mode.

**Test gates:** round-trip law (random document → SVG → import → structural
equality of ids/kinds/slots within ε); property generator for random SVGs
(shapes × transforms × gradients × nesting) parsed without panics; fidelity
report law (every dropped feature appears in the report); id-stability law
(`data-vectra-node` ids survive the round trip).

### 1.9 PNG export (S) — *pulled forward out of Phase 2*

Phase 2 owns the raster *engine*, but a design tool must be able to give a
designer a PNG on day one. Add a minimal readback path: render to a texture →
`copy_texture_to_buffer` → PNG encode (a small pure-Rust encoder) at 1×/2×/3×,
with artboard scope and a transparent option. This is deliberately the *only*
raster capability in Phase 1 and it is what makes the phase shippable to
stakeholders.

**Test gate:** encode/decode round-trip law; deterministic output for a fixed
scene; a golden-image test for a reference artboard.

### Phase 1 kickoff order (first two weeks, concrete)

1. **Hygiene PR (1 day):** LICENSE files, `.gitignore`, CI workflow (fmt · clippy ·
   tests · wasm build · UI tests · smoke), move `TASK-*.md` into `docs/tasks/`,
   add `docs/{ROADMAP,FEATURE_MATRIX,ENGINE_STATUS,ARCHITECTURE}.md` seeded from
   this audit. *Nothing else in Phase 1 is verifiable until this exists.*
2. **Transform design doc (2 days):** write the ADR for workstream 1.1 —
   the affine model, slot naming, world-matrix ownership, migration of existing
   absolute-coordinate documents, and the compatibility rule
   (`Transform2D::IDENTITY` reproduces today's behaviour exactly).
3. **Transform implementation (the rest of the sprint):** core → evaluation →
   render → hit test → export, with the four laws of 1.1 green before any
   feature work starts on top of it.
4. In parallel (no transform dependency): **1.9 PNG export** and the
   **FEATURE_MATRIX seed** (54 rows from Part I/II) so progress is measurable
   from day one.

### Phase 1 exit criteria (all must be true)

1. A designer authors a logo: boolean shapes (n-ary, chained), a gradient stroke,
   clipped layers, text on a path, constraints holding a proportion — with no
   hand-edited JSON.
2. The document saves (`.vectra` v1 **or** v2 if the container lands here),
   exports to SVG, and **imports back** with a fidelity report and no structural
   loss.
3. `cargo test --workspace` grows by the new laws with zero regressions;
   `npm run test:ui` and `npm run smoke` cover the new UI paths.
4. **CI exists** and runs fmt + clippy(-D warnings) + tests + UI + smoke + wasm
   build on every push (§3.2 — this is a Phase-1 deliverable, not hygiene).
5. `FEATURE_MATRIX.md` exists and every Phase-1 row cites a passing test.

---

## Phase 2 — Procreate Raster Engine

**Goal:** a painter can open a Vectra document and paint with pressure and tilt,
using textures, blend modes, alpha lock and selections, at 60 fps — and export
the result as a PNG/JPEG at any scale. This phase is the largest by far; it is a
second product surface inside the same document graph (that is the entire point
of UDG: raster is another node kind, not another app).

### 2.1 New crate: `vectra-raster` (L+)

The **pixel core**, no GPU, no wasm — pure Rust with tests:

```rust
pub struct TileId(u32);
pub const TILE: u32 = 256;
pub struct TileMap { size: (u32, u32), tiles: HashMap<TileId, Tile>, dirty: DirtyRects }
pub struct Tile { pub pixels: Vec<u8>, pub revision: u64 }        // RGBA8 → RGBA16F later
pub struct RasterLayer { pub tiles: TileMap, pub blend: BlendMode, pub opacity: f32,
                         pub alpha_locked: bool, pub clip: Option<MaskRef> }
pub struct BrushSpec {
    pub size: f32, pub flow: f32, pub opacity: f32, pub hardness: f32, pub spacing: f32,
    pub jitter: Jitter, pub scatter: f32, pub angle: Dynamics, pub roundness: Dynamics,
    pub size_dynamics: Dynamics, pub flow_dynamics: Dynamics,      // ← pressure/tilt curves
    pub grain: Option<ResourceId>, pub grain_scale: f32, pub blend: BrushBlend,
}
pub enum Dynamics { Off, Pressure, Tilt, Azimuth, Velocity, Random }   // + a curve
pub struct Stroke { dabs: Vec<Dab>, pub layer: NodeId, pub tile_patches: Vec<TilePatch> }
pub struct Selection { pub kind: SelectionKind, pub mask: TileMap }    // rect/lasso/wand + feather
```

Also: **undo as tile patches** (compress the pre-stroke tiles; a stroke's undo
must be ≤16 ms), **selection ops** (union/subtract/invert/feather/grow), brush
presets as resources, smudge/blend/eraser tools sharing the dab pipeline, and
grain/texture resources (a `ResourceId` into the document's resource table).

**Tests:** dab-accumulation law (stroke replay is deterministic and equals the
live result); tile-patch law (undo restores byte-identical pixels); selection
algebra laws; brush-curve law (pressure→size mapping monotone within range);
grain determinism (same seed ⇒ same texture); memory-budget test (a 4096² layer
with 30 % coverage stays under the tile budget).

### 2.2 Compositing & GPU (L)

**Crates:** `vectra-render` gains a raster pass; new `src/raster_gpu.rs`.

Tile atlas textures; a dab pass (instanced quad per dab, brush shader with
hardness falloff, grain sampling, per-dab blend); layer compositing in the same
frame as vector draws, honouring the shared `BlendMode` set from Phase 1.6; MSAA
/ supersampling options (today: `MultisampleState::default()`, i.e. none);
render-to-texture isolation for groups; cross-backend determinism tests where
possible, golden-image tests where not.

**Perf gates:** 60 fps painting on a 4096² canvas at brush sizes ≤ 512 px on the
reference GPU; ≤2 ms per frame of compositing for 8 layers; no full-canvas
uploads (tile streaming only — assert via the `BufferSink`/GPU stats).

### 2.3 WASM boundary changes (M)

- **Binary channel for pixels.** Tiles cross as `Uint8Array` (or stay GPU-side
  entirely, which is preferred: the host uploads once and never round-trips).
  JSON keeps metadata/commands; pixel data never rides a JSON string.
- New objects: `RasterEngine`, `BrushSettings`, `Selection`; new methods:
  `raster_pointer(phase, x, y, pressure, tilt, azimuth, time)`,
  `raster_commit_stroke() -> TileDirtySummary`, `raster_load_resource`,
  `raster_selection_*`, `raster_export_png(scale) -> Uint8Array`.
- **Event ingestion at pointer rate** (pressure/tilt/azimuth/timestamp) must not
  allocate per event; the boundary gains a compact binary event struct.
- `EvaluatedScene` gains `EvaluatedPrimitive::Raster { resource, transform,
  blend, opacity, mask }`; `EvaluationContext` gains a `RasterProvider` trait
  (the fourth injected evaluator, same law as the other three).

### 2.4 Document container v2 (M)

`.vectra` becomes a chunked container: header + chunk table + JSON document
chunk + **binary resource chunks** (raster tiles, grain textures, fonts),
each independently compressed (gzip now, zstd later). Version bump + migration
from v1 (`vectra-file` owns both directions, tested by a v1→v2→v2 round trip).
`Document.resources: HashMap<ResourceId, ResourceRef>` with content hashes so
saves are still deterministic.

### 2.5 UI: the paint surface (L)

Brushes palette (presets, size/opacity/flow sliders, pressure/tilt curve
editors), layers panel gains raster semantics (alpha lock, clipping, blend),
selection tools + marching-ants overlay, transform-selection tool, colour picker
with a *real* picker (the browser's `<input type="color">` is not one), a canvas
that can go full-screen, and the touch/gesture recogniser extended for
two-finger paint vs pan vs zoom.

**Gate shape for Phase 2:** every raster law green; golden images for
compositing; the 60 fps budget asserted in a benchmark that CI runs on a
reference device or records as a tracked artifact; PNG/JPEG export green; and a
smoke run that paints a stroke, undoes it, and compares pixels.

---

## Phase 3 — Unified Design System

**Goal:** one document that carries vectors, raster, text, components and
variables; a UI/product designer can build a component library with variants and
themes, and the code export reflects it.

### 3.1 One graph (L) — the UDG landing

- `Node { id, kind, transform, parent, children, paint, mask, clip, visible,
  locked }`; **layers and artboards become node kinds** (`LayerKind`, `Frame`),
  so `LayerRegistry`/`ArtboardRegistry` collapse into the node table (migrating
  every command that addressed them).
- One ordered child list per parent replaces `Document.order` + layer lists +
  `child_parent`. z-order is `parent.children`.
- A migration replayer takes v1/v2 documents to v3 through **commands** (never by
  writing fields behind the engine's back — the rule `vectra-file` already
  follows).
- `RasterLayer` nodes join the same graph with masks/clips/blends.

**Test gates:** graph-invariant proptests (every node reachable exactly once;
parent/child symmetric; order total and stable); migration laws (v1 → v3 →
re-save is byte-stable; a document with layers, groups, ops, motion, procedural,
components and raster survives); the whole existing suite green with only
mechanical changes (this is the phase's real acceptance test).

### 3.2 Components v2: variants, overrides, libraries (L)

```rust
pub struct VariantSet { pub axes: Vec<VariantAxis>, pub combinations: HashMap<…, ComponentId> }
pub struct VariantAxis { pub name: String, pub values: Vec<VariantValue>, pub default: VariantValue }
pub struct Override { pub instance: NodeId, pub path: PropPath, pub value: PropValue, pub locked: bool }
pub struct ComponentLibrary { pub source: DocumentRef, pub components: Vec<ComponentId>, pub version: Hash }
```

Instance-in-place editing, nested instances, swap-instance, detach, prop
defaults, boolean props, slot props, overrides with reset, and libraries
(external document references with content-hash pinning).

### 3.3 Variables v2 & modes (M)

Typed variables (`Scalar | Color | String | Bool | Alias`), collections, modes
(Light/Dark/Density…), variable-driven component props, expression support for
strings/colours, and token export (CSS custom properties, Tailwind config,
JSON).

### 3.4 Auto-layout (L)

Stack and grid layout containers as a first-class node behaviour (padding, gap,
alignment, distribution, wrap), with layout constraints integrated with the
solver (the solver already handles linear systems — this is where constraints
earn their keep) and a layout inspector.

### 3.5 Colour management (L)

f32 linear working space end-to-end; sRGB and Display-P3 output; OKLab/OKLCH
editing and interpolation; ICC profile import for the output transform;
premultiplied alpha; dithering; colour variables and palettes as resources;
gamut warnings; export carries `color-interpolation` and PNG iCCP.

**Colour test gates:** round-trip laws (sRGB ↔ P3 ↔ OKLab within ΔE tolerance);
monotonicity of OKLCH interpolation; gradient midpoint golden values; and a
"no regression on legacy `u8` documents" law (v1 colours land on the same
pixels).

### 3.6 Typography v2 (L)

Variable fonts (fvar axes + named instances as parameters), OpenType feature
toggles (`smcp`, `onum`, `ssXX`, `frac`, `liga`), font weight/style matching
within a family, fallback chains per script, bidi + vertical text, area text with
wrapping/justification/hyphenation, text styles as resources, decorations, and
an in-canvas caret with IME.

### 3.7 Procedural expansion (M)

Radial repeat, along-path repeat, symmetry (mirror/rotational), scatter, and
node-graph instancing (repeat *nodes*, not regions); a node-graph canvas UI; and
procedural output feeding raster (noise → grain textures) once Phase 2 exists.

### Phase 3 exit criteria

A component library with 2-axis variants and a light/dark mode pair drives a small
UI screen (auto-layout, text styles, colour-managed gradients, a raster texture);
the same document exports SVG, PNG at 3 scales, and a parametric React component
with typed props and variant selection; the v1→v3 migration is green.

---

## Phase 4 — The Zoah Advantage

**Goal:** the engine does work the designer would otherwise do by hand, and the
code export is good enough to be the handoff.

### 4.1 Real AI, wired honestly (L)

- **Provider seam, not provider code:** `ChatPlanner` gains a concrete
  implementation behind a host-injected HTTP provider (keys live in the desktop
  host / a server, never in wasm or the document). Streaming responses; tool-use
  loop against the command schema; plan editing (accept/reject/modify individual
  commands); an eval harness (a corpus of prompts → expected plans, run in CI)
  so prompt/regression drift is measurable.
- **AI-assisted constraint detection** (the headline): geometric inference over
  the document — near-coincident anchors, near-parallel/perpendicular edges, near-
  equal lengths, collinear runs, concentric centres, tangent circles — ranked by
  confidence, shown as suggestions, accepted as *real* constraints (which is why
  Phase 1.2 must land the vocabulary first).
- **Procedural macros:** "make 4 colour variations", "generate an icon set",
  "align perfectly" become *generated plans* over the real procedural graph and
  constraint solver (not five hard-coded prompts), with preview and undo.
- **AI self-correction maturity:** corrections become a typed taxonomy (wrong id,
  wrong kind, type mismatch, over-constrained, cycle) with targeted re-prompting
  per class, and a bounded budget per intent.

### 4.2 Code export polish (M)

Design tokens → CSS variables + Tailwind; React export with variant maps,
boolean props, slots and style objects; Vue/Svelte exporters as new IR consumers
(the IR was designed for exactly this); SVG → JSX transformer; an asset pipeline
(embedded raster, sprite sheets); and a CLI (`vectra export --format react`)
so the export is CI-usable.

### 4.3 Performance & instrumentation (M)

`criterion` benches for the MES §18 tiers (they exist as claims in reports and
nowhere in code); the >50 ms background tier implemented (worker pool);
perf budgets enforced in CI as tracked numbers; and an in-app perf HUD for the
cases users feel.

### 4.4 Interop expansion (M)

PDF export (print handoff), raster import (PNG/JPEG → image nodes), and
DXF/OpenType/Webfont packaging as demand dictates.

---

## Sequencing, dependencies and the risk register

**The critical path is one item long: transforms (1.1).** Everything else in
Phase 1 either stands on it or ships beside it:

```text
1.1 transforms ──┬─▶ 1.3 smart pen & selection (handles live in local space)
                 ├─▶ 1.4 boolean containers (operands gain transforms)
                 ├─▶ 1.6 masks & compositing (mask refs need placement)
                 ├─▶ 1.8 SVG import (transform attributes are half of SVG)
                 └─▶ 1.2 constraint glyphs (badges must be placed in world space)
1.2 constraints (vocabulary + nonlinear)  ⇄  1.1 (transform slots are constrainable)
1.5 Smart Fill hardening, 1.7 gradients, 1.9 PNG export — independent, start anytime
```

**Risk register (ranked by expected damage):**

| risk | likelihood | damage | mitigation |
|---|---|---|---|
| Transforms ripple through every test, snapshot, exporter and the renderer's absolute-coordinate assumptions | high | high | identity-transform compatibility rule; update the ~650 tests mechanically under a green CI; land 1.1 as one reviewed change with its own design doc |
| Nonlinear solver fails to converge on adversarial systems | medium | medium | bounded iterations, linear initial guess, typed fallback diagnostic, never a silent move; property-test random tangency systems |
| SVG import scope explodes (filters, patterns, `<use>` chains, CSS styles) | high | medium | staged element coverage behind a `FidelityReport`; "unsupported → documented drop", never a silent guess; property-generator test per stage |
| Phase 2 (raster) overruns because it is a second engine | high | high | build a **vertical slice first** — one brush, one layer, one blend mode, tile undo, PNG out — behind the Phase-1 gates before the full brush/texture/selection system |
| The wasm boundary grows from 48 items to 100+ and the hand-written TS mirror drifts | high | medium | introduce a typed client generator (or an app-API layer) in Phase 2.3 before adding raster surface |
| UI shell work is invisible to stakeholders ("the engine got better again") | medium | medium | schedule the canvas-first shell milestone at the *end* of Phase 1 as a demo-able gate, not a trailing cleanup |
| Docs drift again the moment the audit ends | high | medium | the four documents + their CI checks (§14) are Phase-1 deliverables, not follow-ups |

**What to cut first if the phase overruns** (in order): conic gradients; the
`<text>`-in-SVG emission (paths export fine); group isolation modes; the extra
Pathfinder verbs (`Trim`/`Merge`) beyond `Divide`; the eraser and simplify verbs
in 1.3. **What may never be cut:** transforms, CI, SVG import, the constraint
vocabulary, and PNG export — those are the phase's identity.

**Decisions that need ratification before coding** (each is an ADR in
`docs/ARCHITECTURE.md`): (a) the UDG definition in §4; (b) transforms live in
`vectra-core` and are evaluated in geometry/render, never in the UI;
(c) operations may consume other operations' outputs (1.4); (d) masks are node
references, not materialised geometry (1.6); (e) whether the container v2 lands
in Phase 1 or Phase 2 (only Phase 2 *needs* it, but doing it early avoids a
second migration).

---

# PART IV — RULE 4: DOCUMENTATION RECONCILIATION

## 12. `README.md` vs the code — every discrepancy

| # | README says | Code says | Severity |
|---|---|---|---|
| R1 | **"70 tests green workspace-wide"** | 650 `#[test]` + 28 proptest blocks (Rust), 146 UI tests, 79 smoke steps; Task 12.0's gate: 650/146/79 | **critical** — off by ~9×, and it is the first number a new engineer reads |
| R2 | Repository layout lists 10 crates | 14 workspace members: `vectra-draw`, `vectra-dependency`, `vectra-ai`, `vectra-file` are missing | high |
| R3 | Status glyphs: geometry ✅, constraints/expression/operations/procedural/motion/render/export ◌ "scaffolded, planned" | all nine are implemented to varying depth (render: 4,234 LOC + GPU + hit index; export: 1,818 LOC + 764 test LOC) | **critical** — it undersells the product by an order of magnitude |
| R4 | "`vectra-wasm` ◐ wasm-bindgen command-bus API (Task 1.4)" | 48 `#[wasm_bindgen]` items across three modules, plus AI/draw/render/appearance/component surfaces | high |
| R5 | App table: `vectra-desktop` = "◌ Future Tauri shell (reuses vectra-web UI)" | Task 10.0 shipped it: native dialogs, `.vectra` codec, `verify.rs` roundtrip law, release binary | high |
| R6 | Crate dependency DAG diagram | wrong: omits `vectra-draw` (→ constraints), `vectra-dependency` (→ expression, geometry), and lists a `render → export` edge that does not exist; wasm's real fan-in is 12 crates | medium |
| R7 | "Phase-1 task tracker (MES §20)" — tasks 1.1–1.5 only | tasks through 12.0 exist | medium |
| R8 | Build & test documents three `cargo test -p …` invocations | no `tools/check.sh` (offline vendored build), no `npm run test:ui`, no `npm run smoke`, no desktop build instructions | medium |
| R9 | — | missing entirely: LICENSE (declared MIT OR Apache-2.0 in `Cargo.toml`), CI, contribution guide, changelog, the committed-wasm note | medium |

## 13. `MES.md` vs the code — every discrepancy

MES is a *specification*, so drift is expected — but it is currently read as
status. The two tasks that shipped (2.1, 2.2) set the right pattern with inline
"implementation note" blocks; the spec has not been reconciled since.

| # | MES says | Code says | Resolution |
|---|---|---|---|
| M1 | §1 structure: 10 crates; only `vectra-desktop` under `apps/` | 14 crates; `apps/vectra-web` is the primary app; `tests/` contains only a README | rewrite §1 from the workspace file |
| M2 | §3 `Parameter<T>` sketch | matches (adds `InputBinding` detail); `MotionBinding` in §12 is stale | annotate |
| M3 | §4 `Document` sketch | missing `motion`, `procedural`, `layers`, `artboards`, `active_layer` | annotate (this is the *core* artefact of the unified graph) |
| M4 | §5 `NodeKind` sketch: Rectangle/Circle/Arc/Path/Group | **`Text` exists** (the whole of Task 11.0); `Circle` has no ellipse sibling | annotate + add the missing kinds list |
| M5 | §9 `ConstraintSolver` trait with `add_linear` / `solve_nonlinear` / `diagnose` | solver is a struct with plan/apply/solve; **`solve_nonlinear` does not exist**; `ConstraintDiagnostics` is not a type (diagnostics are `DiagnosticCode`s) | rewrite §9 with a Phase-1/Phase-2 split |
| M6 | §10 `OperationNode` sketch (Boolean/Fillet/Offset) | five kinds with `inputs`, `enabled`, `style`; **SmartFill** missing from the sketch | rewrite §10 |
| M7 | §11 `GeometryNode` trait sketch | registry + `ProceduralKind` enum + `nodes::evaluate`; 7 kinds incl. components | rewrite §11 |
| M8 | §12 `MotionBinding` sketch: no anchor on Spring | `from`/`at` are in the document (deliberate, documented) | annotate |
| M9 | §14 `Command` sketch: 5 variants | **62 variants** with exact inverses | replace the sketch with a pointer + a count |
| M10 | §15 WASM boundary: 5 methods | **48 `#[wasm_bindgen]` items**, three objects, string-JSON + (Phase 2) binary | rewrite §15 and state the boundary law explicitly |
| M11 | §18 performance budgets | no measured numbers in MES; achieved figures live only in task reports (e.g. `SetVariable` 37.5 µs @502 nodes) | move to `ENGINE_STATUS.md` and link |
| M12 | §19 MVP acceptance: Phase-0/1 only | the product has shipped through Task 12.0 | replace with a link to `FEATURE_MATRIX.md` |
| M13 | §20 Phase-1 tasks: 1.1–1.5, all ✅ | Tasks 2.1–12.0 exist, undocumented in MES | replace with a link to `ROADMAP.md` |
| M14 | §2 dependency table | `vectra-dependency`/`vectra-draw`/`vectra-ai`/`vectra-file` absent; `wgpu`/`encase` listed but `encase` has no dependent crate (render packs bytes by hand); `naga` is a dev-dep | rewrite from `Cargo.toml` |

**The systemic fix:** MES should stop being a living status board. Its lasting
value is the *doctrine* (§2 law, §3 the `Resolvable` core, §13 the pipeline, §17
the test philosophy, §18 budgets). Everything time-varying moves to the four
documents below; MES keeps an "as-built deltas" appendix listing only doctrinal
changes.

## 14. The four new living documents

**Status: written.** All four now exist in `docs/` as the *seeded first
edition* of the truth layer this audit said the repo was missing — not as
proposals. `docs/ROADMAP.md` (206 lines: Phase 0 closed, Phase 1 workstreams
1.0–1.9 with sizes and dependencies, kickoff order, exit criteria, cut list,
Phases 2–4, critical path, the six pending ADRs, change log);
`docs/FEATURE_MATRIX.md` (283 lines, **151 audited rows** — recount: 58
`IMPLEMENTED` / 13 `PARTIAL` / 80 `MISSING` / 0 `SCAFFOLDED` / 0 `DEPRECATED`
— each row carrying an evidence path, a limits note and a roadmap phase id);
`docs/ENGINE_STATUS.md` (290 lines: the four green gates, the 650/54 test
split, measured build sizes, the debug-wasm finding, the reported-vs-measured
perf verdict, the offline build recipe with its three tooling defects, a
verified-where matrix and a risk register); `docs/ARCHITECTURE.md` (440 lines:
the as-built crate DAG, the as-built data flow including the 7-step mutation
protocol, the target UDG data flow with §4.1's absence table, ten invariants
each with its enforcing test, the wasm boundary surface, the `.vectra`
container, and an ADR index seeded with D1–D6 as `PROPOSED`).

They are cross-referenced and mutually consistent — `ROADMAP.md` §9's six
pending decisions are the ADR rows of `ARCHITECTURE.md` §9, and `FEATURE_MATRIX`
phase ids resolve to `ROADMAP.md` workstream headings. **Three things in the
outlines below are deliberately left as stubs in this first edition**, because
writing them any other way would mean inventing numbers: the `criterion`-driven
performance table and memory budgets of 14.3 (no bench harness exists — see
`ENGINE_STATUS.md` §4 and roadmap 4.3), the platform/toolchain matrix of 14.3
(only one platform has been exercised), and every CI check named below (no CI
exists — roadmap 1.0). What is *not* yet true is therefore the enforcement, not
the content: for now these documents are kept honest by the discipline of their
own update rules rather than by a robot. Wiring `tools/check_docs.py` is
workstream 1.0's first task.

Each has an owner, a review trigger and one CI check that keeps it honest.

### 14.1 `ROADMAP.md` — high-level timeline and phase goals

Contents: the four phases with goal statements and exit criteria; per-workstream
size class and dependencies (the DAG of workstreams, not just a list); the
"definition of done" per phase; what is explicitly *out* of scope per phase; and
a changelog of phase renegotiations (dated, with reasons — the record that
prevents silent scope creep).

**CI check:** every workstream id referenced by `FEATURE_MATRIX.md` exists in
`ROADMAP.md`.

### 14.2 `FEATURE_MATRIX.md` — the IMPLEMENTED/PARTIAL/MISSING table

The matrix is the heartbeat of truth. One row per capability, columns:

| column | content |
|---|---|
| `id` | stable row id (e.g. `vec.smartfill`, `raster.brush.texture`) |
| `capability` | designer-facing sentence ("fill between overlapping paths") |
| `status` | one of the five words, exactly |
| `evidence` | test name(s) and/or file path — **required** for `IMPLEMENTED` |
| `limits` | what the status does *not* promise (for `PARTIAL`/`IMPLEMENTED`) |
| `phase` | the phase that completes it (`—` when finished) |
| `verified` | date + how (CI, manual, static) |

Seeded from Part I/II of this audit (54 rows across the 14 subsystems).

**CI checks:** (a) every `evidence` path exists; (b) every `IMPLEMENTED` row on a
changed crate has a test whose name matches its evidence — the "no status
without a witness" rule.

### 14.3 `ENGINE_STATUS.md` — performance, coverage, limitations

Contents: the MES §18 budget table with **measured** numbers and the exact
reproduction command per row; test-coverage metrics per crate (test count, law
count, untested public functions if measured); memory budgets (tile budgets from
Phase 2, wasm heap ceilings); known limitations as a first-class list (each
entry linking to the `FEATURE_MATRIX` row that carries it); platform/toolchain
matrix (native, wasm32, Tauri; Rust 1.75 floor; wgpu 22; browser support);
and the "how to reproduce every number" section that makes the whole document
auditable.

**CI check:** a `criterion` run publishes JSON; the doc's numbers are updated by
a script that fails if a number is older than N commits without a
re-measurement marker.

### 14.4 `ARCHITECTURE.md` — the UDG data flow and crate map

Contents: the UDG definition (§4 above, with the team's ratification);
the as-built and target crate DAGs; the evaluation data-flow diagram (§6); the
invariants (totality, exact inverses, cycle-freedom, derived topology,
`patch ≡ rebuild`, byte-determinism); the boundary laws (core is the dependency
root and never depends outward; owner crates implement traits and are injected;
wasm is integration only; the renderer never sees a `Document`; the AI only emits
`Command`s); the container format(s); and a decision log (ADR-style: decision,
alternatives, consequences) seeded from the existing `TASK-*-DESIGN.md` docs.

**CI check:** a generated crate-graph (from `cargo metadata`) must match the
diagram's machine-readable block; drift fails the build.

### 14.5 Accompanying hygiene (same PR)

- `LICENSE` + `LICENSE-APACHE`/`LICENSE-MIT` (the manifest already claims them).
- `.gitignore` that excludes built wasm and `dist/`, plus a decision: either
  build wasm in CI (preferred) or keep the committed artifact and add a
  freshness check that fails when `src/` is newer than the binary.
- `.github/workflows/ci.yml`: fmt · clippy `-D warnings` · `cargo test
  --workspace --all-targets` · wasm build · UI typecheck+tests · smoke.
- Move the 28 root-level `TASK-*.md` reports into `docs/tasks/` (they are
  excellent provenance; the root is not a filing cabinet).
- Rewrite `README.md` to ~120 lines: what the product is, the four docs, how to
  build/run/test, and a status line that is *generated from* `FEATURE_MATRIX.md`
  rather than remembered.

---

# PART V — THE CRITICAL PHASE-1 BLOCKERS (SUMMARY)

Ranked by how much of Phase 1 they block. These are the items that must be
resolved *before* Phase 1 features are worth building.

| rank | blocker | status | why it blocks Phase 1 | cheapest unblock |
|---|---|---|---|---|
| 1 | **No transform system** | `MISSING` | every Illustrator workflow assumes groups move, rotate and scale; component scaling, masking, icon sets and (Phase 2) raster placement all inherit this. Retrofitting later multiplies cost across every subsystem | workstream 1.1, done once, before the feature work |
| 2 | **No SVG import** | `MISSING` | the phase's own exit criterion ("hand off an SVG that Vectra can also re-open") is impossible; designers cannot bring work in | new `vectra-import` crate (1.8) |
| 3 | **Constraint vocabulary + no glyphs** | `PARTIAL` | Tangency/Symmetry do not exist; the UI cannot show what is constrained. "Constraint UX" is the phase's first named focus | 1.2 (nonlinear pass + glyph layer) |
| 4 | **No CI, no LICENSE, committed build artifacts** | `MISSING` | 650 tests that run only on request, an unlicensable repo, and a stale-binary hazard — every Phase-1 claim is unverifiable without this | §14.5 (a day of work) |
| 5 | **Boolean limits** (binary, nested-union refused, reflex fillets, polyline results) | `PARTIAL` | Pathfinder-class work is the phase's third focus; chained booleans are how real illustrations are built | 1.4 |
| 6 | **Compositing depth** (4 blend modes; masks are two layer booleans) | `PARTIAL` | clipping masks are the phase's fifth focus; 4 modes and no mask refs cannot express the illustrations the phase targets | 1.6 |
| 7 | **Gradient system** (no conic, no stroke gradients, sRGB-lerp, two-stop editor) | `PARTIAL` | gradients are the phase's sixth focus, and the sRGB interpolation bug silently corrupts output today | 1.7 |
| 8 | **No PNG export** | `MISSING` | a design tool that cannot produce a pixel asset cannot be demoed, handed off or reviewed | 1.9 (small, pulled forward from Phase 2) |
| 9 | **Debug-dashboard UI** | `PARTIAL` | Phase-1 features land in a shell whose default layout is instrumentation; the phase's usability acceptance is judged in this shell | §2.14 items folded into each workstream's UI task + a shell workstream |
| 10 | **AI is a phrase table** | `PARTIAL` | not a Phase-1 blocker *if* product messaging says so; a blocker the moment any AI claim is made | Phase 4.1, with honest labeling until then |

**The single most important sentence in this document:** Vectra's engine is
genuinely good — regions, dependency graph, springs, non-destructive operations
and the command/undo spine are better than most commercial codebases' internals —
but it is a *simulation of the whole product* rather than the product: one
finished subsystem, eleven deep partials, two holes (raster, colour), and a
documentation layer that still describes the first five tasks. Phase 1 is not a
rewrite; it is the disciplined act of turning eleven partials into one coherent,
testable, CI-enforced vector tool — starting with the transform foundation that
everything else has been quietly working around.

---

## Appendix A — Evidence ledger (counts taken from source)

| measure | value | how counted |
|---|---|---|
| Rust `src` lines | 52,021 | `find crates/*/src -name '*.rs' \| xargs wc -l` |
| Rust test lines (`tests/`) | 23,822 | same, `crates/*/tests` |
| `#[test]` attributes | 650 | `grep -rn '#\[test\]' crates/` |
| `proptest!` blocks | 28 | `grep -rn 'proptest!' crates/` |
| UI test cases | 146 | `grep -rn '^\s*\(it\|test\)(' apps/vectra-web/tests` |
| UI test lines | 4,342 | `wc -l apps/vectra-web/tests/*` |
| Web app lines (ts/tsx, excl. wasm) | 14,016 | `find … -not -path '*/wasm/*'` |
| `Command` variants | 62 | parsed from `command.rs` enum body |
| `#[wasm_bindgen]` items | 48 | `lib.rs` 37 + `draw.rs` 2 + `render.rs` 9 |
| Blend modes | 4 | `vectra-core/src/style.rs` |
| Constraint kinds | 8 | `vectra-core/src/constraint.rs` |
| Procedural kinds | 7 | `vectra-core/src/procedural.rs` |
| Node kinds | 6 | `vectra-core/src/document.rs` |
| Rust crate members | 14 | workspace `Cargo.toml` |
| CI workflows | 0 | no `.github/` |
| LICENSE files | 0 | repo root |

Historical gates (quoted from reports; the final state was re-measured by this
audit at **650 Rust / 146 UI / 79 smoke, all green**): 170 (3.1) → 181 (3.2) → 390 (8.0) →
434 (9.0) → 423+89 (10.2) → 537 (10.3) → 547 (10.4) → 549 (10.5) → 576 (10.6) →
589 (10.7) → 633 (11.0) → **650 Rust / 146 UI / 79 smoke (12.0)**.

## Appendix B — What this audit verified, and what it could not

**Verified by execution in this audit (2026-10-07):**

| check | result |
|---|---|
| `cargo test --workspace --lib --tests --no-fail-fast` | **650 passed / 0 failed**, 54 targets |
| `cargo clippy --workspace --all-targets -- -D warnings` | clean, 0 warnings |
| `cargo fmt --all -- --check` | clean |
| `npm run typecheck` (web) | clean |
| `npm run test:ui` | **146 tests, 146 pass, 0 fail** |
| `npm run smoke` | **79 steps, SMOKE PASS** |
| static counts (`#[test]` 650 · `proptest!` 28 · UI 146) | match the suite results exactly |

**Not verified (and therefore not claimed):**

1. Doc-tests (`--doc`) — the bundled toolchain has no `rustdoc`.
2. GPU behaviour: no device, so WGSL compiles and the render laws run headless,
   but no pixels were rendered, and hit-testing/overlay behaviour was not seen.
3. Perf numbers from the reports (37.5 µs `SetVariable`, 1.20 µs propagation,
   190.7 µs patch vs 878.1 µs rebuild) — *(reported)*.
4. The desktop app's native dialogs and `.vectra` file round-trip on disk (no
   display, no Tauri build here) — though `vectra-file`'s own laws ran.
5. Whether the committed wasm binary equals the current Rust sources (the
   `STALE_ENGINE` probe exists; hashes were not diffed).
6. Anything about behaviour on a real tablet/pointer device (pressure, tilt,
   touch gestures).

Anything in this document not backed by a file path, a source count, a measured
run or a quoted report line should be treated as a judgement call — and judged
accordingly.

## Appendix C — Phase gates as executable commands (to be wired into CI)

```bash
# Rust (offline, vendored — the repo's own tooling).
# NOTE: `tools/check.sh` currently appends `--offline` AFTER the caller's args,
# which breaks `fmt` and `clippy … -- -D warnings`; until that one-line fix
# lands, set the environment and call cargo directly (exactly what was run for
# this audit, and what CI should run):
export PATH=/tmp/rust/prefix/bin:$PATH CARGO_HOME=/tmp/cargohome PYTHONPATH=/tmp/pylibs
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --offline -j 2 -- -D warnings
cargo test --workspace --lib --tests --offline --no-fail-fast -j 2

# WASM + web
bash apps/vectra-web/scripts/build-wasm.sh
cd apps/vectra-web && npm ci && npm run typecheck && npm run test:ui && npm run smoke && npm run build

# Desktop
cd apps/vectra-desktop && npm ci && npm run typecheck && npm run tauri:build   # release profile

# Documentation drift (added in Phase 1, §14)
python3 tools/check_docs.py     # FEATURE_MATRIX evidence paths, crate DAG vs cargo metadata, perf-age check
```

Reference results from this audit's run of the Rust and web gates:
**650 Rust tests / 0 failed · clippy clean · fmt clean · 146 UI tests / 0 failed ·
79 smoke steps PASS.**
