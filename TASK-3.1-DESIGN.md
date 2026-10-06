# TASK 3.1 — `vectra-constraints`: Linear Constraint Solver

**Status:** _design drafted, awaiting authorization to build_ (Actions 156–160).
The Task 2.2 exit gate was re-run honestly before any 3.1 work (see §12): the
accepted 7-law file at `crates/vectra-dependency/tests/incremental_laws.rs` had
been destroyed by a bad write and has been rewritten against the real harness
API, all 8 tests in it pass, `cargo test --workspace` is **143 passed / 0
failed**, clippy 0 warnings, smoke 13/13, `test:ui` 8/8, `npm run build` clean.

**Doctrine:** MES §9 (`ConstraintSolver` sketch), §4 (`Document.constraints`),
§8 (dependency graph), §14–§16 (commands / engine / WASM). The engine-first rule
holds: the solver is engine machinery, the React app stays a dumb remote.

---

## 1. Acceptance criteria (from the authorization)

1. **Data model** — a `Constraint` registry on `Document`: `kind`, `targets`
   (`(node, "cx")` pairs), `strength`, `enabled`, id.
2. **Solver** — built on the `cassowary` crate (0.3.0, already pinned). A
   `ConstraintTarget → cassowary::Variable` mapping keeps one Cassowary
   variable per addressed slot. Edits/hints via **edit variables**.
   Output: the set of slots it moved (a `DirtySet` of `NodeId`s at the engine
   level).
3. **Commands** — `AddConstraint` / `RemoveConstraint`, undoable like any other
   command; on over-constraint the weakest constraint is dropped and a
   `Diagnostic::ConstraintDropped` is emitted.
