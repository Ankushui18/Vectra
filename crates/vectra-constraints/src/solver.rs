//! The linear constraint solver (MES §9): `Document` + hints → solved values.
//!
//! # What runs, in order
//!
//! 1. [`crate::plan`] decides which constraints are in force (drops, skips,
//!    hard conflicts) — Cassowary never sees a contradictory hard system.
//! 2. Every slot of every live constraint is **interned** into the variable
//!    pool, and its *current* value is read through
//!    [`vectra_core::Resolvable`] — the same resolution path the geometry
//!    evaluator uses (`$variable`, `ƒexpression`, literal).
//! 3. Rows are added at their [`Strength`].
//! 4. Every interned slot gets an **edit variable**: `STRONG` for the slots the
//!    user just moved or anchored (the hints), `WEAK` for everything else. That
//!    is the "stay close to where things are, but obey what the user just did"
//!    objective, expressed with Cassowary's own edit-variable mechanism rather
//!    than a hand-rolled objective.
//! 5. Solved values are read back with `get_value`; only slots that actually
//!    move become [`SolveWrite`]s. The pool then releases every variable no
//!    live constraint references (that release is the observable effect of the
//!    Undo law's variable-count check).
//!
//! # What this crate does *not* do
//!
//! It never mutates the document. It reports what should change; the engine
//! applies the writes through the ordinary inverse-returning command path, so
//! the changes land in the undo stack, the dependency graph and the scene cache
//! exactly like a user edit.

use std::collections::{BTreeMap, BTreeSet};

use cassowary::{strength, Expression, RelationalOperator, Solver, Term, Variable};
use vectra_core::{
    Command, Constraint, ConstraintTarget, Document, EvaluationContext, ParamValue, Resolvable,
    Strength, VectraError,
};

use crate::plan::{plan, preflight, Dropped, Skipped, SolvePlan};
use crate::pool::VariablePool;
use crate::rows::rows_for;
use crate::ConstraintError;

/// Cassowary strength for a [`Strength`] (1:1 by design).
pub fn cassowary_strength(strength: Strength) -> f64 {
    match strength {
        Strength::Required => strength::REQUIRED,
        Strength::Strong => strength::STRONG,
        Strength::Medium => strength::MEDIUM,
        Strength::Weak => strength::WEAK,
    }
}

/// Counters for one solve (and, for `variables`, the live pool size).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SolverStats {
    /// Interned variables *after* this solve's garbage collection — the
    /// "Cassowary internal variable count".
    pub variables: usize,
    /// Rows loaded into the tableau.
    pub constraints: usize,
    /// Edit variables in the tableau (stays + hints + pointer edits).
    pub edit_variables: usize,
    /// Pointer-driven edit variables currently registered (Task 3.2). Zero for
    /// a one-shot solve; 1–2 while a drag gesture is live.
    pub active_edits: usize,
    /// Slots whose value the solve moved.
    pub writes: usize,
    /// Constraints dropped by the pairwise rule.
    pub dropped: usize,
    /// Constraints skipped because they could not be evaluated.
    pub skipped: usize,
}

/// One slot the solver wants to move.
#[derive(Debug, Clone, PartialEq)]
pub struct SolveWrite {
    /// The slot.
    pub target: ConstraintTarget,
    /// The solved value.
    pub value: f64,
    /// The value the slot currently resolves to.
    pub previous: f64,
    /// How the slot is currently *sourced* — a `Literal`, `$variable` or
    /// `ƒexpression`. Anything but a literal is a link the write has to break.
    pub source: ParamValue,
}

impl SolveWrite {
    /// True when applying this write would break a parametric link, i.e. the
    /// slot is currently driven by a variable or an expression.
    pub fn breaks_link(&self) -> bool {
        !matches!(self.source, ParamValue::Float(ref p) if matches!(p, vectra_core::Parameter::Literal(_)))
    }
}

/// The result of one solve.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SolveOutcome {
    /// Slots to write, in canonical slot order.
    pub writes: Vec<SolveWrite>,
    /// Constraints the pre-pass dropped (the engine disables + diagnoses them).
    pub dropped: Vec<Dropped>,
    /// Constraints that could not be enforced this pass.
    pub skipped: Vec<Skipped>,
    /// Counters.
    pub stats: SolverStats,
}

impl SolveOutcome {
    /// The nodes whose geometry this solve moved (the solver's `DirtySet`).
    pub fn dirty_nodes(&self) -> Vec<vectra_core::NodeId> {
        let mut ids: Vec<vectra_core::NodeId> = Vec::new();
        for write in &self.writes {
            if !ids.contains(&write.target.node_id) {
                ids.push(write.target.node_id);
            }
        }
        ids
    }

