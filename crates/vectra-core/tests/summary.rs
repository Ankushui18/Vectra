//! Task 9.0 RULE 2 — `DocumentSummary`, from the core's own side.
//!
//! `vectra-ai`'s tests prove what the AI layer does *with* a summary; these pin
//! the summary itself: the label text, the source/now split that keeps a
//! parametric slot parametric, the id/name lookup the context check leans on,
//! the budget, and the prose a prompt is built from.

use std::collections::BTreeSet;

use vectra_core::summary::{
    describe_scalar, trim_number, DocumentSummary, NameLookup, STYLE_SLOTS,
};
use vectra_core::{
    new_constraint_id, new_expression_id, new_node_id, Constraint, ConstraintKind,
    ConstraintTarget, Document, NodeId, NodeKind, ParamValue, Parameter,
};

/// A document with `card` (a rectangle whose `width` is `$base * 2`) and `dot`
/// (a circle), plus the variable `base`.
fn document() -> (Document, NodeId, NodeId) {
    let mut doc = Document::new();
    let card = doc.create_node(
        NodeKind::rectangle(0.0, 0.0, 40.0, 30.0),
        Some("card".to_string()),
    );
    let dot = doc.create_node(NodeKind::circle(10.0, 10.0, 5.0), Some("Dot".to_string()));
    doc.set_variable("base".to_string(), 21.0).unwrap();

    let expression = new_expression_id();
    doc.define_expression(expression, "$base * 2".to_string());
    doc.get_node_mut(card)
        .unwrap()
        .set_param(
            "width",
            ParamValue::Float(Parameter::Expression(expression)),
        )
        .unwrap();
    (doc, card, dot)
}

#[test]
fn the_label_names_the_kind_and_the_name() {
    let (doc, card, dot) = document();
    let summary = DocumentSummary::capture(&doc);

    let label = |id: NodeId| {
        summary
            .nodes
            .iter()
            .find(|node| node.id == id.to_string())
            .unwrap()
            .label
            .clone()
    };
    assert_eq!(label(card), "Rectangle 'card'");
    assert_eq!(label(dot), "Circle 'Dot'");

    // Draw order is the document's order, back → front — and every node is in
    // the grounding set.
    let ids: BTreeSet<&str> = summary.node_ids();
    assert_eq!(
        ids,
        BTreeSet::from([card, dot])
            .into_iter()
            .map(|id| id.to_string())
            .collect::<BTreeSet<String>>()
            .iter()
            .map(String::as_str)
            .collect()
    );
    assert_eq!(
        summary
            .nodes
            .iter()
            .map(|node| node.label.as_str())
            .collect::<Vec<_>>(),
        vec!["Rectangle 'card'", "Circle 'Dot'"]
    );
}

#[test]
fn a_parametric_slot_keeps_its_source_and_never_invents_a_value() {
    let (doc, _, _) = document();
    let summary = DocumentSummary::capture(&doc);
    let node = summary.find_node("card").unwrap();

    let width = node
        .slots
        .iter()
        .find(|slot| slot.property == "width")
        .unwrap();
    // Captured without a context there is no evaluator, so a variable reads
    // `None`: the summary reports what the document *says*.
    assert_eq!(width.source, "$base * 2");
    assert_eq!(width.value, None);

    let height = node
        .slots
        .iter()
        .find(|slot| slot.property == "height")
        .unwrap();
    assert_eq!(height.source, trim_number(30.0));
    assert_eq!(height.value, Some(30.0));

    // The prose line carries the source, and the "(now …)" suffix only appears
    // when a value differs from it — which is what stops a model flattening a
    // parametric slot into a number.
    let text = summary.to_text();
    assert!(text.contains("width = $base * 2"), "{text}");
    assert!(!text.contains("width = 42"), "{text}");
    assert!(text.contains("height = 30"), "{text}");
    assert!(text.contains("$base = 21"), "{text}");
    assert!(text.contains("Rectangle 'card'"), "{text}");
}

