//! The procedural graph's data model: typed ports, `GeometryData`, and the
//! registry of nodes (MES §11, Task 7.0).
//!
//! # What lives here, and what does not
//!
//! `core` owns the *schema*: [`GeometryData`], [`PortType`], the node kinds and
//! their ports, the registry, and every validation that can be decided without
//! computing anything. The **algorithms** (generating a grid, displacing a
//! region, smoothing a polyline, hashing noise) live in `vectra-procedural`,
//! which implements [`crate::eval::ProceduralEvaluator`] — the same split as
//! every other subsystem (`core` is the dependency root; algorithms live in
//! owner crates).
//!
//! `core` therefore does **not** depend on `lyon` or `geo` (RULE 1). A region
//! crosses this boundary as rings of [`Point2`]: the conversion to `geo`
//! polygons and back to a `lyon` path already exists and is tested in
//! `vectra-operations::convert`, and reusing it keeps one flattening tolerance
//! in the workspace.
//!
//! # Typed ports (RULE 1)
//!
//! A port carries exactly one [`PortType`]. A wire between mismatched ports is
//! rejected **before anything is stored**, with
//! [`crate::error::ResolveError::ProceduralPortType`] — never a coercion, never
//! a zero. The same holds for an operand whose `ParamValue` variant does not
//! match the port it is written to.
//!
//! # No disguised cycles (RULE 3)
//!
//! Wires express dependency *inside* the graph. A node's `Parameter<f64>`
//! operand may read variables, expressions or motion, but **not** another
//! node's procedural output: that would be geometry ← parameter ← procedural
//! value ← geometry, a cycle wearing a disguise. [`ProceduralNode::validate`]
//! rejects it with [`crate::error::VectraError::CyclicDependency`], and so does
//! every command that can introduce an operand.
//!
//! The rule buys three things: evaluation order stays a straight topological
//! sweep, the published value table is a pure function of the document, and the
//! fixpoint in the engine's settle path terminates in exactly one extra round
//! (see `vectra-procedural`'s pass).
//!
//! # Where a procedural node lives
//!
//! Beside constraints, operations and motion tracks: a registry of its own
//! ([`ProceduralRegistry`]) rather than an entry in
//! [`crate::document::Document::nodes`], because a node's *wiring* is a map and
//! a `NodeKind` variant would ripple through every exhaustive match. The ids
//! share the [`NodeId`] space, so a geometry-producing result can be keyed,
//! ordered, hit-tested and styled exactly like a primitive (Task 4.0 RULE 3).

use crate::document::StyleProperties;
use crate::error::{ResolveError, VectraError};
use crate::geom::{Color, Point2};
use crate::ids::{NodeId, PortId};
use crate::param::{NodeOutputId, ParamValue, Parameter};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// The type a port carries.
///
/// `Scalar` / `Point` / `Color` are **value** ports: they are published in the
/// engine's value table and read by `Parameter::Procedural` slots.
/// `Points` / `Path` / `Region` are **geometry** ports: they compose into the
/// evaluated scene as virtual nodes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PortType {
    /// One number.
    Scalar,
    /// One position.
    Point,
    /// A loose set of positions (a grid's vertices, a point cloud).
    Points,
    /// An ordered polyline, optionally closed.
    Path,
    /// One or more closed rings (a region with holes).
    Region,
    /// One colour.
    Color,
}

impl PortType {
    pub fn tag(self) -> &'static str {
        match self {
            Self::Scalar => "scalar",
            Self::Point => "point",
            Self::Points => "points",
            Self::Path => "path",
            Self::Region => "region",
            Self::Color => "color",
        }
    }

    /// True ⟺ a port of this type can be composed into the scene (RULE 3 of
    /// Task 4.0) rather than published as a value.
    pub fn is_geometry(self) -> bool {
        matches!(self, Self::Points | Self::Path | Self::Region)
    }

    /// True ⟺ a `ParamValue` of this type may be written to a port of this type.
    pub fn accepts_param(self, value: &ParamValue) -> bool {
        matches!(
            (self, value),
            (Self::Scalar, ParamValue::Float(_))
                | (Self::Point, ParamValue::Point(_))
                | (Self::Color, ParamValue::Color(_))
        )
    }

    /// The `ParamValue` kind tag this port expects (`"float"`, `"point"`,
    /// `"color"`), for typed mismatches. `None` for wire-only ports.
    pub fn param_kind(self) -> Option<&'static str> {
        match self {
            Self::Scalar => Some("float"),
            Self::Point => Some("point"),
            Self::Color => Some("color"),
            Self::Points | Self::Path | Self::Region => None,
        }
    }
}

/// A declared port on a procedural node (MES §11's `Port`, typed).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Port {
    pub name: PortId,
    pub ty: PortType,
    /// A required *input* port is one the node cannot evaluate without; the
    /// pass reports a typed diagnostic until it is wired, and the node simply
    /// contributes nothing (Containment Law).
    pub required: bool,
}

impl Port {
    pub fn new(name: impl Into<PortId>, ty: PortType) -> Self {
        Self {
            name: name.into(),
            ty,
            required: true,
        }
    }

    pub fn optional(name: impl Into<PortId>, ty: PortType) -> Self {
        Self {
            name: name.into(),
            ty,
            required: false,
        }
    }
}

/// What flows along a wire (MES §11's `GeometryData`, typed and serializable).
///
/// Deliberately dependency-free: no `lyon`, no `geo`, no trait objects. A value
/// crosses the crate boundary as plain data, which is what lets the document
/// round-trip through JSON and lets a headless test assert a node's output
/// byte-for-byte.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum GeometryData {
    Scalar(f64),
    Point(Point2),
    Points(Vec<Point2>),
    /// A polyline. `closed` means the last vertex connects back to the first
    /// (the ring is *not* repeated vertex-for-vertex).
    Path {
        points: Vec<Point2>,
        closed: bool,
    },
    /// Closed rings: `rings[0]` is the outer boundary, the rest are holes.
    /// Rings do not repeat their first vertex, and their winding is normalized
    /// when they are converted to `geo` regions.
    Region {
        rings: Vec<Vec<Point2>>,
    },
    Color(Color),
}

