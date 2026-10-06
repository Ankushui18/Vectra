# TASK 10.4 — Tree Editing

`TASK-10.3-REPORT.md` §5 disclosed two deviations and called them *open, and priced*:
the Layers Panel's group projection stopped at two levels, and "moving a group moves
its children" was a promise the panel could not keep in a layered document. This task
discharges both, and pays for them with four defects — three in the tree work, one in
the brush fitter that the tighter gate exposed.

A designer can now drag any row anywhere: into a group, out of one, into another
layer, to the front or the back of whatever container the pointer names. The engine
answers with a typed refusal when the drop would make a group its own ancestor, and
one *undo* takes the whole gesture back — including the layer the artwork came from.

---

## 1. The wire: one parent link per row (RULE 1)

### 1.1 `child_parent` replaces `child_groups`

The snapshot used to project the tree as `child_groups`: a nested array, one level
per group, and the panel could not ask for more. It now carries **one link per row**:

```rust
pub struct SnapshotLayer {
    children: Vec<String>,              // the layer's list, back → front
    child_names: Vec<String>,
    child_is_group: Vec<bool>,
    child_can_open: Vec<bool>,
    child_parent: Vec<Option<String>>,  // ← new: the container each row sits in
}
```

`child_parent[i]` is the container of `children[i]` — `None` for a row listed
directly by the layer. Three consequences worth stating:

* **No depth cap.** The panel derives depth by following links; nothing in the
  projection needs to know how deep the tree is.
* **A parent outside the layer reads as a root.** The array is about *this* layer's
  list, so a malformed or half-applied snapshot degrades to "top level" instead of
  inventing a container the layer does not have.
* `SnapshotTreeRow` and `tree_depth` are **gone** — they were the two-level shape's
  furniture, and the panel now computes what they used to say.

`can_open` stayed separate from `is_group` on purpose: a group with no members cannot
be opened, and the panel's disclosure triangle is disabled rather than pretending.

### 1.2 The walk: any depth, nothing hidden, nothing hung

`layerTreeRows` walks the layer's list, emits each root, then recurses into the rows
that name it — top-first, in the engine's own order. Two failure modes are handled
explicitly, because a snapshot is not a precondition, it is data:

* **A row listed twice** is emitted once. A panel that repeats a node lets the user
  drag the wrong one.
* **A row whose parent is missing** is still emitted (as a root), and **a loop** is
  broken: the rescue pass runs *only* for loop members. The first version rescued
  every unreached row, which resurrected the rows of a **closed** group — the
  subtree was correctly hidden, and the fallback undid it. The rule now is
  `closesLoop`, and the tests carry three broken-snapshot shapes (unlisted parent,
  closed chain, loop) to hold the line between "hidden" and "lost".

---

## 2. The engine: parentage is a fact, not a position

### 2.1 `Document::set_parent` — refusals before any mutation

```rust
pub fn set_parent(&mut self, id: NodeId, parent: Option<NodeId>, index: usize)
    -> Result<Option<(Option<NodeId>, usize)>, VectraError>
```

The return is the **previous** `(parent, index)` — or `Ok(None)` when the move was
already satisfied. Every refusal happens before anything moves, so a rejected drop
cannot leave a half-moved document:

| refusal | when |
|---|---|
| `NodeNotFound` | the node does not exist |
| `NotAGroup` | the destination is not a group |
| `GroupCycle { node_id, ancestor }` | the destination is inside the node's **subtree** — including itself, and including a grandchild |

`GroupCycle` is a subtree check, not a parent check: dropping a group into its own
grandchild is the same mistake as dropping it into itself, and it is the one a drag
gesture produces most often (the target row is *visible* precisely because the group
is open). The message names both ids, so the UI log reads the way a person would:
`cannot move … into …: that would make the group its own descendant`.

`Ok(None)` for a no-op is the semantics the round-trip law forced (§4.3-ish): a drag
that lands a node exactly where it already is changed nothing, so it is not an error,
and its inverse is the command itself.

### 2.2 `Command::SetNodeParent` — the inverse, the no-op, and the layer it crossed

```rust
Command::SetNodeParent { id, parent, index }   // "Move into group" / "Move out of group"
```

