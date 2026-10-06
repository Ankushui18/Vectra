//! `vectra-render` — the WebGPU renderer (Task 5.0, MES §13).
//!
//! ```text
//! Document ──[GeometryEvaluator]──▶ EvaluatedScene ──[RenderScene]──▶ GpuRenderer ──▶ Surface
//!  (parametric)      (evaluator)      (concrete)     (tessellate + delta)   (wgpu)
//! ```
//!
//! # RULE 1 — the renderer is a dumb consumer
//!
//! Nothing in this crate knows what a variable, an expression, a constraint or an
//! operation is. The input is an [`vectra_geometry::EvaluatedScene`]: every
//! parameter already resolved, every primitive already concrete, the draw order
//! already decided. The renderer tessellates and draws. It never evaluates, never
//! solves, and never reads a `Document`.
//!
//! # RULE 2 — strict incremental GPU updates
//!
//! [`RenderScene::sync`] turns a dirty set into a [`WritePlan`] of *surgical*
//! writes; [`RenderScene::flush`] applies that plan through a [`BufferSink`],
//! which is either [`GpuRenderer`] (real `wgpu` buffers) or [`MockSink`] (the
//! law tests' byte-exact ledger). Three change classes, three costs:
//!
//! | change | work |
//! |---|---|
//! | reshape / resize | re-tessellate that node → its own vertex + index buffers |
//! | move (drag, solver partner) | one 80-byte instance write |
//! | restyle (colour, width, opacity, blend) | one 80-byte instance write |
//! | dirty but unchanged | **nothing** |
//!
//! # RULE 3 — hit testing is mandatory
//!
//! [`RenderScene::hit_test`] answers "(x, y) → which node?" with a bounding-box
//! grid for candidates and an even-odd containment test for the answer, and the
//! UI translates that into `BeginDrag`. The engine never sees pixels; the
//! renderer never sees documents.
//!
//! # Testing without a GPU
//!
//! Only device acquisition and buffer allocation need a real adapter. Tessellation,
//! the write plan, the sink ledger, hit testing and *shader validity* (parsed and
//! type-checked by `naga`, the same front end wgpu uses) all run on the host — see
//! `tests/`.

pub mod error;
pub mod geometry;
pub mod gpu;
pub mod hit;
pub mod instance;
pub mod scene;
pub mod tessellate;

pub use error::RenderError;
pub use geometry::{
    flatten, rings_local, rings_to_path, Bounds, Flattened, GeometryKey, Mesh, MeshKind, FILL_RULE,
    FLATTEN_TOLERANCE,
};
pub use gpu::{GpuRenderer, GpuStats};
pub use hit::HitIndex;
pub use instance::{Camera, InstanceRaw};
pub use scene::{
    contains_point, BufferSink, MockSink, NodeSlot, RenderScene, SinkCall, UpdateReport, WriteKind,
    WriteOp, WritePlan, CLEAR_COLOR,
};
pub use tessellate::{tessellate, tessellate_flattened, GpuVertex, Tessellation};

/// The renderer crate's version, surfaced at the wasm boundary so the UI can
/// show which build of the renderer is live.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
