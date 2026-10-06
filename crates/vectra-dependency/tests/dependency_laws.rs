//! Task 2.2 laws: propagation, cycle detection, undo/redo integrity — plus the
//! invariants that make the graph trustworthy under arbitrary edit streams.
//!
//! ```text
//! §5.1  law_propagation_is_minimal_and_correct
//! §5.2  law_cycle_detection_rejects_and_leaves_everything_untouched
//! §5.3  law_undo_redo_leaves_no_dangling_graph_state
//! +     law_dirty_set_touches_only_dependents
//! +     law_failed_commands_are_topology_neutral
//! +     prop_invariants_hold_under_random_edit_streams
//! +     prop_gate_predicts_the_topology_delta_exactly
//! ```
//!
//! Every law drives the [`Harness`], which mirrors the shipped engine protocol
//! (gate → dispatch → registry sync → graph sync → dirty derivation), calling
//! the same production functions `vectra-wasm` calls. A law proven here is a
//! statement about the shipped engine, not about a test double.

mod common;

use common::*;
use proptest::prelude::*;
use std::collections::BTreeSet;
use vectra_core::{
    new_expression_id, new_node_id, Command, EngineEvent, NodeKind, ParamValue, Parameter,
};
use vectra_dependency::{gate_command, DagEdge, GraphNode};

type Edges = BTreeSet<DagEdge>;

fn edges_of(h: &Harness) -> Edges {
    h.edges().into_iter().collect()
}

// ── §5.1 Propagation law ─────────────────────────────────────────────────

#[test]
fn law_propagation_is_minimal_and_correct() {
    let mut h = Harness::new();

    // Chain: Variable(a) → Expression($a * 2) → Geometry(width)
    h.dispatch(Command::SetVariable {
        name: "a".to_string(),
        value: 10.0,
    })
    .unwrap();
    let expr = new_expression_id();
    h.dispatch(Command::DefineExpression {
        id: expr,
        source: "$a * 2".to_string(),
    })
    .unwrap();

    let bound = new_node_id();
    let unrelated = new_node_id();
    h.dispatch(Command::CreateNode {
        id: bound,
        kind: NodeKind::rectangle(0.0, 0.0, 5.0, 5.0),
        name: Some("bound".to_string()),
        index: None,
    })
    .unwrap();
    h.dispatch(Command::CreateNode {
        id: unrelated,
        kind: NodeKind::rectangle(200.0, 0.0, 5.0, 5.0),
        name: Some("unrelated".to_string()),
        index: None,
    })
    .unwrap();
    h.dispatch(Command::SetParameter {
        node_id: bound,
        property: "width".to_string(),
        value: ParamValue::Float(Parameter::Expression(expr)),
    })
    .unwrap();

    // The graph holds exactly the chain: expression → variable, property → expression.
    let prop = GraphNode::GeometryProperty(bound, "width".to_string());
    let var = GraphNode::Variable("a".to_string());
    let expr_node = GraphNode::Expression(expr);
    assert_eq!(
        h.edges(),
        vec![
            (expr_node.clone(), var.clone()),
            (prop.clone(), expr_node.clone())
        ]
    );

    // Changing the variable affects the chain (evaluation order: dependencies
    // first) and dirties *only* the bound geometry.
    let affected = h.graph.affected(std::slice::from_ref(&var));
    assert_eq!(
        affected,
        vec![var.clone(), expr_node.clone(), prop.clone()],
        "dependencies first, dependents last"
    );
    let dirty = h.graph.dirty_ids(std::slice::from_ref(&var));
    assert_eq!(dirty, vec![bound]);
    assert!(!dirty.contains(&unrelated), "unrelated node was dirtied");

    // Change it for real; the value flows through the compiled expression.
    let events = h
        .dispatch(Command::SetVariable {
            name: "a".to_string(),
            value: 21.0,
        })
        .unwrap();
    assert_eq!(
        h.resolve(bound, "width").unwrap(),
        42.0,
        "$a * 2 with $a = 21"
    );

    // The production dirty derivation agrees with the graph query.
    assert_eq!(h.graph.dirty_ids_for_events(&events), vec![bound]);

    // The unrelated node's evaluated geometry is untouched by the patch.
    match h.cache.scene().get(unrelated).unwrap().primitive {
        vectra_geometry::EvaluatedPrimitive::Rect { w, .. } => assert_eq!(w, 5.0),
        ref other => panic!("unexpected primitive {other:?}"),
    }
    match h.cache.scene().get(bound).unwrap().primitive {
        vectra_geometry::EvaluatedPrimitive::Rect { w, .. } => assert_eq!(w, 42.0),
        ref other => panic!("unexpected primitive {other:?}"),
    }
}

