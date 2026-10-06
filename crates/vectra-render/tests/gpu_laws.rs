//! **GPU laws**: the shader and the write plan, executed on a real device.
//!
//! Everything else in this crate is verified without a GPU — naga validates the
//! WGSL, a mock sink records the plan, proptests cover tessellation, the spatial
//! index answers pointer queries. What no mock can prove is that the *pipeline
//! as configured* produces the intended pixels: that the vertex stage maps a
//! document rectangle onto the rectangle the camera says, that `fs_fill` emits
//! the fill row, that the stroke pipeline draws the outline and not the fill,
//! that `ALPHA_BLENDING` composites opacity the way the instance's folded alpha
//! assumes, and that an instance write actually *moves* the picture (80 bytes,
//! one row per draw item since Task 10.2).
//!
//! So these tests take a real adapter, render into an offscreen `Rgba8Unorm`
//! texture, copy it back and read pixels. `Rgba8Unorm` (not the `Srgb` variant)
//! is deliberate: no transfer function sits between the shader's output and the
//! bytes we assert on.
//!
//! # Running without a GPU
//!
//! The tests need a Vulkan-capable adapter — in CI that is a software
//! implementation (Mesa's lavapipe). When none is present they print
//! `SKIP: no Vulkan adapter` and succeed: a suite that fails on a machine with no
//! GPU would be a suite people turn off, and a suite that silently *passes*
//! without testing anything would be worse. Read the output, not just the exit
//! code — the same rule the gate script follows.
//!
//! The suite also serialises itself (`vulkan_lock`): `cargo test`'s parallel
//! harness meets drivers that assume one device per process, and lavapipe
//! segfaults under it. Six tests, ~0.3 s, one at a time.

use std::future::Future;
use std::pin::pin;
use std::sync::{Arc, Mutex, MutexGuard};
use std::task::{Context, Poll, Wake, Waker};

use vectra_core::{BlendMode, Color};
use vectra_geometry::{
    DirtySet, EvaluatedNode, EvaluatedPrimitive, EvaluatedScene, EvaluatedStyle,
};
use vectra_render::{Camera, GpuRenderer, RenderScene, CLEAR_COLOR};

const WIDTH: u32 = 256;
const HEIGHT: u32 = 192;
/// `copy_texture_to_buffer` requires a 256-byte-aligned row pitch; 256 px × 4 B
/// is exactly one such row.
const ROW_BYTES: u32 = WIDTH * 4;
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

// ── A device, or a polite skip ────────────────────────────────────────────

/// Poll a future to completion on the current thread.
///
/// `request_device` is `async` on every backend; the tests have no executor, and
/// the waker never needs to do anything because there is exactly one task
/// running on one thread. Twelve lines beats a dependency.
fn block_on<F: Future>(future: F) -> F::Output {
    struct Noop;
    impl Wake for Noop {
        fn wake(self: Arc<Self>) {}
    }
    let waker = Waker::from(Arc::new(Noop));
    let mut cx = Context::from_waker(&waker);
    let mut future = pin!(future);
    loop {
        if let Poll::Ready(value) = future.as_mut().poll(&mut cx) {
            return value;
        }
    }
}

/// One Vulkan device at a time, per process.
///
/// The tests are independent, but drivers are not required to enjoy concurrent
/// device creation from several threads, and software implementations in
/// particular do not: running these in `cargo test`'s default parallel harness
/// produced a `SIGSEGV` inside lavapipe after two tests had already passed. The
/// lock is held for the whole test (including teardown), which is why it is
/// returned to the test rather than dropped here. A poisoned lock means an
/// earlier test panicked; the guard is recovered rather than cascading, because
/// the resource it protects is a driver, not shared state.
fn vulkan_lock() -> MutexGuard<'static, ()> {
    static VULKAN: Mutex<()> = Mutex::new(());
    VULKAN
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// A renderer on a real adapter, or `None` when this machine has none at all.
///
/// The device and queue are *moved into* the renderer — wgpu handles are not
/// `Clone`, and pretending otherwise would mean two loss domains. Everything the
/// tests need afterwards (textures, encoders, `poll`) comes back out of
/// `GpuRenderer::device()` / `queue()`.
fn try_renderer() -> Option<GpuRenderer> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::default());
    let adapter = instance
        .enumerate_adapters(wgpu::Backends::VULKAN)
        .into_iter()
        .next()?;
    let (device, queue) =
        block_on(adapter.request_device(&wgpu::DeviceDescriptor::default(), None)).ok()?;
    let mut renderer = GpuRenderer::new(device, queue, FORMAT).ok()?;
    renderer.set_camera(Camera::fit(800.0, 600.0, [WIDTH as f32, HEIGHT as f32]));
    Some(renderer)
}

