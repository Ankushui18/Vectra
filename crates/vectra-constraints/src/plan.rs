//! The over-constraint pre-pass (MES §9, Task 3.1 doctrine).
//!
//! Cassowary never reports *which* non-required row it failed to satisfy — a
//! weak row that loses simply stays in the tableau with a non-zero residual.
//! So the drop is decided here, deterministically, **before** the tableau sees
//! anything:
//!
//! * rows are grouped by canonical linear form (same slots, same coefficients);
//! * inside a group, the strongest constraint wins, ties broken by registration
//!   order (the **newer** constraint loses);
//! * every other constraint in the group whose constant differs is *dropped* —
//!   it is removed from the active set, disabled in the document and reported
//!   as a `Diagnostic::ConstraintDropped`;
//! * two contradicting `Required` rows are not droppable at all: the command is
//!   rejected with `VectraError::UnsatisfiableConstraints` and nothing is
//!   applied.
//!
//! Grouping is by form, so the rule catches contradictions across kinds too
//! (`Vertical(a.x, b.x)` and `Distance(a.x, b.x) = 100` describe the same line).
//! Identical form **and** constant is redundancy, which is harmless and kept.

use std::collections::BTreeMap;

use vectra_core::{Command, Constraint, ConstraintId, ConstraintKind, Document, Strength};

use crate::rows::{describe_targets, rows_for, validate, Row};
use crate::ConstraintError;

/// A constraint the pre-pass removed from the active set.
#[derive(Debug, Clone, PartialEq)]
pub struct Dropped {
    /// The loser.
    pub id: ConstraintId,
    /// Its kind.
    pub kind: ConstraintKind,
    /// Its strength.
    pub strength: Strength,
    /// The linear form the two rows disagreed on.
    pub form: String,
    /// The winner.
    pub kept: ConstraintId,
    /// The winner's kind.
    pub kept_kind: ConstraintKind,
    /// The winner's strength.
    pub kept_strength: Strength,
    /// The winner's targets, human-readable (for the diagnostic message).
    pub kept_targets: String,
}

/// A constraint that could not be enforced this pass (unresolvable slot),
/// parked with a reason rather than silently ignored (Task 1.3 doctrine:
/// evaluation is total, every fallback is reported).
#[derive(Debug, Clone, PartialEq)]
pub struct Skipped {
    /// The skipped constraint.
    pub id: ConstraintId,
    /// Its kind.
    pub kind: ConstraintKind,
    /// Why it is not in the tableau.
    pub reason: String,
}

/// A hard contradiction between `Required` rows.
#[derive(Debug, Clone, PartialEq)]
pub struct Conflict {
    /// Machine-readable summary.
    pub message: String,
    /// Everyone involved, in registration order.
    pub ids: Vec<ConstraintId>,
}

/// What the solver should load, what it refused to load, and why.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SolvePlan {
    /// Constraints to load into the tableau, in registration order.
    pub kept: Vec<ConstraintId>,
    /// Constraints dropped by the pairwise rule (to be disabled + diagnosed).
    pub dropped: Vec<Dropped>,
    /// Constraints skipped because they cannot be evaluated.
    pub skipped: Vec<Skipped>,
    /// Hard conflict, if the system is inconsistent at `Required`.
    pub conflict: Option<Conflict>,
}

impl SolvePlan {
    /// True when nothing can be enforced this pass.
    pub fn is_empty(&self) -> bool {
        self.kept.is_empty()
    }
}

/// One validated constraint with its rows.
struct Entry<'a> {
    index: usize,
    constraint: &'a Constraint,
    rows: Vec<Row>,
}

/// Sort key: strongest first, then oldest first (so a tie drops the newer one).
fn precedence(entry: &Entry<'_>) -> (Strength, usize) {
    (entry.constraint.strength, entry.index)
}

