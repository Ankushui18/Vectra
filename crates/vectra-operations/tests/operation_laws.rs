//! Operation laws (Task 4.0 §4) — the non-destructive operations contract,
//! executable.
//!
//! These run the **real** pipeline end to end, minus the WASM shell:
//!
//! ```text
//! Engine::dispatch(ApplyOperation) → Document ──GeometryEvaluator──▶ primitives
//!                                            ──OperationsEvaluator──▶ + virtual paths
//! ```
//!
//! * **Boolean Satisfaction Law** — `area(A \ B) == area(A) − area(A ∩ B)` for
//!   arbitrary axis-aligned rectangles. The comparison is made in the
//!   representation the pipeline uses (paths are `f32` — see
//!   `representable_area`), so the assertion is tight (1e-9) rather than
//!   papering over representation error with a loose epsilon.
//! * **Non-Destructive Source Law** — moving a source never mutates its
//!   partner, and the operation's geometry always matches the arithmetic for
//!   the *new* positions.
//! * **Undo Law** — undoing `ApplyOperation` removes the virtual node and
//!   leaves every source byte-identical.
//!
//! Plus focused laws for the modifiers, for disabled/parked operations, and for
//! a source that cannot be a region (a typed diagnostic, never a broken pass).

use geo::{BooleanOps, Coord, LineString, MultiPolygon, Polygon};
use proptest::prelude::*;
use vectra_core::{
    new_operation_id, BooleanOp, Command, Engine, NodeId, OperationId, OperationKind, Parameter,
};
use vectra_geometry::{path_to_svg_data, EvaluatedPrimitive, EvaluatedScene, GeometryEvaluator};
use vectra_operations::{path_area, region_area, OperationsEvaluator};

// ── Harness ──────────────────────────────────────────────────────────────

/// A document plus the composed scene, kept in step the way `vectra-wasm` keeps
/// them (primitive pass, then operations pass).
struct Harness {
    engine: Engine,
    scene: EvaluatedScene,
    diagnostics: Vec<vectra_geometry::Diagnostic>,
    operations: OperationsEvaluator,
}

impl Harness {
    fn new() -> Self {
        Self {
            engine: Engine::new(),
            scene: EvaluatedScene::empty(),
            diagnostics: Vec::new(),
            operations: OperationsEvaluator::new(),
        }
    }

    fn doc(&self) -> &vectra_core::Document {
        self.engine.document()
    }

    fn create_rect(&mut self, name: &str, x: f64, y: f64, w: f64, h: f64) -> NodeId {
        let id = vectra_core::new_node_id();
        self.engine
            .dispatch(Command::CreateNode {
                id,
                kind: vectra_core::NodeKind::rectangle(x, y, w, h),
                name: Some(name.to_string()),
                index: None,
            })
            .expect("create");
        self.refresh();
        id
    }

    fn create_circle(&mut self, name: &str, cx: f64, cy: f64, r: f64) -> NodeId {
        let id = vectra_core::new_node_id();
        self.engine
            .dispatch(Command::CreateNode {
                id,
                kind: vectra_core::NodeKind::circle(cx, cy, r),
                name: Some(name.to_string()),
                index: None,
            })
            .expect("create");
        self.refresh();
        id
    }

    fn apply(&mut self, kind: OperationKind, inputs: Vec<NodeId>) -> OperationId {
        let id = new_operation_id();
        self.engine
            .dispatch(Command::ApplyOperation { id, kind, inputs })
            .expect("apply operation");
        self.refresh();
        id
    }

    fn set_param(&mut self, node: NodeId, property: &str, value: f64) -> Result<(), String> {
        self.engine
            .dispatch(Command::SetParameter {
                node_id: node,
                property: property.to_string(),
                value: vectra_core::ParamValue::float_literal(value),
            })
            .map(|_| ())
            .map_err(|e| e.to_string())?;
        self.refresh();
        Ok(())
    }

