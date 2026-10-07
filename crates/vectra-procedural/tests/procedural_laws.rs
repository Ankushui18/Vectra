//! Task 7.0 — the seven laws, driven through the **production pipeline**.
//!
//! The lab below is the same sequence `vectra-wasm` runs, minus the JSON hop:
//!
//! ```text
//! commands ──▶ graph gate ──▶ Engine::dispatch ──▶ sync registries
//!          ──▶ dirty ids from events ──▶ primitive scene patch
//!          ──▶ procedural pass ──▶ compose ──▶ re-read the slots that read a port
//! ```
//!
//! Every step calls the function the shipped engine calls (`gate_command`,
//! `ExpressionEngine::sync_from_document`, `DependencyGraph::sync`,
//! `DependencyGraph::dirty_ids_for_events`, `IncrementalScene::refresh`,
//! `ProceduralEngine::evaluate`), so a law proven here is a statement about the
//! engine rather than about a test double.
//!
//! The seven laws: **Port Type**, **Acyclicity**, **Determinism**,
//! **Non-Destruction**, **Incrementality**, **Containment**, **Live-Id**.

use vectra_core::{
    new_node_id, Command, Document, Engine, EngineEvent, GeometryData, NodeId, NodeKind,
    NodeOutputId, ParamValue, Parameter, Point2, ProceduralKind, ProceduralNode, VectraError,
};
use vectra_dependency::{gate_command, DependencyGraph, GraphNode, IncrementalScene};
use vectra_expression::ExpressionEngine;
use vectra_geometry::{path_to_svg_data, primitive_to_path, Diagnostic, EvaluatedScene};
use vectra_operations::OperationsEvaluator;
use vectra_procedural::ProceduralEngine;

// ── the lab ────────────────────────────────────────────────────────────

/// How many rounds one settle may run the pass for — the same bound the engine
/// owner uses; see `VectraEngine::run_procedural_fixpoint` for the argument.
const MAX_PASS_ROUNDS: usize = 4;

struct Lab {
    engine: Engine,
    graph: DependencyGraph,
    expressions: ExpressionEngine,
    cache: IncrementalScene,
    operations: OperationsEvaluator,
    procedural: ProceduralEngine,
    /// The pass's last report (laws assert on it directly).
    recomputed: Vec<NodeId>,
    published: Vec<NodeOutputId>,
    diagnostics: Vec<Diagnostic>,
    /// Cycles the gate refused.
    rejections: usize,
}

impl Lab {
    fn new() -> Self {
        Self {
            engine: Engine::new(),
            graph: DependencyGraph::new(),
            expressions: ExpressionEngine::new(),
            cache: IncrementalScene::new(),
            operations: OperationsEvaluator::new(),
            procedural: ProceduralEngine::new(),
            recomputed: Vec::new(),
            published: Vec::new(),
            diagnostics: Vec::new(),
            rejections: 0,
        }
    }

    /// The production mutation path: gate, dispatch, sync, patch.
    fn dispatch(&mut self, cmd: Command) -> Result<Vec<EngineEvent>, VectraError> {
        if let Err(error) = gate_command(&self.graph, &cmd, self.engine.document()) {
            assert!(
                error.is_cycle_rejection(),
                "the gate must only reject cycles: {error}"
            );
            self.rejections += 1;
            return Err(error);
        }
        let events = self.engine.dispatch(cmd)?;
        self.settle();
        self.patch(&events);
        Ok(events)
    }

    fn undo(&mut self) -> Result<Vec<EngineEvent>, VectraError> {
        let events = self.engine.undo()?;
        self.settle();
        self.patch(&events);
        Ok(events)
    }

    fn redo(&mut self) -> Result<Vec<EngineEvent>, VectraError> {
        let events = self.engine.redo()?;
        self.settle();
        self.patch(&events);
        Ok(events)
    }

    /// Registry + graph sync — what the engine owner does after every mutation.
    fn settle(&mut self) {
        self.expressions.sync_from_document(self.engine.document());
        self.graph.sync(self.engine.document());
        self.procedural.sync_from_document(self.engine.document());
    }

    /// A full pass: every caller-visible id is dirty ("re-evaluate everything").
    fn force_full(&mut self) {
        let ids: Vec<NodeId> = self.engine.document().geometry_ids();
        self.recompute(&ids);
    }

