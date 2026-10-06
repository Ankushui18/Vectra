//! Geometry preparation: flattening, bounds, node-local rebasing and the
//! **geometry key** that makes incremental updates possible (Task 5.0 §2/§3).
//!
//! # Why a node-local frame
//!
//! `EvaluatedScene` coordinates are absolute (already resolved). If the renderer
//! tessellated them as-is, then *every* pointer sample of a drag would change
//! every vertex and force a full re-tessellation of the dragged node — precisely
//! the work RULE 2 forbids.
//!
//! So each node is tessellated in its **own frame**: the flattened path is
//! rebased on its own bounding-box minimum, and that minimum travels in the
//! instance's `transform.translate`. Two consequences, both load-bearing:
//!
//! * a pure translation (a drag, a solver partner moving) leaves the local
//!   vertices **bit-identical** and writes only an 80-byte instance — the fast
//!   path the Incremental Update Law tests;
//! * local coordinates stay small, which keeps `f32` precision healthy for
//!   shapes authored far from the origin.
//!
//! # The geometry key
//!
//! A `lyon::path::Path` has no `PartialEq`, so "did this node's shape change?"
//! is answered by comparing a key computed from the same flattened point list
//! the tessellator consumes:
//!
//! * `shape` — the local vertices quantised to `1e-4` and rendered to text;
//! * `origin` — the world-space translation that was factored out.
//!
//! Equality of `shape` means the GPU's vertex buffer is still correct; equality
//! of `origin` means the instance's translate is still correct. They are
//! deliberately separate questions, because they have separate answers.

use lyon::path::iterator::PathIterator;
use lyon::path::Path;
use vectra_core::NodeId;
use vectra_geometry::{primitive_to_path, EvaluatedPrimitive};

/// Flattening tolerance in document units — the same policy `vectra-operations`
/// uses for its own lyon→region conversion (`FLATTEN_TOLERANCE = 0.05`), so the
/// geometry the GPU draws is the geometry the boolean engine reasons about.
pub const FLATTEN_TOLERANCE: f32 = 0.05;

/// Coordinate quantum of [`GeometryKey::shape`]. A move smaller than this is
/// sub-pixel at any sane zoom and is treated as "the same shape" — which is
/// exactly what the incremental fast path wants.
const SHAPE_QUANTUM: f64 = 1e-4;

/// An axis-aligned bounding box in document space (`f32`: it describes geometry
/// that is about to be `f32` on the GPU anyway).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bounds {
    pub min_x: f32,
    pub min_y: f32,
    pub max_x: f32,
    pub max_y: f32,
}

impl Bounds {
    /// The empty box: unions with it are no-ops.
    pub const EMPTY: Self = Self {
        min_x: f32::INFINITY,
        min_y: f32::INFINITY,
        max_x: f32::NEG_INFINITY,
        max_y: f32::NEG_INFINITY,
    };

    pub fn new(min_x: f32, min_y: f32, max_x: f32, max_y: f32) -> Self {
        Self {
            min_x,
            min_y,
            max_x,
            max_y,
        }
    }

    pub fn is_empty(&self) -> bool {
        !(self.max_x >= self.min_x && self.max_y >= self.min_y)
    }

    pub fn width(&self) -> f32 {
        (self.max_x - self.min_x).max(0.0)
    }

    pub fn height(&self) -> f32 {
        (self.max_y - self.min_y).max(0.0)
    }

    pub fn center(&self) -> (f32, f32) {
        (
            (self.min_x + self.max_x) * 0.5,
            (self.min_y + self.max_y) * 0.5,
        )
    }

    pub fn union(&self, other: &Self) -> Self {
        Self {
            min_x: self.min_x.min(other.min_x),
            min_y: self.min_y.min(other.min_y),
            max_x: self.max_x.max(other.max_x),
            max_y: self.max_y.max(other.max_y),
        }
    }

    pub fn contains(&self, x: f32, y: f32) -> bool {
        !self.is_empty() && x >= self.min_x && x <= self.max_x && y >= self.min_y && y <= self.max_y
    }

    /// Translate (the node-local → world step).
    pub fn translated(&self, dx: f32, dy: f32) -> Self {
        Self {
            min_x: self.min_x + dx,
            min_y: self.min_y + dy,
            max_x: self.max_x + dx,
            max_y: self.max_y + dy,
        }
    }

    /// Grow by `amount` on every side (hit-test candidates snap to the pixel
    /// grid, so the query box is always a little generous).
    pub fn expanded(&self, amount: f32) -> Self {
        Self {
            min_x: self.min_x - amount,
            min_y: self.min_y - amount,
            max_x: self.max_x + amount,
            max_y: self.max_y + amount,
        }
    }