    fn undo(&mut self) {
        self.engine.undo().expect("undo");
        self.refresh();
    }

    fn redo(&mut self) {
        self.engine.redo().expect("redo");
        self.refresh();
    }

    /// Re-run both passes: primitives first, then the operation layer on top.
    fn refresh(&mut self) {
        let context = {
            let ctx = self.engine.evaluation_context();
            GeometryEvaluator.evaluate_full(self.engine.document(), &ctx)
        };
        let mut scene = context.scene;
        let evaluation = {
            let ctx = self.engine.evaluation_context();
            self.operations
                .evaluate_all(self.engine.document(), &scene, &ctx)
        };
        evaluation.compose_into(&mut scene, self.engine.document());
        self.diagnostics = context.diagnostics;
        self.diagnostics.extend(evaluation.diagnostics);
        self.scene = scene;
    }

    /// A node's geometry as SVG data. Every primitive is rendered through the
    /// same path view the operations pass reads, so "unchanged" is compared
    /// like-for-like across kinds (a rect compares as its path, not as its
    /// four numbers).
    fn path_of(&self, id: NodeId) -> String {
        let node = self.scene.get(id).expect("node in scene");
        match &node.primitive {
            EvaluatedPrimitive::Path(path) => path_to_svg_data(path),
            primitive => path_to_svg_data(&vectra_geometry::primitive_to_path(primitive)),
        }
    }

    /// The region the operations pass reads for this node: its evaluated
    /// geometry, flattened exactly as the converter flattens it.
    fn region_of(&self, id: NodeId) -> MultiPolygon<f64> {
        let node = self.scene.get(id).expect("node in scene");
        vectra_operations::path_to_multi_polygon(&vectra_geometry::primitive_to_path(
            &node.primitive,
        ))
        .expect("primitive is a region")
    }

    fn area_of(&self, id: NodeId) -> f64 {
        match &self.scene.get(id).expect("node in scene").primitive {
            EvaluatedPrimitive::Path(path) => path_area(path),
            other => region_area(
                &vectra_operations::path_to_multi_polygon(&vectra_geometry::primitive_to_path(
                    other,
                ))
                .expect("primitive is a region"),
            ),
        }
    }

    fn in_scene(&self, id: NodeId) -> bool {
        self.scene.get(id).is_some()
    }
}

/// The area the *pipeline* can represent for `region`: round-trip it through the
/// `f32` path representation the same way the evaluator does, then measure.
///
/// The laws are stated on this quantity rather than on the raw `f64` arithmetic
/// because every coordinate in a `lyon::path::Path` is `f32` (the renderer's
/// vertex format) — a boolean result that lands off the `f32` lattice is
/// rounded on the way out, and that rounding is a *representation* choice, not
/// an arithmetic error. Comparing like-for-like leaves the real residual (float
/// summation order) at ~1e-9, which is what the assertions use.
fn representable_area(region: &MultiPolygon<f64>) -> f64 {
    path_area(&vectra_operations::multi_polygon_to_path(region))
}

/// The same rectangle as a `geo` region, for computing expected areas.
///
/// # Why the coordinates are quantised through `f32`
///
/// Paths in this workspace are `f32` by contract — lyon stores path points as
/// `f32` (the renderer's vertex format), and `primitive_to_path` is where that
/// conversion happens. A boolean result therefore lives on an `f32` lattice, and
/// comparing it against an `f64` expectation would measure the *representation*
/// error rather than the arithmetic. Quantising the expectation the same way
/// removes that term, so the laws assert the pipeline's real accuracy (~1e-9)
/// instead of hiding it behind a loose epsilon.
fn rect_region(x: f64, y: f64, w: f64, h: f64) -> MultiPolygon<f64> {
    let q = |v: f64| (v as f32) as f64;
    let (x, y, w, h) = (q(x), q(y), q(w), q(h));
    rect_region_exact(x, y, w, h)
}

