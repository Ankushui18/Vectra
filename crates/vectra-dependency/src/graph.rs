//! The dependency graph: topology, cycle gate, and dirty propagation (MES §8).
//!
//! # The join
//! Every parametric arrow in the document is one edge:
//!
//! ```text
//! GeometryProperty(node, "width") ──▶ Expression(ƒx) ──▶ Variable("base") ──▶ (value)
//! GeometryProperty(node, "radius") ──▶ Variable("base")
//! GeometryProperty(node, "y")      ──▶ Variable("time")   ← the engine clock
//! ```
//!
//! Edges are read **"from depends on to"** (the MES §8 sketch's `depends_on`).
//! Mutating a node therefore dirties everything reachable along *out*-edges:
//! `Variable("base")` → the expressions that read it → the geometry that binds
//! them.
//!
//! # One derivation, one gate
//! [`DependencyGraph::sync`] is the only thing that ever adds or removes graph
//! state, and it always derives that state from the document itself
//! ([`derive`] — the same function tests use as an oracle). Commands therefore
//! cannot drift the graph: undo/redo/deserialize are all just "the document
//! changed, re-derive". [`DependencyGraph::dry_run`] is the corresponding
//! *pre*-mutation gate: the engine dry-runs the edges a command would add on a
//! scratch copy and rejects the command with
//! [`VectraError::CyclicDependency`](vectra_core::VectraError::CyclicDependency)
//! if they would close a cycle, so the real graph and the document are never
//! touched by an invalid command.
//!
//! # Why [`StableGraph`]
//! Removing a node keeps every other node/edge index valid (deleted slots
//! become holes). Undo/redo of graph elements — delete a rectangle, undo, bind
//! an expression, redo — therefore never invalidates the indices the dirty
//! traversal is holding, which is what makes the incremental path safe.

use petgraph::algo::{has_path_connecting, is_cyclic_directed};
use petgraph::stable_graph::{EdgeIndex, NodeIndex, StableGraph};
use petgraph::Direction;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use vectra_core::{
    Document, EngineEvent, ExpressionId, NodeId, Parameter, VariableId, VectraError,
};
use vectra_expression::{ExpressionEngine, TIME_VARIABLE};
use vectra_geometry::DirtySet;

/// One vertex of the dependency graph (MES §8 / Task 2.2).
///
/// `GeometryProperty` is `(node, property)` rather than a `NodeId` because a
/// single node can have several independently-driven slots: `width` may read
/// `$base` while `height` reads an expression and `opacity` reads the clock.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum GraphNode {
    /// A global scalar variable (the clock is `Variable("time")`).
    Variable(VariableId),
    /// A compiled expression (MES §7), keyed by the same id core persists.
    Expression(ExpressionId),
    /// One parametric `f64` slot of one node: `(node, "width")`.
    GeometryProperty(NodeId, String),
    /// A live state flag read by a motion binding (Task 6.0): `hover`,
    /// `active`, …. An input rather than document state, but a *precise* one —
    /// flipping `hover` dirties exactly the slots that read `hover`, which is
    /// what keeps `set_state` as surgical as every other mutation.
    State(String),
    /// A keyframe track (Task 6.0). Editing a track dirties exactly the slots
    /// that sample it, so the timeline needs no "re-evaluate everything".
    Track(VariableId),
    /// One output port of one procedural node (MES §11, Task 7.0):
    /// `(node, "scalar")`. The vertex exists so a slot that reads a port is a
    /// *real* dependency — `Parameter::Procedural` used to be silent here,
    /// which would have left a slot holding last pass's number with nothing
    /// dirtying it.
    ///
    /// Chain edges (`port → upstream port`) and operand edges
    /// (`port → $var`) are derived too, so the same cycle gate that protects
    /// expressions and motion protects the procedural chain — one gate, one
    /// error type, three graphs.
    Procedural { node: NodeId, port: String },
}

/// MES §8 spells the vertex type `DependencyNode`; Task 2.2 names it
/// `GraphNode`. Same type, both names.
pub type DependencyNode = GraphNode;

/// Edge payload (MES §8 sketch: `DependencyEdge`). Unweighted in Phase 1:
/// every edge means exactly "from depends on to".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct DependencyEdge;

/// A directed dependency edge (`from` depends on `to`).
pub type DagEdge = (GraphNode, GraphNode);

impl GraphNode {
    /// The engine clock, addressable like any other variable (see
    /// [`DependencyGraph::clock`] for the conflation note).
    pub fn clock() -> Self {
        Self::Variable(TIME_VARIABLE.to_string())
    }

    /// Stable lowercase tag for wire/JSON.
    pub fn tag(&self) -> &'static str {
        match self {
            Self::Variable(_) => "variable",
            Self::Expression(_) => "expression",
            Self::GeometryProperty(_, _) => "property",
            Self::State(_) => "state",
            Self::Track(_) => "track",
            Self::Procedural { .. } => "procedural",
        }
    }

    /// Short human label for logs and cycle errors (`"$base"`, `"ƒx 3f2a1b7c"`,
    /// `"1f0c9d2a • width"`).
    pub fn label(&self) -> String {
        match self {
            Self::Variable(name) => format!("${name}"),
            Self::Expression(id) => format!("ƒx {}", short_id(&id.to_string())),
            Self::GeometryProperty(node, property) => {
                format!("{} • {property}", short_id(&node.to_string()))
            }
            Self::State(flag) => format!("◉ {flag}"),
            Self::Track(id) => format!("♪ {id}"),
            Self::Procedural { node, port } => {
                format!("⬡ {} • {port}", short_id(&node.to_string()))
            }
        }
    }

    /// Stable machine key for the wire (`"var:base"`, `"expr:<uuid>"`,
    /// `"prop:<uuid>:width"`, `"state:hover"`, `"track:<id>"`). Unique per
    /// vertex, cheap to compare in JS.
    pub fn key(&self) -> String {
        match self {
            Self::Variable(name) => format!("var:{name}"),
            Self::Expression(id) => format!("expr:{id}"),
            Self::GeometryProperty(node, property) => format!("prop:{node}:{property}"),
            Self::State(flag) => format!("state:{flag}"),
            Self::Track(id) => format!("track:{id}"),
            Self::Procedural { node, port } => format!("proc:{node}:{port}"),
        }
    }

    /// The geometry id this vertex dirties, if any.
    ///
    /// A procedural port reports its **node's** id: a geometry-producing node
    /// is a scene id in its own right (Task 7.0 RULE 4), so dirtying any of its
    /// ports dirties the thing the renderer draws — and the same id is what the
    /// procedural pass reads as its seed set.
    pub fn node_id(&self) -> Option<NodeId> {
        match self {
            Self::GeometryProperty(node, _) | Self::Procedural { node, .. } => Some(*node),
            _ => None,
        }
    }
}

