//! `vectra-procedural` — the typed node graph: generators and modifiers
//! (MES §11, Task 7.0).
//!
//! ```text
//!        document ──▶ [pass]  topo order · dirty walk · typed diagnostics
//!                       │
//!     ┌─────────────────┴──────────────────┐
//!     ▼                                    ▼
//!  region/path ports                  scalar/point/color ports
//!     │                                    │
//!     ▼                                    ▼
//!  virtual scene nodes              the published value table
//!  (RULE 3 of Task 4.0)             read by `Parameter::Procedural`
//! ```
//!
//! # Modules
//!
//! * [`nodes`] — the five kinds as pure functions (grid, repeat, noise, smooth,
//!   source), each returning every output port it declares.
//! * [`noise`] — deterministic integer-hashed value noise (RULE 5).
//! * [`engine`] — [`engine::ProceduralEngine`]: registry sync, the pass, the
//!   published table, and `impl ProceduralEvaluator`.
//! * [`error`] — the typed, *local* failures the pass turns into diagnostics.
//!
//! # The five ironclad rules, and where each one lives
//!
//! | rule | where |
//! |---|---|
//! | **1** `GeometryData` is king, in core, dependency-free, strictly typed ports | `vectra-core::procedural` (types, ports, validation) |
//! | **2** the pass runs last and publishes a table; the evaluator only reads | [`engine::ProceduralEngine`] |
//! | **3** no disguised cycles: an operand may not read a procedural output | `vectra_core::procedural::ProceduralNode::validate`, enforced by every command |
//! | **4** no silent eviction: a drawn result is live geometry | `Document::is_geometry_id` / `geometry_ids` |
//! | **5** deterministic noise, byte-identical across native and wasm | [`noise`] |
//!
//! # The chain lives in the registry, not in a second graph
//!
//! The manifest once declared `petgraph`, from the scaffold's assumption that
//! this crate would own its own DAG. It does not need to: the node records and
//! their wiring are *document data* (`ProceduralRegistry`), the topology is
//! derived from them on demand (`ProceduralRegistry::topological_order`), and the
//! **cycle gate** is the workspace's one gate — `vectra-dependency`'s, which
//! dries a candidate wiring run before anything is stored and raises the same
//! `VectraError::CyclicDependency` expressions and motion already raise. A second
//! graph implementation here would be a second answer to the same question.

pub mod engine;
pub mod error;
pub mod nodes;
pub mod noise;

pub use engine::{
    is_source_kind, ProceduralEngine, ProceduralEvaluation, RegistrySync, FLATTEN_TOLERANCE,
};
pub use error::ProceduralError;
pub use nodes::{evaluate, NodeInputs};
