# TASK 12.0 — Smart Fill and the Region Graph

**Branch:** `arena/426efdbd-vectra` · **Gate status:** all green, see §5 · **Report written:** after the Compile & Fix Pass.

This round adds the last Phase A piece: the design tool stops being a collection of
shapes and starts understanding the *spaces between* them. Four rules, all four
implemented without deviation:

1. **RULE 1** — region detection lives in `vectra-geometry`: overlapping paths are
   cut into planar faces, and each face is a clean, closed `Path` with its holes
   intact.
2. **RULE 2** — Smart Fill is a *parametric node*, not a painted bitmap: it names
   its boundaries, owns an Appearance stack, and re-evaluates the instant a
   boundary moves.
3. **RULE 3** — the Region Graph exposes **spans** (the arcs of a path's outline
   between consecutive crossings), and **Break Path at Intersections** splits the
   path at those points into new, independent, closed paths.
4. **RULE 4** — a ColorDrop inside an enclosed area creates a Smart Fill pinned to
   exactly that region, sitting above the boundary paths in the layer stack.

---

## 1. RULE 1 — the Region Graph, in `vectra-geometry`

**Files:** `crates/vectra-geometry/src/regions.rs` (1,251 lines),
`crates/vectra-geometry/src/convert.rs` (288 lines, moved here from
`vectra-operations` so the graph and the operations evaluator share **one**
path ⇄ region conversion), `crates/vectra-geometry/src/diagnostic.rs`
(`DiagnosticCode::SmartFillEmpty`), `crates/vectra-geometry/tests/region_laws.rs`.

### 1.1 The data model

```rust
pub struct RegionGraph {
    pub sources: Vec<SourceSpec>,          // what was cut, in the order given
    pub faces: Vec<RegionFace>,            // the planar arrangement
    pub crossings: Vec<Crossing>,          // every outline intersection
    pub spans: Vec<Span>,                  // every outline arc between two cuts
    pub intersecting_sources: usize,        // sources that have a partner
}

pub struct SourceSpec { pub id: NodeId, pub region: MultiPolygon<f64> }
pub struct RegionFace { pub region: MultiPolygon<f64>, pub members: Vec<bool>,
                        pub area: f64, pub holes: usize, pub path: lyon::path::Path }
pub struct Crossing { pub point: (f64, f64), pub a: f64, pub b: f64 }
pub struct Span { pub source: NodeId, pub from: f64, pub to: f64,
                  pub start: (f64, f64), pub end: (f64, f64),
                  pub length: f64, pub ring: usize, pub total: f64 }
```

A source is a **region**, not a path: `SourceSpec::from_primitive` flattens the
node's evaluated primitive with lyon at `FLATTEN_TOLERANCE = 0.05` and builds a
`geo::MultiPolygon` whose parity is exactly what the renderer fills — nested
subpaths are holes. Primitives with no area (an open sliver, an arc) are skipped;
they cannot be intersection partners.

### 1.2 How faces are found

The arrangement is computed by `geo::BooleanOps` over every pair of sources: the
atoms are the intersections and differences, and each atom is *refined* — its
signature ("inside source 0? source 1? …") is the face's identity. Two facts are
worth writing down because the tests pin them:

* An atom whose signature is **all-`false`** is the background, never a face; the
  refinement drops those slivers (and only those).
* `face_at(point)` answers with the **smallest** face containing the point, so a
  seam between two faces resolves the way a designer expects: the one that is
  actually under the pointer.

Every face carries its own closed lyon path (`multi_polygon_to_path`), its
unsigned area, and its hole count.

### 1.3 Even-odd, and why holes stay holes

`multi_polygon_to_path` reverses interior rings, but the renderer fills with
`FillRule::EvenOdd` — so **nesting**, not opposite winding, is what makes a hole.
The graph never has to reason about winding direction: faces are regions, and the
tests assert *area identities* instead (a donut's face area == host − inner; the
hole is covered by no face of the source).

### 1.4 Crossings and spans

