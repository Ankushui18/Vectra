//! Shared harness for the Task 2.2 laws.
//!
//! [`Harness`] drives a core [`Engine`] through **exactly the protocol
//! `vectra-wasm` uses**, minus the JSON hop: gate → dispatch → sync the
//! expression registry → sync the graph → derive dirty ids from the events.
//! Every step calls the same production function the WASM engine calls
//! (`gate_command`, `ExpressionEngine::sync_from_document`,
//! `DependencyGraph::sync`, `DependencyGraph::dirty_ids_for_events`), so a law
//! proven here is a statement about the shipped engine.

#![allow(dead_code)]

use std::collections::BTreeSet;
use vectra_core::Resolvable;
use vectra_core::{
    Command, Document, Engine, EngineEvent, NodeId, ParamValue, Parameter, VectraError,
};
use vectra_dependency::{
    derive, gate_command, DagEdge, DependencyGraph, GraphNode, IncrementalScene, ProspectiveEdges,
};
use vectra_expression::ExpressionEngine;
use vectra_geometry::{path_to_svg_data, EvaluatedPrimitive, EvaluatedScene};

/// Variables and expression sources the random driver draws from.
pub const VARIABLES: [&str; 3] = ["a", "b", "c"];
pub const SOURCES: [&str; 6] = [
    "$a * 2",
    "$b + 1",
    "$a + $c",
    "$time * 2",
    "clamp($a, 0, 100)",
    "$a / 2 + $b",
];

pub struct Harness {
    pub engine: Engine,
    pub graph: DependencyGraph,
    pub expressions: ExpressionEngine,
    pub cache: IncrementalScene,
    /// Cycle rejections observed (0 unless a hostile pattern appears).
    pub rejections: usize,
    /// Commands the engine refused (any typed error).
    pub failures: usize,
    pub steps: usize,
    /// Ops the planner declined because nothing was applicable in the current
    /// state (an empty document has no expression to redefine, and so on). A
    /// skip is a first-class outcome, not a failure.
    pub skips: usize,
    /// Nodes whose **presentation** flags moved on the last mutation — the
    /// harness's half of the renderer ledger that `settle` fills in production
    /// (Task 10.2 RULE 4). Empty for every value change.
    pub repainted: Vec<NodeId>,
}

impl Harness {
    pub fn new() -> Self {
        Self {
            engine: Engine::new(),
            graph: DependencyGraph::new(),
            expressions: ExpressionEngine::new(),
            cache: IncrementalScene::new(),
            rejections: 0,
            failures: 0,
            steps: 0,
            repainted: Vec::new(),
            skips: 0,
        }
    }

    /// The production mutation path.
    pub fn dispatch(&mut self, cmd: Command) -> Result<Vec<EngineEvent>, VectraError> {
        self.steps += 1;
        // Stage 1 (vectra-wasm order): expression sources are pre-validated, so
        // an unparsable source never reaches the document at all.
        if let Command::DefineExpression { source, .. } = &cmd {
            if let Err(e) = ExpressionEngine::check_source(source) {
                self.failures += 1;
                return Err(VectraError::command(format!("invalid expression: {e}")));
            }
        }
        // Stage 2: the dependency graph's cycle gate.
        if let Err(e) = gate_command(&self.graph, &cmd, self.engine.document()) {
            assert!(e.is_cycle_rejection(), "gate must only reject cycles: {e}");
            self.rejections += 1;
            self.failures += 1;
            return Err(e);
        }
        let events = match self.engine.dispatch(cmd) {
            Ok(events) => events,
            Err(e) => {
                self.failures += 1;
                return Err(e);
            }
        };
        self.settle();
        self.patch_scene(&events);
        Ok(events)
    }

    pub fn undo(&mut self) -> Result<Vec<EngineEvent>, VectraError> {
        self.steps += 1;
        if let Some(pending) = self.engine.peek_undo() {
            if let Err(e) = gate_command(&self.graph, pending, self.engine.document()) {
                self.rejections += 1;
                self.failures += 1;
                return Err(e);
            }
        }
        let events = match self.engine.undo() {
            Ok(events) => events,
            Err(e) => {
                self.failures += 1;
                return Err(e);
            }
        };
        self.settle();
        self.patch_scene(&events);
        Ok(events)
    }

