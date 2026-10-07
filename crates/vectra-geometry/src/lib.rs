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
/// The lyon ⇄ `geo` boundary (moved here from `vectra-operations` by Task 12.0,
/// which needs it for the Region Graph and cannot depend on the operations
/// crate).
pub mod convert;
pub mod diagnostic;
pub mod evaluator;
pub mod paint;
pub mod paths;
pub mod regions;
pub mod scene;
pub mod text;

pub use angles::{normalize_arc_angles, TAU};
pub use convert::{
    arc_samples, multi_polygon_to_path, path_area, path_to_multi_polygon, region_area,
    FLATTEN_TOLERANCE,
};
pub use diagnostic::{Diagnostic, DiagnosticCode, Severity};
pub use evaluator::{
    is_renderable, is_renderable_point, DirtySet, Evaluator, GeometryEvaluator, SceneEvaluation,
    MAX_RENDERABLE,
};
/// The `geo` coordinate type, re-exported so a boundary (the wasm layer) can
/// name the rings the region graph hands it without depending on `geo` itself.
pub use geo::Coord as GeoCoord;
pub use paint::{
    resolve_appearance, resolve_style, EvaluatedAppearance, EvaluatedAppearanceKind,
    EvaluatedGradient, EvaluatedPaint,
};
pub use paths::{
    build_path, path_bounds, path_to_rings, path_to_svg_data, polyline_to_path,
    primitive_to_curve_path, primitive_to_path, primitive_to_rings, rings_to_path, ResolvedSegment,
    ARC_SEGMENTS_PER_TAU,
};
pub use regions::{
    atoms, crossings, face_containing, path_rings, point_on_rings, ring_crossings,
    ring_pieces_between, ring_point_at, ring_polygon, source_bounds, source_rings, sources_of,
    spans_of, Crossing, RegionFace, RegionGraph, RingArc, SourceSpec, Span, REGION_EPSILON,
};
pub use scene::{
    EvaluatedGlyph, EvaluatedNode, EvaluatedPrimitive, EvaluatedScene, EvaluatedStyle,
    EvaluatedText, TextMetrics,
};
pub use text::{
    clear_shape_cache, layout_text, layout_text_on_path, outline_plans, path_length, resolve_face,
    sample_at, sample_path, shape_cache_stats, FaceRef, FontLibrary, OutlinePlan, PathSample,
    ShapeCacheStats, TextError, TextSpec, BUNDLED_FACE, BUNDLED_FACE_ID, BUNDLED_FAMILY,
    OUTLINE_TOLERANCE, PATH_TOLERANCE,
};

use thiserror::Error;

/// Geometry-level errors (reserved for fallible helpers; scene evaluation
/// itself is total and reports [`Diagnostic`]s instead of `Result`s).
#[derive(Debug, Error)]
pub enum GeometryError {
    #[error("geometry: {0}")]
    Other(String),
}