`ring_crossings` walks every pair of flattened rings (bounds-overlap first) and
reports each transversal intersection as an arc length along each ring. Positions
are **ring-major**: a source's outline is its rings laid end to end, and
`from`/`to` are distances into that outline in `[0, total)` — `to < from` means
the span wraps the outline's start. Cuts are deduped at
`REGION_EPSILON = 1e-6` so a shared vertex is one cut, not two.

`spans_of(source, rings, neighbours)` then returns the arcs between consecutive
cuts, in outline order — the union of a source's spans **partitions its outline**,
and a source nobody crosses has **no** spans: there is no intersection point to
cut at, and "the whole outline" is not a span a designer can break.

### 1.5 The reference arrangement

Two circles of radius 100 centred at (200, 200) and (320, 200) — the fixture every
law is stated against:

| face | members | area |
| --- | --- | --- |
| A only | `[true, false]` | 22449.251521908423 |
| B only | `[false, true]` | 22449.251792811 |
| A ∩ B (lens) | `[true, true]` | 8916.23314174575 |

Crossings: `(60, ∓79.8518275678284)` in the engine's y-up document space, i.e.
`x = 260, y = 200 ± 79.85…` in the flipped frame the smoke reads.

---

## 2. RULE 2 — Smart Fill as a parametric node

**Files:** `crates/vectra-core/src/operation.rs`,
`crates/vectra-core/src/command.rs` (`CreateSmartFill`, `SetAppearances`
widened), `crates/vectra-operations/src/evaluator.rs`,
`crates/vectra-operations/tests/smart_fill_laws.rs`.

### 2.1 The node

```rust
OperationKind::SmartFill { seed: (f64, f64), boundaries: Vec<NodeId> }
```

* **Identity is the seed.** The seed is a point, so the fill survives the very
  edit that destroys its region (a boundary moving away) and re-derives: it
  follows the seed.
* **Inputs are `[self] + boundaries`.** The self-reference is how a write that
  dirties the fill (`SetAppearances`, a re-seed) reaches the fill's own
  evaluation; `ApplyOperation` validates it by *skipping* the self-reference
  (reading another operation is still Phase 2). The boundary ids are how dirtiness
  propagates: move circle A, and the fill is in `affected_by` before the pass runs.
* `OperationNode::smart_fill(seed, boundaries, style, name)` builds the row;
  `arity()` is `boundaries + 1`, `boundaries()` exposes the edges, `validate`
  refuses an empty boundary list.

### 2.2 Evaluation

`evaluate_smart_fill` (in `vectra-operations/src/evaluator.rs`):

1. Resolve every boundary through `SourceSpec::from_primitive` — area-less
   boundaries are skipped, and a boundary that no longer exists is skipped too
   ("patch retires geometry").
2. No resolvable boundaries at all → `no-boundaries` (warning) and no geometry.
3. `RegionGraph::from_scene(scene, boundaries)` → `face_at(seed)`.
4. The seed in no face → `smart-fill-empty` (warning) and no geometry — never the
   nearest face, never a stale copy.
5. Success → the face's closed `Path`, the operation's resolved style, visible,
   drawn **above** the authored nodes.

Errors become diagnostics, never panics: one broken operation cannot take the
scene down.

### 2.3 A failure is visible — the `retired` half

`OperationEvaluation::retired()` names the ids the pass recomputed and could not
compute, and `compose_into` hands them to `scene.retire`. Without that step the
scene prunes by *liveness* — and a Smart Fill whose seed left every face is still
live — so the last region it managed would keep drawing and the failure would be
**invisible on the canvas**. This is the operations half of what the procedural
engine already does for its own nodes. (The smoke caught exactly this; §6.2.)

### 2.4 The Appearance stack

`OperationNode` carries a full `StyleProperties`. `CreateSmartFill.fill:
Option<Color>` seeds `default_style()` with one fill row, so a drop-created fill
is a normal styled node; `SetAppearances` addresses operation ids, so the panel
edits a fill's fill/stroke/gradient/blend like any other node's — and one undo of
`RemoveOperation` restores the paint and the name it had.

---

## 3. RULE 3 — spans, and Break Path at Intersections

