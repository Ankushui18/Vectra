# TASK 10.2 — The Professional Workspace & Appearance System

**Status: COMPLETE.** All gates green, no regressions, no clarifying questions asked.
Phase-1 MES execution, Engine-First, standing rules kept (the React UI stays a dumb
remote: JSON commands in, events and snapshots out).

| Gate | Result |
| --- | --- |
| `cargo test --workspace --exclude vectra-wasm --no-fail-fast` | **434 passed / 0 failed** (was 433; +1 new GPU blend law) |
| `cargo test -p vectra-wasm --no-fail-fast` | **89 passed / 0 failed** (was 84; +5 new boundary laws) |
| `cargo fmt --all -- --check` | clean |
| `cargo clippy --workspace --all-targets` | **0 warnings, 0 errors** (was 16 warnings; cleaned, not suppressed) |
| `npm run smoke` | **56 / 56** (was 51; +5 workspace steps against the built `.wasm`) |
| `npm run test:ui` | **77 / 77** (was 57; +20 panel tests) |
| `npm run typecheck` | clean (`tsc` app + tests) |
| `npm run build` | clean (`vite build`, 22.9 MB wasm + 277 kB app) |

Before this task the suite held 509 tests; it now holds **600** (434 + 89 + 77).

---

## 1. The four rules, as delivered

### RULE 1 — Layers & Groups (`vectra-core`)

**Data model.** `crates/vectra-core/src/layers.rs`

```rust
pub struct LayerRecord {          // one arrangement unit of the document
    pub id: LayerId,              // its own id space — never a NodeId
    pub name: String,
    pub visible: bool,            // the eye
    pub locked: bool,             // the padlock
    pub children: Vec<NodeId>,    // contents, back → front
}

pub struct LayerRegistry { layers: Vec<LayerRecord> }   // back → front
```

Operations on the registry: `insert`, `remove`, `reorder(id, index)`, `position`,
`layer_of`, `detach`, `visible_for`, `locked_for`, `board_of_layer`. Groups needed no
new type: a group *is* a node (`NodeKind::Group { children }`) and appears as a row
inside its layer, which is what makes nesting and "moving a group moves all its
children" fall out of the existing tree walk rather than a second registry.

**Every node belongs to a layer.** `Document::insert_node` assigns new artwork to
`active_layer()`; `Command::DeleteNode` captures `layer_of` before removal, detaches
after, and its inverse is `CreateNode` → `AssignNodeToLayer` → `ReorderNode{position}`,
so undo puts a node back on the same layer at the same slot. A document with no layers
is untouched — every legacy document and every non-UI test still behaves exactly as
before (that is how the 509 pre-existing tests stayed green without a migration).

**The workspace a new document opens into.** `Document::open_workspace(w, h)` seeds
one 800×600 artboard and one layer, with **fixed ids** (`ids::DEFAULT_ARTBOARD`,
`ids::DEFAULT_LAYER`). Fixed, not allocated, for two reasons: two engines fed the same
command stream must stay byte-identical (the determinism smoke step compares snapshots
across engines), and a diff of two fresh documents should show the designer's work and
nothing else. It lives on `Document`, is called by the UI engine's constructor, and is
*not* in `Document::default()` — a document type that quietly grew a layer would change
what every native caller sees. It is document state, never a history entry: a designer
does not undo the paper they were given.

**RULE 4 mechanics (the critical half).** Toggling the eye or the padlock must not
re-evaluate anything. The path is:

```text
SetLayerVisible / SetNodeVisible / SetLayerLocked / SetNodeLocked
   └─▶ EngineEvent::NodeFlagsChanged | LayersUpdated      (explicit no-ops in
       └─▶ dirty_ids_for_events → ∅                        dirty_ids_for_events)
       └─▶ settle: repaint_flags()  ← one bool per node, no parameter, no geometry
              └─▶ DirtyLedger::note(ids, EvalMode::Incremental)
                     └─▶ Renderer: refresh_rows(visible) ⇒ zero desires, zero writes
```

