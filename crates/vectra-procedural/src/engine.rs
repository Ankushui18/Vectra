//! `ProceduralEngine` — the pass that runs last, and the table it publishes
//! (Task 7.0 RULE 2).
//!
//! ```text
//! settle:  primitives ──▶ operations ──▶ procedural ──▶ (re-read the slots
//!                                                            that read a port)
//! ```
//!
//! # Why the pass runs last
//!
//! Two of the five kinds read the *scene*: a `Source` node hands an authored
//! node's evaluated geometry into the graph. The operations pass therefore has
//! to have run first, or a `Source` reading a boolean result would see the
//! primitive that result replaced.
//!
//! # Why the evaluator reads a table rather than evaluating
//!
//! [`vectra_core::ProceduralEvaluator`] is handed an
//! [`vectra_core::EvaluationContext`], and that context carries no document, no
//! registry and no scene — by design, because it is *core's* context and every
//! resolver shares it. So this engine evaluates in the pass, where all three are
//! in hand, and publishes the **value ports** afterwards. `evaluate_float` /
//! `evaluate_point` / `evaluate_color` are then pure lookups:
//!
//! * resolution stays pure — a slot's value is a function of the document and
//!   the clock, exactly like a motion sample;
//! * a slot can never trigger a second evaluation, or recurse;
//! * the failure mode is loud and typed (`ProceduralPortUnavailable`,
//!   `ProceduralPortType`) instead of an ordering surprise.
//!
//! The one consequence is real and is handled by the engine owner: a slot that
//! reads a port was evaluated *before* this pass ran, so the settle path
//! re-reads exactly those slots afterwards (see `published` on
//! [`ProceduralEvaluation`]). That re-read terminates in one round precisely
//! because RULE 3 forbids a value reference back into the graph: the re-read
//! cannot dirty a procedural node, so there is nothing to iterate.
//!
//! # Incrementality (the Incrementality Law)
//!
//! A pass recomputes a node when, and only when:
//!
//! * its record changed since the last sync ([`RegistrySync::changed`]);
//! * it is named in the caller's seed set;
//! * it is a `Source` node whose subject was named in the seed set; or
//! * an upstream node in the chain recomputed.
//!
//! Everything else keeps the outputs it already had, and reruns happen in
//! topological order so a downstream consumer always reads a fresh upstream
//! value. There is no "re-evaluate everything" path: a full pass is simply a
//! seed set containing every node.

use crate::error::ProceduralError;
use crate::nodes::{self, NodeInputs};
use std::collections::{BTreeMap, BTreeSet};
use vectra_core::{
    Document, EvaluationContext, GeometryData, NodeId, NodeOutputId, PortId, ProceduralKind,
    ProceduralNode, ProceduralRegistry, Resolvable,
};
use vectra_geometry::{
    primitive_to_rings, Diagnostic, DiagnosticCode, EvaluatedNode, EvaluatedPrimitive,
    EvaluatedScene, EvaluatedStyle, Severity,
};

/// The flattening tolerance curves are sampled with when a `Source` node pulls
/// geometry into the graph. Deliberately the same value
/// `vectra_operations::convert::FLATTEN_TOLERANCE` uses: one tolerance for the
/// workspace, so a boolean and a noise displacement of the same circle agree.
pub const FLATTEN_TOLERANCE: f32 = 0.05;

/// What a registry sync found.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RegistrySync {
    /// Nodes whose record changed (new, edited, re-wired, re-operanded) — the
    /// pass must recompute these.
    pub changed: Vec<NodeId>,
    /// Nodes that left the document. Their values are dropped immediately, and
    /// their former consumers are recomputed (they now have a missing input).
    pub removed: Vec<NodeId>,
}

impl RegistrySync {
    pub fn is_empty(&self) -> bool {
        self.changed.is_empty() && self.removed.is_empty()
    }
}