    pub fn intersects(&self, other: &Self) -> bool {
        !self.is_empty()
            && !other.is_empty()
            && self.min_x <= other.max_x
            && other.min_x <= self.max_x
            && self.min_y <= other.max_y
            && other.min_y <= self.max_y
    }
}

/// One flattened subpath, in absolute document coordinates.
pub type Ring = Vec<[f32; 2]>;

/// The flattened geometry of a node, shared by the tessellator, the geometry
/// key and the hit index: **one** flattening pass, so the three can never
/// disagree about what the shape is.
#[derive(Debug, Clone, PartialEq)]
pub struct Flattened {
    /// Subpaths in draw order. An open subpath is closed implicitly by the
    /// fill rule and explicitly by the stroke tessellator, matching SVG.
    pub rings: Vec<Ring>,
    pub bounds: Bounds,
}

impl Flattened {
    pub fn vertex_count(&self) -> usize {
        self.rings.iter().map(Vec::len).sum()
    }
}

/// Flatten an evaluated primitive into polyline rings (world coordinates).
///
/// `lyon`'s flattener is the only curve sampler in the renderer, and it runs on
/// the *same* path builder the operations pipeline uses (`primitive_to_path`),
/// so a circle drawn on screen and a circle used as a boolean operand are the
/// same polygon.
pub fn flatten(primitive: &EvaluatedPrimitive) -> Flattened {
    let path = primitive_to_path(primitive);
    flatten_path(&path)
}

/// Flatten a path (the shared body of [`flatten`]).
pub fn flatten_path(path: &Path) -> Flattened {
    let mut rings: Vec<Ring> = Vec::new();
    let mut current: Ring = Vec::new();
    let mut bounds = Bounds::EMPTY;
    for event in path.iter().flattened(FLATTEN_TOLERANCE) {
        match event {
            lyon::path::PathEvent::Begin { at } => {
                if current.len() > 1 {
                    rings.push(std::mem::take(&mut current));
                } else {
                    current.clear();
                }
                current.push([at.x, at.y]);
                bounds = bounds.union(&Bounds::new(at.x, at.y, at.x, at.y));
            }
            lyon::path::PathEvent::Line { to, .. } => {
                current.push([to.x, to.y]);
                bounds = bounds.union(&Bounds::new(to.x, to.y, to.x, to.y));
            }
            lyon::path::PathEvent::End { .. } => {
                if current.len() > 1 {
                    rings.push(std::mem::take(&mut current));
                } else {
                    current.clear();
                }
            }
            _ => {}
        }
    }
    if current.len() > 1 {
        rings.push(current);
    }
    Flattened { rings, bounds }
}

/// Build a `lyon` path from rings in whatever frame the caller wants: the
/// tessellator and the hit test both consume this, so a ring list can be
/// rebased (local frame) or absolute without changing shape.
pub fn rings_to_path(rings: &[Ring]) -> Path {
    let mut builder = lyon::path::Path::builder();
    for ring in rings {
        if ring.len() < 2 {
            continue;
        }
        builder.begin(lyon::math::point(ring[0][0], ring[0][1]));
        for point in &ring[1..] {
            builder.line_to(lyon::math::point(point[0], point[1]));
        }
        builder.end(true);
    }
    builder.build()
}

/// Rebase rings on `origin`, i.e. into the node-local frame the GPU holds.
pub fn rings_local(rings: &[Ring], origin: (f32, f32)) -> Vec<Ring> {
    rings
        .iter()
        .map(|ring| {
            ring.iter()
                .map(|p| [p[0] - origin.0, p[1] - origin.1])
                .collect()
        })
        .collect()
}

/// The render-side identity of a shape — see the module docs for why it is
/// split in two.
#[derive(Debug, Clone, PartialEq)]
pub struct GeometryKey {
    /// Local vertex list, quantised (equality ⟺ the vertex buffer is current).
    pub shape: String,
    /// World-space translation factored out of the vertices (equality ⟺ the
    /// instance translate is current).
    pub origin: (f32, f32),
}

impl GeometryKey {
    /// Compute the key from already-flattened geometry (no extra flattening).
    pub fn of(flattened: &Flattened) -> Self {
        let bounds = flattened.bounds;
        let origin = if bounds.is_empty() {
            (0.0, 0.0)
        } else {
            (bounds.min_x, bounds.min_y)
        };
        let mut shape = String::with_capacity(flattened.vertex_count() * 16);
        for ring in &flattened.rings {
            shape.push('M');
            for point in ring {
                let x = ((point[0] - origin.0) as f64 / SHAPE_QUANTUM).round();
                let y = ((point[1] - origin.1) as f64 / SHAPE_QUANTUM).round();
                shape.push_str(&format!("{x},{y};"));
            }
            shape.push('Z');
        }
        Self { shape, origin }
    }
}