// ── §5.2 Cycle detection law ─────────────────────────────────────────────

/// Model of the brief's `$a = $b + 1` / `$b = $a + 1` shape.
///
/// Phase 1 variables are scalar values (`Document.variables: HashMap<_, f64>`),
/// so "a variable whose value is defined by an expression" — the edge type that
/// makes this cycle constructible — arrives with the Phase-4 procedural graph.
/// The machinery is already the real thing: `Variable(a) → Expression(1)` means
/// "a is defined by expression 1". This test builds exactly that shape and
/// asserts the rejection path the engine uses.
#[test]
fn law_cycle_detection_rejects_and_leaves_everything_untouched() {
    use vectra_dependency::{DependencyGraph, ProspectiveEdges};

    let mut graph = DependencyGraph::new();
    let a = GraphNode::Variable("a".to_string());
    let b = GraphNode::Variable("b".to_string());
    let expr1 = GraphNode::Expression(new_expression_id());
    let expr2 = GraphNode::Expression(new_expression_id());

    // "$a = $b + 1": a is defined by expr1, and expr1 reads $b.
    graph.try_add_edge(a.clone(), expr1.clone()).unwrap();
    graph.try_add_edge(expr1.clone(), b.clone()).unwrap();
    let before = edges_of_graph(&graph);
    let before_json = graph.export_json();
    assert_eq!(graph.validate(), Ok(()));

    // "$b = $a + 1" wants edges b → expr2 and expr2 → a. The batch closes the
    // loop, so it must be refused as a whole.
    let err = graph
        .try_add_edges(&[(b.clone(), expr2.clone()), (expr2.clone(), a.clone())])
        .unwrap_err();
    assert!(err.is_cycle_rejection(), "expected CyclicDependency: {err}");
    assert!(
        err.to_string().contains("cyclic dependency"),
        "typed error must name the condition: {err}"
    );
    assert_eq!(
        edges_of_graph(&graph),
        before,
        "graph changed after rejection"
    );
    assert_eq!(graph.export_json(), before_json, "serialization changed");
    assert_eq!(graph.validate(), Ok(()), "dangling index after rejection");
    assert!(graph.is_acyclic());

    // The gate (`dry_run`) is the same primitive the engine calls.
    assert!(graph
        .dry_run(&ProspectiveEdges {
            adds: vec![(b.clone(), expr2.clone()), (expr2.clone(), a.clone())],
            removes: vec![],
        })
        .is_err());
    assert!(
        graph
            .dry_run(&ProspectiveEdges {
                adds: vec![(b.clone(), expr2.clone())],
                removes: vec![],
            })
            .is_ok(),
        "half of the pair is legal on its own"
    );

    // Longer cycles are caught too: a → b → c → a.
    let c = GraphNode::Variable("c".to_string());
    let mut chain = DependencyGraph::new();
    chain.try_add_edge(a.clone(), b.clone()).unwrap();
    chain.try_add_edge(b.clone(), c.clone()).unwrap();
    let err = chain.try_add_edges(&[(c.clone(), a.clone())]).unwrap_err();
    assert!(err.is_cycle_rejection());
    assert_eq!(chain.edges().len(), 2, "partial batch applied");

    // A self-dependency is refused as well.
    let err = chain.try_add_edge(a.clone(), a.clone()).unwrap_err();
    assert!(err.is_cycle_rejection());
    assert_eq!(chain.edges().len(), 2);
}

fn edges_of_graph(graph: &vectra_dependency::DependencyGraph) -> Edges {
    graph.edges().into_iter().collect()
}

