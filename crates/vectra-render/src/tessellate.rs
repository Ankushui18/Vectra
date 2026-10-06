//! Tessellation: `EvaluatedPrimitive` → triangle meshes (Task 5.0 §1).
//!
//! `lyon::tessellation` does all the work; this module's job is to make its
//! output *surgical*:
//!
//! * the node is tessellated **in its local frame** (see [`crate::geometry`]),
//!   so a translation never invalidates a vertex;
//! * the fill and the stroke are separate meshes with separate buffers, so a
//!   stroke-width change cannot rewrite fill vertices and vice versa;
//! * the vertex format is exactly `vec2<f32>` — 8 bytes — because the per-node
//!   colour/transform data lives in the instance array, not in every vertex.
//!
//! Stroke rendering is *geometric*: lyon expands the outline into triangles at
//! the resolved width, so the shader needs no distance field and no second pass
//! (the brief's "simple approach for Phase 1" — documented as a deliberate
//! choice in `TASK-5.0-DESIGN.md`).

use lyon::tessellation::{
    BuffersBuilder, FillOptions, FillTessellator, FillVertex, FillVertexConstructor, StrokeOptions,
    StrokeTessellator, StrokeVertex, StrokeVertexConstructor, VertexBuffers,
};
use vectra_core::NodeId;
use vectra_geometry::{EvaluatedPrimitive, EvaluatedStyle};

use crate::error::RenderError;
use crate::geometry::{
    flatten, rings_local, rings_to_path, Bounds, Flattened, GeometryKey, Mesh, FILL_RULE,
};

/// One GPU vertex: a position in the node's local frame.
///
/// The layout is declared once, in [`vertex_layout`], and asserted against this
/// struct's size by a unit test — a mismatch between the two is the classic
/// silent corruption in hand-rolled vertex pipelines.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct GpuVertex {
    pub position: [f32; 2],
}

impl GpuVertex {
    /// `vec2<f32>`.
    pub const SIZE: u64 = 8;
}

/// What one node's tessellation produced.
#[derive(Debug, Clone, PartialEq)]
pub struct Tessellation {
    /// World-space bounding box (the fast hit-test prefilter).
    pub world_bounds: Bounds,
    /// The flattened rings in **world** coordinates (exact hit testing).
    pub rings: Vec<Vec<[f32; 2]>>,
    /// `(local shape, world origin)` — the incremental-update key.
    pub key: GeometryKey,
    pub fill: Mesh,
    /// One mesh per **stroke layer** of the node's stack, in stack order, each
    /// expanded to its own width (Task 10.2 RULE 3). A layer with no visible
    /// stroke — zero width, transparent paint, hidden, degenerate geometry —
    /// still gets an entry, empty, so the i-th mesh is always the i-th stroke.
    pub strokes: Vec<Mesh>,
}

impl Tessellation {
    pub fn vertex_count(&self) -> usize {
        self.fill.vertices.len() + self.strokes.iter().map(|m| m.vertices.len()).sum::<usize>()
    }

    pub fn index_count(&self) -> usize {
        self.fill.indices.len() + self.strokes.iter().map(|m| m.indices.len()).sum::<usize>()
    }
}

/// Fill vertex constructor: position only.
struct FillCtor;
impl FillVertexConstructor<GpuVertex> for FillCtor {
    fn new_vertex(&mut self, vertex: FillVertex) -> GpuVertex {
        GpuVertex {
            position: [vertex.position().x, vertex.position().y],
        }
    }
}

/// Stroke vertex constructor: position only. Colour and width live in the
/// instance (the width is already baked into the expanded outline).
struct StrokeCtor;
impl StrokeVertexConstructor<GpuVertex> for StrokeCtor {
    fn new_vertex(&mut self, vertex: StrokeVertex) -> GpuVertex {
        GpuVertex {
            position: [vertex.position().x, vertex.position().y],
        }
    }
}

/// The vertex buffer layout for [`GpuVertex`], used by both pipelines.
pub fn vertex_layout() -> wgpu::VertexBufferLayout<'static> {
    const ATTRIBUTES: [wgpu::VertexAttribute; 1] = wgpu::vertex_attr_array![0 => Float32x2];
    wgpu::VertexBufferLayout {
        array_stride: GpuVertex::SIZE,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &ATTRIBUTES,
    }
}

/// Tessellate one evaluated node (fill + optional stroke) in its local frame.
///
/// A node whose geometry is degenerate (no rings, no area) yields an empty
/// tessellation rather than an error: the engine already diagnoses
/// unrenderable values, and an empty node must never break a frame.
pub fn tessellate(
    id: NodeId,
    primitive: &EvaluatedPrimitive,
    style: &EvaluatedStyle,
) -> Result<Tessellation, RenderError> {
    let flattened = flatten(primitive);
    tessellate_flattened(id, &flattened, style)
}

