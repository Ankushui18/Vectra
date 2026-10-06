# TASK 3.1 — `vectra-constraints`: report-back

**Status: complete.** Constraints are engine-enforced linear rules: they are
solved by the `cassowary` crate, they run inside the same dispatch pipeline as
every other mutation, they participate in undo/redo, and the React remote can
trigger them and show the result (including the over-constrained drop).

| gate | result |
| --- | --- |
| `cargo fmt --all -- --check` | clean |
| `cargo check --workspace --all-targets` | clean |
| `cargo clippy --workspace --all-targets -- -D warnings` | **0** warnings |
| `cargo test --workspace` | **170 passed / 0 failed** (20 of them new: `vectra-constraints`) |
| `cargo test -p vectra-wasm --test constraint_laws` | **7 / 7** |
| `node apps/vectra-web/scripts/smoke.mjs` | **18 / 18** |
| `npm run typecheck` · `test:ui` · `build` | clean · **13 / 13** · ok |
| live preview | dev server up on **:5175**, serving the constraint UI |

> One defect was found **during** this final verification and fixed before the
> numbers above were taken: `rows::tests::rows_are_unit_coefficient_equalties`
> asserted a positional term list, but `Row::new` sorts terms by label and a
> label starts with the node's random uuid — so the assertion flipped ~50 % of
> runs. It now compares against the canonical row. 20 consecutive runs of
> `cargo test -p vectra-constraints` and 5 consecutive runs of
> `cargo test --workspace` are green. Details in §3.4.

---

## 1. Report-back item 1 — final `Constraint` data model + Cassowary mapping

### 1.1 The model (`crates/vectra-core/src/constraint.rs`, landed)

```rust
pub enum Strength { Required, Strong, Medium, Weak }   // serde: snake_case

pub enum ConstraintKind {                              // serde: snake_case
    Coincident, Horizontal, Vertical, Parallel,
    Perpendicular, EqualLength, Distance, Angle,
}
impl ConstraintKind {
    pub fn arity(&self) -> usize;        // 2, 2, 2, 2, 4, 2, 2, 1
    pub fn takes_value(&self) -> bool;   // Distance | Angle | Parallel
}

pub struct ConstraintTarget { pub node_id: NodeId, pub property: String }
// `label()` = "<uuid>.<property>" — the solver's variable-map key

pub struct Constraint {
    pub id: ConstraintId,
    pub kind: ConstraintKind,
    pub targets: Vec<ConstraintTarget>,   // [a,b] / [a1,a2,b1,b2] / [a]
    #[serde(default = "default_strength")] pub strength: Strength,  // Medium
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<f64>,               // operand; None ⇒ engine captures
    #[serde(default = "default_enabled")]  pub enabled: bool,       // true
}

#[serde(transparent)]
pub struct ConstraintRegistry { … }       // insertion-ordered, id-keyed
// insert (duplicate id ⇒ typed error), get/get_mut, contains, remove,
// active() (enabled only), remove_targeting(node_id)

// Document gains: pub constraints: ConstraintRegistry
```

**Deliberate properties.** `value: Option<f64>` is what makes “hold the
separation that exists right now” expressible on the wire without the UI
computing geometry: the engine resolves it once, at dispatch time, so the
document always stores a fully-specified rule the inspector can print and the
solver can replay deterministically. `enabled: false` is a *parked* rule — the
state an over-constrained loser lands in (§5). Targets are plain
`(node, property)` pairs in exactly the vocabulary `Node::get_param` and the
dependency graph already use, so a constraint and a property vertex address the
same slot.

### 1.2 Slot → linear row (`crates/vectra-constraints/src/rows.rs`)

| kind | row as implemented |
| --- | --- |
| `Coincident` / `Horizontal` / `Vertical` / `EqualLength` | `a − b == value.unwrap_or(0)` |
| `Distance` / `Parallel` | `a − b == value` (value required; engine captures when omitted) |
| `Perpendicular` (4 targets) | `(a − b) − (c − d) == value.unwrap_or(0)` |
| `Angle` | `a == value` |