/// First 8 hex characters of a uuid — enough to disambiguate in a log line,
/// short enough to keep cycle errors readable.
fn short_id(id: &str) -> &str {
    if id.len() >= 8 {
        &id[..8]
    } else {
        id
    }
}

/// Result of a full derivation pass: what the document *implies*.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DerivedGraph {
    pub nodes: BTreeSet<GraphNode>,
    pub edges: BTreeSet<DagEdge>,
    /// Non-fatal derivation findings (unparsable sources).
    pub problems: Vec<GraphProblem>,
}

impl DerivedGraph {
    fn insert_edge(&mut self, from: GraphNode, to: GraphNode) {
        self.nodes.insert(from.clone());
        self.nodes.insert(to.clone());
        self.edges.insert((from, to));
    }
}

/// A derivation/reconciliation finding the UI should know about. Never an
/// error: the graph stays usable (and total) even against a hand-built or
/// migrated document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GraphProblem {
    /// `Document.expressions` holds a source the compiler rejects (only
    /// reachable via out-of-band mutation — commands are pre-validated).
    UnparsableExpression { id: ExpressionId, message: String },
    /// `sync` refused to add an edge because it would close a cycle. This
    /// cannot happen for a document reached through commands (the gate runs
    /// first); it is recorded instead of panicking so the engine stays total.
    RejectedEdge {
        from: GraphNode,
        to: GraphNode,
        message: String,
    },
}

impl GraphProblem {
    pub fn message(&self) -> String {
        match self {
            Self::UnparsableExpression { id, message } => {
                format!("expression {id} does not compile: {message}")
            }
            Self::RejectedEdge { from, to, message } => {
                format!("edge {} → {} rejected: {message}", from.label(), to.label())
            }
        }
    }
}

/// What changed during one [`DependencyGraph::sync`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GraphSyncReport {
    pub nodes_added: usize,
    pub nodes_removed: usize,
    pub edges_added: usize,
    pub edges_removed: usize,
    pub problems: Vec<GraphProblem>,
}

impl GraphSyncReport {
    /// True ⟺ the topology changed at all (vs. a no-op reconciliation).
    pub fn changed(&self) -> bool {
        self.nodes_added + self.nodes_removed + self.edges_added + self.edges_removed > 0
    }

    /// True ⟺ no findings (the normal case).
    pub fn is_clean(&self) -> bool {
        self.problems.is_empty()
    }
}

/// Compact summary for snapshots/events.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct GraphSummary {
    pub nodes: usize,
    pub edges: usize,
    pub acyclic: bool,
}

/// Structural self-check failures (used by tests and `debug_assert`s).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum GraphViolation {
    #[error("graph holds {graph_edges} edges but {map_edges} are tracked")]
    EdgeCount {
        graph_edges: usize,
        map_edges: usize,
    },
    #[error("graph holds {graph_nodes} nodes but {indexed_nodes} are indexed")]
    NodeCount {
        graph_nodes: usize,
        indexed_nodes: usize,
    },
    #[error("index map key {node} points at a slot holding {found:?}")]
    WeightMismatch { node: String, found: Option<String> },
    #[error("tracked edge {from} → {to} is stale")]
    StaleEdgeIndex { from: String, to: String },
    #[error("graph contains a cycle")]
    Cyclic,
}

/// The parametric dependency graph (MES §8).
///
/// Invariant (checked by [`DependencyGraph::validate`] and asserted by the
/// property tests after *every* mutation): `graph` is acyclic, the `index` map
/// is a bijection onto live nodes, and the `edges` map is a bijection onto
/// live edges.
#[derive(Debug, Clone, Default)]
pub struct DependencyGraph {
    graph: StableGraph<GraphNode, DependencyEdge>,
    index: HashMap<GraphNode, NodeIndex>,
    edges: HashMap<DagEdge, EdgeIndex>,
}

impl DependencyGraph {
    pub fn new() -> Self {
        Self::default()
    }

    // ── Inspection ─────────────────────────────────────────────────────

    pub fn node_count(&self) -> usize {
        self.index.len()
    }

    pub fn edge_count(&self) -> usize {
        self.edges.len()
    }

    pub fn is_empty(&self) -> bool {
        self.index.is_empty()
    }

    pub fn contains_node(&self, node: &GraphNode) -> bool {
        self.index.contains_key(node)
    }

    pub fn contains_edge(&self, from: &GraphNode, to: &GraphNode) -> bool {
        self.edges.contains_key(&(from.clone(), to.clone()))
    }

    pub fn node_index(&self, node: &GraphNode) -> Option<NodeIndex> {
        self.index.get(node).copied()
    }

    /// All vertices, sorted (deterministic for tests and JSON).
    pub fn nodes(&self) -> Vec<GraphNode> {
        let mut out: Vec<GraphNode> = self.index.keys().cloned().collect();
        out.sort();
        out
    }

    /// All edges, sorted.
    pub fn edges(&self) -> Vec<DagEdge> {
        let mut out: Vec<DagEdge> = self.edges.keys().cloned().collect();
        out.sort();
        out
    }

    /// Out-edges of `from` (what `from` depends on), sorted.
    pub fn out_edges(&self, from: &GraphNode) -> Vec<DagEdge> {
        let mut out: Vec<DagEdge> = self
            .edges
            .keys()
            .filter(|(f, _)| f == from)
            .cloned()
            .collect();
        out.sort();
        out
    }

    /// In-edges of `to` (what depends on `to`), sorted.
    pub fn in_edges(&self, to: &GraphNode) -> Vec<DagEdge> {
        let mut out: Vec<DagEdge> = self
            .edges
            .keys()
            .filter(|(_, t)| t == to)
            .cloned()
            .collect();
        out.sort();
        out
    }

    /// Every edge whose *source* is a property of `node_id`, whatever the
    /// property name (the alias-safe lookup used by the cycle gate).
    pub fn property_edges(&self, node_id: NodeId) -> Vec<DagEdge> {
        let mut out: Vec<DagEdge> = self
            .edges
            .keys()
            .filter(|(f, _)| matches!(f, GraphNode::GeometryProperty(id, _) if *id == node_id))
            .cloned()
            .collect();
        out.sort();
        out
    }

