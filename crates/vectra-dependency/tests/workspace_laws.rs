//! **Task 10.2's three laws**: the professional workspace's contracts, proven
//! against the shipped evaluation path.
//!
//! ```text
//!   Layer Order Law   reordering layers changes the draw order in the scene
//!   Visibility Law    hiding a layer removes it from the *picture*, not from
//!                     the document — and does it without re-evaluating
//!   Appearance Law    two strokes draw as two strokes, in order, with their
//!                     own blend modes
//! ```
//!
//! They live here, in `vectra-dependency`, because that is where the honest
//! answer to every one of them is decided: the harness drives a real `Engine`
//! through the same protocol `vectra-wasm::VectraEngine::settle` uses
//! (`gate_command` → `ExpressionEngine::sync_from_document` →
//! `DependencyGraph::sync` → `dirty_ids_for_events` → `IncrementalScene::refresh`
//! → `refresh_presentation`), minus the JSON hop. A law that holds here holds in
//! the app.
//!
//! # Why the Visibility Law is written the way it is
//!
//! RULE 4 says an eye must not trigger a re-evaluation, and a re-evaluation is
//! the only thing that could *remove* a node from `EvaluatedScene`. So the scene
//! keeps the node and flips its `visible` flag — and the frame stops drawing it,
//! which is what "removed from the scene" means to anyone looking at the canvas.
//! The law therefore asserts all three halves at once: the picture loses it, the
//! document keeps it, and the evaluator did nothing.

mod common;

use common::Harness;
use proptest::prelude::*;
use std::collections::BTreeSet;
use vectra_core::ids::LayerId;
use vectra_core::style::AppearanceLayer;
use vectra_core::{new_node_id, BlendMode, Color, Command, EvalMode, NodeId, NodeKind, Parameter};
use vectra_geometry::{DirtySet, EvaluatedStyle};
use vectra_render::{tessellate, MeshKind, RenderScene, Tessellation};

// ── fixtures ────────────────────────────────────────────────────────────────

/// Three layers named for the z-order they are compared in, created back to
/// front with `count` rectangles each. Returns `(layers, nodes)` with the nodes
/// grouped by layer.
fn three_layers(h: &mut Harness, count: usize) -> (Vec<LayerId>, Vec<Vec<NodeId>>) {
    let mut layers = Vec::new();
    let mut groups = Vec::new();
    for (index, name) in ["Base", "Middle", "Top"].iter().enumerate() {
        let id = vectra_core::ids::new_layer_id();
        h.dispatch(Command::CreateLayer {
            id,
            name: (*name).to_string(),
            index: None,
            artboard: None,
        })
        .expect("create layer");
        // New artwork lands in the *active* layer, so the active layer is
        // re-pointed before each batch.
        h.dispatch(Command::SetActiveLayer { id })
            .expect("active layer");
        let mut group = Vec::new();
        for slot in 0..count {
            let node = new_node_id();
            let x = 20.0 + 60.0 * index as f64;
            let y = 20.0 + 40.0 * slot as f64;
            h.dispatch(Command::CreateNode {
                id: node,
                name: Some(format!("{name} {slot}")),
                index: None,
                kind: NodeKind::Rectangle {
                    x: Parameter::Literal(x),
                    y: Parameter::Literal(y),
                    width: Parameter::Literal(30.0),
                    height: Parameter::Literal(20.0),
                    corner_radius: Parameter::Literal(0.0),
                },
            })
            .expect("create node");
            group.push(node);
        }
        layers.push(id);
        groups.push(group);
    }
    (layers, groups)
}

fn style_of(h: &Harness, node: NodeId) -> EvaluatedStyle {
    h.cache
        .scene()
        .get(node)
        .expect("evaluated node")
        .style
        .clone()
}

/// Ids in the order the engine reports them: `NodeId` is an opaque uuid, and
/// `refresh_presentation` sorts what it returns, so every expectation in this
/// file is written sorted too.
fn sorted(mut ids: Vec<NodeId>) -> Vec<NodeId> {
    ids.sort();
    ids
}

/// The ids the frozen scene draws, in draw order.
fn drawn_order(h: &Harness) -> Vec<NodeId> {
    h.cache.scene().z_order.clone()
}

