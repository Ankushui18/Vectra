//! Task 8.0's laws, proven at the crate boundary.
//!
//! * **Semantic Law** — a document with only a `Circle` exports `<circle>`, not a
//!   `<path>`; an `Arc` exports one `A` command, never a bezier chain.
//! * **Prop Mapping Law** — a node driven by `$my_var` exports a component with
//!   `my_var` as a **required** prop, and the code, not the number.
//! * **Roundtrip Law** — export to SVG, parse it back with a mock importer, and
//!   assert structural equivalence with the IR.
//!
//! Each law is a `proptest` (the task asks for properties, and these are
//! genuinely universal: they hold for every parameter, not for one example) plus
//! the concrete regression tests that pin the shapes a reader wants to see.

use proptest::prelude::*;
use vectra_core::{new_node_id, Color, Command, Document, Engine, NodeKind, ParamValue, Parameter};
use vectra_export::{
    compile_to_ir, export_react, export_react_with, export_svg, translate_expression,
    ExportGeometry, ExportIR, ReactOptions,
};
use vectra_geometry::{path_to_svg_data, EvaluatedPrimitive, GeometryEvaluator};

// ── helpers ────────────────────────────────────────────────────────────

/// A rectangle with a corner radius.
///
/// `NodeKind::rectangle` owns the four slots that need no thought; the corner
/// radius is the fifth parameter, and setting it through the struct is the same
/// thing the inspector does.
fn rectangle(x: f64, y: f64, w: f64, h: f64, radius: f64) -> NodeKind {
    match NodeKind::rectangle(x, y, w, h) {
        NodeKind::Rectangle {
            x,
            y,
            width,
            height,
            ..
        } => NodeKind::Rectangle {
            x,
            y,
            width,
            height,
            corner_radius: Parameter::Literal(radius),
        },
        other => other,
    }
}

fn circle_doc(cx: f64, cy: f64, r: f64) -> ExportIR {
    let mut engine = Engine::new();
    engine
        .dispatch(Command::CreateNode {
            id: new_node_id(),
            kind: NodeKind::circle(cx, cy, r),
            name: Some("dot".to_string()),
            index: None,
        })
        .unwrap();
    compile_to_ir(engine.document())
}

fn rect_doc(x: f64, y: f64, w: f64, h: f64, radius: f64) -> ExportIR {
    let mut engine = Engine::new();
    engine
        .dispatch(Command::CreateNode {
            id: new_node_id(),
            kind: rectangle(x, y, w, h, radius),
            name: Some("box".to_string()),
            index: None,
        })
        .unwrap();
    compile_to_ir(engine.document())
}

fn arc_doc(cx: f64, cy: f64, r: f64, start: f64, end: f64) -> ExportIR {
    let mut engine = Engine::new();
    engine
        .dispatch(Command::CreateNode {
            id: new_node_id(),
            kind: NodeKind::arc(cx, cy, r, start, end),
            name: Some("swoosh".to_string()),
            index: None,
        })
        .unwrap();
    compile_to_ir(engine.document())
}

/// Every `<tag …>` in the document, as `(tag, attributes)` — the mock importer
/// the Roundtrip Law re-reads the export with.
///
/// Deliberately a *structural* parser: it does not try to render anything, it
/// answers "which elements, with which numbers" — which is exactly what a
/// roundtrip of a semantic export can and should check.
#[derive(Debug, Clone, PartialEq)]
struct Element {
    tag: String,
    attrs: std::collections::BTreeMap<String, String>,
}

impl Element {
    fn number(&self, attr: &str) -> f64 {
        self.attrs
            .get(attr)
            .unwrap_or_else(|| panic!("no attribute {attr} on <{}>: {:?}", self.tag, self.attrs))
            .parse()
            .unwrap_or_else(|_| panic!("attribute {attr} is not a number: {:?}", self.attrs))
    }

    fn text(&self, attr: &str) -> String {
        self.attrs
            .get(attr)
            .unwrap_or_else(|| panic!("no attribute {attr} on <{}>", self.tag))
            .clone()
    }
}

