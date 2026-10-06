// VECTRA — geometry + paint shader (Task 5.0, extended by Task 10.2 RULE 3).
//
// Four pipelines, two fragment-stage pairs, one vertex stage:
//
//   * the VERTEX stage is shared: it reads the draw item's instance, applies the
//     node's resolved transform, projects document space → clip space, and hands
//     the fragment stage the *document* position (which is where a gradient is
//     defined) and the item's paint rows.
//   * `fs_fill` / `fs_stroke` are the straight-alpha pair: they emit the item's
//     colour (solid, or sampled from its gradient ramp) and the fixed-function
//     blend state composites it.
//   * `fs_fill_blend` / `fs_stroke_blend` are the backdrop pair: they read the
//     snapshot of the canvas taken just before this draw and composite
//     `Multiply`, `Screen` or `Overlay` **themselves**, because those three modes
//     are functions of the destination colour and no blend state can express
//     them. Their pipeline uses `REPLACE`.
//
// Everything per draw item lives in the instance array, so the vertex buffers
// hold nothing but `vec2<f32>` positions — that is what keeps a drag to one
// 80-byte write, and what makes "two strokes, black under white" two draws of two
// outlines with two rows rather than a special case.

// ── Uniforms ────────────────────────────────────────────────────────────

/// Where the document window is, and how big the target is.
///
/// `view`   = (min_x, min_y, width, height) in document units.
/// `screen` = (width_px, height_px, _, _) — reserved for pixel-space work.
struct Camera {
    view: vec4<f32>,
    screen: vec4<f32>,
}

/// Per draw item (one fill layer or one stroke layer of one node), one element
/// per instance row.
///
/// `transform` = (scale_x, scale_y, translate_x, translate_y): the node's local
///               frame → document space.
/// `color`     = straight (non-premultiplied) RGBA, with the layer's opacity and
///               the node's global opacity already folded into `a`.
/// `params`    = (stroke_width, opacity, selected, blend).
/// `ramp`      = (kind, stop_offset, stop_count, is_stroke): `kind` 0 = solid,
///               1 = linear, 2 = radial; the window counts *stops*, each of which
///               occupies two rows of the ramp array.
/// `frame`     = the gradient frame in document space: linear is
///               (start.x, start.y, end.x, end.y), radial is (center.x, center.y,
///               radius, 0).
struct Instance {
    transform: vec4<f32>,
    color: vec4<f32>,
    params: vec4<f32>,
    ramp: vec4<f32>,
    frame: vec4<f32>,
}

@group(0) @binding(0) var<uniform> camera: Camera;

/// Read-only storage: WebGPU allows read-only storage buffers in the vertex
/// stage, so the instance array needs no fixed maximum size (a uniform array
/// would have to declare `array<Instance, N>` and cap the scene at N nodes).
@group(0) @binding(1) var<storage, read> instances: array<Instance>;

/// Every gradient stop in the scene, laid end to end as two rows per stop:
/// `(offset, r, g, b)` then `(a, 0, 0, 0)`. A stop has five meaningful numbers
/// and a `vec4` holds four; packing the alpha in a second row keeps the stride
/// 16 bytes, which is the only stride a storage buffer may use.
@group(0) @binding(2) var<storage, read> ramps: array<vec4<f32>>;

/// The canvas as it stood before the current blended draw. Only the `_blend`
/// entry points sample it; the `Normal` pipelines bind a 1×1 dummy.
@group(1) @binding(0) var backdrop: texture_2d<f32>;

// ── Vertex stage ────────────────────────────────────────────────────────

struct VertexInput {
    @location(0) position: vec2<f32>,
}

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    /// The item's colour, or — for a gradient — the ramp's first stop as a
    /// fallback (used when the window is empty, so a broken gradient paints its
    /// colour rather than nothing).
    @location(0) color: vec4<f32>,
    /// `(kind, stop_offset, stop_count, is_stroke)`, passed through unchanged.
    @location(1) ramp: vec4<f32>,
    /// The gradient frame in document space.
    @location(2) frame: vec4<f32>,
    /// The fragment's position **in document space**: gradients are defined
    /// against document coordinates, exactly like every other channel of the
    /// engine, so the ramp needs no per-node bookkeeping here.
    @location(3) document_position: vec2<f32>,
    /// The blend mode code, so the fragment stage knows which composite to run.
    @location(4) @interpolate(flat) blend: f32,
}

/// Document space → clip space. The view rectangle maps to the whole target.
///
/// The y flip lives in the *sign of `camera.view.w`* (see `Camera::framing`),
/// not in this function: document space is y-up, a texture's rows run downward,
/// and one sign carries that difference for the projection, the pointer mapping
/// and the hit test alike.
fn document_to_clip(point: vec2<f32>) -> vec4<f32> {
    let uv = (point - camera.view.xy) / camera.view.zw;
    return vec4<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, 0.0, 1.0);
}

@vertex
fn vs_main(
    input: VertexInput,
    @builtin(instance_index) instance_index: u32,
) -> VertexOutput {
    let instance = instances[instance_index];
    // Local frame → document space: scale, then translate.
    let world = input.position * instance.transform.xy + instance.transform.zw;

    var out: VertexOutput;
    out.clip_position = document_to_clip(world);
    out.color = instance.color;
    out.ramp = instance.ramp;
    out.frame = instance.frame;
    out.document_position = world;
    out.blend = instance.params.w;
    return out;
}

// ── Paint ───────────────────────────────────────────────────────────────