/// The rectangle without quantisation (fixtures whose coordinates are exactly
/// representable in `f32` are unaffected by the difference).
fn rect_region_exact(x: f64, y: f64, w: f64, h: f64) -> MultiPolygon<f64> {
    MultiPolygon::new(vec![Polygon::new(
        LineString::new(vec![
            Coord { x, y },
            Coord { x: x + w, y },
            Coord { x: x + w, y: y + h },
            Coord { x, y: y + h },
            Coord { x, y },
        ]),
        Vec::new(),
    )])
}

/// One `(x, y, w, h)` rectangle per operand, for the multi-fixture law.
type RectFixture = (f64, f64, f64, f64, f64, f64, f64, f64);

// ── Focused laws ─────────────────────────────────────────────────────────

#[test]
fn law_boolean_satisfaction_by_area() {
    let mut h = Harness::new();
    // Overlapping rectangles: A = 0..100 × 0..40, B = 60..160 × 20..60.
    let a = h.create_rect("a", 0.0, 0.0, 100.0, 40.0);
    let b = h.create_rect("b", 60.0, 20.0, 100.0, 40.0);
    let op = h.apply(
        OperationKind::Boolean {
            op: BooleanOp::Subtract,
        },
        vec![a, b],
    );

    let (ra, rb) = (
        rect_region(0.0, 0.0, 100.0, 40.0),
        rect_region(60.0, 20.0, 100.0, 40.0),
    );
    let overlap = region_area(&ra.intersection(&rb));
    assert!(overlap > 0.0, "the fixtures must actually overlap");
    let expected = region_area(&ra) - overlap;

    assert!(
        (h.area_of(op) - expected).abs() < 1e-6,
        "area(A \\ B) == area(A) − area(A ∩ B): {} vs {expected}",
        h.area_of(op)
    );
    // The sources are still exactly where they were (RULE 1).
    assert_eq!(h.area_of(a), 4000.0);
    assert_eq!(h.area_of(b), 4000.0);
    // …and the virtual node is a normal scene node with its own identity.
    assert!(h.in_scene(op), "the operation result is in the scene");
    assert_eq!(
        h.doc().operations.get(op).unwrap().inputs,
        vec![a, b],
        "and it remembers what it reads"
    );
}

#[test]
fn law_every_boolean_identity_holds() {
    // Disjoint, touching and nested fixtures, all exact for rectangles.
    let fixtures: [RectFixture; 3] = [
        (0.0, 0.0, 10.0, 10.0, 20.0, 20.0, 10.0, 10.0), // disjoint
        (0.0, 0.0, 10.0, 10.0, 5.0, 0.0, 10.0, 10.0),   // overlapping
        (0.0, 0.0, 20.0, 20.0, 5.0, 5.0, 10.0, 10.0),   // nested
    ];
    for (ax, ay, aw, ah, bx, by, bw, bh) in fixtures {
        let mut h = Harness::new();
        let a = h.create_rect("a", ax, ay, aw, ah);
        let b = h.create_rect("b", bx, by, bw, bh);
        let ra = rect_region(ax, ay, aw, ah);
        let rb = rect_region(bx, by, bw, bh);
        let (area_a, area_b) = (region_area(&ra), region_area(&rb));
        let both = region_area(&ra.intersection(&rb));
        for (op, expected) in [
            (BooleanOp::Union, area_a + area_b - both),
            (BooleanOp::Subtract, area_a - both),
            (BooleanOp::Intersect, both),
            (BooleanOp::Exclude, area_a + area_b - 2.0 * both),
        ] {
            let id = h.apply(OperationKind::Boolean { op }, vec![a, b]);
            // Union of disjoint/nested rects can be two polygons or a shell with
            // a hole; the area identity is the invariant either way.
            assert!(
                (h.area_of(id) - expected).abs() < 1e-6,
                "{op:?} on the fixture: {} vs {expected}",
                h.area_of(id)
            );
        }
    }
}

