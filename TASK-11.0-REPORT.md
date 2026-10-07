# TASK 11.0 — Advanced Parametric Typography

Four rules, one idea: **type is geometry with knobs.** A run is shaped and
outlined once, at evaluation time, into the same `lyon` paths the renderer, the
hit test, the boolean engine and the exporters already understood — so every
number on a text node (`font_size`, `letter_spacing`, `line_height`, and the
text-on-path `offset`) is a `Parameter<f64>`, and moving one re-lays the run out
on the very next evaluation. Nothing downstream ever learns what a glyph is.

| layer | what it owns | where |
|---|---|---|
| `vectra-core` (document) | `NodeKind::Text`, `TextAlign`, `TextPathBinding`, the six typography commands, the slot tables (`scalar_slots`, `text_slots`, `path_offset`), the font provider trait | `crates/vectra-core/src/{document,command,component,eval}.rs` |
| `vectra-geometry` (engine) | the font library, shaping, layout, text-on-path sampling, the outline planner, the diagnostics | `crates/vectra-geometry/src/{text,paths,scene,evaluator,diagnostic}.rs` |
| `vectra-wasm` (boundary) | `outline_text`, the font registry, the typography snapshot rows | `crates/vectra-wasm/src/{lib,snapshot}.rs` |
| `vectra-web` (UI) | the Text tool (`T`), the Text panel, the typography view-model | `apps/vectra-web/src/App.tsx`, `src/components/TextPanel.tsx`, `src/engine/{panels,commands,wire,draw/tools}.ts` |
| everywhere else | the run as a path: SVG export, the AI's command references, the dependency graph | `crates/{vectra-export,vectra-ai,vectra-dependency}/src/…` |

The quick version of the four rules: `font_size` bound to a `$variable` scales a
word by exactly the variable's ratio, per glyph and in the drawn picture (§1);
a run bound to a circle rides the tangent at every glyph and slides along the arc
with an offset that is a parameter like any other (§2); `OutlineText` turns the
shaped glyphs into a group of ordinary `Path` nodes and *hides* the type it
replaced, one undo away (§3); and all of it reaches the GPU through the one
geometry pipeline and the Task 10.2 appearance stack, because a run **is** a
path (§4).

This revision also answers a pre-commit architectural review — a shaping cache,
text that outgrows its path, the outline's z-order and layer, `<path>` versus
`<text>` in SVG, and bounding-box stability under fallback. Three of the five
turned out to be defects, all fixed: **§9** is the round's own section, and §5.1,
§5.4 and §6.3 carry the updated evidence.

---

## 1. RULE 1 — parametric text nodes

### 1.1 The data model, exactly as specified

```rust
// crates/vectra-core/src/document.rs
Text {
    text: String,
    font_family: String,
    font_size: Parameter<f64>,
    letter_spacing: Parameter<f64>,   // document units, negative tightens
    line_height: Parameter<f64>,      // a *multiple* of font_size (CSS's reading)
    alignment: TextAlign,             // Left | Center | Right
    x: Parameter<f64>,                // baseline start (Left) / centre / end
    y: Parameter<f64>,
    on_path: Option<TextPathBinding>, // RULE 2 — absent until bound
}
```

Every number is a `Parameter`, never a plain `f64`, and the node's slot space
says so: `scalar_slots()` returns `["x", "y", "font_size", "letter_spacing",
"line_height"]`, and the dynamic `text_slots()` adds `path_offset` exactly when
`on_path` is present — so the inspector is never offered a slider for a binding
that is not there, and the constraint solver and `SetParameter` address the
offset by name the moment it exists. `line_height` is a **multiple**, not a
distance, so animating the size scales the leading with it instead of collapsing
the lines together.

The glyph outlines never live in the document. The node stays scalar, editable
and small; `vectra-geometry` shapes and outlines it exactly as it builds a
circle's `lyon` path from `cx`/`cy`/`radius`.

### 1.2 Shaping, and the font that always exists

`crates/vectra-geometry/src/text.rs` is the whole text engine:

* **Shaping** — `rustybuzz` 0.20 (HarfBuzz's algorithm as a pure-Rust library)
  applies the face's own GSUB/GPOS tables: kerning, ligatures, mark positioning.
  The unit of work is a *line*, because that is what a shaper shapes;
* **Outlining** — `ttf-parser` 0.25 walks each shaped glyph's contours through a
  `GlyphOutline` builder that applies an `Affine` and writes a `lyon` path —
  the single bridge between the font's y-up units and the document's y-down
  space;
* **One face is bundled** — `assets/vectra-sans.ttf`, a 55 KB subset of DejaVu
  Sans (Bitstream Vera licence, next to the file), family `Vectra Sans`, with
  the generic aliases `sans-serif`, `sans serif`, `system-ui`, `dejavu sans`.
  A document always draws its words on any host, with no download and no system
  font.

A host may register more faces through the boundary
(`register_font(family, bytes)`), which is **validating**: bytes that do not
parse are refused and nothing changes. A family nobody has falls back to the
bundled face *and says so* — the evaluator emits the `font-fallback` diagnostic,
because a silently substituted typeface is worse than a reported one.

`FontLibrary` keeps two names apart on purpose: the normalized key is the
lookup, and the **spelled** name is what a picker shows. `families()` is
de-duplicated by face, so the five spellings of the bundled face are one entry
in the Text panel's menu.

### 1.3 Layout: baselines, alignment, leading

```text
line i's baseline   = y + i · (font_size · line_height)
the pen starts at   = x + alignment.anchor_offset(line_width)
                    = x            (Left)
                    = x − w/2      (Center)
                    = x − w        (Right)
glyph g sits at     = pen + g.offset_x, baseline − g.offset_y
pen                += g.advance + letter_spacing
```

`TextAlign::anchor_offset` is the one place the alignment decision is made, so
the straight layout and the on-path layout cannot disagree about it. The
metrics the node reports are the ones a designer's tools need: the widest line,
`ascender − descender` plus the leading between lines, the ascender, the
descender, one line advance, and the line count.

Evaluation is **total**: empty text, an empty line and a degenerate path all
produce an *empty run*, not an error — the states a text node passes through
while it is being typed. `text_evaluation_is_total` pins it.

### 1.4 The command surface

Six commands, all ordinary history entries:

| command | what it writes | what undo restores |
|---|---|---|
| `SetText { node_id, text }` | the string | the previous string |
| `SetFontFamily { node_id, family }` | the family | the previous family |
| `SetTextAlignment { node_id, alignment }` | `left`/`center`/`right` (lowercase on the wire) | the previous alignment |
| `BindTextToPath { node_id, path, offset }` | the binding, validated **before** anything is written | the previous binding, or an `UnbindTextFromPath` if there was none |
| `UnbindTextFromPath { node_id }` | the run back to its authored baseline | the binding, offset included, exactly |
| `OutlineText { node_id, group_id, name, paths }` | RULE 3 (§3) | the hidden type, the removed group, the removed letterforms |

The numbers are *not* special commands: `font_size`, `letter_spacing`,
`line_height` and `path_offset` are edited with the same `SetParameter` a
rectangle's width uses. On the wire the value is a `ParamValue`
(`{"Float": {"Literal": 32}}`, `{"Float": {"Variable": "scale"}}`), which is how
the boundary knows which of a node's value kinds is being edited before it
routes it to a slot.

### 1.5 The Text tool and the panel

`T` is bound to the Text tool (`engine/draw/tools.ts`): cursor `text`, and
deliberately **not** a drawing tool (`direct: false`, no stroke to track) — its
click places a node through an ordinary `CreateNode` carrying the same
`kind.Text` defaults the panel edits (`text: ''`, `"Vectra Sans"`, size 32,
spacing 0, leading 1.2, `left`). The new node comes back selected from the
dispatch reply, not guessed from a uuid.

`components/TextPanel.tsx` is offered exactly when one unlocked run is selected
(`showsTextPanel`) and holds: the string, the font picker (the engine's own
`font_families()`, plus the document's family **even when this host cannot
resolve it** — silently rewriting a designer's family is worse than showing it
substituted), Size, Spacing, Leading, the three alignment buttons, **Bind to
Path** / **Unbind**, the offset slider with a numeric readout, **Outline**, and
the metrics line. Each numeric field writes `setFloatParam(node.id, prop, …)`,
and any slot whose `*_source` is not `literal` shows the tag
(`variable`, `expression`, `animated`, …) on its label, so a designer can see
that a number is driven without opening a debug view. The offset slider's track
is the engine's own arc length and its mapping is invertible
(`offsetFromSlider`/`sliderFromOffset`); a bound run is not draggable, because
its origin is the path's.

### 1.6 The shaping cache: an offset is a placement, not a re-shape

Shaping and glyph outlining are the expensive half of text, and they are a pure
function of **six things**: the face, the string, the size, the letter spacing,
the leading and the alignment. Everything else about a run — `x`/`y`, the bound
path, the `offset` — is a *placement*. The layout is therefore split in two:

```text
build_local_run(face, spec)  →  LocalRun { glyphs in the run's own frame, metrics }   [expensive]
place(local, x, y | path, offset)  →  EvaluatedText over a transformed Path           [cheap]
```

`shaped_run(face, spec)` memoizes the first half in a **thread-local LRU**
(`SHAPE_CACHE`, 48 runs, hit/miss counters exposed as `shape_cache_stats()`):
thread-local because shaping is CPU work with no shared state and a wasm engine
is single-threaded, and a lock on the hot path of every text evaluation would
cost more than the memo saves. The key is the six inputs compared as **bits**
(`size.to_bits()`, …), not as floats: the memo answers "the same run?", and an
f64 equality that conflated `0.0` with `-0.0` would be a different question.

Two consequences, both tested:

* **an `offset` slider drag re-places** — a cache hit clones an `Arc` and walks
  the glyph events once per frame. `an_offset_change_re_places_instead_of_re_shaping`
  asserts the counters (one miss for a run, a hit for the slide, a second miss
  only when the *size* changes) and that the picture moved anyway;
* **a hit is indistinguishable from a cold layout** — `a_cached_re_placement_equals_a_cold_layout`
  clears the cache and compares an `EvaluatedText` value, glyph for glyph and
  contour for contour; `law2b_a_slide_equals_a_fresh_evaluation` states the same
  thing at the document level, where a memo that handed back geometry it had
  been mutated into (the classic "bake the offset into the remembered outlines"
  bug) would disagree with the fresh document.

The face is part of the key **by identity**, not by name — a host that
re-registers a modified face under a name it used before must not be served the
old outlines. `FontProvider::face_id(family)` lets a provider that owns its
bytes name them once (`FontLibrary` fingerprints each face at registration, FNV-1a);
a provider that cannot gets its bytes hashed on the way in — correct, merely
slower. `a_re_registered_face_is_a_different_face` pins the contract.

---

## 2. RULE 2 — text on a path ("the logo maker")

### 2.1 The binding is a one-way reference

`TextPathBinding { node, offset }` names the source node; it does not copy its
geometry. The evaluator resolves the source **during the run's own evaluation**
(`bound_source_path`), so reshaping the circle re-flows the text with it, and a
source created *after* the binding still moves the run — no cached scene, no
second dependency edge, evaluation stays a single pass. The boundary validates
the direction: `Document::is_text_path_source` accepts exactly `NodeKind::Path`,
`Arc` and `Circle`, so a text node can never bind to text (or to a rectangle),
and `BindTextToPath` to anything else is refused with the sentence saying what is
wrong, before anything is written.

### 2.2 Arc-length sampling: a tangent is not a polygon edge

The run needs *arc length* and *tangent at arc length*. Flattening answers the
first question with a tolerance in document units and answers the second badly:
a circle of radius 440 flattens into 43-unit chords at the renderer's 0.05
tolerance, so neighbouring samples differ in direction by up to 5.6° — and a
word set on those tangents **kinks** instead of following the curve.

`sample_path` therefore refines itself: it flattens at `PATH_TOLERANCE` (0.05,
the renderer's own tolerance), measures `worst_turn` — the largest direction
change between adjacent samples — and, while that exceeds `TANGENT_TURN`
(0.01 rad ≈ 0.57°) and the tolerance is above `MIN_PATH_TOLERANCE` (1e-4),
re-flattens at `ratio = ((TANGENT_TURN / worst)² · 0.8).clamp(0.02, 0.5)` of the
old tolerance. (The turn scales with `√tolerance`; the 0.8 is headroom, since
the worst segment after a refinement is not the one that was worst before it.)
Four refinements is a hard ceiling, and a straight line keeps its original
handful of samples — the budget is spent on the curves that need it. Measured on
the law's own worst case (radius 440.517, "AcdV" at 45.774):

```text
tol 0.05       → worst_turn 0.028946135396337613,   225 samples
tol 0.00477396 → worst_turn 0.009081034273759503,   717 samples
final          → 717 samples, length 2768.231463498187, worst_turn 0.00908
```

`sample_at(samples, distance)` then walks the polyline by `partition_point`,
lerps *within* the containing segment (no snapping to the nearest sample, which
would quantise the offset) and reads the segment's own direction. Past either
end it **clamps**: the position and tangent of the last sample. A designer
dragging the offset past the end of a logo's curve sees the type park at the
end — the documented "clamp, don't skip" policy the evaluator applies to every
other mappable value, pinned by a test on a 400-unit open line (not a circle,
where the end and the start are the same point).

### 2.3 Each glyph's transform: tangent, readability, parallel lines

For glyph *g* at arc distance *d*:

```text
(x, y, θ)      = sample_at(samples, d)
up             = (sin θ, −cos θ)          // font space is y-up; the document is y-down
if up.y > 0    → θ += π                   // the readability pass
drop           = line_index · line_advance
pen            = (x, y) − up·drop + R(θ)·(offset_x, −offset_y)   // on_path_pen
transform      = Affine::on_path(scale, θ, pen)      // scale, rotate, translate
d             += advance + letter_spacing
```

* **rotation is the tangent**: each glyph sits at its own arc position and is
  turned by the direction the curve is going *there*, so a word genuinely follows
  the geometry rather than approximating it with a straight baseline;
* **the readability pass**: half of a circle's tangents point backwards, and text
  rotated onto one of those is upside down. A glyph whose *up* would point into
  the lower half-plane is turned a further half turn — exactly the rule a
  designer applies by hand when they set type on the bottom of a circle. The
  turn is recorded per glyph (`EvaluatedGlyph::angle`) and asserted by law 2:
  `angle ≡ tangent (mod π)`, and no glyph ever has its up pointing down;
* **multi-line runs follow the path too**: line *i* rides a parallel curve one
  line advance toward that glyph's own down direction, so a paragraph on a
  curve stays a paragraph;
* **the glyph's own offset is rotated with it**: `R(θ)·(offset_x, −offset_y)` is
  the shaper's mark/mark-to-base positioning (already in document units, still
  carrying the font's y-up signs), a vector in the glyph's own frame rather than
  a slide along the tangent. `on_path_pen` is that one piece of arithmetic,
  named and unit-tested, because the bundled face has no marks (every offset it
  shapes is zero) and a mistake there would only surface when a host registers a
  face that has them.

A bound run reports its own box as the arc-length window it covers (the whole
path length) by one line advance per line — the number a UI needs to slide it,
not a bounding box that changes shape with the curve.

### 2.4 Offset, alignment, and what law 2 measures

`offset` slides the run along the path in document units and is a
`Parameter<f64>`; alignment decides where the run sits relative to that point,
measured along the curve with the same `anchor_offset` the straight layout uses
(SVG's `startOffset` + `text-anchor` reading: `left` starts the run at the
offset, `center` centres it there, `right` ends it there).
The offset law is a **distance** law: for a small `Δ`, each glyph's on-screen
displacement is `Δ ± tolerance`, with `Δdistance == Δoffset` exact in arc-length
terms, and the box-centre displacement floor at `0.4·|Δ|` — a floor, not a
tight match, because a glyph's *centre* is not its pen position and the
flattened path is not exactly the arc.

**A run longer than its path is truncated, not clamped.** A glyph rides the path
iff its own arc position — `offset` plus the advances before it — lies on the
path; the others are left out, each at its own position, so the survivors keep
their spacing and nothing piles up at the seam. The decision is made at the
**pen position** (the run's arc length), not at the glyph's ink extents: that is
the same measurement the layout advances by, it is what SVG renderers clip by,
and it is cheap — an ink-based test would need every contour's bounds per frame.
Two things follow, and both are laws:

* the **authored number is never rewritten** by the picture. Sliding to
  `path_offset = -500` on a 400-unit line draws nothing, reports every glyph as
  left out, and leaves the document's own `-500` where the designer typed it —
  which is what makes the slider reversible;
* the run reports **how many glyphs it left out** (`EvaluatedText.truncated`),
  which the evaluator turns into the `text-overflow` warning: a designer sees
  the words that fit *and* a sentence saying the rest is off the path, instead
  of a blot at the end. No wire change was needed — `get_snapshot().diagnostics`
  already carries the evaluator's diagnostics.

Three laws pin it: `law2_every_glyph_rides_the_tangent_and_stays_upright` now
checks the window against an **independent** straight layout (whose `distance`
values are the authored arc positions, because nothing can be truncated there),
`law2_reshaping_the_path_moves_the_run` asserts a longer path can only *widen*
the window without moving a single survivor, and
`a_run_slid_off_its_path_keeps_nothing_piled_at_the_seam` walks a run off both
ends of a 400-unit open line — the one geometry where "off the start" and "off
the end" are different answers.

---

## 3. RULE 3 — outline to paths, non-destructively

### 3.1 The command, and why the plan travels with it

The UI cannot build an outline itself — shaping lives in `vectra-geometry`, and a
browser has no glyph ids — so the Outline button calls the boundary's
`outline_text(node_id, name?)`, which lays the scene out if it is cold, shapes
the run, mints the letterform plans, and dispatches:

```rust
OutlineText {
    node_id,                 // the type being replaced (hidden, never deleted)
    group_id,                // the caller's id, so undo and redo address the same group
    name: Option<String>,    // defaults to the text node's own name
    paths: Vec<OutlinePath>, // one per shaped glyph: id, name (= the character), start, segments
}
```

Validation happens **before** any mutation, in the order that matters — the
node exists and is text; the plan is non-empty; the group id and every letterform
id are free (a caller-supplied id that is taken is refused, so undo and redo
always address the same nodes). A refusal leaves the document exactly as it was;
a half-applied outline would be neither the old document nor the new one.

### 3.2 One path per glyph, counters included

`outline_plans(run, text)` flattens each glyph's outline at `OUTLINE_TOLERANCE`
(0.1 — an outline is *authored* geometry a designer will push, pull and boolean,
so a vertex every 0.1 units is already finer than any screen and keeps the
segment list workable), drops contours with fewer than three points, and emits
**one closed plan per glyph**, named by the character it came from so the layers
panel reads `V`, `e`, `c` instead of a row of uuids.

A glyph with a counter (`o`, `A`, `8`) is *two* contours, and the path model is a
start point plus segments — there is no `MoveTo`. The plans use the classic
**keyhole**: the outer contour closes, a zero-width bridge runs out to the
counter, the counter is traced, and the bridge returns. Traversed once in each
direction, the bridge contributes no area and cancels under **both** fill rules
the workspace uses (the renderer fills even-odd, and TrueType's counter windings
are opposite for a nonzero fill) — the hole stays a hole.

### 3.3 Non-destructive, exactly

1. the original text node is **hidden** (`visible = false`), never deleted: its
   string, family, every parameter and its binding are still there;
2. the letterforms are new `Path` nodes inside a new `Group` — real rows of the
   layer tree, addressable by the solver, the AI planner and the pen like any
   other path;
3. the group lands **directly above the type it replaces** — in the *type's own
   layer*, at the type's own row, and below everything that was above the type —
   so a designer's stacking does not change just for outlining. (This is
   `Document::assign_block_above`: the block `[group, letterforms…]` is spliced
   in above the type's run, rather than appended to the layer's tail. The tail
   was a real defect this task shipped and this round found: the outline jumped
   above the artwork that had been above the type. `law3b` pins the frame.)
4. the letterforms **inherit the text node's own paint stack**, so a
   gradient-filled word outlines to gradient-filled letters with no second
   styling step;
5. one `undo` restores the document **byte for byte** — the inverse is a single
   `Batch`: delete the letterforms, delete the group, show the type again (the
   group is removed last, so undo recreates it first and every letterform can be
   re-parented into a group that exists again). Asserted by law 3 and by smoke
   step 75.

### 3.4 What an outline is good for

The letterforms are ordinary regions now: boolean them, distort them, use them as
a clip mask, or export them. The exporters already treat a run as its paths (§4),
so outlining is what a designer runs when they want *editable* letterforms rather
than live type — the same non-destructive operation, on purpose: nothing is lost
and one undo brings the type back.

---

## 4. RULE 4 — renderer integration

### 4.1 One geometry pipeline

`layout_text`/`layout_text_on_path` produce `EvaluatedPrimitive::Text(run)` whose
`outline` is **one** `lyon::path::Path` holding every glyph, in document space.
`paths::primitive_to_path` returns that path for a run, so:

```text
NodeKind::Text → EvaluatedText.outline → renderer flatten (0.05) → lyon tessellate → GPU vertices
```

— the *same* pipeline a pen stroke or an imported SVG takes. There is no
text-specific tessellator, no glyph atlas, no second paint path: the renderer
tessellates text by tessellating a path, which is what keeps it reliable
(rustybuzz + ttf-parser produce the outlines; lyon fills, strokes, culls and
hits them) and what makes every downstream consumer work with no special case.

### 4.2 The appearance stack, unchanged

Fills, strokes, gradients, dash, opacity and blend modes come from the node's
`StyleProperties::resolved_appearances()` — the Task 10.2 stack — which is read
per node and per draw item by the renderer's scene builder. A text node is a
node, so a run takes a gradient fill, a dashed stroke and a blend mode exactly
as a rectangle does; outlining changes nothing about that, because the
letterforms are paths with the same styling machinery.

### 4.3 Every other consumer

| consumer | what it sees |
|---|---|
| hit test / selection | the run's glyph outlines (a click inside an `o`'s counter misses, because the path says so) |
| region engine | a fillable region, so booleans and clipping apply to a run with no adaptor |
| SVG export | the letterforms, as one `<path d>` — the file renders identically everywhere with no font installed and no substitution; a run with unresolved geometry is exported with a warning rather than a broken `<text>` |
| snapshot (`vectra-wasm`) | `SnapshotPrimitive::Text { d, glyphs, width, height, lines }` plus a `text` row with the resolved values and their `*_source` tags; `scene.fonts` carries the picker's options |
| AI planner | the six commands' references (a text edit names its run; a binding names the run and the path; an outline names the type and everything it mints) |
| dependency graph | `Bind`/`UnbindTextToPath` simulate the binding so the prospective edges stay honest; authored text edits make no new edges |

---

## 5. Tests

### 5.1 Rust — **633 passed, 0 failed** (Task 10.7 closed at 589)

| suite | count | what it covers |
|---|---|---|
| `cargo test --workspace --all-targets` | **633 / 0 failed, 54 targets** | everything below plus the whole pre-existing workspace |
| `vectra-geometry` lib (incl. `text.rs`) | 44 (22 in `text`) | face parsing, coverage, metrics, scaling, alignment, spacing, ligatures/kerning, the font library, the shaping cache, truncation, error totality |
| `vectra-geometry` `geometry_eval` | 10 | the pre-existing evaluator laws, unchanged and passing with text in the scene |
| `vectra-geometry` `text_laws` | **21** | the three laws + two review-round laws (10 proptests) + 11 pinned laws |
| `vectra-export` `export_laws` | 17 (1 new) | the semantic/roundtrip laws, plus a run exporting as font-independent `<path>` geometry |

The three laws, as written:

| law | generated | asserts |
|---|---|---|
| **Parametric Text Law** (`law1_a_font_size_variable_relayouts_the_run`) | word `[A-Za-z]{1,12}`, size 4–256, factor ×0.05–0.75 / ×1.25–4.0, origin ±2000 | bind `font_size` to `$scale`; the line width, the type height, every glyph's advance **and** its pen position scale by exactly the factor to 1e-9; same glyph ids (re-laid out, not re-shaped); the *drawn* box moves by ≈`|factor−1| ×` its own extent |
| ↳ its negative half | same | an unrelated circle edit does **not** move a straight run — a law that only ever observed motion would be passed by an evaluator that re-lays out on every edit |
| **Text-on-Path Law** (`law2_every_glyph_rides_the_tangent_and_stays_upright`) | word, size 8–120, radius 20–400, offset ±300 | the drawn glyphs are **exactly** the window an independent straight layout says fits — same ids, same arc positions, same count — each angled onto the tangent (mod π) with an up that never points down, and drawn at the point the sampler returned |
| ↳ slide (`law2_the_offset_slides_the_run_along_the_curve`) | word, size, radius, `delta` | `Δdistance == Δoffset` exactly, and the run's box centre moves ≥ `0.4·|Δ|` on screen — the floor, not a tight match, because a glyph's centre is not its pen |
| ↳ reshape (`law2_reshaping_the_path_moves_the_run`) | word, size, radius | growing the circle moves every glyph — a binding is a reference, not a copy — and can only *widen* the window: every survivor keeps its arc position, because an arc position does not depend on the curve it rides |
| ↳ slide ≡ fresh (`law2b_a_slide_equals_a_fresh_evaluation`) | word, size, radius, offset, slide | the run a slide produces **equals** the run a fresh document produces at the slid offset — glyph for glyph, contour for contour. This is the law a shaping cache fails if a hit ever hands back geometry it has been mutated into |
| ↳ **Family Law** (`law5_a_family_name_cannot_move_the_box`) | word, size 4–256 | `sans-serif`, `system-ui`, `DejaVu Sans` and an unknown family all measure the **same** box and draw their glyphs in the same places: substitution is a `resolve_face` question, never a layout one |
| **Outline Law** (`law3_outlining_yields_valid_closed_paths`) | word, size 12–120, origin | one plan per shaped glyph; every segment is a `Literal` line or `Close`; the last segment closes; the rebuilt path encloses area and matches the glyph's own drawn box within 1e-6 |
| ↳ non-destructive half (`law3_outlining_is_non_destructive_and_undone_exactly`) | same, through the command surface | the type is hidden (not deleted) with every authored value intact; exactly one new path node per glyph, visible and grouped; one undo restores the node set and the geometry bit for bit |
| ↳ fidelity (`law3_an_outline_matches_the_glyph_it_replaced`) | word, size, origin | the outlined region is the glyph it replaced, measured against the contours the glyph *draws* |

Pinned laws (plain `#[test]`s, because they are statements about the design, not
generated claims): `text_evaluation_is_total` (empty string, empty line,
degenerate path → empty run, never an error), `the_bundled_face_always_draws`,
`a_run_slid_off_its_path_keeps_nothing_piled_at_the_seam` (a 400-unit open
line), `binding_is_refused_for_a_non_path_source`,
`unbinding_restores_the_authored_baseline`,
`a_run_reads_its_source_regardless_of_document_order`,
`a_missing_source_is_an_empty_run_not_a_failure`,
`retyping_keeps_every_typographic_property`,
`law3b_the_outline_lands_directly_above_the_type_in_its_own_layer` (layer,
row, and the artwork that was above the type),
`an_offset_variable_re_places_without_re_shaping` (the cache counters through
the engine, with `path_offset` driven by a variable), and
`a_broken_face_still_draws_its_words` (a host whose bytes no shaper can read).

### 5.2 UI — **137 passed, 0 failed, unchanged by this round** (Task 10.7 closed at 128)

`apps/vectra-web/tests/task-11-0-typography.test.ts` adds 9 laws at the UI
boundary: the panel is for one unlocked run and nothing else; the picker offers
the engine's families and never loses the document's; parametric-slot detection;
every number the panel writes is an ordinary `SetParameter`; the three alignment
buttons and nothing else; the offset slider's track is the engine's arc length
and its mapping is invertible over the whole path and no further; `outlineTextPlan`
sends the engine's letterforms unchanged; and `T` is the Text tool, not a drawing
tool, with a text cursor.

### 5.3 Smoke — **75 steps, `SMOKE PASS`** (Task 10.7 closed at 71)

Four new end-to-end steps, against the real wasm module:

* **72** — a parametric run creates, shapes, snapshots and draws:
  `primitive.type === 'text'`, ≥ 6 glyphs, a `d` with `M`, a positive box,
  `scene.fonts` includes `Vectra Sans`, `font_size_source === 'literal'`;
* **73 — the parametric law end to end** — `SetVariable scale 32`, bind
  `font_size` to `{Float: {Variable: "scale"}}`, then `SetVariable … 96`: a
  `Dirty` event, `font_size_source === 'variable'`, the box scales by ≈3, and the
  `d` changes;
* **74 — text on a path** — `BindTextToPath` to a circle moves the run
  (`bound_to` names the source, six glyphs, different `d`); a rectangle source is
  refused with `status: 'error'` and nothing changes; `path_offset` 120 slides it
  again while the string is untouched;
* **75 — outline** — `outline_text` returns the created group; exactly
  `glyphs` fresh node ids appear, all `path`, visible, with geometry; the group
  holds them as a real row of `layers[].children`; one `undo` restores the
  node-id set and the original `primitive.d` byte for byte.

### 5.4 Do the laws have teeth?

Five deliberate mutations, each reverted and re-verified green afterwards. The
first three are the layout's, the last two are this round's:

| mutation | what fails |
|---|---|
| delete the readability pass (`if false && glyph_up(angle).1 > 0.0`) | `law2_every_glyph_rides_the_tangent_and_stays_upright` |
| feed the bound run the **polygonized** path view (the original bug, §6.3) | the tangent law — *"glyph 0 sits at 1.575 but is rotated 4.761"* — **and** the slide law — *"glyph 3 slid 0.960 on screen for an offset of 3.845"* |
| drop `sample_path`'s self-refinement (flatten at 0.05 once) | the tangent law again |
| make the shaping key **size-blind** (`size_bits: 0`) | the cache law (*"a font-size change is a new shaped run: ShapeCacheStats { entries: 1, hits: 2, misses: 1 }"*) **and** the pre-existing `font_size_scales_the_run` — a stale hit draws the old size |
| remove the truncation window (`arc.clamp(0.0, total)`) | four tests across two targets: the truncation unit test, `a_run_slid_off_its_path_keeps_nothing_piled_at_the_seam`, `law2_every_glyph_rides_the_tangent_and_stays_upright` (*"assertion failed: `(left == right)`"*), and `law2_reshaping_the_path_moves_the_run` (*"an arc position must not depend on the curve it rides"*) |

### 5.5 A flake the regression file caught, and the fix

During this task's final gate, proptest's own stored regression seed surfaced a
case the earlier run had passed by luck: word `"I"`, size 4.0, factor 1.05. Law
1's *drawn*-box claim compared two **flattened** boxes, and flattening carries
an absolute 0.05-unit tolerance — at caption sizes a 5 % size change moves the
edges by less than that, so the check was measuring the flattener's noise floor.
Two changes, both principled:

1. the factor strategy is now `0.05..0.75 ∪ 1.25..4.0` — deliberately not a
   whisker away from 1. The exact halves of the law (width/height/advance/pen)
   hold for *any* factor and already cover the near-1 band; the drawn half must
   clear the flattening floor to mean anything;
2. the drawn claim is relative to the box the run actually drew:
   `moved > 0.3 · |factor − 1| · (width_before + height_before)`, instead of a
   threshold tied to `size` alone.

The case is kept in `text_laws.proptest-regressions` and re-runs on every
invocation; eight consecutive full runs of the suite are green with it in the
file.

Two related notes from this round, both about tests that had to *change* rather
than be fixed:

* the round's truncation work retired `a_run_off_the_end_of_its_path_clamps_to_the_end`.
  That test asserted the old clamping behaviour — every glyph present, all of
  them parked at the same end of the path — which is exactly the picture the
  review point called out. It is replaced by
  `a_run_slid_off_its_path_keeps_nothing_piled_at_the_seam`, which asserts the
  opposite (nothing drawn, every glyph reported, the authored number untouched)
  and the *smooth* half (a 20-unit slide moves each survivor exactly 20);
* `law2`'s window claim compares floats to the path's length, so a glyph whose
  arc position lands within `1e-6` of either end is excluded with a
  `prop_assume!`. That is not a hole in the law — it is the one place where
  "on the path" is a knife edge at f64 precision — and the *decision* itself is
  pinned exactly by the deterministic truncation tests.

### 5.6 Gate status

| gate | command | result |
|---|---|---|
| format | `cargo fmt --all --check` | **clean** |
| tests (Rust) | `cargo test --workspace --all-targets --offline` | **633 passed, 0 failed** (54 targets) |
| lints | `cargo clippy --workspace --all-targets --offline` | **clean** (0 warnings, 0 errors) |
| types (UI) | `npm run typecheck` | **exit 0** |
| tests (UI) | `npm run test:ui` | **137 passed, 0 failed** |
| end to end | `npm run smoke` | **75/75, `SMOKE PASS`** |
| build | `npm run build` | **✓** (318.72 kB js / 96.42 kB gzip, 24.75 kB css, 27,054,595 B wasm / 3,518.74 kB gzip) |

---

## 6. Deviations and environment, stated plainly

### 6.1 `wasm-bindgen`, via `scripts/wbgen`, again

The `wasm-bindgen` CLI is still unobtainable here (release assets firewalled,
crates.io unreachable), so the Task 10.6/10.7 route was used unchanged:
`apps/vectra-web/scripts/wbgen` pins `wasm-bindgen-cli-support = "=0.2.129"` in
its **own** workspace and lock, its closure (41 crates) was vendored into
`/tmp/vendor-wbgen` (`DONE: 41 crates vendored; missing 0`), a cargo source
replacement in `/tmp/wbgen-home/config.toml` points crates-io at that directory,
and `cargo build --release --offline --manifest-path
apps/vectra-web/scripts/wbgen/Cargo.toml` produced the driver in 48.6 s. The glue
was then regenerated with the same `--target web` settings:
`wbgen <target/wasm32-unknown-unknown/debug/vectra_wasm.wasm>
apps/vectra-web/src/wasm vectra_wasm`.

The module is the **debug** profile (the repository's default; a release build is
`bash scripts/build-wasm.sh release`): 27,054,595 bytes against 24,413,888 at Task
10.7. The difference is this task's two new unoptimized dependencies
(`rustybuzz`, `ttf-parser`) plus the 55 KB bundled face; gzipped it is 3.5 MB.

### 6.2 Doctests

Unchanged from Task 10.6/10.7: the PyPI toolchain bundle ships `rustc`, `cargo`,
`rustfmt` and `clippy` but **no `rustdoc`**, so `cargo test` without
`--all-targets` ends in `could not execute process rustdoc`. `--all-targets`
(the gate above) runs every other target: 623 tests, 54 targets. Not a code
failure.

### 6.3 What the first compile/test cycles found

Seven real defects, all fixed, all worth recording:

1. **Tangent quantization** (the big one). Law 2 failed on a large circle: the
   bound run's rotation jumped by up to 5.6° from glyph to glyph, because the
   source was the *polygonized* path view — a 64-gon whose edge direction jumps
   at every vertex, however large the circle is. The fix is two-fold: the
   evaluator hands text-on-path the **curve** view
   (`primitive_to_curve_path`; `primitive_to_path` stays the right answer for
   fills, bools and hit tests), and `sample_path` refines its own tolerance until
   its tangents are faithful (`worst_turn` 0.098 before, 0.0091 after, threshold
   0.01). The mutation table in §5.4 keeps this fixed: feeding the polygon view
   back in fails both laws.
2. **Three degenerate one-point contours in the bundled face.** DejaVu's `u`,
   `glyph00192` and `glyph00261` each carry a contour with a single point — a
   real quirk of the source face, faithfully preserved by the subset (verified
   with `fontTools` against the committed asset). A naive flatten of a glyph's
   outline therefore reaches ~48.8 units past the letterform. Outlines must be
   built from the contours the glyph *draws* (rings of ≥ 3 points); law 3
   compares against exactly that box, and `outline_plans` filters the same way.
   `OUTLINE_TOLERANCE` was not widened to hide it.
3. **The font library's `families()` returned normalized keys** — a picker would
   have offered `vectra sans` in lowercase because that is the lookup key. The
   library now keeps the spelled name beside the key, and registers the generic
   aliases as *spellings of one face* rather than as extra families (a menu that
   showed four names for one typeface would be lying about the library). Found by
   smoke step 72, pinned by a new unit test.
4. **The `SetParameter` wire shape** — a geometry slot's value is a `ParamValue`
   (`{"Float": {…}}`), not a bare parameter. Found by smoke step 73 (the command
   was refused with "unknown variant `Variable`" and the run stayed at its old
   size), fixed in steps 73/74.
5. **`*_source` tags are `source_tag()`'s vocabulary** (`variable`, not
   `variable:scale`) — the smoke expectation was wrong, not the engine; corrected
   rather than "fixed" in the engine, because the tag is a documented contract
   shared with every other slot.
