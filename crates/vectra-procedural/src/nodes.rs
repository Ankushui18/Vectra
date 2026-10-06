//! The five node kinds, as plain functions (MES §11).
//!
//! Each one takes what it was wired and the values of its operands, and returns
//! **every one of its output ports** — a node is evaluated once and publishes
//! all of its ports, so a downstream consumer and a `Parameter::Procedural` slot
//! never see two different pictures of the same node.
//!
//! Everything here is pure: no document, no scene, no evaluator, no clock. The
//! pass ([`crate::engine`]) is what knows about documents and dirty sets; these
//! functions only know about numbers and points, which is why they can be tested
//! with literals and why RULE 5's determinism is checkable by calling one twice.

use crate::error::ProceduralError;
use crate::noise;
use std::collections::BTreeMap;
use vectra_core::{Color, GeometryData, Point2, PortId, ProceduralKind};

/// Everything a node kind needs to produce its outputs.
pub struct NodeInputs<'a> {
    /// The kind being evaluated.
    pub kind: &'a ProceduralKind,
    /// Resolved operand values (scalar / point / colour ports), by port name.
    pub operands: &'a BTreeMap<PortId, GeometryData>,
    /// Wired upstream outputs, by input port name.
    pub wires: &'a BTreeMap<PortId, GeometryData>,
    /// Output ports of this node's kind, in schema order.
    pub outputs: &'a [PortId],
}

/// Evaluate a node kind: one map of outputs, or a typed failure that the pass
/// turns into a single diagnostic.
///
/// The returned map is keyed by every port in `inputs.outputs`, so a caller never
/// has to wonder whether a port is "missing" or "empty".
pub fn evaluate(
    inputs: &NodeInputs<'_>,
) -> Result<BTreeMap<PortId, GeometryData>, ProceduralError> {
    match inputs.kind {
        ProceduralKind::Source { .. } => source(inputs),
        ProceduralKind::Grid { .. } => grid(inputs),
        ProceduralKind::Repeat { .. } => repeat(inputs),
        ProceduralKind::Noise { .. } => noise_node(inputs),
        ProceduralKind::Smooth { .. } => smooth(inputs),
    }
}

/// The value of a scalar operand, or a typed failure naming the port.
fn scalar(inputs: &NodeInputs<'_>, port: &str) -> Result<f64, ProceduralError> {
    match inputs.operands.get(port) {
        Some(GeometryData::Scalar(v)) => Ok(*v),
        Some(other) => Err(ProceduralError::OperandType {
            port: port.to_string(),
            expected: "scalar",
            found: other.kind_tag(),
        }),
        None => Err(ProceduralError::MissingOperand(port.to_string())),
    }
}

fn point(inputs: &NodeInputs<'_>, port: &str) -> Result<Point2, ProceduralError> {
    match inputs.operands.get(port) {
        Some(GeometryData::Point(p)) => Ok(*p),
        Some(other) => Err(ProceduralError::OperandType {
            port: port.to_string(),
            expected: "point",
            found: other.kind_tag(),
        }),
        None => Err(ProceduralError::MissingOperand(port.to_string())),
    }
}

/// The wired region feeding a modifier, or a typed failure.
fn region(inputs: &NodeInputs<'_>) -> Result<Vec<Vec<Point2>>, ProceduralError> {
    match inputs.wires.get("region") {
        Some(GeometryData::Region { rings }) => {
            if rings.is_empty() {
                Err(ProceduralError::EmptyResult)
            } else {
                Ok(rings.clone())
            }
        }
        Some(other) => Err(ProceduralError::OperandType {
            port: "region".to_string(),
            expected: "region",
            found: other.kind_tag(),
        }),
        None => Err(ProceduralError::MissingInput("region".to_string())),
    }
}

/// Build the `port → value` map for the ports the kind declares, so a node's
/// output set is exactly its schema.
fn outputs_of(
    inputs: &NodeInputs<'_>,
    values: Vec<(&str, GeometryData)>,
) -> Result<BTreeMap<PortId, GeometryData>, ProceduralError> {
    let mut map: BTreeMap<PortId, GeometryData> = BTreeMap::new();
    for (name, value) in values {
        if !inputs.outputs.iter().any(|port| port == name) {
            return Err(ProceduralError::UnknownPort(name.to_string()));
        }
        map.insert(name.to_string(), value);
    }
    Ok(map)
}

