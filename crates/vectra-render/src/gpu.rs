//! The wgpu layer (Task 5.0 §1/§2): two pipelines, three kinds of buffer, and
//! the *surgical* application of a [`crate::scene::WritePlan`].
//!
//! `GpuRenderer` deliberately does **not** own a surface, a window or a canvas.
//! It takes a `Device` + `Queue` + target format, which means:
//!
//! * the same code type-checks on the host (where the workspace gate runs) and
//!   on `wasm32-unknown-unknown` (where the browser supplies the device);
//! * the web-specific plumbing — canvas, surface, adapter, resize — lives in
//!   `vectra-wasm`, the module that is allowed to know about DOM types.
//!
//! # Buffers
//!
//! | buffer | shape | written when |
//! |---|---|---|
//! | `instances` | one shared storage array, 80 bytes per draw item | a node moves or restyles — **one slice** |
//! | per-node vertex/index | one pair per node per mesh (fill, stroke) | that node's geometry changes |
//! | `camera` | one uniform | the viewport or the view changes |
//!
//! A style change therefore touches exactly the 80 bytes of that item's instance
//! row; a drag touches one instance row per moved node and no vertex data at
//! all. [`BufferSink::write`] is the only method in the renderer that calls
//! `queue.write_buffer`, and it only ever writes the buffer the plan named.

use std::collections::BTreeMap;

use vectra_core::NodeId;

use crate::error::RenderError;
use crate::geometry::{Mesh, MeshKind};
use crate::instance::{Camera, InstanceRaw};
use crate::scene::{BufferSink, DrawMesh, RenderScene, WriteKind};
use crate::tessellate::vertex_layout;

/// The shader, embedded so the pipeline and the file can never disagree.
pub const SHADER_SOURCE: &str = include_str!("shader.wgsl");

/// Vertex stage entry point (shared by every pipeline).
pub const VERTEX_ENTRY: &str = "vs_main";
/// Fill fragment stage.
pub const FILL_ENTRY: &str = "fs_fill";
/// Stroke fragment stage.
pub const STROKE_ENTRY: &str = "fs_stroke";
/// Fill fragment stage for a layer that blends with the backdrop.
pub const FILL_BLEND_ENTRY: &str = "fs_fill_blend";
/// Stroke fragment stage for a layer that blends with the backdrop.
pub const STROKE_BLEND_ENTRY: &str = "fs_stroke_blend";

/// Slack given to a buffer when it grows: rebuilding is cheap, growing every
/// frame of a resize gesture is not.
const BUFFER_GROWTH_SLACK: f64 = 1.5;

/// One stroke layer's GPU buffers.
struct GpuMesh {
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    vertex_bytes: u64,
    index_bytes: u64,
    index_count: u32,
}

impl GpuMesh {
    fn is_empty(&self) -> bool {
        self.index_count == 0
    }
}

/// The GPU mirror of one node: one fill mesh plus one mesh per stroke layer
/// (Task 10.2 RULE 3).
struct GpuNode {
    fill: GpuMesh,
    /// One entry per stroke layer, in stack order. `None` until that layer has
    /// something to draw: a stack that gains a stroke allocates exactly one mesh,
    /// and a stack that loses one leaves the slot empty rather than shifting every
    /// later stroke's buffers.
    strokes: Vec<Option<GpuMesh>>,
}

/// Numbers the UI shows about the GPU side (and the tests can assert on).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GpuStats {
    pub nodes: usize,
    pub buffers: usize,
    pub buffer_bytes: u64,
    pub instance_bytes: u64,
    pub ramp_bytes: u64,
    pub reallocations: usize,
    /// Writes performed by the last `apply` — one per planned buffer write.
    pub writes: usize,
    pub bytes_written: u64,
    /// `draw_indexed` calls: one per non-empty draw item.
    pub draw_calls: usize,
    /// Draw items that read the backdrop, each of which costs a render-pass
    /// split plus a texture copy (see `render`).
    pub blend_draws: usize,
    /// Render passes the last frame used. One, unless a blend mode split it.
    pub passes: usize,
}

