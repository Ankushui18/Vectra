# TASK 10.3 — Canvas Navigation & Workspace Depth

**The report-back.** Task 10.3 closes the three things Task 10.2's own RULEs asked
for that the UI could not yet do: RULE 2's **pan and zoom**, RULE 1's **nested
groups in the panel**, and RULE 2's **artboard box and background editing**. All
three were built engine-first: the camera arithmetic lives in `vectra-render`, the
gesture entry points in `vectra-wasm`, the group contents in the snapshot, and
React only draws what the engine says.

The task's own rules, in the brief's words:

| # | Rule | Where the truth lives |
|---|---|---|
| 1 | Nested groups are visible as a tree in the Layers Panel | `SnapshotLayer::child_groups` (`vectra-wasm/snapshot.rs`) → `layerRows` (`panels.ts`) → `LayersPanel.tsx` |
| 2 | Pan and zoom between artboards, with the pointer still exact | `Camera::{zoom_at,pan_by,zoom_to}` (`vectra-render/instance.rs`) → `Renderer::nav_*` (`vectra-wasm/render.rs`) → wheel / space-drag / zoom controls |
| 3 | An artboard's box and background are editable | `SetArtboardBounds` / `SetArtboardBackground` (existing engine commands) → the artboard bar's four numbers + hex field |
| 4 | The picture, the pointer and the grid never disagree | one camera: shader projection, `client_to_document`, `document_to_client`, `hit_test`, and the CSS grid all read the renderer's `view()` |

---

## 1. Navigation (RULE 2) — engine first

### 1.1 The camera gained four total, pure methods

`crates/vectra-render/src/instance.rs`, on `Camera` (which already owned the
projection, `document_to_screen`, `screen_to_document` and `visible`):

| method | meaning |
|---|---|
| `scale()` | screen pixels per document unit — one number, because the camera is uniform |
| `zoom_at(px, py, factor)` | zoom about a screen point: the document point under the pointer stays under it |
| `pan_by(dx, dy)` | pan by a **pointer's** delta (+x right, +y down), y-flip applied exactly once |
| `zoom_to(scale)` | absolute zoom about the canvas centre; `zoom_at` with the centre for a pointer |
| `MIN_SCALE` / `MAX_SCALE` | 1% … 6400%, as named constants the UI and the laws both read |

Each is **total** (a NaN, an infinity, a zero or a negative factor returns the
camera unchanged) and **pure** (a gesture is a fold over samples, so a wheel burst
is one value rather than a state machine). The clamped factor is the factor
*applied*, so a wheel already at a limit is exactly a no-op rather than a slow
drift.

### 1.2 The renderer remembers where it was pointed

`Renderer::view_state` replaces the old `framed: Option<(x,y,w,h)>` with one state
that has two names, because there are two ways a user aims a camera and they must
survive a resize differently:

* `ViewState::Fit { x, y, width, height }` — "show me this rectangle" (the artboard
  jump and *fit*). Re-derived on every resize: resizing a window must not lose the
  artboard you are working on.
* `ViewState::At { center_x, center_y, scale }` — "I am here, at this zoom" (a pan
  or a wheel). Kept across a resize — the canvas shows *more or less* document
  instead of rescaling it, which is what every editor does — and the stored zoom
  is **CSS** pixels per unit, so moving a window between a 1× and a 2× display does
  not silently double or halve "100%".

One reader (`camera_for_screen`) and one writer (`set_camera`) keep the field, the
GPU's uniform and the overlay's JSON from ever disagreeing.

### 1.3 The gestures, at the wasm boundary

`Renderer::nav_pan(dx, dy)`, `nav_zoom(client_x, client_y, factor)`,
`nav_zoom_to(scale)` (CSS pixels per unit), `nav_scale()` — each returns the visible
window as JSON, so the overlay is *told* where the camera is instead of computing
it. Pointers arrive in **CSS** pixels and are converted once, here, exactly as
`write_pointer` and `client_to_document` already convert them.

Two defects this boundary work caught, both now pinned by laws:

* **`nav_zoom_to` was passing a CSS scale into a device-scale camera.** At dpr 1.5,
  "100%" rendered as 66.7%. Caught by the readout law's dpr matrix.
