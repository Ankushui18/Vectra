//! Per-draw-item uniform data: the **instance** (Task 5.0 §1/§2, extended by
//! Task 10.2 RULE 3).
//!
//! One `InstanceRaw` per **draw item** — one fill or one stroke of one node's
//! paint stack — all of them in a single storage buffer indexed by the draw
//! call's instance range. That layout is what makes RULE 2 achievable: a style
//! change is one 80-byte `write_buffer` at `slot * STRIDE`, and *nothing else in
//! the buffer moves*.
//!
//! It used to be one row per node, which is exactly as long as a node has one
//! fill and one stroke. A stack of paints needs a row each: the black stroke and
//! the white stroke differ only in their union data, so they are the same
//! geometry drawn with two rows. A node therefore owns a *run* of rows (its draw
//! items, in stack order) rather than a single row, and `NodeSlot::items` records
//! which row draws which mesh.
//!
//! # Gradients
//!
//! A gradient needs more than four numbers, so a row carries its frame and a
//! *window* into a second storage buffer: the **ramp table**
//! ([`RampTable`]), an array of `vec4<f32>` rows holding every stop of every
//! gradient in the scene. `ramp.x` selects the paint kind, `ramp.y`/`ramp.z` the
//! window, and the fragment stage walks the window — which keeps a gradient a
//! *data* feature of the same pipeline instead of a second shader.
//!
//! # Why the bytes are packed explicitly
//!
//! Both structs are rows of `vec4<f32>`: 16-byte-aligned, no interior padding,
//! so their byte image *is* their field order. That makes hand-packing exact —
//! and it avoids a real trap: `encase` (the workspace's WGSL serializer) maps
//! Rust's `[f32; 4]` to WGSL's `array<f32, 4>` rather than `vec4<f32>`, whose
//! 4-byte stride is not a legal storage-buffer stride at all. `[f32; 4]` is the
//! right *Rust* shape for a vec4 and the wrong *encase* shape, so the packing is
//! explicit here and the unit tests pin every offset the shader depends on.

use std::collections::BTreeMap;

use vectra_core::{BlendMode, Color, NodeId};
use vectra_geometry::{EvaluatedAppearance, EvaluatedPaint, EvaluatedStyle};

mod raw {
    /// Per-node GPU state.
    ///
    /// * `transform` — `[scale_x, scale_y, translate_x, translate_y]`. Phase 1
    ///   emits unit scales (the `EvaluatedScene` has no scale concept); the field
    ///   exists because the shader's job is "apply the node's resolved
    ///   transform", and a future `Transform` node lands here without touching
    ///   the pipeline.
    /// * `color` — straight (non-premultiplied) RGBA in `0..=1`, with the
    ///   layer's opacity **and** the node's global opacity folded into `a`.
    /// * `params` — `[stroke_width, opacity, selected, blend]`. `stroke_width`
    ///   travels for the inspector/SDF future; the shader does not need it
    ///   because lyon already expanded the stroke. `selected` is the selection
    ///   outline (Task 5.1). `blend` is the `BlendMode` code: `0` normal, `1`
    ///   multiply, `2` screen, `3` overlay — a *number*, not a pipeline, so the
    ///   four modes share two pipelines and a mode change is one row write.
    /// * `ramp` — `[kind, stop_offset, stop_count, 0]`: `kind` 0 = solid (the
    ///   `color` row is the paint), 1 = linear, 2 = radial; the window into the
    ///   ramp table is `(stop_offset, stop_count)` **in stops**, not rows.
    /// * `frame` — the gradient's frame in document space: linear is
    ///   `(start.x, start.y, end.x, end.y)`, radial is `(center.x, center.y,
    ///   radius, 0)`.
    #[derive(Debug, Clone, Copy, PartialEq)]
    pub struct InstanceRaw {
        pub transform: [f32; 4],
        pub color: [f32; 4],
        pub params: [f32; 4],
        pub ramp: [f32; 4],
        pub frame: [f32; 4],
    }

