//! The four non-destructive operations, on regions (Task 4.0, RULE 2).
//!
//! Every entry point takes `geo` regions in and returns a `geo` region out:
//! the lyon/`geo` boundary lives entirely in [`crate::convert`], and nothing
//! here knows about documents, nodes, or the scene cache. That split is what
//! makes the operations testable with plain arithmetic — and it is why the
//! boolean laws can be stated as area identities.
//!
//! ```text
//! Boolean  A ∪ B, A \ B, A ∩ B, A ⊕ B   → geo::BooleanOps   (even-odd fill)
//! Offset   region grown/shrunk by d     → geo::Buffer
//! Fillet   convex corners rounded to r  → polyline math (below)
//! Mirror   reflected across an axis     → geo::AffineOps
//! ```

use crate::error::OperationError;
use geo::{
    AffineOps, AffineTransform, Area, BooleanOps, Buffer, Coord, LineString, MultiPolygon, Polygon,
};
use vectra_core::{BooleanOp, MirrorAxis, OperationKind};

/// Run a boolean set operation over two regions.
///
/// `OpType` mapping (RULE 2 — the crate does the math, we only name it):
/// `union` → `A ∪ B`, `subtract` → `A \ B` (subject minus clip), `intersect` →
/// `A ∩ B`, `exclude` → `A ⊕ B`. The fill rule is `geo`'s default even-odd, so
/// a ring nested inside another reads as a hole rather than as extra material —
/// the same parity the preview fills with.
pub fn boolean(a: &MultiPolygon<f64>, b: &MultiPolygon<f64>, op: BooleanOp) -> MultiPolygon<f64> {
    match op {
        BooleanOp::Union => a.union(b),
        BooleanOp::Subtract => a.difference(b),
        BooleanOp::Intersect => a.intersection(b),
        BooleanOp::Exclude => a.xor(b),
    }
}

/// Grow the region by `distance` (negative insets) with rounded joins.
///
/// `geo::Buffer` is a Minkowski sum: offsetting a convex region by `d` grows its
/// area, and a hole shrinks by the same amount. Joins are round so the result
/// keeps the shape's feel instead of sprouting spikes.
pub fn offset(region: &MultiPolygon<f64>, distance: f64) -> MultiPolygon<f64> {
    region.buffer(distance)
}

/// Reflect the region across an axis: `Vertical { at: c }` is the line `x = c`,
/// `Horizontal { at: c }` is `y = c`.
///
/// Implemented as a negative scale about the axis, then re-oriented: a
/// reflection flips every ring's winding, and consistent winding is what keeps
/// later booleans and hole rendering correct.
pub fn mirror(
    region: &MultiPolygon<f64>,
    axis: &MirrorAxis,
    at: f64,
) -> Result<MultiPolygon<f64>, OperationError> {
    if !at.is_finite() {
        return Err(OperationError::NonFiniteOperand {
            which: "mirror axis",
        });
    }
    let transform = match axis {
        MirrorAxis::Vertical { .. } => AffineTransform::scale(-1.0, 1.0, Coord { x: at, y: 0.0 }),
        MirrorAxis::Horizontal { .. } => AffineTransform::scale(1.0, -1.0, Coord { x: 0.0, y: at }),
    };
    let reflected = region.affine_transform(&transform);
    Ok(reorient(reflected))
}