* **Gestures moved a canvas that had never been measured.** With no `viewport`, a
  wheel re-derived a camera from an invented screen size. Now `navigation_ready()`
  refuses the gesture whole — the same honesty `client_to_document` already showed
  by answering `None`.

### 1.4 The UI

* **Wheel** — a native, non-passive listener on the canvas (React attaches wheel
  handlers passively, where `preventDefault` is a no-op and the page scrolls *while*
  zooming). `wheelZoomFactor` is exponential in scroll distance and unit-aware
  (pixel / line / page), so a trackpad's 2-pixel stream and a mouse's single notch
  land on the same zoom; one event is capped at ±4×.
* **Pan** — middle-button drag, or Space + drag (`panGestureAllowed`). Space is
  tracked globally as a *modifier*, and deliberately ignored when the key is aimed
  at a control, so it stays the activation key for every button in the workspace.
  The pan takes the pointer in the capture phase, so the active tool never also
  sees it.
* **Zoom controls** — `−`, the readout (which is also the 100% button), `+`, `⤢ fit`
  in the canvas's top-right; `+`/`-`/`0`/`1` on the keyboard. Relative for the
  buttons, absolute for the readout: a label saying 240% means 240%.
* **The navigation overlay** (`NavigationOverlay.tsx`) — each artboard outlined and
  tinted with its own background colour, labelled with its name and size, the
  visible window marked with a dashed rectangle. Every corner comes from
  `client.documentToClient`, i.e. from the renderer's camera; the component owns no
  transform, so the y-up/y-down flip cannot be re-derived (and re-broken) here.
* **The grid follows the camera** — 50 document units per cell, offset by the
  visible window's origin. A grid that stayed put while the artwork panned would be
  a wrong ruler, which is worse than no ruler.

---

## 2. Nested groups in the panel (RULE 1)

The engine has always nested (`NodeKind::Group { children }`, and "moving a group
moves its children" is a `workspace_laws` law). What was missing was the
*disclosure*: the snapshot listed a layer's children and stopped.

`SnapshotLayer` now carries two more parallel arrays and one projection:

```rust
pub child_can_open: Vec<bool>,          // a group WITH contents — the triangle
pub child_groups: Vec<SnapshotGroupRow>, // each child's own contents
```

`SnapshotGroupRow { ids, names, is_group, can_open, layer }` is the second level:
the ids inside a group, their display names, whether each is itself a group (the
icon), whether it opens (never, at this depth), and **the layer each row lives in** —
which is not always the layer row above it, and is why the engine reports it rather
than the panel guessing.

Decisions worth stating:

* **`can_open` is not `is_group`.** A folder with nothing in it gets no disclosure
  triangle; a triangle that opens onto emptiness is a lie about the document.
* **The projection is two levels, deliberately.** Two is exactly what this
  workspace can create (a group inside a group). A group nested a third time — only
  reachable from hand-written command JSON — shows as a row with its group icon and
  no disclosure: the panel says nothing about contents it was not given instead of
  flattening them and implying the wrong parent. Widening to arbitrary depth means
  a flat row table with parent links — a different data shape for the panel, and its
  own task (recorded as open, §5).
* **One open-set drives both levels.** Row ids are group node ids and layer ids, so
  "opened" means *this id shows its contents* and nothing about the container's
  kind.
* **A stale id is inert.** The same set survives deletions without a pruning pass.

In the UI, `layerRows` (pure, DOM-free) produces the display list with depth, the
triangle is a real button with `aria-expanded` and an accessible name, and the two
levels indent by the engine's own depth. That gave `LayerRow` a `layerId`, which
also fixed a latent bug: dropping a selected node onto a *node* row previously did
nothing, because the panel looked its layer up by the node's own id.

---

## 3. Artboard box and background (RULE 2)

The engine's `SetArtboardBounds` and `SetArtboardBackground` already existed; the
artboard bar now uses them. The detail row gained four small number fields and a hex
field (`artboard-x`, `artboard-y`, `artboard-width`, `artboard-height`,
`artboard-background`):

* **Edits in progress are local.** A half-typed `12` in the width field must not
  resize the board under the designer's hand nor enter the undo history, so the
  fields hold a draft and commit on Enter/blur.
* **A refused commit is a silent non-event.** An empty or non-positive size is not
  a small board, it is a missing one, so it never reaches the engine; the same for a
  colour that is not `#rrggbb[aa]`.
* **Normalised comparisons.** Re-typing `#FFFFFF` over `#ffffff` is not a command
  and not an undo entry.

---

## 4. Tests

### Rust — 537 (441 workspace + 96 wasm)

| suite | count | what it pins |
|---|---|---|
| `vectra-render/tests/camera_laws.rs` | **7 new** | screen↔document exact inverses; the zoom anchor stays still; pan translates and composes; the clamp is exact and a wheel at a limit is a no-op; degenerate numbers never move the camera; framing still means one pixel per unit; zoom-then-inverse returns the view |
| `vectra-wasm/tests/navigation_laws.rs` | **7 new** | the wheel anchor *through the port*; a pan drag (20 samples) follows the pointer in client coordinates; the readout is CSS pixels per unit at dpr 1/1.5/2 and one client pixel is one unit; **a gesture never moves the shape away from the click** (`hit_test` after pan+zoom); a gesture survives a resize and a jump re-fits; the clamp holds; an unmeasured canvas ignores gestures |

Two of those laws are stated with the tolerance they deserve: "the same point"
means *the same pixel*, so comparisons are sub-pixel, scaled by the camera's own
zoom (a document-unit tolerance is meaningless at 1% and impossible at 6400%). And
where the law is about *meaning* rather than arithmetic — `pan_by(a).pan_by(b)` vs
`pan_by(a + b)` — the comparison is sub-pixel, not byte equality: pinning the
compiler's rounding is not the same as pinning the camera.

**Two real defects were found by these laws** (§1.3) and fixed; the failures were
reported before the fixes, not after.

### UI — 85 (was 77)

* `tests/panels.test.ts` (+5): a group row opens into its contents at depth 2 with
  the panel's own top-first order; `can_open` is not `is_group`; a nested row
  carries its layer; a stale id is inert; the zoom label's rounding rule
  (`240%`, `100%`, `12.5%`, `8.3%`, `—`); the wheel's arithmetic (direction, unit
  modes, exponential composition, sign symmetry, the per-event cap); which presses
  start a pan; the grid follows the camera.
