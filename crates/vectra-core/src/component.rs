//! **Smart Components** (Task 10.6, RULE 1): a master definition, instances
//! that reference it through the procedural graph, and Props that are live
//! parameters rather than copies.
//!
//! # The model in one paragraph
//!
//! A **master** is a [`ProceduralNode`] of kind
//! [`ProceduralKind::ComponentMaster`]: it names the nodes that make up the
//! artwork (*members*) and the [`ComponentProp`]s the master exposes. An
//! **instance** is a [`ProceduralNode`] of kind [`ProceduralKind::Component`]:
//! it names the master and the authored `Group` that holds *its own copy* of
//! the artwork (the clones), and carries the same prop list. Nothing is
//! conceptually duplicated: the clones are ordinary authored nodes, so they
//! render, layer, hit-test and export through the paths that already exist. The
//! only new thing is **where their parameters come from**.
//!
//! ```text
//!   ComponentMaster (procedural node, id = M)
//!     members = [rect, dot]              ← the artwork (authored nodes)
//!     props   = [size, stroke, radius, color]
//!
//!   Component (procedural node, id = I, group = G)     ← the instance
//!     master  = M
//!     group   = G                        ← the clone group (authored node)
//!
//!   G.children = [rect', dot']           ← clones
//!     rect'.width         = 1 * $icon_size         (expression)
//!     rect'.corner_radius = 0.1666 * $icon_size    (expression)
//!     rect'.style.fill    = proc I.color           (published port)
//! ```
//!
//! # Where a prop's value lives (RULE 1's "only that instance's variable")
//!
//! * A **scalar** prop's value is a document variable — `Parameter::Variable`
//!   on the member slots. Writing a prop on an instance writes *that instance's*
//!   variable; the dependency graph then dirties exactly the clone slots that
//!   read it and nothing else moves.
//! * A **colour** prop's value is the instance's own published colour port
//!   (`Parameter::Procedural(I.color)`), because [`Document::variables`] holds
//!   `f64`s and a colour has no variable to live in. The clone's `style.fill`
//!   is bound to that port, so the link is still a dependency-graph edge (a
//!   procedural vertex, which Task 7.0's graph models) and never a copied
//!   literal.
//!
//! Both halves are *links*. That is the law `law_a_prop_write_is_a_link`.
//!
//! # The two laws a prop can obey
//!
//! * [`PropLaw::Direct`] — the authored value.
//! * [`PropLaw::Scaled`] — the value is `factor × the scale prop`,
//!   materialised as one compiled expression per class of slot
//!   (`"0.0833333 * $icon_size"`). This is what keeps a 16px icon from growing
//!   hairline strokes (RULE 3): the stroke and the corner radius are not "set at
//!   generation time", they are *functions of the size*, so dragging the Size
//!   slider re-derives them — and re-deriving is one `DefineExpression`, not a
//!   recompute of the artwork.
//!
//! [`Document::variables`]: crate::document::Document::variables

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::document::{Document, Node, NodeKind};
use crate::error::VectraError;
use crate::geom::Color;
use crate::ids::{new_expression_id, ExpressionId, NodeId, VariableId};
use crate::param::{NodeOutputId, ParamValue, Parameter};
use crate::procedural::{PortType, ProceduralKind};

/// The type of value a prop carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PropType {
    /// One number — lives in a document variable.
    Scalar,
    /// One colour — lives in the owner's published colour port.
    Color,
}

impl PropType {
    pub fn tag(self) -> &'static str {
        match self {
            Self::Scalar => "scalar",
            Self::Color => "color",
        }
    }

    /// The [`PortType`] this prop publishes.
    pub fn port_type(self) -> PortType {
        match self {
            Self::Scalar => PortType::Scalar,
            Self::Color => PortType::Color,
        }
    }
}

/// How a prop's value relates to another prop of the same component.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum PropLaw {
    /// The value is authored: what the panel shows is what the document holds.
    Direct,
    /// The value is *derived*: `value = factor × <scale prop>`, materialised as
    /// a compiled expression on every slot this prop drives.
    ///
    /// Editing a scaled prop sets its factor (`factor = value / scale`), so the
    /// number the designer typed holds at the size they typed it and stays
    /// proportional everywhere else — RULE 3's "optically perfect at every
    /// size", expressed as arithmetic instead of a magic constant.
    Scaled { scale: String, factor: f64 },
}

impl PropLaw {
    pub fn is_scaled(&self) -> bool {
        matches!(self, Self::Scaled { .. })
    }

    /// The scale prop this law reads, if any.
    pub fn scale(&self) -> Option<&str> {
        match self {
            Self::Direct => None,
            Self::Scaled { scale, .. } => Some(scale),
        }
    }
}

/// One slot a prop drives **directly** — the value itself is written there.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct PropTarget {
    /// Index into the owner's member list.
    pub member: usize,
    /// A `SetParameter` property path on that member (`"style.fill"`).
    pub property: String,
}

/// One slot a prop drives through `factor × $<scale variable>` — a compiled
/// expression whose id is kept so re-weighting rewrites it in place.
///
/// A `Scaled` prop (`stroke_width`, `corner_radius`) has one factor for all of
/// its slots: the value the designer set, over the design size. A `Direct` prop
/// with proportional slots — the `size` prop, which drives every geometry slot —
/// needs one factor *per slot*, because a card's `x` and its `width` are not the
/// same fraction of the design. One variable, many fractions: that is what lets
/// a single slider resize a component without distorting it (RULE 1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScaledTarget {
    pub member: usize,
    pub property: String,
    /// The expression bound to that slot (`Parameter::Expression`).
    pub expression: ExpressionId,
    /// The slot's fraction of the design size (`value ÷ design size`).
    #[serde(default = "unit_factor")]
    pub factor: f64,
}