4. **Laws** — Satisfaction (Vertical ⇒ `B.x == A.x` exactly), Overconstrained
   (100 vs 200 ⇒ weaker dropped + diagnostic), Undo (geometry reverts **and**
   Cassowary's internal variable count decreases).
5. **UI** — a “Constraints” section, two-node selection → “Add Vertical
   Constraint”, active constraints in the Inspector, drops in the event log.

**Boundary:** constraints are hard mathematical rules enforced by the engine,
not guidelines. The solver runs deterministically and integrates with the
dependency graph.

---

## 2. Data model — **landed** in `vectra-core` (`src/constraint.rs`)

```rust
pub type ConstraintId = uuid::Uuid;              // ids.rs, new_constraint_id()

pub enum Strength { Required, Strong, Medium, Weak }        // = Cassowary's four
pub enum ConstraintKind {
    Coincident, Horizontal, Vertical, Parallel,
    Perpendicular, EqualLength, Distance, Angle,
}
pub struct ConstraintTarget { pub node_id: NodeId, pub property: String }
pub struct Constraint {
    pub id: ConstraintId,
    pub kind: ConstraintKind,
    pub targets: Vec<ConstraintTarget>,   // [a, b]; [a] for Angle
    pub strength: Strength,               // default Medium
    pub value: Option<f64>,               // Distance / Angle / Parallel operand
    pub enabled: bool,                    // false = parked, not solved
}
#[serde(transparent)]
pub struct ConstraintRegistry { pub constraints: Vec<Constraint> }   // insertion order
```

`Document.constraints` is now a `ConstraintRegistry` (the old
`ConstraintRecord { kind: String, description: String }` placeholder is gone);
`Document` put/serialize order is unchanged, so the document wire format stays
forward-compatible. Helpers: `insert` (rejects a duplicate id — duplicate ids
would make undo non-invertible), `remove`, `remove_targeting(node_id)` (used by
`DeleteNode` so a deleted node can never leave a dangling rule), `active()`,
`get()`, `iter()`.

`Constraint::description()` renders `"vertical <a> ↔ <b>"` for the Inspector;
the solver never reads it.

**Property vocabulary is the existing one** (`Node::get_param` / `set_param`,
the same strings the dependency graph keys edges by):

| node kind | float slots |
| --- | --- |
| Rectangle | `x y width height corner_radius` |
| Circle / Ellipse | `cx cy radius` (`x y` aliases exist; the canonical strings are `cx cy radius`) |
| Arc | `cx cy radius start_angle end_angle` |
| Path / Group | none in Phase 1 |

Targets that name a non-float slot (or an unknown property) are rejected at
command-apply time with the existing typed errors, not silently ignored.

---

## 3. Cassowary mapping (MES §9)

> **As landed.** The pool is `VariablePool` keyed by the target *label*
> (`node.property`) in a `BTreeMap` — deterministic iteration, not `HashMap`
> order. One `cassowary::Variable` per distinct slot, interned on first use and
> reused within a solve; `gc(keep)` releases every variable no live rule
> references, which is the observable behind the Undo law (`solver.variables`
> falls back to 0). A **fresh `Solver` is built per pass**; nothing is carried
> between dispatches. `fetch_changes()` is *not* used: after `suggest_value` on
> the edit variables the solver reads `get_value(v)` for each pooled variable and
> writes only the slots whose value actually changed.

| concept | Cassowary 0.3.0 |
| --- | --- |
| a slot `(node, "width")` | one `cassowary::Variable` in a `HashMap<ConstraintTarget, Variable>` pool |
| a constraint row | `Expression { terms: [Term{v, 1.0}…], constant: -value }` + `RelationalOperator::{EQ,LE,GE}` |
| strength | `strength::{REQUIRED,STRONG,MEDIUM,WEAK}` — a 1:1 mapping of `Strength` |
| “the property the user is dragging” | `add_edit_variable(v, strength::STRONG)` + `suggest_value(v, new)` then `fetch_changes()` |
| solved values | `get_value(v)` for every pooled variable after the solve |

Pool discipline:
* create on first use, **reuse for the life of the solve**;
* **drop** a variable when no active constraint references its slot — this is
  what makes the Undo law's “Cassowary internal variable count decreases”
  observable (`SolverStats.variables` is exported);
* ids are stable across solves so a variable never leaks or aliases.

~~`fetch_changes()` reports only variables that *changed* …~~ **Superseded:** the
solver compares `get_value()` against the value it read back from the document
and keeps only real changes (`SolveOutcome::is_noop()`), which is the same
guarantee without depending on the crate's change list.

**Exactness.** The system is small and the arithmetic is binary-exact for the
in-law cases (`B.x := A.x`), so the Satisfaction law asserts `==` on `f64` as
the brief requires. The residual re-check after applying uses a `1e-9`
tolerance and exists only to catch float noise.

**Halves / integer scaling.** No Phase-1 kind needs a `½` coefficient
(`Coincident`, `Vertical`, `Horizontal`, `EqualLength`, `Distance`, `Angle` are
all unit-coefficient; `Parallel` / `Perpendicular` are ±1). A `slot × k` helper
is reserved for Task 3.2 direct manipulation (centre-preserving resize), where
`k = 2` keeps every coefficient an exact binary float.

---

## 4. Kind semantics (the operator each kind compiles to)

> **As landed (`rows.rs`).** Every kind compiles to one canonical `Row
> { terms, constant }`: duplicate labels are merged, zero coefficients dropped,
> terms sorted by label, and the whole row negated with its constant when the
> leading coefficient is negative — so `Distance(b, a) = 200` and
> `Distance(a, b) = −200` are the *same form* (this is what lets the pre-pass
> recognise a contradiction).
>
> | kind | row (as implemented) |
> | --- | --- |
> | `Coincident` / `Horizontal` / `Vertical` / `EqualLength` | `a − b == value.unwrap_or(0)` |
> | `Distance` / `Parallel` | `a − b == value` (value required in the row; the engine captures it when the wire omits it) |
> | `Perpendicular` (4 targets) | `(a − b) − (c − d) == value.unwrap_or(0)` |
> | `Angle` | `a == value` |
>
> `value` is spelled `Constraint::value` and is `Option<f64>` at the wire; the
> solver never sees `None` (capture happens in `VectraEngine::capture_operands`
> before dispatch). The D1 policy below is implemented over these canonical
> forms, so two rules on the same slot *at the same constant* are recognised as
> a **redundancy** (kept, no drop) rather than a contradiction.

`a`, `b` are targets; `v` is `Constraint::value`.

| kind | row | note |
| --- | --- | --- |
| `Coincident` | `a - b == 0` | component-wise; centre coincidence = two constraints (`cx,cx` + `cy,cy`) |
| `Horizontal` | `a - b == 0` | on the *y-like* slot the UI picked (`y` / `cy`) |
| `Vertical` | `a - b == 0` | on the *x-like* slot (`x` / `cx`) |
| `Parallel` | `a - b == v` | axis-aligned edge offset preserved; `v` defaults to the separation captured at add time |
| `Perpendicular` | `(a₁ - a₂) - (b₁ - b₂) == 0` | slope‑1 diagonals stay orthogonal (4 targets: both endpoints of both edges) |
| `EqualLength` | `a - b == 0` | on magnitude slots (`width` / `height` / `radius`) |
| `Distance` | `a - b == v` | **signed** separation (see OPEN-ITEMS #4) |
| `Angle` | `a == v` | pins one slot (radians as stored) |

The *rows* above are what MES §9 sketches; the mapping from a menu action
(“vertical”) to a slot is a wire-vocabulary decision owned by `commands.ts`,
not geometry logic (it picks which existing slot the rule addresses — the same
kind of knowledge `createBoundRectangle` already carries).

---

## 5. Where the solver runs — the exact integration point

> **As landed.** The pipeline below is what `VectraEngine` does today, with one
> addition and one refinement: a **preflight** runs before `core.dispatch`
> (a `Required`-vs-`Required` contradiction is refused with
> `VectraError::UnsatisfiableConstraints` — nothing applied, nothing pushed), and
> the pass emits `ConstraintsUpdated` events for rules that left the active set.
> Solver writes are `apply_untracked(SetParameter {…})`; when the tableau refuses
> the system anyway (defensive branch) the command is rewound with
> `rollback_last()`, so the document, the history and the pool stay consistent.
> `ConstraintError::UnsatisfiableConstraint` / `DuplicateConstraint` from the
> crate itself remain defensive-only: the pre-pass and the fresh solver make them
> unreachable in practice — they are not test fixtures.
>
> **Determinism.** Registry order is the `BTreeMap`'s; the drop tie-break is
> `(strength, registry index)`. Same input ⇒ byte-identical output (prop law).

The Task 2.2 precedent puts graph + expressions + scene cache in the
composition root (`vectra-wasm::VectraEngine`), and keeps `vectra-core::Engine`
pure. The solver follows that precedent (it must: `vectra-constraints` depends
on `vectra-core`, so core cannot own it).

```
VectraEngine::dispatch_command(json)
  ├─ Command::from_json
  ├─ ExpressionEngine::check_source          (DefineExpression only)
  ├─ gate_command(graph, cmd, doc)           (Task 2.2 cycle gate, dry-run)
  ├─ core.dispatch(cmd)        ── document mutated, inverse pushed on the stack
  ├─ ★ SOLVER PASS ★           ── NEW (Task 3.1)
  │    1. collect active constraints; if none → skip (zero cost when unused)
  │    2. solve(doc, hints=slots this command wrote)      ← cassowary
  │    3. apply changed slot values via Node::set_param
  │    4. amend the history entry: amend_top_backward(Batch[SetParameter…])
  │    5. dropped constraints → Diagnostic::ConstraintDropped
  ├─ expressions.sync_from_document
  ├─ graph.sync
  ├─ graph.dirty_ids_for_events
  ├─ scene.refresh(dirty)
  └─ EngineEvent::{… , ConstraintsUpdated, Dirty{ids, mode}}
```

So: **the solver runs in `VectraEngine::dispatch_command`, after
`core.dispatch` returns `Ok` and before `expressions.sync_from_document` / the
topology re-derivation.** Solver writes therefore flow through the *same*
topology + dirty + scene path as a hand-issued `SetParameter` — constraints
cannot bypass incrementality. `undo` / `redo` run the identical pipeline, so a
rewind is re-solved as well.

**Hints.** The slots the dispatched command wrote (and, later, the drag
targets of Task 3.2) become edit variables at `STRONG`, suggested to their new
values: the solver then minimizes movement *away* from what the user just did,
which is exactly Cassowary's edit-variable use and needs no mutable global
state. Zero constraints active ⇒ the pass is a single `is_empty()` check.

**Determinism.** Constraint order = registry order; pool order = sorted
`ConstraintTarget` (`label()`), never a `HashMap` iteration order; ties in the
drop rule break on `(strength, registry index)`, lowest index losing. Same
input ⇒ same output, byte for byte.

---

## 6. Commands, undo/redo, and the variable-count law

> **As landed.** The command set grew by one:
> `Command::{AddConstraint, RemoveConstraint, SetConstraintEnabled, Batch,
> batch_commands}` — `SetConstraintEnabled` exists because a dropped loser is
> **parked, not forgotten**: it stays in the registry with `enabled: false`, so
> the UI can show it as dropped, the user can re-enable it, and an undo of the
> winning rule can revive the loser. `CommandStack::{amend_top_backward,
> rollback_last}` + `Engine::{apply_untracked, amend_top_backward,
> rollback_last}` are the three core hooks the pass needs.
> `DeleteNode` withdraws the rules that target the deleted node (inverse
> restores both node and rules). One user action = one history entry: the
> solver's writes are folded into the entry that caused them, so undo of
> `AddConstraint` restores geometry *and* drops the rule in a single step.

```rust
Command::AddConstraint   { constraint: Constraint }   // apply: registry.insert
Command::RemoveConstraint{ id: ConstraintId }         // apply: registry.remove → inverse = AddConstraint
```

* Both are ordinary commands: `apply` returns the exact inverse, so they push
  one history entry each and `preview_events()` names `ConstraintsUpdated`.
* **The solver's writes belong to the action that caused them.** The pass
  records each write as `SetParameter` (which has an exact inverse) and folds
  them into the *current* entry's backward command:
  `Command::Batch(Vec<Command>)` (apply = children in order; inverse =
  reversed inverses) + `CommandStack::amend_top_backward(Command)`.
  ⇒ undo of `AddConstraint` restores the geometry **and** drops the rule in one
  step, exactly as the brief's Undo law demands.
* Undo removes the constraint ⇒ the pool GCs the slots it referenced
  ⇒ `SolverStats.variables` decreases ⇒ the law is observable, not implied.

Only two core-level additions are needed: `Command::Batch` and
`amend_top_backward`. Nothing else in the command layer changes.

---

## 7. Diagnostics & the over-constrained policy

> **As landed.** Three codes, all rendered through the existing
> `SnapshotResponse.diagnostics` (no new channel):
> `ConstraintDropped` (tag `constraint-dropped`, **Warning**),
> `ConstraintSkipped` (`constraint-skipped` — a rule that could not be read this
> pass, e.g. a slot that is not a float), and
> `ParametricLinkBroken` (`parametric-link-broken` — see D2).
> The drop policy is the pairwise pre-pass (`plan.rs`): group the active rules by
> canonical `Row::form()`, rank by `(Strength, registry index)`, and let the
> strongest win (`Required` beats all; a tie drops the newest; equal form *and*
> equal constant = redundancy, both kept). Unresolvable rules → `Skipped`;
> two `Required` rules that disagree → `Conflict` → typed error via preflight.
> The loser is **disabled, not deleted**, and the message names both rules:
> `"{kind} [{id8}] was dropped: {kept_kind} [{kept_id8}] at {kept_strength}
> strength already pins that row ({targets})"`.

`vectra-geometry/src/diagnostic.rs` gains
`DiagnosticCode::ConstraintDropped` (tag `"constraint-dropped"`, severity
**Warning** — geometry still evaluated), reusing the existing `Diagnostic`
fields: `node_id` = first target's node, `property` = its slot, message names
both rows + strengths. It flows through the existing
`SnapshotResponse.diagnostics` (canonically sorted) into the Inspector and the
event log — no new channel.

**Policy (pairwise, deterministic):**

| situation | outcome |
| --- | --- |
| same slot constrained to two different constants | the **weaker** is dropped; the survivor holds; `ConstraintDropped` emitted |
| weakest candidates tie | **newer** constraint dropped |
| `Required` vs weaker | weaker dropped (required never yields) |
| `Required` vs `Required`, unsatisfiable | the command **fails** with `VectraError::Command` (nothing applied) — an inconsistent hard system is a bug, not a warning |
| constraint dropped so completely that the system stays unsatisfiable | command fails, registry unchanged |

Cassowary's own `UnsatisfiableConstraint` fires only for `REQUIRED`; the
pairwise table above is therefore implemented by the solver as a *pre-pass*
(the solver tries the candidate along with the active set and consults
`strength` before committing) — deterministic, and it is what makes the
brief's `Distance=100 + Distance=200` case produce a *drop* rather than a
silent choose-one. See OPEN-ITEMS #1: this is my reading of the brief, and
Task-2.2-style doctrine locks it.

---

## 8. Dependency-graph integration (MES §8)

> **As landed: option (a) — no new edge kind in 3.1.** The solver writes through
> the ordinary `SetParameter` path, so the resulting `Dirty` event names the
> solved nodes alongside the slot the user edited, and `full_evals` never moves
> (smoke 14/15). The `constraint` coordinator edge stays deferred to Task 3.2,
> where a *drag* needs the graph to carry the coupling during direct
> manipulation. Constraint targets are validated against the graph's property
> vocabulary (`Node::get_param`), which is why an unaddressable slot is refused
> before dispatch instead of failing inside the solver.

Constraint enforcement happens **inside** `dispatch_command`, before
`graph.sync` — so every solve is followed by a topology re-derivation and a
`Dirty` event naming the adjusted nodes. Solved values land in the document's
slots; anything *depending* on those slots (variables, expressions, caches) is
invalidated by the existing dirty machinery. No second propagation engine.

**Recommended for Phase 3.1:** the solver's output contributes its node ids to
the `Dirty` set through the existing `dirty_ids_for_events` result (engine-level,
no new edge kind).
**Deferred (Task 3.2, recommended):** a real `constraint` coordinator edge
(`prop:a.x ⇄ prop:b.x`) so a *drag* of one node dirties the other through the
graph itself — the cleaner long-term shape, but it perturbs the frozen 2.2
graph wire format and should be its own locked decision. See OPEN-ITEMS #3.

---

## 9. Laws (MES §18 style; landed in `crates/vectra-wasm/tests/constraint_laws.rs`)

All of them run through the **real** pipeline (`VectraEngine` composition root),
never a private solver call — the same discipline as Task 2.2's laws. (The file
lives in `vectra-wasm` because that is where the composition root is; the crate's
own 20 unit tests are in `vectra-constraints`.)

**As landed: 7 tests — 6 laws + 1 property law**, all green:
`law_satisfaction_via_vertical_constraint`,
`law_overconstrained_drops_the_weakest`,
`law_undo_reverts_geometry_and_releases_variables`,
`law_parametric_link_is_broken_and_restorable`,
`law_deleting_a_node_withdraws_its_constraints`,
`law_required_contradiction_is_a_typed_error`,
`prop_solving_is_idempotent_and_deterministic` (idempotence + byte-identical
replay + a three-node vertical chain converging on the dragged slot).

**L1 — Satisfaction.** Two rectangles A, B (A.x=100, B.x=300). `AddConstraint
Vertical(A.x, B.x)` ⇒ after the pass `B.x == 100.0`. Assert `is_empty()` on the
dirty set next: programmatically set `A.x = 150` (`SetParameter`) ⇒ solve ⇒
`B.x == 150.0` **exactly** (`f64::to_bits` equality, the brief's “exactly”).
Also: no full re-evaluation (`eval.full_evals` unchanged), and the `Dirty`
event names B.

**L2 — Overconstrained.** A, B at distance 100; `Distance(A.x, B.x) = 100`
(Medium, enforced) then `Distance(A.x, B.x) = 200` (Weak) ⇒ the **Weak** row is
dropped, `B.x - A.x == 100.0` still holds, and the response carries exactly one
`Diagnostic::ConstraintDropped` naming the dropped id and strength. Second
assertion (the other order): 200-Weak first, then 100-Medium ⇒ the Weak row is
dropped and the distance is 100. Third: `Required 100` + `Medium 200` ⇒ Medium
dropped. Fourth: `Required 100` + `Required 200` ⇒ typed error, registry
unchanged (nothing half-applied).

**L3 — Undo.** A, B apart; `AddConstraint Vertical` (B snaps to A) — one undo
⇒ geometry back to the *unconstrained* values **and**
`stats.variables` decreased to 0; redo ⇒ both return. Plus the cross-check that
undoing the *drag* (`SetParameter A.x`) under a live constraint re-solves
deterministically back to the constrained state.

Extra laws (cheap, high value): **invariance** — solving twice changes nothing
(idempotence); **no-op** — a dispatch that violates nothing does not write any
slot (empty solver delta); **determinism** — the same document+command stream
yields byte-identical snapshots across runs; **wire** — `SnapshotResponse`
exposes the registry canonically sorted.

---

## 10. UI plan (dumb remote)

> **As landed.** Constraints panel sits between *Layers* and *Variables*: two
> pick-chips per layer row (`data-testid=layer-<id>`, third click starts a new
> pair), then **Add Vertical / Coincident / Hold distance / Over-constrain**;
> each rule renders with its strength chip, a `dropped` chip when parked, a
> pause/resume toggle and a remove button. The solver line
> (`solverSummary`) and the diagnostics log carry the evidence; `smoke.mjs`
> steps 14–18 drive the whole story over the real wasm boundary.

* `wire.ts` / `commands.ts` — `AddConstraint` / `RemoveConstraint` builders
  (`addVerticalConstraint(a, b)`, `addCoincidentConstraint(a, b)`, …) plus a
  kind→slot table (`Rectangle→x`, `Circle→cx`, …) documented as *wire
  vocabulary*, not geometry.
* `view-model.ts` — reads `snapshot.constraints[]` (id, kind, targets,
  description, strength, enabled) and `snapshot.diagnostics[]`; derives
  “selection can take a constraint” (exactly two nodes selected).
* `App.tsx` — a **Constraints** panel: “Add Vertical Constraint”, “Add
  Coincident”, “Remove”; active constraints listed in the Inspector with their
  strength; the existing diagnostics log renders `constraint-dropped` entries
  in the warning style. New test-ids: `constraints`, `add-vertical-constraint`,
  `remove-constraint-<id>`, `constraint-list`.
* `snapshot.rs` — `constraints: BTreeMap<ConstraintId, ConstraintView>`
  (canonical ordering is an invariant already locked in 2.2).
* Tests: `tests/view-model.test.ts` gains constraints cases; `smoke.mjs` gains
  steps 14–16 (add constraint ⇒ both x equal in the scene; over-constrain ⇒
  diagnostic in the snapshot; undo ⇒ values and `constraints` count revert).

---

## 11. Implementation order (each step gated)

1. ✅ `vectra-core`: `constraint.rs` + `ids::ConstraintId` + `Document.constraints`
   registry (landed; `cargo check` clean).
2. ✅ `vectra-constraints`: `ConstraintError` → typed; `VariablePool`;
   `rows_for(constraint) -> Vec<LinearRow>`; `solve(doc, hints) -> SolverOutcome
   {writes, dropped, stats}`; unit tests per kind (including the pairwise drop
   table).
3. ✅ `vectra-core::command`: `AddConstraint` / `RemoveConstraint` + `Batch` +
   `amend_top_backward`; core unit tests (inverse exactness, undo across the
   registry).
4. ✅ `vectra-wasm`: the solver pass in `dispatch_command`; `ConstraintsUpdated`
   event; `constraints` + `solver` blocks in the snapshot; `DiagnosticCode`;
   `delete_node` → `remove_targeting`.
5. ✅ Laws L1–L3 + extras (§9) — 7/7.
6. ✅ UI (§10) + smoke steps 14–18.
7. ✅ Full sweep: `cargo fmt --check`, `clippy -D warnings`, `cargo test
   --workspace`, `npm run build`, `test:ui`, `smoke`, plus a live check on the
   :5175 preview.
8. ✅ `TASK-3.1-REPORT.md` with the four requested items (delivered this turn).

---

## 12. Task 2.2 exit-gate re-verification (done before starting 3.1)

| check | result |
| --- | --- |
| `cargo test -p vectra-dependency --test incremental_laws` | **8 passed** (7 accepted laws + 1 property law) |
| `cargo test --workspace` | **143 passed / 0 failed** |
| `cargo clippy --workspace --all-targets` | 0 warnings |
| `cargo fmt --all` | clean |
| `node scripts/smoke.mjs` | **13/13**, `SMOKE PASS: … patch ≡ rebuild` |
| `npm run test:ui` / `typecheck` / `build` | 8/8 · clean · clean (JS 162.47 kB, wasm 5,261.10 kB) |
| release perf (re-measured) | SetVariable e2e 500 nodes **34.06 µs** (budget 2 ms); propagation **1.28 µs @8 vs 1.18 µs @4000 → ratio 0.92**; patch 1 node **174.97 µs** vs full rebuild 4000 nodes **709.83 µs** |
| live preview | dev server alive on **:5175** (pid 34341) |

If §12 had failed, Task 3.1 would not have started.