    /// Patch the primitive scene, run the procedural pass, compose its geometry,
    /// then re-read the slots that read a value port — and run the pass again for
    /// the geometry that re-read moved (the settle fixpoint, RULE 2).
    fn recompute(&mut self, dirty: &[NodeId]) {
        let Self {
            engine,
            expressions,
            cache,
            operations,
            procedural,
            ..
        } = self;

        // 1. Primitives. A slot reading a port uses the table as of the last
        //    pass — correct for unchanged ports, and the re-read below fixes the
        //    ones that moved.
        {
            let ctx = engine
                .evaluation_context()
                .with_expression(expressions)
                .with_procedural(procedural);
            cache.refresh(engine.document(), &ctx, dirty);
        }

        // 2. Operations, exactly as the engine owner runs them (Task 4.0): the
        //    affected set is the registry scan, so a moved source re-runs its
        //    booleans and nothing else.
        {
            let pending: Vec<NodeId> = engine.document().operations.order.clone();
            let affected: Vec<NodeId> = engine.document().operations.affected_by(dirty);
            let mut targets = pending;
            for id in affected {
                if !targets.contains(&id) {
                    targets.push(id);
                }
            }
            let ctx = engine
                .evaluation_context()
                .with_expression(expressions)
                .with_procedural(procedural);
            let pass = operations.evaluate(engine.document(), cache.scene(), &ctx, &targets);
            pass.compose_into(cache.scene_mut(), engine.document());
        }

        // 3–5. RULE 2: the pass and the slots that read it are one fixpoint.
        //      A slot is resolved before the pass runs, so the readers of a
        //      republished port are re-read afterwards; that re-read moves
        //      *geometry*, which is what a `Source` node reads — so the pass runs
        //      again for the chain that reads the node the table just moved.
        //      A round only happens when a port's value changed, and the command
        //      boundary refuses a value reference back into the graph (RULE 3),
        //      so two rounds is the last a legal document can need.
        let mut round_dirty: Vec<NodeId> = dirty.to_vec();
        let mut recomputed: Vec<NodeId> = Vec::new();
        let mut published: Vec<NodeOutputId> = Vec::new();
        let mut notes: Vec<Diagnostic> = Vec::new();
        for round in 0..MAX_PASS_ROUNDS {
            // Operands resolve through expressions and motion; a *procedural*
            // operand is impossible (RULE 3), so the table itself is
            // deliberately absent from this context.
            let evaluation = {
                let ctx = engine.evaluation_context().with_expression(expressions);
                procedural.evaluate(cache.scene(), &ctx, &round_dirty)
            };
            evaluation.compose_into(cache.scene_mut(), engine.document());
            recomputed.extend(evaluation.recomputed.iter().copied());
            notes.extend(evaluation.diagnostics.iter().cloned());
            if evaluation.published.is_empty() {
                break;
            }
            published.extend(evaluation.published.iter().cloned());
            let readers = engine.document().procedural_readers(&evaluation.published);
            if readers.is_empty() {
                break;
            }
            {
                let ctx = engine
                    .evaluation_context()
                    .with_expression(expressions)
                    .with_procedural(procedural);
                cache.refresh(engine.document(), &ctx, &readers);
            }
            round_dirty = readers;
            assert!(
                round + 1 < MAX_PASS_ROUNDS,
                "the settle fixpoint did not converge"
            );
        }
        recomputed.dedup();
        published.sort_by(|a, b| (a.node, &a.port).cmp(&(b.node, &b.port)));
        published.dedup();
        self.recomputed = recomputed;
        self.published = published;
        // Diagnostics come from both layers: the geometry evaluator's (a slot
        // that cannot resolve) and the procedural pass's (a node that cannot
        // compute).
        self.diagnostics = cache.diagnostics().to_vec();
        self.diagnostics.extend(notes);
    }

    fn patch(&mut self, events: &[EngineEvent]) {
        let dirty = self.graph.dirty_ids_for_events(events);
        self.recompute(&dirty);
    }

    fn scene(&self) -> &EvaluatedScene {
        self.cache.scene()
    }

    fn document_json(&self) -> String {
        serde_json::to_string(self.engine.document()).expect("document serializes")
    }

    /// The drawn scene, as bytes: every node's geometry as SVG data, its style,
    /// and the draw order. Node **ids are deliberately excluded** — two engines
    /// built by the same commands mint different uuids, and the Determinism Law
    /// is about the picture, not about identity (identity has its own laws).
    fn scene_signature(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        for (index, id) in self.cache.scene().z_order.iter().enumerate() {
            let Some(node) = self.cache.scene().get(*id) else {
                continue;
            };
            let primitive = match &node.primitive {
                vectra_geometry::EvaluatedPrimitive::Rect { w, h, .. } => format!("rect {w}×{h}"),
                other => path_to_svg_data(&primitive_to_path(other)),
            };
            parts.push(format!(
                "#{index}|{primitive}|{:?}|{:.3}|{:.3}",
                node.style.first_fill().map(|l| l.paint.preview_color()),
                node.style.max_stroke_width(),
                node.style.opacity
            ));
        }
        parts.join("\n")
    }