impl GeometryData {
    pub fn port_type(&self) -> PortType {
        match self {
            Self::Scalar(_) => PortType::Scalar,
            Self::Point(_) => PortType::Point,
            Self::Points(_) => PortType::Points,
            Self::Path { .. } => PortType::Path,
            Self::Region { .. } => PortType::Region,
            Self::Color(_) => PortType::Color,
        }
    }

    pub fn kind_tag(&self) -> &'static str {
        self.port_type().tag()
    }

    pub fn as_scalar(&self) -> Option<f64> {
        match self {
            Self::Scalar(v) => Some(*v),
            _ => None,
        }
    }

    pub fn as_point(&self) -> Option<Point2> {
        match self {
            Self::Point(p) => Some(*p),
            _ => None,
        }
    }

    pub fn as_color(&self) -> Option<Color> {
        match self {
            Self::Color(c) => Some(*c),
            _ => None,
        }
    }

    /// The rings this value covers, if it is a region.
    pub fn rings(&self) -> Option<&[Vec<Point2>]> {
        match self {
            Self::Region { rings } => Some(rings),
            _ => None,
        }
    }

    /// Every vertex in the value, in order (empty for scalars and colours).
    pub fn vertices(&self) -> Vec<Point2> {
        match self {
            Self::Scalar(_) | Self::Color(_) => Vec::new(),
            Self::Point(p) => vec![*p],
            Self::Points(points) => points.clone(),
            Self::Path { points, .. } => points.clone(),
            Self::Region { rings } => rings.iter().flatten().copied().collect(),
        }
    }

    /// True ⟺ this value encloses no area: the region case that makes a
    /// modifier fail with [`crate::error::VectraError`]-typed diagnostics.
    pub fn is_empty_region(&self) -> bool {
        match self {
            Self::Region { rings } => rings.iter().all(|ring| ring.len() < 3),
            _ => false,
        }
    }
}

/// A procedural node's behaviour (MES §11: generators and modifiers).
///
/// The kind declares *ports and defaults*; it computes nothing. Every number it
/// reads is a [`Parameter<f64>`] operand, so a procedural node's inputs are as
/// parametric as any other number in the document — a grid's spacing can be a
/// variable, an expression, or a spring.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ProceduralKind {
    /// Read an authored node's evaluated geometry (Task 4.0 RULE 1: the source
    /// is read, never written). This is the graph's way in.
    Source { node: NodeId },
    /// A rectangular grid of points, and the region that grid spans.
    Grid {
        columns: Parameter<f64>,
        rows: Parameter<f64>,
        spacing: Parameter<f64>,
        origin: Parameter<Point2>,
    },
    /// Instance the input region `count` times, offset by `(dx, dy)` per copy.
    Repeat {
        count: Parameter<f64>,
        dx: Parameter<f64>,
        dy: Parameter<f64>,
    },
    /// Displace the input region's vertices by deterministic value noise, and
    /// publish the noise sample and its greyscale tint as values.
    Noise {
        amplitude: Parameter<f64>,
        frequency: Parameter<f64>,
        seed: Parameter<f64>,
    },
    /// Chaikin-style corner cutting: pull each vertex towards its neighbours.
    Smooth {
        iterations: Parameter<f64>,
        strength: Parameter<f64>,
    },
    /// **A Smart Component master** (Task 10.6 RULE 1): the definition of a
    /// parametric piece of artwork.
    ///
    /// It produces no geometry of its own — its `members` are ordinary authored
    /// nodes, and its job is to declare the [`crate::component::ComponentSpec`]
    /// those members are bound to. Publishing each prop as a value port is what
    /// lets a reader (an instance, an operation, a `Parameter::Procedural`
    /// slot) follow the master's value through the graph.
    ComponentMaster {
        /// The authored nodes this definition is made of.
        members: Vec<NodeId>,
        spec: crate::component::ComponentSpec,
    },
    /// **A Smart Component instance** (Task 10.6 RULE 1): a reference to a
    /// master, plus its own copy of the artwork.
    ///
    /// `group` is the authored group holding the clones; `spec` names the props
    /// and the variables they read. Like the master, an instance produces no
    /// geometry of its own — the clones are real nodes and draw themselves.
    Component {
        master: NodeId,
        group: NodeId,
        spec: crate::component::ComponentSpec,
    },
}