Events are `NodeFlagsChanged + LayerOrderChanged` with an **empty `Dirty`**: grouping
is presentation. In the smoke, a batch that creates a group and moves two nodes into
it dirties exactly the new group — the moved nodes are not re-evaluated, because
membership is not geometry.

The inverse grew up twice during this task:

1. **The no-op.** The first version reported `NodeNotFound` when the move was already
   satisfied. The round-trip law caught it (a redo after an undo landed on the
   document's current state and got an error back). It is now quiet, and its inverse
   is itself.
2. **The layer it crossed** (§4.3). A layer lists its blocks, so a subtree lives in
   exactly one layer — which means moving a node into a group that belongs to
   *another* layer quietly carries the node into that layer. The inverse restores
   parent, position **and** the layer it came from: one undo entry, two facts, because
   a designer's one drag changed two things.

```rust
// the shape of the inverse now
match previous_layer.filter(|layer| Some(*layer) != moved_layer) {
    Some(layer) => Command::batch(vec![
        Command::AssignNodeToLayer { node_id: id, layer },
        Command::SetNodeParent { id, parent: previous_parent, index: previous_index },
    ]),
    None => restore,   // the common case: one command, as before
}
```

`Command::batch` flattens nested batches by construction, so this cannot grow nesting
as more facts join.

### 2.3 The layer's list is the z-order: blocks travel

`LayerRecord::children` **is** the draw order — groups and members side by side — and
`scene.z_order` omits group nodes (they paint nothing). Reordering a group inside a
layer therefore has to move the group's whole **block**, not its id, and
`reorder_within_container` does. `ReorderNode` on a group reports an empty `Dirty`
set and carries the members with it, which is what makes "moving a group moves its
children" true in a layered document — the deviation 10.3 §5 disclosed.

---

## 3. Dragging a node: into a group, out of it, into another layer

### 3.1 The pure decision

The gesture's meaning is a **pure function of the snapshot**, tested without a DOM:

```ts
dropNodeAt(layer, target, dragged)   → { parent, index } | null
dropNodeInto(layers, targetLayerId, target, dragged) → { layer, parent, index } | null
```

Three targets, and the meaning every tree view has trained a designer to expect:

| the row you drop on | what happens |
|---|---|
| a **layer** row | the node moves to that layer's top level, in front |
| an **open group** row (`canOpen === true`) | inside the group, in front of its members |
| any other row | a sibling, directly in front of that row |

`null` means "this changes nothing" — the dragged row itself, one of its own
descendants, a no-op position, or a layer the snapshot does not have. The panel
checks descendants cheaply (`isDescendantOf`) to spare the round trip; the engine
remains the authority and makes the same refusal typed.

`dropNodeInto` is the second half of the gesture: **which layer's tree** the drop
landed in. It reads the target row's own layer, never "the layer above the row" — a
node nested in a group is listed by the layer the engine says holds it, which is also
the layer it lives in. Comparing that with the dragged node's layer is what turns one
gesture into two different commands.

### 3.2 The gestures

* **Same layer** → `SetNodeParent`, one command.
* **Another layer** → `moveNodeToLayer(node, layer, parent, index)` =
  `Batch[AssignNodeToLayer, SetNodeParent]`, in that order and only in that order:
  the move says "at `index` in the layer that lists me", so the node has to *be* in
  that layer before it can be placed inside a group of it. One batch, one undo.
* **The refusal** is the panel's own pre-check *and* the engine's typed error. The
  panel's check is a subset of the engine's on purpose: nothing in the UI is trusted
  to be complete, only to be quick.

The panel's first version routed every node drop through the **dragged** node's
layer, so dropping a row onto another layer's row silently reordered it inside its
own layer. The target row now carries its `layerId`, and the two cases are explicit.

### 3.3 Grouping a selection: one action, one undo

The ▣ button in the panel header (inert with an empty selection) sends one batch:

```ts
batch([
  createGroup(group, name),                       // name: "Group" or "Group N"
  ...selection.map((node, i) => setNodeParent(node, group, i)),
])
```

The group id is chosen by the caller so the very next command in the batch can name
it; the group lands in the layer that already holds the first selected node, and
members from other layers are carried in (§4.3). Because a batch's inverse unwinds
last-in-first-out, **one undo** restores the group's absence *and* every member's
parent and layer.

---

## 4. Four defects the laws found, and the numbers

### 4.1 Overlapping shapes were resolved by UUID — renderer

`RenderScene` rebuilds its spatial index lazily, and it rebuilt it from
`slots.values()` — a `BTreeMap<NodeId, _>`, i.e. **UUID order**. `HitIndex::candidates`
walks a bucket in reverse and hands back front-to-back, so the winner between two
overlapping shapes was whichever id sorted last. Two identical documents could answer
differently in different runs: the smoke's new step 61 (a shape inside a group,
hit-tested through the pixel it is drawn on) flipped between processes, roughly
50/50 — the first *flaky* result this project has had, and the reason it took minutes
rather than seconds to believe.