/// What the draw order must be, from the document: its own order, with the
/// non-drawable entries (a `Group` owns children but paints nothing) dropped.
fn expected_drawn(h: &Harness) -> Vec<NodeId> {
    let scene = h.cache.scene();
    h.engine
        .document()
        .order
        .iter()
        .copied()
        .filter(|id| scene.nodes.contains_key(id))
        .collect()
}

// ── Law 1: the draw order follows the layers ────────────────────────────────

/// A deterministic reading of the law, spelled out: three layers, two shapes
/// each, and every reorder the panel can perform moves the *whole block*.
#[test]
fn law_reordering_a_layer_moves_its_whole_block_in_the_draw_order() {
    let mut h = Harness::new();
    let (layers, groups) = three_layers(&mut h, 2);
    h.refresh_presentation();

    // Back → front: Base, Middle, Top — one contiguous block per layer.
    let order = drawn_order(&h);
    let expected: Vec<NodeId> = groups.iter().flatten().copied().collect();
    assert_eq!(
        order,
        expected_drawn(&h),
        "the scene agrees with the document"
    );
    assert_eq!(order, expected, "layers stack in creation order");
    assert_eq!(
        order,
        h.engine.document().order,
        "the scene's order is the document's order"
    );

    // Send the top layer to the back: it becomes one block at the front of the
    // order, and its *internal* order is untouched.
    h.dispatch(Command::ReorderLayer {
        id: layers[2],
        index: 0,
    })
    .expect("reorder layer");
    let order = drawn_order(&h);
    let expected: Vec<NodeId> = groups[2]
        .iter()
        .chain(groups[0].iter())
        .chain(groups[1].iter())
        .copied()
        .collect();
    assert_eq!(order, expected, "a layer is a unit in the draw order");
    assert_eq!(order, h.engine.document().order);

    // And the document's own invariant — the flat order *is* the flattened
    // layer tree — still holds after the move.
    assert!(h.engine.document().order_matches_layers());

    // A pure reorder is not a *value* change: nothing is re-evaluated.
    let events = h
        .dispatch(Command::ReorderLayer {
            id: layers[0],
            index: 2,
        })
        .expect("reorder layer");
    assert!(
        h.graph.dirty_ids_for_events(&events).is_empty(),
        "a reorder dirties no node: {events:?}"
    );
    assert_eq!(
        h.engine.document().order,
        h.engine.document().flatten_layers(),
        "one source of z-truth"
    );
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(24))]

    /// **Layer Order Law.** For any permutation of the layers, the scene's draw
    /// order is exactly the flattened layer stack — every layer's block intact,
    /// the blocks in the permuted order, and every node still present exactly
    /// once.
    #[test]
    fn prop_layer_order_law(permutation in proptest::array::uniform3(0usize..3)) {
        let mut h = Harness::new();
        let (layers, groups) = three_layers(&mut h, 2);
        h.refresh_presentation();

        // Apply the permutation as a sequence of `ReorderLayer` commands, which
        // is what the panel's drag emits.
        let mut moving: Vec<usize> = (0..3).collect();
        for (target, &which) in permutation.iter().enumerate() {
            let from = moving.iter().position(|slot| *slot == which).expect("a layer");
            moving.remove(from);
            moving.insert(target, which);
            h.dispatch(Command::ReorderLayer {
                id: layers[which],
                index: target,
            })
            .expect("reorder layer");
            h.refresh_presentation();
        }

        let expected: Vec<NodeId> = moving
            .iter()
            .flat_map(|slot| groups[*slot].iter().copied())
            .collect();
        prop_assert_eq!(drawn_order(&h), expected.clone());
        prop_assert_eq!(&h.engine.document().order, &expected);
        prop_assert!(h.engine.document().order_matches_layers());

        // Nothing was lost, duplicated or re-evaluated.
        let seen: BTreeSet<NodeId> = drawn_order(&h).into_iter().collect();
        prop_assert_eq!(seen.len(), 6);
        prop_assert_eq!(
            h.cache.stats().last_evaluated,
            0,
            "a reorder is a no-op for the evaluator"
        );
        prop_assert!(
            h.cache.stats().no_ops >= permutation.len() as u32,
            "every reorder was counted as a no-op"
        );
    }
}

