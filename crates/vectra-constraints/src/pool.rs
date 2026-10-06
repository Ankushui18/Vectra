//! The Cassowary variable pool: one solver variable per addressed slot.
//!
//! Slots are interned by their [`ConstraintTarget`] label
//! (`"<node uuid>.width"`). Interning is stable for the life of the solver, so
//! a slot keeps the same [`Variable`] across solves — and
//! [`VariablePool::gc`] releases the ones no active constraint references any
//! more. That release is what makes the Undo law's *"Cassowary's internal
//! variable count decreases"* observable: undo the constraint that introduced
//! the slots, and the pool shrinks back.

use std::collections::{BTreeMap, BTreeSet};

use cassowary::Variable;
use vectra_core::ConstraintTarget;

/// Stable slot → variable map with explicit garbage collection.
#[derive(Debug, Default)]
pub struct VariablePool {
    vars: BTreeMap<String, Variable>,
}

impl VariablePool {
    /// An empty pool.
    pub fn new() -> Self {
        Self::default()
    }

    /// The variable for `target`, allocating one on first use.
    pub fn intern(&mut self, target: &ConstraintTarget) -> Variable {
        let label = target.label();
        match self.vars.get(&label) {
            Some(variable) => *variable,
            None => {
                let variable = Variable::new();
                self.vars.insert(label, variable);
                variable
            }
        }
    }

    /// The variable already allocated for `label`, if any.
    pub fn get(&self, label: &str) -> Option<Variable> {
        self.vars.get(label).copied()
    }

    /// Release every variable whose slot is not in `keep`.
    ///
    /// Returns the labels that were released (diagnostics/tests).
    pub fn gc(&mut self, keep: &BTreeSet<String>) -> Vec<String> {
        let stale: Vec<String> = self
            .vars
            .keys()
            .filter(|label| !keep.contains(*label))
            .cloned()
            .collect();
        for label in &stale {
            self.vars.remove(label);
        }
        stale
    }

    /// Number of interned slots — the "internal variable count".
    pub fn len(&self) -> usize {
        self.vars.len()
    }

    /// True when no slot is interned.
    pub fn is_empty(&self) -> bool {
        self.vars.is_empty()
    }

    /// Interned labels, in canonical order.
    pub fn labels(&self) -> Vec<String> {
        self.vars.keys().cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vectra_core::new_node_id;

    #[test]
    fn interning_is_stable_and_gc_releases_only_the_unreferenced() {
        let mut pool = VariablePool::new();
        let a = ConstraintTarget::new(new_node_id(), "x");
        let b = ConstraintTarget::new(new_node_id(), "y");

        let va = pool.intern(&a);
        let vb = pool.intern(&b);
        assert_eq!(
            pool.intern(&a),
            va,
            "interning twice returns the same variable"
        );
        assert_eq!(pool.len(), 2);
        assert_ne!(va, vb);

        let keep: BTreeSet<String> = [a.label()].into_iter().collect();
        let released = pool.gc(&keep);
        assert_eq!(released, vec![b.label()]);
        assert_eq!(pool.len(), 1, "the unreferenced slot was released");
        assert_ne!(
            pool.intern(&b),
            vb,
            "…and re-interning after a release allocates a fresh variable"
        );
        assert_eq!(pool.len(), 2);
    }
}