Every row is canonicalized by `Row::new`: duplicate labels summed, zero
coefficients dropped, terms **sorted by label**, and the whole row (coefficients
*and* constant) negated when the leading coefficient is negative. That is why
`Distance(a,b)=200` and `Distance(b,a)=−200` are literally the same row object —
and why the over-constrained pre-pass can recognise a contradiction (same
`form()`, different constant) instead of only noticing it in the tableau.

### 1.3 `ConstraintTarget` → `cassowary::Variable`

| concept | Cassowary 0.3.0 |
| --- | --- |
| a slot `(node, "x")` | one `cassowary::Variable`, interned in `VariablePool` keyed by `label()` (`BTreeMap` — deterministic iteration order) |
| a `Row {terms, constant}` | `Expression::new(terms, −row.constant)` + `cassowary::Constraint::new(expr, Equal, strength)`, one per rule |
| `Strength` | `strength::{REQUIRED, STRONG, MEDIUM, WEAK}` — 1:1 |
| “the slot the user just moved” | `add_edit_variable(v, STRONG | WEAK)` + `suggest_value(current)` — this is the only “hint” the solver gets |
| solved values | `get_value(v)` per pooled variable; a slot is written **only if the value changed** |
| variable lifetime | `pool.gc(keep)` after every pass releases variables no live rule references — this is what makes the Undo law's “Cassowary internal variable count decreases” *observable* |

`SolverStats { variables, constraints, edit_variables, writes, dropped, skipped }`
is exported through the snapshot, so `solver.variables` is a first-class number
the UI and the laws can assert on. A fresh `cassowary::Solver` is built per pass
(nothing is carried between dispatches), which also makes each dispatch's result
independent of any earlier tableau state.

---

## 2. Report-back item 2 — the exact integration point in the dispatch loop

`crates/vectra-wasm/src/lib.rs` — the composition root, i.e. the same place
Task 2.2 put expression compilation, the cycle gate and the scene cache.

```
VectraEngine::dispatch_command(cmd_json)                       // lib.rs:143
  ├─ Command::from_json                                    :144  (typed parse)
  ├─ ExpressionEngine::check_source                        :149  (DefineExpression)
  ├─ ConstraintSolver::preflight(doc, &command)            :156  ★ NEW
  │     a Required-vs-Required contradiction is refused here: nothing applied,
  │     nothing pushed, error "unsatisfiable constraints: …"
  ├─ capture_operands(&mut command)                        :161  ★ NEW
  │     Distance/Parallel/Angle without a value: resolve the value that holds now
  ├─ gate_command(&graph, &command, doc)                   :164  (2.2 cycle gate)
  ├─ core.dispatch(command.clone())                        :167  ← document mutated,
  │                                                                inverse pushed
  ├─ ★ THE SOLVER PASS ★  self.constraint_pass(&command, true)  :168 / def :364
  │     0. bail if the registry is empty AND the pool is empty (two integer compares)
  │     1. hints = slots this command wrote              (fn hints_for :505)
  │     2. solver.solve(doc, ctx, &hints)                (edit vars STRONG for hints,
  │        WEAK otherwise; plan → tableau → get_value → write changed slots only)
  │     3. losers  → Diagnostic::ConstraintDropped + apply_untracked(
  │                  SetConstraintEnabled{enabled:false}) + ConstraintsUpdated
  │        skipped → Diagnostic::ConstraintSkipped (unreadable slot; not fatal)
  │        writes  → apply_untracked(SetParameter{…}) + NodesUpdated
  │                  (+ ParametricLinkBroken when the slot was driven by a variable)
  │     4. amend && inverses non-empty ⇒ amend_top_backward(Batch(inverses))
  │        — the write belongs to the command that caused it (one undo = one action)
  │     Er   → rollback_last(): a tableau refusal rewinds the command entirely
  └─ self.settle(events)                                   :477
        expressions.sync_from_document → graph.sync → graph.dirty_ids_for_events
        → scene.refresh(dirty) → EngineEvent::Dirty{ids, mode}
```