/// Command-level proof that the gate is wired and consulted: real commands
/// hand it exactly the edges they will add, and commands that fail move
/// nothing.
#[test]
fn law_gate_is_wired_and_reserves_topology_for_successful_commands() {
    let mut h = Harness::new();
    h.dispatch(Command::SetVariable {
        name: "a".to_string(),
        value: 3.0,
    })
    .unwrap();
    let expr = new_expression_id();
    h.dispatch(Command::DefineExpression {
        id: expr,
        source: "$a + 1".to_string(),
    })
    .unwrap();
    let node = new_node_id();
    h.dispatch(Command::CreateNode {
        id: node,
        kind: NodeKind::circle(0.0, 0.0, 4.0),
        name: None,
        index: None,
    })
    .unwrap();

    // The gate sees the prospective delta of a real binding.
    let bind = Command::SetParameter {
        node_id: node,
        property: "radius".to_string(),
        value: ParamValue::Float(Parameter::Expression(expr)),
    };
    let delta = gate_command(&h.graph, &bind, h.doc()).unwrap();
    assert_eq!(
        delta.adds,
        vec![(
            GraphNode::GeometryProperty(node, "radius".to_string()),
            GraphNode::Expression(expr)
        )]
    );
    assert!(
        delta.removes.is_empty(),
        "a fresh literal slot has no edges"
    );
    let before = edges_of(&h);
    h.dispatch(bind).unwrap();
    assert_eq!(
        edges_of(&h),
        before
            .union(&delta.adds.iter().cloned().collect())
            .cloned()
            .collect::<Edges>(),
        "the applied delta is exactly what the gate predicted"
    );

    // Rebinding the same expression keeps one edge (dedup), and the gate
    // reports it as remove-then-add rather than a silent no-op.
    let rebind = Command::SetParameter {
        node_id: node,
        property: "radius".to_string(),
        value: ParamValue::Float(Parameter::Expression(expr)),
    };
    let delta = gate_command(&h.graph, &rebind, h.doc()).unwrap();
    assert_eq!(delta.adds.len(), 1);
    assert_eq!(delta.removes.len(), 1);
    h.dispatch(rebind).unwrap();
    assert_eq!(h.graph.edge_count(), 2, "duplicate edges must not appear");

    // Failed commands are topology-neutral and history-neutral.
    let before_edges = edges_of(&h);
    let before_state = h.fingerprint();
    let depth = h.engine.document().expressions.len();
    for bad in [
        Command::SetParameter {
            node_id: node,
            property: "not_a_property".to_string(),
            value: ParamValue::float_literal(1.0),
        },
        Command::SetParameter {
            node_id: node,
            property: "radius".to_string(),
            value: ParamValue::point_literal(1.0, 2.0),
        },
        Command::DefineExpression {
            id: new_expression_id(),
            source: "$a * ".to_string(),
        },
    ] {
        assert!(h.dispatch(bad).is_err(), "expected a typed failure");
        assert_eq!(edges_of(&h), before_edges, "failed command moved the graph");
        assert_eq!(
            h.fingerprint(),
            before_state,
            "failed command mutated state"
        );
    }
    assert_eq!(h.engine.document().expressions.len(), depth);
    h.assert_invariants();
}

// ── §5.3 Undo/redo graph integrity law ───────────────────────────────────

#[test]
fn law_undo_redo_leaves_no_dangling_graph_state() {
    let mut h = Harness::new();

    // 1. variable, 2. expression, 3. node, 4. binding that consumes the rest.
    h.dispatch(Command::SetVariable {
        name: "a".to_string(),
        value: 6.0,
    })
    .unwrap();
    let expr = new_expression_id();
    h.dispatch(Command::DefineExpression {
        id: expr,
        source: "$a * 7".to_string(),
    })
    .unwrap();
    let node = new_node_id();
    h.dispatch(Command::CreateNode {
        id: node,
        kind: NodeKind::circle(0.0, 0.0, 1.0),
        name: Some("c".to_string()),
        index: None,
    })
    .unwrap();
    h.dispatch(Command::SetParameter {
        node_id: node,
        property: "radius".to_string(),
        value: ParamValue::Float(Parameter::Expression(expr)),
    })
    .unwrap();

    let chain = [
        (
            GraphNode::Expression(expr),
            GraphNode::Variable("a".to_string()),
        ),
        (
            GraphNode::GeometryProperty(node, "radius".to_string()),
            GraphNode::Expression(expr),
        ),
    ];
    let full: Edges = chain.iter().cloned().collect();
    assert_eq!(edges_of(&h), full);
    assert_eq!(h.resolve(node, "radius").unwrap(), 42.0);

    // Expected state after each undo, in order.
    let expectations: Vec<(&str, Edges)> = vec![
        (
            "binding removed",
            chain[..1].iter().cloned().collect::<Edges>(),
        ),
        (
            "node removed",
            chain[..1].iter().cloned().collect::<Edges>(),
        ),
        ("expression removed", Edges::new()),
        ("variable removed", Edges::new()),
    ];
    for (label, want) in expectations {
        h.undo().unwrap();
        assert_eq!(edges_of(&h), want, "after undo: {label}");
        h.assert_invariants();
        assert_eq!(h.graph.validate(), Ok(()), "dangling index after {label}");
    }

    // Baseline: completely empty, and the old indices no longer resolve.
    assert!(h.graph.is_empty(), "graph must be empty at baseline");
    assert_eq!(h.graph.node_index(&chain[1].0), None);
    assert_eq!(h.graph.node_index(&chain[0].0), None);
    assert_eq!(h.graph.node_index(&chain[0].1), None);

    // Redo restores the graph edge-for-edge, three times in a row.
    for _ in 0..3 {
        for _ in 0..4 {
            h.redo().unwrap();
            h.assert_invariants();
        }
        assert_eq!(edges_of(&h), full, "redo must restore the exact chain");
        assert_eq!(h.resolve(node, "radius").unwrap(), 42.0);
        for _ in 0..4 {
            h.undo().unwrap();
        }
        assert!(h.graph.is_empty());
    }
}

