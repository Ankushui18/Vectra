//! The wasm `Renderer` (Task 5.0 §6): the WebGPU canvas, driven by the engine.
//!
//! ```text
//! React (dumb remote)          this module            vectra-render
//! ────────────────────         ───────────            ─────────────
//! canvas element        ──▶    attach()/measure()  ──▶ surface + pipelines
//! engine.render_frame() ──▶    present(scene,dirty)──▶ tessellate + delta writes
//! pointer client coords ──▶    pointer_hit()       ──▶ NodeId (RULE 3)
//! ```
//!
//! The division of labour is the point of the task. React owns *pixels and
//! pointers*; this module owns the *GPU*; `vectra-render` owns *tessellation,
//! the write plan and the spatial index*. React never learns a coordinate
//! system, never touches a buffer, and never decides what was clicked — it
//! hands over `clientX`/`clientY` and receives a `NodeId` or nothing.
//!
//! # Why this layer is host-testable
//!
//! Everything except adapter acquisition and surface creation is plain Rust:
//! the ledger that coalesces dirty ids between frames, the viewport, the
//! client→document transform, the pointer query. Only `attach` and `measure`
//! touch the DOM, and they are `#[cfg(target_arch = "wasm32")]` — so
//! `cargo test -p vectra-wasm` exercises the whole pointer chain headlessly, and
//! `tests/render_laws.rs` does exactly that.

use std::collections::BTreeSet;

use serde::Serialize;
use vectra_core::{EvalMode, NodeId};
use vectra_geometry::{DirtySet, EvaluatedScene};
use vectra_render::{Camera, GpuRenderer, GpuStats, RenderScene, CLEAR_COLOR};
use wasm_bindgen::prelude::*;

/// The document window the canvas opens with, in document units. The engine's
/// document space has no extent of its own — this is a *view* choice, made once,
/// in the same place the view transform lives.
pub const DOC_WIDTH: f32 = 800.0;
pub const DOC_HEIGHT: f32 = 600.0;

/// The canvas's box in client coordinates: exactly what
/// `getBoundingClientRect()` reports. A plain struct so the pointer path can be
/// tested without a DOM.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct CanvasRect {
    pub left: f64,
    pub top: f64,
    pub width: f64,
    pub height: f64,
}

impl CanvasRect {
    pub fn new(left: f64, top: f64, width: f64, height: f64) -> Self {
        Self {
            left,
            top,
            width,
            height,
        }
    }

    /// A click inside the box, in CSS pixels from its top-left corner.
    pub fn local(&self, client_x: f64, client_y: f64) -> (f64, f64) {
        (client_x - self.left, client_y - self.top)
    }

    pub fn is_drawable(&self) -> bool {
        self.width > 0.0 && self.height > 0.0
    }
}

/// The dirty ids that accumulated between two drawn frames (RULE 2, seen from
/// the engine's side).
///
/// A gesture produces a sample per pointer event — often eight or more per
/// frame. The renderer must not do eight partial uploads for one visible change,
/// so the engine *coalesces*: ids are unioned, and the frame that finally draws
/// receives one set. Because `RenderScene::sync` compares the new scene against
/// what the GPU already holds, the intermediate states cost literally nothing —
/// a 40-sample drag is one 80-byte write per moved node, not forty.
#[derive(Debug, Default, Clone)]
pub struct DirtyLedger {
    ids: BTreeSet<NodeId>,
    full: bool,
}

impl DirtyLedger {
    pub fn new() -> Self {
        Self::default()
    }

    /// Note a settled mutation. A `Full` evaluation supersedes everything: the
    /// whole scene is being rebuilt, so tracking individual ids would be a lie.
    pub fn note(&mut self, ids: impl IntoIterator<Item = NodeId>, mode: EvalMode) {
        if mode == EvalMode::Full {
            self.full = true;
            self.ids.clear();
            return;
        }
        if self.full {
            return;
        }
        self.ids.extend(ids);
    }

    pub fn is_empty(&self) -> bool {
        !self.full && self.ids.is_empty()
    }

    pub fn len(&self) -> usize {
        self.ids.len()
    }

    pub fn is_full(&self) -> bool {
        self.full
    }

    /// Consume the ledger: what the next frame must reconcile.
    pub fn take(&mut self) -> DirtySet {
        let dirty = if self.full {
            DirtySet::all()
        } else {
            DirtySet::nodes(std::mem::take(&mut self.ids))
        };
        self.full = false;
        dirty
    }
}