impl ProceduralKind {
    pub fn tag(&self) -> &'static str {
        match self {
            Self::Source { .. } => "source",
            Self::Grid { .. } => "grid",
            Self::Repeat { .. } => "repeat",
            Self::Noise { .. } => "noise",
            Self::Smooth { .. } => "smooth",
            Self::ComponentMaster { .. } => "component_master",
            Self::Component { .. } => "component",
        }
    }

    /// The wired (geometry) inputs, in schema order.
    pub fn inputs(&self) -> Vec<Port> {
        match self {
            Self::Source { .. } | Self::Grid { .. } => Vec::new(),
            Self::Repeat { .. } | Self::Noise { .. } | Self::Smooth { .. } => {
                vec![Port::new("region", PortType::Region)]
            }
            // A component reads its artwork from the document (its members are
            // named by id), not from a wire: there is nothing to connect and
            // nothing to type-check at this boundary.
            Self::ComponentMaster { .. } | Self::Component { .. } => Vec::new(),
        }
    }

    /// Every output port, in schema order. The first is the drawn one for the
    /// generator/modifier kinds; the value ports follow.
    pub fn outputs(&self) -> Vec<Port> {
        match self {
            Self::Source { .. } => vec![Port::new("region", PortType::Region)],
            Self::Grid { .. } => vec![
                Port::new("region", PortType::Region),
                Port::new("points", PortType::Points),
                Port::new("center", PortType::Point),
                Port::new("span", PortType::Scalar),
            ],
            Self::Repeat { .. } => vec![Port::new("region", PortType::Region)],
            Self::Noise { .. } => vec![
                Port::new("region", PortType::Region),
                Port::new("scalar", PortType::Scalar),
                Port::new("tint", PortType::Color),
            ],
            Self::Smooth { .. } => vec![Port::new("region", PortType::Region)],
            // One value port per prop, in prop order: a component's props are
            // readable from anywhere that can name a port.
            Self::ComponentMaster { spec, .. } | Self::Component { spec, .. } => spec
                .props
                .iter()
                .map(|prop| Port::new(prop.key.clone(), prop.ty.port_type()))
                .collect(),
        }
    }

    /// The operand (value-writable) ports, in schema order.
    pub fn operands(&self) -> Vec<Port> {
        match self {
            Self::Source { .. } => Vec::new(),
            Self::Grid { .. } => vec![
                Port::new("columns", PortType::Scalar),
                Port::new("rows", PortType::Scalar),
                Port::new("spacing", PortType::Scalar),
                Port::new("origin", PortType::Point),
            ],
            Self::Repeat { .. } => vec![
                Port::new("count", PortType::Scalar),
                Port::new("dx", PortType::Scalar),
                Port::new("dy", PortType::Scalar),
            ],
            Self::Noise { .. } => vec![
                Port::new("amplitude", PortType::Scalar),
                Port::new("frequency", PortType::Scalar),
                Port::new("seed", PortType::Scalar),
            ],
            Self::Smooth { .. } => vec![
                Port::new("iterations", PortType::Scalar),
                Port::new("strength", PortType::Scalar),
            ],
            // A prop *is* an operand port here: `SetComponentProp` writes the
            // variable or the colour it names, and the pass republishes it.
            Self::ComponentMaster { spec, .. } | Self::Component { spec, .. } => spec
                .props
                .iter()
                .map(|prop| Port::new(prop.key.clone(), prop.ty.port_type()))
                .collect(),
        }
    }

    /// The operand values a freshly-created node starts with.
    pub fn default_operands(&self) -> BTreeMap<PortId, ParamValue> {
        let mut map: BTreeMap<PortId, ParamValue> = BTreeMap::new();
        match self {
            Self::Source { .. } => {}
            Self::Grid {
                columns,
                rows,
                spacing,
                origin,
            } => {
                map.insert("columns".into(), ParamValue::Float(columns.clone()));
                map.insert("rows".into(), ParamValue::Float(rows.clone()));
                map.insert("spacing".into(), ParamValue::Float(spacing.clone()));
                map.insert("origin".into(), ParamValue::Point(origin.clone()));
            }
            Self::Repeat { count, dx, dy } => {
                map.insert("count".into(), ParamValue::Float(count.clone()));
                map.insert("dx".into(), ParamValue::Float(dx.clone()));
                map.insert("dy".into(), ParamValue::Float(dy.clone()));
            }
            Self::Noise {
                amplitude,
                frequency,
                seed,
            } => {
                map.insert("amplitude".into(), ParamValue::Float(amplitude.clone()));
                map.insert("frequency".into(), ParamValue::Float(frequency.clone()));
                map.insert("seed".into(), ParamValue::Float(seed.clone()));
            }
            Self::Smooth {
                iterations,
                strength,
            } => {
                map.insert("iterations".into(), ParamValue::Float(iterations.clone()));
                map.insert("strength".into(), ParamValue::Float(strength.clone()));
            }
            // A component's operands are seeded by the command that creates it
            // (its variables exist by then), so a bare `ProceduralNode::new` of
            // this kind carries none — and `operand()` reports `None` until the
            // command fills them in, rather than inventing a value.
            Self::ComponentMaster { .. } | Self::Component { .. } => {}
        }
        map
    }

    /// The port that composes into the scene, if this kind produces geometry.
    /// The port that composes into the scene, if this kind produces geometry.
    ///
    /// A component produces none: its members (or clones) are ordinary authored
    /// nodes and draw themselves, which is what keeps instances inside every
    /// existing path — layers, hit testing, export, the scene.
    pub fn geometry_port(&self) -> Option<PortId> {
        match self {
            Self::ComponentMaster { .. } | Self::Component { .. } => None,
            _ => Some("region".to_string()),
        }
    }

    /// The port type of `port`, if the kind declares it as an output.
    pub fn output_type(&self, port: &str) -> Option<PortType> {
        self.outputs()
            .into_iter()
            .find(|p| p.name == port)
            .map(|p| p.ty)
    }

    /// The port type of `port`, if the kind declares it as a (wired) input.
    pub fn input_type(&self, port: &str) -> Option<PortType> {
        self.inputs()
            .into_iter()
            .find(|p| p.name == port)
            .map(|p| p.ty)
    }

    /// The port type of `port`, if the kind declares it as an operand.
    pub fn operand_type(&self, port: &str) -> Option<PortType> {
        self.operands()
            .into_iter()
            .find(|p| p.name == port)
            .map(|p| p.ty)
    }

    /// The authored node a `Source` reads.
    pub fn source_node(&self) -> Option<NodeId> {
        match self {
            Self::Source { node } => Some(*node),
            _ => None,
        }
    }

    /// One-line description of a **fresh node** of this kind, built from the
    /// schema's own default values.
    ///
    /// This is what [`ProceduralNode::new`] names a node with. For a node that
    /// exists — whose operands may have been rewritten by
    /// [`crate::command::Command::SetProceduralOperand`] — use
    /// [`ProceduralNode::describe`], which reads the *effective* values: a
    /// description built from the kind's template would silently show numbers
    /// the node no longer has.
    pub fn describe(&self) -> String {
        fn short(id: &NodeId) -> String {
            id.to_string().chars().take(8).collect()
        }
        fn operand(name: &str, param: &Parameter<f64>) -> String {
            match param {
                Parameter::Literal(v) => format!("{name}={v}"),
                other => format!("{name}=({})", other.source_tag()),
            }
        }
        fn origin(param: &Parameter<Point2>) -> String {
            match param {
                Parameter::Literal(p) => format!("@({}, {})", p.x, p.y),
                other => format!("@({})", other.source_tag()),
            }
        }
        match self {
            Self::Source { node } => format!("source {}", short(node)),
            Self::Grid {
                columns,
                rows,
                spacing,
                origin: at,
            } => format!(
                "{}×{} grid, {} apart, {}",
                operand("n", columns),
                operand("m", rows),
                operand("s", spacing).trim_start_matches("s="),
                origin(at)
            ),
            Self::Repeat { count, dx, dy } => format!(
                "repeat {} × ({}, {})",
                operand("n", count).trim_start_matches("n="),
                operand("dx", dx).trim_start_matches("dx="),
                operand("dy", dy).trim_start_matches("dy=")
            ),
            Self::Noise {
                amplitude,
                frequency,
                seed,
            } => format!(
                "noise {} {} {}",
                operand("amp", amplitude),
                operand("freq", frequency),
                operand("seed", seed)
            ),
            Self::Smooth {
                iterations,
                strength,
            } => format!(
                "smooth {} {}",
                operand("n", iterations),
                operand("s", strength)
            ),
            Self::ComponentMaster { members, spec } => format!(
                "component master — {} member(s), props[{}]",
                members.len(),
                spec.props
                    .iter()
                    .map(|prop| prop.key.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Self::Component { master, spec, .. } => format!(
                "instance of {} — props[{}]",
                short(master),
                spec.props
                    .iter()
                    .map(|prop| prop.key.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        }
    }

    /// The short label a port is shown under in [`ProceduralNode::describe`].
    pub(crate) fn port_label(port: &str) -> &str {
        match port {
            "columns" => "n",
            "rows" => "m",
            "spacing" => "s",
            "origin" => "@",
            "count" => "n",
            "amplitude" => "amp",
            "frequency" => "freq",
            "iterations" => "n",
            "strength" => "s",
            other => other,
        }
    }

    /// The safe defaults a UI (and the tests) create nodes with.
    pub fn grid(columns: f64, rows: f64, spacing: f64, origin: Point2) -> Self {
        Self::Grid {
            columns: Parameter::Literal(columns),
            rows: Parameter::Literal(rows),
            spacing: Parameter::Literal(spacing),
            origin: Parameter::Literal(origin),
        }
    }

    pub fn repeat(count: f64, dx: f64, dy: f64) -> Self {
        Self::Repeat {
            count: Parameter::Literal(count),
            dx: Parameter::Literal(dx),
            dy: Parameter::Literal(dy),
        }
    }

    pub fn noise(amplitude: f64, frequency: f64, seed: f64) -> Self {
        Self::Noise {
            amplitude: Parameter::Literal(amplitude),
            frequency: Parameter::Literal(frequency),
            seed: Parameter::Literal(seed),
        }
    }

    pub fn smooth(iterations: f64, strength: f64) -> Self {
        Self::Smooth {
            iterations: Parameter::Literal(iterations),
            strength: Parameter::Literal(strength),
        }
    }
}

/// A node in the procedural graph: a kind, its wiring, and its operands.
///
/// `wires` maps an **input port** to the upstream output port that feeds it;
/// `operands` maps an **operand port** to its value. The two maps are keyed by
/// the port names [`ProceduralKind::inputs`] and [`ProceduralKind::operands`]
/// declare, which is the whole reason every node kind can share one command
/// set (`ConnectProcedural` / `SetProceduralOperand`) and one UI panel.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProceduralNode {
    pub id: NodeId,
    pub kind: ProceduralKind,
    /// The display name. Optional on the wire: a record added without one is
    /// named by the engine from its kind (`AddProceduralNode` fills it in), so a
    /// remote control never has to render `ProceduralKind::describe` itself.
    #[serde(default)]
    pub name: String,
    /// A parked node keeps its wiring and its place in the registry but
    /// contributes no geometry and publishes no values — the procedural
    /// analogue of parking a constraint or an operation.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Fill/stroke applied to the drawn result. Optional on the wire, defaulting
    /// to [`ProceduralNode::default_style`] — the same cool fill the engine
    /// gives a node it creates, so the two paths cannot disagree.
    #[serde(default = "ProceduralNode::default_style")]
    pub style: StyleProperties,
    /// Input port → upstream output port.
    #[serde(default)]
    pub wires: BTreeMap<PortId, NodeOutputId>,
    /// Operand port → value.
    #[serde(default)]
    pub operands: BTreeMap<PortId, ParamValue>,
}

/// `serde` default for a field that means "yes" when it is absent.
fn default_true() -> bool {
    true
}

impl ProceduralNode {
    pub fn new(id: NodeId, kind: ProceduralKind) -> Self {
        Self {
            id,
            name: kind.describe(),
            operands: kind.default_operands(),
            kind,
            enabled: true,
            style: Self::default_style(),
            wires: BTreeMap::new(),
        }
    }

    /// Procedural results default to a cool fill, so derived geometry reads as
    /// *derived* — the same idea as the operations layer's warm fill.
    pub fn default_style() -> StyleProperties {
        StyleProperties {
            fill: Parameter::Literal(Color::rgb(0x28, 0xb4, 0x8c)),
            ..StyleProperties::default()
        }
    }

    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }

    pub fn with_enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Wire an input port to an upstream output (builder form; the command is
    /// what validates).
    pub fn with_wire(mut self, port: impl Into<PortId>, from: NodeOutputId) -> Self {
        self.wires.insert(port.into(), from);
        self
    }

    /// Write an operand (builder form).
    pub fn with_operand(mut self, port: impl Into<PortId>, value: ParamValue) -> Self {
        self.operands.insert(port.into(), value);
        self
    }

    /// The value stored for `port`, falling back to the kind's default so an
    /// older document (or a hand-written record) can never leave a node
    /// unreadable.
    pub fn operand(&self, port: &str) -> Option<ParamValue> {
        self.operands
            .get(port)
            .cloned()
            .or_else(|| self.kind.default_operands().remove(port))
    }

    /// This node's output port type, wherever the port comes from.
    pub fn output_type(&self, port: &str) -> Option<PortType> {
        self.kind.output_type(port)
    }

    pub fn wire_for(&self, port: &str) -> Option<&NodeOutputId> {
        self.wires.get(port)
    }

    /// The upstream nodes this node reads directly.
    pub fn upstream(&self) -> Vec<NodeId> {
        let mut ids: Vec<NodeId> = self.wires.values().map(|from| from.node).collect();
        if let Some(source) = self.kind.source_node() {
            ids.push(source);
        }
        ids.sort();
        ids.dedup();
        ids
    }

    /// One-line description for the inspector, from the node's **effective**
    /// operands (engine-rendered, like constraints and operations: the UI never
    /// formats numbers of its own).
    ///
    /// A parked node says so, because "why is this not drawing?" is the first
    /// question a graph's author asks.
    pub fn describe(&self) -> String {
        let mut parts: Vec<String> = vec![self.kind.tag().to_string()];
        if let Some(subject) = self.kind.source_node() {
            parts.push(short(&subject));
        }
        for port in self.kind.operands() {
            let Some(value) = self.operand(&port.name) else {
                continue;
            };
            let label = ProceduralKind::port_label(&port.name);
            let rendered = match &value {
                ParamValue::Float(param) => match param {
                    Parameter::Literal(v) => format!("{v}"),
                    other => format!("({})", other.source_tag()),
                },
                ParamValue::Point(param) => match param {
                    Parameter::Literal(p) => format!("({}, {})", p.x, p.y),
                    other => format!("({})", other.source_tag()),
                },
                ParamValue::Color(param) => match param {
                    Parameter::Literal(c) => c.to_hex(),
                    other => format!("({})", other.source_tag()),
                },
            };
            parts.push(format!("{label}={rendered}"));
        }
        let mut text = parts.join(" ");
        if !self.enabled {
            text.push_str(" · parked");
        }
        text
    }

    /// True ⟺ the node is wired for every required input.
    pub fn is_wired(&self) -> bool {
        self.kind
            .inputs()
            .iter()
            .all(|port| !port.required || self.wires.contains_key(&port.name))
    }

    /// **RULE 1 + RULE 3 + port typing.** Everything a command must check before
    /// this record is stored anywhere.
    ///
    /// `registry` is consulted for wires (the upstream node and port must
    /// exist, and the chain must stay acyclic) — pass the registry the node is
    /// about to join, *without* this node in it, or with this node's previous
    /// record for an in-place edit.
    pub fn validate(&self, registry: &ProceduralRegistry) -> Result<(), VectraError> {
        // Operands: declared port, matching variant, and never a procedural
        // output (RULE 3).
        for (port, value) in &self.operands {
            let Some(ty) = self.kind.operand_type(port) else {
                return Err(VectraError::command(format!(
                    "procedural {} node {} has no operand port '{port}'",
                    self.kind.tag(),
                    short(&self.id)
                )));
            };
            if !ty.accepts_param(value) {
                return Err(VectraError::PropertyTypeMismatch {
                    property: format!("procedural.{}", port),
                    expected: ty.param_kind().unwrap_or("value"),
                    got: value.kind(),
                });
            }
            if param_reads_procedural(value) {
                return Err(VectraError::cyclic(format!(
                    "procedural {} node {} operand '{port}' reads a procedural output: \
                     a value reference back into the graph is a cycle — wire the upstream \
                     node's port instead",
                    self.kind.tag(),
                    short(&self.id)
                )));
            }
        }

        // Wires: declared input, existing upstream, typed match, acyclic chain.
        for (port, from) in &self.wires {
            let Some(expected) = self.kind.input_type(port) else {
                return Err(VectraError::command(format!(
                    "procedural {} node {} has no input port '{port}'",
                    self.kind.tag(),
                    short(&self.id)
                )));
            };
            let upstream = registry.get(from.node).ok_or_else(|| {
                VectraError::command(format!(
                    "procedural wire into {}:{port} reads {}, which is not a registered node",
                    short(&self.id),
                    short(&from.node)
                ))
            })?;
            let found = upstream.output_type(&from.port).ok_or_else(|| {
                VectraError::Resolve(ResolveError::ProceduralPortUnavailable {
                    node: from.node,
                    port: from.port.clone(),
                })
            })?;
            if found != expected {
                return Err(VectraError::Resolve(ResolveError::ProceduralPortType {
                    port: format!("{}:{port}", short(&self.id)),
                    expected: expected.tag(),
                    found: found.tag(),
                }));
            }
            if registry.would_close_cycle(self.id, from.node) {
                return Err(VectraError::cyclic(format!(
                    "procedural wire {}:{port} ← {} would close a cycle in the chain",
                    short(&self.id),
                    short(&from.node)
                )));
            }
        }
        Ok(())
    }
}

/// The procedural port this value reads, if any (RULE 3's ban, and the input to
/// the boundary-time cycle gate — a slot that reads a port is the only way a
/// value can flow *out* of the graph, so it is the only place a disguised cycle
/// can start).
pub fn param_procedural_ref(value: &ParamValue) -> Option<&NodeOutputId> {
    match value {
        ParamValue::Float(p) => match p {
            Parameter::Procedural(output) => Some(output),
            _ => None,
        },
        ParamValue::Point(p) => match p {
            Parameter::Procedural(output) => Some(output),
            _ => None,
        },
        ParamValue::Color(p) => match p {
            Parameter::Procedural(output) => Some(output),
            _ => None,
        },
    }
}

/// True ⟺ this value's parameter reads a procedural output (RULE 3's ban).
pub fn param_reads_procedural(value: &ParamValue) -> bool {
    param_procedural_ref(value).is_some()
}

/// First 8 hex characters of an id — enough to disambiguate in a message.
fn short(id: &NodeId) -> String {
    id.to_string().chars().take(8).collect()
}

/// A wire that pointed *at* a node, recorded so `RemoveProceduralNode` can
/// restore it exactly (its inverse is `AddProceduralNode` + these wires).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IncomingWire {
    pub node_id: NodeId,
    pub port: PortId,
    pub from: NodeOutputId,
}

/// A node withdrawn from the registry, with everything an exact inverse needs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RemovedProceduralNode {
    pub node: ProceduralNode,
    /// Wires from other nodes into this one's outputs, in deterministic order.
    pub incoming: Vec<IncomingWire>,
}