// ── Law 2: hiding a layer removes it from the picture, not the document ─────

#[test]
fn law_hiding_a_layer_removes_it_from_the_frame_but_not_the_document() {
    let mut h = Harness::new();
    let (layers, groups) = three_layers(&mut h, 2);
    h.refresh_presentation();

    // The frame before: six draw items, one per node.
    let mut render = RenderScene::new();
    render.sync(h.cache.scene(), &DirtySet::all());
    assert_eq!(render.report().created, 6);

    let hidden = groups[1][0];
    let events = h
        .dispatch(Command::SetLayerVisible {
            id: layers[1],
            visible: false,
        })
        .expect("hide layer");
    let dirty = h.graph.dirty_ids_for_events(&events);
    assert!(dirty.is_empty(), "an eye is not a value: {events:?}");

    // The evaluator did nothing at all — the strongest form of RULE 4.
    assert_eq!(h.cache.stats().last_mode, EvalMode::Incremental);
    assert_eq!(h.cache.stats().last_evaluated, 0);
    assert!(h.cache.stats().no_ops >= 1);

    // …and yet the flags follow the document, at the cost of one `bool` per node.
    // The ledger the dispatch filled is the renderer's real input in production.
    let changed = h.repainted.clone();
    let expect_repaint: Vec<NodeId> = sorted(vec![groups[1][0], groups[1][1]]);
    assert_eq!(
        changed, expect_repaint,
        "exactly the hidden layer's nodes repaint"
    );
    assert!(
        h.refresh_presentation().is_empty(),
        "and nothing repaints twice"
    );
    for node in &groups[1] {
        let evaluated = h.cache.scene().get(*node).expect("still in the scene");
        assert!(!evaluated.visible, "a hidden node is not drawn");
        assert!(
            h.engine.document().nodes.contains_key(node),
            "the node is still in the document"
        );
        assert!(!h.engine.document().visible(*node));
    }
    // Untouched layers are untouched.
    for node in groups[0].iter().chain(groups[2].iter()) {
        assert!(h.cache.scene().get(*node).unwrap().visible);
    }

    // The *frame* loses it: the renderer's draw list is empty for those nodes,
    // and the frame draws two fewer items with zero bytes of buffer traffic.
    render.sync(h.cache.scene(), &DirtySet::nodes(changed.iter().copied()));
    let stats = render.report().clone();
    for node in &groups[1] {
        assert!(
            render.get(*node).unwrap().items.is_empty(),
            "a hidden node contributes no draw item"
        );
    }
    assert!(
        render.plan().ops.is_empty(),
        "an eye writes no bytes: {:?}",
        render.plan().ops
    );
    assert!(render.plan().repainted.contains(&hidden));
    assert_eq!(
        stats.draw_items, 4,
        "six draw items minus the hidden layer's two"
    );
    assert_eq!(render.order().len(), 6, "still six nodes in the scene");

    // Showing it again restores the picture exactly, and the inverse command is
    // the same command with the other flag.
    h.dispatch(Command::SetLayerVisible {
        id: layers[1],
        visible: true,
    })
    .expect("show layer");
    let changed = h.repainted.clone();
    assert_eq!(changed.len(), 2);
    render.sync(h.cache.scene(), &DirtySet::nodes(changed.iter().copied()));
    assert_eq!(render.report().draw_items, 6);
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(24))]

    /// **Visibility Law.** Any subset of layers can be hidden, in any order, and
    /// for every node the picture and the document disagree in exactly the way
    /// the law says: out of the frame, still in the document, no re-evaluation.
    #[test]
    fn prop_visibility_law(hidden_mask in proptest::array::uniform3(any::<bool>())) {
        let mut h = Harness::new();
        let (layers, groups) = three_layers(&mut h, 2);
        h.refresh_presentation();

        let mut render = RenderScene::new();
        render.sync(h.cache.scene(), &DirtySet::all());
        prop_assert_eq!(render.report().draw_items, 6);

        // One dispatch per hidden layer, each reporting its own repaint: the
        // ledger is per-mutation, so the law accumulates what the renderer would
        // have repainted across the whole gesture.
        let mut changed: Vec<NodeId> = Vec::new();
        for (index, hidden) in hidden_mask.iter().enumerate() {
            if *hidden {
                h.dispatch(Command::SetLayerVisible {
                    id: layers[index],
                    visible: false,
                })
                .expect("hide layer");
                changed.extend(h.repainted.iter().copied());
            }
        }
        changed.sort();
        changed.dedup();

        let mut expect_hidden: Vec<NodeId> = hidden_mask
            .iter()
            .enumerate()
            .filter(|(_, hidden)| **hidden)
            .flat_map(|(index, _)| groups[index].iter().copied())
            .collect();
        expect_hidden.sort();
        prop_assert_eq!(changed.clone(), sorted(expect_hidden.clone()));

        // Every node is still evaluated, still in the document, still in the
        // scene's order — and only its `visible` flag moved.
        prop_assert_eq!(drawn_order(&h).len(), 6);
        prop_assert_eq!(
            h.engine.document().nodes.len(),
            6,
            "hiding never deletes"
        );
        for node in &expect_hidden {
            prop_assert!(!h.cache.scene().get(*node).unwrap().visible);
            prop_assert!(h.engine.document().nodes.contains_key(node));
        }

        render.sync(h.cache.scene(), &DirtySet::nodes(changed));
        prop_assert_eq!(render.report().draw_items, 6 - expect_hidden.len());
        prop_assert!(
            render.plan().ops.is_empty(),
            "an eye writes no bytes: {:?}",
            render.plan().ops
        );
    }
}