    /// True when the solve changed nothing (the overwhelmingly common case in a
    /// document without constraints).
    pub fn is_noop(&self) -> bool {
        self.writes.is_empty() && self.dropped.is_empty() && self.skipped.is_empty()
    }
}

/// The solver: owns the variable pool (and therefore the variable count across
/// solves), rebuilds the tableau on every solve, and — for the duration of a
/// drag gesture — keeps one tableau alive (Task 3.2).
#[derive(Default)]
pub struct ConstraintSolver {
    pool: VariablePool,
    stats: SolverStats,
    /// The active-edits registry: `Some` exactly while a drag gesture is live.
    drag: Option<DragSession>,
}

/// A live drag gesture (Task 3.2).
///
/// Cassowary's edit-variable API is built for exactly this: the slots the
/// pointer drives are *edit variables*, `suggest_value` nudges them, and the
/// tableau pulls every constrained partner along. The difference from a
/// one-shot [`ConstraintSolver::solve`] is lifetime — this tableau is built
/// once, at `BeginDrag`, and reused for every pointer sample until `EndDrag`
/// releases it: no re-planning, no re-loading, no full re-solve per mouse move.
pub struct DragSession {
    /// The node the pointer owns.
    node_id: vectra_core::NodeId,
    /// The pointer-driven slots, in `(x, y)` order — the active-edits registry.
    edits: Vec<ConstraintTarget>,
    /// The persistent tableau with its interned slots.
    tableau: Tableau,
    /// Pointer samples applied so far (reported in the end-of-gesture summary).
    updates: u32,
}

impl DragSession {
    /// The node this gesture is dragging.
    pub fn node_id(&self) -> vectra_core::NodeId {
        self.node_id
    }

    /// The slots the pointer drives (1–2).
    pub fn edits(&self) -> &[ConstraintTarget] {
        &self.edits
    }

    /// Pointer samples applied so far.
    pub fn updates(&self) -> u32 {
        self.updates
    }

    /// `(target, variable)` for each pointer-driven slot.
    fn edit_variables(&self) -> Vec<(ConstraintTarget, Variable)> {
        self.edits
            .iter()
            .filter_map(|target| {
                self.tableau
                    .slots
                    .get(&target.label())
                    .map(|(_, variable, _)| (target.clone(), *variable))
            })
            .collect()
    }
}

/// A loaded tableau: the Cassowary solver, the slots it holds (label →
/// target/variable/last value), and how many rows are in it.
struct Tableau {
    solver: Solver,
    slots: BTreeMap<String, (ConstraintTarget, Variable, f64)>,
    rows: usize,
    /// Every edit variable in the tableau (pointer + hints + stays).
    edits: usize,
}

impl ConstraintSolver {
    /// A solver with an empty pool.
    pub fn new() -> Self {
        Self::default()
    }

    /// Live pool size — the number of interned solver variables.
    pub fn variable_count(&self) -> usize {
        self.pool.len()
    }

    /// Counters from the most recent solve (or drag sample).
    pub fn stats(&self) -> SolverStats {
        self.stats
    }

    /// Plan a solve without running it (drop pre-pass only).
    pub fn plan(&self, doc: &Document) -> SolvePlan {
        plan(doc)
    }

    /// Pre-flight a command against the constraint system.
    ///
    /// Runs before the document is touched, so a rule that can never hold (a
    /// contradiction between `Required` rows, a slot that does not exist) is a
    /// typed rejection with **nothing** applied — the same "not even for a
    /// frame" discipline as the Task 2.2 cycle gate.
    pub fn preflight(doc: &Document, cmd: &Command) -> Result<(), ConstraintError> {
        preflight(doc, cmd)
    }