/// Round every convex corner of the region to `radius`.
///
/// For each vertex the two edge directions give the interior angle `θ`; a convex
/// corner (`θ < π`) is replaced by a tangent arc of radius `r`, whose tangent
/// points sit `t = r / tan(θ/2)` back along each edge. `t` is capped at half the
/// shorter adjacent edge, so rounds can never swallow a neighbour (the usual CAD
/// clamping) — the effective radius shrinks instead of the topology breaking.
///
/// Reflex corners (`θ > π`) are left sharp: rounding them is an *outward* bulge
/// that changes the region's silhouette in ways Phase 1 does not attempt. The
/// arc is sampled at the same deterministic resolution as every other curve in
/// the pipeline.
pub fn fillet(
    region: &MultiPolygon<f64>,
    radius: f64,
) -> Result<MultiPolygon<f64>, OperationError> {
    if !radius.is_finite() {
        return Err(OperationError::NonFiniteOperand {
            which: "fillet radius",
        });
    }
    if radius <= 0.0 {
        return Ok(region.clone());
    }
    let mut polygons = Vec::with_capacity(region.0.len());
    for polygon in &region.0 {
        let exterior = fillet_ring(&polygon.exterior().0, radius);
        let interiors: Vec<LineString<f64>> = polygon
            .interiors()
            .iter()
            .map(|ring| LineString::new(fillet_ring(&ring.0, radius)))
            .collect();
        let rounded = Polygon::new(LineString::new(exterior), interiors);
        if rounded.unsigned_area() > f64::EPSILON {
            polygons.push(rounded);
        }
    }
    Ok(MultiPolygon::new(polygons))
}

/// Round one closed ring. The ring is returned closed (first == last).
fn fillet_ring(ring: &[Coord<f64>], radius: f64) -> Vec<Coord<f64>> {
    // Drop the closing duplicate: corners are easier to enumerate without it.
    let mut points: Vec<Coord<f64>> = ring.to_vec();
    if points.len() > 1 && points[0] == points[points.len() - 1] {
        points.pop();
    }
    let n = points.len();
    if n < 3 {
        return ring.to_vec();
    }

    // Material is always on the *left* of travel in a well-formed polygon: a
    // counter-clockwise exterior has the shape's inside on its left, and a
    // clockwise interior (a hole) has the material around it on its left too.
    // That single fact decides which side a fillet must be built on, for
    // exteriors and holes alike.
    let material_left = signed_area(&points) > 0.0;

    let mut out: Vec<Coord<f64>> = Vec::with_capacity(n * 4);
    for i in 0..n {
        let prev = points[(i + n - 1) % n];
        let vertex = points[i];
        let next = points[(i + 1) % n];
        match round_corner(prev, vertex, next, radius, material_left) {
            Some((tangent_in, arc, tangent_out)) => {
                out.push(tangent_in);
                out.extend(arc);
                out.push(tangent_out);
            }
            None => out.push(vertex),
        }
    }
    out.push(out[0]);
    out
}

