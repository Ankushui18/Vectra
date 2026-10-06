//! Property-based guarantees for event-sourced undo/redo (MES §14, Task 1.5).
//!
//! Laws: inverses are exact, failing commands are stack-neutral, redo is
//! cleared by new work, and undo-all/redo-all round-trips the snapshot.

use proptest::prelude::*;
use vectra_core::{Command, Engine, NodeKind, ParamValue, Parameter, VectraError};

// ── Strategies ─────────────────────────────────────────────────────────

fn var_name() -> impl Strategy<Value = String> {
    "[a-z][a-z0-9_]{0,7}".prop_map(|s| s.to_string())
}

fn finite_f64() -> impl Strategy<Value = f64> {
    (-1e6..1e6f64).prop_filter("finite", |v| v.is_finite())
}

fn rect_kind() -> impl Strategy<Value = NodeKind> {
    (finite_f64(), finite_f64(), 1.0..2000.0f64, 1.0..2000.0f64)
        .prop_map(|(x, y, w, h)| NodeKind::rectangle(x, y, w, h))
}

fn circle_kind() -> impl Strategy<Value = NodeKind> {
    (finite_f64(), finite_f64(), 1.0..2000.0f64).prop_map(|(cx, cy, r)| NodeKind::circle(cx, cy, r))
}

fn any_scalar_kind() -> impl Strategy<Value = NodeKind> {
    prop_oneof![rect_kind(), circle_kind(),]
}

/// Float properties valid for the generated kind.
fn float_prop_for(kind: &NodeKind) -> Vec<&'static str> {
    match kind {
        NodeKind::Rectangle { .. } => vec!["x", "y", "width", "height", "corner_radius"],
        NodeKind::Circle { .. } => vec!["cx", "cy", "radius"],
        _ => vec!["opacity"], // style props exist on every node
    }
}