/// The default fraction: `1.0`, so a target written before this field existed
/// still means "this slot is the whole size".
fn unit_factor() -> f64 {
    1.0
}

/// One exposed Prop of a Smart Component.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ComponentProp {
    /// Stable command key (`"size"`, `"stroke_width"`, `"color"`).
    pub key: String,
    /// The panel's label (`"Size"`, `"Stroke"`, `"Color"`).
    pub label: String,
    pub ty: PropType,
    pub law: PropLaw,
    /// Slots written with the prop's value verbatim.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub targets: Vec<PropTarget>,
    /// Slots written with `factor × $scale`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub scaled: Vec<ScaledTarget>,
}

impl ComponentProp {
    pub fn new(key: impl Into<String>, label: impl Into<String>, ty: PropType) -> Self {
        Self {
            key: key.into(),
            label: label.into(),
            ty,
            law: PropLaw::Direct,
            targets: Vec::new(),
            scaled: Vec::new(),
        }
    }

    pub fn scalar(key: impl Into<String>, label: impl Into<String>) -> Self {
        Self::new(key, label, PropType::Scalar)
    }

    pub fn color(key: impl Into<String>, label: impl Into<String>) -> Self {
        Self::new(key, label, PropType::Color)
    }

    pub fn with_law(mut self, law: PropLaw) -> Self {
        self.law = law;
        self
    }

    /// The factor of a scaled prop (`None` for a direct one).
    pub fn factor(&self) -> Option<f64> {
        match &self.law {
            PropLaw::Direct => None,
            PropLaw::Scaled { factor, .. } => Some(*factor),
        }
    }

    /// This prop, re-written for a **new owner**.
    ///
    /// Placing an instance copies the master's props, but it must not copy the
    /// master's *expressions*: an expression id is one slot's binding, and two
    /// owners sharing one would fight over its source (the master's `2 × $c…`
    /// overwritten by the instance's `2 × $i…`). So every scaled target gets a
    /// fresh id, which [`bind_plan`] then binds to the new owner's variable.
    pub fn for_new_owner(&self) -> Self {
        let mut out = self.clone();
        for target in &mut out.scaled {
            target.expression = new_expression_id();
        }
        out
    }

    /// Every expression this prop owns, deduplicated.
    pub fn expressions(&self) -> Vec<ExpressionId> {
        let mut out: Vec<ExpressionId> = Vec::new();
        for target in &self.scaled {
            if !out.contains(&target.expression) {
                out.push(target.expression);
            }
        }
        out
    }

    /// True ⟺ this prop drives `property` on `member`.
    pub fn drives(&self, member: usize, property: &str) -> bool {
        self.targets
            .iter()
            .any(|t| t.member == member && t.property == property)
            || self
                .scaled
                .iter()
                .any(|t| t.member == member && t.property == property)
    }
}

/// The prop list and the variable map a component node carries.
///
/// Shared by the master and the instance, so one inspector, one serde shape and
/// one set of laws cover both.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct ComponentSpec {
    pub props: Vec<ComponentProp>,
    /// The variable each **scalar** prop's value lives in, keyed by prop.
    ///
    /// A colour prop has no entry: its value lives in the node's operands, where
    /// the published colour port reads it. Keeping the mapping explicit makes
    /// the write path a lookup rather than a name-guess.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub variables: BTreeMap<String, VariableId>,
}

/// The prop a set of props scales from — the one rule, in one place, so that the
/// binder (which creates the variable) and `InstantiateComponent` (which seeds
/// it from the master) can never disagree about which prop that is.
pub fn scale_key(props: &[ComponentProp]) -> Option<&str> {
    props.iter().find_map(|prop| match prop.law.scale() {
        Some(key) => Some(key),
        None if !prop.scaled.is_empty() => Some(prop.key.as_str()),
        None => None,
    })
}

impl ComponentSpec {
    pub fn get(&self, key: &str) -> Option<&ComponentProp> {
        self.props.iter().find(|p| p.key == key)
    }

    pub fn get_mut(&mut self, key: &str) -> Option<&mut ComponentProp> {
        self.props.iter_mut().find(|p| p.key == key)
    }

    pub fn variable(&self, key: &str) -> Option<&VariableId> {
        self.variables.get(key)
    }

    /// The prop this component's scale comes from (`"size"` in the inferred set).
    ///
    /// Either a prop another prop explicitly scales *from* (`stroke_width` ←
    /// `size`), or — because the inferred `size` prop is `Direct` and drives its
    /// own slots — the prop that drives scaled targets of its own.
    pub fn scale_key(&self) -> Option<&str> {
        scale_key(&self.props)
    }

    /// Insert or replace a prop, preserving order.
    pub fn upsert(&mut self, prop: ComponentProp) {
        match self.props.iter_mut().find(|p| p.key == prop.key) {
            Some(existing) => *existing = prop,
            None => self.props.push(prop),
        }
    }
}

/// One `SetParameter` write a creation command must perform for a new owner.
#[derive(Debug, Clone, PartialEq)]
pub struct SlotWrite {
    pub member: usize,
    pub property: String,
    pub value: ParamValue,
}

/// The plan for one owner (a master or a fresh instance): the writes that bind
/// its members' slots, the expressions that must be defined, and the variables
/// that must exist.
///
/// This is the **pure** half of component creation. A command applies it; a test
/// asserts it. Nothing here touches a document.
#[derive(Debug, Clone, PartialEq)]
pub struct BindPlan {
    /// `Variable` / `Expression` / `Procedural` writes, in prop order.
    pub writes: Vec<SlotWrite>,
    /// Expression id → source, for every scaled slot.
    pub expressions: Vec<(ExpressionId, String)>,
    /// Variable name → initial value, for every direct scalar prop.
    pub variables: Vec<(VariableId, f64)>,
    /// The spec (props + variable map) that will describe the owner.
    pub spec: ComponentSpec,
}

