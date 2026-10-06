//! Linear rows: a [`Constraint`] compiled into the equalities the tableau sees.
//!
//! Every Phase-1 kind is an **equality** over float slots with coefficients in
//! `{−1, 0, +1}` and a constant on the right-hand side, which is exactly what
//! Cassowary wants (`Expression + constant == 0`).
//!
//! A [`Row`] is *canonical*: terms are keyed by target label, duplicates are
//! summed, and the whole row is negated when needed so the coefficient of the
//! first (label-sorted) term is positive. Two rows over the same slots then
//! have the same [`Row::form`] exactly when they describe the same line — and
//! comparing their constants tells the solver whether they contradict.
//!
//! Property names are validated against the node **here**, before anything
//! reaches the solver, so a rule naming a slot that does not exist (or is not
//! a float) is a typed error instead of a silently ignored row.

use vectra_core::{
    Constraint, ConstraintKind, ConstraintTarget, Document, EvaluationContext, NodeId, ParamValue,
    Resolvable, VectraError,
};

use crate::ConstraintError;

/// One equality: `Σ coefficient × slot + constant == 0`.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    /// (slot, coefficient) pairs, label-sorted, duplicates summed.
    pub terms: Vec<(ConstraintTarget, f64)>,
    /// Right-hand side, normalized together with the coefficients.
    pub constant: f64,
}

impl Row {
    /// Build a canonical row: sum duplicate slots, sort by label, and make the
    /// leading coefficient positive (multiplying the equality by −1 leaves it
    /// unchanged).
    pub fn new(terms: impl IntoIterator<Item = (ConstraintTarget, f64)>, constant: f64) -> Self {
        let mut merged: Vec<(ConstraintTarget, f64)> = Vec::new();
        for (target, coefficient) in terms {
            match merged.iter_mut().find(|(t, _)| *t == target) {
                Some((_, existing)) => *existing += coefficient,
                None => merged.push((target, coefficient)),
            }
        }
        merged.retain(|(_, coefficient)| *coefficient != 0.0);
        merged.sort_by_key(|(target, _)| target.label());
        let mut constant = constant;
        if merged.first().map(|(_, k)| *k < 0.0).unwrap_or(false) {
            for (_, coefficient) in merged.iter_mut() {
                *coefficient = -*coefficient;
            }
            constant = -constant;
        }
        Self {
            terms: merged,
            constant,
        }
    }

    /// Canonical identity of this row's *linear form* — the slots and
    /// coefficients, without the constant. Same form + different constant ⇒
    /// the two rows contradict each other, which is what the drop pre-pass
    /// detects.
    pub fn form(&self) -> String {
        self.terms
            .iter()
            .map(|(target, coefficient)| format!("{coefficient}*{}", target.label()))
            .collect::<Vec<_>>()
            .join("+")
    }

    /// A row with no slots (`0 == constant`) can never have come from a valid
    /// constraint; it is rejected as degenerate rather than handed to the
    /// solver.
    pub fn is_degenerate(&self) -> bool {
        self.terms.is_empty()
    }
}

/// The float slots a constraint addresses, deduplicated, in canonical order.
pub fn targets_of(constraint: &Constraint) -> Vec<ConstraintTarget> {
    let mut out: Vec<ConstraintTarget> = Vec::new();
    for target in &constraint.targets {
        if !out.contains(target) {
            out.push(target.clone());
        }
    }
    out
}

/// Does this node expose `property` as a float slot?
///
/// Uses the same accessor the evaluator and the dependency graph use, so the
/// vocabulary cannot drift.
pub fn is_float_slot(doc: &Document, target: &ConstraintTarget) -> Result<(), ConstraintError> {
    let node = doc.get_node(target.node_id)?;
    match node.get_param(&target.property) {
        Ok(ParamValue::Float(_)) => Ok(()),
        Ok(other) => Err(VectraError::PropertyTypeMismatch {
            property: target.property.clone(),
            expected: "float",
            got: other.kind(),
        }
        .into()),
        Err(e) => Err(e.into()),
    }
}

/// Kinds whose operand may be captured from the document at add time
/// (`value == None` means "as it is right now").
pub fn captures_value(kind: ConstraintKind) -> bool {
    matches!(
        kind,
        ConstraintKind::Distance | ConstraintKind::Parallel | ConstraintKind::Angle
    )
}

/// Validate arity, operand and target slots. Runs before every solve *and*
/// before an `AddConstraint` is applied, which is what lets a bad rule be
/// rejected without touching the document.
pub fn validate(doc: &Document, constraint: &Constraint) -> Result<(), ConstraintError> {
    let expected = constraint.kind.arity();
    if constraint.targets.len() != expected {
        return Err(ConstraintError::Arity {
            id: constraint.id,
            kind: constraint.kind,
            expected,
            got: constraint.targets.len(),
        });
    }
    if constraint.kind.takes_value()
        && constraint.value.is_none()
        && !captures_value(constraint.kind)
    {
        return Err(ConstraintError::MissingValue {
            id: constraint.id,
            kind: constraint.kind,
        });
    }
    if let Some(value) = constraint.value {
        if !value.is_finite() {
            return Err(ConstraintError::NonFinite {
                id: constraint.id,
                value,
            });
        }
    }
    for target in &constraint.targets {
        is_float_slot(doc, target)?;
    }
    Ok(())
}

