//! **The Region Graph** (Task 12.0 RULE 1).
//!
//! A Smart Fill is not a colour on a shape: it is a region *between* shapes.
//! This module is the machinery that answers "which distinct enclosed faces do
//! these paths create, and which of them is the face under this point".
//!
//! # The three questions, and the three answers
//!
//! ```text
//!   faces(sources)        → every distinct signature of (inside A?, inside B?, …)
//!   face_at(point)        → the face a drop landed in, the smallest one at a seam
//!   crossings + spans     → where the outlines meet, and the arcs between them
//! ```
//!
//! # Faces
//!
//! The atoms are built by **refinement**: start with the sources themselves and,
//! for every other source, split each atom into `atom ∩ other` and
//! `atom \ other`. What comes out is a set of disjoint regions, each of which is
//! either entirely inside or entirely outside every source — the *signature* —
//! and the union of which is the union of the sources. That is a planar
//! arrangement's faces, expressed in the boolean overlay the workspace already
//! trusts (`geo::BooleanOps`, the same maths as Task 4.0's operations), with no
//! second intersection implementation to drift from it.
//!
//! Faces come back with **proper holes**: the region is a `geo::MultiPolygon`
//! (exterior rings counter-clockwise, interiors clockwise) and its path is
//! written by [`multi_polygon_to_path`], so a hole is a hole under *both* the
//! even-odd and the nonzero fill rules.
//!
//! The empty signature — outside every source — is not a face: it is the
//! background, and a Smart Fill never fills it.
//!
//! # Spans (RULE 3)
//!
//! A **span** is one arc of a source's outline between two consecutive
//! intersection points. Spans are measured as arc length along a ring and carry
//! their two endpoints, which is exactly what "break this span" needs: cutting a
//! ring at two of its own crossings produces two independent closed paths.

use crate::convert::{multi_polygon_to_path, path_to_multi_polygon};
use crate::paths::{path_bounds, primitive_to_path};
use crate::scene::{EvaluatedPrimitive, EvaluatedScene};
use geo::{BooleanOps, Contains, Coord, LineString, MultiPolygon, Polygon};
use vectra_core::NodeId;

/// Distance below which two points (or two arc positions) are the same point.
///
/// The Region Graph compares *intersections*: the same crossing found by two
/// different ring pairs, or a crossing that moved because two sources are
/// re-derived through different code paths. 1e-6 document units is far below any
/// visible feature and far above the overlay's own noise.
pub const REGION_EPSILON: f64 = 1e-6;

// ── sources ─────────────────────────────────────────────────────────────────

/// One shape that takes part in a region graph: the node it came from, and the
/// region it covers.
///
/// The region — not the path — is what the graph needs: every question below is
/// "inside or outside", and a `MultiPolygon` answers it with the parity of the
/// source *as the renderer fills it* (nested subpaths are holes).
#[derive(Debug, Clone, PartialEq)]
pub struct SourceSpec {
    pub id: NodeId,
    pub region: MultiPolygon<f64>,
}

impl SourceSpec {
    /// The source's region, from an evaluated node — `None` when the node has
    /// no fillable area (an open sliver, an arc, an empty group).
    pub fn from_primitive(id: NodeId, primitive: &EvaluatedPrimitive) -> Option<Self> {
        let path = primitive_to_path(primitive);
        let region = path_to_multi_polygon(&path)?;
        if region.is_empty() {
            return None;
        }
        Some(Self { id, region })
    }
}

/// Read the named scene nodes as sources, in the order given. Ids that are not
/// in the scene, or that cover no area, are skipped — a group with no children
/// is not an intersection partner.
pub fn sources_of(scene: &EvaluatedScene, ids: &[NodeId]) -> Vec<SourceSpec> {
    ids.iter()
        .filter_map(|id| {
            let node = scene.get(*id)?;
            SourceSpec::from_primitive(*id, &node.primitive)
        })
        .collect()
}

// ── faces ───────────────────────────────────────────────────────────────────

/// One face of the arrangement: a region plus the answer to "inside which
/// sources is it?".
#[derive(Debug, Clone, PartialEq)]
pub struct RegionFace {
    /// The face as a region, holes included.
    pub region: MultiPolygon<f64>,
    /// Parallel to the spec list: `members[i]` ⟺ this face is inside source `i`.
    /// Never all-`false` (that is the background, not a face).
    pub members: Vec<bool>,
    /// Unsigned area — the UI's "fill this region" affordance sorts by it.
    pub area: f64,
    /// How many holes the face has. A Smart Fill over an "O" has one.
    pub holes: usize,
    /// The face as a closed lyon path — what the evaluator publishes.
    pub path: lyon::path::Path,
}

impl RegionFace {
    /// Is this face inside the `index`-th source?
    pub fn is_inside(&self, index: usize) -> bool {
        self.members.get(index).copied().unwrap_or(false)
    }

    /// The signature: which sources bound this face, in spec order. Two faces
    /// with the same signature are the same face in two pieces.
    pub fn signature(&self) -> &[bool] {
        &self.members
    }
}