impl BindPlan {
    /// A stable digest of what this plan binds — the shape a test compares when
    /// it wants "the same binding" without comparing ids.
    pub fn describe(&self) -> Vec<String> {
        self.writes
            .iter()
            .map(|write| {
                format!(
                    "{}:{} <- {}",
                    write.member,
                    write.property,
                    value_tag(&write.value)
                )
            })
            .collect()
    }
}

/// A parameter's source tag, for plan digests and panel text.
pub fn value_tag(value: &ParamValue) -> String {
    match value {
        ParamValue::Float(param) => param.source_tag().to_string(),
        ParamValue::Point(param) => param.source_tag().to_string(),
        ParamValue::Color(param) => param.source_tag().to_string(),
    }
}

// ── naming ───────────────────────────────────────────────────────────────

/// `"card"`-style slug for an id: eight hex digits of its uuid.
pub fn slug(id: NodeId) -> String {
    id.to_string()
        .chars()
        .filter(|c| c.is_ascii_hexdigit())
        .take(8)
        .collect()
}

/// A variable name for `prefix`/`key` that is free in `doc`.
///
/// Deterministic, readable inside an expression (`$icon_size`), and
/// deduplicated — two owners collide only if their hints do, and the numeric
/// suffix keeps even that case honest.
pub fn free_variable(doc: &Document, prefix: &str, key: &str) -> VariableId {
    let base = format!("{prefix}_{key}");
    let mut candidate = base.clone();
    let mut n = 2;
    while doc.variables.contains_key(&candidate) {
        candidate = format!("{base}{n}");
        n += 1;
    }
    VariableId::from(candidate)
}

/// A free expression id (expressions are keyed by uuid, so "free" is trivial —
/// the function exists so call sites read the same way as [`free_variable`]).
pub fn free_expression() -> ExpressionId {
    new_expression_id()
}

/// The source of a scaled slot: `"0.0833333 * $icon_size"`.
pub fn scale_source(factor: f64, scale_variable: &VariableId) -> String {
    format!("{} * ${}", trim_number(factor), scale_variable)
}

/// Numbers in engine-authored sources read like every other engine string: no
/// trailing `.0`.
pub fn trim_number(value: f64) -> String {
    format!("{value}")
}

// ── reading a component ───────────────────────────────────────────────────

/// The authored nodes a component node defines its artwork with: a master's
/// members, or an instance's cloned children.
pub fn members_of(doc: &Document, id: NodeId) -> Vec<NodeId> {
    match doc.procedural.get(id).map(|node| &node.kind) {
        Some(ProceduralKind::ComponentMaster { members, .. }) => members.clone(),
        Some(ProceduralKind::Component { group, .. }) => match doc.nodes.get(group) {
            Some(Node {
                kind: NodeKind::Group { children },
                ..
            }) => children.clone(),
            _ => Vec::new(),
        },
        _ => Vec::new(),
    }
}

/// The component a piece of authored artwork belongs to: the master whose
/// members include it, or the instance whose clones do.
///
/// A member is *listed* by its component rather than wired to it — a wire carries
/// a value, and membership is not a value — so this is a scan of the registry's
/// own order, not a graph lookup.
pub fn owner_of(doc: &Document, id: NodeId) -> Option<NodeId> {
    doc.procedural
        .in_order()
        .filter(|node| {
            matches!(
                node.kind,
                ProceduralKind::ComponentMaster { .. } | ProceduralKind::Component { .. }
            )
        })
        .find(|node| members_of(doc, node.id).contains(&id))
        .map(|node| node.id)
}

/// The master an instance references.
pub fn master_of(doc: &Document, id: NodeId) -> Option<NodeId> {
    match doc.procedural.get(id).map(|node| &node.kind) {
        Some(ProceduralKind::Component { master, .. }) => Some(*master),
        _ => None,
    }
}

/// The instance's clone group, if it is an instance.
pub fn group_of(doc: &Document, id: NodeId) -> Option<NodeId> {
    match doc.procedural.get(id).map(|node| &node.kind) {
        Some(ProceduralKind::Component { group, .. }) => Some(*group),
        _ => None,
    }
}

/// The component spec a component node carries, whichever half it is.
pub fn spec_of(doc: &Document, id: NodeId) -> Option<&ComponentSpec> {
    match doc.procedural.get(id).map(|node| &node.kind) {
        Some(ProceduralKind::ComponentMaster { spec, .. })
        | Some(ProceduralKind::Component { spec, .. }) => Some(spec),
        _ => None,
    }
}

/// Every instance of `master`, in registry order.
pub fn instances_of(doc: &Document, master: NodeId) -> Vec<NodeId> {
    doc.procedural
        .in_order()
        .filter(|node| master_of(doc, node.id) == Some(master))
        .map(|node| node.id)
        .collect()
}

/// True ⟺ `id` is a component master.
pub fn is_master(doc: &Document, id: NodeId) -> bool {
    matches!(
        doc.procedural.get(id).map(|node| &node.kind),
        Some(ProceduralKind::ComponentMaster { .. })
    )
}

/// True ⟺ `id` is a component instance.
pub fn is_instance(doc: &Document, id: NodeId) -> bool {
    master_of(doc, id).is_some()
}