/// Compile a validated constraint into rows (MES §9 operator table).
pub fn rows_for(constraint: &Constraint) -> Result<Vec<Row>, ConstraintError> {
    let id = constraint.id;
    let kind = constraint.kind;
    let targets = &constraint.targets;
    let value = constraint.value.unwrap_or(0.0);

    let rows = match kind {
        // a == b
        ConstraintKind::Coincident
        | ConstraintKind::Horizontal
        | ConstraintKind::Vertical
        | ConstraintKind::EqualLength => vec![Row::new(
            [(targets[0].clone(), 1.0), (targets[1].clone(), -1.0)],
            value,
        )],
        // a - b == value (a separation, preserved or requested)
        ConstraintKind::Distance | ConstraintKind::Parallel => vec![Row::new(
            [(targets[0].clone(), 1.0), (targets[1].clone(), -1.0)],
            value,
        )],
        // (a - b) - (c - d) == 0 — slope-1 diagonals stay orthogonal
        ConstraintKind::Perpendicular => {
            if targets.len() != 4 {
                return Err(ConstraintError::Arity {
                    id,
                    kind,
                    expected: 4,
                    got: targets.len(),
                });
            }
            vec![Row::new(
                [
                    (targets[0].clone(), 1.0),
                    (targets[1].clone(), -1.0),
                    (targets[2].clone(), -1.0),
                    (targets[3].clone(), 1.0),
                ],
                value,
            )]
        }
        // a == value
        ConstraintKind::Angle => vec![Row::new([(targets[0].clone(), 1.0)], value)],
    };

    if let Some(degenerate) = rows.iter().find(|row| row.is_degenerate()) {
        let _ = degenerate;
        return Err(ConstraintError::Degenerate { id, kind });
    }
    Ok(rows)
}

/// The value that makes `constraint` hold *as the document currently is* —
/// used when an operand-capturing kind is added without one. Read through
/// [`vectra_core::Resolvable`], the same path the evaluator uses.
pub fn capture_value(
    doc: &Document,
    ctx: &EvaluationContext<'_>,
    constraint: &Constraint,
) -> Result<f64, ConstraintError> {
    let targets = &constraint.targets;
    let read = |target: &ConstraintTarget| -> Result<f64, ConstraintError> {
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
    };
    match constraint.kind {
        ConstraintKind::Distance | ConstraintKind::Parallel => {
            Ok(read(&targets[0])? - read(&targets[1])?)
        }
        ConstraintKind::Angle => read(&targets[0]),
        _ => Ok(0.0),
    }
}

/// Human-readable target list (`"a.x ↔ b.cx"`), for diagnostics.
pub fn describe_targets(targets: &[ConstraintTarget]) -> String {
    targets
        .iter()
        .map(ConstraintTarget::label)
        .collect::<Vec<_>>()
        .join(" ↔ ")
}