#[test]
fn law_sources_are_never_mutated_by_an_operation() {
    let mut h = Harness::new();
    let a = h.create_rect("a", 0.0, 0.0, 100.0, 40.0);
    let b = h.create_rect("b", 60.0, 20.0, 100.0, 40.0);
    let before_a = h.path_of(a);
    let before_a_params = h.doc().get_node(a).unwrap().kind.clone();
    let op = h.apply(
        OperationKind::Boolean {
            op: BooleanOp::Union,
        },
        vec![a, b],
    );
    let union_area = h.area_of(op);

    // Moving B re-runs the operation…
    h.set_param(b, "x", 20.0).expect("move b");
    assert_ne!(h.path_of(op), "", "the virtual node still has geometry");
    let expected = region_area(
        &rect_region(0.0, 0.0, 100.0, 40.0).union(&rect_region(20.0, 20.0, 100.0, 40.0)),
    );
    assert!(
        (h.area_of(op) - expected).abs() < 1e-6,
        "the union tracks the new position: {} vs {expected}",
        h.area_of(op)
    );

    // …and A is untouched: same evaluated path, same parameters, same document
    // record. This is the law that says "operations are not destructive".
    assert_eq!(h.path_of(a), before_a, "A's geometry is byte-identical");
    assert_eq!(
        h.doc().get_node(a).unwrap().kind,
        before_a_params,
        "A's parameters are byte-identical"
    );
    assert_eq!(h.doc().get_node(a).unwrap().kind, before_a_params);
    // B moved, so the union cannot be a no-op — the fixtures guarantee it.
    assert_ne!(h.area_of(op), union_area);
}

#[test]
fn law_undo_removes_the_operation_and_nothing_else() {
    let mut h = Harness::new();
    let a = h.create_rect("a", 0.0, 0.0, 100.0, 40.0);
    let b = h.create_rect("b", 60.0, 20.0, 100.0, 40.0);
    let before = (h.path_of(a), h.path_of(b));
    let op = h.apply(
        OperationKind::Boolean {
            op: BooleanOp::Intersect,
        },
        vec![a, b],
    );
    assert!(h.in_scene(op));

    h.undo();
    assert!(
        !h.in_scene(op),
        "the virtual node is gone from the composed scene"
    );
    assert!(
        h.doc().operations.is_empty(),
        "and from the registry: {:?}",
        h.doc().operations.order
    );
    assert_eq!(h.path_of(a), before.0, "source A is untouched by the undo");
    assert_eq!(h.path_of(b), before.1, "source B is untouched by the undo");
    assert_eq!(
        h.doc().nodes.len(),
        2,
        "both sources are still in the document"
    );

    h.redo();
    assert!(h.in_scene(op), "redo restores the virtual node");
    assert_eq!(
        h.doc().operations.get(op).unwrap().inputs,
        vec![a, b],
        "with the same id and the same inputs"
    );
}

#[test]
fn law_modifiers_are_applied_to_the_source_not_in_it() {
    let mut h = Harness::new();
    let c = h.create_circle("c", 0.0, 0.0, 10.0);
    let before = h.path_of(c);

    let fat = h.apply(
        OperationKind::Offset {
            distance: Parameter::Literal(5.0),
        },
        vec![c],
    );
    let rounded = h.apply(
        OperationKind::Mirror {
            axis: vectra_core::MirrorAxis::Vertical {
                at: Parameter::Literal(0.0),
            },
        },
        vec![c],
    );
    let filleted = h.apply(
        OperationKind::Fillet {
            radius: Parameter::Literal(2.0),
        },
        vec![c],
    );

    assert_eq!(h.path_of(c), before, "the circle itself never changed");
    assert!(
        h.area_of(fat) > h.area_of(c),
        "offset +5 grows the shape: {} vs {}",
        h.area_of(fat),
        h.area_of(c)
    );
    assert!(
        (h.area_of(rounded) - h.area_of(c)).abs() < 1.0,
        "a mirror preserves area"
    );
    assert!(
        h.area_of(filleted) > 0.0 && h.in_scene(filleted),
        "filleting a circle is a no-op-ish but never a failure"
    );
}