/// The scalar geometry slots a component's `size` prop scales.
///
/// Positions (`x`, `y`, `cx`, `cy`) and magnitudes (`width`, `height`,
/// `radius`) — the member's *geometry*, never its paint. Angles are left alone:
/// a 45° corner is a design decision, not a length.
pub fn geometry_slots(kind: &NodeKind) -> Vec<&'static str> {
    match kind {
        NodeKind::Rectangle { .. } => vec!["x", "y", "width", "height"],
        NodeKind::Circle { .. } => vec!["cx", "cy", "radius"],
        NodeKind::Arc { .. } => vec!["cx", "cy", "radius"],
        // A text node's `size` prop is its **type size**, not a box: the run's
        // box is an output of layout, so scaling it means scaling `font_size`
        // (and, with it, the leading — the leading is a multiple of the size).
        // `x`/`y` come along so an instance's type stays inside the instance.
        // A *bound* run has no origin of its own (the path places it), so it
        // contributes only its size.
        NodeKind::Text { on_path, .. } => match on_path {
            None => vec!["x", "y", "font_size"],
            Some(_) => vec!["font_size"],
        },
        NodeKind::Path { .. } | NodeKind::Group { .. } => Vec::new(),
    }
}

fn float_at(doc: &Document, member: NodeId, property: &str) -> Option<f64> {
    let node = doc.nodes.get(&member)?;
    match node.get_param(property) {
        Ok(ParamValue::Float(Parameter::Literal(value))) => Some(value),
        _ => None,
    }
}

fn color_at(doc: &Document, member: NodeId, property: &str) -> Option<Color> {
    let node = doc.nodes.get(&member)?;
    match node.get_param(property) {
        Ok(ParamValue::Color(Parameter::Literal(color))) => Some(color),
        _ => None,
    }
}

/// The design size of a member set: the largest magnitude any member carries —
/// `24.0` for a 24×24 rounded rectangle.
///
/// A set with no magnitude at all falls back to `1.0`: a scale of one is the
/// identity, and it is the only value that cannot distort artwork.
pub fn design_size(doc: &Document, members: &[NodeId]) -> f64 {
    let mut size = 0.0f64;
    for member in members {
        let Some(node) = doc.nodes.get(member) else {
            continue;
        };
        for slot in geometry_slots(&node.kind) {
            if matches!(slot, "x" | "y" | "cx" | "cy") {
                continue;
            }
            if let Some(value) = float_at(doc, *member, slot) {
                size = size.max(value.abs());
            }
        }
        if let Some(radius) = float_at(doc, *member, "radius") {
            size = size.max(2.0 * radius.abs());
        }
    }
    if size > 0.0 {
        size
    } else {
        1.0
    }
}

/// **Prop inference** — RULE 1's "exposes Props (e.g. Size, Color,
/// CornerRadius)".
///
/// Four props, in this order, because those are the four a designer reaches for:
///
/// | prop | type | law | what it drives |
/// |---|---|---|---|
/// | `size` | scalar | direct | every member's geometry, scaled by `value / design size` |
/// | `stroke_width` | scalar | scaled | every member's `style.stroke_width` |
/// | `corner_radius` | scalar | scaled | every rectangle member's `corner_radius` |
/// | `color` | colour | direct | every member's `style.fill` |
///
/// The scaled props are what make a component *scale* rather than merely
/// shrink: a 2px stroke on a 24px icon reads `0.0833 × size`, which is `1.33` at
/// 16px and never a hairline. Props no member can carry (a corner radius on a
/// circle-only selection) are omitted rather than offered as dead controls.
pub fn infer_props(doc: &Document, members: &[NodeId]) -> Vec<ComponentProp> {
    let size = design_size(doc, members);
    let mut props: Vec<ComponentProp> = Vec::new();

    // size: every geometry slot, scaled.
    let mut size_prop = ComponentProp::scalar("size", "Size");
    for (index, member) in members.iter().enumerate() {
        let Some(node) = doc.nodes.get(member) else {
            continue;
        };
        for slot in geometry_slots(&node.kind) {
            let Some(value) = float_at(doc, *member, slot) else {
                continue;
            };
            if value == 0.0 {
                continue; // a zero stays zero; binding it adds noise, not control
            }
            size_prop.scaled.push(ScaledTarget {
                member: index,
                property: slot.to_string(),
                expression: new_expression_id(),
                factor: value / size,
            });
        }
    }
    props.push(size_prop);

    // stroke_width: scaled — one expression, shared by every member it drives.
    let stroke_width = members
        .iter()
        .filter_map(|member| float_at(doc, *member, "style.stroke_width"))
        .fold(0.0f64, |acc, value| acc.max(value.abs()));
    if stroke_width > 0.0 {
        let expression = new_expression_id();
        let mut stroke =
            ComponentProp::scalar("stroke_width", "Stroke").with_law(PropLaw::Scaled {
                scale: "size".to_string(),
                factor: stroke_width / size,
            });
        for (index, member) in members.iter().enumerate() {
            if float_at(doc, *member, "style.stroke_width").unwrap_or(0.0) == 0.0 {
                continue;
            }
            stroke.scaled.push(ScaledTarget {
                member: index,
                property: "style.stroke_width".to_string(),
                expression,
                factor: stroke_width / size,
            });
        }
        props.push(stroke);
    }

    // corner_radius: scaled, for rectangle members only.
    let corner_radius = members
        .iter()
        .filter(|member| {
            doc.nodes
                .get(member)
                .map(|node| matches!(node.kind, NodeKind::Rectangle { .. }))
                .unwrap_or(false)
        })
        .filter_map(|member| float_at(doc, *member, "corner_radius"))
        .fold(0.0f64, |acc, value| acc.max(value.abs()));
    if corner_radius > 0.0 {
        let expression = new_expression_id();
        let mut corner =
            ComponentProp::scalar("corner_radius", "Corner radius").with_law(PropLaw::Scaled {
                scale: "size".to_string(),
                factor: corner_radius / size,
            });
        for (index, member) in members.iter().enumerate() {
            let Some(node) = doc.nodes.get(member) else {
                continue;
            };
            if !matches!(node.kind, NodeKind::Rectangle { .. }) {
                continue;
            }
            if float_at(doc, *member, "corner_radius").unwrap_or(0.0) == 0.0 {
                continue;
            }
            corner.scaled.push(ScaledTarget {
                member: index,
                property: "corner_radius".to_string(),
                expression,
                factor: corner_radius / size,
            });
        }
        props.push(corner);
    }

    // color: the members' fill, verbatim.
    let mut color = ComponentProp::color("color", "Color");
    for (index, member) in members.iter().enumerate() {
        if color_at(doc, *member, "style.fill").is_some() {
            color.targets.push(PropTarget {
                member: index,
                property: "style.fill".to_string(),
            });
        }
    }
    if !color.targets.is_empty() {
        props.push(color);
    }

    props
}