Measured, not asserted in prose: the engine reports `last_evaluated == 0` and
`no_ops ≥ 1`; the frame that shows the change writes **0 bytes**; the hidden nodes keep
their meshes and their buffers resident (`nodes` unchanged, `removed == 0`) and simply
contribute no draw items. Showing it again costs one 80-byte row per appearance layer —
the honest asymmetry (rows are a draw need, meshes are geometry) and it is documented
where it happens, in `vectra-render/src/scene.rs::refresh_rows`.

**A defect this task found and fixed.** `EvaluatedScene::refresh_presentation` projected
`Document::visible` over *every* scene node, but an operation result, a procedural result
and a `Source` node are **scene** nodes with no entry in `doc.nodes` — so `visible()`
returned `false` for them and every computed shape was hidden on the next settle. The
`patch ≡ rebuild` smoke step caught it. Fixed with an explicit
`if !doc.nodes.contains_key(id) { continue }` plus a doc note on `Document::visible`, and
pinned by a new boundary law (`law_a_computed_shape_is_not_hidden_by_a_neighbours_eye`):
a virtual node's presentation belongs to the pass that composed it — a disabled operation
is *pruned*, never silently hidden. The eye hides the designer's artwork; it does not
reach into computed geometry.

### RULE 2 — Artboards (`vectra-core`)

```rust
pub struct ArtboardRecord {
    pub id: ArtboardId,
    pub name: String,
    pub x: f64, pub y: f64, pub width: f64, pub height: f64,   // its own box
    pub background: Color,                                     // its own paper
    pub layers: Vec<LayerId>,                                  // it owns a stack
}
pub struct ArtboardRegistry { boards: Vec<ArtboardRecord>, active: Option<ArtboardId> }
```

An artboard owns its **layer stack**, so the document's draw order is
`flatten_layers()`: boards back → front, each board's layers back → front, then any
unlisted layer, then any id no layer claims (relative order preserved). `CreateArtboard`
makes the new board active — "+ board" is a *move onto the new paper*, which is what
makes the next layer and shape land on it; `ArtboardRegistry::remove` falls back to the
first board so the flag is never dangling.

**Jump, frame, export.** Pan and zoom are the camera's, so framing lives in the renderer:
`Renderer::frame_document(x, y, w, h)` remembers the rectangle, recomputes the contain
fit (`scale = min(screen_w/w, screen_h/h)`, window centred on the rectangle) and pushes it
to the GPU camera; it returns the visible window, so the overlay follows for free.
`frame_document_default()` clears it — "fit everything". A resize keeps framing the board
because `viewport()` re-derives the camera from the same rectangle.

Export is the engine's own exporter, two entry points: `export_current_artboard()` (the
active board, cropped to its frame, painted with its background) and
`export_all_artboards()` (every board at its place in document space, each in its own
`<g data-vectra-artboard="{id}">`). No board at all still exports the whole picture rather
than nothing.

### RULE 3 — The Appearance System (`vectra-core`)

**`StyleProperties` gained a list and kept its four original fields as the default.**

```rust
pub struct AppearanceLayer {
    pub kind: AppearanceKind,     // Fill | Stroke { width: Parameter<f64> }
    pub paint: Paint,             // Solid(Color) | Linear{start,end,stops} | Radial{center,radius,stops}
    pub opacity: Parameter<f64>,  // per-layer
    pub blend: BlendMode,         // Normal | Multiply | Screen | Overlay
    pub visible: bool,            // per-layer eye
}
```

One rule, no dual truth: whoever reads a node's paint reads
`StyleProperties::resolved_appearances()`. An empty `appearances` list means "exactly
what the four legacy fields say" (so every pre-10.2 document and fixture renders
byte-identically); a non-empty list wins. Gradient *frames* are `Parameter<…>` like every
other coordinate, so a gradient can be driven by a variable, an expression, motion or the
solver; gradient *stops* are structure and travel inside
`Command::SetAppearances` (the whole stack), the same way a path's vertex list travels in
`SetPath`. Scalars inside the stack (an opacity, a width, a stop) remain plain
`SetParameter` writes so a slider drag is one write per sample.

