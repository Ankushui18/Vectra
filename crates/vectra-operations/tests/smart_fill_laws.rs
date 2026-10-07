//! **Task 12.0's parametric laws** — the Smart Fill as a *node*, driven through
//! the real pipeline: `Engine::dispatch` → `GeometryEvaluator` →
//! `OperationsEvaluator`, exactly as `vectra-wasm` runs it.
//!
//! ```text
//!   Parametric Update   moving a boundary  →  the fill's geometry follows
//!   Span Break          BreakPath           →  closed pieces, one undo, no gaps
//! ```
//!
//! The parametric half cannot be tested on the region graph alone. "A fill is
//! an object living in a region, not a property of a path" (RULE 2) has two
//! claims in it, and they are checked separately here:
//!
//! * the boundary write **reaches** the fill — `Command::SetParameter`'s events
//!   name the boundary, and `Document::operations.affected_by` names the fill,
//!   which is what `vectra-wasm::run_operations` reads to pick its targets;
//! * the fill's **geometry is re-derived** — the path the pass publishes is the
//!   new arrangement's, not a stored copy and not the old region translated.
//!
//! Run: `cargo test -p vectra-operations --test smart_fill_laws`.

use proptest::prelude::*;
use vectra_core::{
    new_node_id, new_operation_id, Command, Engine, EngineEvent, NodeId, NodeKind, OperationId,
    ParamValue, Parameter, PathSegment, Point2,
};
use vectra_geometry::{
    path_area, path_to_svg_data, primitive_to_path, EvaluatedPrimitive, EvaluatedScene,
    GeometryEvaluator, RegionGraph, SourceSpec,
};
use vectra_operations::OperationsEvaluator;

// ── Harness ──────────────────────────────────────────────────────────────

/// A document plus the composed scene, kept in step the way `vectra-wasm` keeps
/// them: primitives first, then the operation layer on top.
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

    fn create(&mut self, kind: NodeKind, name: &str) -> NodeId {
        let id = new_node_id();
        self.engine
            .dispatch(Command::CreateNode {
                id,
                kind,
                name: Some(name.to_string()),
                index: None,
            })
            .expect("create");
        self.refresh();
        id
    }

    fn circle(&mut self, cx: f64, cy: f64, r: f64, name: &str) -> NodeId {
        self.create(NodeKind::circle(cx, cy, r), name)
    }

    /// A Smart Fill over `boundaries`, seeded at `seed` — the command RULE 4's
    /// drop and RULE 1's click both send.
    fn smart_fill(&mut self, boundaries: Vec<NodeId>, seed: (f64, f64)) -> OperationId {
        let id = new_operation_id();
        self.engine
            .dispatch(Command::CreateSmartFill {
                id,
                boundaries,
                seed,
                fill: Some(vectra_core::Color {
                    r: 232,
                    g: 98,
                    b: 44,
                    a: 255,
                }),
                name: Some("drop".to_string()),
            })
            .expect("the arrangement has a region at the seed");
        self.refresh();
        id
    }

    /// Re-run both passes. The operations pass is given the ids the last command
    /// dirtied, exactly as the wasm shell does — so "the fill follows a
    /// boundary" is asserted through the same selection logic the studio uses.
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
        let mut diagnostics = context.diagnostics;
        diagnostics.extend(evaluation.diagnostics.iter().cloned());
        self.diagnostics = diagnostics;
        evaluation.compose_into(&mut scene, self.engine.document());
        self.scene = scene;
    }

    /// The absolute path data of a node, if it is in the scene.
    fn drawn(&self, id: NodeId) -> Option<String> {
        self.scene.get(id).map(|node| match &node.primitive {
            EvaluatedPrimitive::Path(path) => path_to_svg_data(path),
            primitive => path_to_svg_data(&primitive_to_path(primitive)),
        })
    }

    /// The area of a node's evaluated geometry.
    fn area(&self, id: NodeId) -> f64 {
        let node = self.scene.get(id).expect("node in scene");
        path_area(&primitive_to_path(&node.primitive))
    }

    /// The region graph over the given boundaries, read off the *evaluated*
    /// scene — the same view `smart_fill_plan` builds.
    fn graph(&self, ids: &[NodeId]) -> RegionGraph {
        let sources: Vec<SourceSpec> = ids
            .iter()
            .filter_map(|id| {
                let node = self.scene.get(*id)?;
                SourceSpec::from_primitive(*id, &node.primitive)
            })
            .collect();
        RegionGraph::build(sources)
    }

    fn move_param(&mut self, id: NodeId, property: &str, value: f64) -> Vec<EngineEvent> {
        let events = self
            .engine
            .dispatch(Command::SetParameter {
                node_id: id,
                property: property.to_string(),
                value: ParamValue::Float(Parameter::Literal(value)),
            })
            .expect("a parameter write");
        self.refresh();
        events
    }

    fn undo(&mut self) {
        self.engine.undo().expect("undo");
        self.refresh();
    }
}

