# TASK 3.2 — Direct Manipulation: report

**Status: complete.** Dragging runs through Cassowary's native edit-variable API,
the command triad lives in `vectra-core` with exact inverses, constrained
partners move in real time, and the React UI remains a dumb remote. Workspace
gate: **30 suites, 181 passed, 0 failed**, `fmt` + `clippy -D warnings` clean,
smoke **21/21**, UI **16/16**.

Plan of record: `TASK-3.2-DESIGN.md` (250 lines — §8 annotated with what each
step produced, §9 records the four deliberate deviations and why).

---

## 1. The command triad (`vectra-core`)

| command | document effect | history | events |
| --- | --- | --- | --- |
| `BeginDrag { node_id }` | **none.** Validates the node exists and that `NodeKind::position_slots()` names two slots; otherwise `VectraError::NotDraggable`. | no entry | `DragStarted { node_id }` |
| `UpdateDrag { node_id, x, y }` | writes `x`/`y` as `Parameter::Literal` (absolute **document** coordinates) through the engine's ordinary untracked-apply path, so every write carries an exact reversed-batch inverse | no entry | `NodesUpdated { ids: [node_id] }` (intent) + the solve's real `Dirty` |
| `EndDrag { node_id }` | none beyond finalising: the gesture's accumulated movement is recorded as **one** entry | one entry — only if something moved | `DragEnded { node_id }` |

* **Slots per kind:** `Rectangle → (x, y)`, `Circle | Arc → (cx, cy)`,
  `Path | Group → none` (not draggable in Phase 1). `NodeKind::canonical_slot`
  folds aliases onto one name (`x`/`center.x` → `cx`, `r` → `radius`,
  `start` → `start_angle`, `end` → `end_angle`, `radius` → `corner_radius`), so
  the variable pool cannot fragment over spelling.
* **Errors:** `VectraError::{DragActive, DragNotActive, NotDraggable}` +
  `is_drag_error`; all three are typed, atomic (nothing applied), and surfaced as
  `{status:"error", message}` on the wire — never a panic.
* **Inverses for Undo/Redo:** `apply` returns a `Batch` whose backward half
  restores the previous parameters, so the gesture undoes as one step and redo
  replays the net movement **without** re-entering the solver.
* **Carried boundary from 3.1 (the `constraint` graph edge kind) — resolved: no
  new edge kind is needed.** A gesture propagates as ordinary *property writes*:
  the slots the solve moves are applied as `SetParameter`, so the existing
  property/expression edges already carry the dirty set. `vectra-dependency`
  maps the triad to `ProspectiveEdges::empty()` (a literal write can only
  *remove* edges, never create a cycle) and ignores the two drag events.

## 2. Cassowary edit integration (`vectra-constraints`, `solver.rs` 1 101 lines)

RULE 1 is the shape of the code, not a comment: the drag path never calls
`SetParameter` in a loop.

```
begin_drag(doc, ctx, node_id) -> SolveOutcome
  DragBusy        if a session is already live
  Unsatisfiable   if the plan has a hard conflict
  NotDraggable    if position_slots() is empty
  ── one-shot seeding solve (see deviation §9.2 of the design) ──
  pointer slots  -> add_edit_variable at STRONG  (even with zero rules)
  other live slots -> WEAK, so the pointer always outranks them
  read_back writes => the document holds the values the tableau already agrees with

update_drag(doc, ctx, x, y) -> SolveOutcome
  NoDragSession when idle
  suggest_value(pointer.x, x); suggest_value(pointer.y, y)   ← native edit API
  get_value for every live slot; write ONLY the slots that changed
  dropped = 0, skipped = 0   (a pointer sample cannot drop or skip a rule)

end_drag(doc) -> usize
  explicit remove_edit_variable loop (never left to Drop)
  tableau dropped; pool.gc() to the live keep-set (plan(doc).kept -> rows::targets_of)
  returns the number of released edit variables
```

* The **active-edits registry** is `ConstraintSolver.drag: Option<DragSession>`
  in `vectra-constraints` — `Some` exactly while a gesture is live — exposed as
  `active_edits()`, `is_dragging()`, `drag_node()`, `drag_updates()` and
  `SolverStats.active_edits`.
* **Real-time partners:** a sample suggests only the pointer's two slots; the
  solver returns every slot that moved, dragged node and constrained partners
  alike, and each one is written where an ordinary edit would land — so the
  incremental path, the `Dirty` set and the undo record all behave as usual.
* **Landed semantic note:** `read_back` compares solved values against the
  document's *current* value (not a load-time capture), which is what makes a
  repeated sample a true no-op — the property test asserts exactly that.

## 3. Protocol guards (`vectra-wasm`, `lib.rs` 1 594 lines)

`DragTxn { node_id, label, first_source, last_value, diagnostics, updates, parked }`
owns the gesture. `BeginDrag` validates without history; each sample refreshes
the txn's `last_value`; `EndDrag` commits via `Engine::record` as a single
`batch(disable parked…, SetParameter …)`, whose inverse is
`batch(enable parked…, SetParameter first_source…)` — so undo restores the rule
set *and* the geometry in one step. **While a gesture is live every other
command — `Undo`, `Redo` and `SetTime` included — is refused with
`VectraError::DragActive` before anything is applied**; `abort_drag` restores the
slots in reverse and releases the session.

## 4. Laws and test results

`crates/vectra-wasm/tests/drag_laws.rs` (801 lines, 8 tests) — the §6 law table,
plus the protocol contract:

| law | what it pins |
| --- | --- |
| `law_drag_partner_follows_the_pointer` | A+B `Vertical`, drag A → x=200 ⇒ **both exactly 200**, dirty = {A, B}, `full_evals` unchanged |
| `law_required_pins_resist_the_drag` | `Required` pins on both slots ⇒ the pointer moves nothing, **0 writes**, no error, no crash; `EndDrag` ⇒ registry empty, pin still in force |
| `law_weaker_pin_snaps_back_on_release` | same pins at `Medium` ⇒ follows while down (STRONG edit wins), returns on `EndDrag` |
| `law_end_drag_persists_literals_and_clears_the_registry` | a `$base`-driven slot ends as `x_source == "literal"` + `parametric-link-broken` diagnostic, graph edge gone, `active_edits == 0`, `$base` untouched |
| `law_one_drag_is_one_undo_step` | one undo restores the whole pre-gesture scene; a **second** undo reaches past the gesture to the rule (proof it was exactly one entry); redo replays the net movement without the solver |
| `law_drag_protocol_rejections_are_typed_and_atomic` | `UpdateDrag` with no gesture; `BeginDrag` on a `Path`; second `BeginDrag`; undo mid-gesture — typed, inert, session intact |
| `prop_drag_is_deterministic_and_idempotent` | the same gesture replayed on a fresh engine produces **byte-identical** snapshots, and a repeated sample (`suggest_value` with the coordinates already in force) writes nothing |
| `law_drag_commands_carry_exact_inverses` | `Command`-level contract: forward/backward are exact pre/post images |

Plus two session units in `solver.rs`:
`the_active_edits_registry_opens_and_closes_with_the_gesture` and
`a_pinned_slot_resists_the_pointer_and_a_weak_rule_does_not`.

**Four first-draft findings, all fixed on the test side (the engine was right):**

1. The partner law assumed B.x was 300 when the gesture began — but adding the
   `Vertical` rule had already solved B onto A's column (100). Corrected, and the
   law got *stronger*: undo must now restore the whole scene, and the second undo
   must remove the rule rather than drag geometry.
2. The rejection law matched a message substring the real text does not contain
   ("already in progress" vs "is in progress") — now asserted against the
   actual wording.
3. A click that moves nothing still leaves `can_undo` true (the `CreateNode` is
   undoable). Rewritten to undo once and require an **empty scene** — which is
   what actually proves the click added no entry.
4. **A pre-existing red suite, found and fixed:** `vectra-dependency`'s
   `prop_invariants_hold_under_random_edit_streams` was failing on a saved
   proptest regression (`ops = [(7, 0, 0)]`) whose assertion `h.steps > 0` was
   unsound — op kind 7 ("redefine an expression") on an *empty* document is a
   legitimate planner `Skip`, not a silent drop. The harness now counts skips,
   the property asserts the real invariant (`steps + skips == ops.len()`), and a
   deterministic `the_random_driver_never_silently_drops_an_op` law exercises
   every op kind plus that empty-document corner. It had been hiding behind a
   truncated `grep`/`head` in earlier gate runs; the gate command now counts
   failures explicitly instead of eyeballing the tail.

## 5. UI — RULE 3 (dumb remote)

`App.tsx` 998 lines, `wire.ts` 332, `commands.ts` 298, `view-model.ts` 214,
`tests/view-model.test.ts` 332 / 16 tests, `scripts/smoke.mjs` 583 / 21 steps.

* **Wire:** the three commands, `DragStarted`/`DragEnded`, and
  `SnapshotNode.position { x, y, x_source, y_source }` (`null` ⇒ no grab),
  `solver.active_edits` / `solver.drag_node`.
* **Builders:** `beginDrag(id)` / `updateDrag(id, x, y)` / `endDrag(id)` — three
  verbs, no arithmetic, no parameter resolution, no constraint knowledge.
* **Canvas:** `onPointerDown` on a shape sends `BeginDrag` and remembers the grab
  offset (`pointer − node.position` — a view offset, the only arithmetic in the
  component); the **window** then owns the pointer, sending one `UpdateDrag` per
  pointer event and re-reading the snapshot per sample, so the dragged node *and
  its partners* move together; release anywhere sends `EndDrag`. No throttling:
  the solver is ~1.2µs/sample and no measurement demanded it.
* **Visibility:** a `.dragging` highlight, a `drag-status` strip reading
  `dragStatus()` (e.g. `✋ dragging bound-rect — 2 edit variable(s) live`), and
  the engine's own `constraint-dropped` / `parametric-link-broken` diagnostics
  in the strip below the canvas. Mid-gesture button presses are refused by the
  engine and appear in the log as typed errors, which is the guard working.

## 6. Gate status

```
cargo fmt --check                                              clean
cargo clippy --workspace --all-targets -- -D warnings           clean
cargo test --workspace --no-fail-fast      30 suites · 181 passed · 0 failed
  ├─ vectra-constraints                     22/22  (incl. 2 drag-session units)
  ├─ vectra-wasm --test drag_laws            8/8
  ├─ vectra-wasm --test constraint_laws      7/7   (3.1 still green)
  └─ vectra-dependency                      9/9 + incremental 8/8
bash apps/vectra-web/scripts/build-wasm.sh                     rebuilt
node apps/vectra-web/scripts/smoke.mjs                       21/21
npm run typecheck / test:ui / build              clean / 16/16 / built
```

Live preview: dev server on **:5175** (Vite, HMR) serving the drag-capable UI.

---

### Where to look next

* `TASK-3.2-DESIGN.md` §9 — the four deliberate deviations (preview events,
  seeding solve, mid-gesture refusals, no throttling).
* `crates/vectra-constraints/src/solver.rs` §`begin_drag`/`update_drag`/`end_drag`
  — the edit-variable lifecycle end to end.
* `crates/vectra-wasm/tests/drag_laws.rs` — the law table, executable.