    /// The camera: where the document window is, and how big the target is.
    ///
    /// * `view` — `[min_x, min_y, width, height]` in document units (the visible
    ///   rectangle). Pan and zoom live here, entirely on the renderer's side: the
    ///   engine's document coordinates never change because the user zoomed.
    ///   `height` is **negative** for the default framing: document space is
    ///   y-up (the evaluator's convention, and what the pre-WebGPU SVG preview
    ///   drew), while a texture's rows run downward. The sign carries the flip,
    ///   and every screen↔document conversion below is derived from it — one
    ///   convention, one implementation, no per-call `if`s.
    /// * `screen` — `[width_px, height_px, 0, 0]`, for future pixel-space work
    ///   (grid lines, hairlines, the selection overlay).
    #[derive(Debug, Clone, Copy, PartialEq)]
    pub struct Camera {
        pub view: [f32; 4],
        pub screen: [f32; 4],
    }
}

pub use raw::{Camera, InstanceRaw};

/// Paint kinds, as the shader reads them from `ramp.x`.
pub const PAINT_SOLID: f32 = 0.0;
pub const PAINT_LINEAR: f32 = 1.0;
pub const PAINT_RADIAL: f32 = 2.0;

impl InstanceRaw {
    /// Bytes per instance. Exposed so the buffer planner and the tests agree with
    /// `encase` instead of assuming a number.
    pub const STRIDE: u64 = 80;

    /// Build the instance for **one draw item**: the `index`-th layer of
    /// `style.appearances`, in the given local frame.
    ///
    /// Returns `None` for a layer that has nothing to draw — that is the same
    /// predicate [`vectra_geometry::EvaluatedAppearance::is_visible`] exposes, so
    /// the renderer, the exporters and the inspector agree on "invisible".
    pub fn for_appearance(
        node: NodeId,
        origin: (f32, f32),
        style: &EvaluatedStyle,
        appearance: &EvaluatedAppearance,
        index: usize,
        ramps: &RampTable,
        selected: bool,
    ) -> Option<Self> {
        if !appearance.is_visible() {
            return None;
        }
        let opacity = (style.opacity.clamp(0.0, 1.0) * appearance.opacity.clamp(0.0, 1.0)) as f32;
        let width = appearance
            .stroke_width()
            .map(|width| width.max(0.0) as f32)
            .unwrap_or(0.0);
        let (kind, window) = ramps.window(node, index as u32, appearance);
        let color = match &appearance.paint {
            EvaluatedPaint::Solid(color) => with_alpha(*color, opacity),
            // A gradient's own alpha lives in its stops; the `color` row keeps a
            // usable fallback (the first stop) so a shader that ignores the ramp
            // still paints the artwork's colour rather than black.
            _ => with_alpha(appearance.paint.preview_color(), opacity),
        };
        let frame = match &appearance.paint {
            EvaluatedPaint::Solid(_) => [0.0, 0.0, 0.0, 0.0],
            EvaluatedPaint::Linear { start, end, .. } => {
                [start.x as f32, start.y as f32, end.x as f32, end.y as f32]
            }
            EvaluatedPaint::Radial { center, radius, .. } => [
                center.x as f32,
                center.y as f32,
                radius.max(0.0) as f32,
                0.0,
            ],
        };
        Some(Self {
            transform: [1.0, 1.0, origin.0, origin.1],
            color,
            params: [
                width,
                opacity,
                if selected { 1.0 } else { 0.0 },
                blend_code(appearance.blend),
            ],
            ramp: [
                kind,
                window.0 as f32,
                window.1 as f32,
                u32::from(appearance.is_stroke()) as f32,
            ],
            frame,
        })
    }

    /// The serialized bytes, exactly what `queue.write_buffer` takes: five
    /// `vec4<f32>` rows, in the shader's declaration order.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(Self::STRIDE as usize);
        for row in [
            self.transform,
            self.color,
            self.params,
            self.ramp,
            self.frame,
        ] {
            push_vec4(&mut out, row);
        }
        out
    }
}

/// The `BlendMode` code the shader switches on.
pub fn blend_code(mode: BlendMode) -> f32 {
    match mode {
        BlendMode::Normal => 0.0,
        BlendMode::Multiply => 1.0,
        BlendMode::Screen => 2.0,
        BlendMode::Overlay => 3.0,
    }
}

/// The largest ramp the GPU will draw. Longer gradients still *exist* (the
/// evaluator and the SVG exporter have no cap) — the renderer draws the first
/// sixteen stops, and the UI clamps its gradient bar to the same number so a
/// designer never meets the limit by surprise. Sixteen is where the ramp stops
/// being visibly smoother and starts being data entry.
pub const MAX_RAMP_STOPS: usize = 16;