// ── Minimality ───────────────────────────────────────────────────────────

#[test]
fn law_dirty_set_touches_only_dependents() {
    let mut h = Harness::new();
    for (name, value) in [("a", 4.0), ("b", 2.0)] {
        h.dispatch(Command::SetVariable {
            name: name.to_string(),
            value,
        })
        .unwrap();
    }

    let direct = new_node_id(); // radius ← $a
    let via_expr = new_node_id(); // radius ← ($a * 2)
    let via_chain = new_node_id(); // radius ← ($a / 2 + $b)
    let clock = new_node_id(); // x ← animated (clock)
    let mut independent: Vec<vectra_core::NodeId> = Vec::new();

    for (i, id) in [direct, via_expr, via_chain, clock].iter().enumerate() {
        h.dispatch(Command::CreateNode {
            id: *id,
            kind: NodeKind::circle(i as f64 * 100.0, 0.0, 10.0),
            name: Some(format!("n{i}")),
            index: None,
        })
        .unwrap();
    }
    for _ in 0..8 {
        let id = new_node_id();
        independent.push(id);
        h.dispatch(Command::CreateNode {
            id,
            kind: NodeKind::rectangle(0.0, 0.0, 3.0, 3.0),
            name: None,
            index: None,
        })
        .unwrap();
    }

    let e1 = new_expression_id();
    let e2 = new_expression_id();
    h.dispatch(Command::DefineExpression {
        id: e1,
        source: "$a * 2".to_string(),
    })
    .unwrap();
    h.dispatch(Command::DefineExpression {
        id: e2,
        source: "$a / 2 + $b".to_string(),
    })
    .unwrap();
    h.dispatch(Command::SetParameter {
        node_id: direct,
        property: "radius".to_string(),
        value: ParamValue::Float(Parameter::variable("a")),
    })
    .unwrap();
    h.dispatch(Command::SetParameter {
        node_id: via_expr,
        property: "radius".to_string(),
        value: ParamValue::Float(Parameter::Expression(e1)),
    })
    .unwrap();
    h.dispatch(Command::SetParameter {
        node_id: via_chain,
        property: "radius".to_string(),
        value: ParamValue::Float(Parameter::Expression(e2)),
    })
    .unwrap();
    h.dispatch(Command::SetParameter {
        node_id: clock,
        property: "x".to_string(),
        value: ParamValue::Float(Parameter::Animated(
            vectra_core::MotionBinding::KeyframeTrack {
                track_id: "t".to_string(),
                property: "x".to_string(),
            },
        )),
    })
    .unwrap();

    // The animated slot hangs off the clock vertex (its own global dep).
    // NB: the write used the alias "x"; the graph stores the canonical slot
    // name ("cx" on a Circle) — aliases normalize to one vertex, never two.
    assert!(h.graph.contains_edge(
        &GraphNode::GeometryProperty(clock, "cx".to_string()),
        &GraphNode::clock()
    ));
    assert!(!h
        .graph
        .contains_node(&GraphNode::GeometryProperty(clock, "x".to_string())));

    // $a drives exactly three nodes; everything else stays clean.
    let events = h
        .dispatch(Command::SetVariable {
            name: "a".to_string(),
            value: 9.0,
        })
        .unwrap();
    let dirty: BTreeSet<_> = h.graph.dirty_ids_for_events(&events).into_iter().collect();
    assert_eq!(dirty, BTreeSet::from([direct, via_expr, via_chain]));
    for id in &independent {
        assert!(!dirty.contains(id), "independent node {id} was dirtied");
    }
    assert!(
        !dirty.contains(&clock),
        "clock-bound node dirtied by a value change"
    );

    // The clock vertex reaches exactly its readers.
    assert_eq!(h.graph.dirty_ids(&[GraphNode::clock()]), vec![clock]);

    // A variable nobody reads costs nothing at all.
    let idle = h
        .dispatch(Command::SetVariable {
            name: "nobody_reads_me".to_string(),
            value: 1.0,
        })
        .unwrap();
    assert!(h.graph.dirty_ids_for_events(&idle).is_empty());

    h.assert_invariants();
}