    pub fn redo(&mut self) -> Result<Vec<EngineEvent>, VectraError> {
        self.steps += 1;
        if let Some(pending) = self.engine.peek_redo() {
            if let Err(e) = gate_command(&self.graph, pending, self.engine.document()) {
                self.rejections += 1;
                self.failures += 1;
                return Err(e);
            }
        }
        let events = match self.engine.redo() {
            Ok(events) => events,
            Err(e) => {
                self.failures += 1;
                return Err(e);
            }
        };
        self.settle();
        self.patch_scene(&events);
        Ok(events)
    }

    /// Registry + graph sync, exactly as the WASM engine does after a mutation.
    pub fn settle(&mut self) {
        self.expressions.sync_from_document(self.engine.document());
        self.graph.sync(self.engine.document());
    }

    /// Incremental scene patch driven by the *production* dirty derivation.
    pub fn patch_scene(&mut self, events: &[EngineEvent]) {
        let dirty = self.graph.dirty_ids_for_events(events);
        // Field-level destructuring: the context borrows the engine + registry,
        // while the cache is mutated — disjoint fields, no whole-`self` borrow.
        let Self {
            engine,
            expressions,
            cache,
            ..
        } = self;
        let ctx = engine.evaluation_context().with_expression(expressions);
        cache.refresh(engine.document(), &ctx, &dirty);
        // **RULE 4**, exactly as `vectra_wasm::VectraEngine::settle` does it: an
        // eye or a padlock is a presentation flag, not a value, so the evaluator
        // never sees it. The cached scene's flags are re-derived from the
        // document here and nowhere else, and the ids that actually moved are
        // recorded — that list is the renderer's ledger in production. When
        // nothing changed this is a no-op (an empty vec, no writes).
        let repainted = cache.scene_mut().refresh_presentation(engine.document());
        let _ = &repainted;
        self.repainted = repainted;
    }

    /// The ids whose *picture* changed on the last mutation, consuming the
    /// record — the harness's twin of the production render ledger.
    pub fn refresh_presentation(&mut self) -> Vec<NodeId> {
        let Self { engine, cache, .. } = self;
        let changed = cache.scene_mut().refresh_presentation(engine.document());
        self.repainted = changed.clone();
        changed
    }

    /// Throw the scene cache away and rebuild it in one full pass — the
    /// "re-evaluate everything" control, used by fixtures that swap a whole
    /// document in and by the law that pins patch ≡ rebuild.
    pub fn force_full_refresh(&mut self) -> vectra_dependency::EvalReport {
        self.cache.invalidate();
        let Self {
            engine,
            expressions,
            cache,
            ..
        } = self;
        let ctx = engine.evaluation_context().with_expression(expressions);
        cache.refresh_full(engine.document(), &ctx)
    }

    pub fn ctx(&self) -> vectra_core::EvaluationContext<'_> {
        self.engine
            .evaluation_context()
            .with_expression(&self.expressions)
    }

    pub fn doc(&self) -> &Document {
        self.engine.document()
    }

    pub fn edges(&self) -> Vec<DagEdge> {
        self.graph.edges()
    }

    pub fn node_ids(&self) -> Vec<NodeId> {
        self.engine.document().order.clone()
    }

    /// Resolve a float property through the full parametric path
    /// (literal / variable / expression / clock).
    pub fn resolve(&self, node: NodeId, property: &str) -> Result<f64, VectraError> {
        let node = self.engine.document().get_node(node)?;
        match node.get_param(property)? {
            ParamValue::Float(param) => Ok(param.resolve(&self.ctx())?),
            other => Err(VectraError::PropertyTypeMismatch {
                property: property.to_string(),
                expected: "float",
                got: other.kind(),
            }),
        }
    }

    /// The graph's derived truth, for equality assertions.
    pub fn derived(&self) -> (BTreeSet<GraphNode>, BTreeSet<DagEdge>) {
        let d = derive(self.engine.document());
        (d.nodes, d.edges)
    }

    /// Snapshot of everything that must not change when a command is rejected.
    pub fn fingerprint(&self) -> String {
        format!(
            "nodes={} edges={} undo={} redo={} vars={:?} exprs={:?}",
            self.graph.node_count(),
            self.graph.edge_count(),
            self.engine.can_undo(),
            self.engine.can_redo(),
            {
                let mut vars: Vec<(String, u64)> = self
                    .engine
                    .document()
                    .variables
                    .iter()
                    .map(|(k, v)| (k.clone(), v.to_bits()))
                    .collect();
                vars.sort();
                vars
            },
            {
                let mut exprs: Vec<(String, String)> = self
                    .engine
                    .document()
                    .expressions
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.source.clone()))
                    .collect();
                exprs.sort();
                exprs
            },
        )
    }

    /// The three invariants that must hold after *every* mutation.
    pub fn assert_invariants(&self) {
        let (nodes, edges) = self.derived();
        assert_eq!(
            self.graph.nodes().into_iter().collect::<BTreeSet<_>>(),
            nodes,
            "graph vertices drifted from the document"
        );
        assert_eq!(
            self.graph.edges().into_iter().collect::<BTreeSet<_>>(),
            edges,
            "graph edges drifted from the document"
        );
        assert_eq!(self.graph.validate(), Ok(()), "index/edge maps drifted");
        assert!(self.graph.is_acyclic(), "graph must never hold a cycle");
        for (from, to) in &edges {
            assert_ne!(from, to, "self dependency survived");
        }
    }
}