/// A renderer on a real device, or a printed skip.
macro_rules! renderer_or_skip {
    ($name:literal) => {{
        // Held until the test returns: the device must be created, used and
        // dropped without another test's device in flight.
        let _vulkan = vulkan_lock();
        match try_renderer() {
            Some(renderer) => (_vulkan, renderer),
            None => {
                println!("SKIP {}: no Vulkan adapter on this machine", $name);
                return;
            }
        }
    }};
}

// ── Scene fixtures ────────────────────────────────────────────────────────

fn style(fill: Color, stroke: Color, width: f64, opacity: f64) -> EvaluatedStyle {
    EvaluatedStyle::solid(fill, stroke, width, opacity)
}

/// Point every fill layer of a style at a new colour. The renderer's laws care
/// about the *picture*, and a restyle has to reach through the stack to change
/// it.
fn recolor(style: &mut EvaluatedStyle, color: Color) {
    for layer in style.appearances.iter_mut() {
        let is_stroke = layer.is_stroke();
        if let vectra_geometry::EvaluatedPaint::Solid(existing) = &mut layer.paint {
            if !is_stroke {
                *existing = color;
            }
        }
    }
}

fn rect_scene(node: EvaluatedNode) -> EvaluatedScene {
    let mut scene = EvaluatedScene::empty();
    let id = node.id;
    scene.nodes.insert(id, node);
    scene.z_order.push(id);
    scene
}

fn rect(
    id: vectra_core::NodeId,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    style: EvaluatedStyle,
) -> EvaluatedNode {
    EvaluatedNode::new(
        id,
        EvaluatedPrimitive::Rect {
            x,
            y,
            w,
            h,
            corner_radius: 0.0,
        },
        style,
    )
}

/// One frame drawn offscreen: the renderer's stats, plus the pixels.
struct Offscreen {
    pixels: Vec<u8>,
}

impl Offscreen {
    fn at(&self, x: u32, y: u32) -> [u8; 4] {
        let offset = (y * ROW_BYTES + x * 4) as usize;
        [
            self.pixels[offset],
            self.pixels[offset + 1],
            self.pixels[offset + 2],
            self.pixels[offset + 3],
        ]
    }
}