/// What one pass produced.
#[derive(Debug, Default)]
pub struct ProceduralEvaluation {
    /// The geometry results to compose into the scene, keyed by their own
    /// `NodeId` (Task 4.0's RULE 3, applied to the procedural layer).
    pub nodes: BTreeMap<NodeId, EvaluatedNode>,
    /// Live nodes that recomputed and produced **no** geometry: their previous
    /// shape must leave the scene, or a failure would be invisible.
    pub retired: Vec<NodeId>,
    /// One note per node that could not be computed (Containment Law).
    pub diagnostics: Vec<Diagnostic>,
    /// Ids the pass actually recomputed, in evaluation order.
    pub recomputed: Vec<NodeId>,
    /// Value ports whose value changed in this pass — the slots the settle path
    /// re-reads so a procedural read is never a pass behind.
    pub published: Vec<NodeOutputId>,
}

impl ProceduralEvaluation {
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty() && self.retired.is_empty() && self.diagnostics.is_empty()
    }

    /// Fold this pass into the scene: compose the geometry results (which also
    /// prunes anything no longer live) and retire what failed.
    pub fn compose_into(&self, scene: &mut EvaluatedScene, doc: &Document) {
        scene.apply_operations(doc, self.nodes.values().cloned());
        scene.retire(&self.retired);
    }
}

/// The stateless-except-for-its-table procedural engine.
///
/// "Stateful" in the same limited sense as `MotionEngine`: it holds a copy of
/// the registry (so a pass never walks the document), the last successful
/// outputs, and the last diagnostics. Nothing it holds is *document state* — the
/// document stays the single source of truth, and a sync makes the copy follow.
#[derive(Debug, Default)]
pub struct ProceduralEngine {
    registry: ProceduralRegistry,
    /// Last successful outputs per node, every declared port.
    outputs: BTreeMap<NodeId, BTreeMap<PortId, GeometryData>>,
    diagnostics: Vec<Diagnostic>,
    /// Nodes whose record changed since the last sync.
    sync_changed: Vec<NodeId>,
}

impl ProceduralEngine {
    pub fn new() -> Self {
        Self::default()
    }

    /// The registry as of the last sync (tests, the inspector, the snapshot).
    pub fn registry(&self) -> &ProceduralRegistry {
        &self.registry
    }

    pub fn node_count(&self) -> usize {
        self.registry.len()
    }

    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    /// A node's last successful outputs, by port — the inspector's view of what
    /// a node produced (`scalar`, `center`, `tint`, …).
    pub fn outputs_of(&self, node: NodeId) -> Option<&BTreeMap<PortId, GeometryData>> {
        self.outputs.get(&node)
    }

    /// Adopt the document's registry, recording what changed.
    ///
    /// The `MotionEngine` precedent: called before every pass, so the copy can
    /// never drift. Consumers of a *removed* node are recomputed too — their
    /// wire is gone, and they must say so (a diagnostic), not keep a stale
    /// picture built from a node that no longer exists.
    pub fn sync_from_document(&mut self, doc: &Document) -> RegistrySync {
        let mut changed: Vec<NodeId> = Vec::new();
        let mut removed: Vec<NodeId> = Vec::new();

        for id in &self.registry.order {
            if !doc.procedural.contains(*id) {
                removed.push(*id);
            }
        }
        for (id, node) in &doc.procedural.nodes {
            match self.registry.get(*id) {
                Some(previous) if previous == node => {}
                _ => changed.push(*id),
            }
        }
        for id in &removed {
            changed.extend(
                self.registry
                    .incoming_wires(*id)
                    .into_iter()
                    .map(|w| w.node_id),
            );
        }
        changed.sort();
        changed.dedup();

        self.registry = doc.procedural.clone();
        for id in &removed {
            self.outputs.remove(id);
            self.diagnostics.retain(|d| d.node_id != Some(*id));
        }
        self.sync_changed = changed.clone();
        RegistrySync { changed, removed }
    }