    /// Solve the active constraint system (one shot).
    ///
    /// `hints` are the slots the user just acted on: they are pinned at
    /// `STRONG`, so the solver moves everything *else* to satisfy the rules.
    /// Returns what should change — this never touches `doc`.
    pub fn solve(
        &mut self,
        doc: &Document,
        ctx: &EvaluationContext<'_>,
        hints: &[ConstraintTarget],
    ) -> Result<SolveOutcome, ConstraintError> {
        let plan = plan(doc);
        if let Some(conflict) = &plan.conflict {
            return Err(ConstraintError::Unsatisfiable {
                message: conflict.message.clone(),
            });
        }

        let mut skipped = plan.skipped.clone();
        let strong: BTreeSet<String> = hints.iter().map(ConstraintTarget::label).collect();
        let Some(tableau) = self.build_tableau(doc, ctx, &plan, &mut skipped, &strong, &[])? else {
            // No live rules: release every variable and report an empty tableau
            // (this is the second half of the undo law's variable-count check).
            self.pool.gc(&BTreeSet::new());
            self.stats = SolverStats {
                variables: self.pool.len(),
                ..SolverStats::default()
            };
            return Ok(SolveOutcome {
                dropped: plan.dropped.clone(),
                skipped,
                stats: self.stats,
                ..SolveOutcome::default()
            });
        };

        // ── 5. read the solution back ────────────────────────────────────
        let writes = read_back(doc, ctx, &tableau.solver, &tableau.slots)?;

        // ── 6. release variables nothing refers to any more ──────────────
        let keep: BTreeSet<String> = tableau.slots.keys().cloned().collect();
        self.pool.gc(&keep);

        let outcome = SolveOutcome {
            stats: SolverStats {
                variables: self.pool.len(),
                constraints: tableau.rows,
                edit_variables: tableau.edits,
                // A one-shot solve holds no *pointer* edits: the active-edits
                // registry belongs to a gesture.
                active_edits: 0,
                writes: writes.len(),
                dropped: plan.dropped.len(),
                skipped: skipped.len(),
            },
            writes,
            dropped: plan.dropped.clone(),
            skipped,
        };
        self.stats = outcome.stats;
        Ok(outcome)
    }

    /// Open a drag gesture on `node_id` (Task 3.2).
    ///
    /// Builds the tableau once and registers the node's canonical position slots
    /// as `STRONG` edit variables — that registry *is* the drag. Slots no rule
    /// mentions are interned too, which is what makes dragging an unconstrained
    /// node work. The plan's drops/skips are reported here, once per gesture.
    pub fn begin_drag(
        &mut self,
        doc: &Document,
        ctx: &EvaluationContext<'_>,
        node_id: vectra_core::NodeId,
    ) -> Result<SolveOutcome, ConstraintError> {
        if self.drag.is_some() {
            return Err(ConstraintError::DragBusy { node_id });
        }
        let pointer = position_targets(doc, node_id)?;

        let plan = plan(doc);
        if let Some(conflict) = &plan.conflict {
            return Err(ConstraintError::Unsatisfiable {
                message: conflict.message.clone(),
            });
        }

        let mut skipped = plan.skipped.clone();
        let strong: BTreeSet<String> = pointer.iter().map(ConstraintTarget::label).collect();
        let Some(tableau) = self.build_tableau(doc, ctx, &plan, &mut skipped, &strong, &pointer)?
        else {
            // Unreachable: `pointer` is never empty, and a non-empty pointer set
            // always keeps the tableau alive.
            return Err(ConstraintError::Other(
                "drag session built without pointer slots".to_string(),
            ));
        };

        // A gesture starts on the geometry as it is; if the system was not
        // satisfied yet, the first read moves what it must (and says so).
        let writes = read_back(doc, ctx, &tableau.solver, &tableau.slots)?;
        self.drag = Some(DragSession {
            node_id,
            edits: pointer,
            tableau,
            updates: 0,
        });
        let keep: BTreeSet<String> = self
            .drag
            .as_ref()
            .map(|session| session.tableau.slots.keys().cloned().collect())
            .unwrap_or_default();
        self.pool.gc(&keep);

        let stats = SolverStats {
            variables: self.pool.len(),
            constraints: self
                .drag
                .as_ref()
                .map(|session| session.tableau.rows)
                .unwrap_or(0),
            edit_variables: self
                .drag
                .as_ref()
                .map(|session| session.tableau.edits)
                .unwrap_or(0),
            active_edits: self.active_edits(),
            writes: writes.len(),
            dropped: plan.dropped.len(),
            skipped: skipped.len(),
        };
        self.stats = stats;
        Ok(SolveOutcome {
            writes,
            dropped: plan.dropped,
            skipped,
            stats,
        })
    }