    /// Every edge whose *source* is an output port of procedural node
    /// `node_id`, whatever the port (the chain/operand topology that a
    /// procedural edit replaces wholesale).
    pub fn port_edges(&self, node_id: NodeId) -> Vec<DagEdge> {
        let mut out: Vec<DagEdge> = self
            .edges
            .keys()
            .filter(|(f, _)| matches!(f, GraphNode::Procedural { node, .. } if *node == node_id))
            .cloned()
            .collect();
        out.sort();
        out
    }

    /// Every edge whose *target* is an output port of procedural node
    /// `node_id` — the wires other nodes have into it.
    pub fn incoming_port_edges(&self, node_id: NodeId) -> Vec<DagEdge> {
        let mut out: Vec<DagEdge> = self
            .edges
            .keys()
            .filter(|(_, t)| matches!(t, GraphNode::Procedural { node, .. } if *node == node_id))
            .cloned()
            .collect();
        out.sort();
        out
    }

    /// Direct dependents of `node`: everything that reads it (`u` where
    /// `u → node`), sorted.
    pub fn dependents(&self, node: &GraphNode) -> Vec<GraphNode> {
        self.in_edges(node)
            .into_iter()
            .map(|(from, _)| from)
            .collect()
    }

    /// Direct dependencies of `node`: everything it reads (`v` where
    /// `node → v`), sorted.
    pub fn dependencies(&self, node: &GraphNode) -> Vec<GraphNode> {
        self.out_edges(node).into_iter().map(|(_, to)| to).collect()
    }

    pub fn summary(&self) -> GraphSummary {
        GraphSummary {
            nodes: self.node_count(),
            edges: self.edge_count(),
            acyclic: self.is_acyclic(),
        }
    }

    /// True ⟺ the graph has no directed cycle (petgraph's DFS check).
    pub fn is_acyclic(&self) -> bool {
        !is_cyclic_directed(&self.graph)
    }

    /// Full structural self-check: index/edge maps agree with the graph and
    /// the graph is acyclic.
    pub fn validate(&self) -> Result<(), GraphViolation> {
        if self.graph.node_count() != self.index.len() {
            return Err(GraphViolation::NodeCount {
                graph_nodes: self.graph.node_count(),
                indexed_nodes: self.index.len(),
            });
        }
        if self.graph.edge_count() != self.edges.len() {
            return Err(GraphViolation::EdgeCount {
                graph_edges: self.graph.edge_count(),
                map_edges: self.edges.len(),
            });
        }
        for (node, idx) in &self.index {
            match self.graph.node_weight(*idx) {
                Some(found) if found == node => {}
                found => {
                    return Err(GraphViolation::WeightMismatch {
                        node: node.key(),
                        found: found.map(|w| w.key()),
                    })
                }
            }
        }
        for ((from, to), idx) in &self.edges {
            match self.graph.edge_endpoints(*idx) {
                Some((a, b))
                    if self.graph.node_weight(a) == Some(from)
                        && self.graph.node_weight(b) == Some(to) => {}
                _ => {
                    return Err(GraphViolation::StaleEdgeIndex {
                        from: from.key(),
                        to: to.key(),
                    })
                }
            }
        }
        if !self.is_acyclic() {
            return Err(GraphViolation::Cyclic);
        }
        Ok(())
    }

    // ── Mutation (cycle-gated) ─────────────────────────────────────────

    /// Add one edge atomically. `Ok(true)` = added, `Ok(false)` = already
    /// present, `Err` = **rejected because it would close a cycle** — nothing
    /// was mutated.
    ///
    /// Cycle test (the textbook rule, O(affected) instead of a clone): adding
    /// `from → to` ("from depends on to") closes a cycle **iff `to` already
    /// reaches `from`**, i.e. `to` already depends on `from`. `petgraph`'s
    /// `has_path_connecting` answers exactly that question.
    pub fn try_add_edge(&mut self, from: GraphNode, to: GraphNode) -> Result<bool, VectraError> {
        let mut created: Vec<GraphNode> = Vec::new();
        match self.insert_edge(from.clone(), to.clone(), &mut created) {
            Ok(added) => Ok(added),
            Err(e) => {
                self.prune_isolated(&created);
                Err(e)
            }
        }
    }

    /// Add several edges atomically (all-or-nothing). Returns how many were
    /// new. On rejection nothing is mutated — no edges, no placeholder nodes.
    pub fn try_add_edges(&mut self, edges: &[DagEdge]) -> Result<usize, VectraError> {
        let mut created: Vec<GraphNode> = Vec::new();
        let mut added: Vec<DagEdge> = Vec::new();
        for (from, to) in edges {
            match self.insert_edge(from.clone(), to.clone(), &mut created) {
                Ok(true) => added.push((from.clone(), to.clone())),
                Ok(false) => {}
                Err(e) => {
                    for (f, t) in added.iter().rev() {
                        self.remove_edge(f, t);
                    }
                    self.prune_isolated(&created);
                    return Err(e);
                }
            }
        }
        Ok(added.len())
    }

    /// Insert the edge into the graph, creating vertices on demand. Nodes
    /// created here are recorded in `created` so a rejected batch can undo
    /// them exactly.
    fn insert_edge(
        &mut self,
        from: GraphNode,
        to: GraphNode,
        created: &mut Vec<GraphNode>,
    ) -> Result<bool, VectraError> {
        let key = (from.clone(), to.clone());
        if self.edges.contains_key(&key) {
            return Ok(false);
        }
        if from == to {
            return Err(VectraError::cyclic(format!(
                "'{}' cannot depend on itself",
                from.label()
            )));
        }
        let from_idx = self.ensure_node(from.clone(), created);
        let to_idx = self.ensure_node(to.clone(), created);
        if has_path_connecting(&self.graph, to_idx, from_idx, None) {
            return Err(VectraError::cyclic(format!(
                "'{}' would depend on '{}', which already depends on '{}' (cycle)",
                from.label(),
                to.label(),
                from.label()
            )));
        }
        let edge_idx = self.graph.add_edge(from_idx, to_idx, DependencyEdge);
        self.edges.insert(key, edge_idx);
        Ok(true)
    }

    fn ensure_node(&mut self, node: GraphNode, created: &mut Vec<GraphNode>) -> NodeIndex {
        if let Some(idx) = self.index.get(&node) {
            return *idx;
        }
        let idx = self.graph.add_node(node.clone());
        self.index.insert(node.clone(), idx);
        created.push(node);
        idx
    }