/// The renderer's device-side state.
pub struct GpuRenderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    fill_pipeline: wgpu::RenderPipeline,
    stroke_pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    camera_buffer: wgpu::Buffer,
    camera_bind_group: wgpu::BindGroup,
    instance_buffer: wgpu::Buffer,
    instance_capacity: u32,
    /// Every gradient stop in the scene, as `vec4<f32>` rows.
    ramp_buffer: wgpu::Buffer,
    ramp_capacity: u64,
    /// Fill/stroke pipelines that composite with straight alpha (blend mode
    /// `Normal`) and the pair that reads the backdrop (the other three modes).
    /// Both pairs are built once, in `new`: a blend mode is a *number* in the
    /// instance row, never a pipeline built per frame.
    fill_blend_pipeline: wgpu::RenderPipeline,
    stroke_blend_pipeline: wgpu::RenderPipeline,
    /// The canvas as it stood before the current blend draw — the only way to
    /// implement `Multiply`, `Screen` and `Overlay`, whose definitions are
    /// functions of the destination colour.
    backdrop_texture: Option<wgpu::Texture>,
    backdrop_view: wgpu::TextureView,
    backdrop_size: (u32, u32),
    backdrop_bind_group_layout: wgpu::BindGroupLayout,
    backdrop_bind_group: wgpu::BindGroup,
    nodes: BTreeMap<NodeId, GpuNode>,
    order: Vec<NodeId>,
    stats: GpuStats,
    writes_this_apply: usize,
    bytes_this_apply: u64,
    camera: Camera,
    target_format: wgpu::TextureFormat,
}