/// Scan a validated set for same-form contradictions.
fn scan(entries: &[Entry<'_>]) -> (Vec<Dropped>, Option<Conflict>) {
    let mut by_form: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (index, entry) in entries.iter().enumerate() {
        for row in &entry.rows {
            by_form.entry(row.form()).or_default().push(index);
        }
    }

    let mut dropped: Vec<Dropped> = Vec::new();
    let mut conflicts: Vec<Conflict> = Vec::new();

    for (form, indices) in &by_form {
        if indices.len() < 2 {
            continue;
        }
        let mut order: Vec<usize> = indices.clone();
        order.sort_by_key(|i| precedence(&entries[*i]));
        let winner = &entries[order[0]];
        let winner_row = winner
            .rows
            .iter()
            .find(|row| row.form() == *form)
            .expect("form came from this entry");

        for loser_index in &order[1..] {
            let loser = &entries[*loser_index];
            if loser.constraint.id == winner.constraint.id {
                continue;
            }
            let loser_row = loser
                .rows
                .iter()
                .find(|row| row.form() == *form)
                .expect("form came from this entry");
            if loser_row.constant == winner_row.constant {
                // Same line, same offset: redundant, not contradictory.
                continue;
            }
            if winner.constraint.strength.is_required() && loser.constraint.strength.is_required() {
                let mut ids = vec![winner.constraint.id, loser.constraint.id];
                ids.sort();
                ids.dedup();
                conflicts.push(Conflict {
                    message: format!(
                        "required {} [{}] and required {} [{}] both pin the same row to different offsets ({} vs {})",
                        winner.constraint.kind.tag(),
                        describe_targets(&winner.constraint.targets),
                        loser.constraint.kind.tag(),
                        describe_targets(&loser.constraint.targets),
                        winner_row.constant,
                        loser_row.constant,
                    ),
                    ids,
                });
                continue;
            }
            dropped.push(Dropped {
                id: loser.constraint.id,
                kind: loser.constraint.kind,
                strength: loser.constraint.strength,
                form: form.clone(),
                kept: winner.constraint.id,
                kept_kind: winner.constraint.kind,
                kept_strength: winner.constraint.strength,
                kept_targets: describe_targets(&winner.constraint.targets),
            });
        }
    }

    let conflict = conflicts.into_iter().next();
    (dropped, conflict)
}

/// Validate the active registry and plan the solve.
///
/// A constraint that cannot be evaluated (deleted node loaded from an older
/// document, unresolvable value) is *skipped with a reason*, never fatal:
/// the solver must not stop enforcing everything else because one rule rotted.
pub fn plan(doc: &Document) -> SolvePlan {
    let mut entries: Vec<Entry<'_>> = Vec::new();
    let mut skipped: Vec<Skipped> = Vec::new();

    for (index, constraint) in doc.constraints.active().enumerate() {
        match validate(doc, constraint).and_then(|()| rows_for(constraint)) {
            Ok(rows) => entries.push(Entry {
                index,
                constraint,
                rows,
            }),
            Err(error) => skipped.push(Skipped {
                id: constraint.id,
                kind: constraint.kind,
                reason: error.to_string(),
            }),
        }
    }

    let (dropped, conflict) = scan(&entries);
    let dropped_ids: Vec<ConstraintId> = dropped.iter().map(|d| d.id).collect();
    let kept = entries
        .iter()
        .map(|entry| entry.constraint.id)
        .filter(|id| !dropped_ids.contains(id))
        .collect();

    SolvePlan {
        kept,
        dropped,
        skipped,
        conflict,
    }
}

/// Would activating `candidate` contradict a `Required` constraint already in
/// the document? (Soft conflicts are a solve-time decision, not an error.)
pub fn check_candidate(doc: &Document, candidate: &Constraint) -> Result<(), ConstraintError> {
    validate(doc, candidate)?;
    let candidate_rows = rows_for(candidate)?;

    let mut entries: Vec<Entry<'_>> = Vec::new();
    for (index, constraint) in doc.constraints.active().enumerate() {
        if constraint.id == candidate.id {
            continue;
        }
        if let Ok(rows) = validate(doc, constraint).and_then(|()| rows_for(constraint)) {
            entries.push(Entry {
                index,
                constraint,
                rows,
            });
        }
    }
    entries.push(Entry {
        index: doc.constraints.len(),
        constraint: candidate,
        rows: candidate_rows,
    });

    match scan(&entries).1 {
        Some(conflict) => Err(ConstraintError::Unsatisfiable {
            message: conflict.message,
        }),
        None => Ok(()),
    }
}

/// Pre-flight a command against the constraint system, *before* it is applied.
///
/// Only the two commands that can make the system harder can fail: adding a
/// constraint, and re-enabling one. Everything else (removal, disabling,
/// geometry edits) can only relax or is settled by the solve itself.
pub fn preflight(doc: &Document, cmd: &Command) -> Result<(), ConstraintError> {
    match cmd {
        Command::AddConstraint { constraint } => check_candidate(doc, constraint),
        Command::SetConstraintEnabled { id, enabled: true } => match doc.constraints.get(*id) {
            Some(existing) if !existing.enabled => {
                let armed = existing.clone().with_enabled(true);
                check_candidate(doc, &armed)
            }
            _ => Ok(()),
        },
        Command::Batch { commands } => {
            for cmd in commands {
                preflight(doc, cmd)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vectra_core::{new_constraint_id, new_node_id, ConstraintTarget, Node, NodeKind};

    fn two_rects() -> (Document, ConstraintTarget, ConstraintTarget) {
        let mut doc = Document::new();
        let a = new_node_id();
        let b = new_node_id();
        doc.insert_node(
            Node::new(a, "a", NodeKind::rectangle(0.0, 0.0, 10.0, 10.0)),
            None,
        )
        .unwrap();
        doc.insert_node(
            Node::new(b, "b", NodeKind::rectangle(300.0, 0.0, 10.0, 10.0)),
            None,
        )
        .unwrap();
        (
            doc,
            ConstraintTarget::new(a, "x"),
            ConstraintTarget::new(b, "x"),
        )
    }

    fn distance(
        a: &ConstraintTarget,
        b: &ConstraintTarget,
        value: f64,
        strength: Strength,
    ) -> Constraint {
        Constraint::new(
            new_constraint_id(),
            ConstraintKind::Distance,
            vec![a.clone(), b.clone()],
        )
        .with_value(value)
        .with_strength(strength)
    }

    #[test]
    fn weaker_row_is_dropped_and_the_stronger_holds() {
        let (mut doc, a, b) = two_rects();
        let strong = distance(&a, &b, 100.0, Strength::Medium);
        let weak = distance(&a, &b, 200.0, Strength::Weak);
        doc.constraints.insert(strong.clone()).unwrap();
        doc.constraints.insert(weak.clone()).unwrap();

        let plan = plan(&doc);
        assert_eq!(plan.kept, vec![strong.id]);
        assert_eq!(plan.dropped.len(), 1);
        assert_eq!(plan.dropped[0].id, weak.id);
        assert_eq!(plan.dropped[0].strength, Strength::Weak);
        assert_eq!(plan.dropped[0].kept, strong.id);
        assert!(plan.conflict.is_none());
    }

    #[test]
    fn equal_strengths_drop_the_newer_constraint() {
        let (mut doc, a, b) = two_rects();
        let first = distance(&a, &b, 100.0, Strength::Medium);
        let second = distance(&a, &b, 200.0, Strength::Medium);
        doc.constraints.insert(first.clone()).unwrap();
        doc.constraints.insert(second.clone()).unwrap();

        let plan = plan(&doc);
        assert_eq!(plan.kept, vec![first.id]);
        assert_eq!(
            plan.dropped[0].id, second.id,
            "the newer constraint loses a tie"
        );
    }

    #[test]
    fn required_conflict_is_a_hard_error_and_never_dropped() {
        let (mut doc, a, b) = two_rects();
        let first = distance(&a, &b, 100.0, Strength::Required);
        let second = distance(&a, &b, 200.0, Strength::Required);
        doc.constraints.insert(first.clone()).unwrap();

        assert!(matches!(
            check_candidate(&doc, &second),
            Err(ConstraintError::Unsatisfiable { .. })
        ));

        // …and the same pair reaches the same verdict through the plan.
        doc.constraints.insert(second).unwrap();
        let plan = plan(&doc);
        assert!(plan.conflict.is_some(), "required/required is a conflict");
        assert!(plan.dropped.is_empty(), "required rows are never dropped");
    }

    #[test]
    fn required_beats_weaker_without_erroring() {
        let (mut doc, a, b) = two_rects();
        doc.constraints
            .insert(distance(&a, &b, 100.0, Strength::Required))
            .unwrap();
        let medium = distance(&a, &b, 200.0, Strength::Medium);
        doc.constraints.insert(medium.clone()).unwrap();

        let plan = plan(&doc);
        assert!(plan.conflict.is_none());
        assert_eq!(plan.dropped.len(), 1);
        assert_eq!(plan.dropped[0].id, medium.id);
    }

    #[test]
    fn redundant_rows_are_kept_and_mirrored_rows_are_recognised() {
        let (mut doc, a, b) = two_rects();
        let first = distance(&a, &b, 100.0, Strength::Medium);
        let redundant = distance(&a, &b, 100.0, Strength::Weak);
        let mirrored = Constraint::new(
            new_constraint_id(),
            ConstraintKind::Distance,
            vec![b.clone(), a.clone()],
        )
        .with_value(-100.0)
        .with_strength(Strength::Weak);
        doc.constraints.insert(first.clone()).unwrap();
        doc.constraints.insert(redundant.clone()).unwrap();
        doc.constraints.insert(mirrored.clone()).unwrap();

        let plan = plan(&doc);
        assert!(
            plan.dropped.is_empty(),
            "same line + same offset is redundancy"
        );
        assert_eq!(plan.kept.len(), 3);
    }

    #[test]
    fn cross_kind_contradictions_are_caught() {
        let (mut doc, a, b) = two_rects();
        let vertical = Constraint::new(
            new_constraint_id(),
            ConstraintKind::Vertical,
            vec![a.clone(), b.clone()],
        )
        .with_strength(Strength::Medium);
        doc.constraints.insert(vertical.clone()).unwrap();
        let contradictory = distance(&a, &b, 100.0, Strength::Weak);
        doc.constraints.insert(contradictory.clone()).unwrap();

        let plan = plan(&doc);
        assert_eq!(plan.kept, vec![vertical.id]);
        assert_eq!(plan.dropped[0].id, contradictory.id);
    }

    #[test]
    fn invalid_constraints_are_skipped_with_a_reason_not_fatal() {
        let (mut doc, a, _b) = two_rects();
        let good = Constraint::new(new_constraint_id(), ConstraintKind::Angle, vec![a.clone()])
            .with_value(1.0);
        doc.constraints.insert(good.clone()).unwrap();
        // A rule whose node no longer exists (e.g. loaded from an old document).
        let rotten = Constraint::new(
            new_constraint_id(),
            ConstraintKind::Angle,
            vec![ConstraintTarget::new(new_node_id(), "x")],
        )
        .with_value(2.0);
        doc.constraints.insert(rotten.clone()).unwrap();

        let plan = plan(&doc);
        assert_eq!(plan.kept, vec![good.id]);
        assert_eq!(plan.skipped.len(), 1);
        assert_eq!(plan.skipped[0].id, rotten.id);
        assert!(!plan.skipped[0].reason.is_empty());
    }

    #[test]
    fn preflight_rejects_bad_targets_before_anything_is_applied() {
        let (doc, a, _b) = two_rects();
        let bad = Constraint::new(
            new_constraint_id(),
            ConstraintKind::Vertical,
            vec![a, ConstraintTarget::new(new_node_id(), "x")],
        );
        let cmd = Command::AddConstraint { constraint: bad };
        assert!(matches!(
            preflight(&doc, &cmd),
            Err(ConstraintError::Document(_))
        ));
    }
}