/// The analytic area of the lens of two radius-`r` circles whose centres are `d`
/// apart — the independent number the region graph is compared against.
fn lens_area(r: f64, d: f64) -> f64 {
    let half = d / 2.0;
    2.0 * r * r * (half / r).acos() - half * (4.0 * r * r - d * d).sqrt()
}

// ── Focused laws ─────────────────────────────────────────────────────────

#[test]
fn law_a_smart_fill_is_the_region_its_seed_is_in() {
    let mut h = Harness::new();
    let a = h.circle(0.0, 0.0, 100.0, "a");
    let b = h.circle(120.0, 0.0, 100.0, "b");
    let fill = h.smart_fill(vec![a, b], (60.0, 0.0));

    // The fill is a scene node with its own geometry…
    let drawn = h.drawn(fill).expect("the fill is drawn");
    assert!(drawn.ends_with('Z'), "a region is a closed path: {drawn}");
    // …and it *is* the lens: the analytic area, to the flattening's own error.
    let area = h.area(fill);
    let expected = lens_area(100.0, 120.0);
    assert!(
        (area - expected).abs() / expected < 0.02,
        "the fill's area {area} vs the analytic lens {expected}"
    );
    // The region graph agrees, from the same scene and a different code path.
    let graph = h.graph(&[a, b]);
    assert_eq!(graph.faces.len(), 3, "A only, B only and the lens");
    assert_eq!(
        graph.face_at((60.0, 0.0)),
        Some(
            graph
                .faces
                .iter()
                .position(|f| f.members == [true, true])
                .unwrap()
        )
    );

    // **RULE 4's paint**: the drop's colour is the fill's own appearance stack,
    // and the boundaries are untouched by it.
    let style = &h.scene.get(fill).expect("in scene").style;
    assert_eq!(style.appearances.len(), 1, "one row: the dropped fill");
    assert!(
        !style.first_fill().expect("a fill row").is_stroke(),
        "the row is a fill, not a stroke"
    );
    let boundary_style = &h.scene.get(a).unwrap().style;
    assert_ne!(
        style.first_fill().map(|layer| format!("{:?}", layer.paint)),
        boundary_style
            .first_fill()
            .map(|layer| format!("{:?}", layer.paint)),
        "the fill did not repaint its boundary"
    );
}

#[test]
fn law_removing_a_boundary_withdraws_the_fill_and_one_undo_restores_it() {
    let mut h = Harness::new();
    let a = h.circle(0.0, 0.0, 100.0, "a");
    let b = h.circle(120.0, 0.0, 100.0, "b");
    let fill = h.smart_fill(vec![a, b], (60.0, 0.0));
    assert!(h.drawn(fill).is_some());

    // A fill reads its boundaries' *geometry*: delete one and the region it was
    // pinned to is not a region any more, so the fill is withdrawn with it.
    h.engine
        .dispatch(Command::DeleteNode { id: b })
        .expect("delete a boundary");
    h.refresh();
    assert!(!h.engine.document().operations.contains(fill), "withdrawn");
    assert!(h.drawn(fill).is_none(), "and not drawn");

    h.undo();
    assert!(h.engine.document().operations.contains(fill), "restored");
    let id = h.engine.document().operations.get(fill).unwrap().id;
    assert_eq!(id, fill);
    assert!(h.drawn(fill).is_some(), "and drawn again");
}