/// Draw `scene` into a fresh target through `renderer`, and read the frame back.
fn draw(renderer: &mut GpuRenderer, scene: &RenderScene) -> Offscreen {
    let texture = renderer.device().create_texture(&wgpu::TextureDescriptor {
        label: Some("test.target"),
        size: wgpu::Extent3d {
            width: WIDTH,
            height: HEIGHT,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    renderer.render(&texture, &view, scene, CLEAR_COLOR);

    // The render() call above took `&mut renderer`; the device comes back out
    // for the readback half of the frame.
    let device = renderer.device();
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("test.readback"),
        size: (ROW_BYTES * HEIGHT) as u64,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("test.copy"),
    });
    encoder.copy_texture_to_buffer(
        wgpu::ImageCopyTexture {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::ImageCopyBuffer {
            buffer: &readback,
            layout: wgpu::ImageDataLayout {
                offset: 0,
                bytes_per_row: Some(ROW_BYTES),
                rows_per_image: Some(HEIGHT),
            },
        },
        wgpu::Extent3d {
            width: WIDTH,
            height: HEIGHT,
            depth_or_array_layers: 1,
        },
    );
    renderer.queue().submit(Some(encoder.finish()));

    let pixels = {
        let slice = readback.slice(..);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        device.poll(wgpu::Maintain::Wait);
        // The mapped range borrows the slice; copying it out ends both borrows
        // before the unmap below.
        slice.get_mapped_range().to_vec()
    };
    readback.unmap();
    Offscreen { pixels }
}

/// Sync a fresh scene into a renderer and push it to the GPU — the first frame.
fn first_frame(renderer: &mut GpuRenderer, scene: &mut RenderScene, evaluated: &EvaluatedScene) {
    scene.sync(evaluated, &DirtySet::all());
    renderer.apply(scene).expect("first apply");
}

/// Document units → target pixels, using the camera the renderer draws with.
/// (The same mapping the pointer path uses, so a hit test and a pixel agree.)
fn pixel_of(doc_x: f32, doc_y: f32) -> (u32, u32) {
    let camera = Camera::fit(800.0, 600.0, [WIDTH as f32, HEIGHT as f32]);
    let (px, py) = camera.document_to_screen(doc_x, doc_y);
    (px.round() as u32, py.round() as u32)
}

// ── L1: the fill pipeline lands where the camera says ─────────────────────

#[test]
fn law_the_fill_pipeline_draws_where_the_camera_says() {
    let (_vulkan, mut renderer) = renderer_or_skip!("fill pipeline");
    let id = vectra_core::new_node_id();
    let red = Color::rgb(255, 0, 0);
    let scene_source = rect_scene(rect(
        id,
        100.0,
        100.0,
        200.0,
        100.0,
        style(red, Color::TRANSPARENT, 0.0, 1.0),
    ));

    let mut scene = RenderScene::new();
    first_frame(&mut renderer, &mut scene, &scene_source);
    let frame = draw(&mut renderer, &scene);

    let (cx, cy) = pixel_of(200.0, 150.0);
    let centre = frame.at(cx, cy);
    assert_eq!(
        centre,
        [255, 0, 0, 255],
        "the rect's centre is solid red at ({cx}, {cy}); got {centre:?}"
    );
    // Corners just inside the rect are still red (the projection is a scale +
    // flip, not an offset that could be clipped).
    for (doc_x, doc_y) in [(102.0, 102.0), (298.0, 198.0)] {
        let (px, py) = pixel_of(doc_x, doc_y);
        assert_eq!(
            frame.at(px, py),
            [255, 0, 0, 255],
            "inside at doc ({doc_x}, {doc_y})"
        );
    }
    // Just outside is the clear colour: the geometry did not stretch.
    for (doc_x, doc_y) in [(95.0, 150.0), (200.0, 95.0), (200.0, 205.0), (10.0, 590.0)] {
        let (px, py) = pixel_of(doc_x, doc_y);
        assert_eq!(
            frame.at(px, py),
            [CLEAR_COLOR.r, CLEAR_COLOR.g, CLEAR_COLOR.b, 255],
            "outside at doc ({doc_x}, {doc_y})"
        );
    }
}

// ── L2: stroke and opacity ────────────────────────────────────────────────

#[test]
fn law_the_stroke_pipeline_draws_the_outline_at_the_right_width() {
    let (_vulkan, mut renderer) = renderer_or_skip!("stroke pipeline");
    let id = vectra_core::new_node_id();
    let fill = Color::rgb(0, 200, 0);
    let stroke = Color::rgb(0, 0, 255);
    // 10 units of stroke on a rect spanning doc x 100..300: lyon expands the
    // outline *centred* on the path, so the ring covers doc x 95..105.
    let scene_source = rect_scene(rect(
        id,
        100.0,
        100.0,
        200.0,
        100.0,
        style(fill, stroke, 10.0, 1.0),
    ));

    let mut scene = RenderScene::new();
    first_frame(&mut renderer, &mut scene, &scene_source);
    let frame = draw(&mut renderer, &scene);

    // Middle of the left edge: stroke.
    let (px, py) = pixel_of(100.0, 150.0);
    assert_eq!(
        frame.at(px, py),
        [0, 0, 255, 255],
        "the outline is blue at ({px}, {py})"
    );
    // Dead centre: fill, untouched by the outline pass.
    let (fx, fy) = pixel_of(200.0, 150.0);
    assert_eq!(frame.at(fx, fy), [0, 200, 0, 255], "the interior is green");
    // 8 units outside the path (doc x 92) is beyond a 5-unit half-width: clear.
    let (ox, oy) = pixel_of(92.0, 150.0);
    assert_eq!(
        frame.at(ox, oy),
        [CLEAR_COLOR.r, CLEAR_COLOR.g, CLEAR_COLOR.b, 255],
        "the outline is not wider than asked"
    );
}

#[test]
fn law_opacity_composites_the_way_the_instance_assumes() {
    let (_vulkan, mut renderer) = renderer_or_skip!("opacity blending");
    let id = vectra_core::new_node_id();
    // Opaque white at 50% opacity over the clear colour: straight-alpha
    // blending gives (fill + clear) / 2 exactly — which only holds because the
    // Rust side folds opacity into the colour's alpha and the pipeline blends
    // with `SrcAlpha`.
    let scene_source = rect_scene(rect(
        id,
        0.0,
        0.0,
        800.0,
        600.0,
        style(Color::rgb(255, 255, 255), Color::TRANSPARENT, 0.0, 0.5),
    ));

    let mut scene = RenderScene::new();
    first_frame(&mut renderer, &mut scene, &scene_source);
    let frame = draw(&mut renderer, &scene);

    let (px, py) = pixel_of(400.0, 300.0);
    let got = frame.at(px, py);
    let expected = [
        ((255 + CLEAR_COLOR.r as u32) / 2) as u8,
        ((255 + CLEAR_COLOR.g as u32) / 2) as u8,
        ((255 + CLEAR_COLOR.b as u32) / 2) as u8,
    ];
    for channel in 0..3 {
        let delta = (got[channel] as i32 - expected[channel] as i32).abs();
        assert!(
            delta <= 1,
            "channel {channel}: got {got:?}, expected {expected:?} (±1 rounding)"
        );
    }
    // The alpha channel composites with `OVER` (`src * 1 + dst * (1 - src.a)`),
    // not with `SrcAlpha` like the colour channels: a translucent fill over an
    // opaque background is still an opaque *target*, which is what a canvas
    // presents. Asserted rather than assumed — the first version of this test
    // expected 191 and the device said 255.
    assert_eq!(
        got[3], 255,
        "an opaque clear stays opaque under a translucent fill"
    );
}

// ── L3: RULE 2, proven by pixels ──────────────────────────────────────────

#[test]
fn law_a_move_rewrites_one_instance_row_and_moves_the_picture() {
    let (_vulkan, mut renderer) = renderer_or_skip!("incremental move");
    let id = vectra_core::new_node_id();
    let red = Color::rgb(255, 0, 0);
    let mut evaluated = rect_scene(rect(
        id,
        100.0,
        400.0,
        100.0,
        100.0,
        style(red, Color::TRANSPARENT, 0.0, 1.0),
    ));

    let mut scene = RenderScene::new();
    first_frame(&mut renderer, &mut scene, &evaluated);
    let before = draw(&mut renderer, &scene);
    let (ox, oy) = pixel_of(150.0, 450.0);
    assert_eq!(
        before.at(ox, oy),
        [255, 0, 0, 255],
        "the rect is where it started"
    );

    // Move it: same shape, new origin. The plan says one instance write…
    if let EvaluatedPrimitive::Rect { x, y, .. } =
        &mut evaluated.nodes.get_mut(&id).unwrap().primitive
    {
        *x = 500.0;
        *y = 400.0;
    }
    scene.sync(&evaluated, &DirtySet::single(id));
    let plan = scene.plan().clone();
    assert_eq!(plan.ops.len(), 1, "one write: {plan:?}");
    assert_eq!(plan.bytes(), 80, "one instance row");
    let stats = renderer.apply(&scene).expect("incremental apply");
    assert_eq!(stats.writes, 1, "the GPU saw one write_buffer call");
    assert_eq!(stats.bytes_written, 80, "and 80 bytes of traffic");

    // …and the pixels moved with it, with no re-tessellation anywhere.
    let after = draw(&mut renderer, &scene);
    let (nx, ny) = pixel_of(550.0, 450.0);
    assert_eq!(
        after.at(nx, ny),
        [255, 0, 0, 255],
        "the rect is where it was moved to"
    );
    assert_eq!(
        after.at(ox, oy),
        [CLEAR_COLOR.r, CLEAR_COLOR.g, CLEAR_COLOR.b, 255],
        "and its old home is empty"
    );
    assert_eq!(scene.report().retessellated, 0);
}

#[test]
fn law_a_restyle_rewrites_one_instance_row_and_recolours_the_picture() {
    let (_vulkan, mut renderer) = renderer_or_skip!("incremental restyle");
    let id = vectra_core::new_node_id();
    let mut evaluated = rect_scene(rect(
        id,
        0.0,
        0.0,
        800.0,
        600.0,
        style(Color::rgb(255, 0, 0), Color::TRANSPARENT, 0.0, 1.0),
    ));

    let mut scene = RenderScene::new();
    first_frame(&mut renderer, &mut scene, &evaluated);
    let (px, py) = pixel_of(400.0, 300.0);
    assert_eq!(draw(&mut renderer, &scene).at(px, py), [255, 0, 0, 255]);

    recolor(
        &mut evaluated.nodes.get_mut(&id).unwrap().style,
        Color::rgb(0, 0, 255),
    );
    scene.sync(&evaluated, &DirtySet::single(id));
    let stats = renderer.apply(&scene).expect("restyle apply");
    assert_eq!(
        (stats.writes, stats.bytes_written),
        (1, 80),
        "colour is one instance row"
    );
    assert_eq!(
        draw(&mut renderer, &scene).at(px, py),
        [0, 0, 255, 255],
        "the fill changed colour on the GPU"
    );
}

// ── L3b: RULE 3, proven by pixels — the blend modes ───────────────────────

/// Point every fill layer of a style at a colour **and** a blend mode (Task 10.2
/// RULE 3). The renderer's laws care about the picture; a blend has to reach
/// through the resolved stack to change it, exactly like a colour.
fn recolor_and_blend(style: &mut EvaluatedStyle, color: Color, blend: BlendMode) {
    recolor(style, color);
    for layer in style.appearances.iter_mut() {
        if !layer.is_stroke() {
            layer.blend = blend;
        }
    }
}

/// **The three modes that read the backdrop composite with it — on the device.**
///
/// `Multiply`, `Screen` and `Overlay` are functions of the destination colour, so
/// no `BlendState` can express them: the shader reads the canvas underneath from
/// a snapshot and composites itself (`composite()` in `shader.wgsl`, straight
/// alpha, the W3C formula). This law is the only place that proves the whole
/// path end to end — instance row → pipeline choice (`REPLACE`, the backdrop
/// pair) → snapshot → fragment maths → pixels:
///
/// ```text
///   Cb = (200, 100, 50)   a red-brown backdrop, drawn Normal
///   Cs = (64, 64, 64)     a dark layer, one per mode
/// ```
///
/// `Cs` is deliberately *not* mid-grey: Overlay is the identity at `0.5`
/// (`2·Cb·0.5 = Cb` on the dark half, `1 − 2(1−Cb)(1−0.5) = Cb` on the light
/// half), so a 50% source would make the overlay column indistinguishable from
/// the backdrop and the law would pass without proving anything. The first run
/// of this test did exactly that, and the honest fix is a source that separates
/// all three modes.
///
/// One frame, one backdrop, three blended layers side by side; each sample point
/// is the arithmetic of the mode it sits under, and *not* the source and *not*
/// the backdrop — which is what makes this a blend law rather than a colour law.
#[test]
fn law_the_blend_modes_composite_against_the_canvas_beneath_them() {
    let (_vulkan, mut renderer) = renderer_or_skip!("blend modes");
    let backdrop = vectra_core::new_node_id();
    let backdrop_color = Color::rgb(200, 100, 50);
    let source = Color::rgb(64, 64, 64);

    let mut evaluated = EvaluatedScene::empty();
    evaluated.nodes.insert(
        backdrop,
        rect(
            backdrop,
            0.0,
            0.0,
            800.0,
            600.0,
            style(backdrop_color, Color::TRANSPARENT, 0.0, 1.0),
        ),
    );
    evaluated.z_order.push(backdrop);

    // Three layers, three modes, three columns — same geometry, same colour.
    let modes = [
        (BlendMode::Multiply, 100.0),
        (BlendMode::Screen, 300.0),
        (BlendMode::Overlay, 500.0),
    ];
    for (mode, x) in modes {
        let id = vectra_core::new_node_id();
        let mut layer = style(source, Color::TRANSPARENT, 0.0, 1.0);
        recolor_and_blend(&mut layer, source, mode);
        evaluated
            .nodes
            .insert(id, rect(id, x, 100.0, 200.0, 400.0, layer));
        evaluated.z_order.push(id);
    }

    let mut scene = RenderScene::new();
    first_frame(&mut renderer, &mut scene, &evaluated);
    let frame = draw(&mut renderer, &scene);

    // The renderer knows what it did: three backdrop reads, each of which costs
    // a snapshot copy and a pass.
    let stats = renderer.stats();
    assert_eq!(
        stats.blend_draws, 3,
        "one blended draw per non-Normal layer"
    );
    assert_eq!(stats.draw_calls, 4, "the backdrop plus the three layers");
    assert!(
        stats.passes >= 4,
        "a Normal pass plus one pass per blended draw: {stats:?}"
    );

    // The backdrop, where nothing blends over it.
    let (px, py) = pixel_of(700.0, 300.0);
    assert_eq!(
        frame.at(px, py),
        [200, 100, 50, 255],
        "the backdrop is drawn as authored, Normal"
    );

    // Each mode's own arithmetic, in 0..1 with `Cb`/`Cs` as the shader sees them.
    let cb = [200.0_f32 / 255.0, 100.0 / 255.0, 50.0 / 255.0];
    let cs = 64.0_f32 / 255.0;
    let multiply = [cb[0] * cs, cb[1] * cs, cb[2] * cs];
    let screen = [
        cb[0] + cs - cb[0] * cs,
        cb[1] + cs - cb[1] * cs,
        cb[2] + cs - cb[2] * cs,
    ];
    let overlay: [f32; 3] = std::array::from_fn(|channel| {
        let channel = cb[channel];
        if channel <= 0.5 {
            2.0 * channel * cs
        } else {
            1.0 - 2.0 * (1.0 - channel) * (1.0 - cs)
        }
    });

    for (mode, x, expected) in [
        (BlendMode::Multiply, 200.0, multiply),
        (BlendMode::Screen, 400.0, screen),
        (BlendMode::Overlay, 600.0, overlay),
    ] {
        // Sample the *centre* of the layer's own rect, not of the column.
        let (px, py) = pixel_of(x, 300.0);
        let got = frame.at(px, py);
        for channel in 0..3 {
            let want = (expected[channel] * 255.0).round() as i32;
            let delta = (got[channel] as i32 - want).abs();
            assert!(
                delta <= 2,
                "{mode:?} channel {channel}: got {got:?}, expected {want} (±2) \
                 — the mode is composited by the shader, not by blend state"
            );
        }
        // …and the answer is neither operands' colour, which is what makes this
        // a law about *compositing* rather than about a buffer copy.
        assert_ne!(got[0..3], [64, 64, 64], "{mode:?} did not simply replace");
        assert_ne!(got[0..3], [200, 100, 50], "{mode:?} ignored the layer");
    }
}

// ── L4: the whole board ───────────────────────────────────────────────────

#[test]
fn law_a_multi_node_scene_draws_in_z_order() {
    let (_vulkan, mut renderer) = renderer_or_skip!("z order");
    // Two overlapping opaque rects: the later one must win where they overlap,
    // which is what "painter's order" means on a real device.
    let back = vectra_core::new_node_id();
    let front = vectra_core::new_node_id();
    let mut evaluated = EvaluatedScene::empty();
    for (id, x, color) in [
        (back, 100.0, Color::rgb(255, 0, 0)),
        (front, 200.0, Color::rgb(0, 0, 255)),
    ] {
        evaluated.nodes.insert(
            id,
            rect(
                id,
                x,
                100.0,
                200.0,
                200.0,
                style(color, Color::TRANSPARENT, 0.0, 1.0),
            ),
        );
        evaluated.z_order.push(id);
    }

    let mut scene = RenderScene::new();
    first_frame(&mut renderer, &mut scene, &evaluated);
    let frame = draw(&mut renderer, &scene);

    let (bx, by) = pixel_of(150.0, 200.0);
    assert_eq!(frame.at(bx, by), [255, 0, 0, 255], "the back rect alone");
    let (fx, fy) = pixel_of(350.0, 200.0);
    assert_eq!(frame.at(fx, fy), [0, 0, 255, 255], "the front rect alone");
    let (ox, oy) = pixel_of(250.0, 200.0);
    assert_eq!(
        frame.at(ox, oy),
        [0, 0, 255, 255],
        "the overlap belongs to the last drawn"
    );

    // Reordering *without* touching a buffer flips the overlap: draw order is
    // not a data upload (RULE 2 covers the order too).
    evaluated.z_order = vec![front, back];
    scene.sync(&evaluated, &DirtySet::nodes(Vec::new()));
    assert!(scene.plan().reordered, "the order changed");
    assert!(scene.plan().ops.is_empty(), "with no buffer traffic at all");
    renderer.apply(&scene).expect("reorder apply");
    let frame = draw(&mut renderer, &scene);
    assert_eq!(
        frame.at(ox, oy),
        [255, 0, 0, 255],
        "the overlap now belongs to the other node"
    );
}