#[test]
fn describe_scalar_speaks_the_documents_own_language() {
    let (mut doc, card, _) = document();
    assert_eq!(describe_scalar(&doc, &Parameter::Literal(12.5)), "12.5");
    assert_eq!(
        describe_scalar(&doc, &Parameter::Variable("base".to_string())),
        "$base"
    );

    let expression = new_expression_id();
    doc.define_expression(expression, "$base + 1".to_string());
    assert_eq!(
        describe_scalar(&doc, &Parameter::Expression(expression)),
        "$base + 1"
    );

    // A dangling expression id is *labelled* as missing, never silently dropped.
    let dangling = describe_scalar(&doc, &Parameter::Expression(new_expression_id()));
    assert!(dangling.contains("missing"), "{dangling}");

    // A slot pointing at a removed expression is reported as **broken**, not as
    // its last known source: the summary describes the document, and the
    // document no longer holds that source.
    doc.get_node_mut(card)
        .unwrap()
        .set_param(
            "height",
            ParamValue::Float(Parameter::Expression(expression)),
        )
        .unwrap();
    doc.remove_expression(expression).unwrap();
    let summary = DocumentSummary::capture(&doc);
    let node = summary.find_node("card").unwrap();
    let height = node
        .slots
        .iter()
        .find(|slot| slot.property == "height")
        .unwrap();
    assert_eq!(height.value, None, "a broken slot resolves to nothing");
    assert!(
        height.source.contains("(missing)"),
        "a dangling reference says so: {height:?}"
    );
    assert!(
        summary.to_text().contains("(missing)"),
        "{}",
        summary.to_text()
    );
}

#[test]
fn lookup_accepts_ids_names_and_prefixes_and_refuses_ambiguity() {
    let (doc, card, dot) = document();
    let summary = DocumentSummary::capture(&doc);

    // Exact id, name in either case, a quoted name, and a unique prefix.
    assert_eq!(summary.resolve_node_id(&card.to_string()), Some(card));
    assert_eq!(summary.resolve_node_id("card"), Some(card));
    assert_eq!(summary.resolve_node_id("DOT"), Some(dot));
    assert_eq!(summary.resolve_node_id("\"card\""), Some(card));
    let prefix = card.to_string()[..8].to_string();
    assert_eq!(summary.resolve_node_id(&prefix), Some(card));

    // Unknown, empty, and a prefix too short to be a discriminator: all misses.
    assert_eq!(summary.resolve_node_id("square"), None);
    assert_eq!(summary.resolve_node_id("  "), None);
    assert_eq!(summary.resolve_node_id(&card.to_string()[..3]), None);

    // A duplicated name is `Ambiguous`, never a coin toss.
    let mut twins = Document::new();
    let a = twins.create_node(
        NodeKind::rectangle(0.0, 0.0, 1.0, 1.0),
        Some("same".to_string()),
    );
    let b = twins.create_node(
        NodeKind::rectangle(0.0, 0.0, 2.0, 2.0),
        Some("same".to_string()),
    );
    let twins = DocumentSummary::capture(&twins);
    match twins.find_node("same") {
        Err(NameLookup::Ambiguous(ids)) => {
            assert_eq!(ids.len(), 2, "{ids:?}");
            assert!(ids.contains(&a.to_string()) && ids.contains(&b.to_string()));
        }
        Ok(node) => panic!("ambiguity was resolved by picking {}", node.id),
        Err(other) => panic!("expected ambiguity, got {other:?}"),
    }
    assert_eq!(twins.resolve_node_id("same"), None);
    assert!(!twins.find_node("SAME").is_ok());
    assert!(twins.find_node("nothing").is_err());
    assert_eq!(twins.resolve_node_id("not-uuid-shaped"), None);
    let _ = new_node_id();
}

#[test]
fn constraints_are_listed_with_their_target_slots() {
    let (mut doc, card, dot) = document();
    let constraint = Constraint::new(
        new_constraint_id(),
        ConstraintKind::Vertical,
        vec![
            ConstraintTarget::new(card, "x"),
            ConstraintTarget::new(dot, "x"),
        ],
    );
    doc.constraints.insert(constraint.clone()).unwrap();

    let summary = DocumentSummary::capture(&doc);
    let listed = summary
        .constraints
        .iter()
        .find(|item| item.id == constraint.id.to_string())
        .expect("the constraint is listed");
    assert_eq!(listed.kind, "vertical");
    assert_eq!(listed.strength, "medium");
    assert!(listed.enabled);
    assert_eq!(
        listed.targets,
        vec![format!("{card}.x"), format!("{dot}.x")],
        "a target is `<node id>.<property>`"
    );
    let text = summary.to_text();
    assert!(text.contains("CONSTRAINTS"), "{text}");
    assert!(text.contains(&format!("{card}.x")), "{text}");
}

