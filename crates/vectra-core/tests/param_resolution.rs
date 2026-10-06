//! Property-based guarantees for `Parameter` resolution (MES §17, Task 1.5).
//!
//! These tests assert the *laws* of the parametric system, not examples:
//! literals are identity, variables are lookups, fallbacks are total, and
//! every failure mode is a typed `ResolveError` — never a panic, never silent.

use proptest::prelude::*;
use std::collections::HashMap;
use vectra_core::{EvaluationContext, MotionBinding, Parameter, Point2, Resolvable, ResolveError};

// ── Strategies ─────────────────────────────────────────────────────────

/// Finite f64s only: NaN/Infinity would make equality meaningless.
fn finite_f64() -> impl Strategy<Value = f64> {
    (-1e6..1e6f64).prop_filter("finite", |v| v.is_finite())
}

/// Valid variable names (MES: non-empty, no whitespace).
fn var_name() -> impl Strategy<Value = String> {
    "[a-z][a-z0-9_]{0,7}".prop_map(|s| s.to_string())
}

fn point2() -> impl Strategy<Value = Point2> {
    (finite_f64(), finite_f64()).prop_map(|(x, y)| Point2::new(x, y))
}

fn ctx_for(vars: &HashMap<String, f64>) -> EvaluationContext<'_> {
    EvaluationContext::new(vars, 0.0)
}

fn assert_f64_eq(a: f64, b: f64) {
    assert!(
        a.to_bits() == b.to_bits(),
        "float mismatch: {a} (!=) {b} (bits {:x} vs {:x})",
        a.to_bits(),
        b.to_bits()
    );
}

// ── Laws ───────────────────────────────────────────────────────────────

proptest! {
    /// LAW 1 — Literal is identity: resolving a literal returns its exact bits.
    #[test]
    fn literal_is_identity(v in finite_f64()) {
        let vars = HashMap::new();
        let ctx = ctx_for(&vars);
        let out: f64 = Parameter::Literal(v).resolve(&ctx).unwrap();
        assert_f64_eq(out, v);
    }

    /// LAW 2 — Variable is a pure lookup: bound name resolves, unbound errors typed.
    #[test]
    fn variable_is_lookup(
        entries in prop::collection::hash_map(var_name(), finite_f64(), 0..8),
        probe in var_name(),
    ) {
        let ctx = ctx_for(&entries);
        let result: Result<f64, _> = Parameter::variable(&probe).resolve(&ctx);
        match entries.get(&probe) {
            Some(expected) => assert_f64_eq(result.unwrap(), *expected),
            None => assert!(
                matches!(&result, Err(ResolveError::UndefinedVariable(n)) if n == &probe),
                "unbound variable must raise UndefinedVariable, got {result:?}"
            ),
        }
    }

    /// LAW 3 — Rebinding a variable is observable through every alias.
    /// (The dependency-graph dirty walk in MES §8 exists to make this cheap;
    ///  this law pins the *semantics* the graph must preserve.)
    #[test]
    fn variable_rebind_is_observable(
        name in var_name(),
        before in finite_f64(),
        after in finite_f64(),
    ) {
        let mut vars = HashMap::new();
        vars.insert(name.clone(), before);
        let p: Parameter<f64> = Parameter::variable(&name);
        assert_f64_eq(p.resolve(&ctx_for(&vars)).unwrap(), before);

        vars.insert(name.clone(), after);
        assert_f64_eq(p.resolve(&ctx_for(&vars)).unwrap(), after);
    }

    /// LAW 4 — Spring static-preview fallback resolves its target exactly
    /// (no motion crate wired).
    #[test]
    fn spring_fallback_is_target(v in finite_f64(), k in finite_f64(), d in finite_f64()) {
        let vars = HashMap::new();
        let ctx = ctx_for(&vars);
        let p: Parameter<f64> = Parameter::Animated(MotionBinding::spring(
            Parameter::Literal(v),
            k,
            d,
            0.0,
            0.0,
        ));
        assert_f64_eq(p.resolve(&ctx).unwrap(), v);
    }

    /// LAW 5 — StateDriven static-preview fallback resolves the false arm.
    #[test]
    fn state_driven_fallback_is_false_arm(t in finite_f64(), f in finite_f64()) {
        let vars = HashMap::new();
        let ctx = ctx_for(&vars);
        let p: Parameter<f64> = Parameter::Animated(MotionBinding::StateDriven {
            state: "hover".to_string(),
            true_value: Box::new(Parameter::Literal(t)),
            false_value: Box::new(Parameter::Literal(f)),
        });
        assert_f64_eq(p.resolve(&ctx).unwrap(), f);
    }

    /// LAW 6 — Keyframe tracks have no static meaning: typed error, never a guess.
    #[test]
    fn keyframe_without_evaluator_is_typed_error(track in var_name()) {
        let vars = HashMap::new();
        let ctx = ctx_for(&vars);
        let p: Parameter<f64> = Parameter::Animated(MotionBinding::KeyframeTrack {
            track_id: track,
            property: "x".to_string(),
        });
        assert!(
            matches!(p.resolve(&ctx), Err(ResolveError::MissingMotionEvaluator)),
            "keyframe with no evaluator must error"
        );
    }

    /// LAW 7 — Expressions without a compiler wired are typed errors carrying the ID.
    #[test]
    fn expression_without_evaluator_carries_id(_seed in any::<u64>()) {
        let vars = HashMap::new();
        let ctx = ctx_for(&vars);
        let id = vectra_core::new_expression_id();
        let p: Parameter<f64> = Parameter::expression(id);
        assert!(
            matches!(p.resolve(&ctx), Err(ResolveError::MissingExpressionEvaluator(e)) if e == id)
        );
    }

    /// LAW 8 — Point literals are identity; scalar variables never silently
    /// coerce to points.
    #[test]
    fn point_literal_is_identity(p in point2()) {
        let vars = HashMap::new();
        let ctx = ctx_for(&vars);
        let out: Point2 = Parameter::Literal(p).resolve(&ctx).unwrap();
        assert_eq!(out, p);
    }

    #[test]
    fn point_variable_is_rejected(name in var_name(), v in finite_f64()) {
        let mut vars = HashMap::new();
        vars.insert(name.clone(), v);
        let ctx = ctx_for(&vars);
        let result: Result<Point2, _> = Parameter::variable(&name).resolve(&ctx);
        assert!(
            matches!(&result, Err(ResolveError::VariableTypeMismatch(n)) if n == &name),
            "scalar variable must not coerce to Point2, got {result:?}"
        );
    }

    /// LAW 9 — Resolution is time-pure without evaluators: same inputs ⇒ same
    /// outputs regardless of `ctx.time` (motion/time coupling arrives only via
    /// injected evaluators).
    #[test]
    fn resolution_is_time_pure_without_evaluators(v in finite_f64(), t1 in finite_f64(), t2 in finite_f64()) {
        let vars = HashMap::new();
        let a = EvaluationContext::new(&vars, t1);
        let b = EvaluationContext::new(&vars, t2);
        let p: Parameter<f64> = Parameter::Literal(v);
        assert_f64_eq(p.resolve(&a).unwrap(), p.resolve(&b).unwrap());
    }
}