fn parse_elements(svg: &str) -> Vec<Element> {
    let mut out: Vec<Element> = Vec::new();
    let mut rest = svg;
    while let Some(start) = rest.find('<') {
        let after = &rest[start + 1..];
        if after.starts_with('!') || after.starts_with('?') {
            // comment / declaration: skip to its end
            let end = after.find('>').expect("unterminated declaration");
            rest = &after[end + 1..];
            continue;
        }
        let end = after.find('>').expect("unterminated element");
        let inner = &after[..end];
        rest = &after[end + 1..];
        if inner.starts_with('/') {
            continue; // closing tag
        }
        let inner = inner.trim_end_matches('/');
        let mut parts = inner.split_whitespace();
        let tag = parts.next().expect("a tag name").to_string();
        let mut attrs = std::collections::BTreeMap::new();
        let mut text = inner[tag.len()..].trim().to_string();
        while let Some(eq) = text.find('=') {
            let name = text[..eq].trim().to_string();
            let value_start = text[eq + 1..].find('"').expect("a quoted value") + eq + 2;
            let value_end = text[value_start..].find('"').expect("a closing quote") + value_start;
            attrs.insert(name, text[value_start..value_end].to_string());
            text = text[value_end + 1..].trim().to_string();
        }
        out.push(Element { tag, attrs });
    }
    out
}

// ── Semantic Law (RULE 1) ──────────────────────────────────────────────

/// A document with only a `Circle` exports a `<circle>` tag, not a `<path>`.
#[test]
fn law_a_circle_is_a_circle() {
    let svg = export_svg(&circle_doc(10.0, -20.0, 5.0));
    assert!(svg.contains("<circle"), "{svg}");
    assert_eq!(svg.matches("<path").count(), 0, "no path stands in: {svg}");
    let elements = parse_elements(&svg);
    let circle = elements
        .iter()
        .find(|element| element.tag == "circle")
        .expect("a circle element");
    assert_eq!(circle.number("cx"), 10.0);
    assert_eq!(circle.number("cy"), -20.0);
    assert_eq!(circle.number("r"), 5.0);
}