/// The gradient parameter at a document-space point — the WGSL transliteration of
/// `vectra_geometry::EvaluatedPaint::gradient_t`. The two must agree: the CPU
/// sampler is what the SVG exporter, the inspector and the pixel laws compare
/// against, so the shader follows it and not the other way round.
fn gradient_t(ramp: vec4<f32>, frame: vec4<f32>, position: vec2<f32>) -> f32 {
    let kind = ramp.x;
    if kind < 1.5 {
        // Linear: project onto the axis.
        let axis = frame.zw - frame.xy;
        let length_sq = dot(axis, axis);
        if length_sq <= 1e-9 {
            return 1.0;
        }
        return dot(position - frame.xy, axis) / length_sq;
    }
    // Radial: normalized distance from the centre.
    let radius = frame.z;
    if radius <= 1e-9 {
        return 1.0;
    }
    return length(position - frame.xy) / radius;
}

/// Sample a gradient's ramp at `t`, clamped to the end stops.
///
/// Straight, non-premultiplied interpolation, matching `EvaluatedGradient::sample`
/// stop for stop — including the "two stops at the same offset are a hard edge"
/// rule, which is what a duplicated stop means in every editor.
fn sample_ramp(ramp: vec4<f32>, t: f32) -> vec4<f32> {
    let offset = u32(ramp.y);
    let count = u32(ramp.z);
    if count == 0u {
        return vec4<f32>(0.0, 0.0, 0.0, 0.0);
    }
    var clamped = clamp(t, 0.0, 1.0);
    var previous = stop_at(offset);
    if clamped <= previous.x {
        return previous;
    }
    var index = 1u;
    loop {
        if index >= count {
            break;
        }
        let stop = stop_at(offset + index);
        if clamped <= stop.x {
            let span = stop.x - previous.x;
            if span <= 1e-9 {
                return stop;
            }
            let local = (clamped - previous.x) / span;
            return vec4<f32>(
                mix(previous.yzw, stop.yzw, local),
                mix(previous.w, stop.w, local),
            );
        }
        previous = stop;
        index = index + 1u;
    }
    return previous;
}

/// One stop, reassembled from its two rows.
fn stop_at(index: u32) -> vec4<f32> {
    let head = ramps[index * 2u];
    let tail = ramps[index * 2u + 1u];
    return vec4<f32>(head.x, head.y, head.z, tail.x);
}

/// The fragment's paint colour: the item's colour for a solid, the sampled ramp
/// for a gradient.
fn paint_color(input: VertexOutput) -> vec4<f32> {
    if input.ramp.x < 0.5 {
        return input.color;
    }
    if u32(input.ramp.z) == 0u {
        return input.color;
    }
    let t = gradient_t(input.ramp, input.frame, input.document_position);
    let sampled = sample_ramp(input.ramp, t);
    // The layer's opacity and the node's global opacity are already folded into
    // `input.color.a`; a gradient's own stop alpha multiplies on top of them.
    return vec4<f32>(sampled.rgb, sampled.a * input.color.a);
}

// ── Backdrop compositing (blend modes) ──────────────────────────────────

/// The separable blend functions of the W3C compositing spec, for the three
/// modes that read the destination. `mode` is the instance's blend code.
fn blend_channel(mode: f32, cb: f32, cs: f32) -> f32 {
    if mode < 1.5 {
        return cb * cs; // Multiply
    }
    if mode < 2.5 {
        return cb + cs - cb * cs; // Screen
    }
    // Overlay: hard-light with the operands swapped — multiply the dark half of
    // the backdrop, screen its light half.
    if cb <= 0.5 {
        return 2.0 * cb * cs;
    }
    return 1.0 - 2.0 * (1.0 - cb) * (1.0 - cs);
}

/// Composite `source` over `destination` with a blend function, straight alpha in
/// and out:
///
/// ```text
/// αo = αs + αb·(1 − αs)
/// co = [(1 − αb)·αs·Cs + (1 − αs)·αb·Cb + αs·αb·B(Cb, Cs)] / αo
/// ```
///
/// The last term is the blend; the first two are what makes it correct where the
/// *other* layer is transparent, which is where a naive `src·dst` goes wrong.
fn composite(mode: f32, dst: vec4<f32>, src: vec4<f32>) -> vec4<f32> {
    let as_ = src.a;
    let ab = dst.a;
    let ao = as_ + ab * (1.0 - as_);
    if ao <= 0.0 {
        return vec4<f32>(0.0, 0.0, 0.0, 0.0);
    }
    var channel = vec3<f32>(0.0, 0.0, 0.0);
    let blended = vec3<f32>(
        blend_channel(mode, dst.r, src.r),
        blend_channel(mode, dst.g, src.g),
        blend_channel(mode, dst.b, src.b),
    );
    channel =
        (src.rgb * as_ * (1.0 - ab) + dst.rgb * ab * (1.0 - as_) + blended * as_ * ab) / ao;
    return vec4<f32>(channel, ao);
}

/// The canvas underneath this fragment.
fn backdrop_at(clip_position: vec4<f32>) -> vec4<f32> {
    return textureLoad(backdrop, vec2<i32>(floor(clip_position.xy)), 0);
}

// ── Fragment stages ─────────────────────────────────────────────────────

@fragment
fn fs_fill(input: VertexOutput) -> @location(0) vec4<f32> {
    return paint_color(input);
}

@fragment
fn fs_stroke(input: VertexOutput) -> @location(0) vec4<f32> {
    return paint_color(input);
}

@fragment
fn fs_fill_blend(input: VertexOutput) -> @location(0) vec4<f32> {
    return composite(input.blend, backdrop_at(input.clip_position), paint_color(input));
}

@fragment
fn fs_stroke_blend(input: VertexOutput) -> @location(0) vec4<f32> {
    return composite(input.blend, backdrop_at(input.clip_position), paint_color(input));
}
