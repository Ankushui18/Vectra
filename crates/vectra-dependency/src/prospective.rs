//! Prospective edges: what a [`Command`] would do to the graph (Task 2.2 §4).
//!
//! The engine must reject a cycle *before* mutating anything, which means it
//! needs the edge delta of a command without applying it. This module derives
//! that delta from the command plus the *current* document:
//!
//! | Command | derived delta |
//! |---|---|
//! | `DefineExpression { id, source }` | remove `Expression(id) → *` (the old source's deps), add the new source's deps |
//! | `RemoveExpression { id }` | remove `Expression(id) → *` |
//! | `SetParameter { node, prop, value }` | remove `GeometryProperty(node, *)` for the touched properties, add the new source's target |
//! | `CreateNode { kind, … }` | add the new node's sources (a JSON-created node can arrive already bound) |
//! | `DeleteNode { id }` | remove that node's property edges |
//! | `RemoveVariable { name }` | remove every edge reading that variable |
//! | `SetVariable { … }` | no edges (a value change moves no topology) |
//!
//! The delta is a *complete* prediction of the command's topology effect —
//! removals included — which is a stronger contract than the cycle gate alone
//! needs, and is exactly what lets the property test
//! `prop_gate_predicts_the_topology_delta_exactly` compare prediction against
//! outcome for every command. Removals can never *introduce* a cycle, so this
//! costs the gate nothing.
//!
//! `SetParameter` is derived by **simulating the write on a clone of that one
//! node** and re-reading its sources. That is deliberately the same code path
//! the real mutation uses (`Node::set_param` + `Node::for_each_float_param`),
//! so property aliases (`"radius"` / `"r"`, `"style.opacity"`) and type errors
//! behave identically and can never disagree with [`crate::derive`].
//!
//! If the simulated write fails (unknown property, type mismatch) the delta is
//! empty: the dispatch itself will reject the command with a typed error, and
//! an empty delta can never cause a spurious cycle rejection.

use crate::graph::{DagEdge, GraphNode};
use vectra_core::{Command, Document, Node, NodeKind, ParamValue, Parameter, ProceduralNode};
use vectra_expression::ExpressionEngine;

/// The edge delta a command would apply.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProspectiveEdges {
    /// Edges that must be removed before the additions (expression redefine).
    pub removes: Vec<DagEdge>,
    /// Edges the command would add (the ones the cycle gate inspects).
    pub adds: Vec<DagEdge>,
}

impl ProspectiveEdges {
    pub fn empty() -> Self {
        Self::default()
    }

    /// True ⟺ the command changes no dependency edges.
    pub fn is_noop(&self) -> bool {
        self.adds.is_empty() && self.removes.is_empty()
    }

    /// True ⟺ the cycle gate has something to check.
    pub fn adds_edges(&self) -> bool {
        !self.adds.is_empty()
    }

    /// One-line description for logs/tests.
    pub fn describe(&self) -> String {
        format!("+{} / -{} edges", self.adds.len(), self.removes.len())
    }
}

/// The engine's pre-dispatch gate: derive what `cmd` would do to the graph and
/// dry-run it. `Ok(delta)` ⟺ the command may proceed; `Err` is always a typed
/// [`vectra_core::VectraError::CyclicDependency`] **and nothing was mutated** —
/// not the graph, not the document, not the history.
///
/// This is the single implementation the engine owner (`vectra-wasm`), the
/// tests, and any future host all call, so "how a command is gated" can never
/// differ between production and proof.
pub fn gate_command(
    graph: &crate::graph::DependencyGraph,
    cmd: &Command,
    doc: &Document,
) -> Result<ProspectiveEdges, vectra_core::VectraError> {
    let delta = prospective_edges(graph, cmd, doc);
    graph.dry_run(&delta)?;
    Ok(delta)
}