So, precisely: **the solver runs in `dispatch_command`, after `core.dispatch`
returns `Ok` and before `sync_from_document` / the topology re-derivation.**
`undo()` (:233) and `redo()` (:261) call the identical pass with
`amend = false / true`, so both a rewind and a replay re-enforce whatever rules
the restored document still has.

Why that placement:

* solver writes travel the *ordinary* `SetParameter` path, so they dirty their
  nodes, re-derive the graph and patch the scene cache exactly like a hand edit —
  a constraint cannot bypass incrementality (`eval.full_evals` never moves);
* the document stays the single source of truth: the solver never mutates
  `Document.variables`, it rewrites the slot and reports
  `ParametricLinkBroken` if that slot was variable-driven;
* hints are *derived from the command*, not from UI state: “make B vertical to A”
  anchors on A because A is the slot the command itself names — no global drag
  state, nothing for Task 3.2 to retrofit.

---

## 3. Report-back item 3 — property-test results

All of these run through the real composition root (`VectraEngine::dispatch_command`
/ `undo` / `redo` + `get_snapshot`) — never a private solver call.
File: `crates/vectra-wasm/tests/constraint_laws.rs` (7 tests, green).

### 3.1 Satisfaction — `law_satisfaction_via_vertical_constraint`

| step | assertion | observed |
| --- | --- | --- |
| two rects A.x=100, B.x=300 | — | — |
| `AddConstraint Vertical(A.x, B.x)` | `B.x == 100.0` **exactly** (f64 `==`), `A.x` untouched | ✅ `solver.variables == 2`, `constraints == 1`, `writes == 1` |
| programmatically move `A.x = 150` (`SetParameter`) | `B.x == 150.0` exactly | ✅ dirty set = **{A, B}** exactly, ≥2 `NodesUpdated` |
| incrementality | no fallback to a full rebuild | ✅ `eval.full_evals` unchanged, `last_evaluated == 2` |

### 3.2 Overconstrained — `law_overconstrained_drops_the_weakest`

| scenario | expected | observed |
| --- | --- | --- |
| `Distance=100` (Medium) then `Distance=200` (Weak) | weaker dropped, survivor exact, **one** `ConstraintDropped` naming the loser's id | ✅ survivor `a − b == 100.0`; loser `enabled:false`; `solver.dropped == 1`, `constraints == 1`; diagnostic severity `warning`, message contains the loser's 8-char id |
| `Required 100` then `Medium 200` | required never yields ⇒ Medium dropped | ✅ `b − a == −100.0` untouched |
| `Required 100` then `Required 100` (same row, same offset) | redundancy, **not** a drop | ✅ both accepted |
| `Required 100` then `Required 777` | hard error: nothing applied | ✅ `status:"error"`, `dropped == 0`, loser never registered, geometry untouched |
| a rule that already holds | must move nothing | ✅ `writes == 0` |

### 3.3 Undo — `law_undo_reverts_geometry_and_releases_variables`

| step | assertion | observed |
| --- | --- | --- |
| add Vertical, B snaps 300 → 100 | pool holds both slots | `solver.variables == 2` |
| **one** undo | rule gone **and** geometry back to the unconstrained 100/300 | ✅ (`constraints == 0`, `variables == **0**` — the Cassowary variable count decreased) |
| redo | rule + geometry return exactly | ✅ `variables == 2`, `B.x == 100.0` |
| undoing the *drag* under a live rule | re-solve deterministically back to the constrained state | ✅ smoke step 16: undo reverts drag+solve together, then rule+link |

### 3.4 The rest of the suite

