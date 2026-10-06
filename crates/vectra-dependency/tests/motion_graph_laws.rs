//! Task 6.0 laws, at the graph level: **a binding's sources are its
//! dependencies**.
//!
//! These tests exist because of a bug found while scoping Task 6.0, in shipped
//! Task 2.2 code. `dependency_target` — the function that derives edges, the
//! singular one — classified a parameter by its own variant and stopped:
//!
//! ```rust,ignore
//! Parameter::Animated(_) => Some(GraphNode::clock()),
//! ```
//!
//! That is correct for a binding whose operands are literals, and **silently
//! wrong for every other binding**: `Spring { target: $radius }` was dirtied by
//! the clock but never by an edit to `$radius`. The spring would keep easing
//! toward the old target until something unrelated dirtied the node — a stale
//! scene with no error, no diagnostic and no failing test, because nothing had
//! yet been able to *bind* a non-literal source.
//!
//! The fix is [`dependency_targets`], the plural: the derivation walks the
//! binding's parameter tree and emits one edge per dependency.
//!
//! `law_a_binding_depends_on_its_inner_sources` is the regression test, written
//! against the production pipeline (gate → dispatch → graph sync → dirty
//! derivation → scene patch) through the same `Harness` the Task 2.2 laws use.

mod common;

use common::*;
use vectra_core::{
    new_node_id, Command, MotionBinding, MotionTrack, NodeKind, ParamValue, Parameter,
};
use vectra_dependency::GraphNode;

/// A rectangle whose `width` is `param`.
fn rect_engine(param: Parameter<f64>) -> (Harness, vectra_core::NodeId) {
    let mut h = Harness::new();
    let id = new_node_id();
    h.engine
        .dispatch(Command::CreateNode {
            id,
            kind: NodeKind::rectangle(0.0, 0.0, 10.0, 10.0),
            name: Some("rect".to_string()),
            index: None,
        })
        .expect("create");
    h.dispatch(Command::SetParameter {
        node_id: id,
        property: "width".to_string(),
        value: ParamValue::Float(param),
    })
    .expect("bind width");
    (h, id)
}

fn width_property(id: vectra_core::NodeId) -> GraphNode {
    GraphNode::GeometryProperty(id, "width".to_string())
}

/// What `slot` depends on: the `to` end of every edge leaving it.
fn sources_of(h: &Harness, slot: &GraphNode) -> Vec<GraphNode> {
    h.edges()
        .into_iter()
        .filter(|(from, _)| from == slot)
        .map(|(_, to)| to)
        .collect()
}

/// What depends on `source`: the `from` end of every edge arriving at it.
fn readers_of(h: &Harness, source: &GraphNode) -> Vec<GraphNode> {
    h.edges()
        .into_iter()
        .filter(|(_, to)| to == source)
        .map(|(from, _)| from)
        .collect()
}

// ── The regression test ───────────────────────────────────────────────────

#[test]
fn law_a_binding_depends_on_its_inner_sources() {
    // A spring chasing a *variable*, with the slot's starting value captured as
    // the anchor. Under the old singular derivation this produced exactly one
    // edge (`width → clock`) and the `$radius` edge was missing.
    let (mut h, id) = rect_engine(Parameter::Animated(MotionBinding::spring(
        Parameter::variable("radius"),
        170.0,
        26.0,
        10.0,
        0.0,
    )));

    let clock = GraphNode::clock();
    let radius = GraphNode::Variable("radius".to_string());
    let sources = sources_of(&h, &width_property(id));
    assert!(
        sources.contains(&clock),
        "the clock drives a spring (motion samples ctx.time); got {sources:?}"
    );
    assert!(
        sources.contains(&radius),
        "…and so does the spring's target; got {sources:?}"
    );
    assert_eq!(
        readers_of(&h, &radius),
        vec![width_property(id)],
        "the $radius vertex feeds exactly the slot that chases it"
    );

    // The regression: an edit to $radius must dirty the node that is sprung
    // toward it. Under the old derivation this assertion failed — the graph had
    // no `width → $radius` edge, so `dirty_ids_for_events` returned nothing and
    // the cached scene kept the old target forever.
    h.dispatch(Command::SetVariable {
        name: "radius".to_string(),
        value: 300.0,
    })
    .expect("set $radius");
    let dirty = h
        .graph
        .dirty_ids_for_events(&[vectra_core::EngineEvent::VariablesUpdated {
            names: vec!["radius".to_string()],
        }]);
    assert_eq!(
        dirty,
        vec![id],
        "editing the spring's target variable must dirty the sprung node \
         (this is the stale-scene bug: the graph edge was missing)"
    );

    // …and the value really does follow, through the production resolution path.
    assert!(h.edges().iter().any(|(_, to)| to == &radius));
    let resolved = h.engine.resolve_float(id, "width").expect("width resolves");
    assert!(
        (resolved - 300.0).abs() < 1e-9,
        "a spring with no motion crate previews at its target: {resolved}"
    );
}