    fn geometry_ids(&self) -> Vec<NodeId> {
        self.cache.scene().z_order.clone()
    }

    fn add_rect(&mut self, name: &str, x: f64, y: f64, w: f64, h: f64) -> NodeId {
        let id = new_node_id();
        self.dispatch(Command::CreateNode {
            id,
            kind: NodeKind::rectangle(x, y, w, h),
            name: Some(name.to_string()),
            index: None,
        })
        .expect("create rect");
        id
    }

    /// Add a procedural node of `kind` and return its id (and the pass's report,
    /// which the laws inspect).
    fn add_node(&mut self, kind: ProceduralKind) -> NodeId {
        let id = new_node_id();
        self.dispatch(Command::AddProceduralNode {
            node: ProceduralNode::new(id, kind),
        })
        .expect("add procedural node");
        id
    }

    fn wire(&mut self, node: NodeId, port: &str, from: NodeOutputId) {
        self.dispatch(Command::ConnectProcedural {
            node_id: node,
            port: port.to_string(),
            from,
        })
        .expect("wire");
    }

    fn out(node: NodeId, port: &str) -> NodeOutputId {
        NodeOutputId::new(node, port)
    }
}

fn grid_lab() -> (Lab, NodeId) {
    let mut lab = Lab::new();
    let grid = lab.add_node(ProceduralKind::grid(3.0, 2.0, 10.0, Point2::ZERO));
    (lab, grid)
}

// ── Law 1: Port Type ───────────────────────────────────────────────────

/// A port carries exactly one type. A slot that reads a *geometry* port is not
/// coerced — it fails with a typed error naming the port, and the node that
/// would have drawn is skipped, while its siblings are untouched.
#[test]
fn law_port_types_are_never_coerced() {
    let (mut lab, grid) = grid_lab();
    let rect = lab.add_rect("rect", 0.0, 0.0, 10.0, 10.0);

    // `span` is a Scalar port: a legal read.
    lab.dispatch(Command::SetParameter {
        node_id: rect,
        property: "width".to_string(),
        value: ParamValue::Float(Parameter::Procedural(Lab::out(grid, "span"))),
    })
    .expect("bind width to a scalar port");
    let width = match lab.scene().get(rect).map(|n| &n.primitive) {
        Some(vectra_geometry::EvaluatedPrimitive::Rect { w, .. }) => *w,
        other => panic!("expected a rect, got {other:?}"),
    };
    assert!((width - 30.0).abs() < 1e-9, "3 × 10 = 30, got {width}");

    // `region` is a Region port: reading it from a scalar slot is refused, with
    // the port and both types named.
    lab.dispatch(Command::SetParameter {
        node_id: rect,
        property: "width".to_string(),
        value: ParamValue::Float(Parameter::Procedural(Lab::out(grid, "region"))),
    })
    .expect("the command itself is legal: the port exists");
    let error = lab
        .diagnostics
        .iter()
        .find(|d| d.node_id == Some(rect))
        .unwrap_or_else(|| {
            panic!(
                "no diagnostic for the mismatched read: {:?}",
                lab.diagnostics
            )
        });
    assert!(
        error.message.contains("carries region"),
        "the diagnostic names the found type: {}",
        error.message
    );
    assert!(
        lab.scene().get(rect).is_none(),
        "a slot that cannot resolve draws nothing — no coercion, no zero"
    );

    // A wire between mismatched ports is refused *before* anything is stored:
    // the document is byte-identical afterwards.
    let smooth = lab.add_node(ProceduralKind::smooth(2.0, 0.5));
    let before = lab.document_json();
    let rejected = lab.dispatch(Command::ConnectProcedural {
        node_id: smooth,
        port: "region".to_string(),
        from: Lab::out(grid, "span"),
    });
    assert!(rejected.is_err(), "span → region must be refused");
    assert_eq!(
        before,
        lab.document_json(),
        "a rejected command leaves the document exactly as it was"
    );
}

// ── Law 2: Acyclicity ──────────────────────────────────────────────────