#[test]
fn a_parked_operation_keeps_its_record_and_loses_its_geometry() {
    let mut h = Harness::new();
    let a = h.create_rect("a", 0.0, 0.0, 10.0, 10.0);
    let b = h.create_rect("b", 5.0, 5.0, 10.0, 10.0);
    let op = h.apply(
        OperationKind::Boolean {
            op: BooleanOp::Subtract,
        },
        vec![a, b],
    );
    assert!(h.in_scene(op));

    h.engine
        .dispatch(Command::SetOperationEnabled {
            id: op,
            enabled: false,
        })
        .expect("disable");
    h.refresh();
    assert!(
        !h.in_scene(op),
        "a parked operation contributes no geometry"
    );
    assert!(
        h.doc().operations.contains(op),
        "but it keeps its registry entry, inputs and id"
    );
    assert!(h.in_scene(a) && h.in_scene(b), "and its sources stay live");

    h.engine
        .dispatch(Command::SetOperationEnabled {
            id: op,
            enabled: true,
        })
        .expect("re-enable");
    h.refresh();
    assert!(h.in_scene(op), "re-arming it brings the geometry back");
}

#[test]
fn a_broken_operation_is_a_diagnostic_not_a_broken_pass() {
    let mut h = Harness::new();
    let a = h.create_rect("a", 0.0, 0.0, 10.0, 10.0);
    let b = h.create_rect("b", 5.0, 5.0, 10.0, 10.0);
    // A fillet whose radius cannot resolve: the operation is registered (the
    // command is legal), the pass reports it, and the rest of the scene is
    // unaffected.
    let op = h.apply(
        OperationKind::Fillet {
            radius: Parameter::variable("no_such_variable"),
        },
        vec![a],
    );
    assert!(!h.in_scene(op), "no geometry for an impossible operation");
    assert!(
        h.diagnostics
            .iter()
            .any(|d| d.code.tag() == "operation-failed"),
        "the failure is reported: {:?}",
        h.diagnostics
    );
    assert!(
        h.in_scene(a) && h.in_scene(b),
        "and every other node still evaluates"
    );

    // Supplying the variable makes the very same operation valid on the next
    // pass — the registry was never damaged.
    h.engine
        .dispatch(Command::SetVariable {
            name: "no_such_variable".to_string(),
            value: 2.0,
        })
        .expect("define the variable");
    h.refresh();
    assert!(
        h.in_scene(op),
        "the operation recovers once its operand exists"
    );
}

#[test]
fn deleting_a_source_withdraws_the_operations_that_read_it() {
    let mut h = Harness::new();
    let a = h.create_rect("a", 0.0, 0.0, 10.0, 10.0);
    let b = h.create_rect("b", 5.0, 5.0, 10.0, 10.0);
    let op = h.apply(
        OperationKind::Boolean {
            op: BooleanOp::Union,
        },
        vec![a, b],
    );

    h.engine
        .dispatch(Command::DeleteNode { id: a })
        .expect("delete the source");
    h.refresh();
    assert!(
        h.doc().operations.is_empty(),
        "an operation without a source has nothing to read"
    );
    assert!(!h.in_scene(op), "and its geometry leaves the scene");
    assert!(h.in_scene(b), "the surviving source is untouched");

    h.undo();
    assert!(
        h.doc().operations.contains(op) && h.in_scene(a) && h.in_scene(op),
        "one undo restores the source and the operation together"
    );
}