| test | what it pins |
| --- | --- |
| `law_parametric_link_is_broken_and_restorable` | solver never writes `Document.variables`: `$base` stays 300, `ParametricLinkBroken` emitted, edge count 1 → 0; undo re-links and the diagnostic is not re-emitted |
| `law_deleting_a_node_withdraws_its_constraints` | no dangling rules; undo restores node **and** rule, enforced again |
| `prop_solving_is_idempotent_and_deterministic` | fixed ids; re-solving a satisfied system changes nothing (state-equal snapshot modulo counters/undo flags), two engines fed the same stream produce **byte-identical** snapshots, and a 3-node vertical chain converges on the dragged slot (−5, −5, −5) |
| `law_required_contradiction_is_a_typed_error` | boundary message starts `unsatisfiable constraints:`, `VectraError::is_constraint_conflict()`, no events |
| `vectra-constraints` unit tests (20) | row canonicalization + mirrored-form identity, arity/value validation, unknown/non-float slots, pool intern/gc, the whole drop pre-pass table (weaker loses, tie drops newer, required beats weaker, required-vs-required is a `Conflict`, redundancy kept), tableau no-op, idempotence, broken-link reporting |

**Defect found and fixed during this verification.**
`rows::tests::rows_are_unit_coefficient_equalties` asserted
`rows[0].terms == vec![(a, 1.0), (b, -1.0)]` and `constant == 100.0`. `Row::new`
sorts terms by `target.label()`, and a label begins with the node's **random
uuid**, so which term leads — and therefore the sign of the canonicalized
constant — varies per run. The assertion was ~50 % flaky (it passed the three
runs taken earlier today and failed one run in the final sweep). It now compares
against the canonical `Row::new(…)` and asserts the *unit coefficients with
opposite signs* and `constant.abs()`, i.e. the semantics that are actually
invariant. Evidence: 20 consecutive `cargo test -p vectra-constraints` runs and
5 consecutive `cargo test --workspace` runs, all green.
No product-code defect: `plan.rs` groups by `form()` (order-canonical) and the
tableau is indifferent to term order.

---

## 4. Report-back item 4 — clean checks + the UI can trigger and display a constraint

### 4.1 Checks

```
cargo fmt --all -- --check            → clean
cargo check --workspace --all-targets → clean
cargo clippy --workspace --all-targets -- -D warnings → 0 warnings, 0 errors
cargo test --workspace                → 170 passed / 0 failed:
    vectra-constraints 20 · vectra-core 23 (+10 param_resolution, +6 undo_redo)
    vectra-dependency 13 (+8 dependency_laws, +8 incremental_laws)
    vectra-expression 27 (+8) · vectra-geometry 22 (+10)
    vectra-wasm 8 (+7 constraint_laws)
node apps/vectra-web/scripts/smoke.mjs → SMOKE PASS, 18/18 steps
npm run typecheck (app + tests)        → clean
npm run test:ui                        → 13 passed / 0 failed
npm run build                          → ok (js 168.04 kB → gzip 54.04 kB, wasm 6.40 MB)
```

Smoke steps 14–18 are the constraint story end-to-end over the real wasm
boundary:

* **14** — `Vertical` snap: `B.x ← A.x`, `solver.variables == 2`,
  `writes == 1`, `$base` untouched, `parametric-link-broken` in the
  diagnostics, the `B.x ← $base` edge gone, `ConstraintsUpdated.ids == [rule]`.
* **15** — drag A: dirty = both nodes, B follows *exactly*, `full_evals` stays 1.
* **16** — undo reverts drag+solve together, then rule+geometry (re-linking
  `$base`, `solver.variables → 0`); redo replays both.
* **17** — 100 vs 200 at Medium vs Weak: `constraint-dropped` names the loser's
  id, loser `enabled: false`, survivor exact.
* **18** — required/required contradiction: typed error, zero mutation.

### 4.2 The remote control (dumb by design)