/// GPU-side numbers, flattened for the wire (`GpuStats` is `Eq`, not `Serialize`).
#[derive(Debug, Clone, Copy, Default, Serialize)]
pub struct GpuStatsWire {
    pub nodes: usize,
    pub buffers: usize,
    pub buffer_bytes: u64,
    pub instance_bytes: u64,
    pub reallocations: usize,
    pub draw_calls: usize,
}

impl From<GpuStats> for GpuStatsWire {
    fn from(stats: GpuStats) -> Self {
        Self {
            nodes: stats.nodes,
            buffers: stats.buffers,
            buffer_bytes: stats.buffer_bytes,
            instance_bytes: stats.instance_bytes,
            reallocations: stats.reallocations,
            draw_calls: stats.draw_calls,
        }
    }
}

/// What one drawn frame cost. This is the readout that makes RULE 2 visible:
/// `writes`/`bytes` are the *GPU traffic* of this frame, `moved`/`restyled`
/// explain it, and `dirty` says how much the engine asked for.
#[derive(Debug, Clone, Default, Serialize)]
pub struct FrameWire {
    pub frame: u64,
    pub ready: bool,
    /// Nodes the renderer holds buffers for after this frame.
    pub nodes: usize,
    /// Ids the ledger handed to this frame.
    pub dirty: usize,
    pub full: bool,
    /// Nodes whose buffers this frame actually wrote.
    pub touched: usize,
    pub writes: usize,
    pub bytes: u64,
    pub created: usize,
    pub retessellated: usize,
    pub moved: usize,
    pub restyled: usize,
    pub removed: usize,
    pub draw_calls: usize,
    /// Bytes of **ramp atlas** re-uploaded by this frame (Task 10.2 RULE 3):
    /// nonzero exactly when a gradient's stops or frame changed. Kept off
    /// `bytes`/`writes` on purpose — those count the *instance* traffic whose
    /// per-node cost the incrementality laws assert.
    pub ramps: usize,
    pub gpu: Option<GpuStatsWire>,
    pub error: Option<String>,
}

/// What a sync decided, copied off the borrowed plan (`WritePlan` is tied to
/// the scene it was computed from, and the frame record outlives that borrow).
#[derive(Debug, Clone, Copy, Default)]
struct PlanCounts {
    touched: usize,
    writes: usize,
    bytes: u64,
    ramps: usize,
    created: usize,
    retessellated: usize,
    moved: usize,
    restyled: usize,
    removed: usize,
}

/// The device + swap chain. Absent on the host and after a failed `attach`.
struct GpuSurface {
    renderer: GpuRenderer,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
}

/// The WebGPU renderer the React canvas drives.
#[wasm_bindgen]
pub struct Renderer {
    gpu: Option<GpuSurface>,
    scene: RenderScene,
    camera: Camera,
    rect: CanvasRect,
    /// Drawing-buffer size in device pixels.
    pixels: (u32, u32),
    /// Where the camera is pointed, when anyone has pointed it. `None` = the
    /// document window the canvas opens with.
    ///
    /// Two ways to name a camera, because there are two ways a user points one:
    /// the artboard dropdown says *"show me this rectangle"* ([`ViewState::Fit`],
    /// which must keep fitting it if the canvas is resized — resizing a window
    /// must not lose the artboard you are working on), while a pan or a wheel
    /// says *"I am here, at this zoom"* ([`ViewState::At`], which must keep its
    /// zoom when the canvas changes size, the way every editor behaves, showing
    /// more or less of the document rather than rescaling it).
    ///
    /// One state and one reader ([`Renderer::camera_for_screen`]) is the point:
    /// the shader's projection, `client_to_document`, and the overlay all come
    /// out of the same three numbers, so a gesture cannot move the picture
    /// without moving the pointer mapping with it.
    view_state: Option<ViewState>,
    frames: u64,
    gpu_stats: Option<GpuStatsWire>,
    error: Option<String>,
}

impl Default for Renderer {
    fn default() -> Self {
        Self::new()
    }
}

#[wasm_bindgen]
impl Renderer {
    /// A renderer with no GPU: the spatial index and the view transform work
    /// immediately, `attach` brings the pixels up. Nothing panics if the browser
    /// has no WebGPU — the UI shows what the frame report says.
    #[wasm_bindgen(constructor)]
    pub fn new() -> Self {
        Self {
            gpu: None,
            scene: RenderScene::new(),
            camera: Camera::fit(DOC_WIDTH, DOC_HEIGHT, [DOC_WIDTH, DOC_HEIGHT]),
            view_state: None,
            rect: CanvasRect::default(),
            pixels: (0, 0),
            frames: 0,
            gpu_stats: None,
            error: None,
        }
    }