/// **Which face is under this point?** — the drop test.
///
/// When several faces contain the point (a point on a shared seam, or a face
/// nested inside another), the **smallest** wins: the designer pointed at the
/// innermost thing they could see, and the outer face's area would be the wrong
/// answer to give them.
pub fn face_containing(faces: &[RegionFace], point: (f64, f64)) -> Option<usize> {
    let probe = geo::Point::new(point.0, point.1);
    faces
        .iter()
        .enumerate()
        .filter(|(_, face)| face.region.contains(&probe))
        .min_by(|(_, a), (_, b)| {
            a.area
                .partial_cmp(&b.area)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .map(|(index, _)| index)
}

/// The refinement itself: atoms of the arrangement, each with its signature.
///
/// See the module note. Sources are processed pairwise, so the work is
/// proportional to the *distinct* signatures rather than to `2^n` in practice —
/// each split that produces an empty or duplicate atom is dropped immediately.
pub fn atoms(specs: &[SourceSpec]) -> Vec<(MultiPolygon<f64>, Vec<bool>)> {
    if specs.is_empty() {
        return Vec::new();
    }
    let n = specs.len();
    let mut atoms: Vec<(MultiPolygon<f64>, Vec<bool>)> = Vec::new();
    // A new source's *own* contribution is what it covers that nothing before it
    // did, and that part has to become an atom of its own — a source disjoint
    // from the rest is still a face.
    for (index, spec) in specs.iter().enumerate() {
        let already = specs[..index]
            .iter()
            .fold(MultiPolygon::<f64>::new(Vec::new()), |acc, other| {
                acc.union(&other.region)
            });
        let fresh = canonical(&spec.region.difference(&already));
        if !fresh.is_empty() {
            let mut members = vec![false; n];
            members[index] = true;
            merge_atom(&mut atoms, fresh, members);
        }
        // …then split every existing atom in two: inside this source, and
        // outside it. The pieces are disjoint and their union is the atom, so
        // nothing is lost and nothing is counted twice.
        let mut refined: Vec<(MultiPolygon<f64>, Vec<bool>)> = Vec::with_capacity(atoms.len() * 2);
        for (region, members) in atoms.drain(..) {
            let inside = canonical(&region.intersection(&spec.region));
            if !inside.is_empty() {
                let mut members_inside = members.clone();
                members_inside[index] = true;
                merge_atom(&mut refined, inside, members_inside);
            }
            let outside = canonical(&region.difference(&spec.region));
            if !outside.is_empty() {
                let mut members_outside = members;
                members_outside[index] = false;
                merge_atom(&mut refined, outside, members_outside);
            }
        }
        atoms = refined;
    }
    atoms.sort_by(|a, b| {
        // Deterministic order: by signature, then by area. Two runs of the same
        // document produce the same face list in the same order.
        a.1.cmp(&b.1).then_with(|| {
            area_of(&a.0)
                .partial_cmp(&area_of(&b.0))
                .unwrap_or(std::cmp::Ordering::Equal)
        })
    });
    atoms
}

/// Fold a piece into the atom list: same signature ⇒ same face, so the pieces
/// are unioned rather than kept apart (a crescent-shaped overlap is one face in
/// two pieces, and it must measure as both).
fn merge_atom(
    atoms: &mut Vec<(MultiPolygon<f64>, Vec<bool>)>,
    region: MultiPolygon<f64>,
    members: Vec<bool>,
) {
    if let Some(existing) = atoms.iter_mut().find(|(_, other)| *other == members) {
        existing.0 = canonical(&existing.0.union(&region));
        return;
    }
    atoms.push((region, members));
}

/// Run the region through the overlay once, so a difference result that is
/// still "raw" (touching rings, an interior that is not yet canonicalised) is
/// normalised before it is measured or compared.
fn canonical(region: &MultiPolygon<f64>) -> MultiPolygon<f64> {
    region.union(&MultiPolygon::<f64>::new(Vec::new()))
}

fn area_of(region: &MultiPolygon<f64>) -> f64 {
    crate::convert::region_area(region)
}

// ── crossings and spans (RULE 3) ─────────────────────────────────────────────

/// Where two sources' outlines meet: the point, and how far along each source's
/// *outline* it sits, in document units.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Crossing {
    pub point: (f64, f64),
    /// Arc length along the first source's outline (ring-major order).
    pub a: f64,
    /// Arc length along the second source's outline.
    pub b: f64,
}

/// One arc of a source's outline between two consecutive crossings — the
/// **span** a designer selects and breaks.
///
/// `from`/`to` are arc lengths along the source's outline; `start`/`end` are
/// the points there, which is what the overlay draws and what the break
/// command cuts at.
#[derive(Debug, Clone, PartialEq)]
pub struct Span {
    /// The node whose outline this span belongs to.
    pub source: NodeId,
    pub from: f64,
    pub to: f64,
    pub start: (f64, f64),
    pub end: (f64, f64),
    /// The span's own arc length.
    pub length: f64,
    /// The ring (subpath) of the source this span lives on, and its index —
    /// the break command needs to know which ring it is cutting.
    pub ring: usize,
    /// The source's total outline length (all rings), for the UI's slider-scale
    /// arithmetic: a span's position is `from / total`.
    pub total: f64,
}

/// The raw rings of a path, in path order: each closed subpath's points, no
/// canonicalisation, no hole analysis.
///
/// Deliberately *not* the `geo` rings: a span is a piece of the path the
/// designer drew, so "the third ring of this path" has to mean what the path
/// says, not what an overlay's ring-sorting decided.
pub fn path_rings(path: &lyon::path::Path) -> Vec<Vec<Coord<f64>>> {
    use lyon::path::iterator::PathIterator;
    let mut rings: Vec<Vec<Coord<f64>>> = Vec::new();
    let mut ring: Vec<Coord<f64>> = Vec::new();
    for event in path.iter().flattened(crate::convert::FLATTEN_TOLERANCE) {
        match event {
            lyon::path::Event::Begin { at } => {
                ring.clear();
                ring.push(Coord::new(at.x as f64, at.y as f64));
            }
            lyon::path::Event::Line { to, .. }
            | lyon::path::Event::Quadratic { to, .. }
            | lyon::path::Event::Cubic { to, .. } => {
                ring.push(Coord::new(to.x as f64, to.y as f64));
            }
            lyon::path::Event::End { .. } => {
                push_ring(&mut rings, &mut ring);
            }
        }
    }
    push_ring(&mut rings, &mut ring);
    rings
}

fn push_ring(rings: &mut Vec<Vec<Coord<f64>>>, ring: &mut Vec<Coord<f64>>) {
    if ring.len() > 1 && ring[0] == ring[ring.len() - 1] {
        ring.pop();
    }
    if ring.len() >= 3 {
        rings.push(std::mem::take(ring));
    }
    ring.clear();
}

/// The rings of a scene node, or none when it is not in the scene.
pub fn source_rings(scene: &EvaluatedScene, id: NodeId) -> Vec<Vec<Coord<f64>>> {
    scene
        .get(id)
        .map(|node| path_rings(&primitive_to_path(&node.primitive)))
        .unwrap_or_default()
}

/// Every point where two rings cross, with the arc position along each.
///
/// Straight segments only: both rings are the *flattened* outlines the renderer
/// fills, so a "crossing" here is a crossing of the picture the designer sees,
/// not of a control polygon.
pub fn ring_crossings(a: &[Coord<f64>], b: &[Coord<f64>]) -> Vec<(f64, f64, (f64, f64))> {
    let mut found: Vec<(f64, f64, (f64, f64))> = Vec::new();
    if a.len() < 3 || b.len() < 3 {
        return found;
    }
    let total_a = ring_length(a);
    let total_b = ring_length(b);
    if total_a <= REGION_EPSILON || total_b <= REGION_EPSILON {
        return found;
    }
    let mut walked_a = 0.0;
    for i in 0..a.len() {
        let p = a[i];
        let q = a[(i + 1) % a.len()];
        let seg_a = (q - p).norm();
        if seg_a <= REGION_EPSILON {
            continue;
        }
        let mut walked_b = 0.0;
        for j in 0..b.len() {
            let r = b[j];
            let s = b[(j + 1) % b.len()];
            let seg_b = (s - r).norm();
            if seg_b <= REGION_EPSILON {
                continue;
            }
            if let Some(hit) = segment_hit(p, q, r, s) {
                let along_a = walked_a + parameter(p, q, hit) * seg_a;
                let along_b = walked_b + parameter(r, s, hit) * seg_b;
                found.push((along_a, along_b, hit));
            }
            walked_b += seg_b;
        }
        walked_a += seg_a;
    }
    // One crossing per point, in arc order along `a`: a ring pair that touches
    // at a vertex reports that vertex from two segments.
    found.sort_by(|x, y| x.0.partial_cmp(&y.0).unwrap_or(std::cmp::Ordering::Equal));
    let mut unique: Vec<(f64, f64, (f64, f64))> = Vec::new();
    for hit in found {
        let duplicate = unique.iter().any(|kept| {
            (kept.2 .0 - hit.2 .0).hypot(kept.2 .1 - hit.2 .1) <= REGION_EPSILON
        });
        if !duplicate {
            unique.push(hit);
        }
    }
    let _ = (total_a, total_b);
    unique
}

/// Where the segment `p→q` meets `r→s`, if the two cross within both segments.
///
/// Collinear overlaps are deliberately *not* crossings: two outlines that run
/// along each other share a seam rather than an intersection point, and a span
/// boundary that is a whole segment has no single point to break at.
fn segment_hit(p: Coord<f64>, q: Coord<f64>, r: Coord<f64>, s: Coord<f64>) -> Option<(f64, f64)> {
    let d1 = q - p;
    let d2 = s - r;
    let denominator = d1.x * d2.y - d1.y * d2.x;
    if denominator.abs() <= 1e-12 {
        return None;
    }
    let offset = r - p;
    let t = (offset.x * d2.y - offset.y * d2.x) / denominator;
    let u = (offset.x * d1.y - offset.y * d1.x) / denominator;
    let tolerance = 1e-9;
    if !(-tolerance..=1.0 + tolerance).contains(&t) || !(-tolerance..=1.0 + tolerance).contains(&u) {
        return None;
    }
    let t = t.clamp(0.0, 1.0);
    Some((p.x + d1.x * t, p.y + d1.y * t))
}

/// How far along `p→q` the point `hit` sits, as a fraction.
fn parameter(p: Coord<f64>, q: Coord<f64>, hit: (f64, f64)) -> f64 {
    let dx = q.x - p.x;
    let dy = q.y - p.y;
    if dx.abs() >= dy.abs() {
        if dx.abs() <= f64::EPSILON {
            0.0
        } else {
            ((hit.0 - p.x) / dx).clamp(0.0, 1.0)
        }
    } else if dy.abs() <= f64::EPSILON {
        0.0
    } else {
        ((hit.1 - p.y) / dy).clamp(0.0, 1.0)
    }
}

fn ring_length(ring: &[Coord<f64>]) -> f64 {
    (0..ring.len())
        .map(|i| {
            let p = ring[i];
            let q = ring[(i + 1) % ring.len()];
            (q - p).norm()
        })
        .sum()
}

/// The point at arc length `distance` along a ring (wrapping).
pub fn ring_point_at(ring: &[Coord<f64>], distance: f64) -> (f64, f64) {
    let total = ring_length(ring);
    if total <= REGION_EPSILON || ring.is_empty() {
        return ring.first().map(|c| (c.x, c.y)).unwrap_or((0.0, 0.0));
    }
    let mut remaining = distance.rem_euclid(total);
    for i in 0..ring.len() {
        let p = ring[i];
        let q = ring[(i + 1) % ring.len()];
        let segment = (q - p).norm();
        if segment <= REGION_EPSILON {
            continue;
        }
        if remaining <= segment {
            let t = remaining / segment;
            return (p.x + (q.x - p.x) * t, p.y + (q.y - p.y) * t);
        }
        remaining -= segment;
    }
    let last = ring[ring.len() - 1];
    (last.x, last.y)
}

/// Every crossing between the sources, in a deterministic order.
pub fn crossings(specs: &[SourceSpec], rings: &[Vec<Vec<Coord<f64>>>]) -> Vec<Crossing> {
    let mut all: Vec<Crossing> = Vec::new();
    for i in 0..specs.len() {
        for j in (i + 1)..specs.len() {
            let (Some(rings_a), Some(rings_b)) = (rings.get(i), rings.get(j)) else {
                continue;
            };
            // Cheap prune: two shapes whose bounding boxes miss each other
            // cannot cross, and this is the common case in a real document.
            if !bounds_overlap(&specs[i].region, &specs[j].region) {
                continue;
            }
            // Arc positions are measured along the *whole outline* (all rings,
            // ring-major order) so a span index is stable whatever ring it is
            // on.
            let mut base_a = 0.0;
            for ring_a in rings_a {
                let mut base_b = 0.0;
                for ring_b in rings_b {
                    for (along_a, along_b, point) in ring_crossings(ring_a, ring_b) {
                        all.push(Crossing {
                            point,
                            a: base_a + along_a,
                            b: base_b + along_b,
                        });
                    }
                    base_b += ring_length(ring_b);
                }
                base_a += ring_length(ring_a);
            }
        }
    }
    all.sort_by(|x, y| {
        x.point
            .0
            .partial_cmp(&y.point.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| {
                x.point
                    .1
                    .partial_cmp(&y.point.1)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .then_with(|| x.a.partial_cmp(&y.a).unwrap_or(std::cmp::Ordering::Equal))
    });
    all.dedup_by(|x, y| {
        (x.point.0 - y.point.0).hypot(x.point.1 - y.point.1) <= REGION_EPSILON
    });
    all
}

fn bounds_overlap(a: &MultiPolygon<f64>, b: &MultiPolygon<f64>) -> bool {
    let (a_min_x, a_min_y, a_max_x, a_max_y) = region_bounds(a);
    let (b_min_x, b_min_y, b_max_x, b_max_y) = region_bounds(b);
    a_min_x <= b_max_x && b_min_x <= a_max_x && a_min_y <= b_max_y && b_min_y <= a_max_y
}

fn region_bounds(region: &MultiPolygon<f64>) -> (f64, f64, f64, f64) {
    let mut bounds: Option<(f64, f64, f64, f64)> = None;
    for polygon in &region.0 {
        for coord in polygon.exterior().0.iter() {
            bounds = Some(match bounds {
                None => (coord.x, coord.y, coord.x, coord.y),
                Some((min_x, min_y, max_x, max_y)) => (
                    min_x.min(coord.x),
                    min_y.min(coord.y),
                    max_x.max(coord.x),
                    max_y.max(coord.y),
                ),
            });
        }
    }
    bounds.unwrap_or((0.0, 0.0, 0.0, 0.0))
}

/// **The spans of one source**: the arcs of its outline between consecutive
/// crossings with any other source.
///
/// A source nobody crosses has **no** spans — there is no intersection point to
/// cut at, and "the whole outline" is not a span a designer can break.
pub fn spans_of(
    source: &SourceSpec,
    rings: &[Vec<Coord<f64>>],
    neighbours: &[Vec<Vec<Coord<f64>>>],
) -> Vec<Span> {
    let mut cuts: Vec<f64> = Vec::new();
    let mut ring_of_cut: Vec<usize> = Vec::new();
    let mut base = 0.0;
    for (index, ring) in rings.iter().enumerate() {
        let ring_len = ring_length(ring);
        for neighbour_rings in neighbours {
            for neighbour in neighbour_rings {
                for (along, _other, _point) in ring_crossings(ring, neighbour) {
                    cuts.push(base + along);
                    ring_of_cut.push(index);
                }
            }
        }
        base += ring_len;
    }
    if cuts.is_empty() {
        return Vec::new();
    }
    // One cut per arc position, in outline order.
    let mut ordered: Vec<(f64, usize)> = cuts
        .into_iter()
        .zip(ring_of_cut)
        .collect();
    ordered.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    ordered.dedup_by(|a, b| (a.0 - b.0).abs() <= REGION_EPSILON);

    let total: f64 = rings.iter().map(|ring| ring_length(ring)).sum();
    let mut spans: Vec<Span> = Vec::new();
    for index in 0..ordered.len() {
        let (from, ring_index) = ordered[index];
        // The next cut along the *outline*; the last span wraps to the first,
        // because an outline is a cycle.
        let (to, next_ring) = match ordered.get(index + 1) {
            Some(next) => *next,
            None => (ordered[0].0 + total, ordered[0].1),
        };
        if to - from <= REGION_EPSILON {
            continue;
        }
        if next_ring != ring_index {
            // The cut that closes the outline is on another ring: the outline is
            // ring-major, so the wrap arc runs to the end of this ring. Keep it
            // simple and honest — the span is the tail of this ring.
            let ring_end: f64 = {
                let mut end = 0.0;
                for (i, ring) in rings.iter().enumerate() {
                    end += ring_length(ring);
                    if i == ring_index {
                        break;
                    }
                }
                end
            };
            if ring_end - from <= REGION_EPSILON {
                continue;
            }
            spans.push(span_at(source.id, rings, from, ring_end, ring_index, total));
            continue;
        }
        spans.push(span_at(source.id, rings, from, to, ring_index, total));
    }
    spans
}

fn span_at(
    id: NodeId,
    rings: &[Vec<Coord<f64>>],
    from: f64,
    to: f64,
    ring: usize,
    total: f64,
) -> Span {
    let start = point_on_rings(rings, from);
    let end = point_on_rings(rings, to);
    Span {
        source: id,
        from,
        to,
        start,
        end,
        length: to - from,
        ring,
        total,
    }
}

/// **Cut a ring at two arc positions** (Task 12.0 RULE 3): the two arcs that
/// together are the ring.
///
/// Each piece is returned as the point list of a *closed* path: the first point
/// is the cut itself and the closing segment (`PathSegment::Close`, in the
/// command this feeds) joins the last point back to the first. The two pieces
/// share the two cut points and partition the ring's arc length, so
/// `piece_a ∪ piece_b == ring` with no gap and no overlap — the whole of what
/// "break a span" has to mean.
///
/// `from` and `to` are arc lengths *within the ring* (see [`Span::from`] for the
/// outline-relative positions the graph reports); `to` may wrap past the ring's
/// length, which is the span that crosses the ring's start point.
pub fn ring_pieces_between(
    ring: &[Coord<f64>],
    from: f64,
    to: f64,
) -> (Vec<(f64, f64)>, Vec<(f64, f64)>) {
    let forward = walk_ring(ring, from, to);
    let back = walk_ring(ring, to, from);
    (forward, back)
}

/// The points along `ring` from arc length `from` to arc length `to`, walking
/// forward and collecting the ring vertices in between.
fn walk_ring(ring: &[Coord<f64>], from: f64, to: f64) -> Vec<(f64, f64)> {
    let total = ring_length(ring);
    if total <= REGION_EPSILON || ring.is_empty() {
        return Vec::new();
    }
    let span = (to - from).rem_euclid(total);
    if span <= REGION_EPSILON {
        return Vec::new();
    }
    let mut points = vec![ring_point_at(ring, from)];
    let mut walked = 0.0;
    for index in 0..ring.len() {
        let p = ring[index];
        let q = ring[(index + 1) % ring.len()];
        let segment = (q - p).norm();
        if segment <= REGION_EPSILON {
            continue;
        }
        walked += segment;
        // The vertex at the end of this segment is *inside* the arc when its
        // forward distance from `from` is strictly between the two cuts.
        let relative = (walked - from).rem_euclid(total);
        if relative > REGION_EPSILON && relative < span - REGION_EPSILON {
            points.push((q.x, q.y));
        }
    }
    points.push(ring_point_at(ring, to));
    points
}

/// The point at arc length `distance` along the outline (ring-major order).
pub fn point_on_rings(rings: &[Vec<Coord<f64>>], distance: f64) -> (f64, f64) {
    let mut remaining = distance;
    for ring in rings {
        let length = ring_length(ring);
        if remaining <= length {
            return ring_point_at(ring, remaining);
        }
        remaining -= length;
    }
    rings
        .last()
        .and_then(|ring| ring.first())
        .map(|c| (c.x, c.y))
        .unwrap_or((0.0, 0.0))
}

// ── the graph ───────────────────────────────────────────────────────────────

/// A built region graph: the sources, their faces, the crossings between them,
/// and each source's spans.
#[derive(Debug, Clone, PartialEq)]
pub struct RegionGraph {
    pub sources: Vec<SourceSpec>,
    pub faces: Vec<RegionFace>,
    pub crossings: Vec<Crossing>,
    pub spans: Vec<Span>,
    /// How many sources have at least one crossing (a source nobody crosses has
    /// no spans to break) — the UI's "did these shapes meet?" answer.
    pub intersecting_sources: usize,
}

impl RegionGraph {
    /// Build the graph over `sources`, in the order given. The order is the
    /// contract: face signatures and the UI's hover answer are both stated in
    /// terms of it.
    pub fn build(sources: Vec<SourceSpec>) -> Self {
        let raw = atoms(&sources);
        let faces: Vec<RegionFace> = raw
            .into_iter()
            .map(|(region, members)| {
                let area = area_of(&region);
                let holes = region.0.iter().map(|p| p.interiors().len()).sum();
                let path = multi_polygon_to_path(&region);
                RegionFace {
                    region,
                    members,
                    area,
                    holes,
                    path,
                }
            })
            .collect();

        let rings: Vec<Vec<Vec<Coord<f64>>>> = sources
            .iter()
            .map(|source| {
                let path = multi_polygon_to_path(&source.region);
                path_rings(&path)
            })
            .collect();
        let cross = crossings(&sources, &rings);
        let mut spans: Vec<Span> = Vec::new();
        let mut intersecting_sources = 0usize;
        for (index, source) in sources.iter().enumerate() {
            let neighbours: Vec<Vec<Vec<Coord<f64>>>> = sources
                .iter()
                .enumerate()
                .filter(|(other, _)| *other != index)
                .map(|(other, _)| rings[other].clone())
                .collect();
            let own = spans_of(source, &rings[index], &neighbours);
            if !own.is_empty() {
                intersecting_sources += 1;
            }
            spans.extend(own);
        }
        Self {
            sources,
            faces,
            crossings: cross,
            spans,
            intersecting_sources,
        }
    }

    /// Build from the names of scene nodes, skipping what has no area.
    pub fn from_scene(scene: &EvaluatedScene, ids: &[NodeId]) -> Self {
        Self::build(sources_of(scene, ids))
    }

    /// The face under a document point, or `None` for empty space.
    pub fn face_at(&self, point: (f64, f64)) -> Option<usize> {
        face_containing(&self.faces, point)
    }

    /// The spans belonging to one source, in outline order.
    pub fn spans_of_source(&self, id: NodeId) -> Vec<&Span> {
        self.spans.iter().filter(|span| span.source == id).collect()
    }

    /// The area of the region whose signature is `members` — the number a
    /// Smart Fill's panel reports, and the quantity its law is stated in.
    pub fn area_of_signature(&self, members: &[bool]) -> f64 {
        self.faces
            .iter()
            .filter(|face| face.members == members)
            .map(|face| face.area)
            .sum()
    }
}

/// The bounds of a source's region — used by the UI to frame a fill.
pub fn source_bounds(source: &SourceSpec) -> (f64, f64, f64, f64) {
    let path = multi_polygon_to_path(&source.region);
    path_bounds(&path)
}

/// Close a ring into a `geo::Polygon` (the one place a ring becomes a polygon,
/// so a "closed path with no gaps" is closed the same way everywhere).
pub fn ring_polygon(ring: &[Coord<f64>]) -> Option<Polygon<f64>> {
    if ring.len() < 3 {
        return None;
    }
    let mut points = ring.to_vec();
    if points[0] != points[points.len() - 1] {
        points.push(points[0]);
    }
    if points.len() < 4 {
        return None;
    }
    Some(Polygon::new(LineString::new(points), Vec::new()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::primitive_to_path;
    use crate::scene::EvaluatedPrimitive;

    fn circle(cx: f64, cy: f64, r: f64) -> SourceSpec {
        SourceSpec {
            id: vectra_core::new_node_id(),
            region: crate::convert::path_to_multi_polygon(&primitive_to_path(
                &EvaluatedPrimitive::Circle { cx, cy, r },
            ))
            .expect("a circle is a region"),
        }
    }

    fn rect(x: f64, y: f64, w: f64, h: f64) -> SourceSpec {
        SourceSpec {
            id: vectra_core::new_node_id(),
            region: crate::convert::path_to_multi_polygon(&primitive_to_path(
                &EvaluatedPrimitive::Rect {
                    x,
                    y,
                    w,
                    h,
                    corner_radius: 0.0,
                },
            ))
            .expect("a rect is a region"),
        }
    }

    fn circle_area(r: f64) -> f64 {
        // The flattened circle's own area — what the region graph sees, not the
        // analytic πr² (a 64-segment polygon is 0.16% smaller).
        crate::convert::path_area(&primitive_to_path(&EvaluatedPrimitive::Circle {
            cx: 0.0,
            cy: 0.0,
            r,
        }))
    }

    /// **The Region Detection Law** (Task 12.0): two overlapping circles
    /// produce **exactly three** distinct regions — `A \ B`, `A ∩ B`, `B \ A` —
    /// with one atom per signature, in signature order, and the middle one
    /// inside both.
    #[test]
    fn two_overlapping_circles_produce_exactly_three_regions() {
        let a = circle(0.0, 0.0, 100.0);
        let b = circle(120.0, 0.0, 100.0);
        let graph = RegionGraph::build(vec![a.clone(), b.clone()]);

        assert_eq!(graph.faces.len(), 3, "three faces: A only, A∩B, B only");
        let signatures: Vec<Vec<bool>> = graph.faces.iter().map(|f| f.members.clone()).collect();
        assert_eq!(
            signatures,
            vec![vec![true, false], vec![true, true], vec![false, true]],
            "one atom per signature, in signature order"
        );

        // The middle face is the overlap, and it is *inside both* — the one
        // that has to be exactly the intersection.
        let overlap = graph
            .faces
            .iter()
            .find(|face| face.members == [true, true])
            .expect("the overlap exists");
        let lens = graph
            .crossings
            .iter()
            .filter(|crossing| {
                crossing.a.is_finite() && crossing.b.is_finite()
            })
            .count();
        assert_eq!(
            lens,
            2,
            "two circles cross at two points: {:?}",
            graph.crossings
        );
        // …and the two crossings are the *lens* corners, one on each circle:
        // x = d/2 for equal radii, y = ±sqrt(r² − (d/2)²).
        let mut ys: Vec<f64> = graph.crossings.iter().map(|c| c.point.1).collect();
        ys.sort_by(f64::total_cmp);
        let expected_y = (100.0f64 * 100.0 - 60.0 * 60.0).sqrt();
        assert!((ys[0] + expected_y).abs() < 0.2, "{ys:?}");
        assert!((ys[1] - expected_y).abs() < 0.2, "{ys:?}");

        // **The areas add up.** A ∪ B is the sum of its disjoint faces, and the
        // union is what `geo` says it is — so the graph neither loses nor
        // double-counts a piece of the plane.
        let union_area = crate::convert::region_area(&a.region.union(&b.region));
        let face_total: f64 = graph.faces.iter().map(|face| face.area).sum();
        assert!(
            (face_total - union_area).abs() < 1e-6,
            "faces {face_total} vs union {union_area}"
        );
        // A\B plus the overlap is A.
        let a_area = crate::convert::region_area(&a.region);
        let a_only = graph
            .faces
            .iter()
            .find(|face| face.members == [true, false])
            .expect("A alone");
        assert!((a_only.area + overlap.area - a_area).abs() < 1e-6);

        // And the "drop test" answers by containment: a point in the lens is the
        // overlap, a point in A's crescent is A alone, the origin of neither…
        let lens_point = (60.0, 0.0);
        let index = graph.face_at(lens_point).expect("a face under the lens");
        assert_eq!(graph.faces[index].members, [true, true]);
        let crescent = (10.0, 0.0);
        let index = graph.face_at(crescent).expect("a face under A's crescent");
        assert_eq!(graph.faces[index].members, [true, false]);
        assert!(graph.face_at((1000.0, 1000.0)).is_none(), "empty space has no face");
    }

    /// A **hole stays a hole**: a donut and a bar through it produce regions
    /// whose middles are holes, and every face's path is closed with the hole
    /// wound the other way — fillable under both fill rules.
    #[test]
    fn holes_remain_holes_in_every_face() {
        // A 100×100 square, and a 40×40 square punched out of the middle, as
        // *one* source with two rings (parity: the inner ring is a hole).
        let mut builder = lyon::path::Builder::new();
        let _ = builder.begin(lyon::math::point(0.0, 0.0));
        builder.line_to(lyon::math::point(100.0, 0.0));
        builder.line_to(lyon::math::point(100.0, 100.0));
        builder.line_to(lyon::math::point(0.0, 100.0));
        builder.close();
        let _ = builder.begin(lyon::math::point(30.0, 30.0));
        builder.line_to(lyon::math::point(30.0, 70.0));
        builder.line_to(lyon::math::point(70.0, 70.0));
        builder.line_to(lyon::math::point(70.0, 30.0));
        builder.close();
        let donut = SourceSpec {
            id: vectra_core::new_node_id(),
            region: crate::convert::path_to_multi_polygon(&builder.build()).expect("a donut"),
        };
        // A bar through the hole, which therefore has no crossings with the
        // donut at all.
        let bar = rect(20.0, 45.0, 60.0, 10.0);
        let graph = RegionGraph::build(vec![donut.clone(), bar.clone()]);

        assert_eq!(graph.faces.len(), 3, "donut, bar, bar∩donut");
        let donut_face = graph
            .faces
            .iter()
            .find(|face| face.members == [true, false])
            .expect("the donut");
        assert_eq!(donut_face.holes, 1, "the hole survived the arrangement");
        assert!((donut_face.area - (100.0 * 100.0 - 40.0 * 40.0)).abs() < 1e-6);

        // The face's path is closed, and its rings are wound opposite ways: the
        // signature of "a hole" under both `nonzero` and `even-odd`.
        let rings = path_rings(&donut_face.path);
        assert_eq!(rings.len(), 2);
        let signed = |ring: &[Coord<f64>]| {
            let mut twice = 0.0;
            for i in 0..ring.len() {
                let p = ring[i];
                let q = ring[(i + 1) % ring.len()];
                twice += p.x * q.y - q.x * p.y;
            }
            twice / 2.0
        };
        assert!(
            signed(&rings[0]) * signed(&rings[1]) < 0.0,
            "exterior {:?} and hole {:?} must wind opposite ways",
            signed(&rings[0]),
            signed(&rings[1])
        );
        // The bar inside the hole is its own face, and the overlap with the
        // donut's *material* is two slivers — the bar crosses the donut's frame.
        let bar_only = graph
            .faces
            .iter()
            .find(|face| face.members == [false, true])
            .expect("the bar");
        let bar_area = crate::convert::region_area(&bar.region);
        assert!(bar_only.area < bar_area, "part of the bar is inside the donut");
    }

    /// **Spans**: crossing outlines are cut at their intersections. Two
    /// overlapping circles give each circle exactly two crossings and therefore
    /// two spans, and the spans' arc lengths add up to the outline.
    #[test]
    fn crossings_cut_each_outline_into_spans() {
        let a = circle(0.0, 0.0, 100.0);
        let b = circle(120.0, 0.0, 100.0);
        let graph = RegionGraph::build(vec![a.clone(), b.clone()]);

        for source in [&a, &b] {
            let spans = graph.spans_of_source(source.id);
            assert_eq!(spans.len(), 2, "two crossings ⇒ two arcs");
            let covered: f64 = spans.iter().map(|span| span.length).sum();
            let total = spans[0].total;
            assert!(
                (covered - total).abs() < 1e-6,
                "the spans must cover the outline exactly once: {covered} vs {total}"
            );
            for span in &spans {
                // Every span begins and ends on a crossing, not in mid-air.
                for endpoint in [span.start, span.end] {
                    assert!(
                        graph.crossings.iter().any(|crossing| {
                            (crossing.point.0 - endpoint.0).hypot(crossing.point.1 - endpoint.1)
                                <= 1e-6
                        }),
                        "span endpoint {endpoint:?} is not a crossing"
                    );
                }
                // …and its endpoints really are where the outline is at those
                // arc lengths.
                let rings = path_rings(&crate::convert::multi_polygon_to_path(&source.region));
                let start = point_on_rings(&rings, span.from);
                assert!(
                    (start.0 - span.start.0).hypot(start.1 - span.start.1) < 1e-6,
                    "span start {start:?} vs {:?}",
                    span.start
                );
            }
        }
        // A shape nobody crosses has no spans at all.
        let alone = circle(500.0, 0.0, 10.0);
        assert!(graph.spans_of_source(alone.id).is_empty());
    }

    /// **The Span Break Law's geometry half**: cutting a ring at the two
    /// crossings that bound one of its spans produces two closed paths whose
    /// arc lengths add up to the ring's, whose endpoints are the cut points,
    /// and whose union is the ring — no gap, no overlap. Cutting at the *same*
    /// pair the other way round gives the complementary pair.
    #[test]
    fn breaking_a_span_gives_two_closed_pieces_with_no_gap() {
        let a = circle(0.0, 0.0, 100.0);
        let b = circle(120.0, 0.0, 100.0);
        let graph = RegionGraph::build(vec![a.clone(), b.clone()]);
        let spans = graph.spans_of_source(a.id);
        assert_eq!(spans.len(), 2);
        let rings = path_rings(&crate::convert::multi_polygon_to_path(&a.region));
        let span = spans[0];

        let (piece_a, piece_b) = ring_pieces_between(&rings[span.ring], span.from, span.to);
        assert!(piece_a.len() >= 2 && piece_b.len() >= 2, "both pieces draw");

        // 1. **The endpoints are the cut points** — the intersection points the
        //    span was measured between.
        for (piece, first, last) in [
            (&piece_a, span.start, span.end),
            (&piece_b, span.end, span.start),
        ] {
            let head = piece[0];
            let tail = *piece.last().expect("non-empty");
            assert!(
                (head.0 - first.0).hypot(head.1 - first.1) < 1e-6,
                "piece starts at the cut point"
            );
            assert!(
                (tail.0 - last.0).hypot(tail.1 - last.1) < 1e-6,
                "piece ends at the other cut point"
            );
        }

        // 2. **No gap**: the pieces' arc lengths add up to the whole ring…
        let arc_length = |piece: &[(f64, f64)]| -> f64 {
            let mut length = 0.0;
            for index in 0..piece.len() {
                let p = piece[index];
                let q = piece[(index + 1) % piece.len()];
                length += (q.0 - p.0).hypot(q.1 - p.1);
            }
            length
        };
        let ring_length_total = ring_length(&rings[span.ring]);
        let combined = arc_length(&piece_a) + arc_length(&piece_b);
        assert!(
            (combined - ring_length_total).abs() < 1e-6,
            "pieces {combined} vs ring {ring_length_total}"
        );
        assert!(
            (arc_length(&piece_a) - span.length).abs() < 1e-6,
            "the first piece *is* the span: {} vs {}",
            arc_length(&piece_a),
            span.length
        );

        // 3. **They are closed regions**: the area the two pieces enclose adds
        //    up to the circle's (a polygon split by a chord, exactly).
        let ring_polygon_area = |piece: &[(f64, f64)]| -> f64 {
            let mut twice = 0.0;
            for index in 0..piece.len() {
                let p = piece[index];
                let q = piece[(index + 1) % piece.len()];
                twice += p.0 * q.1 - q.0 * p.1;
            }
            twice.abs() / 2.0
        };
        let circle_path = crate::convert::multi_polygon_to_path(&a.region);
        assert!(
            (ring_polygon_area(&piece_a) + ring_polygon_area(&piece_b)
                - crate::convert::path_area(&circle_path))
            .abs()
                < 1e-6,
            "the two pieces tile the ring"
        );

        // 4. Cutting the *complementary* pair of cut points gives the other
        //    two pieces — the same ring, split the other way.
        let other = ring_pieces_between(&rings[span.ring], span.to, span.from);
        assert!(
            (arc_length(&other.0) - (ring_length_total - span.length)).abs() < 1e-6,
            "the complement is the rest of the ring"
        );
    }

    /// A source **inside** another source (no crossings, one face strictly
    /// inside) still gives two faces with distinct signatures, and the outer
    /// face is *nested*: the drop test answers with the inner one.
    #[test]
    fn an_uncrossed_contained_source_is_its_own_face() {
        let outer = rect(0.0, 0.0, 100.0, 100.0);
        let inner = rect(25.0, 25.0, 50.0, 50.0);
        let graph = RegionGraph::build(vec![outer.clone(), inner.clone()]);
        assert_eq!(graph.faces.len(), 2);
        let inner_face = graph
            .faces
            .iter()
            .find(|face| face.members == [true, true])
            .expect("the inner square");
        assert!((inner_face.area - 2500.0).abs() < 1e-9);
        let outer_face = graph
            .faces
            .iter()
            .find(|face| face.members == [true, false])
            .expect("the frame");
        assert_eq!(outer_face.holes, 1, "the inner square is a hole in the frame");
        assert!((outer_face.area - 7500.0).abs() < 1e-9);
        // The centre belongs to the inner square, not the frame.
        let index = graph.face_at((50.0, 50.0)).expect("a face");
        assert_eq!(graph.faces[index].members, [true, true]);
        assert!(graph.crossings.is_empty(), "nothing crosses");
        assert_eq!(graph.intersecting_sources, 0);
    }
}
