# TASK 3.2 — Direct Manipulation (dragging with constraints)

**Scope.** The solver exists (3.1); this task makes it *feel* interactive: the
pointer drives Cassowary's own edit-variable API, the constrained partners follow
in real time, and the whole gesture is one undo step. The React UI stays a dumb
remote — it ships pointer coordinates as JSON commands and renders whatever
snapshot comes back.

**Locked rules from the authorization**

1. Use Cassowary's native `add_edit_variable` / `suggest_value` — never fake a
   drag with repeated `SetParameter` + full re-solve.
2. The command triad is `BeginDrag { node_id }`, `UpdateDrag { node_id, x, y }`,
   `EndDrag { node_id }`.
3. The UI sends mouse coordinates only; it computes no geometry.

**Status: LANDED (2026-10-05).** This is the plan of record, kept as written;
§8 is annotated with what each step produced and §9 records the four places the
implementation deliberately deviates from the sketch (plus why). Results and
gate numbers live in `TASK-3.2-REPORT.md`.

---

## 1. The triad, and what each member does

| command | document effect | engine effect | history entry |
| --- | --- | --- | --- |
| `BeginDrag { node_id }` | none | build the **drag session**: plan the live rules, load the tableau, add the node's position slots as STRONG edit variables (all other live slots stay WEAK), suggest their current values | **none** — the gesture has not moved anything yet |
| `UpdateDrag { node_id, x, y }` | the writes the solve produced | `suggest_value(x)` / `suggest_value(y)`, read every session slot back, apply the changed ones through the ordinary inverse-returning path | **none** — folded into the drag's single entry |
| `EndDrag { node_id }` | final pass (rules re-assert ⇒ strength-based snap-back) | `remove_edit_variable` for every active edit, drop the tableau, garbage-collect the pool | **one** — `record(forward = net movement, backward = pre-drag state)` |

### 1.1 Why one entry, not three

The 3.1 report-back locked **D6: one user action = one history entry**, and the
Undo law depends on it (“undo ⇒ geometry reverts”). A drag is one user action.
`BeginDrag` and `UpdateDrag` mutate no document state that has its own inverse —
their only durable effect is the geometry the solver moved — so the honest
representation is a single entry whose forward command is the *net* movement and
whose backward command is the exact pre-drag state (captured as the **first**
inverse per slot, so a slot that moves 100 → 130 → 90 still restores to 100).

* `EndDrag` with no movement (a click) pushes **nothing** — no wasted undo step.
* Undo/redo of a drag needs no re-solve: the entry replays literals, and the
  engine's post-command solve pass is a no-op on values that already satisfy the
  rules (it is also what re-asserts a weaker rule after a snap-back).
* A drag that moves a *variable-driven* slot breaks that link exactly as a
  solver write does in 3.1 (`apply_untracked` + `ParametricLinkBroken`), and the
  backward command restores the **`Parameter`** (the link), not just the value.

### 1.2 New core API

```rust
Command::{BeginDrag{node_id}, UpdateDrag{node_id, x, y}, EndDrag{node_id}}   // serde: {"type":"UpdateDrag",…}
CommandStack::record(forward: Command, backward: Command, label) -> EngineEvent
VectraError::{DragActive{node_id}, DragNotActive{node_id}, NotDraggable{node_id}}
NodeKind::position_slots(&self) -> Option<(&'static str, &'static str)>      // ("x","y") | ("cx","cy") | None
```

`record` is the only stack addition: it registers an action whose effects are
*already applied* (`execute` = apply-then-push; a drag is apply-while-dragging,
push-once-at-the-end). It does the same bookkeeping — push, clear redo, trim —
so an entry recorded this way is indistinguishable from any other.

## 2. The grab offset, and the "absolute coordinates" contract

`UpdateDrag { x, y }` is **absolute**: it means “put this node's position slots
at (x, y)”. That is the contract the Drag Partner law asserts
(`BeginDrag(A); UpdateDrag(A, x=200)` ⇒ `B.x == 200`), and it keeps the engine
free of pointer state.

Grab feel is therefore an **input-mapping** concern, and it lives in the UI as a
tested pure helper (`view-model.ts`): at `mousedown` the UI records
`offset = pointer − node.position` from the snapshot the engine just gave it,
and each move sends `pointer − offset`. That is event→command translation, not
geometry: the UI never knows what a constraint does, never resolves a parameter,
and never touches a shape's size. (If we ever want pixel-perfect multi-touch
handles, the offset can move into `BeginDrag` as an extra field — noted, not
done.)

The engine maps `x`/`y` onto the node's **canonical position slots**
(`NodeKind::position_slots`), so a circle drags on `cx`/`cy` and a rectangle on
`x`/`y` without the UI knowing the difference. Constraint targets are
canonicalised at the boundary for the same reason: `x` on a circle and `cx` are
one slot, so they must be one Cassowary variable.

## 3. The drag session (`vectra-constraints`)