#[test]
fn law_break_path_is_non_destructive_and_one_undo() {
    let mut h = Harness::new();
    let square = h.create(
        NodeKind::Path {
            start: Parameter::Literal(Point2::new(0.0, 0.0)),
            segments: vec![
                PathSegment::Line {
                    to: Parameter::Literal(Point2::new(200.0, 0.0)),
                },
                PathSegment::Line {
                    to: Parameter::Literal(Point2::new(200.0, 200.0)),
                },
                PathSegment::Line {
                    to: Parameter::Literal(Point2::new(0.0, 200.0)),
                },
                PathSegment::Close,
            ],
        },
        "square",
    );
    let bar = h.create(NodeKind::rectangle(100.0, 50.0, 200.0, 100.0), "bar");

    // The crossings: the bar's top and bottom edges meet the square's right edge.
    let graph = h.graph(&[square, bar]);
    let spans = graph.spans_of_source(square);
    assert_eq!(spans.len(), 2, "the bar crosses the square twice");
    let (from, to) = (spans[0].from, spans[0].to);
    let before = h.drawn(square).expect("drawn");
    let before_area = h.area(square);

    // Break at the span's own two cut points: the pieces RULE 3 asks for.
    let ring = vectra_geometry::source_rings(&h.scene, square);
    let (piece_a, piece_b) = vectra_geometry::ring_pieces_between(&ring[0], from, to);
    let (first, second) = (new_node_id(), new_node_id());
    let outline = |id: NodeId, name: &str, points: &[(f64, f64)]| vectra_core::OutlinePath {
        id,
        name: name.to_string(),
        start: Parameter::Literal(Point2::new(points[0].0, points[0].1)),
        segments: points[1..]
            .iter()
            .map(|(x, y)| PathSegment::Line {
                to: Parameter::Literal(Point2::new(*x, *y)),
            })
            .chain(std::iter::once(PathSegment::Close))
            .collect(),
    };
    h.engine
        .dispatch(Command::BreakPath {
            node_id: square,
            pieces: vec![
                outline(first, "square · 1", &piece_a),
                outline(second, "square · 2", &piece_b),
            ],
        })
        .expect("break");
    h.refresh();

    // 1. The source is hidden, not deleted, and nothing about it changed.
    let source = h.engine.document().get_node(square).expect("still there");
    assert!(!source.visible, "hidden");
    assert!(matches!(source.kind, NodeKind::Path { .. }), "still a path");
    // 2. Every piece is a closed path wearing the source's paint.
    for id in [first, second] {
        let node = h.engine.document().get_node(id).expect("a real node");
        match &node.kind {
            NodeKind::Path { segments, .. } => assert!(
                segments.iter().any(|s| matches!(s, PathSegment::Close)),
                "closed"
            ),
            other => panic!("a piece is a path, got {}", other.tag()),
        }
        assert_eq!(node.style, source.style, "same paint");
        assert!(h.drawn(id).expect("drawn").ends_with('Z'));
    }
    // 3. **No gaps**: the pieces' areas add back up to the source's.
    let total = h.area(first) + h.area(second);
    assert!(
        (total - before_area).abs() < 1e-6,
        "pieces {total} vs source {before_area}"
    );
    // 4. One undo, exactly as it was.
    h.undo();
    assert!(
        h.engine.document().get_node(square).unwrap().visible,
        "shown again"
    );
    assert!(
        !h.engine.document().nodes.contains_key(&first),
        "pieces gone"
    );
    assert_eq!(h.drawn(square).expect("drawn"), before, "byte for byte");
}

#[test]
fn law_a_smart_fill_can_be_repainted_through_the_ordinary_command() {
    // RULE 2: a fill has **its own Appearance stack**. The panel's paint write
    // is `SetAppearances`, which has to reach a *virtual* node's record — the
    // registry entry, not a `Node` in `doc.nodes`.
    let mut h = Harness::new();
    let a = h.circle(0.0, 0.0, 100.0, "a");
    let b = h.circle(120.0, 0.0, 100.0, "b");
    let fill = h.smart_fill(vec![a, b], (60.0, 0.0));

    // A stroke, not a fill: the point of the law is that the *whole stack* is
    // the record's, not that a fill can be recoloured.
    let stack = vec![vectra_core::style::AppearanceLayer::stroke(
        vectra_core::Color {
            r: 10,
            g: 20,
            b: 30,
            a: 255,
        },
        4.0,
    )];
    h.engine
        .dispatch(Command::SetAppearances {
            node_id: fill,
            appearances: stack.clone(),
        })
        .expect("a virtual node's paint is editable");
    h.refresh();
    assert_eq!(
        h.engine
            .document()
            .operations
            .get(fill)
            .unwrap()
            .style
            .appearances,
        stack
    );
    assert!(h.scene.get(fill).is_some(), "and the fill still draws");

    // Removing the operation and undoing restores the paint exactly — the
    // reason `ApplyOperation` carries `style`/`name` at all.
    h.engine
        .dispatch(Command::RemoveOperation { id: fill })
        .expect("remove");
    h.undo();
    assert_eq!(
        h.engine
            .document()
            .operations
            .get(fill)
            .unwrap()
            .style
            .appearances,
        stack,
        "the undo restored the paint, not a default"
    );
}