    /// One pointer sample: `suggest_value(x)` / `suggest_value(y)` on the active
    /// edits, then read the whole tableau back.
    ///
    /// Returns the slots that must be written to the document — the dragged node
    /// plus every constrained partner the tableau pulled along.
    pub fn update_drag(
        &mut self,
        doc: &Document,
        ctx: &EvaluationContext<'_>,
        x: f64,
        y: f64,
    ) -> Result<SolveOutcome, ConstraintError> {
        let Some(session) = self.drag.as_mut() else {
            return Err(ConstraintError::NoDragSession);
        };
        session.updates += 1;

        // Absolute coordinates: `x` drives the first position slot, `y` the
        // second (see `position_targets`).
        let requested = [x, y];
        let pointer = session.edits.clone();
        let DragSession { tableau, .. } = session;
        for (index, target) in pointer.iter().enumerate() {
            let Some(asked) = requested.get(index).copied() else {
                break;
            };
            let label = target.label();
            let Some((_, variable, _)) = tableau.slots.get(&label) else {
                continue;
            };
            tableau
                .solver
                .suggest_value(*variable, asked)
                .map_err(|error| ConstraintError::Solver(format!("{error:?}")))?;
        }

        let writes = read_back(doc, ctx, &tableau.solver, &tableau.slots)?;

        // Keep every slot the gesture may still touch; release the rest.
        let keep: BTreeSet<String> = tableau.slots.keys().cloned().collect();
        self.pool.gc(&keep);

        let stats = SolverStats {
            variables: self.pool.len(),
            constraints: self
                .drag
                .as_ref()
                .map(|session| session.tableau.rows)
                .unwrap_or(0),
            edit_variables: self
                .drag
                .as_ref()
                .map(|session| session.tableau.edits)
                .unwrap_or(0),
            active_edits: self.active_edits(),
            writes: writes.len(),
            dropped: 0,
            skipped: 0,
        };
        self.stats = stats;
        Ok(SolveOutcome {
            writes,
            dropped: Vec::new(),
            skipped: Vec::new(),
            stats,
        })
    }

    /// Close the gesture: release the edit variables **explicitly** through the
    /// crate's own API, discard the tableau, and garbage-collect the pool so a
    /// variable that existed only for the drag is gone.
    ///
    /// Returns how many edit variables were released. The document keeps the last
    /// solved values as literals — writing them is the engine's job.
    pub fn end_drag(&mut self, doc: &Document) -> Result<usize, ConstraintError> {
        let Some(session) = self.drag.take() else {
            return Err(ConstraintError::NoDragSession);
        };
        let active = session.edit_variables();
        let mut tableau = session.tableau;
        let mut released = 0usize;
        for (_, variable) in active {
            if tableau.solver.remove_edit_variable(variable).is_ok() {
                released += 1;
            }
        }
        // Variables the live rules still need stay interned; everything that was
        // only there for the gesture goes.
        let keep: BTreeSet<String> = plan(doc)
            .kept
            .iter()
            .filter_map(|id| doc.constraints.get(*id))
            .flat_map(crate::rows::targets_of)
            .map(|target| target.label())
            .collect();
        self.pool.gc(&keep);
        self.stats = SolverStats {
            variables: self.pool.len(),
            ..SolverStats::default()
        };
        Ok(released)
    }

    /// Size of the active-edits registry (0 when nothing is being dragged) —
    /// the Task 3.2 persistence law's observable.
    pub fn active_edits(&self) -> usize {
        self.drag
            .as_ref()
            .map(|session| session.edits.len())
            .unwrap_or(0)
    }

    /// True while a drag gesture is live.
    pub fn is_dragging(&self) -> bool {
        self.drag.is_some()
    }

    /// The node currently being dragged, if any.
    pub fn drag_node(&self) -> Option<vectra_core::NodeId> {
        self.drag.as_ref().map(|session| session.node_id)
    }

    /// Pointer samples applied in the live gesture (0 when idle).
    pub fn drag_updates(&self) -> u32 {
        self.drag
            .as_ref()
            .map(|session| session.updates)
            .unwrap_or(0)
    }