    /// Remove edges that are still present, then drop any just-created node
    /// that ended up isolated (a pure rollback of a rejected insertion).
    fn prune_isolated(&mut self, created: &[GraphNode]) {
        for node in created {
            let isolated = self
                .node_index(node)
                .map(|idx| {
                    self.graph
                        .neighbors_directed(idx, Direction::Outgoing)
                        .next()
                        .is_none()
                        && self
                            .graph
                            .neighbors_directed(idx, Direction::Incoming)
                            .next()
                            .is_none()
                })
                .unwrap_or(false);
            if isolated {
                self.remove_node(node);
            }
        }
    }

    /// Remove one edge. `true` if it existed.
    pub fn remove_edge(&mut self, from: &GraphNode, to: &GraphNode) -> bool {
        match self.edges.remove(&(from.clone(), to.clone())) {
            Some(idx) => {
                self.graph.remove_edge(idx);
                true
            }
            None => false,
        }
    }

    /// Remove a vertex and every incident edge. Other vertices and edges keep
    /// their indices ([`StableGraph`] guarantee). `true` if it existed.
    pub fn remove_node(&mut self, node: &GraphNode) -> bool {
        let Some(idx) = self.index.remove(node) else {
            return false;
        };
        let incident: Vec<DagEdge> = self
            .edges
            .keys()
            .filter(|(f, t)| f == node || t == node)
            .cloned()
            .collect();
        for (from, to) in incident {
            self.remove_edge(&from, &to);
        }
        self.graph.remove_node(idx);
        true
    }

    pub fn clear(&mut self) {
        self.graph.clear();
        self.index.clear();
        self.edges.clear();
    }

    // ── Derivation + reconciliation ────────────────────────────────────

    /// Reconcile the graph with `doc`: apply the minimal delta between the
    /// live graph and [`derive`]. Cycles are impossible here for a document
    /// reached through commands (the gate ran first); if one is nevertheless
    /// found, the offending edge is skipped and reported — never a panic.
    pub fn sync(&mut self, doc: &Document) -> GraphSyncReport {
        let DerivedGraph {
            nodes: desired_nodes,
            edges: desired_edges,
            problems,
        } = derive(doc);
        let mut report = GraphSyncReport {
            problems,
            ..Default::default()
        };
        let before: BTreeSet<GraphNode> = self.index.keys().cloned().collect();

        let stale_edges: Vec<DagEdge> = self
            .edges
            .keys()
            .filter(|e| !desired_edges.contains(*e))
            .cloned()
            .collect();
        for (from, to) in stale_edges {
            if self.remove_edge(&from, &to) {
                report.edges_removed += 1;
            }
        }

        let doomed: Vec<GraphNode> = self
            .index
            .keys()
            .filter(|n| !desired_nodes.contains(*n))
            .cloned()
            .collect();
        for node in doomed {
            if self.remove_node(&node) {
                report.nodes_removed += 1;
            }
        }

        for (from, to) in &desired_edges {
            if self.edges.contains_key(&(from.clone(), to.clone())) {
                continue;
            }
            match self.try_add_edge(from.clone(), to.clone()) {
                Ok(true) => report.edges_added += 1,
                Ok(false) => {}
                Err(e) => report.problems.push(GraphProblem::RejectedEdge {
                    from: from.clone(),
                    to: to.clone(),
                    message: e.to_string(),
                }),
            }
        }

        report.nodes_added = self.index.keys().filter(|n| !before.contains(*n)).count();
        debug_assert!(self.validate().is_ok(), "post-sync invariant");
        report
    }

    // ── Propagation ────────────────────────────────────────────────────

    /// Everything that must be re-evaluated because `changed` changed:
    /// the changed vertices plus their transitive **dependents**, returned in
    /// **evaluation order** (dependencies first, dependents last).
    ///
    /// Traversal walks *against* the arrows — edges read "from depends on to",
    /// so a change flows from `to` to every `from` that reads it
    /// (`$base` → the expressions reading it → the geometry binding them).
    ///
    /// Vertices absent from the graph are still included in the result: a
    /// changed node with no dependents is still a changed node (and its own
    /// property, if any, still needs re-evaluating).
    pub fn affected(&self, changed: &[GraphNode]) -> Vec<GraphNode> {
        let mut reachable: BTreeSet<GraphNode> = BTreeSet::new();
        let mut stack: Vec<GraphNode> = Vec::new();
        for node in changed {
            reachable.insert(node.clone());
            stack.push(node.clone());
        }
        while let Some(current) = stack.pop() {
            for (dependent, _) in self.in_edges(&current) {
                if reachable.insert(dependent.clone()) {
                    stack.push(dependent);
                }
            }
        }
        self.evaluation_order(&reachable)
    }

    /// Geometry ids to re-evaluate for a set of changed graph vertices, sorted.
    ///
    /// Empty ⟺ nothing to re-evaluate. Note that an empty [`DirtySet`] *means*
    /// "full rebuild" in `vectra-geometry`, so this returns `None` for the
    /// empty case rather than an empty set — the two are never confused.
    pub fn dirty_set(&self, changed: &[GraphNode]) -> Option<DirtySet> {
        let ids = self.dirty_ids(changed);
        if ids.is_empty() {
            None
        } else {
            Some(DirtySet::nodes(ids))
        }
    }

    /// Same as [`DependencyGraph::dirty_set`] but as a plain sorted id list
    /// (the shape events and stats use).
    pub fn dirty_ids(&self, changed: &[GraphNode]) -> Vec<NodeId> {
        let mut ids: Vec<NodeId> = self
            .affected(changed)
            .into_iter()
            .filter_map(|n| n.node_id())
            .collect();
        ids.sort();
        ids.dedup();
        ids
    }