* `tests/panels-mount.test.tsx` (+3): an opened group renders its members one
  indent deeper with `aria-expanded="true"`; the navigation overlay draws a rect
  per board, tinted with the board's own colour and labelled, plus the window and
  the icon controls (and an unmeasured camera reads `—`, never 100%); the artboard
  bar's five editors exist and show the engine's numbers.

### Smoke — 59 (was 56)

| step | what it proves |
|---|---|
| 57 | **Pan and zoom.** Five wheel notches keep the document point under the pointer; the readout equals 1.2⁵; a 20-sample pan puts the grabbed point exactly under the pointer (y-flip included); after all of it, `pointer_hit` on the pixel a shape is *drawn* on still selects that shape, and empty space still misses; `nav_zoom_to(1)` is 100% on a 2× display; an unmeasured canvas refuses |
| 58 | **Nested groups.** A group inside a group appears as a row with `child_can_open`, whose `child_groups` names its members, flags the one that is itself a group, reports `can_open: false` (the projection stops there and says so), and gives each row its layer |
| 59 | **Board box and colour.** `SetArtboardBounds` / `SetArtboardBackground` both emit `Dirty { ids: [] }` — a board's box is not a value any node reads; the jump frames the new box; the export paints the new background for the right board and still contains the artwork |

### Gate status (all re-run after the last edit)

```
cargo fmt --all --check          clean
cargo clippy --workspace        0 warnings
cargo test --workspace          537 passed, 0 failed
npm run typecheck               clean (app + tests)
npm run test:ui                 85/85
npm run smoke                   59/59
npm run build                   OK — js 286.13 kB (87.21 kB gzip), css 21.94 kB, wasm 22.96 MB (2.65 MB gzip)
```

**Project total: 622 tests** (537 Rust + 85 UI); 59 smoke steps on top.

---

## 5. Deviations, stated plainly

1. **The group projection is two levels deep** (§2). Complete for everything this
   workspace can create; a deeper tree (hand-written JSON only) shows the group row
   and does not enumerate its contents — disclosed on the wire type and here rather
   than flattened into a wrong-looking tree. **Open**, and priced: it needs a flat
   row table with parent links.