/// Insertion-ordered registry of procedural nodes.
///
/// `BTreeMap` for deterministic iteration (serialization, snapshot, export),
/// with `order` mirroring [`crate::document::Document::order`]: the draw order
/// of geometry-producing results, back → front.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ProceduralRegistry {
    pub nodes: BTreeMap<NodeId, ProceduralNode>,
    pub order: Vec<NodeId>,
}

impl ProceduralRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    pub fn get(&self, id: NodeId) -> Option<&ProceduralNode> {
        self.nodes.get(&id)
    }

    pub fn get_mut(&mut self, id: NodeId) -> Option<&mut ProceduralNode> {
        self.nodes.get_mut(&id)
    }

    pub fn contains(&self, id: NodeId) -> bool {
        self.nodes.contains_key(&id)
    }

    /// Register (or replace) a node, appending to draw order. Returns the
    /// previous record if the id was already present.
    pub fn insert(&mut self, node: ProceduralNode) -> Option<ProceduralNode> {
        let id = node.id;
        if !self.order.contains(&id) {
            self.order.push(id);
        }
        self.nodes.insert(id, node)
    }

    /// Withdraw a node, returning the record **and** the wires that pointed at
    /// it, so the caller can build an exact inverse.
    ///
    /// The incoming wires are *actually removed* from their consumers. A wire
    /// to a node that no longer exists is not a state the graph should hold:
    /// the consumer would keep resolving against an id that cannot publish, and
    /// the UI would show a live-looking connection to nothing. Undo re-adds the
    /// record and re-connects every address in `incoming`, which is why the
    /// inverse is exact — the same discipline `DeleteNode` already applies to
    /// constraints and operations.
    pub fn remove(&mut self, id: NodeId) -> Option<RemovedProceduralNode> {
        let node = self.nodes.remove(&id)?;
        self.order.retain(|existing| *existing != id);
        let incoming = self.incoming_wires(id);
        for wire in &incoming {
            if let Some(consumer) = self.nodes.get_mut(&wire.node_id) {
                consumer.wires.remove(&wire.port);
            }
        }
        Some(RemovedProceduralNode { node, incoming })
    }

    /// Every wire from any node into `id`'s outputs, in deterministic order.
    pub fn incoming_wires(&self, id: NodeId) -> Vec<IncomingWire> {
        let mut out: Vec<IncomingWire> = Vec::new();
        for node in self.in_order() {
            if node.id == id {
                continue; // a node never wires into itself: the gate forbids it
            }
            for (port, from) in &node.wires {
                if from.node == id {
                    out.push(IncomingWire {
                        node_id: node.id,
                        port: port.clone(),
                        from: from.clone(),
                    });
                }
            }
        }
        out
    }

    /// Ids of `Source` nodes that read `node` — the propagation step that makes
    /// editing a primitive re-run the graph that consumes it.
    pub fn sources_referencing(&self, node: NodeId) -> Vec<NodeId> {
        self.in_order()
            .filter(|n| n.kind.source_node() == Some(node))
            .map(|n| n.id)
            .collect()
    }

    /// Iterate nodes in draw order.
    pub fn in_order(&self) -> impl Iterator<Item = &ProceduralNode> {
        self.order.iter().filter_map(|id| self.nodes.get(id))
    }

    /// True ⟺ `id` contributes geometry right now (RULE 4): registered, enabled,
    /// and producing at least one geometry-typed port.
    pub fn is_geometry_id(&self, id: NodeId) -> bool {
        self.nodes
            .get(&id)
            .is_some_and(|n| n.enabled && n.kind.geometry_port().is_some())
    }

    /// The enabled geometry-producing nodes, in draw order (RULE 4).
    pub fn geometry_ids(&self) -> Vec<NodeId> {
        self.order
            .iter()
            .copied()
            .filter(|id| self.is_geometry_id(*id))
            .collect()
    }

    /// Every output port of every node, as `(node, port, type)` in draw order —
    /// the vertex set the dependency graph derives.
    pub fn output_ports(&self) -> Vec<(NodeId, PortId, PortType)> {
        let mut out = Vec::new();
        for node in self.in_order() {
            for port in node.kind.outputs() {
                out.push((node.id, port.name, port.ty));
            }
        }
        out
    }

    /// True ⟺ wiring `consumer ← upstream` would close a chain cycle, i.e.
    /// `upstream` already depends (directly or transitively) on `consumer`.
    ///
    /// The graph-level cycle gate (`DependencyGraph::dry_run`) catches the same
    /// thing through the derived edges; this one makes the *command*
    /// self-contained, because [`crate::command::Command::apply`] is public and
    /// law tests call it directly, with no graph in sight.
    pub fn would_close_cycle(&self, consumer: NodeId, upstream: NodeId) -> bool {
        if consumer == upstream {
            return true;
        }
        // Walk upstream's ancestors: if `consumer` is among them, the new edge
        // would close the loop.
        let mut seen: BTreeSet<NodeId> = BTreeSet::new();
        let mut stack: Vec<NodeId> = vec![upstream];
        while let Some(current) = stack.pop() {
            if current == consumer {
                return true;
            }
            if !seen.insert(current) {
                continue;
            }
            if let Some(node) = self.nodes.get(&current) {
                stack.extend(node.upstream());
            }
        }
        false
    }

    /// The chain in topological order (upstream first), restricted to `scope`.
    ///
    /// `scope` is the set of nodes the caller cares about; every node in it is
    /// emitted after all of its in-scope upstream ancestors. Nodes outside
    /// `scope` are traversed but not emitted, so a partial pass still sees the
    /// values it needs. Deterministic: ties break by registry order.
    pub fn topological_order(&self, scope: &[NodeId]) -> Vec<NodeId> {
        let wanted: BTreeSet<NodeId> = scope.iter().copied().collect();
        let mut emitted: Vec<NodeId> = Vec::new();
        let mut state: BTreeMap<NodeId, u8> = BTreeMap::new(); // 0 unseen, 1 visiting, 2 done
                                                               // Iterate in registry order so the output is stable across runs.
        for id in &self.order {
            self.visit_chain(*id, &wanted, &mut state, &mut emitted);
        }
        emitted
    }

    fn visit_chain(
        &self,
        id: NodeId,
        wanted: &BTreeSet<NodeId>,
        state: &mut BTreeMap<NodeId, u8>,
        emitted: &mut Vec<NodeId>,
    ) {
        match state.get(&id).copied().unwrap_or(0) {
            1 | 2 => return, // visiting (a cycle the gate forbids) or done
            _ => {}
        }
        state.insert(id, 1);
        if let Some(node) = self.nodes.get(&id) {
            for upstream in node.upstream() {
                self.visit_chain(upstream, wanted, state, emitted);
            }
        }
        state.insert(id, 2);
        if wanted.contains(&id) && self.nodes.contains_key(&id) {
            emitted.push(id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::param::Parameter;

    fn grid_node(id: NodeId) -> ProceduralNode {
        ProceduralNode::new(
            id,
            ProceduralKind::grid(3.0, 2.0, 10.0, Point2::new(0.0, 0.0)),
        )
    }

    fn smooth_node(id: NodeId) -> ProceduralNode {
        ProceduralNode::new(id, ProceduralKind::smooth(2.0, 0.5))
    }

    #[test]
    fn a_kinds_ports_are_typed_and_classified() {
        let grid = ProceduralKind::grid(3.0, 2.0, 10.0, Point2::ZERO);
        // Value ports are published, geometry ports are drawn.
        assert_eq!(grid.output_type("span"), Some(PortType::Scalar));
        assert_eq!(grid.output_type("center"), Some(PortType::Point));
        assert_eq!(grid.output_type("points"), Some(PortType::Points));
        assert_eq!(grid.output_type("region"), Some(PortType::Region));
        assert_eq!(grid.output_type("nope"), None);
        assert!(PortType::Region.is_geometry());
        assert!(!PortType::Scalar.is_geometry());
        // A generator has no wired input; a modifier has exactly one.
        assert!(grid.inputs().is_empty());
        assert_eq!(grid.geometry_port().as_deref(), Some("region"));
        let smooth = ProceduralKind::smooth(1.0, 0.5);
        assert_eq!(smooth.inputs().len(), 1);
        assert_eq!(smooth.input_type("region"), Some(PortType::Region));
    }

    #[test]
    fn a_param_value_must_match_its_port_type() {
        assert!(PortType::Scalar.accepts_param(&ParamValue::float_literal(1.0)));
        assert!(!PortType::Scalar.accepts_param(&ParamValue::point_literal(1.0, 2.0)));
        assert!(PortType::Point.accepts_param(&ParamValue::point_literal(1.0, 2.0)));
        assert!(PortType::Color.accepts_param(&ParamValue::color_literal(Color::WHITE)));
        assert!(!PortType::Color.accepts_param(&ParamValue::float_literal(1.0)));
        // Wire-only ports accept no param value at all.
        assert!(!PortType::Region.accepts_param(&ParamValue::float_literal(1.0)));
        assert_eq!(PortType::Region.param_kind(), None);
    }

    #[test]
    fn geometry_data_reports_its_own_type_and_emptiness() {
        let region = GeometryData::Region {
            rings: vec![vec![
                Point2::new(0.0, 0.0),
                Point2::new(1.0, 0.0),
                Point2::new(1.0, 1.0),
            ]],
        };
        assert_eq!(region.port_type(), PortType::Region);
        assert_eq!(region.kind_tag(), "region");
        assert_eq!(region.vertices().len(), 3);
        assert!(!region.is_empty_region());

        let degenerate = GeometryData::Region {
            rings: vec![vec![Point2::ZERO, Point2::new(1.0, 0.0)]],
        };
        assert!(degenerate.is_empty_region());

        assert_eq!(GeometryData::Scalar(2.5).as_scalar(), Some(2.5));
        assert_eq!(
            GeometryData::Point(Point2::new(1.0, 2.0)).as_point(),
            Some(Point2::new(1.0, 2.0))
        );
        assert_eq!(
            GeometryData::Color(Color::WHITE).as_color(),
            Some(Color::WHITE)
        );
        assert_eq!(GeometryData::Scalar(1.0).vertices().len(), 0);
        assert!(GeometryData::Scalar(1.0).rings().is_none());
        assert!(!GeometryData::Scalar(1.0).is_empty_region());
    }

    #[test]
    fn a_fresh_node_carries_its_kinds_defaults() {
        let node = grid_node(crate::ids::new_node_id());
        assert_eq!(node.operands.len(), 4);
        assert!(node.enabled);
        assert!(node.wires.is_empty());
        assert_eq!(
            node.operand("spacing"),
            Some(ParamValue::float_literal(10.0))
        );
        // A missing map entry still reads: the kind's default is the fallback.
        let mut stripped = node.clone();
        stripped.operands.clear();
        assert_eq!(
            stripped.operand("spacing"),
            Some(ParamValue::float_literal(10.0))
        );
        assert_eq!(stripped.operand("nope"), None);
        assert!(node.is_wired(), "a generator needs no wires");
        assert!(!smooth_node(crate::ids::new_node_id()).is_wired());
    }

    #[test]
    fn rule_three_rejects_a_value_reference_back_into_the_graph() {
        // An operand reading another node's procedural output is a cycle in
        // disguise; it must be refused with CyclicDependency, not accepted and
        // discovered later.
        let mut registry = ProceduralRegistry::new();
        let grid = grid_node(crate::ids::new_node_id());
        registry.insert(grid.clone());
        let mut node = smooth_node(crate::ids::new_node_id());
        node.operands.insert(
            "iterations".into(),
            ParamValue::Float(Parameter::Procedural(crate::param::NodeOutputId::new(
                grid.id, "span",
            ))),
        );
        let error = node.validate(&registry).unwrap_err();
        assert!(error.is_cycle_rejection(), "{error}");
        assert!(error.to_string().contains("cycle"), "{error}");
    }

    #[test]
    fn mismatched_port_types_are_a_typed_pre_application_rejection() {
        let mut registry = ProceduralRegistry::new();
        let grid = grid_node(crate::ids::new_node_id());
        registry.insert(grid.clone());
        // `points` is a Points port; a modifier's `region` input is a Region.
        let mut node = smooth_node(crate::ids::new_node_id())
            .with_wire("region", crate::param::NodeOutputId::new(grid.id, "points"));
        let error = node.validate(&registry).unwrap_err();
        match error {
            VectraError::Resolve(ResolveError::ProceduralPortType {
                expected, found, ..
            }) => {
                assert_eq!(expected, "region");
                assert_eq!(found, "points");
            }
            other => panic!("expected a port-type rejection, got {other}"),
        }
        // The same record with the *right* port validates.
        node.wires.clear();
        node.wires.insert(
            "region".into(),
            crate::param::NodeOutputId::new(grid.id, "region"),
        );
        assert!(node.validate(&registry).is_ok());
    }

    #[test]
    fn an_unknown_port_or_operand_is_a_typed_rejection() {
        let registry = ProceduralRegistry::new();
        let mut node = smooth_node(crate::ids::new_node_id());
        node.wires.insert(
            "no_such_port".into(),
            crate::param::NodeOutputId::new(crate::ids::new_node_id(), "region"),
        );
        assert!(node
            .validate(&registry)
            .unwrap_err()
            .to_string()
            .contains("no input port"));
        let mut node = smooth_node(crate::ids::new_node_id());
        node.operands
            .insert("nope".into(), ParamValue::float_literal(1.0));
        assert!(node
            .validate(&registry)
            .unwrap_err()
            .to_string()
            .contains("no operand port"));
    }

    #[test]
    fn the_chain_is_acyclic_and_cycles_are_detected_transitively() {
        let mut registry = ProceduralRegistry::new();
        let a = smooth_node(crate::ids::new_node_id());
        let b = smooth_node(crate::ids::new_node_id());
        registry.insert(a.clone());
        let mut wired_b = b.clone();
        wired_b.wires.insert(
            "region".into(),
            crate::param::NodeOutputId::new(a.id, "region"),
        );
        registry.insert(wired_b.clone());

        // Edge direction is (dependent → dependency). `b ← a` already exists,
        // so asking "would `b ← a` close a cycle?" is false, and asking
        // "would `a ← b` close one?" is true — that is the loop b → a → b.
        assert!(
            !registry.would_close_cycle(b.id, a.id),
            "re-asserting the existing wire is not a cycle"
        );
        assert!(
            registry.would_close_cycle(a.id, b.id),
            "a ← b closes a → b → a"
        );
        assert!(registry.would_close_cycle(b.id, b.id), "self");
        let mut closing = a.clone();
        closing.wires.insert(
            "region".into(),
            crate::param::NodeOutputId::new(b.id, "region"),
        );
        assert!(closing
            .validate(&registry)
            .unwrap_err()
            .is_cycle_rejection());
    }

    #[test]
    fn topological_order_is_upstream_first_and_scope_limited() {
        let mut registry = ProceduralRegistry::new();
        let a = smooth_node(crate::ids::new_node_id());
        let mut b = smooth_node(crate::ids::new_node_id());
        let mut c = smooth_node(crate::ids::new_node_id());
        let b_id = b.id;
        b.wires.insert(
            "region".into(),
            crate::param::NodeOutputId::new(a.id, "region"),
        );
        c.wires.insert(
            "region".into(),
            crate::param::NodeOutputId::new(b_id, "region"),
        );
        registry.insert(a.clone());
        registry.insert(b);
        registry.insert(c.clone());

        let order = registry.topological_order(&[a.id, b_id, c.id]);
        assert_eq!(order, vec![a.id, b_id, c.id], "upstream first");
        // A scope with only the tail still emits it after everything it needs.
        assert_eq!(registry.topological_order(&[c.id]), vec![c.id]);
    }

    #[test]
    fn the_live_id_rule_follows_enabled_and_kind() {
        let mut registry = ProceduralRegistry::new();
        let node = grid_node(crate::ids::new_node_id());
        registry.insert(node.clone());
        assert!(registry.is_geometry_id(node.id));
        assert_eq!(registry.geometry_ids(), vec![node.id]);
        // Parked: registered, but not live geometry (RULE 4's other half).
        registry.get_mut(node.id).unwrap().enabled = false;
        assert!(!registry.is_geometry_id(node.id));
        assert!(registry.geometry_ids().is_empty());
        assert!(registry.contains(node.id), "the record stays");
    }

    #[test]
    fn removing_a_node_reports_the_wires_into_it() {
        let mut registry = ProceduralRegistry::new();
        let grid = grid_node(crate::ids::new_node_id());
        registry.insert(grid.clone());
        let mut consumer = smooth_node(crate::ids::new_node_id());
        consumer.wires.insert(
            "region".into(),
            crate::param::NodeOutputId::new(grid.id, "region"),
        );
        registry.insert(consumer.clone());

        let removed = registry.remove(grid.id).expect("registered");
        assert_eq!(removed.node.id, grid.id);
        assert_eq!(removed.incoming.len(), 1);
        assert_eq!(removed.incoming[0].node_id, consumer.id);
        assert_eq!(removed.incoming[0].port, "region");
        assert!(!registry.contains(grid.id));
        assert!(
            registry.get(consumer.id).unwrap().wires.is_empty(),
            "the incoming wire is withdrawn with the node, and restored with it"
        );
    }

    #[test]
    fn a_source_node_names_its_subject() {
        let subject = crate::ids::new_node_id();
        let source = ProceduralNode::new(
            crate::ids::new_node_id(),
            ProceduralKind::Source { node: subject },
        );
        assert_eq!(source.kind.source_node(), Some(subject));
        assert_eq!(source.upstream(), vec![subject]);
        assert_eq!(source.kind.tag(), "source");
        assert!(source.kind.describe().starts_with("source"));
    }

    #[test]
    fn describe_covers_every_kind() {
        for kind in [
            ProceduralKind::Source {
                node: crate::ids::new_node_id(),
            },
            ProceduralKind::grid(2.0, 2.0, 5.0, Point2::ZERO),
            ProceduralKind::repeat(3.0, 10.0, 0.0),
            ProceduralKind::noise(4.0, 0.2, 7.0),
            ProceduralKind::smooth(2.0, 0.5),
        ] {
            let text = kind.describe();
            assert!(!text.is_empty());
            assert!(!kind.outputs().is_empty());
        }
        // An operand driven by a variable says so instead of printing a value.
        let mut kind = ProceduralKind::grid(2.0, 2.0, 5.0, Point2::ZERO);
        if let ProceduralKind::Grid { spacing, .. } = &mut kind {
            *spacing = Parameter::Variable("gap".into());
        }
        assert!(kind.describe().contains("variable"), "{}", kind.describe());
    }
}
