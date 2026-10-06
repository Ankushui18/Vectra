# Task 5.0 — The WebGPU Renderer: report

**Status: complete.** `vectra-render` tessellates `EvaluatedScene` with `lyon`, uploads
strictly surgical slices through `wgpu`, answers pointer queries from a spatial index, and
the React app now draws the document with it — the SVG projection is gone. All gates green.

Companion documents: `TASK-5.0-DESIGN.md` (the design, the deviations and the open items),
`TASK-4.0-REPORT.md` / `TASK-4.0-DESIGN.md` (the operations layer whose results are now
rendered as ordinary nodes).

Requested deliverables: **§2 the final WGSL shader**, **§3 the incremental update logic**,
**§4 the hit-testing implementation**, **§6 test results**, **§7 workspace gate status**.

---

## 1. What was built, and where

| piece | file | what it does |
| --- | --- | --- |
| tessellation | `crates/vectra-render/src/tessellate.rs` | `EvaluatedPrimitive` → `lyon` path → fill + stroke `Mesh` (`f32` vertices, `u32` indices) |
| geometry keys / local frame | `crates/vectra-render/src/geometry.rs` | flattening, bounds, `GeometryKey{shape, origin}`, even-odd fill rule, invariant tests on `Mesh` |
| instance layout | `crates/vectra-render/src/instance.rs` | 64-byte hand-packed per-node row + the 32-byte camera, and the screen↔document pair |
| the frame plan | `crates/vectra-render/src/scene.rs` | `RenderScene::sync` — dirty set in, `WritePlan` out; `flush` through a `BufferSink` |
| GPU state | `crates/vectra-render/src/gpu.rs` | `GpuRenderer`: pipelines, camera + instance buffers, `BufferSink` impl (the only `write_buffer` site) |
| shader | `crates/vectra-render/src/shader.wgsl` | one vertex stage, two fragment entry points (fill / stroke) |
| hit testing | `crates/vectra-render/src/hit.rs` | `HitIndex`: 32×32 bbox grid, front-to-back candidates |
| wasm canvas | `crates/vectra-wasm/src/render.rs` | the `Renderer` class React drives: attach, measure, present, pointer queries |
| engine seam | `crates/vectra-wasm/src/lib.rs` | `DirtyLedger` fed by `dirty_event`, drained by `render_frame` |
| React | `apps/vectra-web/src/{App.tsx,App.css}`, `src/engine/client.ts`, `src/engine/wire.ts` | canvas in place of the SVG, pointer routed through `pointer_hit` |

`EvaluatedScene` is the *only* input (RULE 1): the renderer never sees a `Document`, a
parameter, a constraint or an operation, and never evaluates anything.

## 2. The final WGSL shader

`crates/vectra-render/src/shader.wgsl` — 104 lines, parsed and type-checked by `naga`
(the same front end `wgpu` uses) in `tests/shader_laws.rs`, and compiled into a real
pipeline and executed in `tests/gpu_laws.rs`.

```wgsl
struct Camera {
    view: vec4<f32>,      // (min_x, min_y, width, height) — height is negative: y-up
    screen: vec4<f32>,    // (width_px, height_px, _, _)
}

struct Instance {         // 64 bytes, one element per draw-order slot
    transform: vec4<f32>, // (scale_x, scale_y, translate_x, translate_y)
    fill: vec4<f32>,      // straight (non-premultiplied) RGBA, opacity folded into a
    stroke: vec4<f32>,
    params: vec4<f32>,    // (stroke_width, opacity, selected, reserved)
}

@group(0) @binding(0) var<uniform> camera: Camera;
@group(0) @binding(1) var<storage, read> instances: array<Instance>;

struct VertexInput {
    @location(0) position: vec2<f32>,
}

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) fill_color: vec4<f32>,
    @location(1) stroke_color: vec4<f32>,
    @location(2) local_position: vec2<f32>,
}

fn document_to_clip(point: vec2<f32>) -> vec4<f32> {
    let uv = (point - camera.view.xy) / camera.view.zw;
    return vec4<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, 0.0, 1.0);
}

@vertex
fn vs_main(
    input: VertexInput,
    @builtin(instance_index) instance_index: u32,
) -> VertexOutput {
    let instance = instances[instance_index];
    let world = input.position * instance.transform.xy + instance.transform.zw;
    var out: VertexOutput;
    out.clip_position = document_to_clip(world);
    out.fill_color = instance.fill;
    out.stroke_color = instance.stroke;
    out.local_position = input.position;
    return out;
}

@fragment
fn fs_fill(input: VertexOutput) -> @location(0) vec4<f32> { return input.fill_color; }

@fragment
fn fs_stroke(input: VertexOutput) -> @location(0) vec4<f32> { return input.stroke_color; }
```