2. **The stored gesture zoom is CSS pixels per unit**, so a dpr change keeps the
   *readout* rather than the physical size. Chosen because "100%" is a statement
   about what the user sees and what the pointer does; the alternative would make a
   monitor change quietly alter the zoom a designer set.
3. **The CSS grid is drawn from the camera rather than in the shader.** It is
   decorative, and the view rectangle it uses is the same one the shader projects
   with, so it cannot disagree — but it is a second drawing path, and moving it into
   `shader.wgsl` is the honest long-term home.
4. **`nav_zoom_to` takes CSS scale while `Camera::zoom_to` takes device scale.** The
   conversion is deliberate and documented at the boundary; it was also a real bug
   for one commit (§1.3), which is why the law asserts it at three dprs.
5. **Space is a pan grip only when it is not aimed at a control** (§1.4). Inside a
   text field or on a focused button, Space stays what the platform says it is.
6. **No new engine commands.** Navigation is not document state: there is no
   `SetFocus` command and no undo entry for moving a camera, because undo must
   unwind *the designer's work*, not their view. Pan and zoom reach the renderer
   directly, which is where the camera already lived.

---

## 6. Files touched

**Rust**

* `crates/vectra-render/src/instance.rs` — `Camera::{scale, zoom_at, pan_by, zoom_to, MIN_SCALE, MAX_SCALE}`.
* `crates/vectra-render/tests/camera_laws.rs` — **new**, 7 laws.
* `crates/vectra-wasm/src/render.rs` — `ViewState` (Fit / At), `camera_for_screen`
  rewrite, `set_camera`, `navigation_ready`, `css_to_device`, `css_point`,
  `nav_pan`, `nav_zoom`, `nav_zoom_to`, `nav_scale`, `remember`.
* `crates/vectra-wasm/src/snapshot.rs` — `SnapshotGroupRow`,
  `SnapshotLayer::{child_can_open, child_groups}`, `group_children`.
* `crates/vectra-wasm/tests/navigation_laws.rs` — **new**, 7 laws.

**Frontend**

* `src/engine/wire.ts` — `SnapshotGroupRowWire`, the two new layer fields.
* `src/engine/panels.ts` — nested `layerRows`, `grandchildRow`, `LayerRow.{canOpen, layerId}`,
  `formatZoom`, `wheelZoomFactor`, `panGestureAllowed`, `ZOOM_STEP`, `GRID_UNITS`,
  `gridStyle`, `overlayBoards`, `NavView`.
* `src/engine/client.ts` — `navPan`, `navZoom`, `navZoomTo`, `navScale`.
* `src/components/NavigationOverlay.tsx` — **new**.
* `src/components/LayersPanel.tsx` — the second level's disclosure, `layerId`-based drops.
* `src/components/ArtboardBar.tsx` — the box and background editors.
* `src/App.tsx` — view/zoom state, `adoptView`, wheel listener, pan triad, zoom
  callbacks, keyboard, overlay and grid wiring.
* `src/App.css` — overlay, navigation controls, board editors.
* `tests/panels.test.ts`, `tests/panels-mount.test.tsx`, `scripts/smoke.mjs`.
* `src/wasm/*` — regenerated by `scripts/build-wasm.sh`.

---

## 7. What a designer can do now

1. **Zoom into the corner being refined.** Wheel over the point you are looking at:
   that point does not move. `Ctrl`-free, no tool to select first.
2. **Throw the canvas around.** Middle-drag, or hold Space and drag; the artwork
   follows the pointer exactly, at any zoom.
3. **Never get lost.** Every artboard is outlined, tinted with its own background
   and labelled with its size, so "which board is that" and "where did my artwork
   go" are answered by the canvas itself. `0` fits everything.
4. **Read the zoom.** The readout is the renderer's own number, and pressing it
   returns to a true 100% — at any device pixel ratio.
5. **See groups inside groups.** Open a layer, then open a group inside it: the
   members appear one indent deeper, with their own layer.
6. **Edit a board.** Four numbers and a hex colour, committed on Enter, costing no
   re-evaluation, and visible immediately in the jump and in the export.
7. **Trust the click.** After any pan and any zoom, the pixel you click is the pixel
   the engine resolves: the drawing overlay, the selection hit-test and the CSS grid
   all read the one camera.