/// A wiring that would close a loop is rejected **before** the document, the
/// history and the graph change — including the disguised cycle (RULE 3), where
/// an operand reads a procedural output instead of wiring it.
#[test]
fn law_cycles_are_rejected_before_anything_changes() {
    let mut lab = Lab::new();
    let a = lab.add_node(ProceduralKind::smooth(1.0, 0.5));
    let b = lab.add_node(ProceduralKind::smooth(1.0, 0.5));
    lab.wire(b, "region", Lab::out(a, "region"));

    let document_before = lab.document_json();
    let depth_before = lab.engine.history_depth();
    let edges_before = lab.graph.edges().len();

    // b → a closes a → b → a.
    let closing = lab.dispatch(Command::ConnectProcedural {
        node_id: a,
        port: "region".to_string(),
        from: Lab::out(b, "region"),
    });
    let error = closing.expect_err("the chain is a DAG");
    assert!(error.is_cycle_rejection(), "{error}");
    assert_eq!(document_before, lab.document_json(), "document untouched");
    assert_eq!(depth_before, lab.engine.history_depth(), "no history entry");
    assert_eq!(edges_before, lab.graph.edges().len(), "no graph edge");
    // The graph's own gate saw it too (the same gate, one layer earlier).
    assert!(lab.rejections >= 1, "the prospective gate refused it");

    // The disguised cycle: an operand reading a procedural output.
    let disguised = lab.dispatch(Command::SetProceduralOperand {
        node_id: b,
        port: "iterations".to_string(),
        value: ParamValue::Float(Parameter::Procedural(Lab::out(a, "region"))),
    });
    assert!(disguised
        .expect_err("a value reference back into the graph")
        .is_cycle_rejection());
    assert_eq!(document_before, lab.document_json());
    assert_eq!(depth_before, lab.engine.history_depth());
}

// ── Law 3: Determinism ─────────────────────────────────────────────────

/// The same document always produces the same picture — however it got there:
/// built twice, replayed, undone and redone, or evaluated after a detour.
#[test]
fn law_the_scene_is_a_function_of_the_document() {
    fn build() -> Lab {
        let mut lab = Lab::new();
        let rect = lab.add_rect("rect", 0.0, 0.0, 40.0, 30.0);
        let source = lab.add_node(ProceduralKind::Source { node: rect });
        let noise = lab.add_node(ProceduralKind::noise(1.5, 0.1, 7.0));
        lab.wire(noise, "region", Lab::out(source, "region"));
        lab.dispatch(Command::SetProceduralOperand {
            node_id: noise,
            port: "amplitude".to_string(),
            value: ParamValue::Float(Parameter::Variable("amp".into())),
        })
        .expect("operand");
        lab.dispatch(Command::SetVariable {
            name: "amp".into(),
            value: 2.0,
        })
        .expect("variable");
        lab
    }

    let first = build();
    let second = build();
    assert_eq!(
        first.scene_signature(),
        second.scene_signature(),
        "two engines, one picture"
    );
    assert!(!first.scene_signature().is_empty(), "and it is not empty");

    // A detour: edit, then undo; the picture must come back byte-identical.
    let mut detoured = build();
    let picture = detoured.scene_signature();
    let noise = detoured
        .engine
        .document()
        .procedural
        .order
        .iter()
        .copied()
        .find(|id| {
            matches!(
                detoured
                    .engine
                    .document()
                    .procedural
                    .get(*id)
                    .map(|n| &n.kind),
                Some(ProceduralKind::Noise { .. })
            )
        })
        .expect("noise node");
    detoured
        .dispatch(Command::SetProceduralOperand {
            node_id: noise,
            port: "seed".to_string(),
            value: ParamValue::Float(Parameter::Literal(99.0)),
        })
        .expect("re-seed");
    assert_ne!(detoured.scene_signature(), picture, "the detour changed it");
    detoured.undo().expect("undo");
    assert_eq!(
        detoured.scene_signature(),
        picture,
        "and undo restored it exactly"
    );
    detoured.redo().expect("redo");
    assert_ne!(
        detoured.scene_signature(),
        picture,
        "and redo re-applies the detour it undid"
    );
    detoured.undo().expect("undo again");
    assert_eq!(
        detoured.scene_signature(),
        picture,
        "back to exactly the same picture"
    );

    // A full re-evaluation from scratch agrees with the incremental one.
    let incremental = detoured.scene_signature();
    detoured.cache.invalidate();
    detoured.force_full();
    assert_eq!(detoured.scene_signature(), incremental, "patch ≡ rebuild");
}

// ── Law 4: Non-Destruction ─────────────────────────────────────────────