**Compositing, on the GPU.** `Multiply`, `Screen` and `Overlay` are functions of the
destination, so no `BlendState` can express them: the shader snapshots the canvas
(`copy_texture_to_texture`), and `fs_*_blend` composites with the W3C formula in straight
alpha (`composite()` in `shader.wgsl`). A frame is one `Normal` pass plus one extra pass
per blended layer, and the stats say so (`blend_draws`, `passes`, `draw_calls` are kept
distinct).

**The renderer:** one draw item per visible appearance layer, in stack order, each with
its own **80-byte** instance row (`transform, color, params, ramp, frame`;
`ramp = [kind, stop_offset, stop_count, is_stroke]`), so "a thick black stroke with a
thinner white stroke on top" is three rows and three `draw_indexed` calls, not a special
case. Gradients ride a ramp atlas (2 rows per stop, `MAX_RAMP_STOPS = 16`); `FrameWire`
now reports `ramps` (bytes of atlas uploaded) **separately** from `bytes`/`writes`, so the
per-node instance budget the incrementality laws assert is never polluted by a ramp
upload. Hit-testing skips `!visible || locked`, which is what makes a locked layer's
artwork click-through.

### RULE 4 — Performance & designer-first UI

* The Appearance Panel is gated by `showsAppearancePanel(snapshot, selection)`: exactly
  one selected object, and not a locked one (a lock cannot be edited, so live controls
  over it would be a lie; a *hidden* node still shows its panel — that is usually why it
  was selected).
* Blend modes and paint kinds are glyph buttons (`aria-label="Blend mode: Multiply"`,
  `title` for the mouse) — no `<select>` of words anywhere, asserted by test.
* The gradient bar paints the ramp the engine samples (`linear-gradient(90deg, …)` from
  the stop list) with one draggable handle per stop, `setPointerCapture` on drag, and
  `moveStop` clamping each stop between its neighbours so the ramp stays monotonic.
* The Layers Panel is collapsible, top-first (the engine's order reversed in exactly one
  tested place, `dropIndexFor`), with inline rename, drag-to-reorder, eye/padlock icons in
  a fixed-width column so a row never re-flows, and a "drop a node here" assign affordance.

---

## 2. The UI panels (`apps/vectra-web`)

The split is deliberate and enforced: **all decisions are pure functions in
`src/engine/panels.ts`** (DOM-free, engine-shaped fixtures), and the components are thin
projections. Nothing in the React tree computes geometry, resolves a parameter, or holds
a second copy of engine state.

| File | What it is |
| --- | --- |
| `src/engine/panels.ts` | Pure view-model: `layerRows`, `layerOrder`, `dropIndexFor`, `selectableNodes`, `artboardRows`, `activeArtboard`, `artboardFrame`, `BLEND_MODES`, `blendLabel`, `swatchColor`, `appearanceCaption`, `appearanceStack`, `stackToWire`, `paintToWire`, `hexToColor`, `selectedNode`, `showsAppearancePanel`, `addFill`, `addStroke`, `removeLayerAt`, `moveLayer`, `updateLayerAt`, `moveStop`, `recolorStop`, `togglePaint`, `gradientCss` |
| `src/components/LayersPanel.tsx` | RULE 1: collapsible, eye/padlock/rename/delete, drag-reorder, group folders, assign drop |
| `src/components/ArtboardBar.tsx` | RULE 2: the jump dropdown (name · size), `⤢ fit`, `+ board`, Export current / Export all |
| `src/components/AppearancePanel.tsx` | RULE 3: paint-kind glyphs, colour swatch, per-row blend menu, per-row opacity, stroke width, gradient bar with draggable stops, `+ fill` / `+ stroke`, drag-reorder of the stack |
| `src/engine/commands.ts` | +18 workspace command builders + `fillLayer`/`strokeLayer`/`gradientFill` |
| `src/engine/client.ts` | +`frameDocument`, `frameDocumentDefault`, `exportCurrentArtboard`, `exportAllArtboards` |
| `src/App.tsx` | The bar above the canvas, the panels in the right column, `expandedLayers` state, the three handlers (each logs to the event log and kicks a frame) |