/// Derive the graph delta `cmd` would apply to `doc`.
pub fn prospective_edges(
    graph: &crate::graph::DependencyGraph,
    cmd: &Command,
    doc: &Document,
) -> ProspectiveEdges {
    match cmd {
        Command::DefineExpression { id, source } => {
            let mut delta = ProspectiveEdges {
                removes: graph.out_edges(&GraphNode::Expression(*id)),
                adds: Vec::new(),
            };
            if let Ok(deps) = ExpressionEngine::check_source(source) {
                for dep in deps {
                    delta
                        .adds
                        .push((GraphNode::Expression(*id), GraphNode::Variable(dep)));
                }
            }
            delta
        }
        Command::RemoveExpression { id } => ProspectiveEdges {
            removes: graph.out_edges(&GraphNode::Expression(*id)),
            adds: Vec::new(),
        },
        // Task 4.0: an operation registry entry is topology-neutral. It reads
        // its inputs' *evaluated* geometry rather than their parameter slots, so
        // it adds/removes no dependency-graph edge; deleting a source node still
        // withdraws the dependent operations (handled by the command itself, and
        // the undo restores them). Consumers that need the operation layer ask
        // `Document::operations.affected_by(&dirty)`.
        Command::ApplyOperation { .. }
        | Command::RemoveOperation { .. }
        | Command::SetOperationEnabled { .. } => ProspectiveEdges::empty(),
        // Task 12.0 RULE 2: a Smart Fill reads its boundaries' *evaluated*
        // geometry exactly as RULE 1's other region operations do, so it adds no
        // slot-level edge: `Document::operations.affected_by(&dirty)` is what
        // carries the dirt, and the region pass re-runs. RULE 3's break mints
        // ordinary paths out of geometry the source already had, so its new nodes
        // carry their own literal slots and nothing else — `CreateNode` arms
        // above are the shape a piece *would* have if it were parametric.
        Command::CreateSmartFill { .. } | Command::BreakPath { .. } => ProspectiveEdges::empty(),
        // Constraints are not dependency-graph edges in Phase 1 (Task 3.1 keeps
        // the Task 2.2 graph wire shape frozen): the solver's geometry writes go
        // through the ordinary `SetParameter` path, and the slots they pin are
        // reported as `NodesUpdated`. The gate therefore has nothing to check
        // for these commands — including when they arrive inside a batch.
        Command::AddConstraint { .. }
        | Command::RemoveConstraint { .. }
        | Command::SetConstraintEnabled { .. } => ProspectiveEdges::empty(),
        // Drags (Task 3.2) only ever write *literals* into the node's position
        // slots, and a literal removes edges rather than adding them — the one
        // edge a drag can break (a slot driven by `$var` / `ƒexpr`) is reported
        // as `ParametricLinkBroken`, exactly like a solver write in 3.1. There
        // is therefore no prospective edge to dry-run; the session's own writes
        // reach the graph through the ordinary `SetParameter` path afterwards.
        Command::BeginDrag { .. } | Command::UpdateDrag { .. } | Command::EndDrag { .. } => {
            ProspectiveEdges::empty()
        }
        Command::Batch { commands } => {
            let mut delta = ProspectiveEdges::empty();
            for cmd in commands {
                let child = prospective_edges(graph, cmd, doc);
                delta.removes.extend(child.removes);
                delta.adds.extend(child.adds);
            }
            delta
        }
        Command::SetParameter {
            node_id,
            property,
            value,
        } => {
            let Ok(node) = doc.get_node(*node_id) else {
                return ProspectiveEdges::empty();
            };
            let mut simulated = node.clone();
            if simulated.set_param(property, value.clone()).is_err() {
                // The command will fail during dispatch with a typed error;
                // there is nothing to gate.
                return ProspectiveEdges::empty();
            }
            ProspectiveEdges {
                removes: graph.property_edges(*node_id),
                adds: edges_from_node(&simulated),
            }
        }
        // A path rewrite (Task 10.1) replaces a path's `start` and `segments`.
        // Both can carry `Parameter::Procedural` references (a procedural graph
        // may drive a path point), so the same simulation `SetParameter` does
        // applies: apply the new geometry to a **clone** of the node, re-derive
        // its outgoing edges, and let `gate_command`'s dry run decide. Only the
        // path node's own edges are affected — the rewrite touches no other node.
        Command::SetPath {
            id,
            start,
            segments,
        } => {
            let Ok(node) = doc.get_node(*id) else {
                return ProspectiveEdges::empty();
            };
            let mut simulated = node.clone();
            if let vectra_core::NodeKind::Path {
                start: slot_start,
                segments: slot_segments,
            } = &mut simulated.kind
            {
                *slot_start = start.clone();
                *slot_segments = segments.clone();
            } else {
                // Not a path: dispatch will refuse it with a typed error.
                return ProspectiveEdges::empty();
            }
            ProspectiveEdges {
                removes: graph.property_edges(*id),
                adds: edges_from_node(&simulated),
            }
        }
        // The appearance stack (Task 10.2 RULE 3) is a node's paint, and paint
        // slots can read sources — so a stack edit is simulated exactly like a
        // path rewrite: swap the whole list in, then read the edges the new list
        // implies. A stack that *shrinks* drops the edges of the layers it lost,
        // which is what `removes` is for.
        Command::SetAppearances {
            node_id,
            appearances,
        } => {
            let Ok(node) = doc.get_node(*node_id) else {
                return ProspectiveEdges::empty();
            };
            let mut simulated = node.clone();
            simulated.style.appearances = appearances.clone();
            ProspectiveEdges {
                removes: graph.property_edges(*node_id),
                adds: edges_from_node(&simulated),
            }
        }
        // ── Task 11.0: text ───────────────────────────────────────────────
        //
        // The three *authored* text edits (the string, the family, the
        // alignment) are not parameters: nothing can bind to a word, so they
        // move no edge. (`SetText` is still a document edit — it re-lays the
        // run out — but topology is about sources, and a string has none.)
        Command::SetText { .. }
        | Command::SetFontFamily { .. }
        | Command::SetTextAlignment { .. } => ProspectiveEdges::empty(),
        // Binding *is* a parameter edit: it adds the `path_offset` slot, which
        // can read a variable or an expression, and unbinding removes it. Both
        // are simulated on a clone exactly like `SetPath`, so the offset's
        // sources become real edges the moment they exist — and withdraw with
        // the slot, rather than lingering as a phantom dependency.
        //
        // The **geometry** link (the run follows the path) is deliberately not
        // an edge: it is resolved during the run's own evaluation
        // (`EvaluatedText` reads its source's evaluated geometry one level
        // deep), so the graph's acyclicity guarantee does not have to reason
        // about it. A cycle of geometry references is therefore *allowed* —
        // and harmless: a run bound to a path that is bound to the run's
        // outline draws whatever the last pass produced, and terminates.
        Command::BindTextToPath {
            node_id,
            path,
            offset,
        } => {
            let Ok(node) = doc.get_node(*node_id) else {
                return ProspectiveEdges::empty();
            };
            let mut simulated = node.clone();
            if let NodeKind::Text { on_path, .. } = &mut simulated.kind {
                *on_path = Some(vectra_core::TextPathBinding {
                    node: *path,
                    offset: offset.clone(),
                });
            } else {
                return ProspectiveEdges::empty();
            }
            ProspectiveEdges {
                removes: graph.property_edges(*node_id),
                adds: edges_from_node(&simulated),
            }
        }
        Command::UnbindTextFromPath { node_id } => {
            let Ok(node) = doc.get_node(*node_id) else {
                return ProspectiveEdges::empty();
            };
            let mut simulated = node.clone();
            if let NodeKind::Text { on_path, .. } = &mut simulated.kind {
                *on_path = None;
            } else {
                return ProspectiveEdges::empty();
            }
            ProspectiveEdges {
                removes: graph.property_edges(*node_id),
                adds: edges_from_node(&simulated),
            }
        }
        // Outlining adds nodes (whose points are literals, so they read
        // nothing) and flips the source's `visible` flag: a visibility flip is
        // not a parameter, so no existing edge is withdrawn either. The type it
        // hides keeps every source it had — un-hiding it through undo must not
        // need the graph rebuilt.
        Command::OutlineText { .. } => ProspectiveEdges::empty(),
        // ── Task 10.6: Smart Components ────────────────────────────────────
        //
        // Creating a component *rebinds* its members' slots: a literal width
        // becomes `$c<slug>_size`, a literal fill becomes a procedural port. So
        // the dry run has to simulate the same plan the command will run — the
        // binding is planned against a clone of each member, and the member's
        // outgoing edges are re-derived from it. (No *new* kind of edge can
        // appear: a binding reads a variable or an expression over variables,
        // never a port, so a component cannot close a cycle — but a member that
        // stops reading a procedural port loses that edge, which is exactly what
        // `removes` is for.)
        Command::CreateComponent {
            id, members, props, ..
        } => {
            if members.is_empty() {
                return ProspectiveEdges::empty();
            }
            let props: Vec<vectra_core::ComponentProp> = if props.is_empty() {
                vectra_core::component::infer_props(doc, members)
            } else {
                props.clone()
            };
            let plan = vectra_core::component::bind_plan(
                doc,
                *id,
                members,
                &vectra_core::component::command_prefix(*id),
                &props,
                None,
            );
            let mut removes: Vec<DagEdge> = Vec::new();
            let mut adds: Vec<DagEdge> = Vec::new();
            for (index, member) in members.iter().enumerate() {
                let Ok(node) = doc.get_node(*member) else {
                    return ProspectiveEdges::empty();
                };
                removes.extend(graph.property_edges(*member));
                let mut simulated = node.clone();
                let mut ok = true;
                for write in plan.writes.iter().filter(|write| write.member == index) {
                    if simulated
                        .set_param(&write.property, write.value.clone())
                        .is_err()
                    {
                        ok = false;
                        break;
                    }
                }
                if !ok {
                    return ProspectiveEdges::empty();
                }
                adds.extend(edges_from_node(&simulated));
            }
            ProspectiveEdges { removes, adds }
        }
        // Placing an instance writes *new* nodes (clones in a group) and binds
        // their slots to fresh variables: nothing in the existing graph moves.
        Command::InstantiateComponent { .. } => ProspectiveEdges::empty(),
        // A prop write touches a variable or the component's own operands — never
        // a node's parameters — and a spec edit touches no slot of any node.
        Command::SetComponentProp { .. } | Command::SetComponentSpec { .. } => {
            ProspectiveEdges::empty()
        }
        // A duplicate carries the source's parameters across unchanged, so its
        // edges are the source's edges.
        Command::DuplicateNode { id, source, .. } => {
            let Ok(node) = doc.get_node(*source) else {
                return ProspectiveEdges::empty();
            };
            let mut copy = node.clone();
            copy.id = *id;
            ProspectiveEdges {
                removes: Vec::new(),
                adds: edges_from_node(&copy),
            }
        }
        // Presentation flags, layer membership, layer order and artboard
        // bookkeeping carry no parametric values at all: there is nothing to
        // simulate and nothing to gate (RULE 4).
        Command::SetNodeVisible { .. }
        | Command::SetNodeLocked { .. }
        | Command::RenameNode { .. }
        | Command::CreateLayer { .. }
        | Command::DeleteLayer { .. }
        | Command::RenameLayer { .. }
        | Command::SetLayerVisible { .. }
        | Command::SetLayerLocked { .. }
        // Task 10.7 RULE 3's two flags: booleans on a layer record, and — like
        // the eye and the padlock — nothing a parametric value can flow from.
        | Command::SetLayerAlphaLocked { .. }
        | Command::SetLayerClippingMask { .. }
        | Command::ReorderLayer { .. }
        | Command::AssignNodeToLayer { .. }
        | Command::DetachNodeFromLayers { .. }
        | Command::SetNodeParent { .. }
        | Command::CreateArtboard { .. }
        | Command::DeleteArtboard { .. }
        | Command::RenameArtboard { .. }
        | Command::SetArtboardBounds { .. }
        | Command::SetArtboardBackground { .. }
        | Command::SetActiveArtboard { .. }
        | Command::SetActiveLayer { .. } => ProspectiveEdges::empty(),
        Command::CreateNode {
            id,
            kind,
            name,
            index: _,
        } => {
            let simulated = Node::new(
                *id,
                name.clone().unwrap_or_else(|| kind.tag().to_string()),
                kind.clone(),
            );
            ProspectiveEdges {
                removes: graph.property_edges(*id),
                adds: edges_from_node(&simulated),
            }
        }
        // Motion (Task 6.0). A binding is derived from its own sources, so the
        // same dry-run the expression gate uses applies: replace this slot's
        // edges with the ones the new binding implies, and refuse a binding that
        // would make a slot depend on itself (a spring whose target reads the
        // very slot it drives — see `MotionCycles` in the design doc).
        Command::BindMotion {
            node_id,
            property,
            binding,
        } => {
            let Ok(node) = doc.get_node(*node_id) else {
                return ProspectiveEdges::empty();
            };
            let mut simulated = node.clone();
            if simulated
                .set_param(
                    property,
                    ParamValue::Float(Parameter::Animated(binding.clone())),
                )
                .is_err()
            {
                return ProspectiveEdges::empty();
            }
            ProspectiveEdges {
                removes: graph.property_edges(*node_id),
                adds: edges_from_node(&simulated),
            }
        }
        // A track is a source: the slots sampling it keep their edge, the
        // vertex is new, and a *removed* track takes its inbound edges with it.
        Command::SetMotionTrack { .. } => ProspectiveEdges::empty(),
        Command::RemoveMotionTrack { track_id } => ProspectiveEdges {
            removes: graph.out_edges(&GraphNode::Track(track_id.clone())),
            adds: Vec::new(),
        },
        Command::SetVariable { .. } => ProspectiveEdges::empty(),
        Command::RemoveVariable { name } => ProspectiveEdges {
            removes: graph.in_edges(&GraphNode::Variable(name.clone())),
            adds: Vec::new(),
        },
        Command::DeleteNode { id } => ProspectiveEdges {
            removes: graph.property_edges(*id),
            adds: Vec::new(),
        },

        // ── Procedural graph (Task 7.0) ────────────────────────────────────
        //
        // A wire is a dependency edge like any other, so the *same* gate that
        // protects expressions and motion protects the chain: the candidate
        // wiring is simulated on a clone of the node, its edges are derived
        // with the same function `derive` uses, and the delta replaces the
        // node's previous port edges wholesale. A wire that would close a loop
        // is therefore rejected before the document is touched — and RULE 3's
        // ban on value references back into the graph is what keeps the
        // procedural half of the graph reachable from the same gate.
        Command::AddProceduralNode { node } => ProspectiveEdges {
            removes: Vec::new(),
            adds: crate::graph::procedural_edges(node),
        },
        Command::RemoveProceduralNode { id } => {
            // Everything the node contributed *and* every wire into it: the
            // command drops the incoming wires too (it returns them in its
            // inverse), so downstream nodes lose those edges as well.
            let mut removes = graph.port_edges(*id);
            removes.extend(graph.incoming_port_edges(*id));
            removes.sort();
            removes.dedup();
            ProspectiveEdges {
                removes,
                adds: Vec::new(),
            }
        }
        Command::ConnectProcedural {
            node_id,
            port,
            from,
        } => {
            let Some(node) = doc.procedural.get(*node_id) else {
                return ProspectiveEdges::empty();
            };
            let mut simulated = node.clone();
            simulated.wires.insert(port.clone(), from.clone());
            ProspectiveEdges {
                removes: graph.port_edges(*node_id),
                adds: crate::graph::procedural_edges(&simulated),
            }
        }
        Command::DisconnectProcedural { node_id, port } => {
            let Some(node) = doc.procedural.get(*node_id) else {
                return ProspectiveEdges::empty();
            };
            let mut simulated = node.clone();
            simulated.wires.remove(port);
            ProspectiveEdges {
                removes: graph.port_edges(*node_id),
                adds: crate::graph::procedural_edges(&simulated),
            }
        }
        Command::SetProceduralOperand {
            node_id,
            port,
            value,
        } => {
            let Some(node) = doc.procedural.get(*node_id) else {
                return ProspectiveEdges::empty();
            };
            let mut simulated = node.clone();
            simulated.operands.insert(port.clone(), value.clone());
            ProspectiveEdges {
                removes: graph.port_edges(*node_id),
                adds: crate::graph::procedural_edges(&simulated),
            }
        }
        // Parking changes no topology: the ports still exist, they simply stop
        // publishing. (The geometry leaves the scene through the live-id rule.)
        Command::SetProceduralEnabled { .. } => ProspectiveEdges::empty(),
    }
}