/// Deterministic, bit-exact fingerprint of a resolved paint stack.
///
/// Every channel that can differ between a patched cache and a rebuilt one has
/// to be in here — that is the whole point of the `patch ≡ rebuild` law. With
/// stacked appearances and gradients that now means: the number of layers, their
/// order, their kinds and widths, their opacity, blend mode and visibility, and
/// for a gradient its frame and every stop (offset by bit pattern, colour by
/// channel). Anything less and a stale gradient could pass the law.
pub fn style_fingerprint(style: &vectra_geometry::EvaluatedStyle) -> String {
    use vectra_geometry::{EvaluatedAppearanceKind, EvaluatedPaint};
    let mut out = format!("|o{:016x}", style.opacity.to_bits());
    for layer in &style.appearances {
        out.push_str(&format!(
            "|{}",
            match layer.kind {
                EvaluatedAppearanceKind::Fill => "fill".to_string(),
                EvaluatedAppearanceKind::Stroke { width } => format!("stroke{width:.6}"),
            }
        ));
        let paint = match &layer.paint {
            EvaluatedPaint::Solid(color) => format!(
                "solid{:02x}{:02x}{:02x}{:02x}",
                color.r, color.g, color.b, color.a
            ),
            EvaluatedPaint::Linear {
                start,
                end,
                gradient,
            } => format!(
                "linear{:.6},{:.6},{:.6},{:.6}{}",
                start.x,
                start.y,
                end.x,
                end.y,
                stops_fingerprint(&gradient.stops)
            ),
            EvaluatedPaint::Radial {
                center,
                radius,
                gradient,
            } => format!(
                "radial{:.6},{:.6},{:.6}{}",
                center.x,
                center.y,
                radius,
                stops_fingerprint(&gradient.stops)
            ),
        };
        out.push_str(&format!(
            "|{paint}|{:016x}|{}|v{}",
            layer.opacity.to_bits(),
            layer.blend.tag(),
            u8::from(layer.visible)
        ));
    }
    out
}

fn stops_fingerprint(stops: &[vectra_core::GradientStop]) -> String {
    let mut out = String::new();
    for stop in stops {
        out.push_str(&format!(
            "[{:016x}:{:02x}{:02x}{:02x}{:02x}]",
            stop.offset.to_bits(),
            stop.color.r,
            stop.color.g,
            stop.color.b,
            stop.color.a
        ));
    }
    out
}

/// Deterministic, bit-exact fingerprint of an evaluated scene.
///
/// `EvaluatedPrimitive` cannot derive `PartialEq` (`lyon::path::Path` has
/// none), so paths compare through their document-space SVG data — the same
/// projection the wire uses — and scalars compare by bit pattern.
pub fn scene_fingerprint(scene: &EvaluatedScene) -> String {
    let mut out = String::new();
    for id in &scene.z_order {
        out.push_str(&format!("{id}|"));
        match scene.nodes.get(id) {
            None => out.push_str("<missing>"),
            Some(node) => {
                out.push_str(&primitive_fingerprint(&node.primitive));
                out.push_str(&style_fingerprint(&node.style));
                // Presentation is part of the scene's identity (Task 10.2): a
                // rebuild that flips an eye and a patch that flips it have to
                // agree, and this fingerprint is what proves it.
                out.push_str(&format!(
                    "|v{}l{}",
                    u8::from(node.visible),
                    u8::from(node.locked)
                ));
            }
        }
        out.push('\n');
    }
    // Nodes present but not in z_order would be a scene-invariant violation;
    // surface them so the fingerprint can never hide one.
    let mut extra: Vec<String> = scene
        .nodes
        .keys()
        .filter(|id| !scene.z_order.contains(id))
        .map(ToString::to_string)
        .collect();
    extra.sort();
    for id in extra {
        out.push_str(&format!("extra:{id}\n"));
    }
    out
}