```rust
pub struct ConstraintSolver {
    pool: VariablePool,
    stats: SolverStats,
    drag: Option<DragSession>,          // ← the active-edits registry
}

pub struct DragSession {
    node_id: NodeId,
    edits: Vec<ConstraintTarget>,       // the pointer-driven slots (1–2)
    solver: cassowary::Solver,          // persistent for the life of the gesture
    slots: BTreeMap<String, (ConstraintTarget, Variable, f64)>,  // label → (target, var, last read-back)
    rows: usize,
}

impl ConstraintSolver {
    fn begin_drag(&mut self, doc, ctx, node_id) -> Result<SolveOutcome, ConstraintError>;
    fn update_drag(&mut self, doc, ctx, x: f64, y: f64) -> Result<SolveOutcome, ConstraintError>;
    fn end_drag(&mut self) -> Result<(), ConstraintError>;
    fn active_edits(&self) -> usize;     // registry size — the Persistence law's observable
    fn is_dragging(&self) -> bool;
}
```

* One **tableau** is built at `BeginDrag` by the same code `solve()` uses
  (factored into `build_tableau`), so a drag and a dispatch can never disagree
  about what is in force. The plan’s drops/skips are reported once, at drag
  start.
* `build_tableau` interned slots: every slot of every live rule (WEAK stays) plus
  the dragged slots (STRONG, even if no rule references them — that is what makes
  dragging an unconstrained node work).
* `update_drag` = `suggest_value` ×2 → `get_value` for every session slot →
  different from the document's current value ⇒ a `SolveWrite`. Re-reads the
  **document** each update, so the writes are always relative to reality.
* `end_drag` calls `remove_edit_variable` for each active edit **explicitly**
  (the registry must be observably empty, not merely dropped), then discards the
  tableau and runs `pool.gc` against the live rules only, so a variable that
  existed only for the gesture is released (`solver.variables` falls back).
* `SolverStats` gains `active_edits`; the snapshot's `solver` block gains
  `edits`, so “the registry is empty” is assertable over the wire.

## 4. Interaction guards

A live session holds a tableau that must not go stale, so:

| situation | behaviour |
| --- | --- |
| any command other than `UpdateDrag`/`EndDrag` for the dragging node (including `undo`/`redo`/another `BeginDrag`) | typed `VectraError::DragActive` — nothing applied |
| `UpdateDrag`/`EndDrag` for a *different* node, or with no drag open | typed `VectraError::DragNotActive` |
| `BeginDrag` on `path` / `group` (no position slots) | typed `VectraError::NotDraggable` |
| required-vs-required conflict already in the registry | the existing preflight (`UnsatisfiableConstraints`) still runs first |

Rejections are ordinary error envelopes: the UI logs them in red and clears its
local drag state. Bare `get_snapshot` / `dependencies` reads stay allowed during
a drag (the UI needs them to render).

## 5. Events

`EngineEvent::{DragStarted{node_id}, DragEnded{node_id}}` — the log gets one line
per gesture, not one per pointer sample; the per-update movement is already
`NodesUpdated` + `Dirty{ids, mode: incremental}`, which is what the canvas and
the layer highlighting consume.

## 6. Laws (`crates/vectra-wasm/tests/drag_laws.rs`)

| law | script | assertion |
| --- | --- | --- |
| **Drag Partner** | A.x=100, B.x=300, `Vertical(A.x,B.x)`; `BeginDrag(A)`; `UpdateDrag(A, 200, …)` | `A.x == 200.0` **and** `B.x == 200.0` exactly; dirty = {A,B}; `solver.edits == 2`, `full_evals` unchanged |
| **Resistance (Required)** | A pinned by `Angle(x)=100` and `Angle(y)=50` at `Required`; `BeginDrag(A)`; `UpdateDrag(A, 500, 500)` | A stays at (100, 50), **zero writes**, solver stable (no error, no crash); `EndDrag` ⇒ registry empty, pin still in force |
| **Snap-back (weaker)** | same pins at `Medium` | during the drag A follows the pointer (STRONG edits beat Medium); after `EndDrag` the rule re-asserts and A returns to the pinned value |
| **EndDrag Persistence** | drag a `$base`-driven B.x (link breaks, diagnosed) then `EndDrag` | the slot is written back as a literal (`position.x_source == "literal"`, graph edge gone), `solver.edits == 0`, `solver.variables` back to the rule-referenced count, `$base` untouched |
| **One undo per drag** | drag A with a partner; one `undo` | both A and B return to pre-drag values; one `redo` returns both; the entry count grew by exactly 1 |
| **Rejections** | `UpdateDrag` with no drag; `BeginDrag` on a path; `undo` mid-drag; second `BeginDrag` | typed errors, nothing mutated, session intact |

Plus unit tests in `vectra-constraints` for the session itself (registry size,
gc after end, idempotent `update_drag` with the same coordinates).

## 7. UI

* `wire.ts` — the three commands + `DragStarted`/`DragEnded` events, and
  `SnapshotNode.position { x, y, x_source, y_source }`.