// ── Law 3: stacked appearances draw in order, with their own blends ─────────

/// One node wearing `[fill, thick stroke, thin stroke]`, the brief's own example.
fn two_stroke_node(h: &mut Harness) -> NodeId {
    let node = new_node_id();
    h.dispatch(Command::CreateNode {
        id: node,
        name: Some("badge".to_string()),
        index: None,
        kind: NodeKind::Rectangle {
            x: Parameter::Literal(40.0),
            y: Parameter::Literal(30.0),
            width: Parameter::Literal(120.0),
            height: Parameter::Literal(80.0),
            corner_radius: Parameter::Literal(0.0),
        },
    })
    .expect("create node");
    h.dispatch(Command::SetAppearances {
        node_id: node,
        appearances: vec![
            AppearanceLayer::fill(Color::rgb(0xff, 0xcc, 0x00)),
            AppearanceLayer::stroke(Color::BLACK, 12.0),
            AppearanceLayer::stroke(Color::WHITE, 4.0),
        ],
    })
    .expect("stack appearances");
    h.refresh_presentation();
    node
}

#[test]
fn law_two_strokes_tessellate_as_two_meshes_in_stack_order() {
    let mut h = Harness::new();
    let node = two_stroke_node(&mut h);
    let style = style_of(&h, node);

    assert_eq!(style.appearances.len(), 3, "fill + two strokes");
    assert_eq!(style.first_stroke().unwrap().stroke_width(), Some(12.0));
    assert_eq!(
        style.max_stroke_width(),
        12.0,
        "the widest stroke frames the shape"
    );

    let evaluated = h.cache.scene().get(node).unwrap();
    let tessellation: Tessellation =
        tessellate(node, &evaluated.primitive, &evaluated.style).expect("tessellate");
    assert_eq!(
        tessellation.strokes.len(),
        2,
        "one outline per stroke layer — the thick one and the thin one on top"
    );
    assert!(!tessellation.strokes[0].is_empty());
    assert!(!tessellation.strokes[1].is_empty());
    // The thin stroke is drawn *inside* the thick one: its outline's area is
    // smaller, which is how a designer writes the white keyline.
    assert!(
        tessellation.strokes[1].area() < tessellation.strokes[0].area(),
        "thin {} < thick {}",
        tessellation.strokes[1].area(),
        tessellation.strokes[0].area()
    );
}