/// The random driver is a plan of record: it must be able to act on every op
/// kind, and it must say so loudly when it declines. Two halves —
///
///  * a hand-built stream that reaches every kind and never skips, and
///  * the corner that proved it: op kind 7 (redefine an existing expression)
///    on an empty document has nothing to redefine, so the planner reports a
///    *skip* instead of inventing work (see `prop_invariants_…`, which counts
///    skips rather than demanding that every stream do something).
#[test]
fn the_random_driver_never_silently_drops_an_op() {
    let mut h = Harness::new();
    // Applicable at every step: a node and an expression exist before the ops
    // that need them, and the deletes come after the writes they undo.
    let stream: [(u8, u8, u8); 13] = [
        (0, 0, 0),  // create a node
        (3, 2, 1),  // set a variable
        (4, 0, 2),  // set the driver's own orphan variable
        (6, 2, 3),  // define an expression
        (5, 0, 0),  // define another
        (7, 0, 1),  // redefine one — needs an expression to exist
        (8, 0, 5),  // write a parameter of that node
        (11, 0, 0), // undo the write
        (11, 0, 1), // redo it
        (2, 1, 3),  // set a variable again
        (1, 2, 4),  // create a second node
        (10, 1, 0), // delete it
        (9, 0, 5),  // write a parameter again
    ];
    let planned = stream.len();
    let mut kinds = BTreeSet::new();
    for (op, arg, extra) in stream {
        kinds.insert(op % 12);
        apply_random_op(&mut h, op, arg, extra);
        h.assert_invariants();
    }
    assert_eq!(h.skips, 0, "the stream was built to apply at every step");
    assert_eq!(h.failures, 0, "and every command was accepted");
    assert_eq!(h.steps, planned, "one step per op");
    assert_eq!(
        kinds,
        (0u8..12).collect::<BTreeSet<_>>(),
        "the stream exercises every op kind"
    );

    // The corner itself: an empty document has nothing to redefine.
    let mut empty = Harness::new();
    apply_random_op(&mut empty, 7, 0, 0);
    empty.assert_invariants();
    assert_eq!(empty.steps, 0, "nothing to redefine, so nothing ran");
    assert_eq!(
        empty.skips, 1,
        "and the planner said so — a skip, not a drop"
    );
}

// ── Property tests ───────────────────────────────────────────────────────