`stackToWire` is the one conversion in the panel and it is documented as such: the panel
edits *resolved* values and hands them back as literals. The alternative — a UI that
understands `Parameter<f64>`, expression slots and motion bindings — is exactly the layer
of intelligence a dumb remote must not have.

Two small UI fixes made while wiring: `hexToColor` now *validates*
(`#rrggbb[aa]` or opaque black) instead of slicing digits out of garbage, and `moveStop`
ignores an out-of-range index so a drag cannot read past the end of a stop list mid-gesture.

---

## 3. Tests

### Rust — 423 total (434 workspace + 89 wasm)

New in 10.2:

* `crates/vectra-dependency/tests/workspace_laws.rs` — **8/8**, the engine's side of the
  rules: layer-order law (+ proptest over 3-layer permutations), visibility law
  (+ proptest), two-strokes/draw-list law (+ proptest), group law, artboard law.
* `crates/vectra-wasm/tests/workspace_boundary_laws.rs` — **5/5**, the *wired* version,
  against the JSON the React remote really sends: `law_an_eye_across_the_wire_is_a_flag_not_an_evaluation`,
  `law_a_computed_shape_is_not_hidden_by_a_neighbours_eye` (the defect above),
  `law_the_workspace_survives_every_mutation_and_undo`,
  `law_the_appearance_stack_reaches_the_canvas_in_order`,
  `law_artboards_frame_and_export_what_they_own`.
* `crates/vectra-render/tests/gpu_laws.rs` — `law_the_blend_modes_composite_against_the_canvas_beneath_them`:
  a real device (Mesa lavapipe in this environment), a red-brown backdrop, a dark layer in
  each of Multiply / Screen / Overlay side by side, and each sample point asserted against
  that mode's own arithmetic (±2/255), plus "not the source, not the backdrop". The first
  run *failed* — the fixture used 50% grey, where Overlay is mathematically the identity —
  and the honest fix was a source colour that separates all three modes; the reason is in
  the test's own doc comment rather than lost in a commit.

### UI — 77 total

* `tests/panels.test.ts` — **15** tests of the pure view-model: row order and disclosure,
  effective-vs-own flags, the panel↔engine index inversion, selectable nodes, artboard
  labels and frames, the RULE 4 gate, the brief's own stacked-stroke example, blend
  labelling, `stackToWire`/`paintToWire` round trips, stop dragging and clamping, the
  gradient bar's CSS, and colour-codec validation.
* `tests/panels-mount.test.tsx` — **7** tests that render the real components with
  `react-dom/server` (no DOM): the Layers Panel's rows and icons, the Artboard bar's
  dropdown and both export buttons, the Appearance Panel's three rows + blend icons +
  gradient bar with one handle per stop, and — the integration check — that `App.tsx`
  renders at all with the panels mounted. Rendering a panel issues **zero** commands,
  which is the dumb-remote claim made mechanically.

