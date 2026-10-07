# TASK 10.7 — The "Procreate" Interaction & Workflow Layer

Four rules, one idea: **the tools get out of the way.** A gesture replaces a menu
bar, a filter replaces a steady hand, a layer flag replaces a lasso, and a drag
replaces a fill dialog. Every one of them lands on machinery that already
existed — the `Command` bus, `LayerRecord`, `EvaluatedScene`, the renderer's
`hit_test` — which is the only reason four features this different can ship as
one task.

| layer | what it owns | where |
|---|---|---|
| `vectra-web` (UI) | the gesture recogniser, the StreamLine filter, the ColorDrop drag, the two new panels' controls | `apps/vectra-web/src/engine/draw/{touch,streamline,colordrop}.ts`, `App.tsx`, `components/DrawSettingsPanel.tsx` |
| `vectra-core` | the two layer flags, the two commands | `crates/vectra-core/src/{layers,command}.rs` |
| `vectra-operations` | the clip mathematics: region ∩ mask, the 1-D clip, the live pass | `crates/vectra-operations/src/clip.rs` |
| `vectra-wasm` (engine) | the drawing boundary (alpha lock at commit) and the settle-time clip pass | `crates/vectra-wasm/src/{draw,lib,snapshot}.rs` |

The environment caveat, stated up front because it shaped the work: the sandbox
restarted mid-task and took `/tmp` with it (toolchain, vendored crates, cargo
home). The toolchain was re-materialised from the PyPI bundle and the crate
closure re-vendored before any of the numbers below were produced — §6.1 has the
evidence, including the one number that changed (239 crates, not 256 — same
lock, a better donor hit; `missing 0` either way).

---

## 1. RULE 1 — the invisible UI

### 1.1 The mapping, literally as specified

| gesture | command | fires |
|---|---|---|
| **2-finger tap** | `Undo` | on the lift of the *last* contact |
| **3-finger tap** | `Redo` | on the lift of the *last* contact |
| **3-finger swipe down** | `Copy`/`Paste` | **live**, the moment 40 px of downward travel is crossed |
| **Ctrl + click** | `Undo` | on release |
| **Ctrl + Shift + click** | `Redo` | on release |

The engine has no clipboard command, and it does not need one: its copy
primitive is `DuplicateNode`. A paste is therefore a duplicate placed above its
source, the copies become the selection, and a second swipe duplicates the copies
— the ordinary "paste again". `App.runCopyPaste` says so in the log
(`⇩ copy/paste · duplicated 2 objects`) and pushes the new selection into the
engine so a prompt typed afterwards means the copies.

### 1.2 The recogniser (`engine/draw/touch.ts`)

A **class**, because a gesture is state across events, and because that state is
what the laws test. Constants, all in screen pixels/ms:

```text
TAP_MAX_MS   = 320     // short enough to exclude a press-and-hold
TAP_SLOP_PX  = 12      // fingers roll; a hand that travels is a drag
SWIPE_MIN_PX = 40      // downward travel that makes a swipe a swipe
SWIPE_CROSS_PX = 40    // sideways drift past which it is a different gesture
```

Three decisions worth naming:

* **The tap is decided on the high-water mark.** A three-finger tap whose middle
  finger lands 30 ms late is still a Redo (`maxContacts`), not a two-finger
  Undo followed by nothing.
* **A tap is decided at the last lift, not on a timer.** No grace period, no
  pending state: a gesture that is still undecided when the hand leaves is
  simply not a tap.
* **`cancel` kills the episode.** A pointer the browser takes away (a window
  blur, an OS takeover) can *never* come back as an Undo when the remaining
  finger lifts. That is the one outcome a designer cannot predict, so it is the
  one outcome the recogniser forbids outright.

### 1.3 "CRITICAL: must not trigger the context menu or scroll"

Three mechanisms, because the browser decides a scroll in three places:

| what | where | why there |
|---|---|---|
| the canvas owns the pointer | inline `touch-action: none` on `<canvas>` | a two-finger drag is the browser's *scroll* gesture, decided at the viewport before any `pointermove` reaches React |
| the menu is refused at the pane | `.canvas-wrap`'s `onContextMenu={e => e.preventDefault()}` | one swallow covers the canvas **and** every overlay inside it, including ones added later |
| the page cannot rubber-band | `html, body { overscroll-behavior: none }` | a gesture that overshoots the canvas edge cannot scroll the workspace behind it |

…and the fourth, the one the recogniser owns: **every decision that fires is
`swallow: true`**, and the canvas calls `preventDefault()` on it. The law
(`every firing decision is swallowed`) asserts exactly that, for all three
gestures, so the property survives a refactor of the routing.

### 1.4 Wiring, and what a gesture does to a stroke already in flight

* `onCanvasDown` offers every contact to the recogniser **before** the tools see
  it. The second finger cancels the half-drawn stroke (`cancelGesture()` — the
  engine's session is cancelled, never committed) and the tools are told nothing
  until the hand lifts.
* `onDrawMove` offers every touch move to the recogniser first; once two contacts
  are down the swipe belongs to the recogniser and the tool loop returns early.
* Ctrl+click is *armed* on press and decided on release, because a Ctrl-*drag* is
  not a click: `modifierClick` re-reads the modifiers at release and refuses a
  pointer that travelled past `DRAG_SLOP_PX`. Cmd is deliberately **not**
  accepted (on macOS ⌘-click is the ordinary secondary modifier).
* Every firing gesture is logged in the designer's words — `⇱ two-finger tap →
  undo` — because an invisible UI that also fails invisibly is worse than none.

---

## 2. RULE 2 — StreamLine ("smooth hand")

### 2.1 The mathematics (`engine/draw/streamline.ts`)

Two stages, both deterministic, both public so the slider can quote them:

```text
amount  s = clamp(slider / 100, 0, 1)                    (NaN → 0)
window  w = 1 + round(s · 5)                             → 1 … 6 samples
weights wᵢ = 1 / (1 + i)   (newest i = 0 ⇒ 1, then ½, ⅓, …)
            Σ wᵢ xᵢ
stage 1  p = ────────        over the last w raw samples
            Σ wᵢ
stage 2  lag cap c = s · 28 px;  p ← hand + (p − hand) · min(1, c / |p − hand|)
release  flush() → the filtered endpoint, then the raw last sample
```

* **0 % is the identity** — `push` returns the caller's own numbers, not a copy
  with a rounded corner, and `flush()` returns `[]`. This is the rule's "0 % =
  raw input" made literal, and it is why the pre-10.7 behaviour is the 0 % case
  rather than a separate code path.
* **Stage 2 is a limit, not a bonus.** The average *is* the smoothing; the cap
  only stops it trailing the hand further than the slider promises — and it
  handles the stylus that jumps without a second rule.
* **The filter sits in front of the tools.** `onCanvasDown` builds one
  `StreamLine` per stroke from the slider; `drawGestureMove` pushes the raw
  document point and hands the *filtered* point to `DrawSession.extend`, which
  is what `moveIntent` sends to the engine. The pen and the brush both receive
  smoothed input, so the engine's `BrushStrokeBuilder`/`PenTool` never see the
  shake. The raw client position still drives the *slop* test — that is a
  screen-space question, and it stays one.
* **The tail is flushed at release**, or the stroke would end where the filter
  was rather than where the hand stopped.

### 2.2 The slider

A real `<input type="range">` (0–100, step 1) in the Pen/Brush settings panel,
with a readout built from the same three numbers the filter uses:

```text
StreamLine 50% · 4-sample pull · ≤14px lag
```

`streamlineSummary(percent)` is the only place those words are written, so the
panel can never describe a different filter from the one running.

### 2.3 The StreamLine Law, measured

`tests/task-10-7-procreate.test.ts` drives a straight line with a hand's shiver
(±6 units of noise on a 4-unit stride, 120 samples) and measures **total
turning** — the sum of the angles between consecutive segments, which is the
honest definition of "jagged" (a position variance would call a perfectly
straight diagonal noisy):

| slider | turning (rad) | vs raw |
|---|---|---|
| raw (0 %) | 130.757 | — |
| 25 % | 82.032 | −37.3 % |
| 50 % | 67.226 | −48.6 % |
| 75 % | 60.587 | −53.7 % |
| 100 % | 59.112 | −54.8 % |

The law asserts three things about that table: every amount is **≥25 % smoother**
than raw, smoothing is **monotone** in the slider, and at 100 % the turning is
**less than half** the hand's. It also pins the lag contract (`|filtered − hand|
≤ s · 28 px` at every sample of every amount), the 0 % identity, the
one-sample-tap case, the tail, and the summary's arithmetic.

**The variance, too, because the brief asks for it in that word.** Turning is the
better proxy for "jagged", but "significantly smoother path (**lower variance**)"
is the criterion as written, so a second law measures the variance itself:
`pathJitter` — the mean squared *second difference* of the polyline, i.e. the
discrete curvature. Second differences are the right instrument here for a
specific reason: they are blind to any constant offset **and to any linear
trend**, and the filter trails the hand by design, so a plain variance of
*positions* would measure the lag (tens of units) and call a perfectly smooth
stroke noisy. Curvature sees only the shake.

Three different hands (seeds `5eed`, `1234`, `beef`), because a law that holds for
one seed is a coincidence — the filter has no idea which noise it is being fed:

| slider | jitter (% of the hand's) | smoother by |
|---|---|---|
| raw (0 %) | 100 % | — |
| 25 % | 24.5 – 25.5 % | **≈ −75 %** |
| 50 % | 13.0 – 13.4 % | **≈ −87 %** |
| 75 % | 10.8 – 11.0 % | **≈ −89 %** |
| 100 % | 9.2 – 9.7 % | **≈ −90 %** |

The law asserts every amount is **at most half** the hand's variance, that the
variance is **monotone** in the slider, and that at full strength it is **≤ 20 %**
of raw — on all three seeds.

---

## 3. RULE 3 — Alpha Lock and Clipping Masks

### 3.1 Two flags on `LayerRecord`, exactly as asked

```rust
pub struct LayerRecord {
    pub id: LayerId, pub name: String,
    pub visible: bool, pub locked: bool,
    pub alpha_locked: bool,     // Task 10.7 RULE 3a
    pub clipping_mask: bool,    // Task 10.7 RULE 3b
    pub children: Vec<NodeId>,
}
```

Both are `#[serde(default)]`, so a document written before Task 10.7 still
loads. Both are toggled by **commands of their own** — `SetLayerAlphaLocked`,
`SetLayerClippingMask` — each one history entry, each emitting `LayersUpdated`
and **zero dirty ids**: toggling a lock resolves no parameter, so the eval panel
shows a no-op while the behaviour changes, and the smoke run proves it
(`Dirty.ids == []`).

### 3.2 Alpha Lock (3a) is enforced at the *drawing boundary*

Alpha lock is not a scene reshape — it constrains what may be *added*.
`draw.rs::commit_draft` therefore hands every draft through `alpha_lock` first:

```text
no alpha-locked active layer   → the draft is untouched
area(draft) > FLATTEN_TOLERANCE → the 2-D clip:  region(draft) ∩ region(layer)
                                 (the largest piece; empty ⇒ a refusal)
otherwise (an open path)        → the 1-D clip:  the stretches of the curve that
                                  are inside the layer, longest run ≥ 2 points
```

Three implementation notes that matter:

* **Which clip runs is decided by the draft's own area**, not by the tool's name,
  so a closed pen path gets the boolean and an open one gets the 1-D clip with
  neither tool special-cased.
* **A stroke with nowhere to land is refused in a sentence** —
  `alpha lock: "Ink" has no artwork here for the new stroke to land on` — never
  stored as an empty path, which would look like a bug.
* **Straight runs are sampled at a bounded gap** (4 document units, ≤256 steps;
  curves are subdivided 24×). This was a real gap the first test run found: two
  anchors a hundred units apart stepped clean over a piece of artwork, so a line
  drawn *across* the layer was refused for having nothing to land on. A
  straight-run sample list is what makes the 1-D clip mean what a designer
  expects.

### 3.3 Clipping Mask (3b) is live geometry

A layer flagged as a clipping mask shows only where it overlaps the layer below.
That *is* a scene reshape, and it happens in `settle`, last:

```text
mask  M = ⋃ { region(n) | n ∈ layer below, visible }      mask_region
shown   = region(node) ∩ M                                 clip_region
```

* **The bottom layer clips to nothing** — `Layers::below` is `None`, so the panel
  disables the toggle and says why ("Nothing below this layer to clip to") rather
  than letting a designer switch on a mask that hides everything.
* **A hidden layer below lends no alpha**: a layer asks for its own eye before it
  masks anything away.
* **The pass is non-compounding.** A clipped node's primitive is a *derivative*,
  so `ClipState` keeps a **baseline** per clipped node — the primitive the
  document produced — and recomputes `baseline ∩ mask` from scratch on every
  pass. Baselines are refreshed only for ids the evaluator just re-derived.
  Without that map, a growing mask would shrink the artwork forever
  (`clip(clip(x, M₁), M₂) ≠ clip(x, M₂)`), which is precisely what the
  **Non-Compounding Law** tests.
* **Undoing the flag restores the document's own geometry** — through the
  evaluator, not from a saved copy (`pending_restores` names the ids, `patch`
  re-derives them), so the restore is byte-for-byte and free of a second source
  of truth.

### 3.4 The panel

Both flags are glyph toggles on the layer row, in the house style (icons with the
name in `title`/`aria-label`, never a raw enum):

| control | glyph | state | note |
|---|---|---|---|
| `layer-alpha-${id}` | `α` / `a` | `aria-pressed` | "Lock alpha — new strokes stay inside what this layer already holds" |
| `layer-clip-${id}` | `▤` / `▥` | `aria-pressed` | disabled on the bottom layer, with the reason in the title |

The snapshot carries `alpha_locked`, `clipping_mask` and `clipped_to` per layer,
so the panel needs no second call and formats no ids.

---

## 4. RULE 4 — ColorDrop

The colour well in the Pen/Brush panel is a **swatch you drag**, not a button you
press: pressing starts a drag, a chip follows the pointer, and the drop is decided
on release against the renderer's own hit test — the same index a click uses, so
no UI-side region maths exists to go wrong.

```text
nodeId = renderer.pointer_hit(clientX, clientY)      // null = empty space
plan   = planColorDrop(nodeId, colour)
       ⇒ fill   when a node answered and the colour is #rrggbb[aa]
       ⇒ noop   otherwise ("empty space — nothing to fill", "not a colour: …")
stack  = the node's resolved appearance stack
       → the bottom-most solid fill is recoloured in place;
         a gradient is left whole and a solid fill is inserted *under* it;
         no fill at all ⇒ one is appended on top.
```

Both refusals are deliberate: a drop in empty space does nothing (as specified),
and a malformed colour never reaches the wire (the engine's colour parser is
strict). A drop of the colour the shape already has is refused *before* the
command, because the engine would faithfully record an undoable no-op — a trap
for the designer pressing undo twice.

The whole thing is one `SetAppearances` command; the toast is
`◧ ColorDrop · filled Square with #e8622c`.

---

## 5. Tests

### 5.1 Rust — **589 passed, 0 failed** (Task 10.6 closed at 576)

Thirteen new laws, all running the real pipeline:

| file | law | what it pins |
|---|---|---|
| `crates/vectra-operations/tests/clip_laws.rs` (8) | **Alpha Lock Law** (proptest, 48 cases) | the clip is inside the content, no bigger than the probe, and *is* the intersection |
| | Pen Stroke Law | `Σ len(runs) == #{points inside}`, order preserved |
| | Clipping Mask Law | a clipping layer is reshaped, the layer below is untouched |
| | Bottom Layer Law | nothing below ⇒ nothing shows |
| | Non-Destructive Law | clearing the flag restores the document's geometry byte-for-byte |
| | **Non-Compounding Law** | a *growing* mask lets the artwork back in (fails if the baseline map is dropped) |
| | Idempotence | a second pass with no edit changes nothing |
| | Empty Locked Layer | refuses the stroke rather than storing emptiness |
| `crates/vectra-wasm/tests/alpha_lock_laws.rs` (5) | **Alpha Lock Law at the protocol boundary** (proptest, 24 cases) | a painted stroke through the real brush door is inside the layer's artwork — or refused |
| | Pen Stroke Law (1-D) | every stored point is inside; the stretch before the artwork is gone |
| | Unlocked Means Untouched | with the flag off the same stroke keeps its own geometry |
| | Nothing To Land On | an empty locked layer refuses, writes nothing |
| | Flag Round-Trip | both flags survive the snapshot and undo in order |

The proptest's tolerance deserves a footnote, because it is the kind of thing
that quietly becomes a lie: the generated cases put rectangles at fractional
coordinates, and a lyon path stores `f32` — at x ≈ 200 the representation error
alone is ~1e-5, so a 4×1-unit sliver carries an area error of ~4e-5, four hundred
times the strict `1e-6` bound the integer fixtures use. The law now compares
generated geometry at `1e-4` relative (**still two orders of magnitude tighter
than any real defect** — a compounding clip, a wrong mask, an empty result) and
keeps the strict bound for everything on nice numbers. The seed that found this
is saved in `clip_laws.proptest-regressions`.

### 5.2 UI — **128 passed, 0 failed** (Task 10.6 closed at 99)

The **no-scroll law** is the brief's CRITICAL clause as a *universal*, not as two
examples: for each of 300 generated hands (2–4 contacts, random positions, a tap
or a swipe), **every** decision the router returns that carries an action must
carry `swallow` — and the law also asserts that all three of `undo`, `redo` and
`copyPaste` fired during the sweep, so the universal is not vacuous for any
gesture. `swallow` is what `App.tsx` turns into `event.preventDefault()` at its
three call sites. The three-finger swipe is the one that matters most: it is what
a browser would otherwise read as a page scroll, and it fires mid-drag.

| file | tests | laws |
|---|---|---|
| `tests/task-10-7-procreate.test.ts` | 22 | **Gesture Law** (2-finger tap ⇒ Undo over 200 randomized hands; 3-finger ⇒ Redo; a slow press / a travelling hand / one finger / a cancelled contact never fire; the swipe fires once, live, and swallowed; Ctrl and Ctrl+Shift), **StreamLine Law** (§2.3, in both instruments), **ColorDrop Law** (fills the shape, does nothing in space, recolours the visible fill, no-ops on the same colour, adds a fill to a shape that has none) |
| `tests/task-10-7-panels.test.tsx` | 7 | the slider is a real 0–100 range input whose readout is the filter's own summary; the well is a draggable swatch; the two flag toggles render their state as glyphs; the clip target is the layer below, and the bottom row is disabled |

`tests/panels.test.ts`'s fixtures gained the three new layer fields, which is how
the compiler found every place a `SnapshotLayerWire` is constructed by hand.

### 5.3 Smoke — **71 steps, `SMOKE PASS`** (Task 10.6 closed at 68)

Three new steps drive the **rebuilt** binary end to end:

* **69** — alpha lock: the toggle round-trips and dirties nothing; a sweep in
  empty space is refused with `/alpha lock/` in the error and no node id; a
  sweep that crosses the artwork's edge commits, and **not one point of the
  stored geometry is past the edge** (nor outside it sideways).
* **70** — clipping mask: the row names the layer below, the bottom row names
  nothing; the upper node becomes a derived path whose coordinates lie inside the
  shared 50×50 corner, the layer below keeps its rect and its width; clearing the
  flag gives the document's primitive back byte-for-byte.
* **71** — ColorDrop's engine half: the renderer's `pointer_hit` finds the square
  and answers `null` for empty space; one `SetAppearances` fill comes back as
  `{ type: 'solid', color: '#e8622c' }`.

### 5.4 Do the laws have teeth?

A law that cannot fail is decoration, so the two smoothing laws were **mutation
tested**. The mutant: `StreamLine.push` returns the raw point unconditionally — a
plausible regression ("just skip the averaging, it is only cosmetic"). It is
caught immediately:

```text
not ok 73 - StreamLine Law: the filter smooths a shaky hand, and more is smoother
not ok 76 - StreamLine Law: the smoothed path has lower variance than the hand
not ok 78 - StreamLine Law: the tail reaches where the hand stopped
# tests 128 · # pass 125 · # fail 3
```

Two independent instruments (turning and curvature) both reject it, and the file
was restored byte-for-byte afterwards (`128 passed, 0 failed`).

### 5.5 Gate status

| gate | command | result |
|---|---|---|
| format | `cargo fmt --all --check` | **clean** |
| types (Rust) | `cargo check --workspace --all-targets` | **0 errors, 0 warnings** |
| tests (Rust) | `cargo test --workspace --all-targets` | **589 passed, 0 failed** (53 targets) |
| lints | `cargo clippy --workspace --all-targets -- -D warnings` | **clean** |
| types (UI) | `npm run typecheck` | **clean** |
| tests (UI) | `npm run test:ui` | **128 passed, 0 failed** |
| end to end | `npm run smoke` | **71/71, `SMOKE PASS`** |
| build | `npm run build` | **✓** (311 kB js, 24.4 MB wasm, 2.9 MB gzipped) |
| dev | `npm run dev` | **✓** live on `0.0.0.0:5173` against the rebuilt engine |

---

## 6. Deviations and environment, stated plainly

### 6.1 The toolchain was rebuilt mid-task, and the crate count changed

The sandbox restarted and wiped `/tmp` — the Rust toolchain, the vendored tree,
the cargo home and the Python helper libs. The chain was re-run from the repo's
own tools (`tools/extract_toolchain.py` → `pip install --target=/tmp/pylibs
tomli tomli_w zstandard` → `tools/vendor_deps.py --jobs 6` → the cargo source
replacement config), and it now reports:

```text
DONE: 239 crates vendored; missing 0
cargo 1.97.0
```

Task 10.6 recorded **256** for the same lock. The difference is not the lock: it
is donor quality. Vendoring prefers a checksum-verified **published tarball** (a
committed `.cargo/registry/cache/…crate`) over a repository *tree*, and this run
found registry copies for several crates that the earlier run had to take from a
tree — each of which also brings its own sub-dependency closure. Fewer entries,
same closure, `missing 0` either way: **the invariant is the `missing 0`, not the
count.**

The `wasm-bindgen` CLI is still unobtainable here (release assets are firewalled,
crates.io is unreachable), so the glue was regenerated the Task 10.6 way: vendor
the driver's own closure from **its own lock** (41 crates, including
`wasm-bindgen-cli-support 0.2.129`), build `scripts/wbgen`, and drive it. The
rebuilt module is 24,413,888 bytes (the pre-10.7 one was 24,178,797).