/// A grid's geometry: `columns × rows` **cells**, `spacing` between
/// neighbouring vertices, anchored at `origin`.
///
/// Cells rather than vertices, because that is what the numbers read as: a 3×2
/// grid with 10-unit spacing is three cells wide and two tall — a 30×20 region
/// with a 4×3 lattice of points. A grid with no cells encloses no area, so it
/// fails rather than drawing a degenerate ring.
fn grid(inputs: &NodeInputs<'_>) -> Result<BTreeMap<PortId, GeometryData>, ProceduralError> {
    let columns = count(scalar(inputs, "columns")?, 0.0..=1024.0, "columns")?;
    let rows = count(scalar(inputs, "rows")?, 0.0..=1024.0, "rows")?;
    let spacing = scalar(inputs, "spacing")?;
    let origin = point(inputs, "origin")?;
    if !spacing.is_finite() {
        return Err(ProceduralError::NonFinite("spacing".to_string()));
    }
    if columns == 0 || rows == 0 || spacing == 0.0 {
        return Err(ProceduralError::EmptyResult);
    }
    if columns.saturating_mul(rows) > 65_536 {
        return Err(ProceduralError::TooLarge {
            cells: columns.saturating_mul(rows),
        });
    }

    let width = columns as f64 * spacing;
    let height = rows as f64 * spacing;
    let x0 = origin.x;
    let y0 = origin.y;

    // The lattice: (columns + 1) × (rows + 1) vertices, row-major.
    let mut points: Vec<Point2> = Vec::with_capacity((columns + 1) * (rows + 1));
    for row in 0..=rows {
        for column in 0..=columns {
            points.push(Point2::new(
                x0 + column as f64 * spacing,
                y0 + row as f64 * spacing,
            ));
        }
    }
    // The region: the rectangle the cells cover, wound counter-clockwise from
    // the origin corner (the renderer fills even-odd, so the winding is a
    // convention rather than a correctness requirement — but it is *this*
    // convention everywhere in the workspace).
    let rings = vec![vec![
        Point2::new(x0, y0),
        Point2::new(x0 + width, y0),
        Point2::new(x0 + width, y0 + height),
        Point2::new(x0, y0 + height),
    ]];

    outputs_of(
        inputs,
        vec![
            ("region", GeometryData::Region { rings }),
            ("points", GeometryData::Points(points)),
            (
                "center",
                GeometryData::Point(Point2::new(x0 + width / 2.0, y0 + height / 2.0)),
            ),
            ("span", GeometryData::Scalar(width)),
        ],
    )
}

/// Replicate the input region `count` times, offset by `(dx, dy)` per copy.
///
/// The copies stay separate rings inside one region. The renderer fills
/// even-odd, so **overlapping copies read as a weave** (the overlap is knocked
/// out) — a deliberate, documented consequence of not running a boolean union
/// here; the defaults offset copies so they do not overlap.
fn repeat(inputs: &NodeInputs<'_>) -> Result<BTreeMap<PortId, GeometryData>, ProceduralError> {
    let rings = region(inputs)?;
    let count = count(scalar(inputs, "count")?, 0.0..=4096.0, "count")?;
    let dx = scalar(inputs, "dx")?;
    let dy = scalar(inputs, "dy")?;
    if !dx.is_finite() || !dy.is_finite() {
        return Err(ProceduralError::NonFinite("dx/dy".to_string()));
    }
    if count == 0 {
        return Err(ProceduralError::EmptyResult);
    }
    let total: usize = rings.iter().map(|r| r.len()).sum();
    if total.saturating_mul(count) > 1_000_000 {
        return Err(ProceduralError::TooLarge {
            cells: total * count,
        });
    }

    let mut out: Vec<Vec<Point2>> = Vec::with_capacity(rings.len() * count);
    for copy in 0..count {
        let ox = dx * copy as f64;
        let oy = dy * copy as f64;
        for ring in &rings {
            out.push(
                ring.iter()
                    .map(|p| Point2::new(p.x + ox, p.y + oy))
                    .collect(),
            );
        }
    }
    outputs_of(
        inputs,
        vec![("region", GeometryData::Region { rings: out })],
    )
}