The fix inserts **back to front, in the scene's own draw order** (a slot the plan no
longer draws goes in first, so it can never outrank something on the canvas), and
`HitIndex::rebuild`'s doc comment — which said the opposite of what `candidates`
does — now says what the code requires. Two laws hold it:

| law | what it pins |
|---|---|
| `the_front_most_shape_wins_the_pixel_it_covers` | two overlapping shapes, ids deliberately **disagreeing** with draw order, both ways round |
| `z_order_decides_between_stacked_shapes` (proptest, *strengthened*) | its generated ids used to ascend with the stack, so UUID order matched draw order and it passed by luck; they now **descend** |

Both fail against the old code and pass against the new — verified by reverting it.

### 4.2 A reorder alone left the index stale — renderer

The index's inputs are bounds *and* order, so a pure z-reorder changes its answer.
`hit_dirty` is now set when the removal pass drops a slot and when the plan reports
`reordered`; and the staleness guard — which compared `HitIndex::len()` (bucket
entries, duplicates counted) against `slots.len()`, an equality that is false for
almost every scene, so the index was rebuilt on **every** pointer query — now compares
an O(1) `hit_indexed` counter. `a_reorder_alone_re_points_the_index` pins the reorder
half: same geometry, only the stack turned over, and the newly-front shape wins. It
fails without the invalidation line.

### 4.3 A cross-layer group could not be undone — engine

Grouping a selection that spanned two layers *worked*, and undoing it did not. One
undo returned the members to their old parents — and left **both of them in the
group's layer**: `SetNodeParent`'s inverse remembered parent and position and forgot
the layer the move had crossed. `law_grouping_a_selection_that_spans_layers_undoes_completely`
is the law; §2.2 is the fix; reverting the fix fails the law and nothing else. The
smoke's step 64 drives the same gesture through the wasm port.

### 4.4 The brush "fit" could throw its handles 700× the chord — Task 10.1

The full-suite gate went red on a proptest in `vectra-draw` that had been failing
intermittently and *accumulating shrink seeds* for several tasks. Two defects, nested:

1. **The law fed the fitter input its contract does not cover.** `fit_stroke` requires
   *distinct* samples (chord-length parameterisation needs a strictly positive step);
   production guarantees it by running `stroke::clean` first, in `brush_stroke` and in
   the wasm port. The law generated raw pointer noise, coincident points and all. It
   now cleans first, with `BrushOptions::default().min_distance` — the same rule, the
   same number.
2. **Cleaning did not weaken the law; it exposed what the coincidence had hidden.** On
   a nine-sample stroke with distinct points, the fitter accepted a candidate whose
   control points sat **400 000** units out (700× the chord). The acceptance test
   measured a 64-chord polyline of that curve, and a coarse polyline of a curve that
   loops far outside its own samples can pass *closer* to a sample than the curve
   does. Measured: the fitter scored the candidate at **12.17** against a 12.39
   tolerance while the true distance from one of its own samples was **57.9**.

Two fixes, both measured:

* `fit::max_error` now scales its polyline resolution with the candidate's **reach**
  (64 chords for a docile curve, up to 8× that for a long-handled one), so the
  quantity being decided is measured honestly. This is the correctness fix: with it,
  the wild candidate is rejected on its own merits and the stretch splits.