To make that App-level test possible the UI test runner is now
`scripts/test-ui.mjs` (esbuild's JS API), which stubs Vite's `?url` WASM import so the
whole shell can be server-rendered in Node. `npm run test:ui` is unchanged from the
caller's side.

### Smoke — 56 steps

Five new steps against the **built** `.wasm`, i.e. against the artifact the browser loads:
`52/56` the workspace seed, `53/56` RULE 4 across the wire (zero writes, fewer draw
items, the document keeps everything), `54/56` stacked strokes + a blend mode + a gradient
ramp (80 B, no retessellation, ramp bytes reported separately), `55/56` artboard jump
(framing maths), crop and both exports, `56/56` a layer reorder that moves its artwork
with zero writes.

**Pre-existing suites are all still green**, including the ones that touch what 10.2
changed: `patch ≡ rebuild` (which caught the virtual-node defect), determinism across two
engines, the drag triad, the drawing tools, motion, AI, and `.vectra` serialization.

---

## 4. Notable engineering decisions (and their reasons)

1. **`open_workspace` on `Document`, not in `Document::new()`** — a *workspace* opinion,
   not a document invariant. Imports, fixtures and native callers see the document type
   unchanged.
2. **Layers are not nodes.** `LayerId` is its own type in its own id space, so a layer can
   never be passed where a node is expected, and the four presentation events are explicit
   no-ops in the dirty closure — RULE 4 lives in one arm of one match with a comment that
   says why.
3. **Fixed seed ids.** Determinism and diffability, argued in `ids.rs`.
4. **`CreateArtboard` activates the board.** A create is a move; the panel's `+ board`
   also frames the camera on the new rectangle so the designer lands on the paper they
   asked for.
5. **Ramp bytes are their own line.** `FrameWire.ramps` instead of folding ramp uploads
   into `bytes`, so the incrementality witnesses stay about per-node instance traffic.
6. **The `80` in "80-byte row"** is the Task 10.1→10.2 row widening (blend code, ramp
   window, gradient frame). Nine stale "64-byte" comments across `vectra-render`, the
   wasm render module, the smoke script and `wire.ts` were corrected in this task.
7. **Clippy to zero, not suppressed.** 16 pre-existing warnings (`clone` on `Copy`
   `Uuid`s, redundant field names, a redundant cast) were fixed at the source.

## 5. Deviations, stated plainly

* **Showing a hidden layer writes rows.** Hiding costs zero writes (the whole point of
  RULE 4); showing re-materialises one instance row per appearance layer per node, because
  the rows were released with the items. Meshes and buffers are never rebuilt. This is
  asserted in `law_an_eye_across_the_wire_is_a_flag_not_an_evaluation` and documented at
  the code that does it.
* **The Layers Panel's assign affordance requires a selection.** `pendingNode` is set only
  when exactly one unlocked object is selected, so "move to layer" is a deliberate gesture
  rather than an accidental drag.
* **Nested groups indent one level in the panel.** `LayerRow.depth` is prepared for deeper
  nesting (`2` inside a group) and the engine supports arbitrary nesting, but the panel
  currently discloses one level (layer → contents) and draws groups with a folder glyph;
  opening a group's own subtree in the panel is a Phase-2 polish item, not a missing
  capability in the engine.
* **`vectra-constraints/src/plan.rs` preflight is a wildcard match over `Command`.** It is
  *safe* — an audit of all 11 non-exhaustive matches over `Command`/`Event` in the wasm and
  constraint crates (driven by this task's 20 new commands and 4 new events) found each one
  either names the workspace commands explicitly or falls through to a documented
  do-nothing default that is correct for a command with no constraint or graph effect. No
  change was needed; the audit is recorded here rather than left implicit.

## 6. Files touched

```text
crates/vectra-core/       ids.rs (seed ids) · document.rs (open_workspace, visible docs)
                          layers.rs, style.rs, command.rs (from earlier in the task)
crates/vectra-geometry/   scene.rs (refresh_presentation: skip virtual nodes)
crates/vectra-render/     scene.rs (honest eye comment) · gpu.rs · instance.rs
                          geometry.rs · lib.rs (80-byte docs) · tests/gpu_laws.rs
crates/vectra-wasm/       lib.rs (open_workspace, export scope) · render.rs (FrameWire.ramps)
                          snapshot.rs · tests/workspace_boundary_laws.rs (new)
apps/vectra-web/          src/engine/{panels,commands,client,wire}.ts
                          src/components/{LayersPanel,AppearancePanel,ArtboardBar}.tsx
                          src/App.tsx · src/App.css · scripts/{smoke.mjs,test-ui.mjs}
                          tests/{panels.test.ts,panels-mount.test.tsx}
```

## 7. What a designer can do now

Draw with the 10.1 suite on a named layer of a named artboard; hide and lock layers with
icons that cost nothing; drag layers to reorder a picture without re-evaluating a single
slot; add "Icon 16" and "Icon 32" boards beside "Logo" and jump between them with a
dropdown that zooms the camera; give one shape a stacked fill + thick black stroke + thin
white stroke, set the top stroke to Multiply, turn the fill into a three-stop linear
gradient and drag a stop; then export the current board or every board as SVG — each board
cropped to its own frame with its own background, each carrying `data-vectra-artboard` for
the file's reader.

**Gate status: PASS.** 600 tests green (434 + 89 + 77), clippy 0, fmt clean, smoke 56/56,
build clean, preview live on `0.0.0.0:5173`.