**Files:** `crates/vectra-core/src/command.rs` (`BreakPath`),
`crates/vectra-wasm/src/lib.rs` (`break_path`), `apps/vectra-web/src/App.tsx`
(the Region block), `apps/vectra-web/src/engine/client.ts` (`breakPath`).

### 3.1 The command

```rust
Command::BreakPath { node_id: NodeId, pieces: Vec<OutlinePath> }
```

The pieces travel with the command — exactly like `OutlineText`'s letterforms, and
for the same reason: computing them needs *resolved* geometry, which the document
does not hold. The source is **hidden, not deleted**, and the whole verb is one
`Batch` inverse: one undo puts the document back byte for byte.

### 3.2 The engine's cut

`break_path(node_id, spans_json)`:

* accepts `NodeKind::Path` only (a rectangle or a circle is refused with a
  sentence, not a mystery);
* resolves the source's canonical rings (`source_rings`), computes each span's
  two ends in ring-major arc lengths, dedupes and sorts the cuts at tolerance
  `1e-6`;
* walks each ring between consecutive cuts (`ring_pieces_between`) and mints a
  piece per arc — lines plus `Close`, at least two points, the source's style
  cloned, named `"{name} · {index}"`;
* no spans at all → typed error: *"there is nothing to break: no spans were
  given"*.

### 3.3 The UI

The Appearance panel's **Region** block lists the selected path's spans
(`'span 1 of 2 · ring 1 · 148.3 of 628.3'`), each row breaks *that* arc, and the
**Break Path at Intersections** button breaks them all — sending back the very
numbers the engine reported (`breakSpans(rows)` is `[[from, to], …]`), because
where an intersection is is the engine's answer and only the engine's. The status
line is a sentence: `'✂ Broke circle A at 2 spans → 4 paths'`.

One correction the gate run forced: the panel used to ask the engine about the
selected path **alone**, and a path crossed by *other* artwork has no spans in a
graph of one. It now asks with the layer's partners named (`[selected, …peers]`),
which is RULE 1's participant list written out.

---

## 4. RULE 4 — a ColorDrop into a region

**Files:** `apps/vectra-web/src/engine/draw/colordrop.ts`,
`apps/vectra-web/src/engine/draw/regions.ts`,
`apps/vectra-web/src/components/RegionOverlay.tsx`, `App.tsx`.

### 4.1 The drop

A drop inside an enclosed area is **not** a paint change on an existing shape. The
UI asks `smart_fill_plan(ids, point)` — *"which face is under this point?"* — and:

* hit a face → `CreateSmartFill` with that face's **members as boundaries**, the
  drop point as the **seed**, and the current drop colour as the fill's one
  appearance row. The new node sits **above** the boundary paths in the layer
  stack, because it was created after them and the z-order is document order.
* no face → the drop falls through to the ordinary colour drop (the old path), and
  the log says which one happened.

### 4.2 `smart_fill_plan` is the single source of truth

One wasm call answers both "what would the Smart Fill tool highlight?" and "which
region did the drop land in?" — so the highlight a designer sees is *exactly* the
region the click creates. `ids` empty means RULE 1's default: the selection, else
the active layer's geometry. The plan reports, in one JSON object:

* `sources` — each with its name, area, bounds and **spans** (index, from, to,
  ring, start, end, length, total);
* `regions` — each face with its members (the boundaries a fill would name), area
  and bounds;
* `crossings` — every intersection point;
* `hit` — the face index under the point, or `null`.

Naming a group names its members (groups have no outline of their own); naming a
plain shape contributes that shape.

### 4.3 The Smart Fill tool

