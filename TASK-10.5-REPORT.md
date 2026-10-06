# TASK 10.5 — Consolidation: One Placement Verb, an Index That Cannot Go Stale

`TASK-10.4-REPORT.md` §6 disclosed two leftovers and called them the next
increment's business: **two commands said "put it here"** (`ReorderNode` and
`SetNodeParent`, §6.2), and **the hit index was invalidated by a flag** nobody was
forced to set (§6.5). Both are discharged here — and chasing the first one turned
up two more defects in the same area, one of them a *live* one in the surviving
verb. The brush fitter's fidelity law also failed again on a fresh run, which
turned out to be measurement, not fitting: that is fixed too (§4.3).

The result is a smaller engine with fewer ways to be wrong: one command places a
node, one Document method decides where a placement lands, and the renderer's
spatial index decides for itself when it is stale.

---

## 1. One placement verb (RULE 1)

### 1.1 What was retired

```rust
Command::ReorderNode { id, index }        // ← gone
Document::reorder_within_container(…)     // ← gone with it
```

`SetNodeParent { id, parent, index }` says everything `ReorderNode` said, and one
thing more: **which container** the index counts in. A reorder is a move whose
parent is the container the node is already in, so nothing needs the second verb.

Call sites migrated (every one of them, verified by grep at the end — the only
mentions left in the tree are the comments that explain the retirement and the
smoke step that asserts it is gone):

| site | was | is |
|---|---|---|
| `DeleteNode`'s inverse | `ReorderNode { index: <layer slot> }` | `SetNodeParent { parent, index: <sibling index> }` |
| `vectra-ai::schema` | its own arm | folded into `SetNodeParent`'s |
| `vectra-dependency::prospective` | gate-free arm | removed (10.4's arm already covers it) |
| `workspace_laws` | `ReorderNode` on a group | `SetNodeParent { parent: None, index: 0 }` |
| `tree_laws` ×2 | `reorder_within_container` | `set_parent` |
| UI `commands.ts` / `wire.ts` | `reorderNode` builder + wire variant | **deleted** (the builder was unused) |
| smoke ×3 | `ReorderNode` sends | `SetNodeParent` sends |

A command the engine no longer names is a **typed error** at the wasm boundary, not
a silent no-op — smoke step 64 asserts exactly that, with the message the log
shows.

One incidental improvement: `DeleteNode`'s inverse used to restore the node's slot
*in the layer's list*; it now restores its **sibling index**, which is the same
number only when the node was at the layer's top level. Deleting a member of a
group and undoing it now puts it back among its siblings instead of at the group's
top level.

### 1.2 The defect it was hiding

The retired verb took a bare `index`, and in a layered document its "container"
was the **layer** — so reordering a member of a group spliced that node into the
layer's list on its own. Measured through the wasm port, on a document whose
group `Pair` held `C` and `B`:

```text
before          rows: A  C→Pair  B→Pair  Pair      z: A C B      (consistent)
ReorderNode C → 0
after           rows: C→Pair  A  B→Pair  Pair      z: C A B      (C still in Pair,
                                                                 but listed first)
```

The panel walks **parent links**, so it drew `C`'s row inside `Pair`; the canvas
reads **the layer's list**, so it drew `C` behind `A`. Two views of one document,
disagreeing about what is on top — and the layer no longer held the group's run as
one unit. `law_the_layer_lists_every_group_as_one_run` (§3.1) is the law that names
the shape; it fails against a reintroduced layer-scoped splice.

---

## 2. Placement lands *between* runs (RULE 2)

### 2.1 The live defect

Retiring the old verb left one placement path, and writing its law found a second
defect in it. `Document::container_index` resolved the destination position as the
position of **the anchor row itself**:

```rust
Some(anchor) => record.children.iter().position(|child| child == anchor)
```

A row is the **end** of its own run. Children are drawn behind their container (and
a group paints nothing), so a group's block reads `[children…, group]` — which
means "insert immediately before the anchor row" splices the moved block *inside
the anchor's run* whenever the anchor is a group. Dropping a shape onto a **closed
group row** — the panel's most ordinary gesture — produced:

```text
before   [A, B, C, G]         (A a plain shape, G a group holding B and C)
drop A onto G's row → SetNodeParent { id: A, parent: null, index: 0 }
after    [B, C, A, G]         ✗ A is inside G's run: the panel calls it a top-level
                                row, the canvas draws it in front of B and C
```

The fix is `Document::run_start` / `run_end`: the destination is the **start of the
anchor's run** (or one past the end of the last sibling's run when the index is
past the container). For a leaf that is the leaf itself; for a group it is the
group's first child — so the block always lands *between* two runs:

```text
after    [A, B, C, G]         ✓ A is behind the whole run, and the walk agrees
```

### 2.2 Why it matters

The invariant is the one the whole panel/canvas pair rests on: **the layer's list
is a valid walk of the tree** — read the list, resolve parent links, and you get
the same order the canvas paints. Every placement now preserves it, and it is
asserted directly (§3.2). This is also what makes 10.2 R1's "moving a group moves
its children" true in layered documents *and* what makes the panel's top-first
reading of a group's contents agree with the z-order underneath it.

---

## 3. The laws

### 3.1 `law_the_layer_lists_every_group_as_one_run` (tree_laws 8th)

A sequence of six placements — the group's canonicalising move, the group to the
front and to the back, a member reordered *inside* its group, a node dropped into
the middle of a group, and a member moved *out* — asserting after each one:

* no group's run is scattered (every block occupies one contiguous run, in document
  order);
* `order_matches_layers()` still holds;
* and at the end, walking the tree reproduces the draw order exactly.

The fixture is canonicalised first, and the law says why: `CreateNode { Group {
children } }` leaves a group listed *after* its members, which is the shape 10.2
and 10.3 documented, and a placement can only repair it. From the first placement
on, nothing may scatter a run.

It is the law that found §2.1: reverted to the anchor-row rule, the law fails —
and so does `law_moving_a_group_moves_its_children`, independently, which is the
kind of coincidence that says the rule is wrong rather than the expectation.

### 3.2 `a_hit_query_rebuilds_only_when_the_index_inputs_changed` (hit_laws 13th)

See §4.

---

## 4. An index that cannot go stale (RULE 3)

### 4.1 The flag is gone

The renderer's spatial index used to be guarded by `hit_dirty: bool` — a promise
every mutating code path had to keep — and Task 10.4 had to disclose that a
forgotten flag would serve a stale answer. The guard is now **a comparison of the
index's own inputs**:

```rust
fn index_inputs(&self) -> impl Iterator<Item = (NodeId, Bounds)> + '_ { … }
// ensure_index: rebuild iff the live inputs differ from the stored ones
```

The inputs are exactly what the index is built from: each slot's world box, and
the order the rows are painted in (a slot the draw plan no longer paints goes in
first, so it can never outrank something on the canvas). There is **nothing left to
remember**, so a path added tomorrow cannot invalidate wrongly — it can only
change the inputs, which the comparison sees.

`hit_dirty` is deleted, along with all three places that set it. Two of those were
doing unnecessary work: the **visibility/lock toggle** is a pair of booleans
`hit_test` reads at query time, so it needs no rebuild at all — RULE 4's "a flag
toggle is free" is now free down to the spatial index, and the law asserts it.

### 4.2 The cost, stated

`ensure_index` is now O(n) per query — one comparison pass over the slots, plus the
small set of drawn ids (the same order as `HitIndex::candidates`, which already
allocates) — against an O(n·cells) rebuild it usually avoids. Queries are
pointer-driven, n is the number of nodes on the canvas, and the index is still
lazy: the comparison happens when a pointer lands, not when the scene changes.

A counter, `RenderScene::hit_rebuilds()`, makes the contract observable. The law
holds it to: one build for the first query, **no** builds for the next four, none
for a visibility toggle, exactly one each for a geometry edit, a reorder and a
removal, and none for a sync that changes nothing. A reorder test that predates
this task (`a_reorder_alone_re_points_the_index`) still passes unchanged — it was
written against the flag and is satisfied by the comparison.