impl GpuRenderer {
    /// Build the pipelines and the shared buffers for a target format.
    ///
    /// `format` comes from the surface's capabilities, so the renderer never
    /// assumes anything about the swap chain it will draw into.
    pub fn new(
        device: wgpu::Device,
        queue: wgpu::Queue,
        format: wgpu::TextureFormat,
    ) -> Result<Self, RenderError> {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("vectra.shader"),
            source: wgpu::ShaderSource::Wgsl(SHADER_SOURCE.into()),
        });

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("vectra.bind_group_layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                // The gradient ramp table (Task 10.2 RULE 3). The fragment stage
                // walks a window of it, which is what makes a gradient data
                // instead of a second shader.
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });

        // Group 1 is the backdrop: the canvas as it stood before the current
        // blend draw. It is bound to *every* pipeline (a dummy 1×1 texture for the
        // `Normal` path, which never samples it) so there is one pipeline layout
        // and one bind group switch.
        let backdrop_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("vectra.backdrop_layout"),
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                }],
            });

        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("vectra.pipeline_layout"),
            bind_group_layouts: &[&bind_group_layout, &backdrop_bind_group_layout],
            push_constant_ranges: &[],
        });

        let make_pipeline = |label: &str, fragment_entry: &str, blend: wgpu::BlendState| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: VERTEX_ENTRY,
                    compilation_options: Default::default(),
                    buffers: &[vertex_layout()],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: fragment_entry,
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        // Straight alpha, painter's order: the fragment stages
                        // emit the colour with opacity already in `a`.
                        //
                        // The **blend pipelines** pass `REPLACE` instead, because
                        // their fragment stage has already composited against the
                        // backdrop sample — see the `_blend` entry points. A fixed
                        // function cannot express `Multiply`/`Screen`/`Overlay`
                        // (they are functions of the destination), which is why
                        // those three need the snapshot rather than a blend state.
                        blend: Some(blend),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    // Both windings reach the raster: lyon winds fills CCW, but
                    // a boolean result (Task 4.0) can legitimately contain
                    // holes, so culling would punch them open.
                    cull_mode: None,
                    ..Default::default()
                },
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview: None,
                // No shader-cache: Phase 1 pipelines are built once per session.
                cache: None,
            })
        };

        let fill_pipeline =
            make_pipeline("vectra.fill", FILL_ENTRY, wgpu::BlendState::ALPHA_BLENDING);
        let stroke_pipeline = make_pipeline(
            "vectra.stroke",
            STROKE_ENTRY,
            wgpu::BlendState::ALPHA_BLENDING,
        );
        let fill_blend_pipeline = make_pipeline(
            "vectra.fill.blend",
            FILL_BLEND_ENTRY,
            wgpu::BlendState::REPLACE,
        );
        let stroke_blend_pipeline = make_pipeline(
            "vectra.stroke.blend",
            STROKE_BLEND_ENTRY,
            wgpu::BlendState::REPLACE,
        );

        // The dummy backdrop: 1×1, transparent, never sampled by the `Normal`
        // pipelines. It exists so a bind group can be built before any blended
        // layer appears.
        let dummy = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("vectra.backdrop.dummy"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let backdrop_view = dummy.create_view(&wgpu::TextureViewDescriptor::default());
        let backdrop_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("vectra.backdrop"),
            layout: &backdrop_bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&backdrop_view),
            }],
        });

        let camera = Camera::framing(800.0, 600.0);
        let camera_bytes = camera.to_bytes();
        let camera_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("vectra.camera"),
            size: camera_bytes.len() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&camera_buffer, 0, &camera_bytes);

        let instance_capacity = 16u32;
        let instance_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("vectra.instances"),
            size: instance_capacity as u64 * InstanceRaw::STRIDE,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // The ramp table's first allocation: room for 64 stops (2 rows each).
        // It grows like every other buffer, with slack, and only when a gradient
        // actually gets longer.
        let ramp_capacity = 64 * 2 * 16u64;
        let ramp_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("vectra.ramps"),
            size: ramp_capacity,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let camera_bind_group = bind_group(
            &device,
            &bind_group_layout,
            &camera_buffer,
            &instance_buffer,
            &ramp_buffer,
        );

        Ok(Self {
            device,
            queue,
            fill_pipeline,
            stroke_pipeline,
            fill_blend_pipeline,
            stroke_blend_pipeline,
            bind_group_layout,
            camera_buffer,
            camera_bind_group,
            instance_buffer,
            instance_capacity,
            ramp_buffer,
            ramp_capacity,
            backdrop_texture: None,
            backdrop_view,
            backdrop_size: (1, 1),
            backdrop_bind_group_layout,
            backdrop_bind_group,
            nodes: BTreeMap::new(),
            order: Vec::new(),
            stats: GpuStats::default(),
            writes_this_apply: 0,
            bytes_this_apply: 0,
            camera,
            target_format: format,
        })
    }

    /// The device the buffers live on.
    ///
    /// Exposed because whoever owns the *surface* also owns reconfiguring it,
    /// and reconfiguring needs the device — `GpuRenderer` is the one place the
    /// device is held, so it lends it out rather than cloning it (wgpu handles
    /// are not `Clone`, and a second `Device` would be a second loss domain).
    pub fn device(&self) -> &wgpu::Device {
        &self.device
    }

    /// The queue the writes go through.
    pub fn queue(&self) -> &wgpu::Queue {
        &self.queue
    }

    pub fn stats(&self) -> GpuStats {
        self.stats
    }

    pub fn camera(&self) -> Camera {
        self.camera
    }

    pub fn target_format(&self) -> wgpu::TextureFormat {
        self.target_format
    }

    /// The number of nodes the GPU currently holds buffers for.
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    /// Point the camera at a document rectangle with a pixel size.
    ///
    /// One 32-byte uniform write: panning and zooming must never re-upload
    /// geometry (RULE 2 applies to the viewport too).
    pub fn set_camera(&mut self, camera: Camera) {
        if camera == self.camera {
            return;
        }
        self.camera = camera;
        let bytes = camera.to_bytes();
        self.queue.write_buffer(&self.camera_buffer, 0, &bytes);
    }

    /// Apply the last plan to the GPU.
    pub fn apply(&mut self, scene: &RenderScene) -> Result<GpuStats, RenderError> {
        self.writes_this_apply = 0;
        self.bytes_this_apply = 0;
        scene.flush(self)?;
        self.stats.nodes = self.nodes.len();
        self.stats.buffers = self.count_buffers();
        self.stats.buffer_bytes = self.count_buffer_bytes();
        self.stats.instance_bytes = self.instance_capacity as u64 * InstanceRaw::STRIDE;
        self.stats.ramp_bytes = self.ramp_capacity;
        self.stats.draw_calls = self.nodes.values().map(draw_calls_of).sum();
        self.stats.writes = self.writes_this_apply;
        self.stats.bytes_written = self.bytes_this_apply;
        Ok(self.stats)
    }

    /// Draw the scene into `view`.
    ///
    /// The draw list is the concatenation of every node's **draw items** — a
    /// fill or a stroke layer, in stack order — so a node with two strokes is
    /// two draws of two meshes, and a node with three stacked fills is three
    /// draws of the same fill mesh. Each draw's instance range is its single row,
    /// so `@builtin(instance_index)` selects that item's paint with no per-draw
    /// uniform rebinding.
    ///
    /// # Blend modes and the frame split
    ///
    /// `Normal` layers are drawn in one pass, back to front, with straight-alpha
    /// blending. A layer with `Multiply`, `Screen` or `Overlay` cannot be
    /// expressed with a fixed-function blend state — each is a function of the
    /// *destination* colour — so such a draw is preceded by a copy of the canvas
    /// into the backdrop texture, and its fragment stage reads the backdrop,
    /// composites, and writes the result with `REPLACE`. The cost is honest and
    /// local: one copy and one extra pass per blended *layer*, and nothing at all
    /// for the (overwhelmingly common) all-Normal scene.
    ///
    /// The target texture must be created with `COPY_SRC` for that path — both
    /// callers do (the surface config in `vectra-wasm`, the law tests' texture).
    pub fn render(
        &mut self,
        target: &wgpu::Texture,
        view: &wgpu::TextureView,
        scene: &RenderScene,
        clear: vectra_core::Color,
    ) {
        // 1. Resolve the draw list before touching the encoder: which node, which
        //    mesh, which instance row, and whether it blends with the backdrop.
        let mut draws: Vec<GpuDraw> = Vec::new();
        for (id, item) in scene.draw_items() {
            let Some(node) = self.nodes.get(&id) else {
                continue;
            };
            let empty = match item.mesh {
                DrawMesh::Fill => node.fill.is_empty(),
                DrawMesh::Stroke(index) => node
                    .strokes
                    .get(index as usize)
                    .and_then(|mesh| mesh.as_ref())
                    .map(GpuMesh::is_empty)
                    .unwrap_or(true),
            };
            if empty {
                continue;
            }
            draws.push(GpuDraw {
                node: id,
                mesh: item.mesh,
                instance: item.slot,
                blend: item.blend,
            });
        }

        let target_size = target.size();
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("vectra.encoder"),
            });

        let mut passes = 0usize;
        let mut blend_draws = 0usize;
        let mut load = Some(wgpu::LoadOp::Clear(wgpu::Color {
            r: clear.r as f64 / 255.0,
            g: clear.g as f64 / 255.0,
            b: clear.b as f64 / 255.0,
            a: clear.a as f64 / 255.0,
        }));

        let mut index = 0;
        while index < draws.len() {
            // A run of `Normal` draws shares one pass (and the clear, if it is
            // the first pass of the frame).
            let run_start = index;
            while index < draws.len() && !draws[index].needs_backdrop() {
                index += 1;
            }
            if index > run_start {
                let range = run_start..index;
                passes += 1;
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("vectra.pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: load.take().unwrap_or(wgpu::LoadOp::Load),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                });
                pass.set_bind_group(0, &self.camera_bind_group, &[]);
                pass.set_bind_group(1, &self.backdrop_bind_group, &[]);
                for draw in &draws[range] {
                    self.draw_one(&mut pass, draw, false);
                }
            }
            if index >= draws.len() {
                break;
            }

            // One blended draw: snapshot the canvas, then draw against it.
            let draw = draws[index].node;
            let mesh = draws[index].mesh;
            let instance = draws[index].instance;
            blend_draws += 1;
            self.ensure_backdrop(target_size);
            encoder.copy_texture_to_texture(
                wgpu::ImageCopyTexture {
                    texture: target,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::ImageCopyTexture {
                    texture: self
                        .backdrop_texture
                        .as_ref()
                        .expect("backdrop exists after ensure_backdrop"),
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::Extent3d {
                    width: target_size.width,
                    height: target_size.height,
                    depth_or_array_layers: 1,
                },
            );
            passes += 1;
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("vectra.pass.blend"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: load.take().unwrap_or(wgpu::LoadOp::Load),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_bind_group(0, &self.camera_bind_group, &[]);
            pass.set_bind_group(1, &self.backdrop_bind_group, &[]);
            self.draw_one(
                &mut pass,
                &GpuDraw {
                    node: draw,
                    mesh,
                    instance,
                    blend: draws[index].blend,
                },
                true,
            );
            index += 1;
        }

        self.queue.submit(Some(encoder.finish()));
        self.stats.passes = passes;
        self.stats.blend_draws = blend_draws;
    }

    /// Issue one draw item: pick the pipeline the item's blend mode needs, bind
    /// the item's mesh, and draw its single instance row.
    fn draw_one<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>, draw: &GpuDraw, blended: bool) {
        let Some(node) = self.nodes.get(&draw.node) else {
            return;
        };
        let instances = draw.instance..draw.instance.saturating_add(1);
        match draw.mesh {
            DrawMesh::Fill => {
                if node.fill.is_empty() {
                    return;
                }
                pass.set_pipeline(if blended {
                    &self.fill_blend_pipeline
                } else {
                    &self.fill_pipeline
                });
                pass.set_vertex_buffer(0, node.fill.vertices.slice(..));
                pass.set_index_buffer(node.fill.indices.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..node.fill.index_count, 0, instances);
            }
            DrawMesh::Stroke(index) => {
                let Some(Some(mesh)) = node.strokes.get(index as usize) else {
                    return;
                };
                if mesh.is_empty() {
                    return;
                }
                pass.set_pipeline(if blended {
                    &self.stroke_blend_pipeline
                } else {
                    &self.stroke_pipeline
                });
                pass.set_vertex_buffer(0, mesh.vertices.slice(..));
                pass.set_index_buffer(mesh.indices.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..mesh.index_count, 0, instances);
            }
        }
    }

    /// Make sure the backdrop texture matches the target, and that the bind group
    /// points at it. Recreated only when the target's size changes — a resize, not
    /// a frame.
    fn ensure_backdrop(&mut self, size: wgpu::Extent3d) {
        let wanted = (size.width.max(1), size.height.max(1));
        if self.backdrop_texture.is_some() && self.backdrop_size == wanted {
            return;
        }
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("vectra.backdrop"),
            size: wgpu::Extent3d {
                width: wanted.0,
                height: wanted.1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: self.target_format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        self.backdrop_bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("vectra.backdrop"),
            layout: &self.backdrop_bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&view),
            }],
        });
        self.backdrop_view = view;
        self.backdrop_texture = Some(texture);
        self.backdrop_size = wanted;
    }

    fn count_buffers(&self) -> usize {
        self.nodes
            .values()
            .map(|node| 2 + node.strokes.iter().flatten().count() * 2)
            .sum::<usize>()
            + usize::from(!self.ramp_buffer_ms())
    }

    fn count_buffer_bytes(&self) -> u64 {
        self.nodes
            .values()
            .map(|node| {
                let fill = node.fill.vertex_bytes + node.fill.index_bytes;
                let strokes: u64 = node
                    .strokes
                    .iter()
                    .flatten()
                    .map(|mesh| mesh.vertex_bytes + mesh.index_bytes)
                    .sum();
                fill + strokes
            })
            .sum::<u64>()
            + self.ramp_capacity
    }

    /// Is the ramp buffer a real allocation? (It always is; the helper exists so
    /// the buffer count and the byte count ask the same question.)
    fn ramp_buffer_ms(&self) -> bool {
        self.ramp_capacity == 0
    }

    /// A zero-byte buffer, used as the "not written yet" half of a stroke layer's
    /// buffer pair. Zero-size buffers are legal in WebGPU and cost nothing.
    fn allocate_zero(&mut self, label: &str) -> wgpu::Buffer {
        self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: 0,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::INDEX,
            mapped_at_creation: false,
        })
    }

    fn allocate_indices(&mut self, mesh: &Mesh) -> wgpu::Buffer {
        self.allocate(
            "vectra.indices",
            mesh.index_bytes().len() as u64,
            wgpu::BufferUsages::INDEX,
        )
    }

    fn note_write(&mut self, bytes: usize) {
        self.writes_this_apply += 1;
        self.bytes_this_apply += bytes as u64;
    }

    /// Grow the shared instance buffer if the scene needs more rows, and rebuild
    /// the bind group that points at it. This is the *only* whole-buffer
    /// operation in the renderer, and it happens O(log n) times per session.
    fn ensure_instance_capacity(&mut self, count: u32) -> Result<(), RenderError> {
        if count <= self.instance_capacity {
            return Ok(());
        }
        let capacity = ((count as f64) * BUFFER_GROWTH_SLACK).ceil() as u32;
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("vectra.instances"),
            size: capacity as u64 * InstanceRaw::STRIDE,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.instance_buffer = buffer;
        self.instance_capacity = capacity;
        self.camera_bind_group = bind_group(
            &self.device,
            &self.bind_group_layout,
            &self.camera_buffer,
            &self.instance_buffer,
            &self.ramp_buffer,
        );
        self.stats.reallocations += 1;
        Ok(())
    }

    /// Grow the ramp storage buffer, and rebuild the bind group that points at it.
    fn ensure_ramp_capacity(&mut self, bytes: u64) -> Result<(), RenderError> {
        if bytes <= self.ramp_capacity {
            return Ok(());
        }
        let capacity = ((bytes as f64) * BUFFER_GROWTH_SLACK).ceil() as u64;
        self.ramp_buffer = self.allocate("vectra.ramps", capacity, wgpu::BufferUsages::STORAGE);
        self.ramp_capacity = capacity;
        self.camera_bind_group = bind_group(
            &self.device,
            &self.bind_group_layout,
            &self.camera_buffer,
            &self.instance_buffer,
            &self.ramp_buffer,
        );
        self.stats.reallocations += 1;
        Ok(())
    }

    fn allocate(&mut self, label: &str, bytes: u64, usage: wgpu::BufferUsages) -> wgpu::Buffer {
        // WebGPU forbids zero-sized buffers: an empty mesh still gets a minimal
        // one so every node's record is uniform.
        let size = bytes.max(4).next_multiple_of(4);
        self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size,
            usage: usage | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }
}