fn primitive_fingerprint(primitive: &EvaluatedPrimitive) -> String {
    match primitive {
        EvaluatedPrimitive::Rect {
            x,
            y,
            w,
            h,
            corner_radius,
        } => format!(
            "rect|{:016x}{:016x}{:016x}{:016x}{:016x}",
            x.to_bits(),
            y.to_bits(),
            w.to_bits(),
            h.to_bits(),
            corner_radius.to_bits()
        ),
        EvaluatedPrimitive::Circle { cx, cy, r } => format!(
            "circle|{:016x}{:016x}{:016x}",
            cx.to_bits(),
            cy.to_bits(),
            r.to_bits()
        ),
        EvaluatedPrimitive::Arc {
            cx,
            cy,
            r,
            start_angle,
            end_angle,
        } => format!(
            "arc|{:016x}{:016x}{:016x}{:016x}{:016x}",
            cx.to_bits(),
            cy.to_bits(),
            r.to_bits(),
            start_angle.to_bits(),
            end_angle.to_bits()
        ),
        EvaluatedPrimitive::Path(path) => format!("path|{}", path_to_svg_data(path)),
        // A run fingerprints through **its geometry and its placement**, not
        // through its text: two runs with the same glyphs in the same places
        // draw the same picture, and the incremental laws are laws about
        // pictures. Every number is bit-exact and every outline is its SVG
        // `d`, so a one-ulp drift in a glyph's rotation or an advance shows up
        // as a difference rather than being rounded away.
        EvaluatedPrimitive::Text(text) => {
            let metrics = &text.metrics;
            let mut out = format!(
                "text|{:016x}{:016x}{:016x}{:016x}{:016x}{}{}|{}",
                metrics.width.to_bits(),
                metrics.height.to_bits(),
                metrics.ascender.to_bits(),
                metrics.descender.to_bits(),
                metrics.line_advance.to_bits(),
                metrics.lines,
                text.glyphs.len(),
                path_to_svg_data(&text.outline)
            );
            for glyph in &text.glyphs {
                out.push_str(&format!(
                    "|{},{},{:016x}{:016x}{:016x}{}",
                    glyph.glyph_id,
                    glyph.cluster,
                    glyph.advance.to_bits(),
                    glyph.distance.to_bits(),
                    glyph.angle.to_bits(),
                    path_to_svg_data(&glyph.outline),
                ));
            }
            out
        }
    }
}

/// Properties that exist on each node kind (the driver picks from these).
pub fn props_for(kind_tag: &str) -> &'static [&'static str] {
    match kind_tag {
        "Rectangle" => &[
            "x",
            "y",
            "width",
            "height",
            "corner_radius",
            "style.opacity",
        ],
        "Circle" => &["cx", "cy", "radius", "style.opacity"],
        "Arc" => &[
            "cx",
            "cy",
            "radius",
            "start_angle",
            "end_angle",
            "style.opacity",
        ],
        _ => &["style.opacity"],
    }
}

/// One planned random step, interpreted against the *current* state so the
/// driver can never reference ids that do not exist.
#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)] // test-only generator: ops are planned one
                                     // at a time, so boxing the payload would
                                     // only add `*` noise at every match site.
pub enum RandomOp {
    Command(Command),
    Undo,
    Redo,
    /// Nothing applicable in the current state (e.g. no nodes to delete).
    Skip,
}

