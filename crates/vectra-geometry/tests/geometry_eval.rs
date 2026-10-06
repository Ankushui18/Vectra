//! Property-based guarantees for geometry evaluation (Task 1.3, MES §17).
//!
//! Laws: literal identity is bit-exact where the pipeline is arithmetic-free,
//! variables propagate through re-evaluation, arcs canonicalize losslessly,
//! lyon paths are finite + continuous, evaluation never halts on a bad node,
//! and incremental patches equal full rebuilds.

use lyon::path::Event;
use proptest::prelude::*;
use vectra_core::{Command, Engine, NodeId, NodeKind, ParamValue, Parameter, PathSegment, Point2};
use vectra_geometry::{
    DiagnosticCode, DirtySet, EvaluatedPrimitive, EvaluatedScene, Evaluator, GeometryEvaluator, TAU,
};

// ── Strategies ─────────────────────────────────────────────────────────

/// Coordinates well inside the GPU-representable range.
fn coord() -> impl Strategy<Value = f64> {
    -1e4..1e4f64
}

/// Non-negative dimensions (avoids intentional clamp diagnostics).
fn dim() -> impl Strategy<Value = f64> {
    0.0..1e4f64
}

fn point2() -> impl Strategy<Value = Point2> {
    (coord(), coord()).prop_map(|(x, y)| Point2::new(x, y))
}

fn var_name() -> impl Strategy<Value = String> {
    "[a-z][a-z0-9_]{0,7}".prop_map(|s| s.to_string())
}

/// Angles across multiple windings, both directions.
fn wide_angle() -> impl Strategy<Value = f64> {
    -30.0..30.0f64
}

fn arb_segment() -> impl Strategy<Value = PathSegment> {
    prop_oneof![
        point2().prop_map(|to| PathSegment::Line {
            to: Parameter::Literal(to)
        }),
        (point2(), point2()).prop_map(|(control, to)| PathSegment::Quadratic {
            control: Parameter::Literal(control),
            to: Parameter::Literal(to)
        }),
        (point2(), point2(), point2()).prop_map(|(control1, control2, to)| {
            PathSegment::Cubic {
                control1: Parameter::Literal(control1),
                control2: Parameter::Literal(control2),
                to: Parameter::Literal(to),
            }
        }),
        Just(PathSegment::Close),
    ]
}

// ── Helpers ────────────────────────────────────────────────────────────

fn assert_bits(a: f64, b: f64, what: &str) {
    assert!(
        a.to_bits() == b.to_bits(),
        "{what} mismatch: {a} vs {b} (bits {:x} vs {:x})",
        a.to_bits(),
        b.to_bits()
    );
}

fn create_node(engine: &mut Engine, kind: NodeKind) -> NodeId {
    let id = vectra_core::new_node_id();
    engine
        .dispatch(Command::CreateNode {
            id,
            kind,
            name: None,
            index: None,
        })
        .unwrap();
    id
}

fn eval_full(engine: &Engine) -> vectra_geometry::SceneEvaluation {
    let ctx = engine.evaluation_context();
    GeometryEvaluator.evaluate_full(engine.document(), &ctx)
}

fn rect_tuple(scene: &EvaluatedScene, id: NodeId) -> (f64, f64, f64, f64, f64) {
    match &scene.get(id).unwrap().primitive {
        EvaluatedPrimitive::Rect {
            x,
            y,
            w,
            h,
            corner_radius,
        } => (*x, *y, *w, *h, *corner_radius),
        other => panic!("expected Rect, got {}", other.tag()),
    }
}

fn assert_rect_scenes_equal(a: &EvaluatedScene, b: &EvaluatedScene) {
    assert_eq!(a.z_order, b.z_order, "z-order must match");
    assert_eq!(a.nodes.len(), b.nodes.len(), "membership must match");
    for id in &a.z_order {
        assert_eq!(rect_tuple(a, *id), rect_tuple(b, *id), "node {id} diverged");
        assert_eq!(
            a.get(*id).unwrap().style,
            b.get(*id).unwrap().style,
            "style of {id} diverged"
        );
    }
}