    /// RULE 3: a client-space pointer position → the node under it, or `null`.
    ///
    /// The whole chain — client box → canvas pixels → document units → spatial
    /// index → exact containment — happens here, in Rust, using the *same*
    /// camera that projected the pixels. That is what makes the pointer agree
    /// with the picture, and what keeps the UI a dumb remote: it reports where
    /// the mouse is, the engine decides what that means.
    pub fn pointer_hit(&mut self, client_x: f64, client_y: f64) -> Option<String> {
        let (x, y) = self.client_to_document(client_x, client_y)?;
        self.scene
            .hit_test(x as f32, y as f32)
            .map(|id| id.to_string())
    }

    /// The document point under a client-space pointer position, as JSON
    /// (`{"x":…,"y":…}`). This is a *view* transform; it reads no parameter and
    /// no rule — the drag protocol needs it only to keep the grab offset while
    /// the pointer moves.
    pub fn pointer_doc(&self, client_x: f64, client_y: f64) -> String {
        match self.client_to_document(client_x, client_y) {
            Some((x, y)) => format!("{{\"x\":{x:.3},\"y\":{y:.3}}}"),
            None => String::from("null"),
        }
    }

    /// The viewport, from the DOM's own numbers. Returns the visible document
    /// rectangle as JSON (the browser gets the camera back, nothing else).
    pub fn set_viewport(
        &mut self,
        left: f64,
        top: f64,
        css_width: f64,
        css_height: f64,
        scale: f64,
    ) -> String {
        self.viewport(left, top, css_width, css_height, scale);
        self.view()
    }

    /// The camera the frame is drawn with: `[min_x, min_y, width, height]` in
    /// document units, as JSON.
    pub fn view(&self) -> String {
        let (x, y, w, h) = self.camera.visible();
        format!("{{\"x\":{x:.3},\"y\":{y:.3},\"w\":{w:.3},\"h\":{h:.3}}}")
    }

    /// **Document units per screen pixel** — the camera's scale, inverted.
    ///
    /// The drawing tools need it for one reason: a grab radius (a handle must be
    /// as easy to hit at 4× zoom as at 1×) and a stroke width (the brush's
    /// `min_distance` filter should not change meaning when the user zooms). It is
    /// derived from the same `Camera::visible` window the renderer draws with, so
    /// an overlay can never be calibrated differently from the scene beneath it.
    pub fn pixels_per_unit(&self) -> Option<f64> {
        if !self.rect.is_drawable() || self.pixels.0 == 0 || self.pixels.1 == 0 {
            return None;
        }
        let (_, _, width, _) = self.camera.visible();
        let width = width.abs() as f64;
        if width <= f64::EPSILON {
            return None;
        }
        // CSS pixels across the canvas ÷ document units across the visible
        // window. CSS pixels, not drawing-buffer pixels: the pointer arrives in
        // client coordinates, so this is the ratio that converts a grab radius
        // between the two.
        Some(self.rect.width / width)
    }

    /// Whether the GPU came up. `false` ⇒ the UI shows the reason from
    /// `stats().error` and stops scheduling frames.
    pub fn gpu_ready(&self) -> bool {
        self.gpu.is_some()
    }

    /// The last frame report (plus the accumulated GPU totals) as JSON.
    pub fn stats(&self) -> String {
        #[derive(Serialize)]
        struct Stats<'a> {
            ready: bool,
            frames: u64,
            pixels: (u32, u32),
            nodes: usize,
            draw_calls: usize,
            error: Option<&'a str>,
            gpu: Option<GpuStatsWire>,
        }
        let stats = Stats {
            ready: self.gpu.is_some(),
            frames: self.frames,
            pixels: self.pixels,
            nodes: self.scene.len(),
            draw_calls: self.scene.draw_calls(),
            error: self.error.as_deref(),
            gpu: self.gpu_stats,
        };
        serde_json::to_string(&stats).unwrap_or_else(|_| String::from("{}"))
    }
}

/// The Rust-facing half of the renderer: what the engine calls, and what the
/// host laws drive. Nothing here is `#[wasm_bindgen]`, so scene and dirty-set
/// arguments cross this boundary as Rust values, not as JSON.
impl Renderer {
    /// Hit-test a **document-space** point, returning the node under it.
    ///
    /// Task 6.0's hover driver: the engine asks the renderer's spatial index
    /// where the pointer is, in the same coordinates the drag path hit tests
    /// with (`pointer_hit` above is the client-space entry point). Plain Rust
    /// rather than a `#[wasm_bindgen]` method because it returns a
    /// [`vectra_core::NodeId`], which is not a JS type — the wasm surface
    /// reports ids as strings, and does so in `lib.rs`, not here. Takes `&mut`
    /// because the spatial index is built lazily on the first query.
    pub fn hit_test(&mut self, x: f64, y: f64) -> Option<vectra_core::NodeId> {
        self.scene.hit_test(x as f32, y as f32)
    }