Trigger — all of it is commands in, events/snapshot out:

1. click two layer rows (pick-chips 1 / 2 appear; a third click starts a new pair)
2. **Add Vertical** ⇒ `AddConstraint` on the two x-like slots
   (**Add Coincident** emits the `cx`/`cy` pair, **Hold distance** captures the
   current signed separation, **Over-constrain** re-asserts it at +100 / Weak)

Display:

* **Constraints panel** (`constraint-list`) lists every rule with kind, the
  engine-rendered description, a strength chip, a `dropped` chip when the solver
  parked it, a pause/resume toggle and remove; the panel header carries the
  solver line (`n var · n rules · n moved [· n dropped]`).
* **Layers** — the solved node's x visibly snaps to the anchor's; the last
  `Dirty` event's ids highlight the re-evaluated rows (`layer-selected`, and the
  log line names them).
* **Event log** — `⊞ constraints: <ids>` per `ConstraintsUpdated`; the
  `constraint-dropped` / `parametric-link-broken` diagnostics render in the
  warning/error style; rejected commands (required contradiction, malformed
  constraint, invalid expression) appear as red typed-error lines.
* **Canvas** — the SVG is a pure projection of the snapshot, so it moves when
  the engine says it moved; `eval` chip proves the patch path (`full ×1`).

Test-ids for automation: `constraint-list`, `constraint-<id>`,
`remove-constraint-<id>`, `add-vertical-constraint`,
`add-coincident-constraint`, `add-distance-constraint`, `over-constrain`,
`constraint-hint`, `layer-<id>`, `engine-status`, `undo`, `redo`, `event-log`.

`tests/view-model.test.ts` (13 cases) covers the pure projections behind this:
`slotFor` (rect → `x`/`y`, circle/arc → `cx`/`cy`, path/group → `null` — the UI
refuses a command the engine could not resolve), `constraintRows` (kind-sorted,
engine description, dropped flag, fallback description), `solverSummary`
(singular/plural, dropped only when non-zero) and `formatEvent` for
`ConstraintsUpdated`.

Live preview: the dev server on **:5175** is serving the new UI (verified by
fetching `/src/App.tsx` through Vite: the constraint panel and all test-ids are
present).

---

## 5. Decisions as landed (the OPEN-ITEMS answers)

| # | decision | as landed |
| --- | --- | --- |
| **D1** | over-constrained policy | **pairwise pre-pass**, not global minimum-violation: group by canonical `Row::form()`, rank by `(Strength, registry index)`; strongest wins, tie drops the newer; equal form **and** constant = redundancy (both kept); Required-vs-Required disagreement = `Conflict` → typed error via preflight, nothing applied. Only equalities over the same slot form are compared in 3.1 |
| **D2** | write a variable-driven slot? | yes, but by **breaking the link**: the solver writes the slot as a literal and emits `ParametricLinkBroken`; `Document.variables` is never written; undo restores geometry *and* the link |
| **D3** | constraint graph edges in 3.1? | **no new edge kind.** Writes take the ordinary `SetParameter` path, so dirty ids carry the solved nodes; the `constraint` coordinator edge stays deferred to Task 3.2 (drag coupling) |
| **D4** | `Distance` signedness | **signed** separation `a − b == value`; the builder captures the current signed value when omitted |
| **D5** | `Parallel` / `Perpendicular` | Phase-1 slot form: `Parallel` = preserved axis offset `a − b == value`; `Perpendicular` = 4 targets, `(a−b) − (c−d) == value`. True vector forms arrive with the non-linear pass (Phase 2+) |
| **D6** | undo granularity | **one user action = one history entry** (writes folded via `amend_top_backward(Batch)`) |
| defaults | — | `Coincident` = 2 targets (centre coincidence = two rules, the button emits both); `Angle` pins one slot; duplicate id ⇒ typed error; `DeleteNode` withdraws targeting rules; snapshot carries `constraints` + `solver` |