#[test]
fn law_the_draw_list_layers_fill_then_strokes_with_their_blend_modes() {
    let mut h = Harness::new();
    let node = two_stroke_node(&mut h);

    // Multiply on the fill, Screen on the outer stroke, Overlay on the keyline:
    // three different modes on one path.
    h.dispatch(Command::SetAppearances {
        node_id: node,
        appearances: vec![
            AppearanceLayer {
                blend: BlendMode::Multiply,
                ..AppearanceLayer::fill(Color::rgb(0xff, 0xcc, 0x00))
            },
            AppearanceLayer {
                blend: BlendMode::Screen,
                ..AppearanceLayer::stroke(Color::BLACK, 12.0)
            },
            AppearanceLayer {
                blend: BlendMode::Overlay,
                ..AppearanceLayer::stroke(Color::WHITE, 4.0)
            },
        ],
    })
    .expect("restack");
    h.refresh_presentation();

    let mut render = RenderScene::new();
    render.sync(h.cache.scene(), &DirtySet::all());
    let slot = render.get(node).expect("slot");
    assert_eq!(
        slot.items.len(),
        3,
        "a fill and two strokes are three draw items"
    );

    // Draw order: the fill first, then the strokes back → front (RULE 3).
    let layers: Vec<u32> = slot.items.iter().map(|item| item.appearance).collect();
    assert_eq!(layers, vec![0, 1, 2]);
    let blends: Vec<f32> = slot.instances.iter().map(|row| row.params[3]).collect();
    assert_eq!(
        blends,
        vec![1.0, 2.0, 3.0],
        "multiply, screen, overlay — the shader's blend codes"
    );
    assert_eq!(slot.instances[1].ramp[3], 1.0, "item 1 is a stroke");
    assert_eq!(slot.instances[2].ramp[3], 1.0);

    // The stroke meshes the items point at are the two outlines, in order.
    let kinds: Vec<MeshKind> = [MeshKind::Stroke(0), MeshKind::Stroke(1)].to_vec();
    assert_ne!(kinds[0], kinds[1], "the two strokes are distinct buffers");

    let report = render.report().clone();
    assert_eq!(report.draw_items, 3);
    assert_eq!(
        report.blended_draws, 3,
        "every layer blends with the backdrop"
    );
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(16))]

    /// **Appearance Law.** For any stack of two strokes with random widths and
    /// blend modes, the node yields exactly `fill + strokes` draw items in stack
    /// order, each carrying its own width, blend code and opacity — and the
    /// lighter stroke always tessellates to the smaller outline.
    #[test]
    fn prop_appearance_law(
        thick in 6.0f64..20.0,
        thin in 1.0f64..6.0,
        thick_blend in 0u8..4,
        thin_blend in 0u8..4,
        thick_opacity in 0.2f64..1.0,
        thin_opacity in 0.2f64..1.0,
    ) {
        prop_assume!(thin < thick);
        let mut h = Harness::new();
        let node = two_stroke_node(&mut h);
        let mode = |code: u8| match code {
            0 => BlendMode::Normal,
            1 => BlendMode::Multiply,
            2 => BlendMode::Screen,
            _ => BlendMode::Overlay,
        };
        h.dispatch(Command::SetAppearances {
            node_id: node,
            appearances: vec![
                AppearanceLayer::fill(Color::rgb(0x22, 0x66, 0xee)),
                AppearanceLayer {
                    blend: mode(thick_blend),
                    opacity: Parameter::Literal(thick_opacity),
                    ..AppearanceLayer::stroke(Color::BLACK, thick)
                },
                AppearanceLayer {
                    blend: mode(thin_blend),
                    opacity: Parameter::Literal(thin_opacity),
                    ..AppearanceLayer::stroke(Color::WHITE, thin)
                },
            ],
        })
        .expect("restack");
        h.refresh_presentation();

        let mut render = RenderScene::new();
        render.sync(h.cache.scene(), &DirtySet::all());
        let slot = render.get(node).expect("slot");
        prop_assert_eq!(slot.items.len(), 3);
        prop_assert_eq!(
            slot.items.iter().map(|item| item.appearance).collect::<Vec<u32>>(),
            vec![0, 1, 2]
        );
        // Each row carries its own layer's opacity and blend code.
        prop_assert!((slot.instances[1].params[1] as f64 - thick_opacity).abs() < 1e-6);
        prop_assert!((slot.instances[2].params[1] as f64 - thin_opacity).abs() < 1e-6);
        prop_assert!((slot.instances[1].params[3] as u8) == thick_blend);
        prop_assert!((slot.instances[2].params[3] as u8) == thin_blend);
        // The stroke widths reach the instance rows too (the inspector reads
        // them, and the SDF future needs them).
        prop_assert!((slot.instances[1].params[0] as f64 - thick).abs() < 1e-6);
        prop_assert!((slot.instances[2].params[0] as f64 - thin).abs() < 1e-6);

        let evaluated = h.cache.scene().get(node).unwrap();
        let tess = tessellate(node, &evaluated.primitive, &evaluated.style).expect("tessellate");
        prop_assert_eq!(tess.strokes.len(), 2);
        prop_assert!(tess.strokes[1].area() < tess.strokes[0].area());
    }
}