/// Displace the input region's vertices with deterministic value noise.
///
/// Two decorrelated fields (seeds `s` and `s + 1`) move the vertex in x and y, so
/// the displacement is isotropic rather than a shear. The `scalar` output is the
/// raw noise field at the region's centroid — a value a slot can bind to — and
/// `tint` is that same number as a greyscale colour.
fn noise_node(inputs: &NodeInputs<'_>) -> Result<BTreeMap<PortId, GeometryData>, ProceduralError> {
    let rings = region(inputs)?;
    let amplitude = scalar(inputs, "amplitude")?;
    let frequency = scalar(inputs, "frequency")?;
    let seed = scalar(inputs, "seed")?;
    if !amplitude.is_finite() || !frequency.is_finite() {
        return Err(ProceduralError::NonFinite(
            "amplitude/frequency".to_string(),
        ));
    }
    let bits = noise::seed_bits(seed);

    let mut out: Vec<Vec<Point2>> = Vec::with_capacity(rings.len());
    for ring in &rings {
        let mut displaced: Vec<Point2> = Vec::with_capacity(ring.len());
        for vertex in ring {
            let nx = noise::value_noise(bits, vertex.x * frequency, vertex.y * frequency) - 0.5;
            let ny = noise::value_noise(
                bits ^ 0x9E37_79B9,
                vertex.x * frequency,
                vertex.y * frequency,
            ) - 0.5;
            displaced.push(Point2::new(
                vertex.x + nx * 2.0 * amplitude,
                vertex.y + ny * 2.0 * amplitude,
            ));
        }
        out.push(displaced);
    }

    let centroid = centroid_of(&rings);
    let sample = noise::value_noise(bits, centroid.x * frequency, centroid.y * frequency);
    let grey = (sample * 255.0).round().clamp(0.0, 255.0) as u8;

    outputs_of(
        inputs,
        vec![
            ("region", GeometryData::Region { rings: out }),
            ("scalar", GeometryData::Scalar(sample)),
            ("tint", GeometryData::Color(Color::rgb(grey, grey, grey))),
        ],
    )
}

/// Chaikin corner cutting, `iterations` times, at `strength` ∈ [0, 1].
///
/// Each edge contributes two points at ¼ and ¾ of its length at full strength;
/// `strength` interpolates from "unchanged" (0) to the classic curve (1), so the
/// amount of rounding is a number the document can animate. Consecutive
/// duplicates are collapsed, and a ring needs at least three vertices to have
/// corners to cut.
fn smooth(inputs: &NodeInputs<'_>) -> Result<BTreeMap<PortId, GeometryData>, ProceduralError> {
    let rings = region(inputs)?;
    let iterations = count(scalar(inputs, "iterations")?, 0.0..=8.0, "iterations")?;
    let strength = scalar(inputs, "strength")?;
    if !strength.is_finite() {
        return Err(ProceduralError::NonFinite("strength".to_string()));
    }
    let strength = strength.clamp(0.0, 1.0);

    let mut out: Vec<Vec<Point2>> = Vec::with_capacity(rings.len());
    for ring in &rings {
        let mut current: Vec<Point2> = ring.clone();
        for _ in 0..iterations {
            if current.len() < 3 || strength == 0.0 {
                break;
            }
            let mut next: Vec<Point2> = Vec::with_capacity(current.len() * 2);
            for index in 0..current.len() {
                let a = current[index];
                let b = current[(index + 1) % current.len()];
                // The two classic Chaikin points on this edge, pulled back
                // towards `a` by `1 − strength`.
                let quarter = Point2::new(a.x + (b.x - a.x) * 0.25, a.y + (b.y - a.y) * 0.25);
                let three_quarters =
                    Point2::new(a.x + (b.x - a.x) * 0.75, a.y + (b.y - a.y) * 0.75);
                next.push(lerp(a, quarter, strength));
                next.push(lerp(a, three_quarters, strength));
            }
            dedupe_in_place(&mut next);
            current = next;
        }
        out.push(current);
    }

    if out.iter().all(|ring| ring.len() < 3) {
        return Err(ProceduralError::EmptyResult);
    }
    outputs_of(
        inputs,
        vec![("region", GeometryData::Region { rings: out })],
    )
}