/// Evaluation never writes. A pass mutates no slot, adds no history entry, and
/// leaves its `Source` nodes exactly as authored — RULE 1 of Task 4.0, now
/// enforced on a layer that reads whole shapes.
#[test]
fn law_evaluation_never_touches_the_document() {
    let mut lab = Lab::new();
    let rect = lab.add_rect("rect", 5.0, 5.0, 40.0, 30.0);
    let source = lab.add_node(ProceduralKind::Source { node: rect });
    let repeat = lab.add_node(ProceduralKind::repeat(3.0, 60.0, 0.0));
    lab.wire(repeat, "region", Lab::out(source, "region"));

    let document = lab.document_json();
    let depth = lab.engine.history_depth();
    let signature = lab.scene_signature();

    for _ in 0..5 {
        lab.force_full();
    }
    assert_eq!(document, lab.document_json(), "no slot moved");
    assert_eq!(depth, lab.engine.history_depth(), "no history entry");
    assert_eq!(
        signature,
        lab.scene_signature(),
        "and the picture is stable"
    );

    // The source node is untouched: its own parameters are what the user set.
    let node = lab.engine.document().get_node(rect).expect("source node");
    match &node.kind {
        NodeKind::Rectangle { x, width, .. } => {
            assert_eq!(*x, Parameter::Literal(5.0));
            assert_eq!(*width, Parameter::Literal(40.0));
        }
        other => panic!("expected a rectangle, got {other:?}"),
    }
}

// ── Law 5: Incrementality ──────────────────────────────────────────────

/// Editing one operand re-runs that node and its consumers, in topological
/// order, and nothing else. A mutation that changes nothing re-runs nothing.
#[test]
fn law_one_edit_recomputes_one_chain() {
    let mut lab = Lab::new();
    let rect = lab.add_rect("rect", 0.0, 0.0, 40.0, 30.0);
    let source = lab.add_node(ProceduralKind::Source { node: rect });
    let noise = lab.add_node(ProceduralKind::noise(1.0, 0.1, 1.0));
    let smooth = lab.add_node(ProceduralKind::smooth(1.0, 0.5));
    lab.wire(noise, "region", Lab::out(source, "region"));
    lab.wire(smooth, "region", Lab::out(noise, "region"));
    // A second, independent generator: it must not be dragged in.
    let lonely = lab.add_node(ProceduralKind::grid(
        2.0,
        2.0,
        25.0,
        Point2::new(200.0, 0.0),
    ));

    lab.force_full();
    let all: Vec<NodeId> = lab.recomputed.clone();
    assert_eq!(
        all.len(),
        4,
        "the full pass ran every *procedural* node (the rect is a primitive): {all:?}"
    );
    for id in [source, noise, smooth, lonely] {
        assert!(all.contains(&id), "…including {id}");
    }

    // Edit the *source rectangle*: the Source re-reads it, and the chain runs.
    let events = lab
        .dispatch(Command::SetParameter {
            node_id: rect,
            property: "width".to_string(),
            value: ParamValue::Float(Parameter::Literal(50.0)),
        })
        .expect("edit");
    assert!(!events.is_empty());
    assert_eq!(
        lab.recomputed,
        vec![source, noise, smooth],
        "exactly the chain, upstream first"
    );
    assert!(
        !lab.recomputed.contains(&lonely),
        "the unrelated generator did not run"
    );

    // Writing the *same* value again still names the node — the engine derives
    // dirt from events and compares no values (the operations pass has the same
    // property, for the same reason: a `Source` reads a whole shape, not a
    // slot). What the law bounds is *scope*: the same chain, never more.
    lab.dispatch(Command::SetParameter {
        node_id: rect,
        property: "width".to_string(),
        value: ParamValue::Float(Parameter::Literal(50.0)),
    })
    .expect("same value");
    assert_eq!(
        lab.recomputed,
        vec![source, noise, smooth],
        "a same-value write re-runs the same bounded chain"
    );
    assert!(!lab.recomputed.contains(&lonely), "and still nobody else");

    // A mutation with **no dependents at all** re-runs nothing: this variable is
    // read by nothing, so the graph's dirty set is empty and the pass does no
    // work — the "nothing to do costs nothing" half of the law.
    lab.dispatch(Command::SetVariable {
        name: "unread".into(),
        value: 1.0,
    })
    .expect("unread variable");
    assert!(
        lab.recomputed.is_empty(),
        "no dependents ⇒ no work: {:?}",
        lab.recomputed
    );
}

// ── Law 6: Containment ─────────────────────────────────────────────────

