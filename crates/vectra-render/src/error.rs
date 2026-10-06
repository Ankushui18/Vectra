//! Renderer errors (Task 5.0).
//!
//! Every failure here is a *renderer* failure, not an engine failure: the input
//! (`EvaluatedScene`) is already resolved and validated, so what can go wrong is
//! tessellation (degenerate geometry, a vertex budget blown), device acquisition
//! (no adapter, no surface), or a WGSL/buffer plumbing problem. Nothing in this
//! module knows about documents, parameters or constraints.

use thiserror::Error;
use vectra_core::NodeId;

#[derive(Debug, Error)]
pub enum RenderError {
    /// `lyon` refused the path, or produced a vertex that is not a finite
    /// number. A non-finite vertex must never reach a GPU buffer: it turns one
    /// bad shape into an invisible or corrupt frame.
    #[error("render: node {node} failed to tessellate: {message}")]
    Tessellation { node: NodeId, message: String },

    /// A single node exceeded the 32-bit index budget. Indices are `u32`
    /// because that is what WebGPU index buffers carry.
    #[error("render: node {node} needs {needed} vertices, over the {cap} vertex budget")]
    VertexBudget {
        node: NodeId,
        needed: usize,
        cap: u32,
    },

    /// No adapter matched the request (browser without WebGPU, headless CI, …).
    #[error("render: no suitable GPU adapter: {0}")]
    NoAdapter(String),

    /// The adapter refused the device request.
    #[error("render: GPU device request failed: {0}")]
    NoDevice(String),

    /// Surface creation / configuration / acquisition failed.
    #[error("render: GPU surface error: {0}")]
    Surface(String),

    /// The device's limits cannot carry the renderer's buffers.
    #[error("render: GPU limits are too small: {0}")]
    Limits(String),

    /// A bind-group / pipeline layout mismatch (a programming error, surfaced
    /// loudly rather than drawn wrongly).
    #[error("render: GPU pipeline error: {0}")]
    Pipeline(String),
}