(The file also carries the rationale comments; the listing above is the executable part.)

Four decisions are visible in those twenty lines, and all four are load-bearing:

1. **One vertex stage, two fragment stages.** `vs_main` is shared; `fs_fill` and
   `fs_stroke` differ only in which colour row they emit. Both meshes are already flat
   triangles (lyon expanded the outline), so a stroke costs a second pipeline, not a
   second geometry pass — no SDF, no stencil in Phase 1.
2. **Everything per-node lives in the instance array.** The vertex buffers hold nothing but
   `vec2<f32>` positions in the node's **local frame**; the frame's origin rides in
   `transform.zw`. That is why a translation leaves every vertex byte bit-identical.
3. **A read-only storage buffer, not a uniform array.** `array<Instance>` needs no
   `array<Instance, N>` declaration, so there is no fixed node cap; the binding is rebuilt
   only when the buffer has to grow (O(log n) times a session).
4. **The y flip is the sign of `camera.view.w`.** Document space is y-up; a texture's rows
   run downward. One sign carries that difference for the projection, the pointer mapping
   and the hit test — no per-call `if`, no way for the three to disagree.

## 3. The incremental update logic (RULE 2)

`RenderScene::sync(&mut self, scene: &EvaluatedScene, dirty: &DirtySet) -> &WritePlan`.

The dirty set is classified per node, never per frame. `GeometryKey{shape, origin}` splits
"the shape changed" from "only its position changed", and the stroke's tessellation inputs
(visible, width) are part of the shape key — a widened outline is a *geometry* change even
though the width lives in the style:

| what changed | work | GPU traffic |
| --- | --- | --- |
| reshape / resize | re-tessellate that node | that node's vertex + index buffers |
| move (drag, solver partner) | none | one instance row (64 B) |
| restyle (colour, opacity) | none | one instance row (64 B) |
| stroke appears / widens / vanishes | re-tessellate the outline | that node's stroke vertex + index buffers |
| dirty but unchanged | none | **nothing** |
| nothing dirty | none | **nothing** |

`flush(&mut self, sink)` then turns the plan into calls on a `BufferSink` —
`create_node` / `write` / `drop_node` / `ensure_instances` / `set_order`. `GpuRenderer`
implements it (and is the *only* place in the crate that calls `write_buffer`); `MockSink`
implements it as a byte-exact ledger, which is how the laws below assert "untouched" as
*byte equality* rather than as a call count.

Above the renderer, `VectraEngine` funnels **every** dirty set it publishes through one
function:

```rust
fn dirty_event(&mut self, ids: Vec<NodeId>, mode: EvalMode) -> EngineEvent {
    self.render_dirty.note(ids.iter().copied(), mode);   // the canvas's copy
    EngineEvent::Dirty { ids, mode }                     // the UI's copy
}
```

Commands, undo/redo, the clock, each drag sample and a full rebuild all end there, so the
canvas cannot miss a change. `render_frame` drains the ledger:

```rust
pub fn render_frame(&mut self, renderer: &mut Renderer) -> String {
    if !self.scene.is_warm() { self.settle(Vec::new()); }
    let dirty = self.render_dirty.take();
    renderer.present(self.scene.scene(), &dirty).  …
}
```

The ledger **coalesces**, which is the piece that makes the drag fast path real: a gesture
settles once per pointer sample, but the frame that finally draws receives the union. Since
`sync` compares the new scene against what the GPU already holds, the intermediate states
cost literally nothing — twenty samples and one pointer sample produce the same single
64-byte write.

Numbers, each asserted somewhere in the suites (see §6 for which):

| scenario | dirty | writes | bytes |
| --- | --- | --- | --- |
| first frame after mount, 2 nodes | full | ≥ 4 (create) | vertices + indices of both |
| one node moves | 1 | **1** | **64** |
| one node is restyled | 1 | **1** | **64** |
| a stroke's width changes | 1 | 2 (stroke meshes only) | the outline's, fill untouched |
| 40 drag samples, one frame | 1 | **1** | **64** |
| idle frame | 0 | **0** | **0** |
| `force_full_evaluation` then a frame | full | **0** | **0** |
| draw order changes | 0 | **0** | 0 (iteration order only) |
| node added / removed | 1 | create / drop only | that node's |