fn bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    camera: &wgpu::Buffer,
    instances: &wgpu::Buffer,
    ramps: &wgpu::Buffer,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("vectra.camera_bind_group"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: camera.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: instances.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: ramps.as_entire_binding(),
            },
        ],
    })
}

fn draw_calls_of(node: &GpuNode) -> usize {
    usize::from(!node.fill.is_empty())
        + node
            .strokes
            .iter()
            .flatten()
            .filter(|mesh| !mesh.is_empty())
            .count()
}

/// One draw the frame will issue: a node's draw item, resolved to buffers.
struct GpuDraw {
    node: NodeId,
    mesh: DrawMesh,
    instance: u32,
    blend: f32,
}

impl GpuDraw {
    fn needs_backdrop(&self) -> bool {
        self.blend != 0.0
    }
}

/// The GPU-side implementation of the sink contract — **the only place in the
/// renderer that calls `write_buffer`.**
impl BufferSink for GpuRenderer {
    fn create_node(
        &mut self,
        id: NodeId,
        slot: u32,
        fill: &Mesh,
        strokes: &[Mesh],
    ) -> Result<(), RenderError> {
        let _ = slot;
        let alloc = |renderer: &mut Self, label: &str, mesh: &Mesh, kind: MeshKind| {
            let vertices = renderer.allocate(
                label,
                mesh.vertex_bytes().len() as u64,
                wgpu::BufferUsages::VERTEX,
            );
            let indices = renderer.allocate_indices(mesh);
            let entry = GpuMesh {
                vertices,
                indices,
                vertex_bytes: mesh.vertex_bytes().len() as u64,
                index_bytes: mesh.index_bytes().len() as u64,
                index_count: 0,
            };
            let _ = kind;
            entry
        };
        let fill_mesh = alloc(self, "vectra.fill", fill, MeshKind::Fill);
        let stroke_meshes: Vec<Option<GpuMesh>> = strokes
            .iter()
            .enumerate()
            .map(|(index, mesh)| {
                Some(alloc(
                    self,
                    "vectra.stroke",
                    mesh,
                    MeshKind::Stroke(index as u32),
                ))
            })
            .collect();
        self.nodes.insert(
            id,
            GpuNode {
                fill: fill_mesh,
                strokes: stroke_meshes,
            },
        );
        Ok(())
    }