/// A `Source` node hands the upstream node's own evaluated primitive straight
/// through as a region.
///
/// The conversion happens in the pass (which has the scene); this arm exists so
/// the kind is total, and it is only reached with an already-converted region.
fn source(inputs: &NodeInputs<'_>) -> Result<BTreeMap<PortId, GeometryData>, ProceduralError> {
    match inputs.wires.get("geometry") {
        Some(GeometryData::Region { rings }) => outputs_of(
            inputs,
            vec![(
                "region",
                GeometryData::Region {
                    rings: rings.clone(),
                },
            )],
        ),
        Some(other) => Err(ProceduralError::OperandType {
            port: "geometry".to_string(),
            expected: "region",
            found: other.kind_tag(),
        }),
        None => Err(ProceduralError::MissingInput("geometry".to_string())),
    }
}

fn lerp(a: Point2, b: Point2, t: f64) -> Point2 {
    Point2::new(a.x + (b.x - a.x) * t, a.y + (b.y - a.y) * t)
}

fn dedupe_in_place(points: &mut Vec<Point2>) {
    points.dedup_by(|a, b| (a.x - b.x).abs() < 1e-12 && (a.y - b.y).abs() < 1e-12);
}

/// The mean of every vertex of every ring (the noise node's sample point).
fn centroid_of(rings: &[Vec<Point2>]) -> Point2 {
    let mut count = 0usize;
    let mut sx = 0.0;
    let mut sy = 0.0;
    for ring in rings {
        for point in ring {
            sx += point.x;
            sy += point.y;
            count += 1;
        }
    }
    if count == 0 {
        Point2::ZERO
    } else {
        Point2::new(sx / count as f64, sy / count as f64)
    }
}

/// A non-negative integer operand: rounded, bounds-checked, and *reported* when
/// it falls outside what the node can build.
fn count(
    value: f64,
    range: std::ops::RangeInclusive<f64>,
    port: &str,
) -> Result<usize, ProceduralError> {
    if !value.is_finite() {
        return Err(ProceduralError::NonFinite(port.to_string()));
    }
    let rounded = value.round();
    if !range.contains(&rounded) {
        return Err(ProceduralError::OutOfRange {
            port: port.to_string(),
            value: rounded,
            max: *range.end(),
            min: *range.start(),
        });
    }
    Ok(rounded as usize)
}

#[cfg(test)]
mod tests {
    use super::*;
    use vectra_core::ProceduralKind;

    fn operand(port: &str, value: GeometryData) -> (String, GeometryData) {
        (port.to_string(), value)
    }

