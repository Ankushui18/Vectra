//! Write the two sample exports the Task 8.0 report quotes.
//!
//! ```text
//! cargo run -p vectra-export --example samples
//! ```
//!
//! The document is chosen to exercise every rule at once: a rounded rectangle
//! whose width is an **expression** and whose radius is a **variable** (RULE 2),
//! a circle (RULE 1: it must stay a `<circle>`), and an arc (RULE 1: one `A`).

use std::path::Path;

use vectra_core::{
    new_expression_id, new_node_id, Command, Engine, NodeKind, ParamValue, Parameter,
};
use vectra_export::{compile_to_ir, export_react, export_svg};

fn main() {
    let mut engine = Engine::new();
    for (name, value) in [("base", 40.0), ("gap", 8.0)] {
        engine
            .dispatch(Command::SetVariable {
                name: name.into(),
                value,
            })
            .unwrap();
    }
    let twice = new_expression_id();
    engine
        .dispatch(Command::DefineExpression {
            id: twice,
            source: "$base * 2".to_string(),
        })
        .unwrap();

    let card = new_node_id();
    engine
        .dispatch(Command::CreateNode {
            id: card,
            kind: NodeKind::rectangle(0.0, 0.0, 1.0, 60.0),
            name: Some("card".to_string()),
            index: None,
        })
        .unwrap();
    for (property, value) in [
        ("width", ParamValue::Float(Parameter::Expression(twice))),
        (
            "corner_radius",
            ParamValue::Float(Parameter::Variable("gap".into())),
        ),
    ] {
        engine
            .dispatch(Command::SetParameter {
                node_id: card,
                property: property.to_string(),
                value,
            })
            .unwrap();
    }

    engine
        .dispatch(Command::CreateNode {
            id: new_node_id(),
            kind: NodeKind::circle(140.0, 30.0, 30.0),
            name: Some("dot".to_string()),
            index: None,
        })
        .unwrap();
    engine
        .dispatch(Command::CreateNode {
            id: new_node_id(),
            kind: NodeKind::arc(210.0, 30.0, 30.0, 0.0, std::f64::consts::PI),
            name: Some("half".to_string()),
            index: None,
        })
        .unwrap();

    let ir = compile_to_ir(engine.document());
    let svg = export_svg(&ir);
    let tsx = export_react(&ir);

    // The crate's own directory, so the files land in the repository no matter
    // where the command is run from.
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let svg_path = root.join("TASK-8.0-SAMPLE.svg");
    let tsx_path = root.join("TASK-8.0-SAMPLE.tsx");
    std::fs::write(&svg_path, &svg).unwrap();
    std::fs::write(&tsx_path, &tsx).unwrap();
    println!("wrote {}", svg_path.display());
    println!("wrote {}", tsx_path.display());
    println!("\n----- React -----\n{tsx}");
}