Deltas from the original design sketch, now reflected in `TASK-3.1-DESIGN.md`
(§3–§10 carry an “As landed” note):

1. a dropped loser is **parked** (`enabled: false`), not deleted — the registry
   keeps it so the UI can show it, the user can re-enable it and undo can revive
   it; that is why `SetConstraintEnabled` exists as a command;
2. `fetch_changes()` is not used — the solver compares `get_value()` against the
   value it read from the document (`SolveOutcome::is_noop()`), same guarantee,
   no dependency on the crate's change list;
3. three diagnostics, not one: `ConstraintDropped`, `ConstraintSkipped`,
   `ParametricLinkBroken`;
4. the laws live in `crates/vectra-wasm/tests/constraint_laws.rs` (the crate
   root is where the composition root is), with `vectra-constraints` holding 20
   unit tests.

---

## 6. Files

**Engine**

* `crates/vectra-core/src/constraint.rs` — model + registry (new)
* `crates/vectra-core/src/{ids,command,engine,error,document,lib}.rs` —
  `ConstraintId`, the 4 constraint commands + `Batch`/`batch_commands`,
  `Engine::{apply_untracked, amend_top_backward, rollback_last}`,
  `CommandStack::{amend_top_backward, rollback_last}`,
  `EngineEvent::ConstraintsUpdated`, `VectraError::{ConstraintNotFound,
  UnsatisfiableConstraints, is_constraint_conflict}`, `Document.constraints`
* `crates/vectra-constraints/` — `rows.rs`, `pool.rs`, `plan.rs`, `solver.rs`,
  `lib.rs` (`ConstraintError` + re-exports); 20 tests
* `crates/vectra-geometry/src/diagnostic.rs` — `ConstraintDropped`,
  `ConstraintSkipped`, `ParametricLinkBroken` + constructors
* `crates/vectra-dependency/src/{graph,prospective}.rs` — arms for the new
  command/event variants (no prospective edges, no constraint edge kind)
* `crates/vectra-wasm/src/lib.rs` — preflight, `capture_operands`,
  `constraint_pass`, `hints_for`, `settle` wiring; `src/snapshot.rs` —
  `constraints` + `solver` blocks, chained diagnostics
* `crates/vectra-wasm/tests/constraint_laws.rs` — the 7 laws

**UI** — `apps/vectra-web/src/engine/{wire,commands,view-model}.ts`,
`src/App.{tsx,css}`, `tests/view-model.test.ts` (13), `scripts/smoke.mjs` (18),
`src/wasm/*` regenerated by `scripts/build-wasm.sh`.

**Docs** — `TASK-3.1-DESIGN.md` (amended to as-landed), `TASK-3.1-OPEN-ITEMS.md`,
this report.

---

## 7. Boundaries (what 3.1 does *not* do)

* No direct manipulation: 3.1 sets edit variables programmatically (the “drag”
  in the laws/smoke is a `SetParameter`); Task 3.2 owns pointer-driven editing
  and the `constraint` graph edge that makes a drag propagate through the graph.
* `Parallel` / `Perpendicular` are slot-form only; the vector forms need MES §9's
  non-linear pass.
* No inequality constraints (`>=`, `<=`) are exposed on the wire — Cassowary
  supports them, the Phase-1 vocabulary does not use them.
* The drop pre-pass reasons about equalities over one canonical slot form; a
  contradiction spread across several rows (`Distance(A,B)=100`,
  `Distance(B,C)=0`, `Distance(A,C)=100`) is reported by the tableau as an
  unsatisfiable *required* system rather than as a pairwise drop.

---

## 8. Reproduce

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo test -p vectra-wasm --test constraint_laws -- --nocapture
bash apps/vectra-web/scripts/build-wasm.sh      # must precede smoke
node apps/vectra-web/scripts/smoke.mjs
cd apps/vectra-web && npm run typecheck && npm run test:ui && npm run build
```