    /// The ids to re-evaluate for a batch of engine events — the production
    /// dirty derivation (the engine owner and the law tests call this same
    /// function, so no test can pass against a private re-implementation).
    ///
    /// * `NodesUpdated` / `NodesRemoved` — the node itself changed (a bound
    ///   slot, a new node, a delete), so it is dirty directly.
    /// * `VariablesUpdated` / `ExpressionsUpdated` — the *source* changed; the
    ///   graph turns it into the geometry that reads it.
    /// * `OrderChanged` / `StackChanged` / `ConstraintsUpdated` — structural,
    ///   no ids (a constraint is not a graph vertex in Phase 1; the geometry it
    ///   moves is reported as `NodesUpdated`).
    /// * `Dirty` — an output of this pipeline; ignored (idempotent even if
    ///   callers pass back their own event list).
    pub fn dirty_ids_for_events(&self, events: &[EngineEvent]) -> Vec<NodeId> {
        let mut direct: Vec<NodeId> = Vec::new();
        let mut changed: Vec<GraphNode> = Vec::new();
        for event in events {
            match event {
                EngineEvent::NodesUpdated { ids } | EngineEvent::NodesRemoved { ids } => {
                    direct.extend(ids.iter().copied());
                }
                EngineEvent::VariablesUpdated { names } => {
                    changed.extend(names.iter().cloned().map(GraphNode::Variable));
                }
                EngineEvent::ExpressionsUpdated { ids } => {
                    changed.extend(ids.iter().copied().map(GraphNode::Expression));
                }
                // A constraint change is not a graph vertex (Task 3.1 keeps the
                // frozen 2.2 wire shape): the geometry it moves arrives as the
                // `NodesUpdated` events the solver pass emits alongside it.
                EngineEvent::ConstraintsUpdated { .. } => {}
                // Drag lifecycle events (Task 3.2) carry no geometry of their
                // own: the movement arrives as `NodesUpdated` on every pointer
                // sample, which is what `settle` feeds to the dirty closure.
                EngineEvent::DragStarted { .. } | EngineEvent::DragEnded { .. } => {}
                // Operation registry events (Task 4.0): an operation is not a
                // graph vertex in Phase 1 either. The virtual node it produces
                // re-evaluates because `OperationsEvaluator` (in
                // `vectra-operations`) intersects the registry with this dirty
                // set — an O(ops × inputs) scan, not a graph traversal, which
                // is why no edge kind is needed to keep the result correct.
                EngineEvent::OperationsUpdated { .. } => {}
                // Procedural changes (Task 7.0) name their own scene id: the
                // node is a graph vertex (`GraphNode::Procedural`) *and*, when
                // it produces geometry, a live scene id — so it is dirty
                // directly, and the pass then propagates along the chain and to
                // the slots that read its value ports.
                EngineEvent::ProceduralUpdated { ids } => {
                    direct.extend(ids.iter().copied());
                }
                // Motion tracks (Task 6.0) *are* graph vertices: a track edit is
                // a source change, so the slots that sample it re-evaluate and
                // nothing else does.
                EngineEvent::TracksUpdated { ids } => {
                    changed.extend(ids.iter().cloned().map(GraphNode::Track));
                }
                // **Task 10.2 RULE 4, in one arm.** Presentation flags and
                // layer/artboard bookkeeping carry no parametric value, so they
                // dirty nothing: toggling an eye must not re-evaluate a single
                // slot. `LayerOrderChanged` is the same story for a different
                // reason — the *order* changed, not any node's values, and the
                // scene re-reads its order from the document (`resync_order`)
                // instead of re-evaluating.
                EngineEvent::NodeFlagsChanged { .. }
                | EngineEvent::LayersUpdated { .. }
                | EngineEvent::LayerOrderChanged
                | EngineEvent::ArtboardsUpdated { .. }
                | EngineEvent::OrderChanged
                | EngineEvent::StackChanged { .. }
                | EngineEvent::Dirty { .. } => {}
            }
        }
        let mut ids = direct;
        ids.extend(self.dirty_ids(&changed));
        ids.sort();
        ids.dedup();
        ids
    }

    /// Kahn ordering over the induced subgraph, dependencies first and ties
    /// broken by vertex order so the result is fully deterministic.
    ///
    /// A leftover (only possible if a cycle slipped past [`DependencyGraph::validate`])
    /// is appended in sorted order rather than dropped.
    fn evaluation_order(&self, set: &BTreeSet<GraphNode>) -> Vec<GraphNode> {
        let pending_in_set = |node: &GraphNode| -> usize {
            self.out_edges(node)
                .iter()
                .filter(|(_, to)| set.contains(to))
                .count()
        };
        let mut remaining: BTreeMap<GraphNode, usize> =
            set.iter().map(|n| (n.clone(), pending_in_set(n))).collect();
        // dependency → the vertices that depend on it
        let mut dependents: BTreeMap<GraphNode, Vec<GraphNode>> = BTreeMap::new();
        for (from, to) in self.edges.keys() {
            if set.contains(from) && set.contains(to) {
                dependents.entry(to.clone()).or_default().push(from.clone());
            }
        }
        let mut ready: BTreeSet<GraphNode> = remaining
            .iter()
            .filter(|(_, count)| **count == 0)
            .map(|(node, _)| node.clone())
            .collect();
        let mut ordered: Vec<GraphNode> = Vec::with_capacity(set.len());
        let mut emitted: HashSet<GraphNode> = HashSet::new();
        while let Some(node) = ready.iter().next().cloned() {
            ready.remove(&node);
            ordered.push(node.clone());
            emitted.insert(node.clone());
            for dependent in dependents.get(&node).cloned().unwrap_or_default() {
                if let Some(count) = remaining.get_mut(&dependent) {
                    *count = count.saturating_sub(1);
                    if *count == 0 {
                        ready.insert(dependent);
                    }
                }
            }
        }
        for node in set {
            if !emitted.contains(node) {
                ordered.push(node.clone());
            }
        }
        ordered
    }

    // ── Pre-mutation gate ──────────────────────────────────────────────

    /// Dry-run a prospective edge delta on a scratch copy: `Ok` ⟺ applying it
    /// keeps the graph acyclic.
    ///
    /// This is the function the engine calls *before* dispatching a command
    /// that adds edges. It is side-effect free by construction (the only
    /// mutation is on the clone), so a rejected command leaves the live graph,
    /// the document, and the undo stack untouched.
    pub fn dry_run(&self, prospective: &ProspectiveEdges) -> Result<(), VectraError> {
        if prospective.adds.is_empty() {
            return Ok(());
        }
        let mut scratch = self.clone();
        for (from, to) in &prospective.removes {
            scratch.remove_edge(from, to);
        }
        scratch.try_add_edges(&prospective.adds).map(|_| ())
    }
}

/// The dependency graph's own edge-delta vocabulary, re-exported here so
/// `graph` stays self-contained.
pub use crate::prospective::ProspectiveEdges;

