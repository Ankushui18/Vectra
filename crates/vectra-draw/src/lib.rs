//! # vectra-draw — the drawing tools' mathematics (Task 10.1)
//!
//! The professional drawing suite is a *UI* feature built on a locked parametric
//! engine, and this crate is the one piece of it that is neither UI nor engine:
//! the mathematics that has to be true in the same instant it is on screen.
//!
//! ```text
//!   ┌─ pen ────────────────┐   ┌─ brush ────────────────────────────────┐
//!   │ bezier.rs  (handles) │   │ stroke.rs  (pressure / velocity)       │
//!   │ pen.rs     (gestures)│   │ fit.rs     (cubics through samples)    │
//!   └──────────────────────┘   │ outline.rs (a stroke becomes a shape)  │
//!                              └────────────────────────────────────────┘
//!                        ┌─ quick shape ──────────────────┐
//!                        │ quickshape.rs (recognize + fit)│
//!                        └────────────────────────────────┘
//! ```
//!
//! ## Why it is a crate and not a folder in the web app
//!
//! Task 10.1's four laws are `proptest`s, and a `proptest` can only prove code it
//! can call. If the pen's handle algebra lived in TypeScript, the Handle Symmetry
//! Law would be a test of a *reimplementation* — the classic way a proven property
//! and a shipped behaviour drift apart. So the algebra lives here, native and
//! compiled to wasm, and the browser calls the very functions the laws exercise.
//!
//! ## What it produces: engine types, and only engine types
//!
//! Every public result is `Vec<PathSegment>` / `Vec<Point2>` / `Vec<Constraint>` —
//! [`vectra_core`] types with `Parameter<Point2>` control points. Nothing in this
//! crate knows about pixels, canvases, React or the renderer, and the web app
//! receives the same values a hand-authored node would carry. That is RULE 1's
//! requirement stated as a type signature.
//!
//! ## No geometry is re-implemented
//!
//! `vectra-geometry` remains the owner of *path semantics* (how segments become
//! rings, how primitives are built). What is here is the *inverse* direction,
//! which the engine deliberately does not have: turning a human gesture into
//! parametric geometry. Fitting a curve to noisy samples and recognizing a drawn
//! primitive are not operations on a `Document` — they are how a `Document` gets
//! made.

pub mod bezier;
pub mod fit;
pub mod outline;
pub mod pen;
pub mod quickshape;
pub mod stroke;

pub use bezier::{Anchor, HANDLE_FRACTION};
pub use fit::{fit_stroke, FitResult, FitStats};
pub use outline::{expand, Ring};
pub use pen::{draft_from_anchors, drag_handle, HandleSide, PathDraft, PenSession};
pub use quickshape::{plan_quick_shape, recognize, SnapKind, SnapPlan};
pub use stroke::{clean, stamp, BrushProfile, Sample, StampedPoint};

use vectra_core::Point2;

/// The whole brush pipeline, as one call: samples in, a filled closed path out.
///
/// This is the function the wasm boundary exposes as a single operation, and it
/// is worth reading as a table of the four stages:
///
/// | stage | module | what it decides |
/// | --- | --- | --- |
/// | `clean` | [`stroke`] | which samples are real |
/// | `stamp` | [`stroke`] | pressure, or velocity when there is none |
/// | `expand` | [`outline`] | the width becomes a contour |
/// | `fit` | [`fit`] | the contour becomes cubic segments |
///
/// The result is a centreline-free filled shape in engine vocabulary, which is
/// exactly RULE 3's *"a clean, expanded `Path` (a filled shape) … never a jagged
/// polyline"*.
#[derive(Debug, Clone, PartialEq)]
pub struct StrokeResult {
    /// The filled shape's first point.
    pub start: Point2,
    /// Its segments, closed.
    pub segments: Vec<vectra_core::PathSegment>,
    /// Per-sample widths, so the UI can show the taper while drawing.
    pub stamps: Vec<StampedPoint>,
    /// The fitted curve's statistics (segment count, worst error, depth).
    pub fit: FitStats,
    /// The outline ring the fit was computed from, for a live preview.
    pub ring: Ring,
}

/// Options for [`brush_stroke`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BrushOptions {
    /// The curve-fit tolerance in document units.
    pub tolerance: f64,
    /// Samples closer than this are dropped as noise.
    pub min_distance: f64,
    /// A step this many times the running step is treated as a glitch.
    pub spike_factor: f64,
    pub profile: BrushProfile,
}

impl Default for BrushOptions {
    fn default() -> Self {
        Self {
            // One document unit ≈ one screen pixel at 100 % zoom: below this the
            // user cannot see the difference between the fit and their stroke.
            tolerance: 0.35,
            min_distance: 0.5,
            spike_factor: 8.0,
            profile: BrushProfile::default(),
        }
    }
}

/// Turn a pointer trail into a branch-free filled path (RULE 3).
pub fn brush_stroke(samples: &[Sample], options: &BrushOptions) -> StrokeResult {
    let cleaned = clean(samples, options.min_distance, options.spike_factor);
    let stamps = stamp(&cleaned, &options.profile);
    let ring = outline::ensure_counter_clockwise(expand(&stamps));
    let points: Vec<Point2> = ring.clone();
    let mut fit = fit::fit_stroke(&points, options.tolerance);
    fit.segments.push(vectra_core::PathSegment::Close);
    StrokeResult {
        start: fit.start,
        segments: fit.segments,
        stamps,
        fit: fit.stats,
        ring,
    }
}
