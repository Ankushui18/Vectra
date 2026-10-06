//! Serializable projection of the graph for the dependency inspector.
//!
//! The remote control pulls this on demand (`dependencies_json`) and renders
//! it next to the snapshot — the UI joins `property` vertices to layer names
//! by `node_id`, so the panel shows `rect • width ← ƒx 3f2a1b7c ← $base`
//! instead of opaque uuids.
//!
//! Ordering is fully deterministic (nodes by kind then key, edges by
//! endpoints) so two engines in the same state serialize byte-identically —
//! which is what makes "graph unchanged" assertions in tests meaningful.

use crate::graph::{DagEdge, DependencyGraph, GraphNode, GraphSummary};
use serde::Serialize;

/// One vertex, tagged by kind for a discriminated union in TypeScript.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum GraphNodeView {
    Variable {
        key: String,
        label: String,
        variable: String,
    },
    Expression {
        key: String,
        label: String,
        id: String,
    },
    Property {
        key: String,
        label: String,
        node_id: String,
        property: String,
    },
    /// A live state flag read by a motion binding (Task 6.0): `◉ hover`.
    State {
        key: String,
        label: String,
        state: String,
    },
    /// A keyframe track sampled by a motion binding: `♪ intro`.
    Track {
        key: String,
        label: String,
        track: String,
    },
    /// One output port of one procedural node (Task 7.0): `⬡ 1f0c9d2a • scalar`.
    Procedural {
        key: String,
        label: String,
        node_id: String,
        port: String,
    },
}

impl From<&GraphNode> for GraphNodeView {
    fn from(node: &GraphNode) -> Self {
        match node {
            GraphNode::Variable(name) => Self::Variable {
                key: node.key(),
                label: node.label(),
                variable: name.clone(),
            },
            GraphNode::Expression(id) => Self::Expression {
                key: node.key(),
                label: node.label(),
                id: id.to_string(),
            },
            GraphNode::GeometryProperty(node_id, property) => Self::Property {
                key: node.key(),
                label: node.label(),
                node_id: node_id.to_string(),
                property: property.clone(),
            },
            GraphNode::State(flag) => Self::State {
                key: node.key(),
                label: node.label(),
                state: flag.clone(),
            },
            GraphNode::Track(id) => Self::Track {
                key: node.key(),
                label: node.label(),
                track: id.clone(),
            },
            GraphNode::Procedural { node: id, port } => Self::Procedural {
                key: node.key(),
                label: node.label(),
                node_id: id.to_string(),
                port: port.clone(),
            },
        }
    }
}

/// One edge, referenced by vertex key (`from` depends on `to`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GraphEdgeView {
    pub from: String,
    pub to: String,
}

/// The whole graph, render-ready.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GraphExport {
    pub nodes: Vec<GraphNodeView>,
    pub edges: Vec<GraphEdgeView>,
    pub summary: GraphSummary,
}

/// Deterministic vertex sort: sources first (variables, tracks, states,
/// expressions), then the geometry they drive, each by key. The order is what
/// makes the exported graph diffable.
fn kind_rank(node: &GraphNode) -> u8 {
    match node {
        GraphNode::Variable(_) => 0,
        GraphNode::Track(_) => 1,
        GraphNode::State(_) => 2,
        GraphNode::Expression(_) => 3,
        // A port is a source like an expression: geometry reads it, it reads
        // the document.
        GraphNode::Procedural { .. } => 4,
        GraphNode::GeometryProperty(_, _) => 5,
    }
}