    /// Client coordinates → document units. `None` when the canvas has no box
    /// yet (hidden, or not measured): a pointer event that cannot be placed is
    /// ignored rather than guessed.
    pub fn client_to_document(&self, client_x: f64, client_y: f64) -> Option<(f64, f64)> {
        if !self.rect.is_drawable() || self.pixels.0 == 0 || self.pixels.1 == 0 {
            return None;
        }
        let (local_x, local_y) = self.rect.local(client_x, client_y);
        // CSS box → drawing buffer: the ratio absorbs devicePixelRatio and any
        // rounding in the backing-store size.
        let sx = self.pixels.0 as f64 / self.rect.width;
        let sy = self.pixels.1 as f64 / self.rect.height;
        let (x, y) = self
            .camera
            .screen_to_document((local_x * sx) as f32, (local_y * sy) as f32);
        Some((x as f64, y as f64))
    }

    /// The frame report as JSON — what `render_frame` hands back to JS.
    pub fn frame_json(&self, frame: &FrameWire) -> String {
        serde_json::to_string(frame).unwrap_or_else(|_| String::from("{}"))
    }

    /// One frame: reconcile the scene against the GPU, then draw it.
    ///
    /// The engine calls this from `render_frame`; React only schedules it (once
    /// per animation frame, so a 40-sample drag settles once).
    pub fn present(&mut self, scene: &EvaluatedScene, dirty: &DirtySet) -> FrameWire {
        // The plan borrows the scene, so everything read off it is copied out
        // before the frame record is built.
        let counts = {
            let plan = self.scene.sync(scene, dirty);
            PlanCounts {
                touched: plan.touched().len(),
                writes: plan.ops.len(),
                bytes: plan.bytes() as u64,
                ramps: plan.ramps.unwrap_or(0),
                created: plan
                    .ops
                    .iter()
                    .filter(|op| op.kind == vectra_render::WriteKind::Create)
                    .count(),
                retessellated: plan.retessellated.len(),
                moved: plan.moved.len(),
                restyled: plan.restyled.len(),
                removed: plan.removed.len(),
            }
        };
        let mut frame = FrameWire {
            frame: self.frames + 1,
            ready: self.gpu.is_some(),
            nodes: self.scene.len(),
            dirty: dirty.len(),
            full: dirty.is_full(),
            touched: counts.touched,
            writes: counts.writes,
            bytes: counts.bytes,
            created: counts.created,
            retessellated: counts.retessellated,
            moved: counts.moved,
            restyled: counts.restyled,
            removed: counts.removed,
            draw_calls: self.scene.draw_calls(),
            ramps: counts.ramps,
            gpu: self.gpu_stats,
            error: None,
        };

        if let Some(gpu) = &mut self.gpu {
            match gpu.renderer.apply(&self.scene) {
                Ok(stats) => {
                    self.gpu_stats = Some(stats.into());
                    frame.gpu = self.gpu_stats;
                    frame.draw_calls = stats.draw_calls;
                }
                Err(error) => {
                    let message = error.to_string();
                    self.error = Some(message.clone());
                    frame.error = Some(message);
                }
            }
            if let Some(error) = self.draw() {
                self.error = Some(error.clone());
                frame.error = frame.error.or(Some(error));
            }
        }

        self.frames += 1;
        frame
    }

    /// Set the viewport from plain numbers: the canvas box in client
    /// coordinates, its CSS size, and the device pixel ratio.
    ///
    /// `measure` reads exactly these from the DOM; the host laws call it
    /// directly. Returns the drawing-buffer size.
    pub fn viewport(
        &mut self,
        left: f64,
        top: f64,
        css_width: f64,
        css_height: f64,
        scale: f64,
    ) -> (u32, u32) {
        let scale = if scale.is_finite() && scale > 0.0 {
            scale
        } else {
            1.0
        };
        let width = css_width.max(0.0);
        let height = css_height.max(0.0);
        self.rect = CanvasRect::new(left, top, width, height);
        self.pixels = (
            ((width * scale).round() as u32).max(1),
            ((height * scale).round() as u32).max(1),
        );
        self.camera = self.camera_for_screen();
        self.pixels
    }