/// What the document implies — the single source of truth for graph state.
///
/// Total: an unparsable expression source contributes no edges and a
/// [`GraphProblem`] (pre-validation means commands can never store one).
pub fn derive(doc: &Document) -> DerivedGraph {
    let mut derived = DerivedGraph::default();

    for (id, record) in &doc.expressions {
        match ExpressionEngine::check_source(&record.source) {
            Ok(deps) => {
                for dep in deps {
                    derived.insert_edge(GraphNode::Expression(*id), GraphNode::Variable(dep));
                }
            }
            Err(e) => derived.problems.push(GraphProblem::UnparsableExpression {
                id: *id,
                message: e.to_string(),
            }),
        }
    }

    for node in doc.nodes.values() {
        node.for_each_float_param(|property, param| {
            for target in dependency_targets(param) {
                derived.insert_edge(
                    GraphNode::GeometryProperty(node.id, property.to_string()),
                    target,
                );
            }
        });
    }

    // The procedural graph (Task 7.0). Two kinds of edge, one per output port:
    //
    //   ⬡ n • scalar  →  ◉ hover        (an operand reading a state flag)
    //   ⬡ n • region  →  ⬡ up • region  (a wire to an upstream port)
    //
    // A `Source` node's dependency on an *authored* node is deliberately not an
    // edge here: it reads whole geometry, not a slot, exactly like an operation
    // — so it is handled by the same dirty-id scan the operations pass uses
    // (`ProceduralRegistry::sources_referencing`), not by a fabricated edge to
    // every property of every source.
    for node in doc.procedural.in_order() {
        for edge in procedural_edges(node) {
            derived.insert_edge(edge.0, edge.1);
        }
    }

    derived.problems.sort_by_key(|a| a.message());
    derived
}

/// The edges one procedural node contributes, in **port order** (the first
/// output port first), for every output port the kind declares.
///
/// Deterministic by construction: ports come from `ProceduralKind::outputs`,
/// wires and operands from `BTreeMap`s.
pub fn procedural_edges(node: &vectra_core::ProceduralNode) -> Vec<DagEdge> {
    let mut edges: Vec<DagEdge> = Vec::new();
    for port in node.kind.outputs() {
        let from = GraphNode::Procedural {
            node: node.id,
            port: port.name.clone(),
        };
        for target in node.wires.values() {
            edges.push((
                from.clone(),
                GraphNode::Procedural {
                    node: target.node,
                    port: target.port.clone(),
                },
            ));
        }
        for value in node.operands.values() {
            for target in crate::prospective::param_value_targets(value) {
                edges.push((from.clone(), target));
            }
        }
        if let Some(source) = node.kind.source_node() {
            let _ = source; // documented above: the pass scans for it instead
        }
    }
    edges
}

/// The graph vertex a float parameter depends on, if any. Singular convenience
/// over [`dependency_targets`] for callers that only need "is this parametric,
/// and by what?" (inspector badges, one-edge tests). **Never use it for edge
/// derivation** — it drops every dependency but the first.
pub fn dependency_target(param: &Parameter<f64>) -> Option<GraphNode> {
    dependency_targets(param).into_iter().next()
}

/// Every graph vertex a float parameter depends on (Task 6.0).
///
/// # Why this is plural
///
/// A motion binding is a *tree of parameters*: `Spring { target: … }` and
/// `StateDriven { true_value, false_value }` each contain further
/// `Parameter<f64>`s, which may themselves be animated. The singular version
/// that shipped with Task 2.2 mapped `Animated(_)` to the clock and stopped,
/// which is correct for a literal-targeted spring and **silently wrong for
/// every other kind**: a `Spring { target: $radius }` would be re-sampled by the
/// clock but never re-aimed by an edit to `$radius`, leaving the scene stale
/// until something unrelated dirtied the node.
///
/// So the derivation walks the binding and emits an edge per dependency:
///
/// * `Variable` / `Expression` → their own vertices;
/// * `Animated` → the clock **and** everything the binding reads:
///   `StateDriven` → the flag's [`GraphNode::State`], `KeyframeTrack` → its
///   [`GraphNode::Track`], plus the recursive walk through nested parameters;
/// * `Procedural` / `Interaction` → `None` until their graphs land (Phase 4/5).
///
/// Over-approximation is still sound: a vertex that turns out not to be read
/// costs an extra evaluation, never a stale scene.
pub fn dependency_targets(param: &Parameter<f64>) -> Vec<GraphNode> {
    let mut out: Vec<GraphNode> = Vec::new();
    collect_targets(param, &mut out);
    out.sort();
    out.dedup();
    out
}