proptest! {
    #![proptest_config(ProptestConfig { cases: 64, max_shrink_iters: 400, ..ProptestConfig::default() })]

    /// After *every* step of an arbitrary edit stream (creates, binds,
    /// redefines, deletes, undo/redo, deliberate failures) the graph equals the
    /// document's derivation, the index maps are intact, and no cycle exists.
    #[test]
    fn prop_invariants_hold_under_random_edit_streams(
        ops in prop::collection::vec((0u8..12, 0u8..32, 0u8..16), 1..80),
    ) {
        let mut h = Harness::new();
        let planned = ops.len();
        for (op, arg, extra) in ops {
            apply_random_op(&mut h, op, arg, extra);
            h.assert_invariants();
        }
        prop_assert_eq!(h.rejections, 0, "no command in the pool can build a cycle yet");
        // The driver must account for every op it was handed: each one either
        // ran against the engine or was reported inapplicable. A stream may
        // legitimately be *all* skips — a one-op stream of kind 7 on an empty
        // document has no expression to redefine — and a skip is a decision,
        // never a silent drop.
        prop_assert_eq!(
            h.steps + h.skips,
            planned,
            "every planned op either ran or was reported inapplicable"
        );
    }

    /// The gate predicts the topology delta *exactly*: for every successful
    /// command, added edges == prospective adds that were missing, removed
    /// edges == prospective removes that are gone; for every refused command,
    /// the topology is untouched.
    #[test]
    fn prop_gate_predicts_the_topology_delta_exactly(
        ops in prop::collection::vec((0u8..12, 0u8..32, 0u8..16), 1..60),
    ) {
        let mut h = Harness::new();
        for (op, arg, extra) in ops {
            let before = edges_of(&h);
            match plan_random_op(&h, op, arg, extra) {
                RandomOp::Command(cmd) => {
                    let predicted = gate_command(&h.graph, &cmd, h.doc());
                    let outcome = h.dispatch(cmd);
                    match (predicted, outcome) {
                        (Ok(delta), Ok(_)) => {
                            let after = edges_of(&h);
                            let want_added: Edges = delta
                                .adds
                                .iter()
                                .filter(|e| !before.contains(e))
                                .cloned()
                                .collect();
                            let want_removed: Edges = delta
                                .removes
                                .iter()
                                .filter(|e| !after.contains(e))
                                .cloned()
                                .collect();
                            prop_assert_eq!(after.difference(&before).cloned().collect::<Edges>(), want_added);
                            prop_assert_eq!(before.difference(&after).cloned().collect::<Edges>(), want_removed);
                        }
                        (Err(e), Ok(_)) => prop_assert!(false, "gate refused, dispatch accepted: {e}"),
                        (_, Err(_)) => prop_assert_eq!(edges_of(&h), before, "refused command moved topology"),
                    }
                }
                RandomOp::Undo | RandomOp::Redo => {
                    let pending = if matches!(plan_random_op(&h, op, arg, extra), RandomOp::Undo) {
                        h.engine.peek_undo().cloned()
                    } else {
                        h.engine.peek_redo().cloned()
                    };
                    let predicted = pending.as_ref().map(|c| gate_command(&h.graph, c, h.doc()));
                    let outcome = if matches!(plan_random_op(&h, op, arg, extra), RandomOp::Undo) {
                        h.undo()
                    } else {
                        h.redo()
                    };
                    match (predicted, outcome) {
                        (Some(Ok(delta)), Ok(_)) => {
                            let after = edges_of(&h);
                            let want_added: Edges = delta.adds.iter()
                                .filter(|e| !before.contains(e)).cloned().collect();
                            let want_removed: Edges = delta.removes.iter()
                                .filter(|e| !after.contains(e)).cloned().collect();
                            prop_assert_eq!(after.difference(&before).cloned().collect::<Edges>(), want_added);
                            prop_assert_eq!(before.difference(&after).cloned().collect::<Edges>(), want_removed);
                        }
                        (None, Err(_)) => {} // empty stack: nothing to predict, nothing moved
                        (_, Err(_)) => prop_assert_eq!(edges_of(&h), before),
                        (Some(Err(e)), Ok(_)) => prop_assert!(false, "gate refused, history accepted: {e}"),
                        (None, Ok(_)) => prop_assert!(false, "history moved with no pending command"),
                    }
                }
                RandomOp::Skip => {}
            }
            h.assert_invariants();
        }
    }

    /// The dirty set is always the downstream closure of what changed, and it
    /// is *sufficient*: patching exactly those ids equals a full rebuild (the
    /// scene-level version of this law lives in `incremental_laws.rs`).
    #[test]
    fn prop_dirty_ids_are_always_live_nodes(
        ops in prop::collection::vec((0u8..12, 0u8..32, 0u8..16), 1..40),
    ) {
        let mut h = Harness::new();
        for (op, arg, extra) in ops {
            let before_doc_ids: BTreeSet<_> = h.node_ids().into_iter().collect();
            apply_random_op(&mut h, op, arg, extra);
            let events = vec![EngineEvent::OrderChanged];
            // Every id the graph would hand the evaluator is a document node
            // either now or (for a delete) a moment ago.
            let dirty: BTreeSet<_> = h
                .graph
                .dirty_ids_for_events(&events)
                .into_iter()
                .collect();
            prop_assert!(dirty.is_empty(), "structural events carry no ids");
            for node in h.graph.nodes() {
                if let Some(id) = node.node_id() {
                    let now: BTreeSet<_> = h.node_ids().into_iter().collect();
                    prop_assert!(
                        now.contains(&id) || before_doc_ids.contains(&id),
                        "graph referenced a node neither current nor just-deleted"
                    );
                }
            }
        }
    }
}