/// A node that cannot compute contributes no geometry, reports exactly one
/// diagnostic naming it, and leaves every sibling byte-identical.
#[test]
fn law_a_broken_node_cannot_take_the_scene_down() {
    let mut lab = Lab::new();
    let grid = lab.add_node(ProceduralKind::grid(2.0, 2.0, 20.0, Point2::ZERO));
    let healthy = lab.add_node(ProceduralKind::repeat(2.0, 60.0, 0.0));
    lab.wire(healthy, "region", Lab::out(grid, "region"));
    // A modifier with no wire at all: the normal state while authoring.
    let broken = lab.add_node(ProceduralKind::smooth(2.0, 0.5));

    let healthy_geometry = lab
        .scene()
        .get(healthy)
        .map(|node| path_to_svg_data(&primitive_to_path(&node.primitive)))
        .expect("the healthy node draws");

    assert!(
        lab.scene().get(broken).is_none(),
        "an unwired node contributes nothing"
    );
    let mine: Vec<&Diagnostic> = lab
        .diagnostics
        .iter()
        .filter(|d| d.node_id == Some(broken))
        .collect();
    assert_eq!(
        mine.len(),
        1,
        "exactly one diagnostic: {:?}",
        lab.diagnostics
    );
    assert_eq!(mine[0].code.tag(), "procedural-missing-input");
    assert!(
        lab.scene().get(healthy).is_some(),
        "the sibling kept drawing"
    );
    assert_eq!(
        healthy_geometry,
        path_to_svg_data(&primitive_to_path(
            &lab.scene().get(healthy).unwrap().primitive
        )),
        "and its bytes did not move"
    );

    // Wiring it up clears the diagnostic and makes it draw — no error, no
    // rebuild of its sibling.
    lab.wire(broken, "region", Lab::out(healthy, "region"));
    assert!(
        lab.diagnostics.iter().all(|d| d.node_id != Some(broken)),
        "the diagnostic is gone"
    );
    assert!(lab.scene().get(broken).is_some(), "and it now draws");
}

// ── Law 7: Live-Id (RULE 4 — the silent eviction trap) ─────────────────

/// A procedural result is scene geometry exactly while its node is enabled, and
/// **an unrelated operation pass cannot evict it**.
///
/// This is the trap the reconnaissance found: `apply_operations` ends by pruning
/// every cached id failing `Document::is_geometry_id`, so a drawn procedural
/// result that was not listed there would disappear the next time any operation
/// was composed — silently, one frame later.
#[test]
fn law_a_procedural_result_survives_a_neighbouring_operation() {
    let mut lab = Lab::new();
    let a = lab.add_rect("a", 0.0, 0.0, 40.0, 30.0);
    let b = lab.add_rect("b", 20.0, 10.0, 40.0, 30.0);
    let grid = lab.add_node(ProceduralKind::grid(
        2.0,
        2.0,
        15.0,
        Point2::new(100.0, 100.0),
    ));
    assert!(
        lab.scene().z_order.contains(&grid),
        "the procedural result draws"
    );

    // Now compose an unrelated boolean, which ends in `apply_operations` — the
    // call that prunes every cached id failing `Document::is_geometry_id`.
    let union = new_node_id();
    lab.dispatch(Command::ApplyOperation {
        id: union,
        kind: vectra_core::OperationKind::Boolean {
            op: vectra_core::BooleanOp::Union,
        },
        inputs: vec![a, b],
            style: None,
            name: None,
        })
    .expect("union");
    assert!(
        lab.scene().z_order.contains(&union),
        "the operation composed: {:?}",
        lab.geometry_ids()
    );
    assert!(
        lab.scene().z_order.contains(&grid),
        "…and the procedural result is still there (RULE 4): {:?}",
        lab.geometry_ids()
    );

    // Parking it retires the geometry; re-arming brings it back.
    lab.dispatch(Command::SetProceduralEnabled {
        id: grid,
        enabled: false,
    })
    .expect("park");
    assert!(!lab.scene().z_order.contains(&grid), "parked ⇒ not drawn");
    assert!(
        lab.scene().z_order.contains(&union),
        "the union is unaffected"
    );

    lab.dispatch(Command::SetProceduralEnabled {
        id: grid,
        enabled: true,
    })
    .expect("re-arm");
    // The re-arm is a **patch**, and a patch re-derives the draw order. That
    // path must ask the same question the composer asks (`geometry_ids()`, not
    // the authored `order`) or a live-but-virtual id falls out of `z_order`
    // while staying cached — present in `nodes`, invisible to the renderer.
    assert!(
        lab.scene().z_order.contains(&grid),
        "re-armed ⇒ drawn again: {:?}",
        lab.scene().z_order
    );
    assert!(
        lab.scene().z_order.contains(&union),
        "and the patch did not evict the operation pass's own result"
    );

    // Deleting it removes the geometry for good; undo restores both record and
    // picture.
    let picture = lab.scene_signature();
    lab.dispatch(Command::RemoveProceduralNode { id: grid })
        .expect("remove");
    assert!(!lab.scene().z_order.contains(&grid));
    lab.undo().expect("undo");
    assert_eq!(lab.scene_signature(), picture, "undo restored the picture");
}