#[test]
fn arity_and_missing_inputs_are_rejected_before_anything_is_stored() {
    let mut h = Harness::new();
    let a = h.create_rect("a", 0.0, 0.0, 10.0, 10.0);
    let b = h.create_rect("b", 5.0, 5.0, 10.0, 10.0);

    // A boolean needs exactly two inputs.
    for inputs in [vec![a], vec![a, b, a]] {
        let error = h
            .engine
            .dispatch(Command::ApplyOperation {
                id: new_operation_id(),
                kind: OperationKind::Boolean {
                    op: BooleanOp::Union,
                },
                inputs,
            })
            .expect_err("arity violation");
        assert!(error.is_operation_error(), "{error}");
    }
    // A modifier needs exactly one.
    let error = h
        .engine
        .dispatch(Command::ApplyOperation {
            id: new_operation_id(),
            kind: OperationKind::Offset {
                distance: Parameter::Literal(1.0),
            },
            inputs: vec![a, b],
        })
        .expect_err("arity violation");
    assert!(error.is_operation_error(), "{error}");

    // An unknown source is refused, and nothing is registered.
    let error = h
        .engine
        .dispatch(Command::ApplyOperation {
            id: new_operation_id(),
            kind: OperationKind::Fillet {
                radius: Parameter::Literal(1.0),
            },
            inputs: vec![vectra_core::new_node_id()],
        })
        .expect_err("unknown input");
    assert!(
        !error.is_operation_error(),
        "that is a document error: {error}"
    );
    assert!(h.doc().operations.is_empty(), "nothing was stored");

    // A refused operation is not a history entry: in an engine that has done
    // nothing else, the only thing that could make `can_undo` true is a
    // command that was refused.
    let mut fresh = Engine::new();
    let error = fresh
        .dispatch(Command::ApplyOperation {
            id: new_operation_id(),
            kind: OperationKind::Boolean {
                op: BooleanOp::Union,
            },
            inputs: vec![vectra_core::new_node_id()],
        })
        .expect_err("arity violation");
    assert!(error.is_operation_error(), "{error}");
    assert!(
        !fresh.can_undo(),
        "a refused operation is not a history entry"
    );

    // Undo/redo of an operation is a first-class command pair.
    let op = h.apply(
        OperationKind::Boolean {
            op: BooleanOp::Exclude,
        },
        vec![a, b],
    );
    h.undo();
    h.redo();
    assert!(h.doc().operations.contains(op));
}

// ── Proptests ────────────────────────────────────────────────────────────

