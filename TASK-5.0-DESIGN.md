# Task 5.0 — The WebGPU Renderer: design

The requirement, in the user's terms: the renderer is a **dumb consumer** of
`EvaluatedScene`; GPU updates are **strictly incremental** (touch the affected buffer
slices of the affected nodes, never rebuild the scene per frame); and **hit testing is
mandatory** — a canvas click must arrive at the engine as a `NodeId` it can hand to
`BeginDrag`.

Those three statements are RULE 1, RULE 2 and RULE 3 below. Everything in this document
either implements one of them or is bookkeeping that keeps them true.

---

## 0. Process note — this document is written *after* the code

Task 5.0 was authorised with "Do not ask clarifying questions. Proceed." The implementation
therefore landed code-first: the crate, its laws, the wasm seam and the React swap were
written and made green before this file existed. This document is therefore **descriptive**
of shipped code, not prescriptive of planned code. Every claim below is about a file that
exists and a test that runs; where the design *chose* something over an alternative, the
choice is argued in place. Where the code fell short of the intent, the gap is in §11
rather than quietly omitted.

What did *not* deviate: the standing rules. The renderer reads no `Document`, resolves no
parameter, and evaluates nothing.

---

## 1. The pipeline, and what crosses each boundary

```text
Document ─[GeometryEvaluator]─▶ EvaluatedScene ─[RenderScene]─▶ WritePlan ─[GpuRenderer]─▶ Surface
 parametric      (already built)   resolved         tessellate       surgical slices      wgpu
```

Each arrow is a *type*, and each type is the contract:

| boundary | value | who may produce it |
| --- | --- | --- |
| engine → renderer | `&EvaluatedScene` + `&DirtySet` | the wasm shell (`render_frame`) |
| renderer internal | `WritePlan` (node, slot, buffer, bytes) | `RenderScene::sync` |
| renderer → GPU | `BufferSink` calls | `GpuRenderer` (real) or `MockSink` (tests) |
| renderer → engine | `FrameWire` (dirty vs writes/bytes) | `Renderer::present` |
| renderer → React | `NodeId` for a pointer, JSON camera/stats | `pointer_hit`, `view`, `stats` |

There is no path in which a React component, a `Document` or a solver value reaches a
buffer. `EvaluatedScene` being *flattened and baked* (no transform field, coordinates
already resolved) is what lets the shader be twenty lines.

## 2. RULE 1 — the dumb consumer

Three consequences shaped the module boundaries:

1. **The renderer owns tessellation, not evaluation.** `primitive_to_path` (already part of
   `vectra-geometry`) is the only bridge from a primitive to lyon; the renderer never asks
   *why* a shape is where it is.
2. **Draw order comes in with the scene.** `z_order` is the engine's decision (it is also
   what the `Operations` layer composes into), so the renderer treats it as data and can
   change it without a single buffer write.
3. **A node that fails to tessellate is dropped, not fatal.** A degenerate or
   non-finite primitive inside the engine's tolerances is logged and skipped; the frame
   stays honest. (The evaluator diagnoses such values upstream; the renderer's job is to
   keep drawing.)

## 3. RULE 2 — the write plan

### 3.1 Change classes

The whole design is one question, asked per dirty node: *what does this node's new state
differ in, and what is the cheapest upload that expresses the difference?*

`GeometryKey { shape, origin }` answers the first half. `shape` is a hash-free structural
key of the flattened rings (so an unchanged shape with a new origin is *the same shape*);
`origin` is the shape's bounding-box minimum. The stroke's tessellation inputs join the
geometry test — a wider outline is a different mesh even though the width is a style field:

```rust
let geometry_changed = previous.map_or(true, |slot| {
    slot.key.shape != key.shape || slot.stroke_key != stroke_key
});
let moved   = previous.map_or(true, |slot| slot.key.origin != key.origin);
let style   = previous.map_or(true, |slot| slot.style != node.style);
if !created && !geometry_changed && !moved && !style { continue; }   // zero writes
```

### 3.2 The node-local frame (why a drag is 64 bytes)

Tessellation happens in the node's **own** frame: vertices are rebased on the shape's
bounding-box minimum, and that origin travels to the GPU as `transform.zw` with unit scale.
A translation therefore changes *only* the instance row — the vertex bytes stay
bit-identical, and the unit test in `instance.rs` pins the mapping (`smaller_translation`).

This is the difference between a drag costing 64 bytes and a drag costing a re-upload of a
1000-vertex path at 60 Hz. It also makes the local frame available to the fragment stage
(`local_position`), which Phase 2's distance-field work will need.

### 3.3 The instance row

One `InstanceRaw` per node, in one storage buffer indexed by `instance_index`:

```
transform: vec4<f32>   fill: vec4<f32>   stroke: vec4<f32>   params: vec4<f32>   = 64 bytes
```

Four `vec4<f32>` rows mean the struct has no interior padding, so its byte image *is* its
field order — hand-packed little-endian (`push_vec4`), with every offset the shader reads
pinned by `the_instance_layout_matches_the_wgsl_struct`. Free-list slots let a removed
node's row be reused instead of leaking the array; a dropped slot is zeroed rather than
compacted, so no other node's bytes move when a node dies.

