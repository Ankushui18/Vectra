//! `vectra-geometry` — evaluation of parametric documents to render-ready scenes.
//!
//! Owns *mathematical resolution and data transformation* (Task 1.3, MES §5):
//!
//! * [`scene`] — [`EvaluatedScene`]: flat drawables + explicit z-order
//! * [`evaluator`] — [`GeometryEvaluator`]: total `Document → EvaluatedScene`
//!   evaluation with [`DirtySet`] incrementality
//! * [`paths`] — lyon [`lyon::path::Path`] construction from resolved points
//! * [`angles`] — arc-angle canonicalization
//! * [`diagnostic`] — [`Diagnostic`]s: every fallback, clamp, and skip
//!
//! Boundary: this crate never touches `wgpu`, GPU buffers, or tessellation
//! output — it hands [`EvaluatedScene`] to `vectra-render`, which owns all of that.
//!
//! ```text
//! Document ──[GeometryEvaluator]──▶ EvaluatedScene ──[Tessellator]──▶ RenderScene ──▶ Screen
//!   (parametric)      (this crate)      (concrete)        (vectra-render)
//! ```

pub mod angles;
pub mod diagnostic;
pub mod evaluator;
pub mod paint;
pub mod paths;
pub mod scene;

pub use angles::{normalize_arc_angles, TAU};
pub use diagnostic::{Diagnostic, DiagnosticCode, Severity};
pub use evaluator::{
    is_renderable, is_renderable_point, DirtySet, Evaluator, GeometryEvaluator, SceneEvaluation,
    MAX_RENDERABLE,
};
pub use paint::{
    resolve_appearance, resolve_style, EvaluatedAppearance, EvaluatedAppearanceKind,
    EvaluatedGradient, EvaluatedPaint,
};
pub use paths::{
    build_path, path_to_rings, path_to_svg_data, polyline_to_path, primitive_to_path,
    primitive_to_rings, rings_to_path, ResolvedSegment, ARC_SEGMENTS_PER_TAU,
};
pub use scene::{EvaluatedNode, EvaluatedPrimitive, EvaluatedScene, EvaluatedStyle};

use thiserror::Error;

/// Geometry-level errors (reserved for fallible helpers; scene evaluation
/// itself is total and reports [`Diagnostic`]s instead of `Result`s).
#[derive(Debug, Error)]
pub enum GeometryError {
    #[error("geometry: {0}")]
    Other(String),
}