    /// The camera for the current drawing-buffer size: the framed rectangle if
    /// there is one, else the document window the canvas opens with.
    ///
    /// **Contain**, both ways: the extra space of a box whose aspect does not
    /// match the rectangle shows *more document* rather than stretching the
    /// drawing, which is why the scale is a single number over both axes.
    fn camera_for_screen(&self) -> Camera {
        let screen = [self.pixels.0 as f32, self.pixels.1 as f32];
        match self.view_state {
            None => Camera::fit(DOC_WIDTH, DOC_HEIGHT, screen),
            Some(ViewState::Fit {
                x,
                y,
                width,
                height,
            }) => {
                let width = width.max(1.0);
                let height = height.max(1.0);
                let scale = (screen[0] / width)
                    .min(screen[1] / height)
                    .max(f32::MIN_POSITIVE);
                let visible = (screen[0] / scale, screen[1] / scale);
                let center = (x + width / 2.0, y + height / 2.0);
                Camera::window(
                    center.0 - visible.0 / 2.0,
                    center.1 - visible.1 / 2.0,
                    visible.0,
                    visible.1,
                    screen,
                )
            }
            // A gesture's camera: the centre and zoom it left behind, stretched
            // over whatever the canvas is now. No fitting, so a resize cannot
            // quietly change the zoom the designer chose.
            //
            // The stored zoom is **CSS pixels per unit**, so the camera's device
            // scale is that times the current ratio: on a 2× display "100%" still
            // means one document unit per CSS pixel, which is what the pointer
            // and the readout mean, and what a laptop-to-monitor move must not
            // silently double.
            Some(ViewState::At {
                center_x,
                center_y,
                scale,
            }) => {
                let css_scale = if scale.is_finite() && scale > 0.0 {
                    scale
                } else {
                    1.0
                };
                let scale = (css_scale as f64 * self.css_to_device()) as f32;
                let scale = if scale.is_finite() && scale > 0.0 {
                    scale
                } else {
                    1.0
                };
                let visible = ((screen[0] / scale).max(1.0), (screen[1] / scale).max(1.0));
                Camera::window(
                    center_x - visible.0 / 2.0,
                    center_y - visible.1 / 2.0,
                    visible.0,
                    visible.1,
                    screen,
                )
            }
        }
    }

    /// Install a camera as the current one: the field, the GPU's uniform, and
    /// the JSON the overlay reads, in one place so the three cannot disagree.
    ///
    /// Every camera writer goes through here — `frame_document`, `nav_pan`,
    /// `nav_zoom` — and the gesturing writers then record *the result* (centre
    /// and zoom) in `view_state`, so what the camera is and what the renderer
    /// remembers are always the same three numbers.
    fn set_camera(&mut self, camera: Camera) -> String {
        self.camera = camera;
        if let Some(gpu) = self.gpu.as_mut() {
            gpu.renderer.set_camera(self.camera);
        }
        self.view()
    }

    /// **May a gesture move this canvas at all?**
    ///
    /// Only a canvas with a measured box has a pointer map, and a gesture whose
    /// pointer cannot be placed is exactly the case `client_to_document` already
    /// refuses: `None`. So navigation refuses too — the camera is left exactly
    /// where it was rather than being re-derived from an invented screen size,
    /// which is what "a hidden canvas does not move when the user rolls the
    /// wheel over the wrong element" means in code.
    fn navigation_ready(&self) -> bool {
        self.rect.is_drawable() && self.pixels.0 > 0 && self.pixels.1 > 0
    }

    /// The camera the *next* gesture starts from: the camera for the canvas as
    /// it is now, so a gesture sees any resize it has not been told about.
    fn gesture_camera(&self) -> Camera {
        self.camera_for_screen()
    }

    /// Acquire the swap chain's next image and draw the scene into it. Returns
    /// an error string when the surface needs reconfiguring (the canvas was
    /// resized behind our back) or the device was lost.
    fn draw(&mut self) -> Option<String> {
        let gpu = self.gpu.as_mut()?;
        if self.pixels.0 == 0 || self.pixels.1 == 0 {
            return None; // a zero-area target cannot be rendered into
        }
        let frame = match gpu.surface.get_current_texture() {
            Ok(frame) => frame,
            Err(error) => {
                // A lost or outdated surface is recovered by reconfiguring once;
                // the frame is simply skipped, so the loop keeps running.
                gpu.surface.configure(gpu.renderer.device(), &gpu.config);
                return Some(format!("surface: {error}"));
            }
        };
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        // The target texture travels with the view since Task 10.2: a blended
        // draw snapshots the frame it is about to composite onto, and only the
        // texture (not the view) can be a copy source.
        gpu.renderer
            .render(&frame.texture, &view, &self.scene, CLEAR_COLOR);
        frame.present();
        None
    }
}