### 6.2 Doctests

`cargo test --workspace` still ends in `error: doctest failed … could not execute
process rustdoc`: the PyPI toolchain bundle ships `rustc`, `cargo`, `rustfmt` and
`clippy` but **no `rustdoc`**. `--all-targets` (the gate above) runs everything
else. Unchanged from Task 10.6, and not a code failure.

### 6.3 What the first compile/test cycle found

Three real defects, all now fixed, all worth recording because each one was a
*silent* kind of wrong:

1. **Two commands not registered in two matches** (`vectra-dependency`'s
   prospective-edge analysis and `vectra-ai`'s id-namespace map): both are
   exhaustive `match`es over `Command`, so adding the layer flags broke the build
   in the two places that must know every command's referents. The compiler is
   the gate here, and it did its job.
2. **Straight pen runs were sampled only at their endpoints** (§3.2) — a line
   drawn across the layer was refused for having "nothing to land on". Found by
   the new law on its first honest run.
3. **The Non-Compounding Law's first failure was the test, twice over**: it grew
   only `width` (a mask 100×40, so 4,000 was the *right* answer, not 10,000), and
   its harness re-evaluated the whole scene, which quietly restores the clipped
   node from the document and hides compounding. It now grows both axes and
   patches incrementally (`DirtySet` + `apply_partial`), which is what `settle`
   does.

### 6.4 Honest limits

* **One run per stroke, for the 1-D clip.** A node's geometry is a single chain
  (`NodeKind::Path { start, segments }`), so an open path that leaves the artwork
  and re-enters keeps the **longest** inside stretch, not several. The 2-D case
  (brush, shapes, closed paths) keeps the largest *region* — the intersection
  itself has every ring; a single-chain path is what cannot.
* **Clipped geometry is what exports.** A clipping-masked layer exports its
  clipped outline, because that is what "visible only where it overlaps" means in
  a document that is geometry, not pixels.
* **The gesture layer is tested headlessly.** The recogniser, the routing table
  and the `swallow` contract are covered by laws; an actual browser scroll cannot
  be reproduced in Node, so the viewport half is carried by `touch-action: none`,
  the context-menu swallow and `overscroll-behavior: none` (§1.3) and is verified
  by the live preview rather than by an assertion.

---

## 7. Files touched

**New** — `crates/vectra-operations/src/clip.rs`,
`crates/vectra-operations/tests/clip_laws.rs`,
`crates/vectra-wasm/tests/alpha_lock_laws.rs`,
`apps/vectra-web/src/engine/draw/{touch,streamline,colordrop}.ts`,
`apps/vectra-web/src/components/DrawSettingsPanel.tsx`,
`apps/vectra-web/tests/{task-10-7-procreate,task-10-7-panels}.test.{ts,tsx}`.

**Changed** — `crates/vectra-core/src/{layers,command}.rs`,
`crates/vectra-operations/src/lib.rs`, `crates/vectra-dependency/src/prospective.rs`,
`crates/vectra-ai/src/schema.rs`, `crates/vectra-wasm/src/{lib,draw,snapshot}.rs`
and its `Cargo.toml` (a `lyon` type is named by the clip helpers; `geo` is a
test-only dependency for the region maths), `apps/vectra-web/src/{App.tsx,App.css}`,
`apps/vectra-web/src/engine/{commands,wire,panels,draw/pointer}.ts`,
`apps/vectra-web/src/components/LayersPanel.tsx`,
`apps/vectra-web/scripts/smoke.mjs`, `apps/vectra-web/tests/panels.test.ts`,
`tools/vendor_deps.py` (the retried code search), `Cargo.lock`.

---

## 8. What a designer can do now

Undo with two fingers, redo with three, copy/paste with a three-finger drag —
without touching a menu and without the page scrolling underneath them. Pull
StreamLine up and a shaky hand draws a clean line (54.8 % less turning at the
top of the slider, and 0 % is exactly the old behaviour). Lock a layer's alpha and
shade inside the lines: the stroke lands on the artwork, stops at its edge, and is
refused in a sentence if there is nothing there to land on. Clip a layer to the
one below and it shows only where they overlap — live, non-destructively, and one
undo away from being unclipped. Drag a colour from the well onto a shape and the
shape takes it; drag it into empty space and nothing happens at all.