* `MAX_HANDLE_RATIO = 4.0` refuses a candidate that only "fits" by throwing its
  handles far outside its stretch. This is a **shape** rule — a fit with 400 000-unit
  handles can be within tolerance and still be a monstrous thing to edit, export or
  constrain — and the fallback is the recursion, whose worst case is the sample
  polyline, which is exact.

`a_fit_keeps_its_handles_near_its_samples` holds the shape; the fidelity law (4096
cases) holds the accuracy.

---

## 5. Tests

### Rust — 547 passed, 0 failed (was 537)

| suite | now | note |
|---|---|---|
| `vectra-dependency/tests/tree_laws.rs` | **7** | was 6; the cross-layer grouping law is new |
| `vectra-render/tests/hit_laws.rs` | **12** | was 10; stacking + reorder-repoints, and one strengthened proptest |
| `vectra-draw` unit tests | **43** | + the fitter's handle invariant |
| `vectra-draw/tests/draw_laws.rs` | 9 | the fidelity law now cleans first |
| `camera_laws` / `navigation_laws` | 7 / 7 | unchanged |
| `vectra-wasm` | 96 | unchanged |

The seven tree laws: division of labour, block travel, cycle refusal, one-undo round
trip, lasting parentage, inheritance, across-layers.

### UI — 87 passed, 0 failed (was 85)

* `RULE 1 (10.4): a tree nests to any depth, siblings in the engine's order` — three
  levels, the any-depth walk, closed-subtree hiding, and three broken-snapshot shapes.
* `RULE 1 (10.4): a drop names the container and the position it lands at`.
* `RULE 1 (10.4): a drop onto another layer names that layer, and the move is one
  undo` — the cross-layer decision plus the exact batch, in order.
* `panels-mount.test.tsx` — the group action exists, is inert with an empty selection,
  and the fixture is `child_parent`-shaped.

### Smoke — 64 steps, 0 failed (was 59)

New: 58 (a group inside a group is a row that opens — migrated to `child_parent`),
60 (grouping is one undo, and cycles are refused), 61 (the canvas draws the tree
without knowing it is one: `z_order` and `pointer_hit`), 62 (a group carries its
subtree in the z-order), 63 (a drop into another layer is one batch, one undo),
64 (a selection that spans layers undoes layer by layer).

### Gate status (all re-run after the last edit)

| gate | result |
|---|---|
| `cargo fmt --check` | clean |
| `cargo clippy --workspace --all-targets` | 0 warnings |
| `cargo test --workspace --no-fail-fast` | **547 passed, 0 failed** |
| `bash apps/vectra-web/scripts/build-wasm.sh` | ok — glue regenerated after every Rust edit |
| `npm run typecheck` | clean (app + tests) |
| `npm run test:ui` | **87 passed, 0 failed** |
| `npm run smoke` | **64 steps**, three consecutive runs, 0 failed |
| `npm run build` | js 288.68 kB (88.23 gzip), css 21.94 kB, wasm 23.07 MB (2.67 MB gzip) |
| dev server | `vite --host 0.0.0.0 --port 5173`, restarted on the final glue |

---

## 6. Deviations, stated plainly

1. **Task 10.3 §5's two deviations are discharged.** The projection is arbitrary-depth
   (§1.1), and a group's children travel with it in a layered document (§2.3).
2. **Two commands say "put it here".** `ReorderNode` (a container-local index) and
   `SetNodeParent` (parent + index). They overlap: `SetNodeParent` with the container
   a node already has *is* a reorder. Kept both because `ReorderNode` is the older,
   smaller verb the canvas and control paths already send, and folding it away would
   churn call sites for no user-visible change. A future cleanup can retire it.
3. **The panel reads top-first; the engine lists bottom-first.** "The front of the
   container" is the **end** of the engine's list. The conversion lives in exactly two
   places (the walk and `dropNodeAt`). Flipping the engine would renumber every
   fixture, every snapshot assertion and the export order — a large, silent change for
   no user-visible gain, so it is disclosed instead.