6. **A bound glyph's own shaped offset was slid along the tangent instead of
   rotated with the glyph** (and its `offset_y` was dropped), found while writing
   §2.3 of this report. The bundled face hides it completely — no mark
   positioning, so every offset it shapes is zero — which is why it is now one
   named function (`on_path_pen`) with two direct unit tests rather than a pair
   of lines inside the layout loop.

7. **The outline landed at the *tail* of the type's layer, not in the type's
   place.** RULE 3's placement half was asserted, but only in a document with no
   layers — and the layer path appended the block (`land_on_layer` pushed to the
   layer's tail), so outlining artwork that sat under something jumped the
   letterforms above it. Found by this round's z-order review; fixed by
   `Document::assign_block_above` (splice the block in above the type's run) and
   pinned by `law3b`, which outlines into a *second* layer with artwork above the
   type and checks the layer, the row, and the flat order.

### 6.4 Honest limits

* **Text is shaped in wasm, from the engine's font library** — not by the
  browser. A host that wants a system font registers its bytes
  (`register_font`); families nobody has fall back to `Vectra Sans` with a
  diagnostic. There is no system-font enumeration and no bold/italic synthesis;
  the model is one family per run, exactly as RULE 1 specifies.
* **Lines come from explicit newlines.** There is no wrap width in the model, so
  a long paragraph does not wrap by itself.
* **Direction is the face's default.** Shaping applies the face's own GSUB/GPOS
  (kerning, ligatures, marks), but a run is shaped left-to-right per line;
  bidirectional line reordering is out of scope here.
* **`letter_spacing` is applied to shaped glyph advances**, so it tracks glyphs,
  not characters (a ligature is one glyph and gets one spacing).
* **An outlined letterform is a flattened polygon** (0.1 units) with a keyhole
  bridge for its counters: it fills identically under both fill rules, but a
  designer editing vertices will find the bridge, and `OutlineText` is not a
  round-trip back to live type (undo is).
* **A big word outlined becomes a big group** — one node per glyph, which is what
  RULE 3 asks for and what makes each letterform independently editable.
* **The shaping cache is per-thread, 48 runs, keyed on exact bits.** A document
  with more than 48 distinct runs (a page of differently-sized headings, say)
  will evict and re-shape — correct, just not free — and a `font_size` that
  changes every frame (an *animated* size) misses every frame, because that *is*
  a different run. Sliding an `offset` is the case the memo exists for, and it
  is the case the counters prove.
* **Truncation is decided at the pen position, not the ink extent.** A glyph
  whose advance ends exactly on the path's end still draws, so a descender or a
  swash can overhang by a few units. Anything finer would have to measure every
  contour's bounds per frame, and it would disagree with the advance the layout
  itself used.
* **Mark positioning is fixed but not covered by the bundled face.** The bundled
  subset has no combining marks, so every offset it shapes is zero and no
  end-to-end test can exercise `on_path_pen`'s rotation; it is pinned by two
  direct unit tests instead. A host that registers a face with real mark
  positioning (DejaVu Sans, any Noto) gets the correct behaviour, which is what
  `FontProvider` is for.

---

## 7. Files touched

**New** — `crates/vectra-geometry/src/text.rs` (the engine),
`crates/vectra-geometry/tests/text_laws.rs` + its
`.proptest-regressions`, `crates/vectra-geometry/assets/vectra-sans.ttf` +
`LICENSE-VectraSans.txt`, `apps/vectra-web/src/components/TextPanel.tsx`,
`apps/vectra-web/tests/task-11-0-typography.test.ts`.

**Changed** — `crates/vectra-core/src/{document,command,component,eval,lib}.rs`
(the kind, the six commands, the slots, the font provider — and, this round,
`Document::assign_block_above` for the outline's placement),
`crates/vectra-geometry/src/{paths,scene,evaluator,diagnostic,lib}.rs` and its
`Cargo.toml` (rustybuzz + ttf-parser), `crates/vectra-wasm/src/{lib,snapshot}.rs`,
`crates/vectra-export/src/ir.rs` (a run exports as its paths) and
`crates/vectra-export/tests/export_laws.rs` (the font-independence law),
`crates/vectra-ai/src/schema.rs` (the commands' references),
`crates/vectra-dependency/src/prospective.rs` and `tests/common/mod.rs`,
`apps/vectra-web/src/{App.tsx}` and
`src/engine/{panels,commands,wire,client,draw/tools}.ts`,
`apps/vectra-web/scripts/smoke.mjs`, `Cargo.toml`/`Cargo.lock`,
`tools/vendor_deps.py` (the wbgen vendoring support), and the regenerated
`apps/vectra-web/src/wasm/*` glue and module.

---

## 8. What a designer can do now

Press `T`, click, type. Set the size, the spacing, the leading and the alignment
— and see that Size is driven by `$scale`, because the tag says so, and because
dragging the variable re-lays the word out live, with no regenerate step. Bind
the word to a circle and it rides the curve, every letter on its own tangent and
the right way up; slide the offset and it walks along the arc and parks at the
end. Hit **Outline** and the letterforms become real paths — group them, boolean
them, distort them — while the type itself is hidden, intact, and one undo away.
Export the artwork and the words travel inside the SVG, so it opens the same
everywhere, with no font to install and no substitution to fear.

---

## 9. Round two — the five architectural points

A pre-commit review asked five questions about the architecture this task ships.
Every one is answered below with the change it caused (or the reason it caused
none), the test that now pins it, and the honest remainder. Three of the five
were **defects**, not questions: the shaping cache, the handling of a run longer
than its path, and the outline's layer placement.

| # | point | verdict | change | pinned by |
|---|---|---|---|---|
| 1 | the shaping cache | a real performance gap, and a correctness trap | shape/placement split + thread-local LRU keyed on face × tipography | `an_offset_change_re_places_instead_of_re_shaping`, `a_cached_re_placement_equals_a_cold_layout`, `law2b_a_slide_equals_a_fresh_evaluation`, `law5`-adjacent face identity tests, mutation D (§5.4) |
| 2 | text longer than its path | a real defect (clamping piled glyphs at the seam) | truncate the window, count the drop, diagnose it, never rewrite the authored offset | `law2`'s window claim, `law2_reshaping…`, `a_run_slid_off_its_path_keeps_nothing_piled_at_the_seam`, `an_overlong_run_keeps_the_window_that_fits`, mutation E (§5.4) |
| 3 | `OutlineText` z-order and layer | a real defect (the block landed at the layer's tail) | `Document::assign_block_above` | `law3b_the_outline_lands_directly_above_the_type_in_its_own_layer` |
| 4 | SVG `<path>` versus `<text>` | already correct; now a law | none needed | `text_exports_as_font_independent_paths` |
| 5 | bbox stability under font fallback | already correct; now a law, plus one hardening | bad bytes fall back instead of emptying the node | `law5_a_family_name_cannot_move_the_box`, `a_broken_face_still_draws_its_words` |

### 9.1 The shaping cache (point 1)

**The finding.** `GeometryEvaluator` is a stateless `Copy` unit struct, and a
`NodeKind::Text` re-ran `resolve_face` → `shape_line` (rustybuzz) → glyph
outlining (ttf-parser) → layout on **every** frame. Dragging the `path_offset`
slider paid for rustybuzz and for walking every contour of every glyph, per
frame, to move a word a few units along an arc it had already been shaped for.

**The change.** The layout is split at the point where the expensive half ends:
`build_local_run(face, spec)` produces glyphs *in the run's own frame* (upright,
anchored about their own origin, with their arc positions and metrics), and
`layout_text` / `layout_text_on_path` only **place** that: a translation for a
straight run, and `T(pen) ∘ R(θ) ∘ T(−local_pen)` per glyph for a bound one — the
same affine the direct construction used to apply while it was still shaping. The
memo (`shaped_run`) is a 48-entry LRU in a `thread_local!`, keyed on
`(face identity, text, size bits, spacing bits, leading bits, alignment)`.

**Why the key looks like that.** The six inputs are exactly the ones the shaped
run is a pure function of; `x`/`y`, the bound path and `offset` are deliberately
*not* in it, because including them would defeat the memo and, worse, make the
cache key *the placement*. The face is in it **by identity**, not by family name:
`FontProvider::face_id` lets a provider name its bytes (the `FontLibrary`
fingerprints each face once, at registration) and a provider that cannot gets a
fingerprint computed on the way in — correct, and merely slower. Floats are
compared by bits so the key answers "the same run?" rather than "the same
number?".

**Evidence.** `an_offset_change_re_places_instead_of_re_shaping` reads the
counters: one miss for the run, a **hit** for the 120-unit slide, a second miss
only when the size changes — and it asserts the run *moved*, so a stale hit
cannot pass. `a_cached_re_placement_equals_a_cold_layout` clears the cache and
compares the two `EvaluatedText` values exactly (glyph ids, distances, angles,
advances, and every contour event). `law2b_a_slide_equals_a_fresh_evaluation`
states it at the document level: a slide must equal a fresh evaluation at the
slid offset. Mutation D (`size_bits: 0`) fails the cache law *and* the
pre-existing `font_size_scales_the_run` — a stale hit is visible geometry, which
is the whole reason the laws are written against the scene rather than against
the cache's own API.

### 9.2 Text exceeding its path (point 2)

**The finding.** The old behaviour clamped: every glyph's arc position was
clamped into the path's span, so a run slid past the end of its path stacked all
of its glyphs on the path's last point — a blot, not a word, and one that grew
with the string. Law 2's own wording ("a run may overhang the ends, which is the
documented clamping behaviour") was describing the bug.

**The change.** A glyph is drawn iff its own arc position (`offset` + the
advances before it) lies on the path; the rest are left out **at their own
positions**, so the survivors keep their spacing. The count is reported as
`EvaluatedText.truncated`, which the evaluator turns into the `text-overflow`
warning: the words that fit *and* a sentence saying how many did not, on the
diagnostics channel `get_snapshot()` already exposes. The authored `path_offset`
is never rewritten — clamping was the old bug, and rewriting the designer's
number would make the slider irreversible.

**Why the pen position.** The decision is made at the run's arc length, not at
each glyph's ink extents: it is the same measurement the layout advances by, it
is what SVG renderers clip by, and an ink-based test would need every contour's
bounds per frame. §6.4 records the consequence honestly (a descender can overhang
the path's end by a few units).

**Evidence.** `law2_every_glyph_rides_the_tangent_and_stays_upright` checks the
window against an **independent** straight layout — whose `distance` values are
the authored arc positions, because a straight run has no end to run off — and
asserts ids, positions and counts match the window that layout implies.
`law2_reshaping_the_path_moves_the_run` asserts a longer path can only widen the
window and never moves a survivor. The two deterministic tests walk a run off
both ends of a 400-unit open line (the one geometry where the two ends are
distinguishable) and through a 200-unit line with an 8-glyph run, comparing every
survivor against an unclipped reference. Mutation E (the window removed) fails
four tests.

### 9.3 `OutlineText`'s z-order and layer (point 3)

**The finding.** The command placed the group at the type's index in the *flat*
order (`insert_node(group, text_index + 1)`) — and then re-placed it: the layer
path pushed the group and its letterforms onto the **tail** of the type's layer
and resynced the flat order from the layers, which silently *overwrote* the
careful placement. In a document with layers, outlining a word that sat under a
shape moved the letterforms above it. The bug was invisible to the existing laws
because they outline in a document with no layers (where the flat order *is* the
placement) and because the smoke step checks group membership, not rows.

**The change.** `Document::assign_block_above(block, layer, anchor)` splices the
whole block (`[group, letterforms…]`, the order `node_block` returns) into the
layer immediately after the **anchor's run** — read before the block is detached,
so the block's own presence in that list cannot shift the anchor. `OutlineText`
now calls it once, with the type's layer and the type as the anchor; the old
`land_on_layer` (a tail-append helper) is gone. No layer ⇒ no-op, because the
flat order already is the placement.

**Evidence.** `law3b_the_outline_lands_directly_above_the_type_in_its_own_layer`
runs the adversarial version: the type is moved into a **second** layer, a shape
is created in that same layer *after* it, and the test then checks

* the group's layer is the type's own layer (not the active one, which is the
  first layer — the two answers differ in this document),
* the group's row inside that layer is the type's row + 1,
* the letterforms follow the group, in order, one row each,
* the shape that was above the type is still above the **whole block**, in the
  layer list *and* in the flat order,
* the type is hidden and the group is visible.

### 9.4 SVG: `<path>`, not `<text>` (point 4)

**The finding.** Already the strategy — `crates/vectra-export/src/ir.rs` exports
a run as `ExportGeometry::Path { d }` taken from the evaluated run's own outline,
and warns (`text … has no resolved geometry`) rather than emitting a `<text>`
element when the run could not be resolved. There was no law saying so, which is
how a strategy stops being one.

**The change.** None in code. `text_exports_as_font_independent_paths` now pins
four things: the node exports as a `Path` with a `d`; that `d` is **exactly**
`path_to_svg_data(run.outline)` for the same evaluation (the export re-uses the
evaluation rather than outlining a second time, so it cannot disagree with the
canvas); the SVG contains `<path>` and neither `<text` nor `font-family` nor the
family's name; and a document naming a family this machine has never seen exports
m **byte-identical file** — the letterforms travel, the font does not.

### 9.5 Bounding-box stability under fallback (point 5)

**The finding.** The run's box is measured from the *outlines* the face produced
(`primitive_bounds_of` over the evaluated run) and scaled purely by
`size / units_per_em`, so a substitution cannot move a box by a layout accident —
it can only hand back different outlines, which is a `resolve_face` question.
Two gaps around that: nothing tested it, and a provider whose bytes no shaper can
read (the `FontProvider` trait does not require validation — only
`FontLibrary::register` does) would have made the node *vanish*, taking every
constraint that reads its box with it.

**The change.** The evaluator now retries with the bundled face when a run fails
with `InvalidFace`/`DegenerateFace` from a non-bundled face, reporting the
`font-fallback` substitution exactly like an unknown family. The layout path
itself is untouched: the fallback decision happens before shaping, and the
placement (straight or bound) is resolved before it, so the retry is one closure
call.

**Evidence.** `law5_a_family_name_cannot_move_the_box` asserts, for four
spellings (`sans-serif`, `system-ui`, `DejaVu Sans`, unknown `Helvetica Neu`),
that the box *and every glyph's box* equal the bundled run's — a substitution is
allowed to change letterforms only when a host deliberately supplies different
bytes under the same family, and then it is a different face.
`a_broken_face_still_draws_its_words` drives a `FontProvider` that returns `b"this
is not a font at all"` for every family: no errors, a `font-fallback` diagnostic,
glyphs drawn, and a box equal to the bundled run's.

### 9.6 What this round did not do

* No **thread-shared** cache: the memo is `thread_local!`, because a lock on the
  hot path of every text evaluation costs more than the memo saves on this
  workload, and the wasm engine is single-threaded. A native multi-threaded host
  gets one memo per thread (still correct, just warmer per thread).
* No **prefix-shaped** cache for typing: each distinct string is its own entry
  (48 of them), which is the right granularity for editing a word, not for
  streaming one character at a time.
* No **wrap width**: lines still come from explicit newlines (§6.4).
* No **UI for the overflow diagnostic**: the warning is on the boundary's
  diagnostics channel (`get_snapshot().diagnostics`), which the shell already
  renders for font fallback and solver warnings; the panel does not yet add a
  text-specific affordance, and the canvas shows the truth — the words that fit.
* No **`.gitignore`**: the repository has none, and `target/`, `dist/`,
  `node_modules/` and `scripts/wbgen/target/` are untracked (they are the
  harness's excluded directories). A commit must name its paths — or add a
  `.gitignore` first — because `git add -A` would otherwise swallow build
  output, including the 27 MB module.