/// The colour a `color` prop shows by default: the first member's fill.
pub fn color_default(doc: &Document, members: &[NodeId]) -> Color {
    members
        .iter()
        .find_map(|member| color_at(doc, *member, "style.fill"))
        .unwrap_or_else(|| Color::rgb(0x22, 0x66, 0xee))
}

// ── binding ───────────────────────────────────────────────────────────────

/// **The binding plan** for one owner — the pure half of `CreateComponent` and
/// `InstantiateComponent`.
///
/// `owner` is the id the plan's colour ports will point at (the caller knows it
/// before the node exists), `prefix` the owner's variable hint, and `seed` the
/// value the scale prop starts at (a fresh master: the design size; an instance:
/// whatever the master currently holds).
///
/// Every prop's slots are resolved to a *source*, never a copy:
///
///  * scaled scalars → `Parameter::Expression` (`factor * $<prefix>_size`)
///  * direct scalars → `Parameter::Variable(<prefix>_<key>)`
///  * direct colours → `Parameter::Procedural(<owner>.<key>)`
///
/// The variable-name prefix a **master**'s bindings use (`c<slug>`).
///
/// The name matters: it is what a designer sees in the variable list, and it is
/// what keeps two components' props from colliding. Both the command and the
/// dependency graph's dry run need the same one, so it is defined once.
pub fn command_prefix(owner: NodeId) -> String {
    format!("c{}", slug(owner))
}

/// The variable-name prefix an **instance**'s bindings use (`i<slug>`).
pub fn instance_prefix(owner: NodeId) -> String {
    format!("i{}", slug(owner))
}

pub fn bind_plan(
    doc: &Document,
    owner: NodeId,
    members: &[NodeId],
    prefix: &str,
    props: &[ComponentProp],
    seed: Option<f64>,
) -> BindPlan {
    let design = design_size(doc, members);
    let mut plan = BindPlan {
        writes: Vec::new(),
        expressions: Vec::new(),
        variables: Vec::new(),
        spec: ComponentSpec::default(),
    };

    // The scale prop's variable must exist before any scaled source names it.
    //
    // A `Scaled` prop names its scale explicitly (`stroke_width` scales from
    // `size`). A *direct* prop that drives proportional slots is its own scale:
    // the `size` prop is one variable and every geometry slot reads a fraction
    // of it, which is why a component resizes instead of distorting.
    let scale_key: Option<String> = scale_key(props).map(str::to_string);
    let scale_variable = scale_key.as_deref().map(|key| {
        let name = free_variable(doc, prefix, key);
        let value = seed.unwrap_or(design);
        plan.variables.push((name.clone(), value));
        plan.spec.variables.insert(key.to_string(), name.clone());
        name
    });

    for prop in props {
        let mut bound = prop.clone();
        bound.scaled.clear();

        if let (PropLaw::Scaled { factor, .. }, Some(scale_variable)) =
            (&prop.law, scale_variable.as_ref())
        {
            let source = scale_source(*factor, scale_variable);
            for target in &prop.scaled {
                bound.scaled.push(target.clone());
                if !plan
                    .expressions
                    .iter()
                    .any(|(id, _)| *id == target.expression)
                {
                    plan.expressions.push((target.expression, source.clone()));
                }
                plan.writes.push(SlotWrite {
                    member: target.member,
                    property: target.property.clone(),
                    value: ParamValue::Float(Parameter::Expression(target.expression)),
                });
            }
        }

        match prop.ty {
            PropType::Scalar => {
                if let PropLaw::Direct = prop.law {
                    // The scale prop's variable was created before the loop (a
                    // scaled source has to name it); every other direct scalar
                    // prop mints its own.
                    let name = if Some(prop.key.as_str()) == scale_key.as_deref() {
                        scale_variable
                            .clone()
                            .unwrap_or_else(|| free_variable(doc, prefix, &prop.key))
                    } else {
                        let name = free_variable(doc, prefix, &prop.key);
                        let value = default_scalar(doc, members, &prop.key);
                        plan.variables.push((name.clone(), value));
                        name
                    };
                    plan.spec.variables.insert(prop.key.clone(), name.clone());
                    for target in &prop.targets {
                        plan.writes.push(SlotWrite {
                            member: target.member,
                            property: target.property.clone(),
                            value: ParamValue::Float(Parameter::Variable(name.clone())),
                        });
                    }
                    // …and the slots this prop drives *proportionally*, each
                    // through its own compiled `fraction × $variable`. The target
                    // keeps the expression id, so re-writing a prop of this kind
                    // is a source edit rather than a re-bind.
                    if let Some(scale_variable) = scale_variable.as_ref() {
                        for target in &prop.scaled {
                            if !plan
                                .expressions
                                .iter()
                                .any(|(id, _)| *id == target.expression)
                            {
                                plan.expressions.push((
                                    target.expression,
                                    scale_source(target.factor, scale_variable),
                                ));
                            }
                            bound.scaled.push(target.clone());
                            plan.writes.push(SlotWrite {
                                member: target.member,
                                property: target.property.clone(),
                                value: ParamValue::Float(Parameter::Expression(target.expression)),
                            });
                        }
                    }
                }
            }
            PropType::Color => {
                // The value is read through the owner's own published port.
                for target in &prop.targets {
                    plan.writes.push(SlotWrite {
                        member: target.member,
                        property: target.property.clone(),
                        value: ParamValue::Color(Parameter::Procedural(NodeOutputId::new(
                            owner,
                            prop.key.clone(),
                        ))),
                    });
                }
            }
        }

        plan.spec.upsert(bound);
    }

    plan
}