/// **The overlay's camera, across the wasm boundary** (Task 10.1 RULE 4).
///
/// Its own `#[wasm_bindgen]` block rather than a member of the JS surface above:
/// the `impl Renderer` blocks in this file are organised by *audience* — the first
/// is JavaScript, the second is the Rust-facing half the engine and the host laws
/// drive, and this one is the single method the drawing overlay needs in the
/// browser. Putting it anywhere else would have meant a JSON-shaped method in the
/// Rust API, or a DOM-shaped concern in the engine's.
#[wasm_bindgen]
impl Renderer {
    /// **Document points → client points**, as JSON — the camera the drawing
    /// overlay is placed with (Task 10.1 RULE 4).
    ///
    /// The overlay is DOM: anchors, Bézier handles and the in-progress path are
    /// SVG sitting on the WebGPU canvas. They are placed here, through
    /// `Camera::document_to_screen` and the same CSS-box/pixel ratios
    /// `client_to_document` inverts, for the same reason the pointer is read
    /// here: there is one camera. A handle that disagrees with the curve beneath
    /// it — and document space is **y-up** while the DOM is y-down, so a naive
    /// `viewBox` overlay would be mirrored — is worse than no handle at all.
    ///
    /// Input `[[x, y], …]` in document units, output `[[client_x, client_y], …]`
    /// in viewport pixels, so React renders without a transform of its own
    /// (Task 1.4's dumb-remote rule). A canvas with no box answers
    /// `{"ok":false,"points":[]}` rather than guessing.
    pub fn document_to_client(&self, points_json: &str) -> String {
        let empty = || String::from("{\"ok\":false,\"points\":[]}");
        if !self.rect.is_drawable() || self.pixels.0 == 0 || self.pixels.1 == 0 {
            return empty();
        }
        let Ok(points) = serde_json::from_str::<Vec<[f64; 2]>>(points_json) else {
            return empty();
        };
        // Backing store → CSS box, the inverse of `client_to_document`.
        let sx = self.pixels.0 as f64 / self.rect.width;
        let sy = self.pixels.1 as f64 / self.rect.height;
        if sx <= 0.0 || sy <= 0.0 {
            return empty();
        }
        let mapped: Vec<[f64; 2]> = points
            .iter()
            .map(|point| {
                let (px, py) = self
                    .camera
                    .document_to_screen(point[0] as f32, point[1] as f32);
                [px as f64 / sx, py as f64 / sy]
            })
            .collect();
        serde_json::json!({ "ok": true, "points": mapped }).to_string()
    }
}

#[cfg(target_arch = "wasm32")]
mod web {
    //! Everything that touches the DOM. On the host the API simply does not
    //! exist, which is why nothing above needs a browser to be tested.

    use super::*;

    #[wasm_bindgen]
    impl Renderer {
        /// Initialise WebGPU on `canvas` (Task 5.0 §6: React passes the element
        /// on mount). Resolves to a JSON status string; rejects only when the
        /// canvas cannot host a surface at all — a browser without WebGPU gets
        /// `{"ready":false,…}` so the UI can say so instead of crashing.
        pub async fn attach(
            &mut self,
            canvas: web_sys::HtmlCanvasElement,
        ) -> Result<String, JsValue> {
            let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
                backends: wgpu::Backends::BROWSER_WEBGPU,
                ..wgpu::InstanceDescriptor::default()
            });
            // The canvas is *owned* by the surface, so it cannot be dropped
            // while the swap chain lives — that is what `'static` buys.
            let surface = instance
                .create_surface(wgpu::SurfaceTarget::Canvas(canvas.clone()))
                .map_err(|error| JsValue::from_str(&format!("surface: {error}")))?;

            let adapter = instance
                .request_adapter(&wgpu::RequestAdapterOptions {
                    power_preference: wgpu::PowerPreference::HighPerformance,
                    compatible_surface: Some(&surface),
                    force_fallback_adapter: false,
                })
                .await
                .ok_or_else(|| JsValue::from_str("no WebGPU adapter"))?;

            let (device, queue) = adapter
                .request_device(&wgpu::DeviceDescriptor::default(), None)
                .await
                .map_err(|error| JsValue::from_str(&format!("device: {error}")))?;