fn collect_targets(param: &Parameter<f64>, out: &mut Vec<GraphNode>) {
    match param {
        Parameter::Literal(_) => {}
        Parameter::Variable(name) => out.push(GraphNode::Variable(name.clone())),
        Parameter::Expression(id) => out.push(GraphNode::Expression(*id)),
        Parameter::Animated(binding) => {
            // Motion is sampled at `ctx.time`, so the clock is always a
            // dependency — and the binding's own sources are gathered below.
            out.push(GraphNode::clock());
            for flag in binding.state_flags() {
                out.push(GraphNode::State(flag.to_string()));
            }
            if let Some(track) = binding.track_id() {
                out.push(GraphNode::Track(track.clone()));
            }
            for inner in binding.inner_parameters() {
                collect_targets(inner, out);
            }
        }
        // Task 7.0: a slot fed by a procedural port depends on that port (and
        // through it on the node's operands and upstream chain). Before this,
        // the arm was empty and a procedural read was dirtied by *nothing*.
        Parameter::Procedural(output) => out.push(GraphNode::Procedural {
            node: output.node,
            port: output.port.clone(),
        }),
        Parameter::Interaction(_) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vectra_core::{Command, Engine, NodeKind, ParamValue, Parameter};

    fn rect_engine() -> (Engine, NodeId) {
        let mut engine = Engine::new();
        let id = vectra_core::new_node_id();
        engine
            .dispatch(Command::CreateNode {
                id,
                kind: NodeKind::rectangle(0.0, 0.0, 10.0, 10.0),
                name: Some("rect".to_string()),
                index: None,
            })
            .unwrap();
        (engine, id)
    }

    #[test]
    fn edges_are_deduped_and_indices_stay_stable_across_removal() {
        let mut graph = DependencyGraph::new();
        let a = GraphNode::Variable("a".to_string());
        let b = GraphNode::Variable("b".to_string());
        let e = GraphNode::Expression(vectra_core::new_expression_id());
        assert!(graph.try_add_edge(e.clone(), a.clone()).unwrap());
        assert!(graph.try_add_edge(e.clone(), b.clone()).unwrap());
        // Duplicate add is a no-op, not a second parallel edge.
        assert!(!graph.try_add_edge(e.clone(), a.clone()).unwrap());
        assert_eq!(graph.node_count(), 3);
        assert_eq!(graph.edge_count(), 2);

        let a_idx = graph.node_index(&a).unwrap();
        let b_idx = graph.node_index(&b).unwrap();
        assert!(graph.remove_node(&e));
        assert_eq!(graph.node_count(), 2);
        assert_eq!(graph.edge_count(), 0);
        // StableGraph keeps the surviving indices valid after a removal.
        assert_eq!(graph.node_index(&a), Some(a_idx));
        assert_eq!(graph.node_index(&b), Some(b_idx));
        assert_eq!(graph.validate(), Ok(()));
    }

    #[test]
    fn cycle_gate_rejects_and_rolls_back_completely() {
        let mut graph = DependencyGraph::new();
        let a = GraphNode::Variable("a".to_string());
        let b = GraphNode::Variable("b".to_string());
        let c = GraphNode::Variable("c".to_string());
        graph.try_add_edge(b.clone(), a.clone()).unwrap();

        // b→c is fine; c→b would close b → c → b.
        let err = graph
            .try_add_edges(&[(b.clone(), c.clone()), (c.clone(), b.clone())])
            .unwrap_err();
        assert!(err.is_cycle_rejection(), "expected CyclicDependency: {err}");

        // All-or-nothing: the good edge of the batch is gone too, and the
        // graph is exactly what it was.
        assert_eq!(graph.edges(), vec![(b.clone(), a.clone())]);
        assert_eq!(graph.node_count(), 2, "placeholder node c was pruned");
        assert_eq!(graph.validate(), Ok(()));
        assert!(graph.is_acyclic());

        // Self-dependency is rejected as well.
        let self_err = graph.try_add_edge(a.clone(), a.clone()).unwrap_err();
        assert!(self_err.is_cycle_rejection());
        assert_eq!(graph.node_count(), 2);
    }

    #[test]
    fn dry_run_never_touches_the_live_graph() {
        let mut graph = DependencyGraph::new();
        let a = GraphNode::Variable("a".to_string());
        let b = GraphNode::Variable("b".to_string());
        graph.try_add_edge(b.clone(), a.clone()).unwrap();
        let before = graph.edges();

        let mut scratch_ok = DependencyGraph::new();
        scratch_ok.try_add_edge(b.clone(), a.clone()).unwrap();
        // ok: c → b (nothing depends on c yet)
        let c = GraphNode::Variable("c".to_string());
        assert!(scratch_ok
            .dry_run(&ProspectiveEdges {
                adds: vec![(c.clone(), b.clone())],
                removes: vec![],
            })
            .is_ok());
        // rejected: a → b
        let err = scratch_ok
            .dry_run(&ProspectiveEdges {
                adds: vec![(a.clone(), b.clone())],
                removes: vec![],
            })
            .unwrap_err();
        assert!(err.is_cycle_rejection());

        assert_eq!(scratch_ok.edges(), before, "dry-run is side-effect free");
        assert_eq!(scratch_ok.validate(), Ok(()));
    }

    #[test]
    fn dry_run_applies_removes_before_it_checks_adds() {
        // Vertex roles: a → m → e → z (a depends on m, m on e, e on z).
        let mut graph = DependencyGraph::new();
        let a = GraphNode::Variable("a".to_string());
        let m = GraphNode::Variable("m".to_string());
        let e = GraphNode::Expression(vectra_core::new_expression_id());
        let z = GraphNode::Variable("z".to_string());
        graph.try_add_edge(a.clone(), m.clone()).unwrap();
        graph.try_add_edge(m.clone(), e.clone()).unwrap();
        graph.try_add_edge(e.clone(), z.clone()).unwrap();
        let before = graph.edges();

        // Adding e → a ("e depends on a") closes a ⇝ e … but only because of
        // a → m. Without that removal the check must reject:
        let rejected = graph
            .dry_run(&ProspectiveEdges {
                adds: vec![(e.clone(), a.clone())],
                removes: vec![],
            })
            .unwrap_err();
        assert!(rejected.is_cycle_rejection(), "expected a cycle rejection");

        // …and removing a → m *first* (a redefine, a delete, an undo) makes
        // the very same addition legal:
        assert!(graph
            .dry_run(&ProspectiveEdges {
                adds: vec![(e.clone(), a.clone())],
                removes: vec![(a.clone(), m.clone())],
            })
            .is_ok());

        // Removing an unrelated edge is not a blanket bypass.
        let still_cyclic = graph
            .dry_run(&ProspectiveEdges {
                adds: vec![(e.clone(), a.clone())],
                removes: vec![(e.clone(), z.clone())],
            })
            .unwrap_err();
        assert!(still_cyclic.is_cycle_rejection());

        assert_eq!(
            graph.edges(),
            before,
            "dry-run never touches the live graph"
        );
        assert_eq!(graph.validate(), Ok(()));
    }

    #[test]
    fn propagation_is_minimal_and_ordered() {
        let mut graph = DependencyGraph::new();
        let a = GraphNode::Variable("a".to_string());
        let unrelated = GraphNode::Variable("unrelated".to_string());
        let e1 = GraphNode::Expression(vectra_core::new_expression_id());
        let e2 = GraphNode::Expression(vectra_core::new_expression_id());
        let prop = GraphNode::GeometryProperty(vectra_core::new_node_id(), "width".into());
        let other = GraphNode::GeometryProperty(vectra_core::new_node_id(), "radius".into());
        // a → e1 → e2 → prop ; unrelated → other
        graph.try_add_edge(e1.clone(), a.clone()).unwrap();
        graph.try_add_edge(e2.clone(), e1.clone()).unwrap();
        graph.try_add_edge(prop.clone(), e2.clone()).unwrap();
        graph
            .try_add_edge(other.clone(), unrelated.clone())
            .unwrap();

        let affected = graph.affected(std::slice::from_ref(&a));
        assert_eq!(
            affected,
            vec![a.clone(), e1.clone(), e2.clone(), prop.clone()],
            "dependencies must come before dependents (evaluation order)"
        );
        assert!(!affected.contains(&unrelated));
        assert!(!affected.contains(&other), "unrelated geometry stays clean");
        assert_eq!(
            graph.dirty_ids(std::slice::from_ref(&a)),
            vec![prop.node_id().unwrap()]
        );

        // The empty case is None, never an empty DirtySet (which means "full").
        let idle = GraphNode::Variable("idle".to_string());
        assert_eq!(
            graph.dirty_ids(std::slice::from_ref(&idle)),
            Vec::<NodeId>::new()
        );
        assert!(graph.dirty_set(&[idle]).is_none());
        assert!(graph.dirty_set(&[a]).is_some());
    }

    #[test]
    fn dirty_ids_follow_events_through_the_graph() {
        let mut graph = DependencyGraph::new();
        let node = vectra_core::new_node_id();
        let other = vectra_core::new_node_id();
        let e = GraphNode::Expression(vectra_core::new_expression_id());
        let var = GraphNode::Variable("base".to_string());
        graph
            .try_add_edge(
                GraphNode::GeometryProperty(node, "width".to_string()),
                e.clone(),
            )
            .unwrap();
        graph.try_add_edge(e.clone(), var.clone()).unwrap();
        graph
            .try_add_edge(
                GraphNode::GeometryProperty(other, "radius".to_string()),
                GraphNode::Variable("radius".to_string()),
            )
            .unwrap();

        // A variable change reaches exactly the geometry that reads it.
        let ids = graph.dirty_ids_for_events(&[EngineEvent::VariablesUpdated {
            names: vec!["base".to_string()],
        }]);
        assert_eq!(ids, vec![node]);

        // An expression change reaches its consumers.
        let ids = graph.dirty_ids_for_events(&[EngineEvent::ExpressionsUpdated {
            ids: vec![match e {
                GraphNode::Expression(id) => id,
                _ => unreachable!(),
            }],
        }]);
        assert_eq!(ids, vec![node]);

        // Direct node updates are dirty without any graph contact.
        let ids = graph.dirty_ids_for_events(&[EngineEvent::NodesUpdated { ids: vec![other] }]);
        assert_eq!(ids, vec![other]);

        // A variable nobody reads costs nothing — the empty case is empty.
        assert!(graph
            .dirty_ids_for_events(&[EngineEvent::VariablesUpdated {
                names: vec!["unused".to_string()]
            }])
            .is_empty());

        // Structural events carry no ids, and repeat/self-referential event
        // lists stay idempotent.
        assert!(graph
            .dirty_ids_for_events(&[
                EngineEvent::OrderChanged,
                EngineEvent::StackChanged {
                    can_undo: true,
                    can_redo: false
                }
            ])
            .is_empty());
        let once = graph.dirty_ids_for_events(&[EngineEvent::VariablesUpdated {
            names: vec!["base".to_string()],
        }]);
        let twice = graph.dirty_ids_for_events(&[
            EngineEvent::VariablesUpdated {
                names: vec!["base".to_string()],
            },
            EngineEvent::VariablesUpdated {
                names: vec!["base".to_string()],
            },
        ]);
        assert_eq!(once, twice);
    }

    #[test]
    fn derive_reads_variables_expressions_and_the_clock() {
        let (mut engine, id) = rect_engine();
        let e = vectra_core::new_expression_id();
        engine
            .dispatch(Command::DefineExpression {
                id: e,
                source: "$base * 2".to_string(),
            })
            .unwrap();
        engine
            .dispatch(Command::SetParameter {
                node_id: id,
                property: "width".to_string(),
                value: ParamValue::Float(Parameter::variable("base")),
            })
            .unwrap();
        engine
            .dispatch(Command::SetParameter {
                node_id: id,
                property: "height".to_string(),
                value: ParamValue::Float(Parameter::Expression(e)),
            })
            .unwrap();
        engine
            .dispatch(Command::SetParameter {
                node_id: id,
                property: "x".to_string(),
                value: ParamValue::Float(Parameter::Animated(
                    vectra_core::MotionBinding::StateDriven {
                        state: "on".to_string(),
                        true_value: Box::new(Parameter::Literal(1.0)),
                        false_value: Box::new(Parameter::Literal(0.0)),
                    },
                )),
            })
            .unwrap();

        let derived = derive(engine.document());
        assert!(derived.problems.is_empty());
        assert!(derived.edges.contains(&(
            GraphNode::GeometryProperty(id, "width".to_string()),
            GraphNode::Variable("base".to_string())
        )));
        assert!(derived.edges.contains(&(
            GraphNode::Expression(e),
            GraphNode::Variable("base".to_string())
        )));
        assert!(derived.edges.contains(&(
            GraphNode::GeometryProperty(id, "height".to_string()),
            GraphNode::Expression(e)
        )));
        // Animated → the clock vertex.
        assert!(derived.edges.contains(&(
            GraphNode::GeometryProperty(id, "x".to_string()),
            GraphNode::clock()
        )));
        // Literal slots contribute nothing (y is still literal).
        assert!(!derived
            .nodes
            .contains(&GraphNode::GeometryProperty(id, "y".to_string())));
    }

    #[test]
    fn sync_applies_minimal_deltas_and_then_idles() {
        let (mut engine, id) = rect_engine();
        let mut graph = DependencyGraph::new();

        let first = graph.sync(engine.document());
        assert_eq!(first.nodes_added, 0, "all-literal document has no edges");
        assert!(first.is_clean());

        engine
            .dispatch(Command::SetVariable {
                name: "base".to_string(),
                value: 5.0,
            })
            .unwrap();
        let var_only = graph.sync(engine.document());
        assert_eq!(
            var_only.nodes_added, 0,
            "a variable nobody reads is not a vertex"
        );

        engine
            .dispatch(Command::SetParameter {
                node_id: id,
                property: "width".to_string(),
                value: ParamValue::Float(Parameter::variable("base")),
            })
            .unwrap();
        let bound = graph.sync(engine.document());
        assert_eq!(bound.edges_added, 1);
        assert_eq!(bound.nodes_added, 2);
        assert_eq!(graph.node_count(), 2);
        assert!(graph.summary().acyclic);

        // Idempotent: nothing changed in the document → nothing changes here.
        let again = graph.sync(engine.document());
        assert!(!again.changed());
        assert!(again.is_clean());

        // Undo drops exactly the delta again.
        engine.undo().unwrap();
        let undone = graph.sync(engine.document());
        assert_eq!(undone.edges_removed, 1);
        assert_eq!(undone.nodes_removed, 2);
        assert!(graph.is_empty());
        assert_eq!(graph.validate(), Ok(()));
    }

    #[test]
    fn sync_reports_unparsable_sources_without_dropping_the_rest() {
        let mut doc = Document::new();
        let id = vectra_core::new_node_id();
        doc.insert_node(
            vectra_core::Node::new(id, "rect", NodeKind::rectangle(0.0, 0.0, 1.0, 1.0)),
            None,
        )
        .unwrap();
        // Out-of-band mutation: commands would have rejected this source.
        doc.define_expression(vectra_core::new_expression_id(), "$a * ".to_string());
        doc.variables.insert("base".to_string(), 3.0);
        doc.get_node_mut(id)
            .unwrap()
            .set_param("width", ParamValue::Float(Parameter::variable("base")))
            .unwrap();

        let mut graph = DependencyGraph::new();
        let report = graph.sync(&doc);
        assert_eq!(graph.edges().len(), 1, "the valid binding still lands");
        assert!(!report.is_clean());
        assert!(matches!(
            report.problems.first(),
            Some(GraphProblem::UnparsableExpression { .. })
        ));
    }
}