/// The tessellation body, on already-flattened geometry (so the caller can
/// share one flattening pass between the key, the hit rings and the meshes).
pub fn tessellate_flattened(
    id: NodeId,
    flattened: &Flattened,
    style: &EvaluatedStyle,
) -> Result<Tessellation, RenderError> {
    let key = GeometryKey::of(flattened);
    let origin = key.origin;
    let local_rings = rings_local(&flattened.rings, origin);
    let path = rings_to_path(&local_rings);

    let mut fill_buffers: VertexBuffers<GpuVertex, u32> = VertexBuffers::new();
    if !flattened.bounds.is_empty() {
        let mut tessellator = FillTessellator::new();
        let options = FillOptions::DEFAULT.with_fill_rule(FILL_RULE);
        tessellator
            .tessellate_path(
                &path,
                &options,
                &mut BuffersBuilder::new(&mut fill_buffers, FillCtor),
            )
            .map_err(|error| RenderError::Tessellation {
                node: id,
                message: error.to_string(),
            })?;
    }

    // One outline per stroke layer. The order is the stack's order, which is the
    // order the strokes are drawn in — a thin white stroke over a thick black one
    // is a *list*, and this loop is where that list becomes geometry.
    let mut strokes: Vec<Mesh> = Vec::new();
    for layer in style.strokes() {
        let width = layer.stroke_width().unwrap_or(0.0).max(0.0);
        let visible = width > 0.0 && layer.is_visible() && !flattened.bounds.is_empty();
        let mut stroke_buffers: VertexBuffers<GpuVertex, u32> = VertexBuffers::new();
        if visible {
            let mut tessellator = StrokeTessellator::new();
            let options = StrokeOptions::DEFAULT.with_line_width(width.min(f32::MAX as f64) as f32);
            tessellator
                .tessellate_path(
                    &path,
                    &options,
                    &mut BuffersBuilder::new(&mut stroke_buffers, StrokeCtor),
                )
                .map_err(|error| RenderError::Tessellation {
                    node: id,
                    message: error.to_string(),
                })?;
        }
        strokes.push(Mesh {
            vertices: stroke_buffers.vertices.iter().map(|v| v.position).collect(),
            indices: stroke_buffers.indices.clone(),
        });
    }

    let fill = Mesh {
        vertices: fill_buffers.vertices.iter().map(|v| v.position).collect(),
        indices: fill_buffers.indices.clone(),
    };
    // A non-finite vertex must never reach a buffer: one NaN turns a shape into
    // an invisible (or screen-filling) triangle. The evaluator promises finite
    // inputs, so this is a backstop, not a policy.
    let mut checked: Vec<(&Mesh, String)> = vec![(&fill, "fill".to_string())];
    checked.extend(
        strokes
            .iter()
            .enumerate()
            .map(|(index, mesh)| (mesh, format!("stroke[{index}]"))),
    );
    for (mesh, kind) in checked {
        if !mesh.is_finite() {
            return Err(RenderError::Tessellation {
                node: id,
                message: format!("{kind} mesh contains a non-finite vertex"),
            });
        }
        if !mesh.indices_in_bounds() {
            return Err(RenderError::Tessellation {
                node: id,
                message: format!("{kind} mesh has an index out of bounds"),
            });
        }
    }

    Ok(Tessellation {
        world_bounds: flattened.bounds,
        rings: flattened.rings.clone(),
        key,
        fill,
        strokes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use vectra_core::Color;

    fn style() -> EvaluatedStyle {
        EvaluatedStyle::solid(Color::rgb(0x22, 0x66, 0xee), Color::TRANSPARENT, 0.0, 1.0)
    }

    #[test]
    fn vertex_layout_matches_the_struct() {
        let layout = vertex_layout();
        assert_eq!(layout.array_stride, GpuVertex::SIZE);
        assert_eq!(
            std::mem::size_of::<GpuVertex>() as u64,
            GpuVertex::SIZE,
            "repr(C) GpuVertex must be exactly two f32s"
        );
        assert_eq!(layout.attributes.len(), 1);
        assert_eq!(layout.attributes[0].offset, 0);
        assert_eq!(layout.attributes[0].format, wgpu::VertexFormat::Float32x2);
    }

    #[test]
    fn a_rectangle_tessellates_to_its_own_area() {
        let rect = EvaluatedPrimitive::Rect {
            x: 10.0,
            y: 20.0,
            w: 30.0,
            h: 40.0,
            corner_radius: 0.0,
        };
        let t = tessellate(NodeId::nil(), &rect, &style()).unwrap();
        assert_eq!(t.fill.indices.len(), 6, "two triangles");
        // lyon winds fills clockwise, so the *signed* area is negative; the
        // magnitude is the region's area, which is what a fill must cover.
        assert!((t.fill.area() - 1200.0).abs() < 1e-6);
        assert!(t.fill.signed_area() < 0.0, "clockwise winding");
        // The local frame is rebased on the bounding box minimum…
        assert_eq!(t.key.origin, (10.0, 20.0));
        assert_eq!(
            t.world_bounds,
            Bounds::new(10.0, 20.0, 40.0, 60.0),
            "world bounds are absolute"
        );
        // …so local vertices start at (0,0).
        let min_x = t
            .fill
            .vertices
            .iter()
            .map(|v| v[0])
            .fold(f32::INFINITY, f32::min);
        let min_y = t
            .fill
            .vertices
            .iter()
            .map(|v| v[1])
            .fold(f32::INFINITY, f32::min);
        assert!(min_x.abs() < 1e-4 && min_y.abs() < 1e-4);
    }

    #[test]
    fn a_translation_moves_the_origin_and_keeps_every_vertex_identical() {
        let at_origin = EvaluatedPrimitive::Rect {
            x: 0.0,
            y: 0.0,
            w: 10.0,
            h: 10.0,
            corner_radius: 0.0,
        };
        let moved = EvaluatedPrimitive::Rect {
            x: 137.5,
            y: -42.25,
            w: 10.0,
            h: 10.0,
            corner_radius: 0.0,
        };
        let a = tessellate(NodeId::nil(), &at_origin, &style()).unwrap();
        let b = tessellate(NodeId::nil(), &moved, &style()).unwrap();
        assert_eq!(a.key.shape, b.key.shape, "same shape");
        assert_ne!(a.key.origin, b.key.origin, "different placement");
        assert_eq!(
            a.fill.vertices, b.fill.vertices,
            "the local vertex data is bit-identical — this is the drag fast path"
        );
    }

    #[test]
    fn a_stroke_only_appears_when_it_is_visible() {
        let rect = EvaluatedPrimitive::Rect {
            x: 0.0,
            y: 0.0,
            w: 10.0,
            h: 10.0,
            corner_radius: 0.0,
        };
        let t = tessellate(NodeId::nil(), &rect, &style()).unwrap();
        assert!(
            t.strokes.is_empty(),
            "a style with no stroke layer has no outline"
        );

        // A *hidden* stroke layer still owns a mesh; it is simply empty, which is
        // what keeps the draw list's indices stable when the eye is clicked
        // (Task 10.2 RULE 4: a toggle changes no geometry).
        let mut hidden =
            EvaluatedStyle::solid(Color::rgb(0x22, 0x66, 0xee), Color::BLACK, 2.0, 1.0);
        hidden.appearances[1].visible = false;
        let t = tessellate(NodeId::nil(), &rect, &hidden).unwrap();
        assert_eq!(t.strokes.len(), 1);
        assert!(t.strokes[0].is_empty(), "a hidden stroke draws nothing");

        let stroked = EvaluatedStyle::solid(Color::rgb(0x22, 0x66, 0xee), Color::BLACK, 2.0, 1.0);
        let t = tessellate(NodeId::nil(), &rect, &stroked).unwrap();
        assert_eq!(t.strokes.len(), 1, "one mesh per stroke layer");
        assert!(
            !t.strokes[0].is_empty(),
            "the outline is expanded to triangles"
        );
        // A 2-wide stroke on a 10×10 square: outer ring 12×12, inner 8×8 — the
        // stroke mesh is two nested windings, so its area is their difference.
        assert!((t.strokes[0].area() - (144.0 - 64.0)).abs() < 1.0);
    }

    #[test]
    fn a_circle_tessellates_close_to_its_area() {
        let circle = EvaluatedPrimitive::Circle {
            cx: 0.0,
            cy: 0.0,
            r: 10.0,
        };
        let t = tessellate(NodeId::nil(), &circle, &style()).unwrap();
        // Not πr²: the circle is sampled into a 64-gon by `primitive_to_path`,
        // so the tessellated area is that inscribed polygon's (0.9987 · πr²).
        let expected = 0.5 * 64.0 * 100.0 * (std::f64::consts::TAU / 64.0).sin();
        let area = t.fill.area();
        assert!(
            (area - expected).abs() / expected < 0.001,
            "{area} vs {expected}"
        );
    }
}