/// Which of a node's two tessellated meshes a write refers to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum MeshKind {
    Fill,
    /// The **n**-th stroke of the node's paint stack (Task 10.2 RULE 3).
    ///
    /// A painter with a thick black stroke and a thinner white one on top is
    /// asking for two outlines, not one outline drawn twice: the geometry
    /// depends on the width, so each stroke layer owns its own mesh. The index
    /// counts the node's stroke layers in stack order, which is also draw order.
    Stroke(u32),
}

impl MeshKind {
    pub fn tag(&self) -> String {
        match self {
            Self::Fill => "fill".to_string(),
            Self::Stroke(index) => format!("stroke[{index}]"),
        }
    }
}

/// The tessellation of one node in its local frame.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Mesh {
    pub vertices: Vec<[f32; 2]>,
    pub indices: Vec<u32>,
}

impl Mesh {
    pub fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }

    /// Bytes as the GPU wants them: `f32x2` per vertex, little-endian.
    pub fn vertex_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.vertices.len() * 8);
        for v in &self.vertices {
            out.extend_from_slice(&v[0].to_le_bytes());
            out.extend_from_slice(&v[1].to_le_bytes());
        }
        out
    }

    /// `u32` little-endian indices.
    pub fn index_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.indices.len() * 4);
        for i in &self.indices {
            out.extend_from_slice(&i.to_le_bytes());
        }
        out
    }

    /// The mesh's area — the magnitude of the signed triangle-sum. lyon winds
    /// fills clockwise (negative), so this is what the Validity Law compares
    /// against the primitive's analytic area.
    pub fn area(&self) -> f64 {
        self.signed_area().abs()
    }

    /// Signed area of the triangle soup: positive for counter-clockwise
    /// winding. The sign is a *winding* witness (kept so the law suite can
    /// assert lyon's convention), never an area.
    pub fn signed_area(&self) -> f64 {
        let mut area = 0.0;
        for triangle in self.indices.chunks_exact(3) {
            let a = self.vertices[triangle[0] as usize];
            let b = self.vertices[triangle[1] as usize];
            let c = self.vertices[triangle[2] as usize];
            area += ((b[0] as f64 - a[0] as f64) * (c[1] as f64 - a[1] as f64)
                - (c[0] as f64 - a[0] as f64) * (b[1] as f64 - a[1] as f64))
                * 0.5;
        }
        area
    }

    /// True when every vertex is a finite number in the GPU-representable
    /// range. The Tessellation Validity Law's first clause.
    pub fn is_finite(&self) -> bool {
        self.vertices.iter().all(|v| {
            v[0].is_finite() && v[1].is_finite() && v[0].abs() < 1.0e12 && v[1].abs() < 1.0e12
        })
    }

    /// True when every index addresses a vertex of this mesh.
    pub fn indices_in_bounds(&self) -> bool {
        self.indices
            .iter()
            .all(|i| (*i as usize) < self.vertices.len())
    }
}

/// Convenience alias used by the scene layer to name a node's mesh.
pub fn empty_mesh() -> Mesh {
    Mesh::default()
}

/// The one place a node's fill-rule parity is decided: `EvenOdd`, matching both
/// `lyon`'s default and the `geo::boolean` semantics the operations pipeline
/// canonicalises to (Task 4.0 §3.1). A renderer that filled with NonZero what a
/// boolean cut with EvenOdd would show two different shapes for one document.
pub const FILL_RULE: lyon::path::FillRule = lyon::path::FillRule::EvenOdd;

/// A polygon wrapper used by the hit test (geo's `Contains` does the crossing
/// math; the parity bookkeeping across rings is the caller's).
pub(crate) fn ring_polygon(ring: &Ring) -> Option<geo_types::Polygon<f64>> {
    if ring.len() < 3 {
        return None;
    }
    let coordinates: Vec<geo_types::Coord<f64>> = ring
        .iter()
        .map(|p| geo_types::Coord {
            x: p[0] as f64,
            y: p[1] as f64,
        })
        .collect();
    Some(geo_types::Polygon::new(
        geo_types::LineString::new(coordinates),
        Vec::new(),
    ))
}

/// A node with no drawable geometry (an empty path, a zero-size rectangle).
pub fn is_drawable(flattened: &Flattened) -> bool {
    !flattened.bounds.is_empty() && flattened.vertex_count() >= 2
}

/// Debug helper: the node id a flattened shape belongs to (kept for the tests'
/// error messages, so a failure names the node that produced it).
pub fn describe(id: NodeId, flattened: &Flattened) -> String {
    format!(
        "node {id}: {} ring(s), bounds {:?}",
        flattened.rings.len(),
        flattened.bounds
    )
}