## 4. The hit-testing implementation (RULE 3)

```rust
RenderScene::hit_test(&mut self, x: f32, y: f32) -> Option<NodeId>
```

Two stages, because a pointer query has two different jobs:

* **Which nodes to test** — `HitIndex`, a uniform 32×32 grid over the scene's bounding box,
  rebuilt lazily from the slots' world bounds (`ensure_index` after any sync). A query finds
  its cell in O(1), and candidates come back **front to back** so the topmost shape wins.
  The grid is deliberately coarse: cell size derives from the scene's own extent, and the
  grid's slack covers it, so a cell lookup never needs a clamp (an early version clamped,
  which made a far-away point hit a shape; the law in `hit.rs` now pins that down).
* **What the answer is** — an even-odd parity test over the node's flattened rings, in the
  node's local frame (`contains_point` → `ring_polygon` → `geo::Contains`). This is what
  makes clicking a circle's bounding-box corner return `None`, a hole not a hit, and an arc
  a wedge rather than a disc.

The pointer's document coordinates come from the **same camera** that drew the pixels:
`Camera::{document_to_screen, screen_to_document}` are exact inverses, and
`tests/gpu_laws.rs` pins the Rust pair against the WGSL projection transcribed in
`instance.rs`'s unit tests. So "what the pointer hits" and "what the pixels show" cannot
drift apart.

React sees none of this. It reports where the mouse is:

```ts
const id = client.pointerHit(e.clientX, e.clientY);   // undefined → null, one thing to test
if (!id) return;                                      // empty space: no node, no command
client.dispatch(beginDrag(id));                       // the engine owns the gesture
```

## 5. Requirement 6 — the wasm `Renderer` and the canvas swap

`crates/vectra-wasm/src/render.rs` exposes a `Renderer` class:

| method | JS-visible | what it does |
| --- | --- | --- |
| `new()` | ✓ | CPU-side state (spatial index, camera); no GPU yet, and nothing panics |
| `attach(canvas)` | ✓ async | requests adapter + device, creates the surface, builds the pipelines |
| `measure(canvas)` | ✓ | reads the box + device pixel ratio, resizes the backing store, refits the camera |
| `set_viewport(l,t,w,h,scale)` | ✓ | the same path from plain numbers (what the host laws drive) |
| `pointer_hit(clientX, clientY)` | ✓ | RULE 3: client point → `NodeId` |
| `pointer_doc(clientX, clientY)` | ✓ | client point → document point (the grab offset) |
| `view()` / `stats()` | ✓ | the visible document rectangle / device + buffer totals |
| `present(&scene, &dirty)` | – | the frame (Rust types, called by `render_frame`) |

Only `attach` and `measure` name `web-sys`, and both are `#[cfg(target_arch = "wasm32")]` —
which is why the whole pointer chain is host-testable and why `cargo test -p vectra-wasm`
covers it. React's side:

1. On mount, the client is asked to `attachCanvas(canvasRef.current)`; the status
   (device up, pixel size, draw calls, or a failure message) goes into the log and, on
   failure, into a banner over the canvas.
2. A `ResizeObserver` calls `measureCanvas` so the backing store and the camera follow the
   panel.
3. A `requestAnimationFrame` loop calls `renderFrame()` once per displayed frame and shows
   the frame's cost under the canvas (`dirty 1 · wrote 1 buffer 64 B · draw 4`).
4. `pointerdown` → `pointer_hit` → `BeginDrag`; `pointermove` → `pointer_doc` →
   `UpdateDrag`; `pointerup`/`pointercancel` anywhere → `EndDrag`. A click on a shape also
   names it in the Inspector (the renderer said which node; the UI only obeys).

The SVG projection is deleted: `PreviewNode`, the arc/path `d` builders, `DOC_W/DOC_H`,
`getScreenCTM`/`DOMPoint`, the per-node CSS affordances. `App.tsx` no longer knows a
coordinate system, and the drag offset is the only arithmetic left (a view offset).

## 6. Test results

All suites were run on this machine; the GPU suite ran against Mesa's **lavapipe** (software
Vulkan) and can be re-run anywhere with `XDG_RUNTIME_DIR=/tmp cargo test -p vectra-render`.