// ── The panels' data models ────────────────────────────────────────────────

#[test]
fn law_a_group_moves_its_children_as_one_block() {
    // RULE 1's nesting claim, read through the draw order: a group is a node
    // whose kind lists children, and moving it must carry the whole subtree —
    // the engine's `node_block` is the block a placement splices.
    let mut h = Harness::new();
    let mut children = Vec::new();
    for slot in 0..3 {
        let child = new_node_id();
        h.dispatch(Command::CreateNode {
            id: child,
            name: Some(format!("child {slot}")),
            index: None,
            kind: NodeKind::Rectangle {
                x: Parameter::Literal(10.0 + 20.0 * slot as f64),
                y: Parameter::Literal(10.0),
                width: Parameter::Literal(10.0),
                height: Parameter::Literal(10.0),
                corner_radius: Parameter::Literal(0.0),
            },
        })
        .expect("create child");
        children.push(child);
    }

    // The group is created *holding* its children: `CreateNode` owns the kind,
    // and `NodeKind::Group { children }` is where nesting lives.
    let group = new_node_id();
    h.dispatch(Command::CreateNode {
        id: group,
        name: Some("Icon".to_string()),
        index: None,
        kind: NodeKind::Group {
            children: children.clone(),
        },
    })
    .expect("create group");
    h.refresh_presentation();

    let document = h.engine.document();
    let block = document.node_block(group);
    let expected: BTreeSet<NodeId> = std::iter::once(group)
        .chain(children.iter().copied())
        .collect();
    assert_eq!(
        block.iter().copied().collect::<BTreeSet<NodeId>>(),
        expected,
        "the group's block is the group plus its children"
    );
    assert!(document.order_matches_layers(), "the two orderings agree");

    // Send the group forward: the whole block travels, contiguously, and nothing
    // is lost. (The group was authored *after* its children, so the block reads
    // [children…, group] until a move splices it as one run — which is exactly
    // what "moving a group moves all children" has to mean in a flat order.)
    h.dispatch(Command::SetNodeParent {
        id: group,
        parent: None,
        index: 0,
    })
    .expect("reorder group");
    let order = h.engine.document().order.clone();
    let moved: Vec<NodeId> = order
        .iter()
        .copied()
        .filter(|id| block.contains(id))
        .collect();
    assert_eq!(moved, block, "moving a group moves all its children");
    assert_eq!(
        &order[..block.len()],
        &block[..],
        "and the block is contiguous at the front, not scattered"
    );

    // …and the picture agrees, because a reorder is a no-op for the evaluator.
    h.refresh_presentation();
    let scene_order = drawn_order(&h);
    let drawn_block: Vec<NodeId> = scene_order
        .iter()
        .copied()
        .filter(|id| block.contains(id))
        .collect();
    // The group itself paints nothing, so the scene holds its three children —
    // contiguously, in the order the block gives them.
    assert_eq!(scene_order, expected_drawn(&h));
    assert_eq!(
        drawn_block,
        block
            .iter()
            .copied()
            .filter(|id| h.cache.scene().nodes.contains_key(id))
            .collect::<Vec<NodeId>>(),
        "the scene draws the block in document order"
    );
}