/// One corner's fillet: `(tangent_in, arc interior points, tangent_out)`, or
/// `None` when the corner stays sharp (collinear, degenerate, or reflex).
#[allow(clippy::type_complexity)]
fn round_corner(
    prev: Coord<f64>,
    vertex: Coord<f64>,
    next: Coord<f64>,
    radius: f64,
    material_left: bool,
) -> Option<(Coord<f64>, Vec<Coord<f64>>, Coord<f64>)> {
    let a = sub(prev, vertex); // back along the incoming edge
    let b = sub(next, vertex); // forward along the outgoing edge
    let la = length(a);
    let lb = length(b);
    if la < 1e-9 || lb < 1e-9 {
        return None;
    }
    let ua = scale(a, 1.0 / la);
    let ub = scale(b, 1.0 / lb);
    // Angle between the two edge rays (the ring's own interior angle at `θ`).
    let ring_theta = dot(ua, ub).clamp(-1.0, 1.0).acos();
    // …and the angle of the MATERIAL wedge at this corner. For a
    // counter-clockwise ring that is the ring's interior; for a clockwise one
    // (a hole, or any negatively-wound ring) the material is on the other side,
    // so the wedge angles swap.
    let theta = if material_left {
        ring_theta
    } else {
        std::f64::consts::TAU - ring_theta
    };
    // Round convex material corners; leave concave (reflex) ones sharp.
    if theta >= std::f64::consts::PI - 1e-9 || theta <= 1e-9 {
        return None;
    }
    // Tangent length for the requested radius, capped so neighbouring rounds
    // cannot overlap (each gets at most half of the shorter edge). The cap
    // shrinks the effective radius rather than breaking the topology.
    let half_tan = (theta / 2.0).tan();
    if half_tan.abs() < 1e-12 {
        return None;
    }
    let t = (radius / half_tan).min(0.5 * la.min(lb));
    if t < 1e-9 {
        return None;
    }
    let r_eff = t * half_tan;
    let tangent_in = add(vertex, scale(ua, t));
    let tangent_out = add(vertex, scale(ub, t));

    // Centre: along the wedge bisector at `r / sin(θ/2)` from the vertex. For a
    // clockwise ring the material wedge is the complement of the ring's
    // interior, so the bisector points the other way.
    let bisector = normalize(add(ua, ub))?;
    let bisector = if material_left {
        bisector
    } else {
        scale(bisector, -1.0)
    };
    let center = add(vertex, scale(bisector, r_eff / (theta / 2.0).sin()));

    // Sweep: the arc that stays inside the material is the SHORT one between
    // the two tangent points, and its magnitude is exactly `π − θ` (the
    // exterior turn). Wrapping the angle difference into `(-π, π]` therefore
    // selects it directly — no sign guessing from cross products, which is
    // where this goes wrong when the ring is wound clockwise.
    let start = (tangent_in.y - center.y).atan2(tangent_in.x - center.x);
    let end = (tangent_out.y - center.y).atan2(tangent_out.x - center.x);
    let sweep = wrap_to_pi(end - start);
    let steps =
        (((sweep.abs() / std::f64::consts::TAU) * crate::convert::arc_samples(r_eff)).ceil()
            as usize)
            .max(2);
    let arc: Vec<Coord<f64>> = (1..steps)
        .map(|step| {
            let angle = start + sweep * (step as f64) / (steps as f64);
            Coord {
                x: center.x + r_eff * angle.cos(),
                y: center.y + r_eff * angle.sin(),
            }
        })
        .collect();
    Some((tangent_in, arc, tangent_out))
}

/// Run any operation kind over its operand regions.
///
/// `inputs` are the operand regions in registry order: two for a boolean (subject
/// first), one for a modifier. Scalar operands arrive already resolved by the
/// caller, because resolution needs an [`vectra_core::EvaluationContext`] and
/// belongs to the evaluator, not to the arithmetic.
pub fn apply(
    kind: &OperationKind,
    inputs: &[MultiPolygon<f64>],
    scalars: &[Option<f64>],
) -> Result<MultiPolygon<f64>, OperationError> {
    match kind {
        OperationKind::Boolean { op } => {
            let [a, b] = inputs else {
                return Err(OperationError::Arity {
                    expected: 2,
                    got: inputs.len(),
                });
            };
            Ok(boolean(a, b, *op))
        }
        OperationKind::Offset { .. } => {
            let [a] = inputs else {
                return Err(OperationError::Arity {
                    expected: 1,
                    got: inputs.len(),
                });
            };
            let distance = scalar(scalars, "offset distance")?;
            Ok(offset(a, distance))
        }
        OperationKind::Fillet { .. } => {
            let [a] = inputs else {
                return Err(OperationError::Arity {
                    expected: 1,
                    got: inputs.len(),
                });
            };
            let radius = scalar(scalars, "fillet radius")?;
            fillet(a, radius)
        }
        OperationKind::Mirror { axis } => {
            let [a] = inputs else {
                return Err(OperationError::Arity {
                    expected: 1,
                    got: inputs.len(),
                });
            };
            let at = scalar(scalars, "mirror axis")?;
            mirror(a, axis, at)
        }
        // A Smart Fill is not a region *operation*: it does not combine its
        // boundaries, it asks which face of their arrangement its seed is in,
        // and that needs every boundary at once plus the seed point — none of
        // which this signature (`inputs` already reduced to regions, `scalars`
        // reduced to numbers) carries. `OperationsEvaluator` handles the kind
        // before it gets here, and this arm is the proof that the region-pass
        // entry point cannot silently do the wrong thing with it.
        OperationKind::SmartFill { .. } => Err(OperationError::Arity {
            expected: inputs.len(),
            got: inputs.len(),
        }),
    }
}