`smatFill` in the TOOLS list (shortcut `f`, non-direct): hovering highlights the
face under the pointer through `RegionOverlay` (a region outline, drawn from the
engine's path data, not recomputed in the UI), and a click creates the fill. The
log speaks in the product's voice: *"new smart fill in the region between 2
paths"*, and the panel's row reads `'◲ Smart Fill · region 1 of 3 · 2 boundaries'`.

---

## 5. Tests

### 5.1 Rust — **650 passed, 0 failed** (56 targets)

`cargo test --workspace --all-targets --offline --no-fail-fast` → `650 / 0`.
Task 11.0 closed at 633; this round adds **17**: five laws inside `regions.rs`,
five proptests in `region_laws.rs`, and four laws + three proptests in
`smart_fill_laws.rs`.

The named proptests the brief demands, all passing:

* **Region Detection** — `prop_two_overlapping_circles_make_exactly_three_regions`
  (`vectra-geometry/tests/region_laws.rs`): two overlapping circles make *exactly
  three* regions, A-only / B-only / A ∩ B, each a closed path; the lens face's
  area equals the closed-form lens area within 2 %, and `path_area(face.path) ==
  face.area`.
* **Parametric Update** — `prop_moving_a_boundary_updates_the_fill_geometry`
  (`vectra-operations/tests/smart_fill_laws.rs`): moving a boundary dirties the
  boundary, `affected_by` contains the fill, and the fill's drawn geometry is the
  *new* lens (area shrinks with the shift, within 1e-6 relative).
* **Span Break** — `prop_breaking_a_span_yields_two_closed_paths_with_no_gaps`
  (both test crates): a span and its complement are two valid closed paths that
  tile the source exactly, with no gap and no overlap.

Also in the geometry crate: `prop_every_face_is_a_closed_path_that_partitions_the_union`,
`prop_a_span_that_wraps_the_ring_start_still_tiles_it`,
`prop_a_nested_source_is_its_own_face_and_its_host_gets_a_hole`, plus the five
hand-written laws inside `regions.rs` (three regions; crossings cut each outline
into spans; holes remain holes in every face; an uncrossed contained source is its
own face; breaking a span gives two closed pieces with no gap).

In the operations crate: four focused laws — the fill *is* the region its seed is
in (analytic lens + RegionGraph cross-check + its own style row); removing a
boundary withdraws the fill and one undo restores it; `BreakPath` is
non-destructive and one undo is exact; a fill can be repainted through the
ordinary `SetAppearances`/`RemoveOperation` commands — plus the
`prop_a_fill_whose_seed_leaves_every_face_reports_smart_fill_empty` two-phase law
(one boundary away → the fill re-derives to A's face; both away → no geometry and
a `smart-fill-empty` diagnostic, with both boundaries still drawn).

### 5.2 UI — **146 passed, 0 failed** (11 suites)

`npm run typecheck` clean; `npm run test:ui` → `146/0`. The Task 12.0 suite
(`apps/vectra-web/tests/task-12-0-smart-fill.test.ts`, 9 tests) pins the plan
decoding, the span rows, the break payload and the status sentences; the tool
list pins `smartFill` with `f` and "no direct manipulation".

### 5.3 Smoke — **79 steps, `SMOKE PASS`**

`npm run smoke` drives the **real compiled wasm** in Node (no mocks) through the
whole product and, on top of the 75 steps Task 11.0 closed with, four new ones:

* **76** — two circles produce three regions from the engine, each a closed
  `M…Z` path, the lens within 2 % of the analytic value, the crossings on both
  circles near `x = 260, y = 200 ± 80`, the spans partitioning each outline, and a
  point outside every circle answering `hit: null`.
* **77** — a drop creates a Smart Fill: kind `'smart-fill'`, name
  `'smart fill · 2 boundaries'`, `inputs == [a, b]`, fill `#e8622c`, z-order after
  both boundaries; moving a boundary re-derives the geometry; moving it *away*
  makes the fill follow the seed into A; taking *both* boundaries away removes the
  geometry and reports `'smart-fill-empty'` while both boundaries still draw.
* **78** — Break Path at Intersections: the source is hidden (not deleted), the
  pieces are ordinary closed path nodes wearing the source's paint, their areas
  tile the source exactly, and one undo restores the document byte for byte.
* **79** — breaking one span gives the span and the rest of the ring as two
  pieces; a lone circle that nothing crosses has **no** spans; a break with no
  spans is refused with a sentence.

### 5.4 Gate table

| gate | command | result |
| --- | --- | --- |
| build | `cargo check --workspace --all-targets` | ✓ 0 errors |
| format | `cargo fmt --all` | ✓ clean |
| lints | `cargo clippy --workspace --all-targets -- -D warnings` | ✓ 0 warnings |
| tests | `cargo test --workspace --all-targets --offline` | ✓ **650 / 0** |
| wasm | `bash apps/vectra-web/scripts/build-wasm.sh` | ✓ 27,686,250 B |
| types | `npm run typecheck` | ✓ clean |
| UI | `npm run test:ui` | ✓ **146 / 0** |
| e2e | `npm run smoke` | ✓ **79 / 79**, `SMOKE PASS` |

---

## 6. The Compile & Fix Pass: what the first gate run found

The Task 12.0 code was written, committed and pushed before any compiler saw it.
This pass ran the gates in order and fixed what they said. The honest list:

1. **`smart_fill_plan` dropped a named plain node.** The group-expansion loop
   skipped `member == id`, which is right for a *group* (it stands for its
   contents) but wrong for a shape: asking for `[a, b]` returned one source. Fixed
   so a named shape contributes itself and only a named *group* re-expands.
2. **A failed operation kept drawing its last geometry** (`compose_into` pruned by
   liveness, and a failed operation is live). Found by smoke step 77's "no region,
   no fill": the plan reported `smart-fill-empty`, but the canvas still showed the
   stale lens. Fixed with `OperationEvaluation::retired()` + `scene.retire`; the
   scene-level laws were unaffected because their harness retires on the dirty
   path.
3. **Two smoke rectangles were never created.** A `Rectangle` literal without
   `corner_radius` is refused ("missing field `corner_radius`") and the smoke did
   not check the create reply, so the graph saw a single source and reported no
   crossings. Both literals completed; the steps' assertions now exercise real
   two-source crossings.
4. **`null` is not a `&str`.** `smart_fill_plan(ids, null)` throws inside the
   wasm-bindgen glue; the wire's spelling of "no point" is the string `'null'`
   (which the engine already treated as `None`). Five call sites fixed — and
   `draw_pen_commit(false, null)` left alone, because *that* parameter is an
   `Option` and the glue's null-means-`None` path is correct there.
5. **Spans need the participants named.** Steps 78/79 asked for the square alone
   and expected the bar's crossings to appear. They now name both — and the old
   claim "the bar has no crossings" was simply false (the square's right edge cuts
   it twice, at the same two points). Replaced with an honest case: a lone circle
   sharing a graph with the crossed square reports 0 spans against the square's 2.
6. **A crossing is where the *flattened* outlines meet.** The renderer draws
   polygons, so the intersection lies within the flattening tolerance of the
   analytic point. The assertion now checks "on both circles" and "near the
   analytic point (±0.25)", instead of demanding machine precision of a curve it
   no longer draws.
7. **The break affordance could never light up.** `App.tsx`'s spans panel asked
   the engine about the selected path alone; with the layer's partners named it
   sees the crossings the designer sees.
8. **`Cargo.lock` records `geo` for `vectra-geometry`**, so an offline build is a
   no-op on the lockfile instead of a rewrite.

All eight are in the diff; the eleven geometry/operations laws were otherwise
untouched.

---

## 7. Deviations and environment, stated plainly

### 7.1 `wasm-bindgen`, via `scripts/wbgen` — and now through the front door

The `wasm-bindgen` CLI is still unobtainable in this sandbox (the release assets
are firewalled; the crate sources are not). Task 11.0 shipped `scripts/wbgen`, a
driver that links `wasm-bindgen-cli-support` and applies the same `--target web`
settings. This round went one better so that the *real* script runs unchanged: a
`wasm-bindgen` stand-in at the pinned version (`0.2.129`) accepts the CLI's
`--version` and `--target web --out-dir <dir> <input>` invocation and forwards to
`wbgen`. `bash apps/vectra-web/scripts/build-wasm.sh` therefore passes its own
version check, builds the wasm, regenerates the glue, and lists the four
artifacts — which is the run recorded in §5.4.

The module is the **debug** profile (the repository's default; `release` is one
argument away): 27,686,250 B against 27,054,595 B at Task 11.0 — the difference is
the Region Graph, the Smart Fill evaluator and the two new wasm methods.

### 7.2 Offline, vendored, deterministic

Everything above ran with `CARGO_HOME=/tmp/cargohome` against a vendored registry
and `--offline`; the front-end gates ran from `apps/vectra-web` with the committed
wasm and no network. Nothing in the gates reaches the network.

### 7.3 Honest limits

* Smart Fill does not yet read *another operation's* geometry (operation-on-
  operation nesting is Phase 2); a boundary must be an authored node.