| suite | result | what it proves |
| --- | --- | --- |
| `vectra-render` unit (`--lib`) | **20 / 20** | hit index (5), instance layout + camera (10), tessellator (5) |
| `tests/tessellation_laws.rs` | **6 / 6** | Tessellation Validity Law |
| `tests/incremental_laws.rs` | **11 / 11** | Incremental Update Law (mock buffer tracking) |
| `tests/hit_laws.rs` | **10 / 10** | Hit-Test Law (proptest) |
| `tests/shader_laws.rs` | **6 / 6** | the WGSL parses, validates, and has the bindings/layout the Rust side assumes |
| `tests/gpu_laws.rs` | **6 / 6** | the shader **runs on a real device** and the pixels are asserted |
| `vectra-wasm/tests/render_laws.rs` | **9 / 9** | the engine → ledger → canvas seam, and the pointer chain |
| `apps/vectra-web/scripts/smoke.mjs` | **26 / 26 steps** | the same through the compiled wasm module in Node |
| `apps/vectra-web/tests/view-model.test.ts` | **23 / 23** | unchanged (view-model projections) |

### 6.1 The three laws named in the brief

* **Tessellation Validity Law** — `tessellation_laws.rs`: proptests over rectangles,
  circles, polygons and arcs assert every mesh is finite, index-in-bounds and non-degenerate
  (area checks against closed forms: a rect's exact area, a circle against the inscribed
  64-gon within 0.1 %, an angle-sorted polygon against its shoelace area, a stroke ring
  against 144 − 64 ± 1), plus a 32-case proptest that syncs and flushes whole random scenes
  through `MockSink` — including "a second sync of an unchanged scene writes nothing".
* **Incremental Update Law** — `incremental_laws.rs`: "recolour A ⇒ only A's uniform buffer
  is touched". The brief's own case is the first test; the suite goes further because the
  same machinery makes a stronger statement available: a **move does not touch vertex data
  either** (the local-frame design), a resize rewrites *that node's* meshes and leaves its
  neighbour byte-identical, a widened stroke rewrites only the stroke's buffers, adding and
  removing nodes touch only their own buffers, and a 64-step sweep of style/move/resize
  edits asserts "writes name the edited node and no other, and the untouched node's bytes
  are identical" at every step. This suite caught a real bug: stroke *width* was classified
  as instance-only, so a widened outline never rebuilt.
* **Hit-Test Law** — `hit_laws.rs`: proptests over scenes of non-overlapping jelly-bean
  rectangles laid out by rejection sampling on a jittered grid (one shape per cell, so
  "centre" and "outside" have unambiguous answers by construction). Every centre returns
  exactly its own `NodeId`; far-away points and cell gutters return `None`; jittered
  interior points never produce a phantom hit; stacked shapes return the last drawn; a move
  re-points the index. Analytic cases cover the ones a sampler cannot state: a circle's
  bounding-box corner is a miss, a path's hole is a miss, an arc is a wedge not a disc, and
  a removed node stops hitting.

### 6.2 The GPU laws (what a mock cannot prove)

`gpu_laws.rs` takes a real adapter, renders into an offscreen `Rgba8Unorm` texture and reads
the pixels back. `Rgba8Unorm` (not the `Srgb` variant) is deliberate: no transfer function
sits between the shader's output and the bytes asserted on. Verified this session:

* a red rect at doc `(100,100)–(300,200)` is `[255, 0, 0, 255]` at the pixel its camera
  says, and the clear colour everywhere just outside it;
* a 10-unit blue outline on a green rect: blue at the path edge, green in the interior,
  clear 8 units out — the outline is neither missing nor twice as wide;
* opacity 0.5 white over an opaque clear composites to `135 / 136 / 139` (the arithmetic
  answer ±1) — and the alpha channel stays 255, because `ALPHA_BLENDING` composites alpha
  with `OVER`; the test's first version expected 191 and the device said 255, which is now
  asserted as the contract;
* two overlapping rects draw in `z_order`, and reordering flips the overlap **with zero
  buffer writes**;
* a move writes exactly 1 buffer / 64 bytes and the rect is at its new position and *not*
  at its old one; a restyle writes exactly 1 buffer / 64 bytes and the pixel changes colour.

If no adapter is present the suite prints `SKIP …: no Vulkan adapter on this machine` and
passes — read the output, not just the exit code. The suite also **serialises itself**
(`vulkan_lock`): `cargo test`'s parallel harness meets drivers that assume one device per
process, and lavapipe segfaults under it (observed once, after two tests had passed). Six
tests, one at a time, ~0.3 s; three consecutive full-workspace runs are green since.