fn all_coords_finite(at: lyon::math::Point) -> bool {
    at.x.is_finite() && at.y.is_finite()
}

// ── Laws ───────────────────────────────────────────────────────────────

proptest! {
    /// LAW 1a — Rect identity: literal-only rects evaluate bit-exact, diagnosis-free.
    #[test]
    fn rect_identity(
        x in coord(), y in coord(), w in dim(), h in dim(), r_raw in dim(),
    ) {
        // Keep the radius inside the valid domain so no clamp fires.
        let r = r_raw.min(0.5 * w.min(h));
        let mut engine = Engine::new();
        let id = create_node(&mut engine, NodeKind::Rectangle {
            x: Parameter::Literal(x),
            y: Parameter::Literal(y),
            width: Parameter::Literal(w),
            height: Parameter::Literal(h),
            corner_radius: Parameter::Literal(r),
        });
        let eval = eval_full(&engine);
        assert!(eval.diagnostics.is_empty(), "literal rect must be clean: {:?}", eval.diagnostics);
        let (ex, ey, ew, eh, er) = rect_tuple(&eval.scene, id);
        assert_bits(ex, x, "x");
        assert_bits(ey, y, "y");
        assert_bits(ew, w, "w");
        assert_bits(eh, h, "h");
        assert_bits(er, r, "corner_radius");
    }

    /// LAW 1b — Circle identity: literal-only circles evaluate bit-exact.
    #[test]
    fn circle_identity(cx in coord(), cy in coord(), r in dim()) {
        let mut engine = Engine::new();
        let id = create_node(&mut engine, NodeKind::circle(cx, cy, r));
        let eval = eval_full(&engine);
        assert!(eval.diagnostics.is_empty());
        match &eval.scene.get(id).unwrap().primitive {
            EvaluatedPrimitive::Circle { cx: ecx, cy: ecy, r: er } => {
                assert_bits(*ecx, cx, "cx");
                assert_bits(*ecy, cy, "cy");
                assert_bits(*er, r, "r");
            }
            other => panic!("expected Circle, got {}", other.tag()),
        }
    }

    /// LAW 1c — In-range arcs pass normalization through: start is exact, and
    /// the sweep matches `end - start` (one rounding of the sweep difference
    /// is the only permitted drift — normalization is arithmetic, not a copy).
    #[test]
    fn arc_in_range_identity(
        (s, e) in (0.0..TAU).prop_flat_map(|s| (Just(s), s..=TAU)),
    ) {
        let mut engine = Engine::new();
        let id = create_node(&mut engine, NodeKind::Arc {
            cx: Parameter::Literal(0.0),
            cy: Parameter::Literal(0.0),
            radius: Parameter::Literal(5.0),
            start_angle: Parameter::Literal(s),
            end_angle: Parameter::Literal(e),
        });
        let eval = eval_full(&engine);
        assert!(eval.diagnostics.is_empty());
        match &eval.scene.get(id).unwrap().primitive {
            EvaluatedPrimitive::Arc { start_angle, end_angle, .. } => {
                assert_eq!(*start_angle, s, "in-range start must pass through");
                assert!(
                    (*end_angle - e).abs() < 1e-9,
                    "end drifted: {end_angle} vs {e}"
                );
                assert!(
                    ((*end_angle - *start_angle) - (e - s)).abs() < 1e-9,
                    "sweep must be preserved"
                );
            }
            other => panic!("expected Arc, got {}", other.tag()),
        }
    }

    /// LAW 2 — Variable law: one variable drives two nodes; rebinding +
    /// re-evaluating updates both bit-exactly, nothing else moves.
    #[test]
    fn variable_drives_scene(name in var_name(), v1 in dim(), v2 in dim()) {
        let mut engine = Engine::new();
        engine.dispatch(Command::SetVariable { name: name.clone(), value: v1 }).unwrap();
        let rect = create_node(&mut engine, NodeKind::Rectangle {
            x: Parameter::Literal(1.0),
            y: Parameter::Literal(2.0),
            width: Parameter::variable(&name),
            height: Parameter::Literal(10.0),
            corner_radius: Parameter::Literal(0.0),
        });
        let circle = create_node(&mut engine, NodeKind::Circle {
            cx: Parameter::Literal(0.0),
            cy: Parameter::Literal(0.0),
            radius: Parameter::variable(&name),
        });

        let before = eval_full(&engine);
        assert!(before.diagnostics.is_empty());
        assert_bits(rect_tuple(&before.scene, rect).2, v1, "rect.w @v1");
        match &before.scene.get(circle).unwrap().primitive {
            EvaluatedPrimitive::Circle { r, .. } => assert_bits(*r, v1, "circle.r @v1"),
            other => panic!("expected Circle, got {}", other.tag()),
        }

        engine.dispatch(Command::SetVariable { name: name.clone(), value: v2 }).unwrap();
        let after = eval_full(&engine);
        assert!(after.diagnostics.is_empty());
        let (x, y, w, h, r) = rect_tuple(&after.scene, rect);
        assert_bits(w, v2, "rect.w @v2");
        assert_bits(x, 1.0, "rect.x untouched");
        assert_bits(y, 2.0, "rect.y untouched");
        assert_bits(h, 10.0, "rect.h untouched");
        assert_bits(r, 0.0, "rect.corner untouched");
        match &after.scene.get(circle).unwrap().primitive {
            EvaluatedPrimitive::Circle { r, .. } => assert_bits(*r, v2, "circle.r @v2"),
            other => panic!("expected Circle, got {}", other.tag()),
        }
    }

    /// LAW 3 — Arc integrity: arbitrary windings canonicalize to
    /// `start ∈ [0, TAU)`, `sweep ∈ [0, TAU]`, with the full-circle rule
    /// (`sweep == TAU` ⟺ non-zero winding landing back on its start).
    #[test]
    fn arc_integrity(s in wide_angle(), e in wide_angle()) {
        let mut engine = Engine::new();
        let id = create_node(&mut engine, NodeKind::Arc {
            cx: Parameter::Literal(0.0),
            cy: Parameter::Literal(0.0),
            radius: Parameter::Literal(5.0),
            start_angle: Parameter::Literal(s),
            end_angle: Parameter::Literal(e),
        });
        let eval = eval_full(&engine);
        assert!(eval.diagnostics.is_empty(), "finite angles must be clean: {:?}", eval.diagnostics);
        match &eval.scene.get(id).unwrap().primitive {
            EvaluatedPrimitive::Arc { start_angle, end_angle, .. } => {
                let sweep = *end_angle - *start_angle;
                assert!(
                    (0.0..TAU).contains(start_angle),
                    "start {start_angle} outside [0, TAU)"
                );
                assert!(
                    (0.0..=TAU).contains(&sweep),
                    "sweep {sweep} outside [0, TAU]"
                );
                // Contract check against independently computed expectation.
                let raw = e - s;
                let expected = if raw == 0.0 {
                    0.0
                } else {
                    let r = raw.rem_euclid(TAU);
                    if r == 0.0 { TAU } else { r }
                };
                assert!(
                    (sweep - expected).abs() < 1e-9,
                    "sweep {sweep} != expected {expected} for {s} → {e}"
                );
                if sweep == TAU {
                    assert_ne!(raw, 0.0, "TAU sweep requires non-zero winding");
                }
            }
            other => panic!("expected Arc, got {}", other.tag()),
        }
    }

    /// LAW 4 — Lyon validity: complex paths produce finite, continuous,
    /// properly terminated event streams that preserve segment order/types.
    #[test]
    fn lyon_path_validity(
        start in point2(),
        segments in prop::collection::vec(arb_segment(), 0..8),
    ) {
        let mut engine = Engine::new();
        let id = create_node(&mut engine, NodeKind::Path {
            start: Parameter::Literal(start),
            segments: segments.clone(),
        });
        let eval = eval_full(&engine);
        assert!(!eval.has_errors(), "finite path must not error: {:?}", eval.diagnostics);
        let path = match &eval.scene.get(id).unwrap().primitive {
            EvaluatedPrimitive::Path(p) => p,
            other => panic!("expected Path, got {}", other.tag()),
        };
        let events: Vec<_> = path.iter().collect();
        assert!(!events.is_empty(), "path must yield events");

        // First event begins exactly at `start` (same f64→f32 conversion).
        match &events[0] {
            Event::Begin { at } => {
                assert_eq!(at.x, start.x as f32, "begin.x must match start");
                assert_eq!(at.y, start.y as f32, "begin.y must match start");
            }
            other => panic!("first event must be Begin, got {other:?}"),
        }
        // Every subpath is properly terminated (pins the explicit end() fix).
        assert!(
            matches!(events.last().unwrap(), Event::End { .. }),
            "path must end with End, got {:?}", events.last().unwrap()
        );

        // Finiteness + continuity + draw-event census.
        let mut prev_to: Option<lyon::math::Point> = None;
        let mut draw_tags: Vec<&str> = Vec::new();
        for event in &events {
            match *event {
                Event::Begin { at } => {
                    assert!(all_coords_finite(at), "Begin has non-finite coords");
                    prev_to = Some(at);
                }
                Event::Line { from, to } => {
                    assert!(all_coords_finite(from) && all_coords_finite(to));
                    assert_eq!(Some(from), prev_to, "Line.from must continue the subpath");
                    prev_to = Some(to);
                    draw_tags.push("line");
                }
                Event::Quadratic { from, ctrl, to } => {
                    assert!(all_coords_finite(from) && all_coords_finite(ctrl) && all_coords_finite(to));
                    assert_eq!(Some(from), prev_to, "Quad.from must continue the subpath");
                    prev_to = Some(to);
                    draw_tags.push("quad");
                }
                Event::Cubic { from, ctrl1, ctrl2, to } => {
                    assert!(all_coords_finite(from) && all_coords_finite(ctrl1)
                        && all_coords_finite(ctrl2) && all_coords_finite(to));
                    assert_eq!(Some(from), prev_to, "Cubic.from must continue the subpath");
                    prev_to = Some(to);
                    draw_tags.push("cubic");
                }
                Event::End { last, first, .. } => {
                    assert!(all_coords_finite(last) && all_coords_finite(first));
                    assert_eq!(Some(last), prev_to, "End.last must be the subpath tip");
                    prev_to = None;
                }
            }
        }
        let expected: Vec<&str> = segments.iter().filter_map(|s| match s {
            PathSegment::Line { .. } => Some("line"),
            PathSegment::Quadratic { .. } => Some("quad"),
            PathSegment::Cubic { .. } => Some("cubic"),
            PathSegment::Close => None,
        }).collect();
        assert_eq!(draw_tags, expected, "draw events must preserve segment order/types");
    }

    /// LAW 5 — Resilience: a NaN path coordinate skips its node with a typed
    /// diagnostic while the sibling still evaluates (evaluation never halts).
    #[test]
    fn path_nan_isolates_failure(nan_x in any::<bool>()) {
        let mut engine = Engine::new();
        let bad_point = if nan_x { Point2::new(f64::NAN, 1.0) } else { Point2::new(1.0, f64::NAN) };
        let bad = create_node(&mut engine, NodeKind::Path {
            start: Parameter::Literal(Point2::ZERO),
            segments: vec![PathSegment::Line { to: Parameter::Literal(bad_point) }],
        });
        let good = create_node(&mut engine, NodeKind::rectangle(0.0, 0.0, 5.0, 5.0));

        let eval = eval_full(&engine);
        assert!(eval.scene.get(bad).is_none(), "NaN path must be skipped");
        assert!(eval.scene.get(good).is_some(), "sibling must survive");
        assert!(eval.has_errors());
        assert!(
            eval.errors().any(|d| d.code == DiagnosticCode::NonFiniteValue && d.node_id == Some(bad)),
            "expected NonFiniteValue for the bad node, got {:?}", eval.diagnostics
        );
    }

    /// LAW 6 — Resilience: a path point bound to a missing variable skips its
    /// node with `UnresolvableParameter`; the scene still evaluates.
    #[test]
    fn path_missing_variable_isolates_failure(name in var_name()) {
        let mut engine = Engine::new();
        // Scalar variables cannot resolve to points either — both spellings
        // must fail typed, never silent. Here: expression-free missing var on
        // a point-typed property... variables are scalar-only, so bind the
        // *start* through a variable name and expect the typed rejection.
        let bad = create_node(&mut engine, NodeKind::Path {
            start: Parameter::Variable(name.clone()),
            segments: vec![],
        });
        let good = create_node(&mut engine, NodeKind::circle(1.0, 2.0, 3.0));

        let eval = eval_full(&engine);
        assert!(eval.scene.get(bad).is_none());
        assert!(eval.scene.get(good).is_some());
        assert!(
            eval.errors().any(|d| d.code == DiagnosticCode::UnresolvableParameter),
            "expected UnresolvableParameter, got {:?}", eval.diagnostics
        );
    }

    /// LAW 7 — Incrementality: patching a cached scene with a partial
    /// evaluation equals a fresh full rebuild, exactly.
    #[test]
    fn incremental_patch_equals_rebuild(
        widths in prop::collection::vec(dim(), 1..6),
        target in any::<usize>(),
        new_width in dim(),
    ) {
        let mut engine = Engine::new();
        let ids: Vec<NodeId> = widths.iter().map(|w| {
            create_node(&mut engine, NodeKind::rectangle(0.0, 0.0, *w, 1.0))
        }).collect();
        let victim = ids[target % ids.len()];

        let mut cached = eval_full(&engine).scene;
        engine.dispatch(Command::SetParameter {
            node_id: victim,
            property: "width".to_string(),
            value: ParamValue::float_literal(new_width),
        }).unwrap();

        let dirty = DirtySet::single(victim);
        let ctx = engine.evaluation_context();
        let partial = GeometryEvaluator.evaluate(engine.document(), &ctx, &dirty);
        assert!(partial.diagnostics.is_empty());
        cached.apply_partial(engine.document(), partial.scene, &dirty);

        let fresh = eval_full(&engine).scene;
        assert_rect_scenes_equal(&cached, &fresh);
    }

    /// LAW 8 — Incremental delete: a deleted node drops out of the patched
    /// scene; order of survivors is preserved.
    #[test]
    fn incremental_delete_equals_rebuild(
        widths in prop::collection::vec(dim(), 1..6),
        target in any::<usize>(),
    ) {
        let mut engine = Engine::new();
        let ids: Vec<NodeId> = widths.iter().map(|w| {
            create_node(&mut engine, NodeKind::rectangle(0.0, 0.0, *w, 1.0))
        }).collect();
        let victim = ids[target % ids.len()];

        let mut cached = eval_full(&engine).scene;
        engine.dispatch(Command::DeleteNode { id: victim }).unwrap();

        let dirty = DirtySet::single(victim);
        let ctx = engine.evaluation_context();
        let partial = GeometryEvaluator.evaluate(engine.document(), &ctx, &dirty);
        cached.apply_partial(engine.document(), partial.scene, &dirty);

        let fresh = eval_full(&engine).scene;
        assert_rect_scenes_equal(&cached, &fresh);
        assert!(cached.get(victim).is_none());
    }
}