// ── The three named laws, as properties ──────────────────────────────────

proptest! {
    #![proptest_config(ProptestConfig { cases: 32, max_shrink_iters: 200, ..ProptestConfig::default() })]

    /// **The Parametric Update Law.** Moving a boundary re-derives the fill's
    /// geometry: the fill is among the dependents of the boundary (so the pass
    /// recomputes it), and the path it publishes is the *new* arrangement's.
    ///
    /// A fill that had stored its region would pass the first assertion and fail
    /// the second — which is exactly why both are here.
    #[test]
    fn prop_moving_a_boundary_updates_the_fill_geometry(
        r in 60.0f64..100.0,
        // The circles part by at most `0.45r`, so the seed — the midpoint of
        // their centres — is still inside *both* and the fill is still a lens.
        // Pulling them further apart is the other law's territory: there the
        // fill follows the seed into `a` alone.
        shift_frac in 0.05f64..0.45,
    ) {
        let mut h = Harness::new();
        let a = h.circle(0.0, 0.0, r, "a");
        let b = h.circle(r, 0.0, r, "b");
        // The seed is inside the lens of two radius-`r` circles `r` apart.
        let fill = h.smart_fill(vec![a, b], (r / 2.0, 0.0));
        let before = h.drawn(fill).expect("the lens draws");
        let before_area = h.area(fill);
        prop_assert!(
            (before_area - lens_area(r, r)).abs() / before_area < 0.02,
            "the first fill is the analytic lens: {before_area} vs {}",
            lens_area(r, r)
        );

        // Move B: the write names the boundary, and the operations layer names
        // the fill as a dependent — this is the dirtiness RULE 2 asks for.
        let moved = r * (1.0 + shift_frac);
        let events = h.move_param(b, "cx", moved);
        // The core publishes *what changed* (`NodesUpdated`); the `Dirty` the
        // UI sees is the shell's own wrapper (`vectra_wasm::dirty_event`),
        // built from this set plus the dependents below — so asserting on the
        // pair is asserting on exactly the ids that drive `run_operations`.
        let changed: Vec<NodeId> = events
            .iter()
            .flat_map(|event| match event {
                EngineEvent::NodesUpdated { ids } => ids.clone(),
                _ => Vec::new(),
            })
            .collect();
        prop_assert!(changed.contains(&b), "the boundary is named: {changed:?}");
        let affected = h.engine.document().operations.affected_by(&changed);
        prop_assert!(affected.contains(&fill), "the fill depends on its boundary");

        // …and the geometry is the *new* region.
        let after = h.drawn(fill).expect("the narrower lens still draws");
        prop_assert_ne!(&after, &before, "re-derived, not replayed");
        let after_area = h.area(fill);
        prop_assert!(
            (after_area - lens_area(r, moved)).abs() / after_area < 0.02,
            "the fill is the new lens: {after_area} vs {}",
            lens_area(r, moved)
        );
        prop_assert!(after_area < before_area, "two circles further apart make a smaller lens");
    }

    /// **The Parametric Update Law, the honest failure.** A fill is pinned to
    /// the *seed*: it paints the face the seed is in, whatever that face is now
    /// — moving one boundary away leaves the seed inside `a`, and the fill is
    /// `a`'s face rather than the vanished lens or a stale copy of it.
    ///
    /// And when *both* boundaries leave the seed in no face at all, the fill is
    /// **empty and says so**: it does not adopt the nearest region, keep the old
    /// geometry, or take the document down with it.
    #[test]
    fn prop_a_fill_whose_seed_leaves_every_face_reports_smart_fill_empty(
        r in 60.0f64..100.0,
        separation in 1.0f64..3.0,
    ) {
        let mut h = Harness::new();
        let a = h.circle(0.0, 0.0, r, "a");
        let b = h.circle(r, 0.0, r, "b");
        let fill = h.smart_fill(vec![a, b], (r / 2.0, 0.0));
        let lens = h.drawn(fill).expect("the lens draws");

        // The lens is gone, the seed is not: the circles are apart, the seed is
        // inside `a` alone, so the fill is `a`'s face — re-derived, not replayed.
        h.move_param(b, "cx", r * (1.0 + separation));
        let widened = h.drawn(fill).expect("the seed is still inside a");
        prop_assert_ne!(&widened, &lens, "the fill followed its seed");
        let (fill_area, a_area) = (h.area(fill), h.area(a));
        prop_assert!(
            (fill_area - a_area).abs() / a_area < 1e-6,
            "and that face is `a`: {fill_area} vs {a_area}"
        );

        // Take both boundaries off the seed: no face encloses it any more.
        h.move_param(a, "cx", -3.0 * r);
        prop_assert!(h.drawn(fill).is_none(), "no region, no geometry");
        prop_assert!(
            h.diagnostics
                .iter()
                .any(|d| d.code == vectra_geometry::DiagnosticCode::SmartFillEmpty),
            "the diagnostic names the reason: {:?}",
            h.diagnostics
        );
        // The boundaries are untouched: a vanished region never edits the shapes
        // that failed to make one.
        prop_assert!(h.drawn(a).is_some());
        prop_assert!(h.drawn(b).is_some());
    }

    /// **The Span Break Law.** Cutting a ring at one span's two crossings yields
    /// two valid, closed paths that tile the ring — asserted on the *filled*
    /// areas, which is the currency both halves of the pipeline agree on.
    #[test]
    fn prop_breaking_a_span_yields_two_closed_paths_with_no_gaps(
        side in 40.0f64..120.0,
        half_height in 5.0f64..20.0,
        offset in 5.0f64..30.0,
    ) {
        let mut h = Harness::new();
        let square = h.create(
            NodeKind::Path {
                start: Parameter::Literal(Point2::new(0.0, 0.0)),
                segments: vec![
                    PathSegment::Line { to: Parameter::Literal(Point2::new(side, 0.0)) },
                    PathSegment::Line { to: Parameter::Literal(Point2::new(side, side)) },
                    PathSegment::Line { to: Parameter::Literal(Point2::new(0.0, side)) },
                    PathSegment::Close,
                ],
            },
            "square",
        );
        // A bar overlapping the square's right edge, crossing it twice.
        let bar = h.create(
            NodeKind::rectangle(side - offset, side / 2.0 - half_height, side, half_height * 2.0),
            "bar",
        );

        let graph = h.graph(&[square, bar]);
        let spans = graph.spans_of_source(square);
        prop_assert_eq!(spans.len(), 2, "two crossings, two spans");
        let span = spans[0];
        let ring = vectra_geometry::source_rings(&h.scene, square);
        prop_assert!(!ring.is_empty());
        let (piece_a, piece_b) =
            vectra_geometry::ring_pieces_between(&ring[span.ring], span.from, span.to);
        prop_assert!(piece_a.len() >= 2 && piece_b.len() >= 2);

        // Both pieces start and end at the crossings — no gap between them: the
        // span runs cut→cut, the complementary arc runs cut→cut the other way,
        // so each piece's head is the other's tail.
        let near = |a: (f64, f64), b: (f64, f64)| {
            (a.0 - b.0).abs() < 1e-6 && (a.1 - b.1).abs() < 1e-6
        };
        let head_a = piece_a[0];
        let tail_a = *piece_a.last().unwrap();
        let head_b = piece_b[0];
        let tail_b = *piece_b.last().unwrap();
        prop_assert!(near(head_a, span.start), "the span starts at the first cut");
        prop_assert!(near(tail_a, span.end), "and ends at the second");
        prop_assert!(near(head_b, span.end), "the other arc starts where it ended");
        prop_assert!(near(tail_b, span.start), "and closes the ring at the first cut");

        // Their areas add up to the square's, and the pieces *tile* it: a
        // shoelace over the arcs, against the path area of the source.
        let shoelace = |points: &[(f64, f64)]| -> f64 {
            let mut twice = 0.0;
            for index in 0..points.len() {
                let p = points[index];
                let q = points[(index + 1) % points.len()];
                twice += p.0 * q.1 - q.0 * p.1;
            }
            twice.abs() / 2.0
        };
        let source_area = h.area(square);
        prop_assert!((shoelace(&piece_a) + shoelace(&piece_b) - source_area).abs() < 1e-6);
    }
}