    fn inputs<'a>(
        kind: &'a ProceduralKind,
        operands: &'a BTreeMap<PortId, GeometryData>,
        wires: &'a BTreeMap<PortId, GeometryData>,
        outputs: &'a [PortId],
    ) -> NodeInputs<'a> {
        NodeInputs {
            kind,
            operands,
            wires,
            outputs,
        }
    }

    fn grid_operands(columns: f64, rows: f64, spacing: f64) -> BTreeMap<PortId, GeometryData> {
        BTreeMap::from([
            operand("columns", GeometryData::Scalar(columns)),
            operand("rows", GeometryData::Scalar(rows)),
            operand("spacing", GeometryData::Scalar(spacing)),
            operand("origin", GeometryData::Point(Point2::ZERO)),
        ])
    }

    fn ports_of(kind: &ProceduralKind) -> Vec<PortId> {
        kind.outputs().into_iter().map(|p| p.name).collect()
    }

    fn square_region() -> BTreeMap<PortId, GeometryData> {
        BTreeMap::from([operand(
            "region",
            GeometryData::Region {
                rings: vec![vec![
                    Point2::new(0.0, 0.0),
                    Point2::new(10.0, 0.0),
                    Point2::new(10.0, 10.0),
                    Point2::new(0.0, 10.0),
                ]],
            },
        )])
    }

    #[test]
    fn a_grid_produces_cells_lattice_centre_and_span() {
        let kind = ProceduralKind::grid(3.0, 2.0, 10.0, Point2::ZERO);
        let operands = grid_operands(3.0, 2.0, 10.0);
        let wires = BTreeMap::new();
        let ports = ports_of(&kind);
        let out = evaluate(&inputs(&kind, &operands, &wires, &ports)).unwrap();

        // 3×2 cells ⇒ a 30×20 region and a 4×3 = 12-point lattice.
        assert_eq!(out["region"].rings().unwrap()[0].len(), 4);
        assert_eq!(out["points"].vertices().len(), 12);
        assert_eq!(out["center"].as_point(), Some(Point2::new(15.0, 10.0)));
        assert_eq!(out["span"].as_scalar(), Some(30.0));
        // Every declared port is present: no "missing" vs "empty" ambiguity.
        assert_eq!(out.len(), ports.len());
    }

    #[test]
    fn a_degenerate_grid_fails_instead_of_drawing_nothing() {
        let kind = ProceduralKind::grid(0.0, 2.0, 10.0, Point2::ZERO);
        let operands = grid_operands(0.0, 2.0, 10.0);
        let wires = BTreeMap::new();
        let ports = ports_of(&kind);
        let error = evaluate(&inputs(&kind, &operands, &wires, &ports)).unwrap_err();
        assert!(matches!(error, ProceduralError::EmptyResult), "{error}");

        // A count past the guard rail is *refused*, not attempted…
        let huge = grid_operands(4000.0, 2.0, 10.0);
        let error = evaluate(&inputs(&kind, &huge, &wires, &ports)).unwrap_err();
        assert!(
            matches!(error, ProceduralError::OutOfRange { .. }),
            "{error}"
        );

        // …and so is a grid whose cell count passes the memory guard, even
        // though each count is individually legal.
        let kind = ProceduralKind::grid(1024.0, 1024.0, 1.0, Point2::ZERO);
        let wide = grid_operands(1024.0, 1024.0, 1.0);
        let error = evaluate(&inputs(&kind, &wide, &wires, &ports)).unwrap_err();
        assert!(matches!(error, ProceduralError::TooLarge { .. }), "{error}");
    }

    #[test]
    fn repeat_instances_the_region_and_refuses_a_zero_count() {
        let kind = ProceduralKind::repeat(3.0, 20.0, 0.0);
        let wires = square_region();
        let operands = BTreeMap::from([
            operand("count", GeometryData::Scalar(3.0)),
            operand("dx", GeometryData::Scalar(20.0)),
            operand("dy", GeometryData::Scalar(0.0)),
        ]);
        let ports = ports_of(&kind);
        let out = evaluate(&inputs(&kind, &operands, &wires, &ports)).unwrap();
        let rings = out["region"].rings().unwrap();
        assert_eq!(rings.len(), 3, "three copies");
        assert_eq!(rings[0][0], Point2::new(0.0, 0.0));
        assert_eq!(rings[1][0], Point2::new(20.0, 0.0));
        assert_eq!(rings[2][0], Point2::new(40.0, 0.0));

        let zero = BTreeMap::from([
            operand("count", GeometryData::Scalar(0.0)),
            operand("dx", GeometryData::Scalar(20.0)),
            operand("dy", GeometryData::Scalar(0.0)),
        ]);
        let error = evaluate(&inputs(&kind, &zero, &wires, &ports)).unwrap_err();
        assert!(matches!(error, ProceduralError::EmptyResult));
    }

    #[test]
    fn noise_displaces_deterministically_and_publishes_a_sample_and_tint() {
        let kind = ProceduralKind::noise(2.0, 0.05, 3.0);
        let wires = square_region();
        let operands = BTreeMap::from([
            operand("amplitude", GeometryData::Scalar(2.0)),
            operand("frequency", GeometryData::Scalar(0.05)),
            operand("seed", GeometryData::Scalar(3.0)),
        ]);
        let ports = ports_of(&kind);
        let first = evaluate(&inputs(&kind, &operands, &wires, &ports)).unwrap();
        let second = evaluate(&inputs(&kind, &operands, &wires, &ports)).unwrap();
        assert_eq!(first, second, "the same call twice is byte-identical");

        let moved = first["region"].rings().unwrap()[0][0];
        assert!(
            (moved.x - 0.0).abs() > 1e-9 || (moved.y - 0.0).abs() > 1e-9,
            "the corner actually moved: {moved:?}"
        );
        assert!(
            (moved.x - 0.0).abs() <= 2.0 + 1e-9 && (moved.y - 0.0).abs() <= 2.0 + 1e-9,
            "and stayed inside the amplitude bound: {moved:?}"
        );

        let sample = first["scalar"].as_scalar().unwrap();
        assert!((0.0..=1.0).contains(&sample));
        let tint = first["tint"].as_color().unwrap();
        assert_eq!(tint.r, tint.g);
        assert_eq!(tint.g, tint.b);
        assert_eq!(
            (sample * 255.0).round() as u8,
            tint.r,
            "the tint is the sample"
        );

        // A different seed is a different picture.
        let other = BTreeMap::from([
            operand("amplitude", GeometryData::Scalar(2.0)),
            operand("frequency", GeometryData::Scalar(0.05)),
            operand("seed", GeometryData::Scalar(4.0)),
        ]);
        assert_ne!(
            evaluate(&inputs(&kind, &other, &wires, &ports)).unwrap(),
            first
        );
    }

    #[test]
    fn smoothing_keeps_the_ring_closed_and_inside_the_original_hull() {
        let kind = ProceduralKind::smooth(2.0, 1.0);
        let wires = square_region();
        let operands = BTreeMap::from([
            operand("iterations", GeometryData::Scalar(2.0)),
            operand("strength", GeometryData::Scalar(1.0)),
        ]);
        let ports = ports_of(&kind);
        let out = evaluate(&inputs(&kind, &operands, &wires, &ports)).unwrap();
        let ring = &out["region"].rings().unwrap()[0];
        assert!(
            ring.len() > 4,
            "corner cutting adds vertices: {}",
            ring.len()
        );
        for point in ring {
            assert!(
                (-1e-9..=10.0 + 1e-9).contains(&point.x)
                    && (-1e-9..=10.0 + 1e-9).contains(&point.y),
                "a cut corner stays inside the square: {point:?}"
            );
        }
        // Zero iterations is the identity, and reproduces exactly.
        let identity = BTreeMap::from([
            operand("iterations", GeometryData::Scalar(0.0)),
            operand("strength", GeometryData::Scalar(1.0)),
        ]);
        let same = evaluate(&inputs(&kind, &identity, &wires, &ports)).unwrap();
        assert_eq!(same["region"].rings().unwrap()[0].len(), 4);
    }

    #[test]
    fn a_missing_input_is_a_typed_failure_not_a_panic() {
        let kind = ProceduralKind::smooth(1.0, 1.0);
        let operands = BTreeMap::from([
            operand("iterations", GeometryData::Scalar(1.0)),
            operand("strength", GeometryData::Scalar(1.0)),
        ]);
        let empty = BTreeMap::new();
        let ports = ports_of(&kind);
        let error = evaluate(&inputs(&kind, &operands, &empty, &ports)).unwrap_err();
        assert!(matches!(error, ProceduralError::MissingInput(_)), "{error}");

        // …and a wrong-typed operand is named.
        let wrong = BTreeMap::from([
            operand("iterations", GeometryData::Point(Point2::ZERO)),
            operand("strength", GeometryData::Scalar(1.0)),
        ]);
        let error = evaluate(&inputs(&kind, &wrong, &square_region(), &ports)).unwrap_err();
        match error {
            ProceduralError::OperandType {
                port,
                expected,
                found,
                ..
            } => {
                assert_eq!(port, "iterations");
                assert_eq!(expected, "scalar");
                assert_eq!(found, "point");
            }
            other => panic!("expected a typed operand failure, got {other}"),
        }
    }

    #[test]
    fn every_kind_declares_its_ports_and_resolves_them() {
        // A smoke over the schema itself: each kind's outputs are exactly the
        // ports it produces, and each output port has a declared type.
        for kind in [
            ProceduralKind::grid(2.0, 2.0, 5.0, Point2::ZERO),
            ProceduralKind::repeat(2.0, 10.0, 0.0),
            ProceduralKind::noise(1.0, 0.1, 0.0),
            ProceduralKind::smooth(1.0, 1.0),
        ] {
            for port in kind.outputs() {
                assert!(!port.name.is_empty(), "{}: a port needs a name", kind.tag());
                assert!(
                    kind.output_type(&port.name) == Some(port.ty),
                    "{}: {} is declared as {:?}",
                    kind.tag(),
                    port.name,
                    port.ty
                );
            }
            assert!(kind.geometry_port().is_some());
        }
    }
}