// ── the value path, end to end ─────────────────────────────────────────

/// A slot reads a port, the port is republished, and the slot follows **in the
/// same settle** — the reason the engine owner re-reads the readers.
#[test]
fn a_slot_follows_a_republished_port_within_one_settle() {
    let mut lab = Lab::new();
    let rect = lab.add_rect("rect", 0.0, 0.0, 10.0, 10.0);
    let grid = lab.add_node(ProceduralKind::grid(2.0, 3.0, 10.0, Point2::ZERO));
    lab.dispatch(Command::SetParameter {
        node_id: rect,
        property: "width".to_string(),
        value: ParamValue::Float(Parameter::Procedural(Lab::out(grid, "span"))),
    })
    .expect("bind width to the grid's span");
    lab.force_full();

    let width = |lab: &Lab| match lab.scene().get(rect).map(|n| &n.primitive) {
        Some(vectra_geometry::EvaluatedPrimitive::Rect { w, .. }) => *w,
        other => panic!("expected a rect, got {other:?}"),
    };
    assert!((width(&lab) - 20.0).abs() < 1e-9, "2 × 10 = 20");

    // Change the spacing: the port republishes, and the slot follows *now* —
    // not one settle later.
    lab.dispatch(Command::SetProceduralOperand {
        node_id: grid,
        port: "spacing".to_string(),
        value: ParamValue::Float(Parameter::Variable("gap".into())),
    })
    .expect("operand");
    lab.dispatch(Command::SetVariable {
        name: "gap".into(),
        value: 25.0,
    })
    .expect("variable");
    assert!(
        (width(&lab) - 50.0).abs() < 1e-9,
        "2 × 25 = 50, got {}",
        width(&lab)
    );

    // And the *colour* door: a style slot reads the noise node's tint.
    let noise = lab.add_node(ProceduralKind::noise(1.0, 0.1, 5.0));
    lab.wire(noise, "region", Lab::out(grid, "region"));
    let tint = lab
        .procedural
        .outputs_of(noise)
        .and_then(|ports| ports.get("tint"))
        .and_then(GeometryData::as_color)
        .expect("the noise node publishes a colour");
    lab.dispatch(Command::SetParameter {
        node_id: rect,
        property: "style.fill".to_string(),
        value: ParamValue::Color(Parameter::Procedural(Lab::out(noise, "tint"))),
    })
    .expect("bind a colour slot");
    let fill = lab
        .scene()
        .get(rect)
        .and_then(|n| n.style.first_fill())
        .map(|layer| layer.paint.preview_color())
        .expect("rect");
    assert_eq!(
        fill, tint,
        "the style slot resolved through the colour port"
    );
}

/// …and the chain that reads *that slot* follows in the same settle.
///
/// The fixup above moves a slot, and a slot can be a whole **shape** as far as
/// the procedural layer is concerned: `⬡ source` reads `rect`, and there is no
/// graph edge between them (a geometry read is a registry scan, not an edge). So
/// the pass runs a second time, seeded with the reader the fixup moved. Without
/// that round the source keeps the old rectangle — still in the scene, still
/// *drawn*, one edit behind, which is how the web smoke found it: the incremental
/// picture stopped matching the rebuilt one.
#[test]
fn a_source_reading_a_reader_follows_in_the_same_settle() {
    let mut lab = Lab::new();
    let rect = lab.add_rect("rect", 0.0, 0.0, 10.0, 10.0);
    let grid = lab.add_node(ProceduralKind::grid(2.0, 3.0, 10.0, Point2::ZERO));
    lab.dispatch(Command::SetParameter {
        node_id: rect,
        property: "width".to_string(),
        value: ParamValue::Float(Parameter::Procedural(Lab::out(grid, "span"))),
    })
    .expect("bind width to the grid's span");
    let source = lab.add_node(ProceduralKind::Source { node: rect });
    lab.force_full();

    // The source's region *is* the rect, as a path — same shape, same order.
    let shapes = |lab: &Lab| {
        let path = |id: NodeId| {
            let node = lab.scene().get(id).expect("drawn");
            path_to_svg_data(&primitive_to_path(&node.primitive))
        };
        (path(rect), path(source))
    };
    let (rect_before, source_before) = shapes(&lab);
    assert_eq!(rect_before, "M0 0L20 0L20 10L0 10Z", "2 × 10 = 20 wide");
    assert_eq!(
        source_before, rect_before,
        "…and the source reads the rect as it is"
    );

    // Move the table — an *operand* edit, not a slot edit. The rect is a reader
    // of the port (the fixup handles that one), and the source is a reader of the
    // rect (only the second round handles that).
    lab.dispatch(Command::SetProceduralOperand {
        node_id: grid,
        port: "spacing".to_string(),
        value: ParamValue::Float(Parameter::Literal(25.0)),
    })
    .expect("operand");
    let (rect_after, source_after) = shapes(&lab);
    assert_eq!(rect_after, "M0 0L50 0L50 10L0 10Z", "2 × 25 = 50");
    assert_ne!(
        rect_after, rect_before,
        "the operand edit moved the rect (the fixup round)"
    );
    assert_eq!(
        source_after, rect_after,
        "…and the source followed it without waiting for a rebuild"
    );
}