/// The value a **direct scalar** prop starts at: the largest number the slots
/// the document exposes for it currently hold.
fn default_scalar(doc: &Document, members: &[NodeId], key: &str) -> f64 {
    match key {
        "size" => design_size(doc, members),
        "stroke_width" => members
            .iter()
            .filter_map(|member| float_at(doc, *member, "style.stroke_width"))
            .fold(0.0f64, |acc, value| acc.max(value.abs())),
        "corner_radius" => members
            .iter()
            .filter_map(|member| float_at(doc, *member, "corner_radius"))
            .fold(0.0f64, |acc, value| acc.max(value.abs())),
        _ => design_size(doc, members),
    }
}

/// The operand values a new component node starts with: one per prop.
///
/// Scalar props read their variable (the instance's own); colour props carry the
/// seed colour the master holds now. This is *the* moment a prop value is
/// created for an owner — nothing else writes operands except
/// `SetComponentProp`.
pub fn seed_operands(spec: &ComponentSpec, color: Color) -> BTreeMap<String, ParamValue> {
    let mut operands: BTreeMap<String, ParamValue> = BTreeMap::new();
    for prop in &spec.props {
        match prop.ty {
            PropType::Scalar => {
                if let Some(variable) = spec.variable(&prop.key) {
                    operands.insert(
                        prop.key.clone(),
                        ParamValue::Float(Parameter::Variable(variable.clone())),
                    );
                } else if let Some(target) = prop.scaled.first() {
                    // A scaled prop has no variable of its own: it *is*
                    // `factor × $scale`, so the port republishes that expression
                    // and a reader of the port sees the number on screen.
                    operands.insert(
                        prop.key.clone(),
                        ParamValue::Float(Parameter::Expression(target.expression)),
                    );
                }
            }
            PropType::Color => {
                operands.insert(
                    prop.key.clone(),
                    ParamValue::Color(Parameter::Literal(color)),
                );
            }
        }
    }
    operands
}

/// **The Icon Studio macro** (RULE 3): one master, N optically-correct sizes,
/// each on its own artboard.
///
/// For every size the plan emits:
///
/// 1. `CreateArtboard` — a square board of exactly that size, laid out left to
///    right with a gutter so the set reads as a sheet.
/// 2. `CreateLayer` — a layer on that board (which makes it the active layer, so
///    the instance's clones land on the board they belong to).
/// 3. `InstantiateComponent` — a full copy of the master's artwork.
/// 4. `SetComponentProp { prop: "size" }` — the instance's own size variable.
///    Because `stroke_width` and `corner_radius` are *scaled* props, that one
///    write re-derives both through their compiled expressions: the 2px stroke
///    of a 24px master is `1.333` at 16px, not a hairline.
///
/// Nothing here is special-cased for icons. The macro is a *planner*, and every
/// command it emits is a command the UI could have clicked — which is why
/// `law_the_icon_scaling_law` (in `crates/vectra-wasm/tests/component_laws.rs`)
/// can assert the whole thing without a renderer.
pub fn icon_set_plan(
    doc: &Document,
    master: NodeId,
    sizes: &[f64],
) -> Result<Vec<crate::command::Command>, VectraError> {
    use crate::command::Command;

    let node = doc
        .procedural
        .get(master)
        .ok_or_else(|| VectraError::command(format!("no component master {master}")))?;
    let ProceduralKind::ComponentMaster { spec, .. } = &node.kind else {
        return Err(VectraError::command(format!(
            "{master} is not a component master"
        )));
    };
    if !spec.props.iter().any(|prop| prop.key == "size") {
        return Err(VectraError::command(format!(
            "component master {master} exposes no `size` prop to scale"
        )));
    }
    if let Some(invalid) = sizes.iter().find(|size| !size.is_finite() || **size <= 0.0) {
        return Err(VectraError::command(format!(
            "icon sizes must be positive and finite (got {invalid})"
        )));
    }

    let base = node.name.clone();
    let mut commands: Vec<Command> = Vec::new();
    let mut cursor = 0.0f64;
    for size in sizes {
        let board = crate::ids::new_artboard_id();
        commands.push(Command::CreateArtboard {
            id: board,
            name: format!("{base} {}", trim_number(*size)),
            x: cursor,
            y: 0.0,
            width: *size,
            height: *size,
            background: Color::WHITE,
        });
        commands.push(Command::CreateLayer {
            id: crate::ids::new_layer_id(),
            name: format!("{base} {}", trim_number(*size)),
            index: None,
            artboard: Some(board),
        });
        let instance = crate::ids::new_node_id();
        commands.push(Command::InstantiateComponent {
            id: instance,
            master,
            name: Some(format!("{base} {}px", trim_number(*size))),
            index: None,
        });
        commands.push(Command::SetComponentProp {
            target: instance,
            prop: "size".to_string(),
            value: ParamValue::Float(Parameter::Literal(*size)),
        });
        cursor += size + ICON_GUTTER;
    }
    Ok(commands)
}