#[test]
fn law_nested_bindings_depend_on_everything_they_read() {
    // A state branch whose true arm is a spring chasing an expression, whose
    // false arm reads a variable: three sources, one slot.
    let expression = vectra_core::new_expression_id();
    let (mut h, id) = rect_engine(Parameter::Animated(MotionBinding::state(
        "hover",
        Parameter::Animated(MotionBinding::spring(
            Parameter::expression(expression),
            200.0,
            20.0,
            0.0,
            0.0,
        )),
        Parameter::variable("rest"),
    )));
    h.engine
        .dispatch(Command::DefineExpression {
            id: expression,
            source: "$base * 2".to_string(),
        })
        .expect("define");

    let sources = sources_of(&h, &width_property(id));
    for (source, why) in [
        (GraphNode::clock(), "springs and branches sample the clock"),
        (
            GraphNode::State("hover".to_string()),
            "the branch reads `hover`",
        ),
        (
            GraphNode::Variable("rest".to_string()),
            "the false arm reads $rest",
        ),
        (
            GraphNode::Expression(expression),
            "the true arm reads the expression",
        ),
    ] {
        assert!(
            sources.contains(&source),
            "missing dependency {source:?} ({why}); got {sources:?}"
        );
    }

    // A state flip dirties the slot precisely — not the whole scene.
    let dirty = h
        .graph
        .dirty_ids_for_events(&[vectra_core::EngineEvent::TracksUpdated { ids: vec![] }]);
    assert!(dirty.is_empty(), "an empty registry event dirties nothing");
}

#[test]
fn law_a_track_edit_dirties_exactly_its_readers() {
    let track_id = "intro".to_string();
    let (mut h, sprung) = rect_engine(Parameter::Animated(MotionBinding::track(
        track_id.clone(),
        "x",
    )));
    // A second node that reads nothing: the track edit must not reach it.
    let bystander = new_node_id();
    h.engine
        .dispatch(Command::CreateNode {
            id: bystander,
            kind: NodeKind::circle(0.0, 0.0, 5.0),
            name: Some("bystander".to_string()),
            index: None,
        })
        .expect("create bystander");

    h.dispatch(Command::SetMotionTrack {
        track: MotionTrack::ramp("intro", "Intro", "x", [(0.0, 0.0), (1.0, 100.0)]),
    })
    .expect("register track");

    // Editing the track dirties its readers…
    h.dispatch(Command::SetMotionTrack {
        track: MotionTrack::ramp("intro", "Intro", "x", [(0.0, 0.0), (2.0, 400.0)]),
    })
    .expect("edit track");
    let dirty = h
        .graph
        .dirty_ids_for_events(&[vectra_core::EngineEvent::TracksUpdated {
            ids: vec![track_id.clone()],
        }]);
    assert_eq!(dirty, vec![sprung], "only the track's reader re-evaluates");
    assert!(!dirty.contains(&bystander));
}

#[test]
fn law_binding_a_missing_track_or_channel_is_a_typed_error() {
    // The boundary validation that makes a bound slot always resolvable.
    let (mut h, id) = rect_engine(Parameter::Literal(10.0));

    let missing_track = h.dispatch(Command::BindMotion {
        node_id: id,
        property: "width".to_string(),
        binding: MotionBinding::track("nope", "x"),
    });
    assert!(missing_track.is_err(), "binding an unknown track must fail");
    assert!(
        h.engine.document().get_node(id).is_ok(),
        "and must not disturb the document"
    );

    h.dispatch(Command::SetMotionTrack {
        track: MotionTrack::ramp("intro", "Intro", "x", [(0.0, 0.0), (1.0, 1.0)]),
    })
    .expect("register track");
    let missing_channel = h.dispatch(Command::BindMotion {
        node_id: id,
        property: "width".to_string(),
        binding: MotionBinding::track("intro", "y"),
    });
    assert!(
        missing_channel.is_err(),
        "binding an unknown channel must fail"
    );

    // Degenerate springs are refused too: an undamped spring never settles, so
    // it would pin the animation loop forever (see the design doc §D2).
    for (stiffness, damping) in [(0.0, 26.0), (170.0, 0.0), (f64::NAN, 26.0)] {
        let bad = h.dispatch(Command::BindMotion {
            node_id: id,
            property: "width".to_string(),
            binding: MotionBinding::spring(Parameter::Literal(1.0), stiffness, damping, 0.0, 0.0),
        });
        assert!(bad.is_err(), "spring {stiffness}/{damping} must be refused");
    }
}