#[test]
fn the_text_names_an_empty_canvas_and_labels_a_truncated_list() {
    let empty = DocumentSummary::capture(&Document::new());
    let text = empty.to_text();
    assert!(text.contains("0 node(s)"), "{text}");
    assert!(text.contains("none (the canvas is empty)"), "{text}");
    assert!(empty.node_ids().is_empty());
    assert_eq!(empty.variable_value("base"), None);

    // A budget below the node count clips the *text* and says so, leaving the
    // data whole.
    let mut doc = Document::new();
    for index in 0..5 {
        doc.create_node(
            NodeKind::rectangle(0.0, 0.0, 1.0, 1.0),
            Some(format!("n{index}")),
        );
    }
    let summary = DocumentSummary::capture(&doc);
    let clipped = summary.to_text_within(2);
    assert!(
        clipped.contains("n0") && clipped.contains("n1"),
        "{clipped}"
    );
    assert!(!clipped.contains("n2"), "{clipped}");
    assert!(clipped.contains("3 more node(s)"), "{clipped}");
    assert_eq!(summary.nodes.len(), 5, "the budget clips text, not data");
    assert_eq!(summary.to_text_within(5).matches("id=").count(), 5);
}

#[test]
fn variables_and_style_slots_are_reported_the_way_a_prompt_needs_them() {
    let (doc, card, _) = document();
    let summary = DocumentSummary::capture(&doc);
    assert_eq!(summary.variable_value("base"), Some(21.0));
    assert_eq!(summary.variable_value("missing"), None);
    assert_eq!(summary.variables.len(), 1);
    assert_eq!(summary.variables[0].name, "base");

    // Styles are named constants so an AI sets a colour without guessing the
    // spelling; they are not geometry slots.
    assert!(STYLE_SLOTS.contains(&"style.fill"));
    assert_eq!(STYLE_SLOTS.len(), 4);
    assert!(vectra_core::Node::property_feeds_geometry("width"));
    assert!(!vectra_core::Node::property_feeds_geometry("style.fill"));

    // Geometry slots come from the kind's own list — the single source.
    let node = summary.find_node(&card.to_string()).unwrap();
    // Geometry slots come first, from the kind's own list (the single source),
    // and the four style slots follow — a model is told every slot it may name.
    let properties: Vec<&str> = node
        .slots
        .iter()
        .map(|slot| slot.property.as_str())
        .collect();
    let expected: Vec<&str> = NodeKind::rectangle(0.0, 0.0, 0.0, 0.0)
        .scalar_slots()
        .iter()
        .copied()
        .chain(STYLE_SLOTS)
        .collect();
    assert_eq!(properties, expected);
    // A style slot carries its source (a hex, or `$accent`) and **no number** —
    // a colour is not a scalar, and the summary does not pretend otherwise.
    let fill = node
        .slots
        .iter()
        .find(|slot| slot.property == "style.fill")
        .unwrap();
    assert!(fill.source.starts_with('#'), "{fill:?}");
    assert_eq!(fill.value, None);
}

#[test]
fn the_text_block_reads_like_a_prompt_section() {
    let (mut doc, card, dot) = document();
    doc.constraints
        .insert(Constraint::new(
            new_constraint_id(),
            ConstraintKind::Vertical,
            vec![
                ConstraintTarget::new(card, "x"),
                ConstraintTarget::new(dot, "x"),
            ],
        ))
        .unwrap();
    let expression = new_expression_id();
    doc.define_expression(expression, "$base * 4".to_string());
    let summary = DocumentSummary::capture(&doc);
    let text = summary.to_text();
    // The block is a *section*: a header that counts what follows, then one
    // section per registry, in the order a reader (or a model) expects.
    let header = text.lines().next().unwrap();
    assert!(header.starts_with("DOCUMENT v1 — 2 node(s)"), "{header}");
    assert!(header.contains("1 variable(s)"), "{header}");
    assert!(header.contains("1 constraint(s)"), "{header}");
    let position = |needle: &str| {
        text.find(needle)
            .unwrap_or_else(|| panic!("missing {needle}"))
    };
    assert!(position("VARIABLES") < position("NODES ("));
    assert!(position("NODES (") < position("EXPRESSIONS"));
    assert!(position("EXPRESSIONS") < position("CONSTRAINTS"));
    // Both nodes are listed, in draw order, with their exact ids.
    // The label keeps the name as the document spells it — the summary reports.
    assert!(
        position(&format!("Rectangle 'card'  id={card}"))
            < position(&format!("Circle 'Dot'  id={dot}"))
    );
    assert!(text.contains("$base * 4"), "{}", text);
}