/// The gutter between generated icon artboards, in document units.
pub const ICON_GUTTER: f64 = 16.0;

/// The classic icon ladder an "icon set" means when nothing else is said.
pub const DEFAULT_ICON_SIZES: [f64; 3] = [16.0, 32.0, 48.0];

// ── the designer-facing view (RULE 4) ─────────────────────────────────────

/// One prop, as the Smart Component panel shows it: a label, a kind, a value and
/// whether that value is derived. Never a graph, never JSON.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PropView {
    pub key: String,
    pub label: String,
    /// `"scalar"` or `"color"`.
    pub ty: &'static str,
    /// `"direct"` or `"scaled"`.
    pub law: &'static str,
    /// The prop's current value, resolved through the document.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<f64>,
    /// The colour, as `#rrggbb`, for a colour prop.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    /// True ⟺ the value is derived from another prop (a slider still edits it —
    /// it edits the *factor*).
    pub derived: bool,
    /// The prop it is derived from.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    /// What a slider should span, from the document's own numbers.
    pub min: f64,
    pub max: f64,
}

/// What the panel needs to know about the current selection.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ComponentView {
    /// `"master"`, `"instance"`, `"selection"` or `"none"`.
    pub role: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The master an instance references.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub master: Option<String>,
    pub props: Vec<PropView>,
    /// How many instances exist for this master (masters only).
    pub instances: usize,
    /// True ⟺ "Create Component" makes sense for this selection.
    pub can_create: bool,
    /// A short, human sentence the panel prints verbatim.
    pub headline: String,
}

/// Inspect a selection: which component role it plays, and every prop it exposes.
///
/// The `selection` may be a component node id itself, one or more member/clone
/// ids, or any authored selection — a clone is mapped back to its instance, an
/// instance to itself, and anything else to "this could become a component".
pub fn inspect(doc: &Document, selection: &[NodeId]) -> ComponentView {
    let component = selection
        .iter()
        .find_map(|id| {
            if is_master(doc, *id) || is_instance(doc, *id) {
                Some(*id)
            } else {
                None
            }
        })
        .or_else(|| {
            // A clone (or a member) answers through its owner: the instance or
            // master that lists it. A *clone* is additionally wired to its
            // component's output ports (the procedural edge), so the wire is
            // checked first and the membership scan is the total fallback.
            selection
                .iter()
                .find_map(|id| doc.procedural.sources_referencing(*id).into_iter().next())
                .filter(|id| is_master(doc, *id) || is_instance(doc, *id))
                .or_else(|| {
                    selection.iter().find_map(|id| {
                        owner_of(doc, *id)
                            .filter(|owner| is_master(doc, *owner) || is_instance(doc, *owner))
                    })
                })
        });

    let Some(id) = component else {
        return ComponentView {
            role: if selection.is_empty() {
                "none"
            } else {
                "selection"
            },
            id: None,
            name: None,
            master: None,
            props: Vec::new(),
            instances: 0,
            can_create: !selection.is_empty(),
            headline: if selection.is_empty() {
                "Select artwork to create a component.".to_string()
            } else {
                format!(
                    "✨ {} shape(s) selected — ready to become a component.",
                    selection.len()
                )
            },
        };
    };

    let name = doc
        .procedural
        .get(id)
        .map(|node| node.name.clone())
        .unwrap_or_else(|| slug(id));
    let is_master_here = is_master(doc, id);
    let master = master_of(doc, id);
    let instances = if is_master_here {
        instances_of(doc, id).len()
    } else {
        0
    };
    let mut props = Vec::new();
    if let Some(spec) = spec_of(doc, id) {
        for prop in &spec.props {
            props.push(prop_view(doc, id, spec, prop));
        }
    }
    let headline = if is_master_here {
        format!(
            "Component '{}' — {} prop(s), {} instance(s).",
            name,
            props.len(),
            instances
        )
    } else {
        format!(
            "Instance '{}' of '{}' — {} prop(s).",
            name,
            master
                .and_then(|master| doc.procedural.get(master).map(|node| node.name.clone()))
                .unwrap_or_else(|| "?".to_string()),
            props.len()
        )
    };
    ComponentView {
        role: if is_master_here { "master" } else { "instance" },
        id: Some(id.to_string()),
        name: Some(name),
        master: master.map(|master| master.to_string()),
        props,
        instances,
        can_create: false,
        headline,
    }
}