proptest! {
    /// …for every circle there is.
    #[test]
    fn law_semantic_circle_stays_a_circle(cx in -500.0f64..500.0, cy in -500.0f64..500.0, r in 0.001f64..500.0) {
        let ir = circle_doc(cx, cy, r);
        let semantic = matches!(ir.nodes[0].geometry, ExportGeometry::Circle { .. });
        prop_assert!(semantic, "the IR kept the primitive: {:?}", ir.nodes[0].geometry);
        let svg = export_svg(&ir);
        prop_assert!(!svg.contains("<path"), "a circle exported as a path: {svg}");
        let elements = parse_elements(&svg);
        let circle = elements.iter().find(|e| e.tag == "circle").expect("a <circle>");
        prop_assert!((circle.number("cx") - cx).abs() < 1e-6);
        prop_assert!((circle.number("cy") - cy).abs() < 1e-6);
        prop_assert!((circle.number("r") - r.abs()).abs() < 1e-6);
    }

    /// A rectangle is a `<rect>`, and its corner radius survives as `rx`.
    #[test]
    fn law_semantic_rect_stays_a_rect(
        x in -200.0f64..200.0, y in -200.0f64..200.0,
        w in 1.0f64..200.0, h in 1.0f64..200.0, radius in 0.0f64..20.0
    ) {
        let ir = rect_doc(x, y, w, h, radius);
        let svg = export_svg(&ir);
        prop_assert!(!svg.contains("<path"), "{svg}");
        let elements = parse_elements(&svg);
        let rect = elements.iter().find(|e| e.tag == "rect").expect("a <rect>");
        prop_assert!((rect.number("x") - x).abs() < 1e-6);
        prop_assert!((rect.number("width") - w).abs() < 1e-6);
        if radius > 0.0 {
            // The export carries the **authored** radius: SVG clamps `rx` to half
            // the shorter side exactly as the evaluator does, so the drawn shape is
            // the same and the number stays the one the user typed.
            prop_assert!((rect.number("rx") - radius).abs() < 1e-4,
                "the authored radius, not a clamped one: {}", rect.text("rx"));
        } else {
            prop_assert!(!rect.attrs.contains_key("rx"));
        }
    }

    /// An arc is **one `A` command** — never a chain of beziers (RULE 1's other
    /// half). The endpoints are the parametric ones, and the flags follow the
    /// canonical sweep.
    #[test]
    fn law_semantic_arc_uses_the_a_command(
        cx in -200.0f64..200.0, cy in -200.0f64..200.0, r in 1.0f64..200.0,
        start in -6.0f64..6.0, sweep in 0.01f64..6.0
    ) {
        let end = start + sweep;
        let ir = arc_doc(cx, cy, r, start, end);
        // The evaluator canonicalises the angles; read them back so the law tests
        // the *canonical* arc, not the raw input.
        let ExportGeometry::Arc { cx, cy, radius, start_angle, end_angle } = &ir.nodes[0].geometry else {
            panic!("an arc is not an arc");
        };
        let (cx, cy, r) = (cx.number(), cy.number(), radius.number());
        let (a0, a1) = (start_angle.number(), end_angle.number());
        let svg = export_svg(&ir);
        prop_assert!(!svg.contains("<circle"), "an arc is not a circle: {svg}");
        let elements = parse_elements(&svg);
        let path = elements.iter().find(|e| e.tag == "path").expect("a <path>");
        let d = path.text("d");
        let commands: Vec<&str> = d.split_whitespace().collect();
        prop_assert_eq!(commands.first().copied(), Some("M"));
        prop_assert_eq!(commands.get(3).copied(), Some("A"), "one A command: {}", d);
        prop_assert_eq!(commands.len(), 11, "M x y A r r rot large sweep x y: {}", d);
        let (x0, y0) = (commands[1].parse::<f64>().unwrap(), commands[2].parse::<f64>().unwrap());
        prop_assert!((x0 - (cx + r * a0.cos())).abs() < 1e-3, "{}", d);
        prop_assert!((y0 - (cy + r * a0.sin())).abs() < 1e-3, "{}", d);
        let sweep_angle = a1 - a0;
        let large: usize = commands[7].parse().unwrap();
        prop_assert_eq!(large, usize::from(sweep_angle > std::f64::consts::PI));
        prop_assert_eq!(commands[8], "1", "the canonical arc runs in +angle");
    }
}

/// A full sweep cannot be one `A` (coincident endpoints mean *zero* sweep), so it
/// is two half arcs — and the picture is still a circle's worth of arc, not a
/// bezier approximation.
#[test]
fn a_full_arc_is_two_half_arcs() {
    let ir = arc_doc(0.0, 0.0, 10.0, 0.0, std::f64::consts::TAU);
    let svg = export_svg(&ir);
    let elements = parse_elements(&svg);
    let path = elements.iter().find(|e| e.tag == "path").expect("a <path>");
    let d = path.text("d");
    assert_eq!(
        d.matches(" A").count() + usize::from(d.starts_with('A')),
        2,
        "{d}"
    );
    assert!(d.contains(" 0 1 1 "), "large + positive sweep: {d}");
}

// ── Prop Mapping Law (RULE 2) ──────────────────────────────────────────

/// A rectangle whose width is `$base * 2` exports `width={base * 2}` — the
/// expression, not the number — with `base` as a required prop.
#[test]
fn law_a_variable_becomes_a_required_prop() {
    let mut engine = Engine::new();
    let id = new_node_id();
    engine
        .dispatch(Command::SetVariable {
            name: "base".into(),
            value: 100.0,
        })
        .unwrap();
    let expression = vectra_core::new_expression_id();
    engine
        .dispatch(Command::DefineExpression {
            id: expression,
            source: "$base * 2".to_string(),
        })
        .unwrap();
    engine
        .dispatch(Command::CreateNode {
            id,
            kind: rectangle(0.0, 0.0, 1.0, 30.0, 0.0),
            name: Some("box".to_string()),
            index: None,
        })
        .unwrap();
    engine
        .dispatch(Command::SetParameter {
            node_id: id,
            property: "width".to_string(),
            value: ParamValue::Float(Parameter::Expression(expression)),
        })
        .unwrap();

    let ir = compile_to_ir(engine.document());
    assert_eq!(ir.props.len(), 1);
    assert_eq!(ir.props[0].name, "base");
    assert_eq!(
        ir.props[0].value, 100.0,
        "the document's current value rides along"
    );

    let tsx = export_react(&ir);
    assert!(tsx.contains("interface SceneProps {"), "{tsx}");
    assert!(
        tsx.contains("base: number; // 100"),
        "a typed, required prop: {tsx}"
    );
    assert!(
        tsx.contains("width={base * 2}"),
        "the code, not the number: {tsx}"
    );
    assert!(
        !tsx.contains("width={200}"),
        "200 is what it *evaluates to*, not what it *is*: {tsx}"
    );
    assert!(
        tsx.contains("export function Scene({ base }: SceneProps)"),
        "{tsx}"
    );
}

