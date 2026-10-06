//! `vectra-dependency` — the parametric dependency graph, dirty propagation,
//! and incremental scene evaluation (MES §8, Task 2.2).
//!
//! This crate is the *brain* the MES §16 loop always promised: it is what lets
//! `SetVariable $base` touch one rectangle instead of the document.
//!
//! ```text
//!                       commands (JSON from the remote control)
//!                                     │
//!                    ┌────────────────▼─────────────────┐
//!                    │  prospective_edges(cmd, doc)     │  dry-run: what WOULD
//!                    │  graph.dry_run(&delta)           │  change → cycle gate
//!                    └────────────────┬─────────────────┘
//!                                     │  (reject: VectraError::CyclicDependency)
//!                    ┌────────────────▼─────────────────┐
//!                    │  Engine::dispatch                │  the only mutation
//!                    └────────────────┬─────────────────┘
//!                                     │
//!          ┌──────────────────────────▼───────────────────────────┐
//!          │  graph.sync(doc)        — reconcile with `derive(doc)`│  single
//!          │  dirty = closure(events) — downstream + touched       │  derivation
//!          │  scene.refresh(doc, ctx, dirty) — patch the cache     │
//!          └──────────────────────────┬───────────────────────────┘
//!                                     │
//!                     EngineEvent::Dirty { ids, mode } → UI
//! ```
//!
//! # The four rules this crate is built on
//!
//! 1. **State is derived, never maintained twice.** [`graph::derive`] reads
//!    the document; [`DependencyGraph::sync`] reconciles the live graph with
//!    that derivation. Undo, redo, and command replay need no special cases,
//!    because nothing is remembered that could drift.
//! 2. **Cycles are impossible, not merely unlikely.** Every edge insertion is
//!    gated (`try_add_edge`: adding `u → v` is safe ⟺ `v` cannot already reach
//!    `u`), and commands are dry-run through
//!    [`DependencyGraph::dry_run`] *before* the document is touched — so a
//!    rejected command leaves the document, the history, and the graph exactly
//!    as they were.
//! 3. **Dirty means sufficient and minimal.** The dirty set is the downstream
//!    closure of the mutation plus the mutated nodes themselves: everything
//!    that can change, nothing that cannot.
//! 4. **Index stability is a feature.** [`petgraph::stable_graph::StableGraph`]
//!    (per the Task 2.2 spec) keeps every unrelated node/edge index valid
//!    across removals, so undo/redo of graph elements can never dangle a
//!    reference.
//!
//! # Layers
//!
//! | Module | Owns |
//! |---|---|
//! | [`graph`] | vertices/edges, the cycle gate, sync, propagation |
//! | [`prospective`] | "what would this command do to the graph?" |
//! | [`incremental`] | the scene cache and the patch path |
//! | [`wire`] | deterministic JSON projection for the dependency inspector |
//!
//! Boundary: no `wgpu`, no renderer, no tessellation. The graph produces
//! **ids**; `vectra-geometry` turns ids into an evaluated scene;
//! `vectra-render` would consume that scene. Nothing here draws anything.

pub mod graph;
pub mod incremental;
pub mod prospective;
pub mod wire;

pub use graph::{
    dependency_target, derive, DagEdge, DependencyEdge, DependencyGraph, DependencyNode,
    DerivedGraph, GraphNode, GraphProblem, GraphSummary, GraphSyncReport, GraphViolation,
};
pub use incremental::{EvalReport, EvalStats, IncrementalScene};
pub use prospective::{
    edges_from_kind, edges_from_node, gate_command, param_target, param_value_target,
    prospective_edges, ProspectiveEdges,
};
pub use wire::{GraphEdgeView, GraphExport, GraphNodeView};