proptest! {
    #![proptest_config(ProptestConfig { cases: 64, max_shrink_iters: 400, ..ProptestConfig::default() })]

    /// **Boolean Satisfaction Law.** For arbitrary rectangles,
    /// `area(Subtract(A, B)) == area(A) − area(A ∩ B)` — computed independently
    /// by `geo` on the same operands, so the assertion compares two different
    /// code paths to the same number.
    #[test]
    fn prop_subtract_area_is_exactly_a_minus_the_overlap(
        ax in -50.0f64..50.0, ay in -50.0f64..50.0, aw in 1.0f64..80.0, ah in 1.0f64..80.0,
        bx in -50.0f64..50.0, by in -50.0f64..50.0, bw in 1.0f64..80.0, bh in 1.0f64..80.0,
    ) {
        let mut h = Harness::new();
        let a = h.create_rect("a", ax, ay, aw, ah);
        let b = h.create_rect("b", bx, by, bw, bh);
        let op = h.apply(OperationKind::Boolean { op: BooleanOp::Subtract }, vec![a, b]);

        // Both sides of the comparison are the pipeline's own regions, so the
        // assertion measures the boolean arithmetic and nothing else.
        let ra = h.region_of(a);
        let rb = h.region_of(b);
        let expected = representable_area(&ra.difference(&rb));
        prop_assert!(
            (h.area_of(op) - expected).abs() < 1e-9,
            "area {} vs expected {expected}", h.area_of(op)
        );
        // …and the same number read through the identity the law is named for:
        // `area(A) − area(A ∩ B)`, each term measured straight off the operand
        // regions. This form is one `f32`-rounding looser than the assertion
        // above — it subtracts two separately rounded areas instead of rounding
        // one difference — so its tolerance is relative (≈ one `f32` ulp), while
        // the strict 1e-9 form above stays the primary statement.
        let identity = representable_area(&ra) - representable_area(&ra.intersection(&rb));
        let tolerance = 1e-6 * expected.abs().max(1.0);
        prop_assert!(
            (h.area_of(op) - identity).abs() <= tolerance,
            "area {} vs area(A) − area(A ∩ B) = {identity} (tol {tolerance})", h.area_of(op)
        );
        // Non-negativity is the check that a boolean result cannot invent or
        // destroy material.
        prop_assert!(h.area_of(op) >= -1e-9);
    }

    /// **Non-Destructive Source Law.** Moving B re-runs the operation and never
    /// touches A: identical evaluated path, identical parameters, and a result
    /// that matches the arithmetic for B's new position.
    #[test]
    fn prop_moving_a_source_never_mutates_its_partner(
        bx0 in -40.0f64..0.0, by0 in -40.0f64..0.0, bw in 5.0f64..60.0, bh in 5.0f64..60.0,
        dx in 0.0f64..60.0, dy in 0.0f64..60.0,
    ) {
        let mut h = Harness::new();
        let a = h.create_rect("a", 0.0, 0.0, 100.0, 40.0);
        let b = h.create_rect("b", bx0, by0, bw, bh);
        let op = h.apply(OperationKind::Boolean { op: BooleanOp::Union }, vec![a, b]);

        let a_path = h.path_of(a);
        let a_kind = h.doc().get_node(a).unwrap().kind.clone();

        // Wherever B lands (including outside the document's visible area), the
        // union must simply track it.
        let (bx1, by1) = (bx0 + dx, by0 + dy);
        h.set_param(b, "x", bx1).map_err(TestCaseError::fail)?;
        h.set_param(b, "y", by1).map_err(TestCaseError::fail)?;

        prop_assert_eq!(h.path_of(a), a_path, "A's evaluated path is untouched");
        prop_assert_eq!(h.doc().get_node(a).unwrap().kind.clone(), a_kind, "A's parameters are untouched");

        let expected = representable_area(&h.region_of(a).union(&h.region_of(b)));
        prop_assert!(
            (h.area_of(op) - expected).abs() < 1e-9,
            "union tracks the move: {} vs {expected}", h.area_of(op)
        );
    }

    /// **Undo Law.** Applying an operation and undoing it leaves the document
    /// exactly as it was — same nodes, same parameters, same geometry — and the
    /// virtual node is gone. Redo restores it identically.
    #[test]
    fn prop_undo_of_an_operation_is_an_exact_pre_image(
        ax in -30.0f64..30.0, bx in -30.0f64..30.0, w in 1.0f64..60.0, hgt in 1.0f64..60.0,
    ) {
        let mut h = Harness::new();
        let a = h.create_rect("a", ax, 0.0, w, hgt);
        let b = h.create_rect("b", bx, 10.0, w, hgt);
        let before_nodes: Vec<(String, NodeId)> = h.doc().nodes.iter().map(|(k, v)| (v.name.clone(), *k)).collect();
        let before_paths = (h.path_of(a), h.path_of(b));

        let op = h.apply(OperationKind::Boolean { op: BooleanOp::Intersect }, vec![a, b]);
        let op_path = h.path_of(op);

        h.undo();
        prop_assert!(h.doc().operations.is_empty(), "the operation record is gone");
        prop_assert!(!h.in_scene(op), "…and so is its geometry");
        prop_assert_eq!(h.doc().nodes.len(), 2, "both sources remain");
        prop_assert_eq!(h.path_of(a), before_paths.0.clone());
        prop_assert_eq!(h.path_of(b), before_paths.1.clone());
        let after_nodes: Vec<(String, NodeId)> = h.doc().nodes.iter().map(|(k, v)| (v.name.clone(), *k)).collect();
        prop_assert_eq!(after_nodes, before_nodes);

        h.redo();
        prop_assert!(h.doc().operations.contains(op));
        prop_assert_eq!(h.path_of(op), op_path, "redo reproduces the same geometry byte for byte");
    }
}