* `commands.ts` — `beginDrag(id)`, `updateDrag(id, x, y)`, `endDrag(id)`.
* `view-model.ts` — `pointerToCanvas(svg, clientX, clientY)` (screen → SVG user
  units via the inverse CTM; pure, takes a matrix-like object so it is testable
  without a DOM) and `dragTarget(pointer, grabOffset)`; plus `formatEvent` lines
  for the drag events.
* `App.tsx` — `onMouseDown` on each shape (records the grab offset from
  `node.position`, sends `BeginDrag`), `onMouseMove`/`onMouseUp` on the SVG root
  (sends `UpdateDrag` per sample, `EndDrag` on release), a `.dragging` class on
  the shape, and a `drag-chip` in the header while a gesture is live. Snapshot
  pulls are coalesced to one per animation frame; every pointer sample still
  reaches the engine.
* `smoke.mjs` — steps 19–21: drag a partner, resistance under a required pin,
  one-undo/redo, registry empty at the end.

## 8. Order of work (each step gated)

1. ✅ core: `NodeKind::position_slots` / `canonical_slot`, the triad, labels,
   preview/apply arms, `EngineEvent::{DragStarted,DragEnded}`,
   `CommandStack::record` + `Engine::record`, `VectraError::{DragActive,
   DragNotActive,NotDraggable}` + `is_drag_error`.
2. ✅ constraints (`solver.rs`, 1 101 lines): `DragSession` + private `Tableau`,
   `begin_drag` / `update_drag` / `end_drag`, `SolverStats.active_edits`,
   `ConstraintError::{DragBusy,NoDragSession,NotDraggable}`, crate exports.
3. ✅ wasm (`src/lib.rs`, 1 594 lines): `DragTxn`, the triad branches, the
   drag-active guard on *every* other command, `constraint_pass` →
   `ConstraintPass { events, diagnostics, inverses, writes, disabled }`,
   `positions()`, snapshot `solver.active_edits` / `solver.drag_node` /
   `node.position`.
4. ✅ `vectra-dependency`: triad → `ProspectiveEdges::empty()`, drag events
   ignored (compile gate + the fidelity property still holds).
5. ✅ laws + unit tests (§6): `crates/vectra-wasm/tests/drag_laws.rs` (8 tests)
   and two session tests in `solver.rs`; full workspace gate green.
6. ✅ UI (RULE 3) + smoke steps 19–21: triad builders, `PreviewNode` pointer
   handlers, window-level samples/release, `dragStatus`, wasm-level smoke.
7. ✅ `TASK-3.2-REPORT.md` (triad, edit integration, test results, gate status).

### 8.1 What each gate proved

| gate | command | result |
| --- | --- | --- |
| core compiles, all targets | `cargo check --workspace --all-targets` | clean, zero warnings |
| drag laws | `cargo test -p vectra-wasm --test drag_laws` | 8/8 |
| solver session units | `cargo test -p vectra-constraints` | 22/22 |
| whole workspace | `cargo test --workspace --no-fail-fast` | 30 suites, 181 passed, 0 failed |
| lint | `cargo clippy --workspace --all-targets -- -D warnings` | clean |
| end-to-end (real wasm) | `node apps/vectra-web/scripts/smoke.mjs` | 21/21 |
| UI logic | `npm run test:ui` | 16/16 |

---

## 9. As built — the four deliberate deviations

The plan above was written before the code; these are the places where the
implementation is deliberately different, with the reason. Nothing else moved.

1. **`UpdateDrag` still previews `NodesUpdated { ids: [node_id] }`, not a full
   dirty set** (§5). The engine's `preview_events` run *before* the solve, so
   they cannot know which partners the solver will move. The real answer is the
   `Dirty` event the solve emits (dragged node + every partner), which is what
   the canvas and the layer highlighting consume; the preview stays the honest
   "this node's slots moved" intent.
2. **The first sample of a gesture re-solves once, then suggests** (§3). With
   live rules the initial `read_back` write (position → literal) changes the
   system, so `begin_drag` performs one ordinary solve before registering the
   edits; every later sample is pure `suggest_value`. RULE 1 is untouched: the
   *drag* is native editing — the one-shot seeding solve is the same code path
   `solve` already used.
3. **Mid-gesture undo/redo is refused, typed** (§4). The design sketched
   "mid-drag undo is a UI problem"; the landed protocol makes the engine the
   guard: while a gesture is live every other command — `Undo`, `Redo`,
   `SetTime` included — returns `VectraError::DragActive` before anything is
   applied. The UI therefore cannot corrupt a session even if it tries.
4. **No pointer-sample throttling, and no coalescing** (§7). The solver measures
   ~1.2µs and a sample is one `suggest_value`; the UI sends one `UpdateDrag` per
   pointer event and re-reads the snapshot per sample, which is what makes the
   partners move in real time. The design's "coalesce to one snapshot per
   animation frame" was dropped as unnecessary: measure first, optimise later.

Two smaller notes: `SnapshotNode.position` is `Option<_>` (absent ⇒ Phase 1 has
no float slot to drag, e.g. `Path`/`Group`), and the grab offset lives in the UI
as `pointer − node.position` — a view offset, not geometry, which is the only
arithmetic the component performs.