            self.measure(&canvas)?;
            let mut config = surface
                .get_default_config(&adapter, self.pixels.0, self.pixels.1)
                .ok_or_else(|| JsValue::from_str("adapter cannot present to this surface"))?;
            // A blend mode other than Normal reads the frame so far, so the
            // surface must be copyable as well as presentable (Task 10.2 RULE 3).
            config.usage |= wgpu::TextureUsages::COPY_SRC;
            surface.configure(&device, &config);
            let renderer = GpuRenderer::new(device, queue, config.format)
                .map_err(|error| JsValue::from_str(&error.to_string()))?;
            self.gpu = Some(GpuSurface {
                renderer,
                surface,
                config,
            });
            self.error = None;
            Ok(self.stats())
        }

        /// Read the canvas box, resize the backing store, and update the camera.
        /// Cheap enough to call on every resize event (and on mount).
        pub fn measure(&mut self, canvas: &web_sys::HtmlCanvasElement) -> Result<String, JsValue> {
            let rect = canvas.get_bounding_client_rect();
            let scale = web_sys::window()
                .map(|window| window.device_pixel_ratio())
                .unwrap_or(1.0);
            let (css_w, css_h) = (rect.width(), rect.height());
            let (px_w, px_h) = (
                (css_w * scale).round().max(1.0) as u32,
                (css_h * scale).round().max(1.0) as u32,
            );
            let changed = (px_w, px_h) != self.pixels;
            canvas.set_width(px_w);
            canvas.set_height(px_h);
            self.viewport(rect.left(), rect.top(), css_w, css_h, scale);

            if changed {
                if let Some(gpu) = &mut self.gpu {
                    gpu.config.width = px_w;
                    gpu.config.height = px_h;
                    let device = gpu.renderer.device();
                    gpu.surface.configure(device, &gpu.config);
                }
            }
            Ok(self.view())
        }
    }
}

/// **The artboard camera, across the wasm boundary** (Task 10.2 RULE 2).
///
/// Its own `#[wasm_bindgen]` block for the same reason the overlay's camera
/// has one: the impl blocks in this file are organised by *audience*, and the
/// rectangle an artboard jump hands over is JavaScript's input. The method body
/// is the Rust one above — plain camera arithmetic, host-testable — so the
/// boundary adds a call, not a behaviour.
#[wasm_bindgen]
impl Renderer {
    /// **Frame a document rectangle** (Task 10.2 RULE 2) — what the artboard
    /// dropdown's jump and "fit" both mean.
    ///
    /// Pan and zoom belong to the camera, and the camera belongs to the renderer,
    /// so this is where the whole feature lives: the UI hands over the rectangle
    /// it read from the engine's artboard record (document units, y-up) and gets
    /// the camera back. No projection is computed anywhere else, and the
    /// rectangle is remembered so a resize keeps framing it.
    ///
    /// Returns the visible window as JSON — the same shape `view()` reports, so
    /// the overlay follows for free.
    pub fn frame_document(&mut self, x: f64, y: f64, width: f64, height: f64) -> String {
        self.view_state = Some(ViewState::Fit {
            x: x as f32,
            y: y as f32,
            width: width as f32,
            height: height as f32,
        });
        let camera = self.camera_for_screen();
        self.set_camera(camera)
    }

    /// Forget the framed rectangle and go back to the opening view of the whole
    /// document — the "zoom to fit everything" the canvas starts in.
    pub fn frame_document_default(&mut self) -> String {
        self.view_state = None;
        let camera = self.camera_for_screen();
        self.set_camera(camera)
    }

    // ── Navigation (Task 10.3 RULE 2) ──────────────────────────────────────
    //
    // A gesture arrives as one of exactly three things — a wheel at a pointer,
    // a drag delta, or a button asking for an absolute zoom — and each one is
    // *one* camera transition, applied to the camera on screen right now. The
    // arithmetic is `Camera`'s (`zoom_at`/`pan_by`/`zoom_to`), which is what the
    // navigation laws in `vectra-render/tests/camera_laws.rs` pin down; these
    // methods add the two things only the renderer can know: the drawing-buffer
    // size behind the pointer, and the fact that the camera must also reach the
    // GPU and the overlay.
    //
    // Pointers arrive in **CSS** pixels with the canvas's own origin (the same
    // convention as `write_pointer`), so the wheel position converts at this
    // boundary exactly once. Every method returns the visible window as JSON, so
    // the React overlay follows the gesture without a second round trip.

    /// **Pan by a pointer delta in CSS pixels** — a drag with the pan tool (or a
    /// middle-button drag) while looking at artwork.
    ///
    /// `dx`/`dy` are the pointer's own movement, +x right and +y down, because
    /// that is the data a `pointermove` event carries and re-deriving the flip at
    /// the call site is how signs get lost. The picture follows the pointer: the
    /// document point that was under the finger stays under the finger.
    pub fn nav_pan(&mut self, dx: f64, dy: f64) -> String {
        if !self.navigation_ready() {
            return self.view();
        }
        let scale = self.css_to_device();
        let camera = self
            .gesture_camera()
            .pan_by((dx * scale) as f32, (dy * scale) as f32);
        self.remember(camera)
    }