/// The pass's report is stable and ordered, so the UI can diff it.
#[test]
fn the_published_table_is_deterministic_and_typed() {
    let (mut lab, grid) = grid_lab();
    lab.force_full();
    let outputs = lab
        .procedural
        .outputs_of(grid)
        .expect("the grid published its ports");
    assert_eq!(outputs.len(), 4, "region, points, center, span");
    assert_eq!(outputs["span"], GeometryData::Scalar(30.0));
    assert_eq!(
        outputs["center"],
        GeometryData::Point(Point2::new(15.0, 10.0))
    );
    assert_eq!(outputs["points"].vertices().len(), 12);

    // A port that does not exist is a typed failure, not a zero.
    let missing = Lab::out(grid, "nope");
    let error = lab
        .engine
        .document()
        .procedural
        .get(grid)
        .expect("node")
        .kind
        .output_type("nope");
    assert!(error.is_none());
    assert_eq!(
        lab.procedural.diagnostics().len(),
        0,
        "and the pass is quiet about a healthy graph: {:?}",
        lab.procedural.diagnostics()
    );
    let _ = missing;
}

/// A document round-trips through JSON with its graph intact — the wire form the
/// remote control and (Task 8.0) the export layer will both use.
#[test]
fn the_graph_round_trips_through_json() {
    let (mut lab, grid) = grid_lab();
    let smooth = lab.add_node(ProceduralKind::smooth(2.0, 0.5));
    lab.wire(smooth, "region", Lab::out(grid, "region"));

    let json = serde_json::to_string(lab.engine.document()).expect("serialize");
    let restored: Document = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(restored.procedural.len(), 2);
    let node = restored.procedural.get(smooth).expect("wired node");
    assert_eq!(node.wires["region"], Lab::out(grid, "region"));
    assert_eq!(
        serde_json::to_string(&restored).expect("reserialize"),
        json,
        "stable round trip"
    );
    // And a document *without* the field still loads (serde default).
    let legacy = json.replace("\"procedural\":", "\"ignored_procedural\":");
    let parsed: Document = serde_json::from_str(&legacy).expect("legacy document");
    assert!(parsed.procedural.is_empty());
}

/// The graph vertex exists for a port, and a slot that reads it depends on it —
/// the dependency-layer half of the value path.
#[test]
fn a_slot_reading_a_port_depends_on_it_in_the_graph() {
    let (mut lab, grid) = grid_lab();
    let rect = lab.add_rect("rect", 0.0, 0.0, 10.0, 10.0);
    lab.dispatch(Command::SetParameter {
        node_id: rect,
        property: "width".to_string(),
        value: ParamValue::Float(Parameter::Procedural(Lab::out(grid, "span"))),
    })
    .expect("bind");

    let property = GraphNode::GeometryProperty(rect, "width".to_string());
    let port = GraphNode::Procedural {
        node: grid,
        port: "span".to_string(),
    };
    assert!(
        lab.graph
            .edges()
            .contains(&(property.clone(), port.clone())),
        "width → ⬡ grid • span: {:?}",
        lab.graph.out_edges(&property)
    );

    // Editing a variable that feeds the node's *operand* dirties the slot.
    lab.dispatch(Command::SetProceduralOperand {
        node_id: grid,
        port: "spacing".to_string(),
        value: ParamValue::Float(Parameter::Variable("gap".into())),
    })
    .expect("operand");
    lab.dispatch(Command::SetVariable {
        name: "gap".into(),
        value: 12.0,
    })
    .expect("variable");
    assert!(
        lab.graph.edges().contains(&(
            GraphNode::Procedural {
                node: grid,
                port: "span".to_string()
            },
            GraphNode::Variable("gap".to_string())
        )),
        "the port depends on the variable its operand reads"
    );
    let width = match lab.scene().get(rect).map(|n| &n.primitive) {
        Some(vectra_geometry::EvaluatedPrimitive::Rect { w, .. }) => *w,
        other => panic!("expected a rect, got {other:?}"),
    };
    assert!((width - 36.0).abs() < 1e-9, "3 × 12 = 36, got {width}");
}