/// Every edge a node's parameters imply — the per-node half of
/// [`crate::derive`], reused by the simulation above.
pub fn edges_from_node(node: &Node) -> Vec<DagEdge> {
    let mut edges: Vec<DagEdge> = Vec::new();
    node.for_each_float_param(|property, param| {
        for target in crate::graph::dependency_targets(param) {
            edges.push((
                GraphNode::GeometryProperty(node.id, property.to_string()),
                target,
            ));
        }
    });
    edges
}

/// Edges a *kind* implies when materialized as a fresh node (used by the
/// `CreateNode` pre-check; `CreateNode` can arrive bound when the document is
/// authored as JSON rather than through the UI).
pub fn edges_from_kind(node_id: vectra_core::NodeId, kind: &NodeKind) -> Vec<DagEdge> {
    edges_from_node(&Node::new(node_id, kind.tag(), kind.clone()))
}

/// Convenience for callers that only need "would this parameter bind to
/// something?" (inspector badges, tests).
pub fn param_target(param: &Parameter<f64>) -> Option<GraphNode> {
    crate::graph::dependency_target(param)
}

/// The graph vertex a `ParamValue` source resolves to, if it is parametric
/// (a typed view of [`edges_from_node`] for a single slot).
pub fn param_value_target(value: &ParamValue) -> Option<GraphNode> {
    param_value_targets(value).into_iter().next()
}

/// Every graph vertex a `ParamValue` source depends on, in vertex order.
///
/// Plural for the same reason [`crate::graph::dependency_targets`] is: a motion
/// binding carries a tree of parameters, and a procedural operand may read
/// several things at once. A `Point` operand can only be a literal (a
/// `Parameter::Procedural` point is banned by Task 7.0 RULE 3, and variables are
/// scalar-only), so it contributes nothing — stated here rather than left as an
/// unexplained `None`.
pub fn param_value_targets(value: &ParamValue) -> Vec<GraphNode> {
    match value {
        ParamValue::Float(p) => crate::graph::dependency_targets(p),
        ParamValue::Point(p) => match p {
            Parameter::Procedural(_) => Vec::new(),
            _ => Vec::new(),
        },
        ParamValue::Color(_) => Vec::new(),
    }
}

/// Every edge a *procedural record* implies when stored as-is (used by the
/// `AddProceduralNode` pre-check: a node can arrive already wired).
pub fn edges_from_procedural_node(node: &ProceduralNode) -> Vec<DagEdge> {
    crate::graph::procedural_edges(node)
}