## 7. Workspace gate

* `cargo fmt --all -- --check` — clean
* `cargo clippy --workspace --all-targets -- -D warnings` — **0 warnings**
* `cargo test --workspace` — **27 test targets, 279 passed, 0 failed** (Task 4.0's gate was
  211 passed; Task 5.0 added 59 render tests + 9 wasm render laws)
* `npm run typecheck` — clean · `npm run test:ui` — **23 / 23** · `npm run build` — OK
* `node scripts/smoke.mjs` — **26 / 26** through the compiled wasm module
* `bash apps/vectra-web/scripts/build-wasm.sh` — builds the wasm32 target (wgpu included)
  and regenerates the bindings

## 8. How to reproduce

```bash
# engine + laws (GPU laws need a Vulkan adapter; lavapipe is fine)
export PATH=/usr/local/cargo/bin:$PATH CARGO_HOME=/usr/local/cargo RUSTUP_HOME=/usr/local/rustup
XDG_RUNTIME_DIR=/tmp cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --all -- --check

# the canvas, in a browser
cd apps/vectra-web
bash scripts/build-wasm.sh debug     # cargo build --target wasm32 + wasm-bindgen --target web
npm run dev                          # http://localhost:5173 (needs a WebGPU browser)
node scripts/smoke.mjs               # 26 steps against the real module, no browser needed
```

In the app: add a circle and a rectangle, drag them (the frame readout under the canvas
should read `wrote 1 buffer 64 B` per frame no matter how fast you move), add a Vertical
constraint between two layers and drag one — the partner follows and the same one-write
budget holds. Click empty space: nothing is sent to the engine.

## 9. Honest gaps

1. **The browser path was not executed here.** This environment has no browser, so
   `attach` → adapter → configure → present has never run: it is `#[cfg(wasm32)]` code that
   compiles for the wasm target and is exercised *as logic* on the host, and the frame
   decision path is exercised in Node against the real artifact (smoke 25–26). The pixels
   were proven on a real device — but through a host Vulkan surface, not a canvas.
2. **No selection or hover highlight on the canvas.** The instance carries a `selected`
   slot (`params.z`) and nothing writes it yet; dragging a node highlights it in the
   Inspector and in the readouts, not in the pixels. That is Task 5.1's overlay.
3. **The document grid is a CSS overlay**, not shader output: exact only because the Phase-1
   camera is fixed (the canvas box is the document window). A pan/zoom camera must move it
   into the fragment stage — and the same goes for hairlines and the selection outline.
4. **One frame per `requestAnimationFrame`, always.** There is no damage tracking above the
   dirty set, no "skip the frame if nothing changed" switch; an idle frame is *cheap*
   (0 writes, one encoder + one pass) but it is not free, and on a laptop it is a fan.
5. **Stroke joins/caps are lyon's defaults** (`StrokeOptions::MINIMUM_MITER_LIMIT`); the
   style layer has no join/cap/miter controls yet, so the renderer picks and the user cannot.

## 10. Bugs the laws caught (fixed at the source)

1. **Stroke width was classified as style-only.** A wider outline is a *different mesh*;
   the incremental suite's "change only its colour" test failed with a vertex write, which
   exposed that `sync` compared only the shape key. Fixed by making the stroke's
   tessellation inputs (`visible`, `width`) part of the change test, with per-mesh writes so
   a widened stroke still leaves the fill's buffers alone.
2. **A far-away point hit a shape.** `HitIndex::candidates` clamped the cell coordinate, so
   a query outside the grid landed in an edge cell. Fixed to floor and reject out-of-range
   columns/rows (the grid's built-in slack covers the scene).
3. **`encase` mapped `[f32; 4]` to `array<f32, 4>`** — stride 4, which is not a legal
   storage-buffer stride, so the first instance write panicked. The instance and camera rows
   are hand-packed now (little-endian, offsets pinned by a unit test), and the dependency is
   gone from the crate.
4. **lyon winds fills clockwise**, so `signed_area()` is negative for a correctly filled
   region; three assertions were silently testing the wrong sign. Areas go through
   `Mesh::area()` now, and the sign is asserted only as a winding witness.
5. **A camera that drew y-down.** The first framing put `view = [0, 0, w, h]`, which renders
   the document upside-down relative to the engine's y-up space (and to the SVG preview it
   replaced). The flip is now the sign of `view.w`, with the screen↔document pair derived
   from it and a law asserting document `+y` points *up* the screen.