4. **A cross-layer group *relocates* its members.** A subtree lives in one layer, so
   adopting a node into a group in another layer moves the node into that layer. The
   alternative — keeping it in its old layer while its parent is elsewhere — is a tree
   the model forbids. Undo restores the layer (§2.2), and the forward direction is a
   choice, not an oversight.
5. **The hit index is invalidated by a flag.** Correctness now rests on every path
   that mutates a slot setting `hit_dirty`: `sync_node`, the removal pass, and a
   reorder. All three are pinned by laws; a fourth path that forgets it will be a
   stale-hit bug, and the guard cannot detect it for you.
6. **`MAX_HANDLE_RATIO` is a shape rule, not the correctness fix** (§4.4). The
   honest metric is what makes the fitter correct; the ratio keeps the *document*
   sane. A stretch whose honest fit wants longer handles now splits instead — more
   segments, same fidelity.
7. **The fitter's laws changed shape.** The fidelity law now cleans its input first,
   which is a statement about the fitter's documented contract
   (`fit::chord_length_params`), not a relaxation: the defect it then found was worth
   more than the one it stopped testing.
8. **Still open from 10.3:** the CSS grid is drawn from the camera rather than in the
   shader (decorative, and the same view rectangle the shader projects with), and
   navigation is renderer-side with no document state and no undo entry. Neither
   changed here.

---

## 7. Files touched

**Engine (Rust).** `crates/vectra-core/src/command.rs` (`SetNodeParent`, its no-op and
its layer-restoring inverse), `document.rs` (`parent_of`, `set_parent`,
`sibling_index`, block-contiguous layer reorder), `error.rs` (`GroupCycle`,
`NotAGroup`), `layers.rs` (`LayerRegistry::layer_of`).
`crates/vectra-dependency/src/prospective.rs` (the gate-free arm),
`tests/tree_laws.rs` (7 laws). `crates/vectra-ai/src/schema.rs` (`referenced_nodes`).
`crates/vectra-wasm/src/snapshot.rs` (`child_parent`; `SnapshotTreeRow`/`tree_depth`
removed).

**Renderer (Rust).** `crates/vectra-render/src/scene.rs` (the index's insertion order,
`hit_indexed`, the two invalidation points), `src/hit.rs` (`rebuild`'s contract),
`tests/hit_laws.rs` (2 laws + 1 strengthened proptest).

**Brush fitter (Rust, Task 10.1 territory).** `crates/vectra-draw/src/fit.rs`
(`MAX_HANDLE_RATIO`, `max_error`'s adaptive resolution, the cap's polyline in the
failed-solve branch, the handle invariant test),
`crates/vectra-draw/tests/draw_laws.rs` (the fidelity law cleans first).

**UI (TypeScript).** `src/engine/panels.ts` (`layerTreeRows`, `dropNodeAt`,
`dropNodeInto`, `isDescendantOf`), `src/engine/wire.ts` (`SetNodeParent`, `Batch`),
`src/engine/commands.ts` (`setNodeParent`, `batch`, `createGroup`,
`moveNodeToLayer`), `src/components/LayersPanel.tsx` (the group button, draggable
rows, the drop decision, the cross-layer branch), `src/App.tsx` (`groupSelection`),
`tests/panels.test.ts`, `tests/panels-mount.test.tsx`, `scripts/smoke.mjs`.

---

## 8. What a designer can do now

1. **Open a group and see it** — a group inside a group is a row that opens again, at
   any depth, with the same disclosure triangle a layer has.
2. **Drag a shape into a group** — drop it on an open group row and it becomes a
   member, in front of the others. Undo takes it back out in one step.
3. **Drag a shape out** — drop it on a sibling, or on the layer row, and it leaves the
   group for that position.
4. **Drag a shape into another layer** — one gesture, one undo entry, and the layer's
   list is where the drop said it would be.
5. **Select across layers and press ▣** — one group, in the first selected node's
   layer, one undo, and the members go back to their own layers when you undo.
6. **Try to make a group its own parent** — the canvas refuses, in words: *"cannot
   move … into …: that would make the group its own descendant"*, with the document
   untouched.
7. **Click overlapping shapes** — the one you can see on top is the one you get, in
   every run, and after every reorder.