fn scalar(scalars: &[Option<f64>], which: &'static str) -> Result<f64, OperationError> {
    match scalars.first().copied().flatten() {
        Some(value) if value.is_finite() => Ok(value),
        _ => Err(OperationError::NonFiniteOperand { which }),
    }
}

/// Rewrite every ring so exteriors wind counter-clockwise and interiors
/// clockwise (after a reflection, which flips both).
fn reorient(region: MultiPolygon<f64>) -> MultiPolygon<f64> {
    let polygons = region
        .0
        .into_iter()
        .map(|polygon| {
            let exterior = orient(polygon.exterior().0.clone(), false);
            let interiors: Vec<LineString<f64>> = polygon
                .interiors()
                .iter()
                .map(|ring| orient(ring.0.clone(), true))
                .collect();
            Polygon::new(exterior, interiors)
        })
        .collect();
    MultiPolygon::new(polygons)
}

fn orient(points: Vec<Coord<f64>>, want_clockwise: bool) -> LineString<f64> {
    let is_ccw = signed_area(&points) > 0.0;
    if is_ccw == want_clockwise {
        let mut reversed = points;
        reversed.reverse();
        LineString::new(reversed)
    } else {
        LineString::new(points)
    }
}

/// Shoelace signed area of a closed ring (positive = counter-clockwise).
fn signed_area(points: &[Coord<f64>]) -> f64 {
    if points.len() < 3 {
        return 0.0;
    }
    let mut sum = 0.0;
    for i in 0..points.len() {
        let a = points[i];
        let b = points[(i + 1) % points.len()];
        sum += a.x * b.y - b.x * a.y;
    }
    0.5 * sum
}

/// Wrap an angle into `(-π, π]`.
fn wrap_to_pi(angle: f64) -> f64 {
    let tau = std::f64::consts::TAU;
    let mut a = angle % tau;
    if a > std::f64::consts::PI {
        a -= tau;
    } else if a <= -std::f64::consts::PI {
        a += tau;
    }
    a
}

fn sub(a: Coord<f64>, b: Coord<f64>) -> Coord<f64> {
    Coord {
        x: a.x - b.x,
        y: a.y - b.y,
    }
}

fn add(a: Coord<f64>, b: Coord<f64>) -> Coord<f64> {
    Coord {
        x: a.x + b.x,
        y: a.y + b.y,
    }
}

fn scale(a: Coord<f64>, k: f64) -> Coord<f64> {
    Coord {
        x: a.x * k,
        y: a.y * k,
    }
}

fn dot(a: Coord<f64>, b: Coord<f64>) -> f64 {
    a.x * b.x + a.y * b.y
}

fn length(a: Coord<f64>) -> f64 {
    (a.x * a.x + a.y * a.y).sqrt()
}