/// Every property the registry currently targets on `node_id`.
pub fn properties_targeting(doc: &Document, node_id: NodeId) -> Vec<String> {
    let mut out = Vec::new();
    for constraint in doc.constraints.iter() {
        for target in &constraint.targets {
            if target.node_id == node_id && !out.contains(&target.property) {
                out.push(target.property.clone());
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use vectra_core::{new_constraint_id, new_node_id, ConstraintTarget, Node, NodeKind};

    fn rect(doc: &mut Document) -> ConstraintTarget {
        let id = new_node_id();
        doc.insert_node(
            Node::new(id, "r", NodeKind::rectangle(0.0, 0.0, 10.0, 10.0)),
            None,
        )
        .unwrap();
        ConstraintTarget::new(id, "width")
    }

    #[test]
    fn rows_are_unit_coefficient_equalties() {
        let mut doc = Document::new();
        let a = rect(&mut doc);
        let b = rect(&mut doc);

        let vertical = Constraint::new(
            new_constraint_id(),
            ConstraintKind::Vertical,
            vec![a.clone(), b.clone()],
        );
        validate(&doc, &vertical).unwrap();
        let rows = rows_for(&vertical).unwrap();
        assert_eq!(rows.len(), 1);
        // `Row::new` canonically sorts by target label — and a label begins
        // with the node's (random) uuid, so the two terms may come out in
        // either order, with the whole row negated to keep the leading
        // coefficient positive. Compare against the canonical row instead of a
        // positional list (a positional assertion here passes ~half the time).
        let expected = Row::new([(a.clone(), 1.0), (b.clone(), -1.0)], 0.0);
        assert_eq!(rows[0], expected);
        assert_eq!(rows[0].constant, 0.0);
        // Unit coefficients of opposite sign: `a - b == 0`.
        assert_eq!(rows[0].terms.len(), 2);
        let magnitudes: Vec<f64> = rows[0].terms.iter().map(|(_, k)| k.abs()).collect();
        assert!(
            magnitudes.iter().all(|m| *m == 1.0),
            "unit coefficients: {:?}",
            rows[0].terms
        );
        assert_eq!(rows[0].terms[0].1, -rows[0].terms[1].1);

        let distance = Constraint::new(
            new_constraint_id(),
            ConstraintKind::Distance,
            vec![a.clone(), b.clone()],
        )
        .with_value(100.0);
        validate(&doc, &distance).unwrap();
        // Same canonicalization: the operand travels with `a - b` in whichever
        // orientation the label sort chose, so the sign is not fixed.
        let row = &rows_for(&distance).unwrap()[0];
        assert_eq!(*row, Row::new([(a.clone(), 1.0), (b.clone(), -1.0)], 100.0));
        assert_eq!(
            row.constant.abs(),
            100.0,
            "the operand survives canonicalization: {row:?}"
        );
    }

    #[test]
    fn mirrored_rows_normalize_to_the_same_form() {
        let mut doc = Document::new();
        let a = rect(&mut doc);
        let b = rect(&mut doc);
        // a - b == 100  vs  b - a == 200  ⟺  a - b == -200: the same line.
        let one = Constraint::new(
            new_constraint_id(),
            ConstraintKind::Distance,
            vec![a.clone(), b.clone()],
        )
        .with_value(100.0);
        let two = Constraint::new(
            new_constraint_id(),
            ConstraintKind::Distance,
            vec![b.clone(), a.clone()],
        )
        .with_value(200.0);
        let rows_one = rows_for(&one).unwrap();
        let rows_two = rows_for(&two).unwrap();
        // Both normalize to the same line, so their constants are comparable —
        // and here they disagree. (Which sign the canonical row ends up with
        // depends on label order; only the *comparison* is meaningful.)
        assert_eq!(rows_one[0].form(), rows_two[0].form());
        assert_ne!(rows_one[0].constant, rows_two[0].constant);

        // The same line with the SAME offset (`b - a == -100` ⟺ `a - b == 100`)
        // is redundant, not contradictory.
        let mirror = Constraint::new(new_constraint_id(), ConstraintKind::Distance, vec![b, a])
            .with_value(-100.0);
        let rows_mirror = rows_for(&mirror).unwrap();
        assert_eq!(rows_one[0].form(), rows_mirror[0].form());
        assert_eq!(rows_one[0].constant, rows_mirror[0].constant);
    }

    #[test]
    fn perpendicular_uses_four_targets() {
        let mut doc = Document::new();
        let a = rect(&mut doc);
        let b = rect(&mut doc);
        let a_y = ConstraintTarget::new(a.node_id, "x");
        let b_y = ConstraintTarget::new(b.node_id, "x");
        let c = Constraint::new(
            new_constraint_id(),
            ConstraintKind::Perpendicular,
            vec![a, a_y, b, b_y],
        );
        validate(&doc, &c).unwrap();
        let rows = rows_for(&c).unwrap();
        let mut coefficients: Vec<i64> = rows[0].terms.iter().map(|(_, k)| *k as i64).collect();
        coefficients.sort_unstable();
        // (a - b) - (c - d): two +1 and two −1, in canonical (label-sorted)
        // order, which is not the order they were supplied in.
        assert_eq!(coefficients, vec![-1, -1, 1, 1]);
        assert_eq!(rows[0].terms.len(), 4);
    }

    #[test]
    fn unknown_and_non_float_slots_are_typed_errors() {
        let mut doc = Document::new();
        let a = rect(&mut doc);
        let unknown = Constraint::new(
            new_constraint_id(),
            ConstraintKind::Vertical,
            vec![a.clone(), ConstraintTarget::new(a.node_id, "nope")],
        );
        assert!(matches!(
            validate(&doc, &unknown),
            Err(ConstraintError::Document(
                VectraError::UnknownProperty { .. }
            ))
        ));

        let missing_node = Constraint::new(
            new_constraint_id(),
            ConstraintKind::Vertical,
            vec![a.clone(), ConstraintTarget::new(new_node_id(), "x")],
        );
        assert!(matches!(
            validate(&doc, &missing_node),
            Err(ConstraintError::Document(VectraError::NodeNotFound(_)))
        ));
    }

    #[test]
    fn arity_and_value_are_checked() {
        let mut doc = Document::new();
        let a = rect(&mut doc);

        let short = Constraint::new(
            new_constraint_id(),
            ConstraintKind::Vertical,
            vec![a.clone()],
        );
        assert!(matches!(
            validate(&doc, &short),
            Err(ConstraintError::Arity {
                expected: 2,
                got: 1,
                ..
            })
        ));

        // Angle captures its operand, so a missing value is legal…
        let no_value = Constraint::new(new_constraint_id(), ConstraintKind::Angle, vec![a.clone()]);
        assert!(validate(&doc, &no_value).is_ok());

        // …but a non-finite one is not.
        let infinite = Constraint::new(new_constraint_id(), ConstraintKind::Angle, vec![a])
            .with_value(f64::INFINITY);
        assert!(matches!(
            validate(&doc, &infinite),
            Err(ConstraintError::NonFinite { .. })
        ));
    }
}