/// Every gradient stop in the scene, in one storage array.
///
/// Layout: two `vec4<f32>` rows per stop — `(offset, r, g, b)` then
/// `(alpha, 0, 0, 0)` — because a stop has five meaningful numbers and a `vec4`
/// holds four. The window in [`InstanceRaw::ramp`] counts **stops**, so a shader
/// reads `rows[2 * (offset + i)]`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RampTable {
    rows: Vec<[f32; 4]>,
    windows: BTreeMap<(NodeId, u32), (u32, u32)>,
}

impl RampTable {
    /// Append one node's gradients, in stack order: the caller decides the
    /// order, and it must be a **stable** one (the scene's own order does), or a
    /// window would move whenever an unrelated node appeared.
    pub fn extend(&mut self, id: NodeId, style: &EvaluatedStyle) {
        for (index, layer) in style.appearances.iter().enumerate() {
            let stops = layer.paint.stops();
            if stops.is_empty() {
                continue;
            }
            let offset = (self.rows.len() / 2) as u32;
            let count = stops.len().min(MAX_RAMP_STOPS) as u32;
            for stop in stops.iter().take(count as usize) {
                self.rows.push([
                    stop.offset as f32,
                    stop.color.r as f32 / 255.0,
                    stop.color.g as f32 / 255.0,
                    stop.color.b as f32 / 255.0,
                ]);
                self.rows.push([stop.color.a as f32 / 255.0, 0.0, 0.0, 0.0]);
            }
            self.windows.insert((id, index as u32), (offset, count));
        }
    }

    /// The `(kind, window)` a draw item's `ramp` row carries.
    fn window(
        &self,
        node: NodeId,
        index: u32,
        appearance: &EvaluatedAppearance,
    ) -> (f32, (u32, u32)) {
        let kind = match &appearance.paint {
            EvaluatedPaint::Solid(_) => PAINT_SOLID,
            EvaluatedPaint::Linear { .. } => PAINT_LINEAR,
            EvaluatedPaint::Radial { .. } => PAINT_RADIAL,
        };
        let window = self.windows.get(&(node, index)).copied().unwrap_or((0, 0));
        (kind, window)
    }

    pub fn rows(&self) -> &[[f32; 4]] {
        &self.rows
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// The bytes the ramp storage buffer holds.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.rows.len() * 16);
        for row in &self.rows {
            push_vec4(&mut out, *row);
        }
        out
    }
}

/// Append one `vec4<f32>` row little-endian — the byte order every WebGPU
/// backend expects for `Float32x4`/array data.
fn push_vec4(out: &mut Vec<u8>, row: [f32; 4]) {
    for value in row {
        out.extend_from_slice(&value.to_le_bytes());
    }
}

/// Straight-alpha colour in `0..=1`, opacity folded into `a` (the blend state
/// multiplies by `src.a`, so opacity belongs there, not in a separate uniform).
pub fn with_alpha(color: Color, opacity: f32) -> [f32; 4] {
    [
        color.r as f32 / 255.0,
        color.g as f32 / 255.0,
        color.b as f32 / 255.0,
        (color.a as f32 / 255.0) * opacity,
    ]
}

impl Camera {
    /// A camera that shows exactly the document rectangle `[0, 0, width,
    /// height]`, y-up — the framing the SVG preview used, so the WebGPU canvas
    /// and the retired projection agree on what "the canvas" means.
    ///
    /// `view.y` is the rectangle's **top** edge in document units and `view.w`
    /// is `-height`; see the `view` field for why the sign lives there.
    pub fn framing(width: f32, height: f32) -> Self {
        let w = width.max(1.0);
        let h = height.max(1.0);
        Self {
            view: [0.0, h, w, -h],
            screen: [w, h, 0.0, 0.0],
        }
    }

    /// A camera over an arbitrary document rectangle and target size. The
    /// primitive both [`Camera::framing`] and [`Camera::fit`] are built from.
    pub fn window(min_x: f32, min_y: f32, width: f32, height: f32, screen: [f32; 2]) -> Self {
        Self {
            view: [
                min_x,
                min_y + height.max(1.0),
                width.max(1.0),
                -height.max(1.0),
            ],
            screen: [screen[0].max(1.0), screen[1].max(1.0), 0.0, 0.0],
        }
    }