proptest! {
    /// LAW 1 — Create/undo/remove/redo restores the exact node (id + kind + name).
    #[test]
    fn create_undo_redo_is_identity(
        kind in any_scalar_kind(),
        name in "[A-Z][a-z]{0,9}",
    ) {
        let mut engine = Engine::new();
        let id = vectra_core::new_node_id();
        engine.dispatch(Command::CreateNode {
            id, kind: kind.clone(), name: Some(name.clone()), index: None,
        }).unwrap();

        engine.undo().unwrap();
        assert!(engine.document().get_node(id).is_err(), "undo of create must remove");

        engine.redo().unwrap();
        let node = engine.document().get_node(id).unwrap();
        assert_eq!(node.id, id);
        assert_eq!(node.kind, kind);
        assert_eq!(node.name, name);
    }

    /// LAW 2 — SetParameter/undo restores the previous value bit-for-bit.
    #[test]
    fn set_parameter_undo_restores_previous(
        kind in any_scalar_kind(),
        prop_idx in 0..3usize,
        first in finite_f64(),
        second_name in var_name(),
    ) {
        let mut engine = Engine::new();
        let id = vectra_core::new_node_id();
        engine.dispatch(Command::CreateNode {
            id, kind: kind.clone(), name: None, index: None,
        }).unwrap();

        let props = float_prop_for(&kind);
        let prop = props[prop_idx % props.len()];
        let before = engine.document().get_node(id).unwrap().get_param(prop).unwrap();

        // Literal write then variable rebind, then unwind both.
        engine.dispatch(Command::SetParameter {
            node_id: id, property: prop.to_string(),
            value: ParamValue::Float(Parameter::Literal(first)),
        }).unwrap();
        engine.dispatch(Command::SetVariable { name: second_name.clone(), value: 5.0 }).unwrap();
        engine.dispatch(Command::SetParameter {
            node_id: id, property: prop.to_string(),
            value: ParamValue::Float(Parameter::variable(&second_name)),
        }).unwrap();

        engine.undo().unwrap(); // back to literal
        assert_eq!(
            engine.document().get_node(id).unwrap().get_param(prop).unwrap(),
            ParamValue::Float(Parameter::Literal(first))
        );
        engine.undo().unwrap(); // drop variable
        engine.undo().unwrap(); // back to creation value
        assert_eq!(
            engine.document().get_node(id).unwrap().get_param(prop).unwrap(),
            before
        );
    }

    /// LAW 3 — Failing commands are stack-neutral (never poison undo).
    #[test]
    fn failing_commands_are_stack_neutral(
        existing in 0..4usize,
        bogus_prop in "[a-z]{1,10}",
    ) {
        let mut engine = Engine::new();
        for _ in 0..existing {
            let id = vectra_core::new_node_id();
            engine.dispatch(Command::CreateNode {
                id, kind: NodeKind::rectangle(0.0, 0.0, 10.0, 10.0),
                name: None, index: None,
            }).unwrap();
        }
        let snapshot_before = engine.snapshot_json();
        let can_undo_before = engine.can_undo();

        // Three distinct failure modes.
        let missing_node = vectra_core::new_node_id();
        assert!(matches!(
            engine.dispatch(Command::SetParameter {
                node_id: missing_node,
                property: "width".to_string(),
                value: ParamValue::float_literal(1.0),
            }),
            Err(VectraError::NodeNotFound(_))
        ));
        assert!(matches!(
            engine.dispatch(Command::DeleteNode { id: missing_node }),
            Err(VectraError::NodeNotFound(_))
        ));
        // Unknown property on a real node (if any exist).
        if let Some(first) = engine.document().order.first().copied() {
            let prop = format!("no_such_prop_{bogus_prop}");
            assert!(matches!(
                engine.dispatch(Command::SetParameter {
                    node_id: first,
                    property: prop,
                    value: ParamValue::float_literal(1.0),
                }),
                Err(VectraError::UnknownProperty { .. })
            ));
        }

        assert_eq!(engine.snapshot_json(), snapshot_before, "failed commands must not mutate");
        assert_eq!(engine.can_undo(), can_undo_before, "failed commands must not touch the stack");
        assert!(!engine.can_redo());
    }

    /// LAW 4 — New work clears redo (no branching timelines in Phase 1).
    #[test]
    fn new_work_clears_redo(a in var_name(), b in var_name()) {
        prop_assume!(a != b);
        let mut engine = Engine::new();
        engine.dispatch(Command::SetVariable { name: a.clone(), value: 1.0 }).unwrap();
        engine.dispatch(Command::SetVariable { name: b.clone(), value: 2.0 }).unwrap();
        engine.undo().unwrap();
        assert!(engine.can_redo());
        engine.dispatch(Command::SetVariable { name: b.clone(), value: 3.0 }).unwrap();
        assert!(!engine.can_redo(), "executing after undo must clear redo");
        assert_eq!(engine.get_variable(&b), Some(3.0));
    }

    /// LAW 5 — Undo-all then redo-all round-trips the snapshot exactly.
    #[test]
    fn undo_all_redo_all_roundtrips_snapshot(
        writes in prop::collection::vec((var_name(), finite_f64()), 1..12),
    ) {
        let mut engine = Engine::new();
        // Deduplicate names so every write is deterministic (no HashMap-order flake).
        let mut seen = std::collections::HashSet::new();
        let mut applied = 0usize;
        for (name, value) in &writes {
            if seen.insert(name.clone()) {
                engine.dispatch(Command::SetVariable { name: name.clone(), value: *value }).unwrap();
                applied += 1;
            }
        }
        // Compare semantically (serde_json::Value object equality is
        // order-insensitive): HashMap reinsertion order after undo/redo is
        // intentionally unspecified, but the document content must round-trip.
        let head: serde_json::Value = serde_json::from_str(&engine.snapshot_json()).unwrap();
        for _ in 0..applied {
            engine.undo().unwrap();
        }
        assert!(!engine.can_undo());
        for _ in 0..applied {
            engine.redo().unwrap();
        }
        let restored: serde_json::Value =
            serde_json::from_str(&engine.snapshot_json()).unwrap();
        assert_eq!(restored, head, "redo-all must restore head snapshot");
    }

    /// LAW 6 — Variable undo restores the *previous* value, and undo of a
    /// first-time set removes the variable entirely.
    #[test]
    fn variable_undo_restores_prior_or_removes(
        name in var_name(), v1 in finite_f64(), v2 in finite_f64(),
    ) {
        let mut engine = Engine::new();
        engine.dispatch(Command::SetVariable { name: name.clone(), value: v1 }).unwrap();
        engine.dispatch(Command::SetVariable { name: name.clone(), value: v2 }).unwrap();
        assert_eq!(engine.get_variable(&name), Some(v2));

        engine.undo().unwrap();
        assert_eq!(engine.get_variable(&name), Some(v1));

        engine.undo().unwrap();
        assert_eq!(engine.get_variable(&name), None, "undo of first set must remove");
    }
}