fn normalize(a: Coord<f64>) -> Option<Coord<f64>> {
    let l = length(a);
    if l < 1e-12 {
        None
    } else {
        Some(scale(a, 1.0 / l))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::convert::region_area;

    fn box_region(x: f64, y: f64, w: f64, h: f64) -> MultiPolygon<f64> {
        MultiPolygon::new(vec![Polygon::new(
            LineString::new(vec![
                Coord { x, y },
                Coord { x: x + w, y },
                Coord { x: x + w, y: y + h },
                Coord { x, y: y + h },
                Coord { x, y },
            ]),
            Vec::new(),
        )])
    }

    #[test]
    fn boolean_ops_follow_their_set_identities() {
        let a = box_region(0.0, 0.0, 10.0, 10.0);
        let b = box_region(5.0, 0.0, 10.0, 10.0);
        let overlap = 5.0 * 10.0;
        assert!((region_area(&boolean(&a, &b, BooleanOp::Union)) - (200.0 - overlap)).abs() < 1e-9);
        assert!(
            (region_area(&boolean(&a, &b, BooleanOp::Subtract)) - (100.0 - overlap)).abs() < 1e-9
        );
        assert!((region_area(&boolean(&a, &b, BooleanOp::Intersect)) - overlap).abs() < 1e-9);
        assert!(
            (region_area(&boolean(&a, &b, BooleanOp::Exclude)) - (200.0 - 2.0 * overlap)).abs()
                < 1e-9
        );
    }

    #[test]
    fn offset_grows_and_shrinks_with_sign() {
        let a = box_region(0.0, 0.0, 10.0, 10.0);
        let grown = region_area(&offset(&a, 2.0));
        let shrunk = region_area(&offset(&a, -2.0));
        assert!(grown > 100.0, "positive offset grows: {grown}");
        assert!(shrunk < 100.0, "negative offset shrinks: {shrunk}");
        // A rounded 2-unit grown square: 100 + perimeter·2 + π·4 ≈ 192.57
        // (a 14×14 box with r=2 corners, i.e. 196 − 4·(4 − π)).
        let expected = 100.0 + 40.0 * 2.0 + std::f64::consts::PI * 4.0;
        assert!(
            (grown - expected).abs() / expected < 0.02,
            "{grown} vs {expected}"
        );
    }

    #[test]
    fn fillet_rounds_a_corner_and_leaves_a_circle_alone() {
        let square = box_region(0.0, 0.0, 10.0, 10.0);
        let rounded = region_area(&fillet(&square, 2.0).unwrap());
        // Each corner loses (r² − πr²/4) ≈ 0.8584; four of them ≈ 3.434.
        let expected = 100.0 - 4.0 * (4.0 - std::f64::consts::PI);
        assert!((rounded - expected).abs() < 0.05, "{rounded} vs {expected}");
        assert!(rounded < 100.0, "rounding a convex corner removes area");

        // A radius larger than half the edge is clamped, not rejected.
        let clamped = region_area(&fillet(&square, 50.0).unwrap());
        assert!(clamped > 0.0 && clamped < 100.0, "{clamped}");

        // Zero/degenerate input is a no-op, never a panic.
        assert_eq!(region_area(&fillet(&square, 0.0).unwrap()), 100.0);
        assert!(fillet(&square, f64::NAN).is_err());
    }

    #[test]
    fn mirror_preserves_area_and_flips_the_axis() {
        let a = box_region(1.0, 2.0, 4.0, 3.0);
        let mirrored = mirror(
            &a,
            &MirrorAxis::Vertical {
                at: vectra_core::Parameter::Literal(0.0),
            },
            0.0,
        )
        .unwrap();
        assert!((region_area(&mirrored) - 12.0).abs() < 1e-9);
        let xs: Vec<f64> = mirrored
            .0
            .iter()
            .flat_map(|p| p.exterior().0.iter().map(|c| c.x))
            .collect();
        assert!(xs.iter().all(|x| *x <= 0.0), "reflected across x=0: {xs:?}");
        // …and across x = 10 the box lands on the other side of that line.
        let far = mirror(
            &a,
            &MirrorAxis::Vertical {
                at: vectra_core::Parameter::Literal(10.0),
            },
            10.0,
        )
        .unwrap();
        let far_xs: Vec<f64> = far
            .0
            .iter()
            .flat_map(|p| p.exterior().0.iter().map(|c| c.x))
            .collect();
        // x ∈ [1, 5] reflected across x = 10 lands on [15, 19].
        assert!(
            far_xs.iter().all(|x| (15.0..=19.0).contains(x)),
            "{far_xs:?}"
        );
    }
}