    /// Run the pass.
    ///
    /// `seeds` are the dirty scene ids from the dependency graph. A node is
    /// recomputed when its record changed, when it is seeded, when it is a
    /// `Source` whose subject is seeded, or when an upstream node recomputed.
    pub fn evaluate(
        &mut self,
        scene: &EvaluatedScene,
        ctx: &EvaluationContext,
        seeds: &[NodeId],
    ) -> ProceduralEvaluation {
        let sync_changed = std::mem::take(&mut self.sync_changed);

        // 1. Seeds: the named ids, plus every `Source` node that reads one.
        let mut wanted: BTreeSet<NodeId> = BTreeSet::new();
        for id in seeds {
            if self.registry.contains(*id) {
                wanted.insert(*id);
            }
            wanted.extend(self.registry.sources_referencing(*id));
        }
        wanted.extend(
            sync_changed
                .into_iter()
                .filter(|id| self.registry.contains(*id)),
        );

        // 2. Downstream closure along the wires: an upstream recompute
        //    invalidates everything that consumes it.
        loop {
            let before = wanted.len();
            for node in self.registry.in_order() {
                if !wanted.contains(&node.id) && node.upstream().iter().any(|u| wanted.contains(u))
                {
                    wanted.insert(node.id);
                }
            }
            if wanted.len() == before {
                break;
            }
        }

        // 3. Evaluate in topological order (upstream first).
        let scope: Vec<NodeId> = wanted.iter().copied().collect();
        let order = self.registry.topological_order(&scope);
        let mut fresh: BTreeMap<NodeId, Option<BTreeMap<PortId, GeometryData>>> = BTreeMap::new();
        let mut diagnostics: Vec<Diagnostic> = Vec::new();
        let mut recomputed: Vec<NodeId> = Vec::new();

        for id in &order {
            let Some(node) = self.registry.get(*id) else {
                continue;
            };
            recomputed.push(*id);
            if !node.enabled {
                // Parked: keeps its record and its wiring, publishes nothing.
                fresh.insert(*id, None);
                continue;
            }
            match evaluate_node(node, &self.outputs, &fresh, scene, ctx) {
                Ok(outputs) => {
                    fresh.insert(*id, Some(outputs));
                }
                Err(error) => {
                    diagnostics.push(node_diagnostic(node, &error));
                    fresh.insert(*id, None);
                }
            }
        }

        // 4. Apply: untouched nodes keep exactly what they had (no rebuild).
        let mut published: Vec<NodeOutputId> = Vec::new();
        let mut retired: Vec<NodeId> = Vec::new();
        for id in &recomputed {
            let Some(result) = fresh.remove(id) else {
                continue;
            };
            match result {
                Some(outputs) => {
                    // A value port is "published" when its value changed — that
                    // is the set the settle path re-reads. Reporting unchanged
                    // ports would re-evaluate slots that cannot have moved.
                    let changed = self
                        .outputs
                        .get(id)
                        .map(|previous| *previous != outputs)
                        .unwrap_or(true);
                    if changed {
                        published.extend(value_ports(*id, &outputs));
                    }
                    self.outputs.insert(*id, outputs);
                }
                None => {
                    self.outputs.remove(id);
                    if self.registry.is_geometry_id(*id) {
                        retired.push(*id);
                    }
                }
            }
        }
        self.diagnostics.retain(|d| match d.node_id {
            Some(id) => !recomputed.contains(&id),
            None => false,
        });
        self.diagnostics.extend(diagnostics.iter().cloned());

        // 5. Compose geometry for the nodes that recomputed successfully — and
        //    only those; everything else is already in the scene cache.
        let mut nodes: BTreeMap<NodeId, EvaluatedNode> = BTreeMap::new();
        for id in &recomputed {
            if retired.contains(id) {
                continue;
            }
            let Some(node) = self.registry.get(*id) else {
                continue;
            };
            let Some(outputs) = self.outputs.get(id) else {
                continue;
            };
            if let Some(evaluated) = geometry_node(node, outputs, ctx) {
                nodes.insert(*id, evaluated);
            }
        }

        published.sort_by(|a, b| (a.node, &a.port).cmp(&(b.node, &b.port)));
        published.dedup();
        retired.sort();

        ProceduralEvaluation {
            nodes,
            retired,
            diagnostics,
            recomputed,
            published,
        }
    }
}