    fn write(
        &mut self,
        id: NodeId,
        slot: u32,
        kind: WriteKind,
        bytes: &[u8],
    ) -> Result<(), RenderError> {
        if matches!(kind, WriteKind::Instance) {
            self.ensure_instance_capacity(slot + 1)?;
        }
        if !self.nodes.contains_key(&id) {
            // A write for a node whose buffers were dropped: nothing to do. The
            // plan is derived from the scene, so this only happens if a caller
            // replays a stale plan, which the tests do not do.
            return Ok(());
        }

        match kind {
            WriteKind::Create => {}
            WriteKind::Vertices(MeshKind::Fill) => {
                self.write_mesh_buffer(id, bytes, MeshBuffer::FillVertices);
            }
            WriteKind::Indices(MeshKind::Fill) => {
                self.write_mesh_buffer(id, bytes, MeshBuffer::FillIndices);
                if let Some(node) = self.nodes.get_mut(&id) {
                    node.fill.index_count = (bytes.len() / 4) as u32;
                }
            }
            WriteKind::Vertices(MeshKind::Stroke(index)) => {
                self.write_mesh_buffer(id, bytes, MeshBuffer::StrokeVertices(index));
            }
            WriteKind::Indices(MeshKind::Stroke(index)) => {
                self.write_mesh_buffer(id, bytes, MeshBuffer::StrokeIndices(index));
                if let Some(node) = self.nodes.get_mut(&id) {
                    if let Some(Some(mesh)) = node.strokes.get_mut(index as usize) {
                        mesh.index_count = (bytes.len() / 4) as u32;
                    }
                }
            }
            WriteKind::Ramps => {
                // The whole stop array, one write. It is the unit the shader
                // addresses, so a partial update would need the shader to know
                // about holes; the table is small (640 B for a busy scene).
                if !bytes.is_empty() {
                    self.ensure_ramp_capacity(bytes.len() as u64)?;
                    self.queue.write_buffer(&self.ramp_buffer, 0, bytes);
                }
            }
            WriteKind::Instance => {
                // **The surgical write.** One draw item's 80 bytes at its own slot:
                // no other row of the instance array is touched, and no vertex
                // or index buffer is involved at all.
                if !bytes.is_empty() {
                    let offset = slot as u64 * InstanceRaw::STRIDE;
                    self.queue
                        .write_buffer(&self.instance_buffer, offset, bytes);
                }
            }
        }
        self.note_write(bytes.len());
        Ok(())
    }