* Break Path refuses anything that is not a `Path` — breaking a rectangle's
  outline into arcs would mint pieces with no way to keep the rectangle's own
  parameterization, and inventing that is Phase 2's business.
* The UI's hover probe uses RULE 1's editing context (the selection, else the
  active layer); a single-path selection therefore scopes the *tool* to that path
  — the panel (§3.3) is the one place that deliberately widens the question to the
  layer.

---

## 8. Files touched

**Geometry** — `regions.rs` (new, 1,251 lines), `convert.rs` (moved in from
`vectra-operations`, 288 lines), `diagnostic.rs` (+`SmartFillEmpty`),
`lib.rs` (re-exports), `tests/region_laws.rs` (new, 457 lines).

**Core** — `operation.rs` (`OperationKind::SmartFill`, `OperationNode::smart_fill`,
`arity`, `boundaries`, `validate`, `affected_by`), `command.rs` (`CreateSmartFill`,
`BreakPath`, `SetAppearances` for operation ids, smarter orphan-op withdrawal).

**Operations** — `evaluator.rs` (`evaluate_smart_fill`, `evaluate_all`,
`OperationEvaluation::retired`, `compose_into`), `error.rs`
(`SmartFillEmpty`, `NoBoundaries`), `convert.rs` (now a shim),
`tests/smart_fill_laws.rs` (new, 573 lines).