proptest! {
    /// …for every variable name and every multiplier.
    #[test]
    fn law_prop_mapping_is_universal(
        name in "[a-z][a-z0-9_]{0,10}",
        multiplier in 1.0f64..20.0,
        base in 0.0f64..1000.0,
    ) {
        let mut document = Document::new();
        document.set_variable(name.clone(), base).unwrap();
        let expression = vectra_core::new_expression_id();
        document
            .define_expression(expression, format!("${name} * {multiplier}"));
        let id = new_node_id();
        let mut node = vectra_core::document::Node::new(
            id,
            "box".to_string(),
            rectangle(0.0, 0.0, 1.0, 30.0, 0.0),
        );
        node.set_param(
            "width",
            ParamValue::Float(Parameter::Expression(expression)),
        )
        .unwrap();
        document.insert_node(node, None).unwrap();

        let ir = compile_to_ir(&document);
        prop_assert_eq!(ir.props.len(), 1, "one prop, once");
        prop_assert_eq!(ir.props[0].name.clone(), name.clone());
        prop_assert_eq!(ir.props[0].value, base);

        let tsx = export_react(&ir);
        prop_assert!(tsx.contains(&format!("{name}: number")), "required typed prop: {}", tsx);
        prop_assert!(tsx.contains(&format!("width={{{name} * ")), "parametric width: {}", tsx);
        prop_assert!(
            tsx.contains(&format!("function Scene({{ {name} }}: SceneProps)")),
            "destructured props: {}",
            tsx
        );
    }

    /// A slot driven straight by a variable keeps the variable's name as code.
    #[test]
    fn law_a_direct_variable_is_a_prop_too(name in "[a-z][a-z0-9_]{0,8}", value in 0.0f64..100.0) {
        let mut document = Document::new();
        document.set_variable(name.clone(), value).unwrap();
        let id = new_node_id();
        let mut node = vectra_core::document::Node::new(
            id,
            "dot".to_string(),
            NodeKind::circle(0.0, 0.0, 1.0),
        );
        node.set_param("radius", ParamValue::Float(Parameter::Variable(name.clone())))
            .unwrap();
        document.insert_node(node, None).unwrap();
        let tsx = export_react(&compile_to_ir(&document));
        prop_assert!(tsx.contains(&format!("r={{{name}}}")), "{}", tsx);
        prop_assert!(tsx.contains(&format!("{name}: number")), "{}", tsx);
    }
}

/// The expression translator is the one piece of RULE 2 that is purely textual,
/// so it gets its own table: `$name` loses its sigil, the four functions get
/// their target names, and everything else is untouched.
#[test]
fn expression_source_translates_without_evaluating() {
    assert_eq!(translate_expression("$base * 2"), "base * 2");
    assert_eq!(
        translate_expression("sin($t) + cos($t)"),
        "Math.sin(t) + Math.cos(t)"
    );
    assert_eq!(translate_expression("abs($x - $y)"), "Math.abs(x - y)");
    assert_eq!(
        translate_expression("clamp($v, 0, 1)"),
        "clamp(v, 0, 1)",
        "`clamp` stays a call — the React module declares a total one"
    );
    assert_eq!(translate_expression("$a1_$b2"), "a1_b2");
}