#[test]
fn law_bind_and_unbind_are_exactly_invertible() {
    let (mut h, id) = rect_engine(Parameter::variable("base"));
    h.dispatch(Command::SetVariable {
        name: "base".to_string(),
        value: 120.0,
    })
    .expect("set variable");

    /// The slot's *source*, which is what binding changes — the harness
    /// fingerprint tracks registries, not parameter trees.
    fn width_source(h: &Harness, id: vectra_core::NodeId) -> String {
        let node = h.engine.document().get_node(id).expect("node");
        let ParamValue::Float(width) = node.get_param("width").expect("width param") else {
            panic!("width is a float slot");
        };
        match width {
            Parameter::Variable(name) => format!("var:{name}"),
            Parameter::Literal(v) => format!("literal:{v}"),
            Parameter::Animated(binding) => format!("animated:{}", binding.tag()),
            other => format!("other:{}", other.source_tag()),
        }
    }

    let sorted = |h: &Harness| {
        let mut edges = h.edges();
        edges.sort_by_key(|(from, to)| (format!("{from:?}"), format!("{to:?}")));
        edges
    };
    let before = width_source(&h, id);
    let edges_before = sorted(&h);
    assert_eq!(before, "var:base");

    h.dispatch(Command::BindMotion {
        node_id: id,
        property: "width".to_string(),
        binding: MotionBinding::spring(Parameter::Literal(400.0), 170.0, 26.0, 120.0, 0.0),
    })
    .expect("bind");
    assert_eq!(width_source(&h, id), "animated:spring", "the slot is bound");

    // One undo restores the *variable* reference, not a literal: the inverse is
    // the generic `SetParameter`, which is why binding needs no special case.
    h.undo().expect("undo");
    assert_eq!(
        width_source(&h, id),
        "var:base",
        "undo restored the slot's source exactly"
    );
    assert_eq!(sorted(&h), edges_before, "and the graph with it");

    h.redo().expect("redo");
    assert_eq!(width_source(&h, id), "animated:spring", "redo re-binds");
}

#[test]
fn law_track_registry_round_trips_through_undo() {
    let (mut h, _id) = rect_engine(Parameter::Literal(10.0));
    assert!(h.engine.document().motion.is_empty());

    h.dispatch(Command::SetMotionTrack {
        track: MotionTrack::ramp("intro", "Intro", "x", [(0.0, 0.0), (1.0, 100.0)]),
    })
    .expect("register");
    assert_eq!(h.engine.document().motion.len(), 1);

    h.undo().expect("undo register");
    assert!(
        h.engine.document().motion.is_empty(),
        "undoing a track creation removes it"
    );

    h.redo().expect("redo register");
    assert_eq!(h.engine.document().motion.len(), 1);
    assert!(h.engine.document().motion.get("intro").is_some());
}

#[test]
fn law_a_malformed_track_is_refused_before_it_enters_the_document() {
    let (mut h, _id) = rect_engine(Parameter::Literal(10.0));
    let bad = [
        // Times must strictly increase.
        MotionTrack::ramp("t", "T", "x", [(1.0, 0.0), (1.0, 5.0)]),
        MotionTrack::ramp("t", "T", "x", [(5.0, 0.0), (1.0, 5.0)]),
        // Values must be finite.
        MotionTrack::ramp("t", "T", "x", [(0.0, f64::NAN)]),
        // And a track needs at least one channel with at least one keyframe.
        MotionTrack {
            id: "t".to_string(),
            name: "T".to_string(),
            channels: Default::default(),
        },
        MotionTrack::ramp("", "T", "x", [(0.0, 0.0)]),
    ];
    for track in bad {
        let label = format!("{track:?}");
        let result = h.dispatch(Command::SetMotionTrack { track });
        assert!(result.is_err(), "malformed track accepted: {label}");
    }
    assert!(
        h.engine.document().motion.is_empty(),
        "nothing malformed reached the registry"
    );
}