/// Decide the next step without applying it — this is what lets the fidelity
/// law compare "what the gate predicted" with "what the mutation did".
pub fn plan_random_op(h: &Harness, op: u8, arg: u8, extra: u8) -> RandomOp {
    let vars = VARIABLES;
    match op % 12 {
        0 | 1 => {
            let id = vectra_core::new_node_id();
            let kind = match op % 3 {
                0 => vectra_core::NodeKind::circle(
                    (arg % 8) as f64 * 10.0,
                    50.0,
                    (extra % 40) as f64 + 1.0,
                ),
                1 => vectra_core::NodeKind::rectangle(
                    (arg % 8) as f64 * 10.0,
                    10.0,
                    (extra % 60) as f64 + 1.0,
                    (arg % 30) as f64 + 1.0,
                ),
                _ => vectra_core::NodeKind::arc(0.0, 0.0, (extra % 30) as f64 + 1.0, 0.0, 1.5),
            };
            RandomOp::Command(Command::CreateNode {
                id,
                kind,
                name: Some(format!("n{}", h.node_ids().len())),
                index: None,
            })
        }
        2 | 3 => RandomOp::Command(Command::SetVariable {
            name: vars[arg as usize % vars.len()].to_string(),
            value: (extra as f64 % 25.0) - 5.0,
        }),
        4 => RandomOp::Command(Command::SetVariable {
            name: "orphan".to_string(),
            value: (extra % 10) as f64,
        }),
        5 | 6 => RandomOp::Command(Command::DefineExpression {
            id: vectra_core::new_expression_id(),
            source: SOURCES[arg as usize % SOURCES.len()].to_string(),
        }),
        7 => {
            let ids: Vec<_> = h.doc().expressions.keys().copied().collect();
            match ids.get(arg as usize % ids.len().max(1)) {
                Some(id) => RandomOp::Command(Command::DefineExpression {
                    id: *id,
                    source: SOURCES[extra as usize % SOURCES.len()].to_string(),
                }),
                None => RandomOp::Skip,
            }
        }
        8 | 9 => {
            let nodes = h.node_ids();
            match nodes.get(arg as usize % nodes.len().max(1)) {
                None => RandomOp::Skip,
                Some(node) => {
                    let tag = h.doc().get_node(*node).unwrap().kind.tag().to_string();
                    let props = props_for(&tag);
                    let property = props[extra as usize % props.len()].to_string();
                    let exprs: Vec<_> = h.doc().expressions.keys().copied().collect();
                    let value = match extra % 6 {
                        0 => ParamValue::float_literal((extra % 20) as f64),
                        1 => ParamValue::Float(Parameter::variable(
                            vars[arg as usize % vars.len()].to_string(),
                        )),
                        2 if !exprs.is_empty() => ParamValue::Float(Parameter::Expression(
                            exprs[arg as usize % exprs.len()],
                        )),
                        3 => ParamValue::Float(Parameter::Animated(
                            vectra_core::MotionBinding::StateDriven {
                                state: "on".to_string(),
                                true_value: Box::new(Parameter::Literal(1.0)),
                                false_value: Box::new(Parameter::Literal(0.0)),
                            },
                        )),
                        // Deliberately invalid: exercises the typed-failure path
                        // (and must be topology-neutral).
                        4 => {
                            let _ = props;
                            ParamValue::float_literal(1.0)
                        }
                        _ => ParamValue::float_literal(7.5),
                    };
                    let property = if extra % 6 == 4 {
                        "no_such_property".to_string()
                    } else {
                        property
                    };
                    RandomOp::Command(Command::SetParameter {
                        node_id: *node,
                        property,
                        value,
                    })
                }
            }
        }
        10 => {
            let nodes = h.node_ids();
            match nodes.get(arg as usize % nodes.len().max(1)) {
                Some(node) => RandomOp::Command(Command::DeleteNode { id: *node }),
                None => RandomOp::Skip,
            }
        }
        _ => {
            if extra % 2 == 0 {
                RandomOp::Undo
            } else {
                RandomOp::Redo
            }
        }
    }
}

/// Apply one random step through the production protocol.
pub fn apply_random_op(h: &mut Harness, op: u8, arg: u8, extra: u8) {
    match plan_random_op(h, op, arg, extra) {
        RandomOp::Command(cmd) => {
            let _ = h.dispatch(cmd);
        }
        RandomOp::Undo => {
            let _ = h.undo();
        }
        RandomOp::Redo => {
            let _ = h.redo();
        }
        RandomOp::Skip => {
            h.skips += 1;
        }
    }
}

/// Also exercised by the laws: the brief's "prospective vs. real" comparison.
pub fn delta_sets(delta: &ProspectiveEdges) -> (BTreeSet<DagEdge>, BTreeSet<DagEdge>) {
    (
        delta.adds.iter().cloned().collect(),
        delta.removes.iter().cloned().collect(),
    )
}