/// One prop's current value, resolved through the document (so a scaled prop
/// shows `factor × size`, the number the designer sees on screen).
pub fn prop_view(
    doc: &Document,
    owner: NodeId,
    spec: &ComponentSpec,
    prop: &ComponentProp,
) -> PropView {
    let derived = prop.law.is_scaled();
    let from = prop.law.scale().map(str::to_string);
    let scale_value = prop
        .law
        .scale()
        .and_then(|key| spec.variable(key))
        .and_then(|variable| doc.variables.get(variable).copied());
    let (value, color) = match prop.ty {
        PropType::Scalar => {
            let raw = prop
                .factor()
                .zip(scale_value)
                .map(|(factor, scale)| factor * scale)
                .or_else(|| {
                    spec.variable(&prop.key)
                        .and_then(|variable| doc.variables.get(variable).copied())
                });
            (raw, None)
        }
        PropType::Color => {
            let color = doc
                .procedural
                .get(owner)
                .and_then(|node| node.operand(&prop.key))
                .and_then(|value| match value {
                    // A literal, or nothing to show: a colour that is a variable
                    // or a procedural input is reported by its *name* elsewhere.
                    ParamValue::Color(Parameter::Literal(color)) => Some(color),
                    _ => None,
                })
                .or_else(|| {
                    members_of(doc, owner)
                        .first()
                        .and_then(|m| color_at(doc, *m, "style.fill"))
                });
            (None, color.map(|color| color.to_hex()))
        }
    };
    let max = match prop.ty {
        PropType::Scalar => (value.unwrap_or(0.0).abs() * 4.0).max(1.0),
        PropType::Color => 0.0,
    };
    PropView {
        key: prop.key.clone(),
        label: prop.label.clone(),
        ty: prop.ty.tag(),
        law: if derived { "scaled" } else { "direct" },
        value,
        color,
        derived,
        from,
        min: 0.0,
        max,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::new_node_id;

    fn rect(doc: &mut Document, id: NodeId, width: f64, radius: f64, stroke: f64) {
        let mut node = Node::new(id, "rect", NodeKind::rectangle(0.0, 0.0, width, width));
        node.set_param("corner_radius", ParamValue::float_literal(radius))
            .expect("radius");
        node.set_param("style.stroke_width", ParamValue::float_literal(stroke))
            .expect("stroke");
        doc.insert_node(node, None).expect("insert");
    }

    #[test]
    fn inference_exposes_the_four_designer_props() {
        let mut doc = Document::new();
        let id = new_node_id();
        rect(&mut doc, id, 24.0, 4.0, 2.0);
        let props = infer_props(&doc, &[id]);
        let keys: Vec<&str> = props.iter().map(|p| p.key.as_str()).collect();
        assert_eq!(keys, vec!["size", "stroke_width", "corner_radius", "color"]);
        let stroke = props.iter().find(|p| p.key == "stroke_width").unwrap();
        assert!((stroke.factor().unwrap() - 2.0 / 24.0).abs() < 1e-12);
        let corner = props.iter().find(|p| p.key == "corner_radius").unwrap();
        assert!((corner.factor().unwrap() - 4.0 / 24.0).abs() < 1e-12);
        // The rect sits at the origin, and a zero stays zero: binding `x = 0.0`
        // to a scale would add a slider that can only drag the artwork off its
        // own artboard, so only the magnitudes are bound.
        let size = props.iter().find(|p| p.key == "size").unwrap();
        let slots: Vec<&str> = size.scaled.iter().map(|t| t.property.as_str()).collect();
        assert_eq!(slots, vec!["width", "height"]);

        // A rect that is *not* at the origin scales its position too: 12,6 is
        // part of the shape's geometry, not a label on it.
        let mut placed = Document::new();
        let mut node = Node::new(
            new_node_id(),
            "offset",
            NodeKind::rectangle(12.0, 6.0, 24.0, 10.0),
        );
        node.set_param("corner_radius", ParamValue::Float(Parameter::Literal(2.0)))
            .unwrap();
        placed.insert_node(node.clone(), None).unwrap();
        let props = infer_props(&placed, &[node.id]);
        let size = props.iter().find(|p| p.key == "size").unwrap();
        let slots: Vec<&str> = size.scaled.iter().map(|t| t.property.as_str()).collect();
        assert_eq!(slots, vec!["x", "y", "width", "height"]);
    }

    #[test]
    fn a_circle_offers_no_corner_radius_prop() {
        let mut doc = Document::new();
        let id = new_node_id();
        let mut node = Node::new(id, "dot", NodeKind::circle(10.0, 10.0, 10.0));
        node.set_param("style.stroke_width", ParamValue::float_literal(1.5))
            .unwrap();
        doc.insert_node(node, None).unwrap();
        let props = infer_props(&doc, &[id]);
        assert!(props.iter().all(|p| p.key != "corner_radius"));
        assert!(props.iter().any(|p| p.key == "stroke_width"));
    }

    #[test]
    fn a_bind_plan_names_every_expression_and_variable() {
        let mut doc = Document::new();
        let id = new_node_id();
        rect(&mut doc, id, 24.0, 4.0, 2.0);
        let owner = new_node_id();
        let props = infer_props(&doc, &[id]);
        let plan = bind_plan(&doc, owner, &[id], "card", &props, None);
        assert!(plan
            .variables
            .iter()
            .any(|(name, value)| name == "card_size" && *value == 24.0));
        assert!(plan
            .expressions
            .iter()
            .any(|(_, source)| source.starts_with("0.08333") && source.contains("$card_size")));
        assert!(plan.writes.iter().any(|write| {
            write.property == "style.stroke_width"
                && matches!(write.value, ParamValue::Float(Parameter::Expression(_)))
        }));
        assert!(plan.writes.iter().any(|write| {
            write.property == "style.fill"
                && matches!(write.value, ParamValue::Color(Parameter::Procedural(_)))
        }));
    }

    #[test]
    fn slug_is_stable_and_eight_digits() {
        let id = new_node_id();
        let s = slug(id);
        assert_eq!(s.len(), 8);
        assert_eq!(s, slug(id));
        assert!(s.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn free_variable_never_collides() {
        let mut doc = Document::new();
        doc.variables.insert("card_size".into(), 1.0);
        assert_eq!(
            free_variable(&doc, "card", "size"),
            VariableId::from("card_size2")
        );
    }

    #[test]
    fn the_icon_macro_refuses_a_non_master() {
        let doc = Document::new();
        assert!(icon_set_plan(&doc, new_node_id(), &DEFAULT_ICON_SIZES).is_err());
    }

    #[test]
    fn design_size_reads_the_largest_magnitude() {
        let mut doc = Document::new();
        let id = new_node_id();
        rect(&mut doc, id, 24.0, 4.0, 2.0);
        assert_eq!(design_size(&doc, &[id]), 24.0);
    }
}