    /// **Zoom about a CSS-pixel position** — one notch of a wheel or a trackpad
    /// pinch. `factor > 1` zooms in.
    ///
    /// The position matters: the document point under the pointer is the one the
    /// user is looking at, so it is the one that must not move. That invariance
    /// is `Camera::zoom_at`'s definition and law 2 of the navigation laws.
    pub fn nav_zoom(&mut self, client_x: f64, client_y: f64, factor: f64) -> String {
        if !self.navigation_ready() {
            return self.view();
        }
        let Some((px, py)) = self.css_point(client_x, client_y) else {
            return self.view();
        };
        let camera = self.gesture_camera().zoom_at(px, py, factor as f32);
        self.remember(camera)
    }

    /// **Zoom to an absolute scale** about the middle of the canvas — the "100%",
    /// "200%" and "fit width" controls, and the keyboard shortcuts that mean the
    /// same thing. Absolute rather than relative so a button and its label can
    /// never disagree about what "%" means.
    ///
    /// `scale` is **CSS pixels per unit** — the unit `nav_scale` reports and the
    /// unit the buttons are labelled in — so the conversion to the camera's
    /// device scale happens here, at the same boundary every other CSS number in
    /// this file crosses. Omitting it was a real defect, caught by the dpr-1.5
    /// case of the readout law: "100%" was 66.7% on a 1.5x display.
    pub fn nav_zoom_to(&mut self, scale: f64) -> String {
        if !self.navigation_ready() {
            return self.view();
        }
        let device = scale * self.css_to_device();
        let camera = self.gesture_camera().zoom_to(device as f32);
        self.remember(camera)
    }

    /// The current zoom, in **CSS pixels per document unit** — the number the
    /// zoom readout prints as a percentage.
    ///
    /// CSS pixels, not device pixels: the readout is about what the user sees and
    /// what the pointer does, and a 2× display must not claim the artwork is at
    /// 200%. `pixels_per_unit` is the inverse of this for the tools' grab radii.
    pub fn nav_scale(&self) -> f64 {
        let ratio = self.css_to_device();
        if ratio <= 0.0 || !ratio.is_finite() {
            return self.gesture_camera().scale() as f64;
        }
        self.gesture_camera().scale() as f64 / ratio
    }

    /// Record a gesture's camera: the remembered centre and zoom, then the camera
    /// itself. One writer, so the state that survives a resize is exactly the
    /// state the laws have just checked.
    fn remember(&mut self, camera: Camera) -> String {
        let (x, y, w, h) = camera.visible();
        let ratio = self.css_to_device();
        let css_scale = if ratio > 0.0 && ratio.is_finite() {
            (camera.scale() as f64 / ratio) as f32
        } else {
            camera.scale()
        };
        self.view_state = Some(ViewState::At {
            center_x: x + w / 2.0,
            center_y: y + h / 2.0,
            scale: css_scale,
        });
        self.set_camera(camera)
    }

    /// Device pixels per CSS pixel for the canvas box — the ratio `viewport`
    /// recorded, re-derived from the two sizes it was told.
    fn css_to_device(&self) -> f64 {
        if self.rect.width <= 0.0 || self.pixels.0 == 0 {
            return 1.0;
        }
        self.pixels.0 as f64 / self.rect.width
    }

    /// A CSS-pixel position in the canvas → **device** pixels in the drawing
    /// buffer, the frame the camera's `screen` row is expressed in.
    fn css_point(&self, client_x: f64, client_y: f64) -> Option<(f32, f32)> {
        if !self.rect.is_drawable() || self.pixels.0 == 0 || self.pixels.1 == 0 {
            return None;
        }
        let (local_x, local_y) = self.rect.local(client_x, client_y);
        let sx = self.pixels.0 as f64 / self.rect.width;
        let sy = self.pixels.1 as f64 / self.rect.height;
        Some(((local_x * sx) as f32, (local_y * sy) as f32))
    }
}

/// **Where the camera is pointed**, when anyone has pointed it (Task 10.3 RULE 2).
///
/// Two variants because the two ways of aiming a camera survive a resize
/// differently, and the difference is a promise to the user rather than an
/// implementation detail — see the field comment on `Renderer::view_state`.
#[derive(Clone, Copy, Debug)]
enum ViewState {
    /// Fit this document rectangle into the canvas, as tightly as its aspect
    /// ratio allows (the artboard jump and "fit"). Re-derived on every resize.
    Fit {
        x: f32,
        y: f32,
        width: f32,
        height: f32,
    },
    /// The centre of the view in document units and how many **CSS** pixels one
    /// document unit covers — what a pan or a wheel leaves behind. Held across a
    /// resize (showing more or less artwork instead of rescaling it) and across a
    /// device-pixel-ratio change (100% stays 100%, whatever the display).
    At {
        center_x: f32,
        center_y: f32,
        scale: f32,
    },
}