    /// Intern every slot the plan's live rules need **plus** `pointer`, load the
    /// rows, and add the edit variables (STRONG for `strong`/`pointer`, WEAK
    /// stays for everything else).
    ///
    /// `Ok(None)` = nothing to solve (no live rules and no pointer slots) — the
    /// fast path a document without constraints takes.
    fn build_tableau(
        &mut self,
        doc: &Document,
        ctx: &EvaluationContext<'_>,
        plan: &SolvePlan,
        skipped: &mut Vec<Skipped>,
        strong: &BTreeSet<String>,
        pointer: &[ConstraintTarget],
    ) -> Result<Option<Tableau>, ConstraintError> {
        // ── 2. read the current value of every slot a live row needs ──────
        let mut slots: BTreeMap<String, (ConstraintTarget, Variable, f64)> = BTreeMap::new();
        let mut live: Vec<&Constraint> = Vec::new();
        for id in &plan.kept {
            let Some(constraint) = doc.constraints.get(*id) else {
                continue;
            };
            let mut readable = true;
            let mut reason = String::new();
            for target in crate::rows::targets_of(constraint) {
                match current_value(doc, ctx, &target) {
                    Ok(value) => {
                        let label = target.label();
                        let variable = self.pool.intern(&target);
                        slots.insert(label, (target, variable, value));
                    }
                    Err(error) => {
                        readable = false;
                        reason = error.to_string();
                        break;
                    }
                }
            }
            if readable {
                live.push(constraint);
            } else {
                skipped.push(Skipped {
                    id: constraint.id,
                    kind: constraint.kind,
                    reason,
                });
            }
        }

        // The slots the pointer drives are interned even when no rule mentions
        // them: an unconstrained node must still be draggable.
        for target in pointer {
            let label = target.label();
            if slots.contains_key(&label) {
                continue;
            }
            let value = current_value(doc, ctx, target)?;
            let variable = self.pool.intern(target);
            slots.insert(label, (target.clone(), variable, value));
        }

        if live.is_empty() && pointer.is_empty() {
            return Ok(None);
        }

        // ── 3. load the tableau ───────────────────────────────────────────
        let mut cassowary = Solver::new();
        let mut rows_loaded = 0usize;
        for constraint in &live {
            for row in rows_for(constraint)? {
                let terms: Vec<Term> = row
                    .terms
                    .iter()
                    .map(|(target, coefficient)| Term {
                        variable: self
                            .pool
                            .get(&target.label())
                            .expect("live slot is interned"),
                        coefficient: *coefficient,
                    })
                    .collect();
                let expression = Expression::new(terms, -row.constant);
                let cn = cassowary::Constraint::new(
                    expression,
                    RelationalOperator::Equal,
                    cassowary_strength(constraint.strength),
                );
                cassowary
                    .add_constraint(cn)
                    .map_err(|error| match error {
                        cassowary::AddConstraintError::UnsatisfiableConstraint => {
                            ConstraintError::Unsatisfiable {
                                message: format!(
                                    "required {} [{}] cannot hold together with the constraints already in force",
                                    constraint.kind.tag(),
                                    crate::rows::describe_targets(&constraint.targets)
                                ),
                            }
                        }
                        other => ConstraintError::Solver(format!("{other:?}")),
                    })?;
                rows_loaded += 1;
            }
        }

        // ── 4. stays (WEAK), hints (STRONG) and pointer edits (STRONG) ────
        let driven: BTreeSet<String> = pointer.iter().map(ConstraintTarget::label).collect();
        let mut edits = 0usize;
        for (label, (_, variable, current)) in &slots {
            let is_driven = driven.contains(label) || strong.contains(label);
            let strength = if is_driven {
                strength::STRONG
            } else {
                strength::WEAK
            };
            cassowary
                .add_edit_variable(*variable, strength)
                .map_err(|error| ConstraintError::Solver(format!("{error:?}")))?;
            cassowary
                .suggest_value(*variable, *current)
                .map_err(|error| ConstraintError::Solver(format!("{error:?}")))?;
            edits += 1;
        }

        Ok(Some(Tableau {
            solver: cassowary,
            slots,
            rows: rows_loaded,
            edits,
        }))
    }
}

/// The canonical position slots of a node as solver targets, in `(x, y)` order.
///
/// `Err(NotDraggable)` for kinds with no position at all (`Path`, `Group`).
pub fn position_targets(
    doc: &Document,
    node_id: vectra_core::NodeId,
) -> Result<Vec<ConstraintTarget>, ConstraintError> {
    let node = doc.get_node(node_id)?;
    match node.position_slots() {
        Some((x, y)) => Ok(vec![
            ConstraintTarget::new(node_id, x),
            ConstraintTarget::new(node_id, y),
        ]),
        None => Err(ConstraintError::NotDraggable { node_id }),
    }
}

/// Read every interned slot back from the tableau, keeping only real changes.
///
/// "Real" is measured against the **document**, not against the value captured
/// while loading: in a drag session the document already holds the previous
/// sample's solution, so each pointer sample reports a write only for the slots
/// that actually moved.
fn read_back(
    doc: &Document,
    ctx: &EvaluationContext<'_>,
    solver: &Solver,
    slots: &BTreeMap<String, (ConstraintTarget, Variable, f64)>,
) -> Result<Vec<SolveWrite>, ConstraintError> {
    let mut writes = Vec::new();
    for (target, variable, _) in slots.values() {
        let solved = solver.get_value(*variable);
        if !solved.is_finite() {
            return Err(ConstraintError::NonFinite {
                id: vectra_core::ConstraintId::nil(),
                value: solved,
            });
        }
        let current = current_value(doc, ctx, target)?;
        if solved != current {
            writes.push(SolveWrite {
                target: target.clone(),
                value: solved,
                previous: current,
                source: source_of(doc, target)?,
            });
        }
    }
    Ok(writes)
}

/// Resolve a slot's current numeric value through the ordinary resolution path.
pub fn current_value(
    doc: &Document,
    ctx: &EvaluationContext<'_>,
    target: &ConstraintTarget,
) -> Result<f64, ConstraintError> {
    let node = doc.get_node(target.node_id)?;
    match node.get_param(&target.property)? {
        ParamValue::Float(param) => Ok(param.resolve(ctx).map_err(VectraError::from)?),
        other => Err(VectraError::PropertyTypeMismatch {
            property: target.property.clone(),
            expected: "float",
            got: other.kind(),
        }
        .into()),
    }
}