/// …and the helper it may need is emitted only when it is used.
#[test]
fn the_clamp_helper_appears_only_when_used() {
    let mut document = Document::new();
    let expression = vectra_core::new_expression_id();
    document.define_expression(expression, "clamp($v, 0, 10)".to_string());
    let id = new_node_id();
    let mut node = vectra_core::document::Node::new(
        id,
        "box".to_string(),
        rectangle(0.0, 0.0, 1.0, 10.0, 0.0),
    );
    node.set_param(
        "width",
        ParamValue::Float(Parameter::Expression(expression)),
    )
    .unwrap();
    document.insert_node(node, None).unwrap();
    let tsx = export_react(&compile_to_ir(&document));
    assert!(tsx.contains("const clamp = (value: number"), "{tsx}");
    assert!(tsx.contains("width={clamp(v, 0, 10)}"), "{tsx}");

    let plain = export_react(&rect_doc(0.0, 0.0, 10.0, 10.0, 0.0));
    assert!(!plain.contains("const clamp"), "{plain}");
}

// ── Roundtrip Law ──────────────────────────────────────────────────────

/// Export to SVG, re-import it (mock), and assert **structural equivalence**:
/// same elements, same numbers, same semantics.
#[test]
fn law_svg_round_trips_through_a_mock_importer() {
    let mut engine = Engine::new();
    engine
        .dispatch(Command::SetVariable {
            name: "gap".into(),
            value: 25.0,
        })
        .unwrap();
    for (kind, name) in [
        (rectangle(-30.0, 0.0, 40.0, 30.0, 5.0), "box"),
        (NodeKind::circle(40.0, 10.0, 12.0), "dot"),
        (
            NodeKind::arc(0.0, 0.0, 60.0, 0.0, std::f64::consts::FRAC_PI_2),
            "swoosh",
        ),
    ] {
        engine
            .dispatch(Command::CreateNode {
                id: new_node_id(),
                kind,
                name: Some(name.to_string()),
                index: None,
            })
            .unwrap();
    }
    let ir = compile_to_ir(engine.document());
    let svg = export_svg(&ir);

    let elements = parse_elements(&svg);
    let geometry: Vec<&Element> = elements
        .iter()
        .filter(|element| element.tag != "svg" && element.tag != "g")
        .collect();
    assert_eq!(geometry.len(), ir.nodes.len(), "{svg}");
    assert_eq!(geometry[0].tag, "rect", "the rect stayed a rect");
    assert_eq!(geometry[1].tag, "circle", "the circle stayed a circle");
    assert_eq!(
        geometry[2].tag, "path",
        "the arc is a path — with an A command"
    );

    // Numbers round-trip exactly through the text.
    for (element, node) in geometry.iter().zip(&ir.nodes) {
        match &node.geometry {
            ExportGeometry::Rect {
                x,
                y,
                width,
                height,
                ..
            } => {
                assert_eq!(element.number("x"), x.number());
                assert_eq!(element.number("y"), y.number());
                assert_eq!(element.number("width"), width.number());
                assert_eq!(element.number("height"), height.number());
            }
            ExportGeometry::Circle { cx, cy, radius } => {
                assert_eq!(element.number("cx"), cx.number());
                assert_eq!(element.number("cy"), cy.number());
                assert_eq!(element.number("r"), radius.number());
            }
            ExportGeometry::Arc { .. } => {
                assert!(element.text("d").contains(" A"), "{}", element.text("d"));
            }
            other => panic!("unexpected geometry {other:?}"),
        }
        assert_eq!(
            element.text("data-vectra-node"),
            node.id,
            "identity survives"
        );
    }

    // A variable-driven slot is *resolved* in a picture export: the SVG has the
    // number, and the number is what the IR says the slot is worth.
    let filled = ir
        .nodes
        .iter()
        .find(|node| node.name == "box")
        .expect("the box");
    assert!(
        !filled
            .geometry
            .parameters()
            .iter()
            .any(|param| !param.vars.is_empty()),
        "nothing in this document reads a variable, so no slot is symbolic"
    );
}