/// Evaluate one node: gather its wires and operands, then run its kind.
///
/// Reads upstream values from `previous` (last pass) first and `fresh` (this
/// pass) second, which is what lets a partial pass work without cloning the
/// whole graph: a node whose upstream just recomputed sees the new value, a node
/// whose upstream did not sees the one it always had.
fn evaluate_node(
    node: &ProceduralNode,
    previous: &BTreeMap<NodeId, BTreeMap<PortId, GeometryData>>,
    fresh: &BTreeMap<NodeId, Option<BTreeMap<PortId, GeometryData>>>,
    scene: &EvaluatedScene,
    ctx: &EvaluationContext,
) -> Result<BTreeMap<PortId, GeometryData>, ProceduralError> {
    let mut wires: BTreeMap<PortId, GeometryData> = BTreeMap::new();
    for (port, from) in &node.wires {
        let value = fresh
            .get(&from.node)
            .and_then(|maybe| maybe.as_ref())
            .or_else(|| previous.get(&from.node))
            .and_then(|outputs| outputs.get(&from.port))
            .cloned();
        match value {
            Some(value) => {
                wires.insert(port.clone(), value);
            }
            None => {
                return Err(ProceduralError::MissingInput(format!(
                    "{port} (upstream {} produced nothing)",
                    from.node
                )))
            }
        }
    }

    // A `Source` node reads the *scene*: the authored node's evaluated shape,
    // flattened into rings. The upstream is read, never touched (RULE 1 of
    // Task 4.0).
    if let Some(subject) = node.kind.source_node() {
        let Some(evaluated) = scene.get(subject) else {
            return Err(ProceduralError::SourceUnavailable(subject));
        };
        let rings = primitive_to_rings(&evaluated.primitive, FLATTEN_TOLERANCE);
        if rings.is_empty() {
            return Err(ProceduralError::EmptyResult);
        }
        wires.insert("geometry".to_string(), GeometryData::Region { rings });
    }

    let mut operands: BTreeMap<PortId, GeometryData> = BTreeMap::new();
    for port in node.kind.operands() {
        let Some(value) = node.operand(&port.name) else {
            return Err(ProceduralError::MissingOperand(port.name));
        };
        operands.insert(port.name.clone(), resolve_operand(&value, ctx)?);
    }

    let ports: Vec<PortId> = node.kind.outputs().into_iter().map(|p| p.name).collect();
    nodes::evaluate(&NodeInputs {
        kind: &node.kind,
        operands: &operands,
        wires: &wires,
        outputs: &ports,
    })
}

/// A `ParamValue` operand as `GeometryData`, resolved through the ordinary
/// parameter path (variables, expressions and motion all work here; a
/// procedural source is impossible — RULE 3 rejects it at the command boundary).
fn resolve_operand(
    value: &vectra_core::ParamValue,
    ctx: &EvaluationContext,
) -> Result<GeometryData, ProceduralError> {
    match value {
        vectra_core::ParamValue::Float(param) => param
            .resolve(ctx)
            .map(GeometryData::Scalar)
            .map_err(|error| ProceduralError::OperandUnresolved {
                port: "float".to_string(),
                message: error.to_string(),
            }),
        vectra_core::ParamValue::Point(param) => param
            .resolve(ctx)
            .map(GeometryData::Point)
            .map_err(|error| ProceduralError::OperandUnresolved {
                port: "point".to_string(),
                message: error.to_string(),
            }),
        vectra_core::ParamValue::Color(param) => param
            .resolve(ctx)
            .map(GeometryData::Color)
            .map_err(|error| ProceduralError::OperandUnresolved {
                port: "color".to_string(),
                message: error.to_string(),
            }),
    }
}

/// The node's **value** ports (published for resolution) — everything that is
/// not geometry. Geometry ports are composed into the scene instead.
fn value_ports(node: NodeId, outputs: &BTreeMap<PortId, GeometryData>) -> Vec<NodeOutputId> {
    outputs
        .iter()
        .filter(|(_, value)| !value.port_type().is_geometry())
        .map(|(port, _)| NodeOutputId::new(node, port.clone()))
        .collect()
}

/// The drawn result of a node, as a standard `EvaluatedNode` with a `Path`
/// primitive (Task 4.0 RULE 3) — or `None` for a kind with no geometry port.
fn geometry_node(
    node: &ProceduralNode,
    outputs: &BTreeMap<PortId, GeometryData>,
    ctx: &EvaluationContext,
) -> Option<EvaluatedNode> {
    let port = node.kind.geometry_port()?;
    let data = outputs.get(&port)?;
    let primitive = match data {
        GeometryData::Region { rings } => {
            EvaluatedPrimitive::Path(vectra_geometry::rings_to_path(rings))
        }
        GeometryData::Path { points, closed } => {
            EvaluatedPrimitive::Path(vectra_geometry::polyline_to_path(points, *closed))
        }
        GeometryData::Points(points) => {
            EvaluatedPrimitive::Path(vectra_geometry::polyline_to_path(points, false))
        }
        GeometryData::Scalar(_) | GeometryData::Point(_) | GeometryData::Color(_) => return None,
    };
    Some(EvaluatedNode {
        id: node.id,
        primitive,
        style: style_of(&node.style, ctx),
        // A procedural result belongs to no layer: it is visible and pickable
        // unless its *source* node says otherwise, and its own flags are the
        // defaults.
        visible: true,
        locked: false,
    })
}