/// How a slot is currently sourced (for the link-breaking report).
fn source_of(doc: &Document, target: &ConstraintTarget) -> Result<ParamValue, ConstraintError> {
    let node = doc.get_node(target.node_id)?;
    Ok(node.get_param(&target.property)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use vectra_core::{
        new_constraint_id, new_node_id, ConstraintKind, Engine, NodeId, NodeKind, ParamValue,
        Parameter,
    };

    /// Two rectangles with a live constraint, ready to solve.
    fn scene(a_x: f64, b_x: f64) -> (Engine, NodeId, NodeId) {
        let mut engine = Engine::new();
        let a = new_node_id();
        let b = new_node_id();
        engine
            .dispatch(Command::CreateNode {
                id: a,
                kind: NodeKind::rectangle(a_x, 0.0, 10.0, 10.0),
                name: Some("a".into()),
                index: None,
            })
            .unwrap();
        engine
            .dispatch(Command::CreateNode {
                id: b,
                kind: NodeKind::rectangle(b_x, 0.0, 10.0, 10.0),
                name: Some("b".into()),
                index: None,
            })
            .unwrap();
        (engine, a, b)
    }

    fn vertical(a: NodeId, b: NodeId) -> Constraint {
        Constraint::new(
            new_constraint_id(),
            ConstraintKind::Vertical,
            vec![ConstraintTarget::new(a, "x"), ConstraintTarget::new(b, "x")],
        )
    }

    fn solve(
        engine: &Engine,
        solver: &mut ConstraintSolver,
        hints: &[ConstraintTarget],
    ) -> SolveOutcome {
        let ctx = engine.evaluation_context();
        solver.solve(engine.document(), &ctx, hints).unwrap()
    }

    /// Write a solve's result back, the way the engine does. Crate-level tests
    /// have to do this by hand: the solver proposes, the engine commits.
    fn apply_writes(engine: &mut Engine, outcome: &SolveOutcome) {
        for write in &outcome.writes {
            let node = engine
                .document_mut()
                .get_node_mut(write.target.node_id)
                .unwrap();
            node.set_param(
                &write.target.property,
                ParamValue::Float(Parameter::Literal(write.value)),
            )
            .unwrap();
        }
    }

    /// Solve *and* write the result back — so a test starts from a document
    /// whose rules already hold.
    fn solve_and_apply(
        engine: &mut Engine,
        solver: &mut ConstraintSolver,
        hints: &[ConstraintTarget],
    ) -> SolveOutcome {
        let outcome = {
            let ctx = engine.evaluation_context();
            solver.solve(engine.document(), &ctx, hints).unwrap()
        };
        apply_writes(engine, &outcome);
        outcome
    }

    #[test]
    fn the_active_edits_registry_opens_and_closes_with_the_gesture() {
        let (mut engine, a, b) = scene(100.0, 300.0);
        let cn = vertical(a, b);
        engine
            .dispatch(Command::AddConstraint {
                constraint: cn.clone(),
            })
            .unwrap();
        let mut solver = ConstraintSolver::new();
        solve_and_apply(&mut engine, &mut solver, &[ConstraintTarget::new(a, "x")]);
        let pooled_with_rule = solver.variable_count();

        let outcome = {
            let ctx = engine.evaluation_context();
            solver
                .begin_drag(engine.document(), &ctx, a)
                .expect("gesture opens")
        };
        assert!(
            outcome.writes.is_empty(),
            "nothing had to move: {outcome:?}"
        );
        assert_eq!(solver.active_edits(), 2, "x and y are registered");
        assert!(solver.is_dragging());
        assert_eq!(solver.drag_node(), Some(a));
        assert_eq!(solver.stats().active_edits, 2);

        // One pointer sample: the dragged slot and the partner both move.
        let outcome = {
            let ctx = engine.evaluation_context();
            solver
                .update_drag(engine.document(), &ctx, 250.0, 40.0)
                .expect("sample applies")
        };
        let mut moved: Vec<ConstraintTarget> =
            outcome.writes.iter().map(|w| w.target.clone()).collect();
        moved.sort_by_key(|target| target.label());
        assert_eq!(moved.len(), 3, "a.x, a.y and the partner b.x: {outcome:?}");
        assert_eq!(solver.drag_updates(), 1);
        apply_writes(&mut engine, &outcome);

        // Repeating the same coordinates is a no-op — the tableau already holds
        // that solution, and the document already has the values.
        let outcome = {
            let ctx = engine.evaluation_context();
            solver
                .update_drag(engine.document(), &ctx, 250.0, 40.0)
                .expect("repeat sample")
        };
        assert!(
            outcome.writes.is_empty(),
            "re-suggesting the same point writes nothing: {outcome:?}"
        );

        // EndDrag releases the pointer's edit variables explicitly.
        let released = solver.end_drag(engine.document()).expect("gesture closes");
        assert_eq!(released, 2, "both pointer edits were removed");
        assert_eq!(solver.active_edits(), 0);
        assert!(!solver.is_dragging());
        assert_eq!(solver.drag_node(), None);
        assert!(
            solver.variable_count() <= pooled_with_rule,
            "the pool keeps the slots the live rule still needs: {} <= {}",
            solver.variable_count(),
            pooled_with_rule
        );
        assert!(
            solver.end_drag(engine.document()).is_err(),
            "no session left"
        );
    }

    #[test]
    fn a_pinned_slot_resists_the_pointer_and_a_weak_rule_does_not() {
        let (mut engine, a, _b) = scene(100.0, 300.0);
        // Pin a.x to 100 with a Required rule.
        engine
            .dispatch(Command::AddConstraint {
                constraint: Constraint::new(
                    new_constraint_id(),
                    ConstraintKind::Angle,
                    vec![ConstraintTarget::new(a, "x")],
                )
                .with_strength(Strength::Required)
                .with_value(100.0),
            })
            .unwrap();

        let mut solver = ConstraintSolver::new();
        let outcome = {
            let ctx = engine.evaluation_context();
            solver.begin_drag(engine.document(), &ctx, a).unwrap();
            solver
                .update_drag(engine.document(), &ctx, 900.0, 900.0)
                .unwrap()
        };
        let pinned = outcome
            .writes
            .iter()
            .find(|write| write.target == ConstraintTarget::new(a, "x"));
        assert!(
            pinned.is_none(),
            "a Required rule outranks the pointer's STRONG edit: {outcome:?}"
        );
        solver.end_drag(engine.document()).unwrap();

        // The same pin at Weak yields to the pointer while it is down.
        let mut engine = Engine::new();
        let node = new_node_id();
        engine
            .dispatch(Command::CreateNode {
                id: node,
                kind: NodeKind::rectangle(100.0, 0.0, 10.0, 10.0),
                name: Some("a".into()),
                index: None,
            })
            .unwrap();
        engine
            .dispatch(Command::AddConstraint {
                constraint: Constraint::new(
                    new_constraint_id(),
                    ConstraintKind::Angle,
                    vec![ConstraintTarget::new(node, "x")],
                )
                .with_strength(Strength::Weak)
                .with_value(100.0),
            })
            .unwrap();
        let mut solver = ConstraintSolver::new();
        let outcome = {
            let ctx = engine.evaluation_context();
            solver.begin_drag(engine.document(), &ctx, node).unwrap();
            solver
                .update_drag(engine.document(), &ctx, 400.0, 0.0)
                .unwrap()
        };
        assert!(
            outcome
                .writes
                .iter()
                .any(|write| write.target == ConstraintTarget::new(node, "x")
                    && write.value == 400.0),
            "a Weak rule cannot hold a slot against the pointer: {outcome:?}"
        );
        solver.end_drag(engine.document()).unwrap();
    }

    #[test]
    fn a_vertical_constraint_moves_only_the_free_slot() {
        let (mut engine, a, b) = scene(100.0, 300.0);
        let cn = vertical(a, b);
        engine
            .dispatch(Command::AddConstraint {
                constraint: cn.clone(),
            })
            .unwrap();
        let mut solver = ConstraintSolver::new();

        // Anchor A (the constraint's first target): B snaps to A, exactly.
        let outcome = solve(&engine, &mut solver, &[ConstraintTarget::new(a, "x")]);
        assert_eq!(outcome.writes.len(), 1);
        assert_eq!(outcome.writes[0].target, ConstraintTarget::new(b, "x"));
        assert_eq!(outcome.writes[0].value, 100.0);
        assert!(!outcome.writes[0].breaks_link());
        assert_eq!(outcome.stats.variables, 2);
        assert_eq!(outcome.stats.constraints, 1);
        assert_eq!(outcome.stats.edit_variables, 2);
    }

    #[test]
    fn an_already_satisfied_system_is_a_noop() {
        let (mut engine, a, b) = scene(100.0, 100.0);
        engine
            .dispatch(Command::AddConstraint {
                constraint: vertical(a, b),
            })
            .unwrap();
        let mut solver = ConstraintSolver::new();
        let outcome = solve(&engine, &mut solver, &[]);
        assert!(outcome.is_noop(), "no writes, no drops: {outcome:?}");
        assert_eq!(solver.variable_count(), 2, "the pool still holds the slots");
    }

    #[test]
    fn solving_is_idempotent_and_the_pool_shrinks_when_rules_leave() {
        let (mut engine, a, b) = scene(100.0, 300.0);
        let cn = vertical(a, b);
        engine
            .dispatch(Command::AddConstraint {
                constraint: cn.clone(),
            })
            .unwrap();
        let mut solver = ConstraintSolver::new();
        let first = solve(&engine, &mut solver, &[ConstraintTarget::new(a, "x")]);
        assert_eq!(first.writes[0].value, 100.0);
        assert_eq!(solver.variable_count(), 2);

        // Apply the write; solving again must be a no-op (idempotence).
        engine
            .dispatch(Command::SetParameter {
                node_id: b,
                property: "x".into(),
                value: ParamValue::Float(Parameter::Literal(100.0)),
            })
            .unwrap();
        let second = solve(&engine, &mut solver, &[]);
        assert!(second.writes.is_empty(), "second solve moved nothing");
        assert_eq!(solver.variable_count(), 2);

        // Remove the rule: the pool releases both slots.
        engine
            .dispatch(Command::RemoveConstraint { id: cn.id })
            .unwrap();
        let third = solve(&engine, &mut solver, &[]);
        assert!(third.is_noop());
        assert_eq!(solver.variable_count(), 0, "no live rule ⇒ no variables");
    }

    #[test]
    fn a_bound_slot_is_reported_as_a_broken_link() {
        let (mut engine, a, b) = scene(100.0, 300.0);
        engine
            .dispatch(Command::SetVariable {
                name: "base".into(),
                value: 300.0,
            })
            .unwrap();
        engine
            .dispatch(Command::SetParameter {
                node_id: b,
                property: "x".into(),
                value: ParamValue::Float(Parameter::variable("base".to_string())),
            })
            .unwrap();
        engine
            .dispatch(Command::AddConstraint {
                constraint: vertical(a, b),
            })
            .unwrap();

        let mut solver = ConstraintSolver::new();
        let outcome = solve(&engine, &mut solver, &[ConstraintTarget::new(a, "x")]);
        assert_eq!(outcome.writes.len(), 1);
        assert!(
            outcome.writes[0].breaks_link(),
            "the write has to unlink $base"
        );
        assert_eq!(
            outcome.writes[0].source,
            ParamValue::Float(Parameter::variable("base".to_string()))
        );
        // The solver never writes variables: it only proposes the slot value.
        assert_eq!(engine.get_variable("base"), Some(300.0));
    }

    #[test]
    fn mirrored_distance_rows_do_not_reach_the_tableau() {
        let (mut engine, a, b) = scene(100.0, 200.0);
        let strong = Constraint::new(
            new_constraint_id(),
            ConstraintKind::Distance,
            vec![ConstraintTarget::new(a, "x"), ConstraintTarget::new(b, "x")],
        )
        .with_value(100.0)
        .with_strength(Strength::Medium);
        let weak = Constraint::new(
            new_constraint_id(),
            ConstraintKind::Distance,
            vec![ConstraintTarget::new(b, "x"), ConstraintTarget::new(a, "x")],
        )
        .with_value(-300.0)
        .with_strength(Strength::Weak);
        engine
            .dispatch(Command::AddConstraint {
                constraint: strong.clone(),
            })
            .unwrap();
        engine
            .dispatch(Command::AddConstraint {
                constraint: weak.clone(),
            })
            .unwrap();

        let mut solver = ConstraintSolver::new();
        let outcome = solve(&engine, &mut solver, &[ConstraintTarget::new(a, "x")]);
        assert_eq!(outcome.dropped.len(), 1, "mirrored rows conflict");
        assert_eq!(outcome.dropped[0].id, weak.id);
        assert_eq!(outcome.dropped[0].kept, strong.id);
        // 100 - 200 = -100 ⟹ b.x := a.x - 100 = 0
        assert_eq!(outcome.writes.len(), 1);
        assert_eq!(outcome.writes[0].value, 0.0);
    }

    #[test]
    fn preflight_is_a_pure_read() {
        let (engine, a, b) = scene(0.0, 0.0);
        let bad = Constraint::new(
            new_constraint_id(),
            ConstraintKind::Vertical,
            vec![ConstraintTarget::new(a, "x")],
        );
        let cmd = Command::AddConstraint {
            constraint: bad.clone(),
        };
        assert!(ConstraintSolver::preflight(engine.document(), &cmd).is_err());
        assert!(
            engine.document().constraints.is_empty(),
            "nothing was applied"
        );
        let ok = Command::AddConstraint {
            constraint: vertical(a, b),
        };
        assert!(ConstraintSolver::preflight(engine.document(), &ok).is_ok());
    }
}