    fn drop_node(&mut self, id: NodeId) -> Result<(), RenderError> {
        self.nodes.remove(&id);
        Ok(())
    }

    fn ensure_instances(&mut self, count: u32) -> Result<(), RenderError> {
        self.ensure_instance_capacity(count)
    }

    fn set_order(&mut self, order: &[NodeId]) -> Result<(), RenderError> {
        self.order = order.to_vec();
        Ok(())
    }
}

/// The four mesh buffers, named so the growth path can be shared.
#[derive(Debug, Clone, Copy)]
enum MeshBuffer {
    FillVertices,
    FillIndices,
    /// One stroke layer's vertices, by stack index.
    StrokeVertices(u32),
    /// One stroke layer's indices, by stack index.
    StrokeIndices(u32),
}

impl GpuRenderer {
    /// Write one of the four mesh buffers, growing it first if the new mesh no
    /// longer fits. Growth is per-node and per-buffer: a node that gets bigger
    /// never disturbs another node's buffers.
    fn write_mesh_buffer(&mut self, id: NodeId, bytes: &[u8], which: MeshBuffer) {
        let needs_growth = match self.nodes.get(&id) {
            Some(node) => allocated_of(node, which) < bytes.len() as u64,
            None => return,
        };
        if needs_growth {
            let is_vertices = matches!(
                which,
                MeshBuffer::FillVertices | MeshBuffer::StrokeVertices(_)
            );
            let label = if is_vertices {
                "vectra.vertices"
            } else {
                "vectra.indices"
            };
            let usage = if is_vertices {
                wgpu::BufferUsages::VERTEX
            } else {
                wgpu::BufferUsages::INDEX
            };
            let wanted = ((bytes.len() as f64) * BUFFER_GROWTH_SLACK).ceil() as u64;
            let buffer = self.allocate(label, wanted, usage);
            // A stroke layer that has never written before gets its buffer *pair*
            // now: the half the caller did not name is a zero-byte placeholder
            // until the matching write arrives (it always does — the plan emits
            // vertices and indices together).
            let needs_placeholder = match (which, self.nodes.get(&id)) {
                (
                    MeshBuffer::StrokeVertices(index) | MeshBuffer::StrokeIndices(index),
                    Some(node),
                ) => node
                    .strokes
                    .get(index as usize)
                    .map(|slot| slot.is_none())
                    .unwrap_or(true),
                _ => false,
            };
            let mut placeholder = if needs_placeholder {
                Some(self.allocate_zero("vectra.mesh.placeholder"))
            } else {
                None
            };
            if let Some(node) = self.nodes.get_mut(&id) {
                match which {
                    MeshBuffer::FillVertices => {
                        node.fill.vertices = buffer;
                        node.fill.vertex_bytes = wanted;
                    }
                    MeshBuffer::FillIndices => {
                        node.fill.indices = buffer;
                        node.fill.index_bytes = wanted;
                    }
                    MeshBuffer::StrokeVertices(index) | MeshBuffer::StrokeIndices(index) => {
                        let slot = ensure_stroke_slot(node, index);
                        match slot {
                            Some(mesh) if is_vertices => {
                                mesh.vertices = buffer;
                                mesh.vertex_bytes = wanted;
                            }
                            Some(mesh) => {
                                mesh.indices = buffer;
                                mesh.index_bytes = wanted;
                            }
                            None => {
                                // The first write for this stroke layer: the
                                // buffer it was allocated becomes the one the
                                // layer keeps, and the other half waits for the
                                // indices write that always follows.
                                let placeholder = placeholder
                                    .take()
                                    .expect("a missing stroke slot has a placeholder");
                                node.strokes[index as usize] = Some(if is_vertices {
                                    GpuMesh {
                                        vertices: buffer,
                                        indices: placeholder,
                                        vertex_bytes: wanted,
                                        index_bytes: 0,
                                        index_count: 0,
                                    }
                                } else {
                                    GpuMesh {
                                        vertices: placeholder,
                                        indices: buffer,
                                        vertex_bytes: 0,
                                        index_bytes: wanted,
                                        index_count: 0,
                                    }
                                });
                            }
                        }
                    }
                }
            }
            self.stats.reallocations += 1;
        }
        if bytes.is_empty() {
            return;
        }
        let is_vertices = matches!(
            which,
            MeshBuffer::FillVertices | MeshBuffer::StrokeVertices(_)
        );
        let Some(node) = self.nodes.get(&id) else {
            return;
        };
        let buffer = match which {
            MeshBuffer::FillVertices => &node.fill.vertices,
            MeshBuffer::FillIndices => &node.fill.indices,
            MeshBuffer::StrokeVertices(index) | MeshBuffer::StrokeIndices(index) => {
                match node.strokes.get(index as usize).and_then(|m| m.as_ref()) {
                    Some(mesh) if is_vertices => &mesh.vertices,
                    Some(mesh) => &mesh.indices,
                    None => return,
                }
            }
        };
        self.queue.write_buffer(buffer, 0, bytes);
    }
}

fn allocated_of(node: &GpuNode, which: MeshBuffer) -> u64 {
    let vertices = matches!(
        which,
        MeshBuffer::FillVertices | MeshBuffer::StrokeVertices(_)
    );
    match which {
        MeshBuffer::FillVertices => node.fill.vertex_bytes,
        MeshBuffer::FillIndices => node.fill.index_bytes,
        MeshBuffer::StrokeVertices(index) | MeshBuffer::StrokeIndices(index) => {
            match node.strokes.get(index as usize).and_then(|m| m.as_ref()) {
                Some(mesh) if vertices => mesh.vertex_bytes,
                Some(mesh) => mesh.index_bytes,
                None => 0,
            }
        }
    }
}

/// Grow the stroke list so `index` exists, returning that entry.
fn ensure_stroke_slot(node: &mut GpuNode, index: u32) -> Option<&mut GpuMesh> {
    while node.strokes.len() <= index as usize {
        node.strokes.push(None);
    }
    node.strokes[index as usize].as_mut()
}