    /// The default framing stretched to a target of any aspect ratio, showing
    /// **at least** all of `[0, 0, width, height]` and centring it in the extra
    /// space (CSS `object-fit: contain`).
    ///
    /// Contain rather than stretch: a canvas box that is not exactly the
    /// document's aspect ratio must not distort the drawing, because the user is
    /// judging shape. The visible document rectangle grows on the longer axis.
    pub fn fit(width: f32, height: f32, screen: [f32; 2]) -> Self {
        let (doc_w, doc_h) = (width.max(1.0), height.max(1.0));
        let (sw, sh) = (screen[0].max(1.0), screen[1].max(1.0));
        let doc_aspect = doc_w / doc_h;
        let screen_aspect = sw / sh;
        if screen_aspect >= doc_aspect {
            // Wider than the document: keep the height, widen the window.
            let view_w = doc_h * screen_aspect;
            let extra = (view_w - doc_w) / 2.0;
            Self::window(-extra, 0.0, view_w, doc_h, [sw, sh])
        } else {
            // Taller than the document: keep the width, grow the height.
            let view_h = doc_w / screen_aspect;
            let extra = (view_h - doc_h) / 2.0;
            Self::window(0.0, -extra, doc_w, view_h, [sw, sh])
        }
    }

    /// Document point → screen pixel, origin at the target's **top-left** with
    /// +y downward — the coordinate system a pointer event arrives in.
    ///
    /// Derived from the same `view` rectangle the shader projects with, so the
    /// pixel under a document point is the pixel the GPU paints it on.
    pub fn document_to_screen(&self, x: f32, y: f32) -> (f32, f32) {
        let u = (x - self.view[0]) / self.view[2];
        let v = (y - self.view[1]) / self.view[3];
        (u * self.screen[0], v * self.screen[1])
    }

    /// Screen pixel → document point: the exact inverse of
    /// [`Camera::document_to_screen`]. This is RULE 3's entry point — a raw
    /// pointer coordinate becomes a document coordinate here and nowhere else.
    pub fn screen_to_document(&self, px: f32, py: f32) -> (f32, f32) {
        let u = px / self.screen[0];
        let v = py / self.screen[1];
        (
            self.view[0] + u * self.view[2],
            self.view[1] + v * self.view[3],
        )
    }

    /// The document rectangle currently visible: `(min_x, min_y, w, h)` with
    /// `h` positive. For the inspector and for tests; the shader reads the raw
    /// `view` row.
    pub fn visible(&self) -> (f32, f32, f32, f32) {
        let (w, h) = (self.view[2], self.view[3]);
        let (min_y, height) = if h < 0.0 {
            (self.view[1] + h, -h)
        } else {
            (self.view[1], h)
        };
        (self.view[0], min_y, w, height)
    }

    // ── Navigation (Task 10.3 RULE 2: "pan and zoom between artboards") ────
    //
    // Four methods, all pure, all total: each returns a camera and never
    // mutates. A gesture is therefore a *fold* over pointer samples, which is
    // what makes a wheel burst one value instead of a state machine, and what
    // lets the laws below be written as equalities rather than as transcripts.
    //
    // The units are the camera's own: `screen` is in *device* pixels (the same
    // row the shader projects with), so a caller holding CSS pixels converts
    // once, at the boundary, exactly as `client_to_document` already does.

    /// **Screen pixels per document unit** — the zoom, as a number.
    ///
    /// One number because the camera is uniform: `fit` and `window` both derive
    /// both axes from one rectangle, and a canvas that scaled x and y differently
    /// would not be showing shapes, it would be showing a distortion. The UI's
    /// "240%" readout and the clamp below are the same quantity.
    pub fn scale(&self) -> f32 {
        let width = self.view[2];
        if !width.is_finite() || width == 0.0 {
            return 1.0;
        }
        self.screen[0] / width
    }