Opacity is folded into the colour's alpha (and mirrored in `params.y` for the inspector).
The pipeline blends with `ALPHA_BLENDING`, which is `SrcAlpha/OneMinusSrcAlpha` for colour
and `OVER` for alpha — the GPU law asserts both halves, because assuming the wrong alpha
rule is exactly the kind of bug a mock cannot see.

### 3.4 The plan → sink protocol

```rust
fn create_node(id, slot, fill: &Mesh, stroke: &Mesh);
fn write(id, slot, kind: WriteKind, bytes: &[u8]);
fn drop_node(id);
fn ensure_instances(count);
fn set_order(&[NodeId]);
```

`RenderScene` decides *what* to write; the sink decides *how*. `GpuRenderer` is the only
implementation that touches a device, and it is the only place in the crate where
`write_buffer` appears — so "surgical updates" is a property of one reviewable file.
`MockSink` keeps per-node byte images and a call ledger, which lets the laws assert
*touched-ness* as byte equality rather than as a call count.

Growth is amortised, not per-frame: mesh buffers are allocated per node at their size,
the instance buffer grows at 1.5× slack (rebuilding the bind group only then), and buffers
are floored at 4 bytes because WebGPU forbids zero-sized buffers.

### 3.5 Coalescing above the renderer

A gesture settles once per pointer sample (Task 3.2's `UpdateDrag`), but a frame is drawn
once per `requestAnimationFrame`. `DirtyLedger` sits between them: `settle` notes ids
(a `Full` evaluation supersedes the set), `render_frame` drains it. Because `sync` compares
against the GPU's current state, the intermediate states are free — the frame uploads the
net difference only. `tests/render_laws.rs` proves "40 samples, 1 write"; the smoke test
proves it again through the compiled artifact.

## 4. RULE 3 — hit testing

The grid (candidate generation) and the parity test (the answer) are separated because they
answer different questions, and conflating them is how hit testing usually goes wrong:

* **Coarse stage.** `HitIndex` is a uniform 32×32 grid over the scene bbox. Cell size comes
  from the scene's own extent; queries floor the cell coordinate and reject out-of-range
  columns/rows (the grid's slack already covers the scene — an early version clamped and a
  point a whole screen away "hit" a shape). Candidates are deduplicated by `NodeId` and
  returned front-to-back.
* **Exact stage.** Even-odd parity over the node's flattened rings, evaluated in the node's
  local frame — the same rings and the same fill rule the tessellator used. Holes are holes,
  a circle's bbox corner is a miss, an arc is a wedge.

The camera is the third leg: `Camera::{document_to_screen, screen_to_document}` are the
exact inverses of the shader's projection (asserted by transcribing the WGSL in a unit test
and comparing), so a pointer resolves to the document point whose pixels the user is
looking at. Document space is y-up; the flip is the *sign* of `view.w`, which makes it one
convention implemented once rather than three conversions that can disagree.

The transform from client coordinates to canvas pixels is also renderer-side
(`CanvasRect` + the drawing-buffer ratio), so React passes raw `clientX/clientY` and learns
nothing about DPI, boxes or scales.

## 5. The shader’s shape

* **One vertex stage, two fragment stages.** Both meshes are already triangles; only the
  colour source differs. Two pipelines on one vertex layout, no second geometry pass, no
  stencil, no SDF in Phase 1.
* **A read-only storage buffer for instances** (`array<Instance>`) rather than a uniform
  array: no fixed node cap, and the binding is rebuilt only when the buffer grows.
* **The camera uniform is 32 bytes** and is written only on `set_camera` (pan/zoom must not
  re-upload geometry either).
* **No vertex-stage per-draw uniforms.** Each draw's instance range is the node's single
  slot, so `@builtin(instance_index)` selects the node's data with no rebinding.

## 6. The wasm seam

`Renderer` is the class React drives (`new` / `attach` / `measure` / `pointer_hit` /
`pointer_doc` / `view` / `stats`), plus one Rust-facing method (`present`) that the engine
calls from `render_frame`. The split is deliberate:

* the DOM/WebGPU half is `#[cfg(target_arch = "wasm32")]` — two functions, `attach` and
  `measure`, are the only code in the crate that names `web-sys`;
* everything else (viewport, client→document, the ledger, the frame record) is plain Rust,
  so `cargo test -p vectra-wasm` covers the pointer chain and the frame budget headlessly;
* a browser without WebGPU resolves `attach` with `ready: false` and a message rather than
  rejecting, and hit testing keeps working, because the index is CPU-side state.

`render_frame(&mut self, renderer: &mut Renderer)` is the only coupling between the engine
and the canvas: the engine pushes a scene and a dirty set, and gets back a frame record.

## 7. The React swap

The SVG preview was deleted, not kept beside the canvas. That is the substantive point of
requirement 6: the old preview was a *second implementation of the document* (arc → path
`d` in TypeScript, a y-flip group, per-node DOM hit areas), and every shape it drew was a
chance for the two implementations to disagree. Afterwards:

* one `<canvas>`; `App.tsx` holds no coordinate system and no shape knowledge;
* pointer events send raw client coordinates and receive a `NodeId`;
* the drag offset is the only arithmetic left (a view offset, not geometry);
* the frame readout under the canvas reports what the GPU actually paid
  (`dirty 1 · wrote 1 buffer 64 B · draw 4`), which is the incrementality witness the Task
  2.2 doctrine asks to be *visible*.

The document grid survives as a CSS overlay, exact only because the Phase-1 camera is fixed
(§11.3).

## 8. Testing strategy

Three layers, each covering what the one below cannot:

| layer | suite | covers |
| --- | --- | --- |
| pure Rust, no device | `--lib`, `tessellation_laws`, `incremental_laws`, `hit_laws` | tessellation validity, the plan, the sink ledger, the index |
| shader/plan structure | `shader_laws` (naga parse + validate + binding/layout assertions) | that the WGSL and the Rust layouts agree *before* a device exists |
| a real device | `gpu_laws` (offscreen render + pixel readback) | that the configured pipeline produces the intended pixels |
| the seam | `render_laws` (wasm, host) + smoke steps 25–26 (real artifact, Node) | engine → ledger → canvas, and pointer → `NodeId` → `BeginDrag` |

The GPU suite skips with a printed `SKIP` line when no adapter exists, and runs for real
when one does (lavapipe is enough). A suite that fails on a GPU-less machine gets disabled;
a suite that silently passes without testing anything is worse — the skip is *printed* so
the log is the evidence, not the exit code. It also takes a process-wide lock for the whole
test, including teardown: `cargo test`'s parallel harness met lavapipe's assumption of one
device per process and segfaulted. Serialising six 0.05-second tests is cheaper than
explaining a flaky suite later.

## 9. Deliberate deviations from the brief

1. **"simple distance-field or multi-pass for Phase 1" strokes → neither.** lyon expands the
   outline geometrically, so the stroke pipeline exists but the *geometry* is flat triangles
   in both cases; a distance field would buy AA and round joins the style model cannot ask
   for yet. The alternative (a second pass over the same mesh with a stencil) would have
   added a pipeline and an attachment for no Phase-1 capability.
2. **"per-node uniform buffer" → one instance *storage* buffer.** A per-node uniform binding
   would mean a bind group per node (or dynamic offsets, which cap the number of nodes),
   and it would make every style change a binding change. One array indexed by
   `instance_index` gives the same 64-byte write with no rebinding, and removes the node cap.
3. **`encase` was dropped.** It maps `[f32; 4]` to WGSL `array<f32, 4>` (stride 4), not
   `vec4<f32>`; the first instance write panicked. The rows are hand-packed with a unit test
   pinning every offset the shader reads — less magic, and no dependency.
4. **The camera is y-up** (the sign of `view.w`), matching the engine's document space and
   the preview the canvas replaced. The alternative — screen-down, "SVG convention" — was
   the first draft and would have quietly flipped every shape vertically.
5. **Hit testing is mesh-accurate, not bbox-accurate.** The brief allowed "simple bbox grid
   or R-tree"; a bbox-only answer would have made a click on a circle's corner select it.
   The grid is the bbox structure; the answer is parity over the rings.

## 10. Gate status

* `cargo fmt --all -- --check` — clean
* `cargo clippy --workspace --all-targets -- -D warnings` — 0 warnings
* `cargo test --workspace` — 27 targets, **279 passed / 0 failed** (59 of them from
  `vectra-render`, 9 from the wasm render laws; Task 4.0's gate was 211 passed)
* `npm run typecheck` / `test:ui` (23) / `build` — clean
* `node scripts/smoke.mjs` — 26 / 26 steps through the compiled wasm module
* `bash apps/vectra-web/scripts/build-wasm.sh debug` — wasm32 build + bindings

## 11. Open items

1. **The browser path is unexecuted in this environment** (no browser, no canvas): `attach`
   → adapter → configure → present compiles for wasm32 and is exercised as logic on the
   host, but has never run in a browser. First thing to do on a machine with one.
2. **Selection / hover in the pixels.** `params.z` is reserved and unwritten; the overlay is
   Task 5.1.
3. **The grid must move into the shader** when the camera gains pan/zoom (and hairlines and
   the selection outline with it). Today's CSS overlay is exact only because the camera is
   fixed.
4. **Frame scheduling.** One frame per `requestAnimationFrame` unconditionally: cheap when
   idle (0 writes) but never skipped. Damage tracking above the dirty set (and a
   `present_after`-style hook) belongs with the pan/zoom work.
5. **Clipping.** One draw per node per mesh, no scissor or viewport culling: a scene with ten
   thousand off-screen nodes still submits ten thousand draws (each rejected by the
   rasteriser). The spatial index already knows which nodes are off-screen; wiring that into
   the plan is the natural next incrementality win.
6. **Stroke joins/caps** are lyon defaults until the style model can express them.
7. **A hover/preview path for `pointer_hit`** — the current query is per-event; a hover
   highlight would want the query throttled to a frame, which is a UI decision, not a
   renderer one.