impl DependencyGraph {
    /// Project the graph for the UI.
    pub fn export(&self) -> GraphExport {
        let mut nodes = self.nodes();
        nodes.sort_by(|a, b| {
            kind_rank(a)
                .cmp(&kind_rank(b))
                .then_with(|| a.key().cmp(&b.key()))
        });

        let mut edges: Vec<DagEdge> = self.edges();
        edges.sort_by(|(af, at), (bf, bt)| {
            af.key()
                .cmp(&bf.key())
                .then_with(|| at.key().cmp(&bt.key()))
        });

        GraphExport {
            nodes: nodes.iter().map(GraphNodeView::from).collect(),
            edges: edges
                .into_iter()
                .map(|(from, to)| GraphEdgeView {
                    from: from.key(),
                    to: to.key(),
                })
                .collect(),
            summary: self.summary(),
        }
    }

    /// [`DependencyGraph::export`] as JSON (never panics).
    pub fn export_json(&self) -> String {
        serde_json::to_string(&self.export()).unwrap_or_else(|_| {
            r#"{"nodes":[],"edges":[],"summary":{"nodes":0,"edges":0,"acyclic":true}}"#.to_string()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vectra_core::NodeId;

    #[test]
    fn export_is_deterministic_and_typed() {
        let mut graph = DependencyGraph::new();
        let node = NodeId::nil();
        let var = GraphNode::Variable("base".to_string());
        let expr = GraphNode::Expression(vectra_core::new_expression_id());
        let prop = GraphNode::GeometryProperty(node, "width".to_string());
        graph.try_add_edge(prop.clone(), expr.clone()).unwrap();
        graph.try_add_edge(expr.clone(), var.clone()).unwrap();

        let export = graph.export();
        assert_eq!(export.summary.nodes, 3);
        assert_eq!(export.summary.edges, 2);
        assert!(export.summary.acyclic);

        // Vertices: variables, then expressions, then properties.
        assert!(matches!(export.nodes[0], GraphNodeView::Variable { .. }));
        assert_eq!(export.nodes[0].key_of(), "var:base");
        assert!(matches!(export.nodes[1], GraphNodeView::Expression { .. }));
        assert_eq!(export.nodes[2].key_of(), format!("prop:{node}:width"));
        match &export.nodes[2] {
            GraphNodeView::Property {
                property, node_id, ..
            } => {
                assert_eq!(property, "width");
                assert_eq!(node_id, &node.to_string());
            }
            other => panic!("unexpected vertex {other:?}"),
        }

        // Edges, sorted by source key: expr→var precedes prop→expr because
        // "expr:…" < "prop:…". The chain is property → expression → variable.
        assert_eq!(export.edges[0].from, format!("expr:{}", uuid_of(&expr)));
        assert_eq!(export.edges[0].to, "var:base");
        assert_eq!(export.edges[1].from, format!("prop:{node}:width"));
        assert_eq!(export.edges[1].to, format!("expr:{}", uuid_of(&expr)));

        let json = graph.export_json();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["nodes"][0]["kind"], "variable");
        assert_eq!(parsed["nodes"][2]["kind"], "property");

        // Determinism: same state ⇒ same bytes, whatever the insertion order.
        let mut clone = DependencyGraph::new();
        clone.try_add_edge(expr.clone(), var.clone()).unwrap();
        clone.try_add_edge(prop.clone(), expr.clone()).unwrap();
        assert_eq!(clone.export_json(), json);

        // And the export tracks removals.
        let mut shrink = clone.clone();
        assert!(shrink.remove_edge(&prop, &expr));
        assert_eq!(shrink.export().edges.len(), 1);
        assert_eq!(shrink.export().summary.nodes, 3, "vertices persist");
    }

    impl GraphNodeView {
        pub(crate) fn key_of(&self) -> String {
            match self {
                Self::Variable { key, .. }
                | Self::Expression { key, .. }
                | Self::Property { key, .. }
                | Self::State { key, .. }
                | Self::Track { key, .. }
                | Self::Procedural { key, .. } => key.clone(),
            }
        }
    }

    fn uuid_of(node: &GraphNode) -> String {
        match node {
            GraphNode::Expression(id) => id.to_string(),
            other => panic!("not an expression: {other:?}"),
        }
    }
}