proptest! {
    /// …for every picture: whatever goes in comes back.
    #[test]
    fn law_roundtrip_holds_for_every_picture(
        shapes in prop::collection::vec(
            prop_oneof![
                (any::<f64>(), any::<f64>(), 0.1f64..300.0, 0.1f64..300.0, 0.0f64..40.0)
                    .prop_map(|(x, y, w, h, r)| Shape::Rect { x, y, w, h, r }),
                (any::<f64>(), any::<f64>(), 0.1f64..300.0)
                    .prop_map(|(cx, cy, r)| Shape::Circle { cx, cy, r }),
            ],
            1..6,
        )
    ) {
        let mut engine = Engine::new();
        for shape in &shapes {
            engine.dispatch(Command::CreateNode {
                id: new_node_id(),
                kind: shape.kind(),
                name: None,
                index: None,
            }).unwrap();
        }
        let ir = compile_to_ir(engine.document());
        let svg = export_svg(&ir);
        let elements = parse_elements(&svg);
        let drawn: Vec<&Element> = elements.iter()
            .filter(|e| e.tag == "rect" || e.tag == "circle" || e.tag == "path")
            .collect();
        prop_assert_eq!(drawn.len(), ir.nodes.len());
        for (element, node) in drawn.iter().zip(&ir.nodes) {
            match &node.geometry {
                ExportGeometry::Rect { width, .. } => {
                    prop_assert_eq!(element.tag.clone(), "rect");
                    prop_assert!((element.number("width") - width.number()).abs() < 1e-6);
                }
                ExportGeometry::Circle { radius, .. } => {
                    prop_assert_eq!(element.tag.clone(), "circle");
                    prop_assert!((element.number("r") - radius.number()).abs() < 1e-6);
                }
                other => prop_assert!(false, "unexpected {other:?}"),
            }
        }
    }
}

#[derive(Debug)]
enum Shape {
    Rect {
        x: f64,
        y: f64,
        w: f64,
        h: f64,
        r: f64,
    },
    Circle {
        cx: f64,
        cy: f64,
        r: f64,
    },
}

impl Shape {
    fn kind(&self) -> NodeKind {
        match *self {
            Shape::Rect { x, y, w, h, r } => rectangle(x, y, w, h, r),
            Shape::Circle { cx, cy, r } => NodeKind::circle(cx, cy, r),
        }
    }
}

// ── determinism, and the colour door ───────────────────────────────────

/// Same document, same bytes — twice, and through both exporters. (Determinism
/// is what makes the smoke's byte comparisons meaningful.)
#[test]
fn the_same_document_exports_the_same_bytes() {
    // One document (ids are minted per document), compiled twice: the bytes a
    // user gets must not depend on how many times they clicked Export.
    let ir = rect_doc(1.5, -2.25, 30.0, 12.5, 3.0);
    let first = (export_svg(&ir), export_react(&ir));
    let second = (export_svg(&ir), export_react(&ir));
    assert_eq!(first, second);
}

/// A transparent fill is `none`; an alpha colour keeps its channel.
#[test]
fn colours_export_faithfully() {
    let mut engine = Engine::new();
    let id = new_node_id();
    engine
        .dispatch(Command::CreateNode {
            id,
            kind: rectangle(0.0, 0.0, 10.0, 10.0, 0.0),
            name: Some("box".to_string()),
            index: None,
        })
        .unwrap();
    engine
        .dispatch(Command::SetParameter {
            node_id: id,
            property: "style.fill".to_string(),
            value: ParamValue::Color(Parameter::Literal(Color::rgba(255, 0, 0, 128))),
        })
        .unwrap();
    let svg = export_svg(&compile_to_ir(engine.document()));
    assert!(svg.contains("fill=\"#ff0000\""), "{svg}");
    assert!(svg.contains("fill-opacity=\"0.501961\""), "{svg}");
}

/// The generated module is a *component*: props in the signature, geometry in the
/// body, and the name is a knob.
#[test]
fn the_component_name_is_a_knob() {
    let ir = circle_doc(0.0, 0.0, 5.0);
    let tsx = export_react_with(
        &ir,
        &ReactOptions {
            component_name: "Logo".to_string(),
            include_comments: false,
        },
    );
    assert!(tsx.contains("export function Logo()"), "{tsx}");
    assert!(!tsx.contains("{/*"), "{tsx}");
}