### 4.3 The fitter's metric was approximate (Task 10.1 territory, found by the gate)

The full-workspace gate failed again on `fitted_curves_stay_near_their_samples`,
this time by 0.0033 units at a 5.1633 tolerance — 0.06%, nothing like the 58-vs-12
blow-up fixed in 10.4. The arithmetic said why: the fitter *accepts* a candidate by
measuring it against a **chord polyline**, and a polyline lies inside the curve it
approximates. Its error is the sagitta — `max|B″|·h²/8`, which for that curve at
its resolution is ~0.003 units. So the acceptance test could accept a curve whose
*true* deviation was tolerance + 0.003, and the law, measuring with a different
resolution, caught the difference.

Two fixes, both principled:

1. **`fit::max_error` is now conservative.** It adds the polyline's own sagitta
   bound (`fit::chord_sagitta_bound`, derived from `|B″| ≤ 6·max(|P0−2P1+P2|,
   |P1−2P2+P3|)`) to the measured value, so `max_error <= tolerance` is a statement
   about the **curve**, not about a polyline drawn near it. Fits split marginally
   more often; the quarter-circle unit test still fits in its usual segment count.
2. **The law accounts for its own measurement.** It measures against 200 chords per
   slice, so it adds that polyline's sagitta bound — computed, not a fudge factor —
   instead of the previous `1e-6` allowance that pretended the discretisation was
   free.

20 000 proptest cases pass. The old seeds in `draw_laws.proptest-regressions` are
still there and still pass.

---

## 5. Tests

### Rust — 549 passed, 0 failed (was 547)

| suite | now | change |
|---|---|---|
| `vectra-dependency/tests/tree_laws.rs` | **8** | `law_the_layer_lists_every_group_as_one_run` (+2 uses migrated) |
| `vectra-render/tests/hit_laws.rs` | **13** | `a_hit_query_rebuilds_only_when_the_index_inputs_changed` |
| `vectra-draw` unit + `draw_laws` | 43 + 9 | the conservative metric (20 000-case stress) |
| `camera_laws` / `navigation_laws` / `workspace_laws` | 7 / 7 / 8 | unchanged (one migrated to `SetNodeParent`) |

### UI — 87 passed, 0 failed

Unchanged: the panel's own decision function was right; the *engine's* placement
was not. No UI behaviour changed, and the two `ReorderNode`-shaped types are gone
from `wire.ts`/`commands.ts` with the typecheck serving as the proof that nothing
referenced them.

### Smoke — 64 steps, 0 failed

* **Step 64 (new): runs, not rows, and one verb.** A drop onto a closed group row,
  sent exactly as the panel sends it, lands the node *behind the group's whole run*;
  the walk of the tree reproduces the layer's list; `scene.z_order` shows the
  shapes in that order; and `ReorderNode` is refused as an unknown command. It is
  the end-to-end form of §2.1 — the same document that used to draw `A` in front of
  a group's members.
* **Steps renumbered.** The banners had drifted denominators (`/56`, `/57`, `/59`)
  from earlier tasks, and a step numbered 25 that no longer exists. They are now
  contiguous `1..64` with one denominator, which is what the run prints.

### Gate status (all re-run after the last edit)

| gate | result |
|---|---|
| `cargo fmt --check` | clean |
| `cargo clippy --workspace --all-targets` | 0 warnings |
| `cargo test --workspace --no-fail-fast` | **549 passed, 0 failed** |
| `PROPTEST_CASES=20000` on the fitter's fidelity law | pass |
| `bash apps/vectra-web/scripts/build-wasm.sh` | ok — glue regenerated after the last Rust edit |
| `npm run typecheck` | clean (app + tests) |
| `npm run test:ui` | **87 passed, 0 failed** |
| `npm run smoke` | **64 steps**, three consecutive runs, 0 failed |
| `npm run build` | js 288.68 kB (88.23 gzip), css 21.94 kB, wasm 23.05 MB (2.66 MB gzip) |
| dev server | `vite --host 0.0.0.0 --port 5173` |