    /// **Zoom about a screen point**: whatever document point is under
    /// `(px, py)` stays under it.
    ///
    /// This is the whole reason a wheel event carries a pointer position. The
    /// implementation is the definition: read the document point, scale the
    /// window, and solve for the origin that keeps that point's normalised
    /// position unchanged — `u` and `v` below are that invariance, stated once.
    ///
    /// The factor is clamped into [`Camera::MIN_SCALE`]..[`Camera::MAX_SCALE`],
    /// and the *clamped* factor is what gets applied, so a wheel already at the
    /// limit is exactly a no-op rather than a slow drift.
    pub fn zoom_at(&self, px: f32, py: f32, factor: f32) -> Camera {
        if !factor.is_finite() || factor <= 0.0 || !px.is_finite() || !py.is_finite() {
            return *self;
        }
        let scale = self.scale();
        let target = (scale * factor).clamp(Self::MIN_SCALE, Self::MAX_SCALE);
        let applied = target / scale;
        if !applied.is_finite() || (applied - 1.0).abs() <= f32::EPSILON {
            return *self;
        }
        let (dx, dy) = self.screen_to_document(px, py);
        let u = px / self.screen[0];
        let v = py / self.screen[1];
        let width = self.view[2] / applied;
        let height = self.view[3] / applied;
        Camera {
            view: [dx - u * width, dy - v * height, width, height],
            screen: self.screen,
        }
    }

    /// **Pan by a screen-pixel delta**: the document moves with the pointer.
    ///
    /// `dx`/`dy` are in the pointer's own direction — `+dx` to the right, `+dy`
    /// downwards — so a drag can pass its movement straight through without the
    /// y-flip being re-derived (and re-broken) at every call site. Zoom is
    /// untouched, which is what makes panning a pure translation.
    pub fn pan_by(&self, dx: f32, dy: f32) -> Camera {
        if !dx.is_finite() || !dy.is_finite() {
            return *self;
        }
        let scale = self.scale();
        if !scale.is_finite() || scale <= 0.0 {
            return *self;
        }
        Camera {
            view: [
                self.view[0] - dx / scale,
                self.view[1] + dy / scale,
                self.view[2],
                self.view[3],
            ],
            screen: self.screen,
        }
    }

    /// **Zoom to an absolute scale** about the middle of the target — the
    /// "100%" button. `zoom_at` with the centre for a pointer, so there is one
    /// implementation of zooming and one place for the clamp to live.
    pub fn zoom_to(&self, scale: f32) -> Camera {
        let current = self.scale();
        if !current.is_finite() || current <= 0.0 || !scale.is_finite() || scale <= 0.0 {
            return *self;
        }
        self.zoom_at(self.screen[0] * 0.5, self.screen[1] * 0.5, scale / current)
    }

    /// The zoom limits: 1% (a 32,000-unit poster on an 800-pixel canvas) to
    /// 6400% (the corner of an icon at handle precision). Stated as constants
    /// because the UI shows a percentage and the laws assert the clamp; if the
    /// two ever disagreed, one of them would be lying to the designer.
    pub const MIN_SCALE: f32 = 0.01;
    pub const MAX_SCALE: f32 = 64.0;

