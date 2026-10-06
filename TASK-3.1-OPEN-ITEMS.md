# TASK 3.1 — open items (decisions I need before writing the solver)

Three of these are real doctrinal forks; the rest are defaults I will take
unless corrected. Nothing here blocks the parts of the build that are already
unambiguous (the `vectra-core` data model is landed; §D1–D3 are the solver's
semantics, §D4+ are small).

Context for all of them: `/home/user/vectra/TASK-3.1-DESIGN.md`.

---

## D1 — Over-constrained policy: global weakest-drop or pairwise?

**The brief says** (Acceptance #3): “if over-constrained, Cassowary drops the
weakest constraint — the engine must detect this and emit
`Diagnostic::ConstraintDropped`.”
**Cassowary's reality**: it only reports `UnsatisfiableConstraint` for
`REQUIRED` rows it *cannot add at all*; for a weak/medium row that loses, it
**never reports anything** — the row stays in the solver and is merely not
satisfied (its error is minimized). Cassowary has no “I dropped a constraint”
event, and its `fetch_changes`/`get_value` cannot tell you *which* row lost.

**Option A (my recommendation)** — the solver implements the drop itself, as a
deterministic **pairwise** rule over *equalities on the same slot*:
* two different constants on one slot ⇒ the weaker row is **removed from the
  solver and from the document's active set**, the survivor holds;
* tie ⇒ the **newer** constraint is dropped;
* `Required` never yields; `Required` vs `Required` unsatisfiable ⇒ the command
  **fails** with a typed `VectraError::Command` (nothing is applied);
* the drop is emitted as a `Diagnostic::ConstraintDropped` (Warning) naming the
  dropped id, its strength and the winner.

*Pros*: deterministic, matches the brief's language literally (“the weakest
constraint is dropped” — it really is removed, not merely minimized), matches
the Overconstrained law's wording (“weaker dropped”), testable without poking
Cassowary internals. *Cons*: a policy layer on top of Cassowary; only the
equality-on-a-slot shape is detected in Phase 1 (general inequality
contradictions among `Required` rows still surface as a typed add error).

**Option B — literal global minimum**: keep every row in Cassowary and detect
“dropped” by inspecting residuals below a tolerance (a row whose residual never
reaches 0 is reported). *Cons*: cannot distinguish “dropped” from “merely not
satisfied yet”, tolerance-dependent, and it does **not** remove the loser, so
the document and the solver disagree about the active set. I do not recommend
this.

**Option C — report through `diagnose()` only**: implement MES §9's
`ConstraintDiagnostics::{Satisfied, Overconstrained, Conflicting}` on top of
Cassowary and let the UI show the state; no automatic drop. *Cons*: contradicts
the brief's explicit “Cassowary drops the weakest constraint”.

→ **Which reading is doctrine?**

## D2 — May a plain `$variable` be written by the solver?

The brief's grammar allows any slot, including a slot whose source is
`Parameter::Variable("x")`. Today a variable is a plain `f64` in
`Document.variables` (Task 1.3) and everything downstream reads it.

* **Option A (my recommendation)**: yes, if the user constrains a
  variable-bound slot, the solver writes `Document.variables["x"]` and the
  dependents re-evaluate (the existing dirty machinery propagates it). Simple,
  consistent, no special case.
* **Option B**: the solver may only write literal slots; a constraint that
  resolves to a variable/expression source is rejected with a typed error
  (“constrained slots must be literal”). Safer for Phase 1 (nothing can fight
  an authored value), but it makes e.g. “make these two driven rectangles equal”
  impossible, and the UI would have to grey out bound slots.

→ **A or B?**

## D3 — Do constraints become *dependency-graph* edges in 3.1?

* **Option A (my recommendation, smaller diff)**: no new edge kind yet. The
  solver runs inside `dispatch_command`, its writes go through the normal
  dirty/scene path, and the adjusted node ids are added to the emitted
  `Dirty` set. The graph wire format stays exactly as Task 2.2 froze it.
* **Option B**: add a `constraint` coordinator edge (`prop:a.x ⇄ prop:b.x`) to
  `DependencyGraph` now, so dragging A dirties B *through the graph*. Cleaner
  long term (and what Task 3.2's direct manipulation will want), but it changes
  the frozen graph export, the laws' fingerprints, and the snapshot.

→ **A now / B later, or B now?**

## D4 — `Distance` is signed in my mapping

`a - b == value` (linear). The brief writes `dist(a,b) == value`; a true
absolute distance is piecewise-linear (not Cassowary-expressible in one row).
My reading: signed separation, and the AddConstraint builder captures the
*current* signed separation when the user does not supply a value. `EqualLength`
stays on magnitude slots. → Confirm, or should `Distance` reject negative
values / capture `|a-b|`?

## D5 — `Parallel` / `Perpendicular` on slots, not edges

MES §9 names them as edge relations (`cross == 0`, `dot == 0`) in a
vector world that Phase 1 does not have (no node exposes an edge vector; `Path`
slots are `Point2` and out of the float system). My mapping: `Parallel` =
preserved axis offset `a.y - b.y == value` (value captured at add time),
`Perpendicular` = slope-1 diagonals stay orthogonal over 4 targets. The true
vector forms land with MES §9's `solve_nonlinear` (Phase 2+). → Confirm the
Phase-1 reading, or should these two kinds be **rejected as unimplemented** in
Phase 1 (typed error) until the vector pass exists?

## D6 — Undo granularity: one entry per user action

My plan folds the solver's writes into the *causing* command's history entry
(`Command::Batch` + `CommandStack::amend_top_backward`), so “Add Vertical
Constraint” is **one** undo (geometry + registry together) — the brief's Undo
law only holds that way. The alternative (solver writes as separate history
entries) would need several undos per action and no undo would ever be a true
pre-image. → Confirm the single-entry reading (it is also the only one that
satisfies “undo ⇒ geometry reverts to the unconstrained state”).

---

## Defaults I will take unless corrected

* `Coincident` is 2-target component-wise; centre coincidence is two
  constraints (the UI button emits both).
* `Angle` pins a single slot to the given value (radians as stored).
* Duplicate `ConstraintId` insert ⇒ typed error (undo must be invertible).
* `DeleteNode` removes constraints targeting the deleted node (a diagnostic
  note, not a drop warning).
* `SnapshotResponse` gains `constraints` (canonically sorted by id) and
  `solver` stats (`variables`, `constraints`, `edit_variables`) — the latter is
  what makes the variable-count law observable over the wire.
* Half-coefficient (`slot × 2`) helpers are reserved for Task 3.2; no Phase-1
  kind needs them.