Environment note: the sandbox reset mid-task (toolchains, `node_modules` and
`target/` gone; sources intact). Rust, `wasm32-unknown-unknown`, `wasm-bindgen`
0.2.129 and the npm tree were reinstalled from the recorded recipes, and every gate
above was run afterwards.

---

## 6. Deviations, stated plainly

1. **`Command::ReorderNode` is removed, not deprecated.** A hand-written or
   AI-generated `ReorderNode` is now a typed parse error. Nothing in the repo
   referenced it but the sites migrated above; no persisted artefact contains
   commands (`.vectra` stores a `Document`), so there is nothing to keep it alive
   for.
2. **One label covers two gestures.** `SetNodeParent` labels itself "Move into
   group" / "Move out of group" from its intent, and a reorder within a group now
   arrives as the former. The engine's `label()` reaches the AI's record, not the
   panel's history entry (the panel names its own commands), so the wording is
   cosmetic — but it *is* the reason the label was not rewritten to something
   neutral: "move a node into a group" is what a designer did.
3. **The index's laziness is now O(n) per query.** Disclosed with the numbers in
   §4.2; the alternative (a revision counter bumped by every mutating method) is
   the flag again with extra steps.
4. **`hit_rebuilds()` is a public diagnostic.** It exists so a law can hold the
   laziness contract. It is not part of the wasm surface.
5. **The fitter's acceptance is conservative, so it splits slightly more often.**
   That is the price of `max_error` meaning what it says. Measured effect: the
   quarter-circle unit test's segment count is unchanged.
6. **The smoke's step numbers changed** (renumbered contiguously). Reports before
   this one refer to the old numbers; the titles are the stable identifiers.
7. **`DeleteNode`'s inverse changed semantics for grouped nodes** (§1.1) — a
   strict improvement, and one that no test depended on.
8. Still open from 10.3/10.4: the CSS grid is drawn from the camera rather than in
   the shader, and navigation remains renderer-side with no document state.

---

## 7. Files touched

**Engine.** `crates/vectra-core/src/command.rs` (`ReorderNode` and its four arms
removed; `DeleteNode`'s inverse now records `(parent, sibling index)`),
`document.rs` (`reorder_within_container` removed; `run_start`/`run_end` added and
`container_index` resolved through them; `set_parent`'s doc no longer cross-refers to
a verb that does not exist).

**Dependents.** `crates/vectra-ai/src/schema.rs`, `crates/vectra-dependency/src/prospective.rs`,
`crates/vectra-dependency/tests/workspace_laws.rs`,
`crates/vectra-dependency/tests/tree_laws.rs` (2 migrations + 1 new law, plus the
`scattered_group` invariant helper).

**Renderer.** `crates/vectra-render/src/scene.rs` (`hit_dirty`/`hit_indexed` →
`hit_inputs`/`hit_rebuilds`, `index_inputs`, the comparison in `ensure_index`),
`crates/vectra-render/tests/hit_laws.rs`.

**Brush fitter.** `crates/vectra-draw/src/fit.rs` (`chord_sagitta_bound`, the
conservative `max_error`), `crates/vectra-draw/tests/draw_laws.rs` (the law's own
bound).

**UI.** `src/engine/commands.ts`, `src/engine/wire.ts` (`reorderNode` and its wire
variant removed), `scripts/smoke.mjs` (3 sends migrated, 1 step added, all steps
renumbered).

---

## 8. What a designer can do now

Nothing looks different, and that is the point: the same gestures, on an engine
with fewer ways to be wrong.

1. **Drop a shape onto a closed group row** — it lands in front of the group as a
   whole (as the row suggests), not buried between the group's members. The panel
   and the canvas agree about it, which they did not before.
2. **Reorder a member inside a group** — it moves among its siblings, and the
   group's run travels as one unit.
3. **Undo a delete of a member of a group** — it comes back among its siblings, in
   the layer it was in.
4. **Click overlapping shapes** — unchanged, and now with the guarantee that the
   index was built from the boxes and the stacking order actually on screen.
5. **Toggle an eye or a lock** — nothing at all moves: not the geometry, not the
   buffers, not the spatial index.