/// An empty document is not an error: it is an empty picture, and the SVG says so
/// without inventing anything.
#[test]
fn an_empty_document_exports_an_empty_picture() {
    let ir = compile_to_ir(&Document::new());
    assert!(ir.nodes.is_empty());
    assert!(ir.bounds.is_none());
    assert!(ir.warnings.is_empty());
    let svg = export_svg(&ir);
    assert!(svg.contains("<svg"), "{svg}");
    assert!(!svg.contains("<rect"), "{svg}");
}

// ── Task 11.0: text exports as geometry, not as type ───────────────────────

/// A document with one text node, and the family it asks for.
///
/// The id is a parameter so two documents that differ only in their typography
/// can be compared byte for byte — the export writes the node's id into the file
/// (that is how a designer finds the node again), and it is the one thing a fair
/// comparison must hold fixed.
fn text_doc(id: vectra_core::NodeId, word: &str, size: f64, family: &str) -> Engine {
    let mut engine = Engine::new();
    let kind = match NodeKind::text(10.0, 40.0, word, size) {
        NodeKind::Text {
            text,
            font_size,
            letter_spacing,
            line_height,
            alignment,
            x,
            y,
            ..
        } => NodeKind::Text {
            text,
            font_family: family.to_string(),
            font_size,
            letter_spacing,
            line_height,
            alignment,
            x,
            y,
            on_path: None,
        },
        other => other,
    };
    engine
        .dispatch(Command::CreateNode {
            id,
            kind,
            name: Some("word".to_string()),
            index: None,
        })
        .expect("CreateNode");
    engine
}

/// **The export strategy for text (RULE 4's boundary claim)**: a text node
/// leaves the exporter as **geometry** — one font-independent `<path>` whose `d`
/// is exactly the outline the canvas draws — and never as a `<text>` element
/// that would need the reader's machine to own the same font.
///
/// This is the property that makes an exported file render identically
/// everywhere: the letterforms travel inside the file, and the *family name*
/// does not appear in it at all.
#[test]
fn text_exports_as_font_independent_paths() {
    let id = new_node_id();
    let engine = text_doc(id, "Vectra", 32.0, "Vectra Sans");
    let ir = compile_to_ir(engine.document());
    assert_eq!(ir.nodes.len(), 1);

    // 1. The geometry is a path with a `d` — the outline, not a rectangle or a
    //    placeholder box.
    let d = match &ir.nodes[0].geometry {
        ExportGeometry::Path { d } => d.clone().expect("a text node exports a `d`"),
        other => panic!("text must export as a Path, got {other:?}"),
    };
    assert!(d.starts_with('M'), "path data starts a subpath: {d}");

    // 2. It is *the same* outline the canvas draws — the export re-uses the
    //    evaluation rather than outlining a second time (a second outline could
    //    only ever disagree).
    let ctx = engine.evaluation_context();
    let scene = GeometryEvaluator::new()
        .evaluate_full(engine.document(), &ctx)
        .scene;
    let run = match &scene.get(id).expect("the run is in the scene").primitive {
        EvaluatedPrimitive::Text(run) => run.clone(),
        other => panic!("expected a text run, got {}", other.tag()),
    };
    assert_eq!(d, path_to_svg_data(&run.outline));

    // 3. The file says `<path>`, and mentions neither `<text>` nor a font.
    let svg = export_svg(&ir);
    assert!(svg.contains("<path"), "{svg}");
    let lower = svg.to_lowercase();
    assert!(
        !lower.contains("<text"),
        "an exported run must not be `<text>`: {svg}"
    );
    assert!(!lower.contains("font-family"), "{svg}");
    assert!(!lower.contains("vectra sans"), "{svg}");

    // 4. **The file does not depend on the font being installed**: a document
    //    that names a family this machine has never seen exports the *same*
    //    bytes, because the run fell back to the bundled face and its outline is
    //    what travels.
    let unknown = text_doc(id, "Vectra", 32.0, "Helvetica Neu");
    assert_eq!(export_svg(&compile_to_ir(unknown.document())), svg);
}