    /// Two `vec4<f32>` rows: 32 bytes.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(32);
        push_vec4(&mut out, self.view);
        push_vec4(&mut out, self.screen);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: u128) -> NodeId {
        NodeId::from_u128(n)
    }

    /// One draw item, built the way the renderer builds it.
    fn instance(
        style: &EvaluatedStyle,
        index: usize,
        origin: (f32, f32),
        ramps: &RampTable,
    ) -> InstanceRaw {
        let node = id(1);
        InstanceRaw::for_appearance(
            node,
            origin,
            style,
            &style.appearances[index],
            index,
            ramps,
            false,
        )
        .expect("a visible layer")
    }

    fn ramps_for(style: &EvaluatedStyle) -> RampTable {
        let mut ramps = RampTable::default();
        ramps.extend(id(1), style);
        ramps
    }

    #[test]
    fn an_instance_is_exactly_eighty_bytes() {
        let style = EvaluatedStyle::solid(Color::rgb(1, 2, 3), Color::rgba(4, 5, 6, 128), 2.5, 0.5);
        let ramps = ramps_for(&style);
        let fill = instance(&style, 0, (10.0, 20.0), &ramps);
        let bytes = fill.to_bytes();
        assert_eq!(bytes.len(), InstanceRaw::STRIDE as usize);
        // WGSL sees five vec4<f32> rows in order: transform, color, params,
        // ramp, frame.
        let read = |offset: usize| {
            f32::from_le_bytes([
                bytes[offset],
                bytes[offset + 1],
                bytes[offset + 2],
                bytes[offset + 3],
            ])
        };
        // transform = scale(1, 1), translate(10, 20)
        assert_eq!(read(0), 1.0);
        assert_eq!(read(8), 10.0);
        assert_eq!(read(12), 20.0);
        // color = rgb(1,2,3), node opacity 0.5 × layer opacity 1.0 in alpha
        assert!((read(16) - 1.0 / 255.0).abs() < 1e-6);
        assert!((read(28) - 0.5).abs() < 1e-6);
        // params = (stroke_width, opacity, selected, blend): a fill has no width
        assert_eq!(read(32), 0.0);
        assert_eq!(read(36), 0.5, "opacity in params.y");
        assert_eq!(read(40), 0.0, "not selected");
        assert_eq!(read(44), 0.0, "blend: normal");
        // ramp = (kind, stop_offset, stop_count, is_stroke) of a solid
        assert_eq!(read(48), PAINT_SOLID);
        assert_eq!(read(60), 0.0, "a fill is not a stroke");
        // frame is empty for a solid
        assert_eq!(read(64), 0.0);

        // The stroke layer carries its width and its own alpha.
        let stroke = instance(&style, 1, (10.0, 20.0), &ramps);
        let bytes = stroke.to_bytes();
        assert!((read_of(&bytes, 28) - (128.0 / 255.0) * 0.5).abs() < 1e-6);
        assert_eq!(read_of(&bytes, 32), 2.5, "the stroke's width");
        assert_eq!(read_of(&bytes, 60), 1.0, "is_stroke");
    }

    fn read_of(bytes: &[u8], offset: usize) -> f32 {
        f32::from_le_bytes([
            bytes[offset],
            bytes[offset + 1],
            bytes[offset + 2],
            bytes[offset + 3],
        ])
    }

    #[test]
    fn a_row_of_instances_has_the_declared_stride() {
        let style = EvaluatedStyle::default();
        let ramps = ramps_for(&style);
        let a = instance(&style, 0, (0.0, 0.0), &ramps);
        let b = instance(&style, 0, (100.0, 200.0), &ramps);
        let mut bytes = a.to_bytes();
        bytes.extend(b.to_bytes());
        assert_eq!(bytes.len(), 2 * InstanceRaw::STRIDE as usize);
        // Row 1 begins exactly at the stride, which is what the slot-offset
        // write in `GpuRenderer::write` depends on.
        assert_eq!(read_of(&bytes, 72), 0.0, "row 0's translate ends there");
        assert_eq!(read_of(&bytes, 88), 100.0, "row 1 starts at the stride");
        assert_eq!(read_of(&bytes, 92), 200.0);
    }

    #[test]
    fn the_instance_layout_matches_the_wgsl_struct() {
        // Every offset the shader reads, spelled out. If a field is added or
        // reordered, this test and `shader.wgsl`'s `struct Instance` must move
        // together — which is the point.
        let style = EvaluatedStyle::default();
        let instance = instance(&style, 0, (7.0, 9.0), &ramps_for(&style));
        assert_eq!(instance.to_bytes().len(), 80);
        assert_eq!(InstanceRaw::STRIDE, 80);
        assert_eq!(std::mem::size_of::<InstanceRaw>(), 80);
        assert_eq!(std::mem::size_of::<Camera>(), 32);
    }

    #[test]
    fn opacity_is_folded_in_alpha_not_left_beside_it() {
        let mut style = EvaluatedStyle::solid(
            Color::rgba(255, 255, 255, 255),
            Color::TRANSPARENT,
            0.0,
            0.25,
        );
        style.appearances[0].opacity = 0.5;
        let instance = instance(&style, 0, (0.0, 0.0), &ramps_for(&style));
        // Global opacity *and* the layer's own: 0.25 × 0.5.
        assert_eq!(instance.color[3], 0.125);
        assert_eq!(instance.params[1], 0.125);
    }

    #[test]
    fn an_invisible_layer_produces_no_draw_item() {
        // The predicate the renderer, the exporters and the inspector share: a
        // hidden appearance layer is not drawn (Task 10.2 RULE 3).
        let mut style = EvaluatedStyle::default();
        style.appearances[0].visible = false;
        let ramps = ramps_for(&style);
        assert!(InstanceRaw::for_appearance(
            id(1),
            (0.0, 0.0),
            &style,
            &style.appearances[0],
            0,
            &ramps,
            false
        )
        .is_none());
    }

    #[test]
    fn a_gradient_instance_carries_its_window_and_frame() {
        use vectra_core::GradientStop;
        use vectra_geometry::EvaluatedGradient;
        let mut style = EvaluatedStyle::default();
        style.appearances[0].paint = EvaluatedPaint::Linear {
            start: vectra_core::Point2::new(10.0, 20.0),
            end: vectra_core::Point2::new(30.0, 40.0),
            gradient: EvaluatedGradient::new(
                &[
                    GradientStop::new(0.0, Color::BLACK),
                    GradientStop::new(1.0, Color::WHITE),
                ],
                Color::BLACK,
            ),
        };
        style.appearances[0].blend = BlendMode::Multiply;
        let ramps = ramps_for(&style);
        let instance = instance(&style, 0, (0.0, 0.0), &ramps);
        assert_eq!(instance.ramp[0], PAINT_LINEAR);
        assert_eq!(instance.ramp[1], 0.0, "the window starts at stop 0");
        assert_eq!(instance.ramp[2], 2.0, "two stops");
        assert_eq!(instance.frame, [10.0, 20.0, 30.0, 40.0]);
        assert_eq!(instance.params[3], blend_code(BlendMode::Multiply));
        // One stop = two rows of `vec4<f32>`: colour, then alpha.
        assert_eq!(ramps.len(), 4);
        assert_eq!(ramps.rows()[2], [1.0, 1.0, 1.0, 1.0], "white stop");
        assert_eq!(ramps.rows()[3], [1.0, 0.0, 0.0, 0.0], "its alpha row");
    }

    #[test]
    fn a_camera_serializes_to_its_two_rows() {
        let camera = Camera::framing(800.0, 600.0);
        let bytes = camera.to_bytes();
        assert_eq!(bytes.len(), 32);
        let view_x = f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        let view_h = f32::from_le_bytes([bytes[12], bytes[13], bytes[14], bytes[15]]);
        let screen_w = f32::from_le_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]);
        assert_eq!(
            (view_x, view_h),
            (0.0, -600.0),
            "the flip travels in view.w"
        );
        assert_eq!(screen_w, 800.0);
    }

    /// The WGSL projection, transcribed: `document_to_clip` followed by the NDC
    /// → pixel mapping. Any drift between this and `shader.wgsl` is a bug in one
    /// of the two, and this test is where it shows up.
    fn shader_pixel(camera: &Camera, x: f32, y: f32) -> (f32, f32) {
        let uv = (
            (x - camera.view[0]) / camera.view[2],
            (y - camera.view[1]) / camera.view[3],
        );
        let clip = (uv.0 * 2.0 - 1.0, 1.0 - uv.1 * 2.0);
        (
            (clip.0 + 1.0) * 0.5 * camera.screen[0],
            (1.0 - clip.1) * 0.5 * camera.screen[1],
        )
    }

    #[test]
    fn document_y_points_up_on_screen() {
        // The engine's document space is y-up and the canvas must agree, or a
        // shape would drag *away* from the pointer that grabbed it.
        let camera = Camera::framing(800.0, 600.0);
        assert_eq!(
            camera.document_to_screen(0.0, 0.0),
            (0.0, 600.0),
            "origin: bottom-left"
        );
        assert_eq!(
            camera.document_to_screen(0.0, 600.0),
            (0.0, 0.0),
            "y=600: top-left"
        );
        assert_eq!(camera.document_to_screen(800.0, 0.0), (800.0, 600.0));
    }

    #[test]
    fn screen_to_document_is_the_inverse_of_document_to_screen() {
        let camera = Camera::framing(800.0, 600.0);
        for (x, y) in [(0.0, 0.0), (123.5, 456.25), (800.0, 600.0), (-40.0, 700.0)] {
            let (px, py) = camera.document_to_screen(x, y);
            let (rx, ry) = camera.screen_to_document(px, py);
            assert!(
                (rx - x).abs() < 1e-4 && (ry - y).abs() < 1e-4,
                "({x}, {y}) → ({px}, {py}) → ({rx}, {ry})"
            );
        }
    }

    #[test]
    fn the_transcribed_projection_agrees_with_the_camera() {
        let camera = Camera::framing(800.0, 600.0);
        for (x, y) in [(0.0, 0.0), (400.0, 300.0), (800.0, 600.0)] {
            let expected = shader_pixel(&camera, x, y);
            let actual = camera.document_to_screen(x, y);
            assert!(
                (actual.0 - expected.0).abs() < 1e-3 && (actual.1 - expected.1).abs() < 1e-3,
                "({x}, {y}): {actual:?} vs {expected:?}"
            );
        }
    }
}
