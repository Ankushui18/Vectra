//! Print both exports of one small document — the shape a reader wants to see.
//!
//! ```text
//! cargo run -p vectra-export --example preview
//! ```

use vectra_core::{
    new_expression_id, new_node_id, Command, Engine, NodeKind, ParamValue, Parameter,
};
use vectra_export::{compile_to_ir, export_react, export_svg};

fn main() {
    let mut engine = Engine::new();
    engine
        .dispatch(Command::SetVariable {
            name: "base".into(),
            value: 40.0,
        })
        .unwrap();
    engine
        .dispatch(Command::SetVariable {
            name: "gap".into(),
            value: 24.0,
        })
        .unwrap();
    let twice = new_expression_id();
    engine
        .dispatch(Command::DefineExpression {
            id: twice,
            source: "$base * 2".to_string(),
        })
        .unwrap();

    let box_ = new_node_id();
    engine
        .dispatch(Command::CreateNode {
            id: box_,
            kind: NodeKind::rectangle(0.0, 0.0, 10.0, 30.0),
            name: Some("box".to_string()),
            index: None,
        })
        .unwrap();
    engine
        .dispatch(Command::SetParameter {
            node_id: box_,
            property: "width".to_string(),
            value: ParamValue::Float(Parameter::Expression(twice)),
        })
        .unwrap();
    engine
        .dispatch(Command::SetParameter {
            node_id: box_,
            property: "corner_radius".to_string(),
            value: ParamValue::Float(Parameter::Variable("gap".into())),
        })
        .unwrap();

    engine
        .dispatch(Command::CreateNode {
            id: new_node_id(),
            kind: NodeKind::circle(120.0, 15.0, 15.0),
            name: Some("dot".to_string()),
            index: None,
        })
        .unwrap();
    engine
        .dispatch(Command::CreateNode {
            id: new_node_id(),
            kind: NodeKind::arc(0.0, 0.0, 60.0, 0.0, std::f64::consts::FRAC_PI_2),
            name: Some("swoosh".to_string()),
            index: None,
        })
        .unwrap();

    let ir = compile_to_ir(engine.document());
    println!("===== SVG =====\n{}", export_svg(&ir));
    println!("===== REACT =====\n{}", export_react(&ir));
    println!("===== WARNINGS =====\n{:#?}", ir.warnings);
}