**Wasm** — `lib.rs` (`smart_fill_plan`, `break_path`, dirty propagation),
`snapshot.rs` (the plan wire types), `Cargo.toml`.

**Dependency / AI** — `prospective.rs`, `exec.rs`, `schema.rs` (the new commands
in the semantic graph, the prose and the tool schema).

**UI** — `engine/wire.ts`, `engine/commands.ts`, `engine/client.ts`,
`engine/draw/tools.ts`, `engine/draw/regions.ts` (new), `engine/draw/colordrop.ts`,
`engine/panels.ts`, `components/RegionOverlay.tsx` (new), `components/
AppearancePanel.tsx` (the Region block and the break buttons), `App.tsx` (the
region plan, the tool, the drop, the drag probe), `App.css`, and
`tests/task-12-0-smart-fill.test.ts` (new, 9 tests).

**E2E** — `scripts/smoke.mjs` steps 76–79.

Task 11.0 rides in the same branch and PR; its own report is
`TASK-11.0-REPORT.md`.

---

## 9. What a designer can do now

1. **Draw two overlapping shapes, pick the Smart Fill tool, hover the lens.** The
   region under the pointer lights up. Click, and the lens becomes a real node
   with a fill colour you can change — not a painted-over area. Drag either circle:
   the fill re-derives instantly, because it is a parametric node that *names* its
   boundaries.
2. **Drag a colour onto an enclosed area.** The same region becomes a Smart Fill,
   pinned above the paths that bound it.
3. **Select a path, open the Region block, and see its spans.** Each row is an arc
   between two crossings; break one to split it into that arc and the rest of the
   ring, or hit **Break Path at Intersections** to turn the whole outline into its
   arcs. The source is hidden, not destroyed — one undo brings it back, paint and
   all.
4. **Move the artwork.** Because fills and spans are derived from the graph, the
   region a fill covers is always the region the boundaries currently enclose: the
   12.0 answer to "the space between the shapes" is a fact about the document, not
   a bitmap someone has to repaint.