/// Style resolution: the same fallbacks the primitive and operations evaluators
/// use, so a procedural result degrades instead of vanishing. The rule itself
/// lives in `vectra_geometry` (Task 10.2 RULE 3), one implementation for all
/// three evaluators.
fn style_of(
    style: &vectra_core::document::StyleProperties,
    ctx: &EvaluationContext,
) -> EvaluatedStyle {
    vectra_geometry::resolve_style(style, ctx)
}

/// One typed diagnostic for one failed node (Containment Law: local, named, and
/// never fatal).
fn node_diagnostic(node: &ProceduralNode, error: &ProceduralError) -> Diagnostic {
    let (code, severity) = match error {
        ProceduralError::MissingInput(_) | ProceduralError::MissingOperand(_) => {
            (DiagnosticCode::ProceduralMissingInput, Severity::Warning)
        }
        ProceduralError::EmptyResult => (DiagnosticCode::ProceduralEmptyResult, Severity::Warning),
        _ => (DiagnosticCode::ProceduralFailed, Severity::Warning),
    };
    Diagnostic {
        node_id: Some(node.id),
        property: Some(node.kind.tag().to_string()),
        severity,
        code,
        message: format!(
            "procedural {} {}: {error}",
            node.kind.tag(),
            short(&node.id)
        ),
    }
}

fn short(id: &NodeId) -> String {
    id.to_string().chars().take(8).collect()
}

/// The engine owner calls this before a pass; a `Source` node's subject may be
/// any live geometry id.
pub fn is_source_kind(kind: &ProceduralKind) -> bool {
    matches!(kind, ProceduralKind::Source { .. })
}

// ── The value door: a table lookup, never an evaluation (RULE 2) ─────────

impl ProceduralEngine {
    /// The published value at `output`, or a typed failure.
    fn lookup(&self, output: &NodeOutputId) -> Result<&GeometryData, vectra_core::ResolveError> {
        self.outputs
            .get(&output.node)
            .and_then(|ports| ports.get(&output.port))
            .ok_or_else(|| vectra_core::ResolveError::ProceduralPortUnavailable {
                node: output.node,
                port: output.port.clone(),
            })
    }
}

impl vectra_core::ProceduralEvaluator for ProceduralEngine {
    fn evaluate_float(
        &self,
        output: &NodeOutputId,
        _ctx: &EvaluationContext,
    ) -> Result<f64, vectra_core::ResolveError> {
        match self.lookup(output)? {
            GeometryData::Scalar(value) => Ok(*value),
            other => Err(vectra_core::ResolveError::ProceduralPortType {
                port: format!("{} • {}", short(&output.node), output.port),
                expected: "scalar",
                found: other.kind_tag(),
            }),
        }
    }

    fn evaluate_point(
        &self,
        output: &NodeOutputId,
        _ctx: &EvaluationContext,
    ) -> Result<vectra_core::Point2, vectra_core::ResolveError> {
        match self.lookup(output)? {
            GeometryData::Point(point) => Ok(*point),
            other => Err(vectra_core::ResolveError::ProceduralPortType {
                port: format!("{} • {}", short(&output.node), output.port),
                expected: "point",
                found: other.kind_tag(),
            }),
        }
    }

    fn evaluate_color(
        &self,
        output: &NodeOutputId,
        _ctx: &EvaluationContext,
    ) -> Result<vectra_core::Color, vectra_core::ResolveError> {
        match self.lookup(output)? {
            GeometryData::Color(color) => Ok(*color),
            other => Err(vectra_core::ResolveError::ProceduralPortType {
                port: format!("{} • {}", short(&output.node), output.port),
                expected: "color",
                found: other.kind_tag(),
            }),
        }
    }
}
