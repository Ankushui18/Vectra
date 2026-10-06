//! `vectra-operations` — non-destructive operations (MES §10, Task 4.0).
//!
//! Operations are *virtual shapes*: an [`vectra_core::OperationNode`] reads its
//! source nodes' evaluated geometry and produces a new one. The sources are
//! never touched — they stay editable, draggable and constrained (RULE 1) — and
//! the result is a first-class scene node with its own id (RULE 3).
//!
//! # The pipeline (RULE 2)
//!
//! ```text
//! EvaluatedScene ──▶ lyon::path::Path ──flatten──▶ geo::MultiPolygon
//!                                                       │  boolean / offset / fillet / mirror
//!                                                       ▼
//! EvaluatedNode  ◀── lyon::path::Path  ◀──build──  geo::MultiPolygon
//! ```
//!
//! `geo` (specifically its `BooleanOps`, `Buffer` and `AffineOps` algorithms)
//! owns all the math. `lyon` stays a path/tessellation crate here — it is used
//! to flatten curves and to build the result path, never to intersect regions.
//!
//! # Modules
//!
//! * [`convert`] — the lyon ⇄ `geo` boundary, with the even-odd ring model.
//! * [`ops`] — the four operations as plain region → region functions.
//! * [`evaluator`] — [`OperationsEvaluator`]: document + primitive scene →
//!   virtual nodes + diagnostics.

pub mod convert;
pub mod error;
pub mod evaluator;
pub mod ops;

pub use convert::{
    multi_polygon_to_path, path_area, path_to_multi_polygon, region_area, FLATTEN_TOLERANCE,
};
pub use error::OperationError;
pub use evaluator::{node_path_data, OperationEvaluation, OperationsEvaluator};
pub use ops::{boolean, fillet, mirror, offset};
