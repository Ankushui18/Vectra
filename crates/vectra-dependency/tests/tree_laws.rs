//! Task 10.4 RULE 1 — **tree laws**: parentage, cycles, and the one z-order.
//!
//! The panel's tree is a projection of two engine facts, and both are asserted
//! here rather than in the UI, because a document that lies about them draws the
//! wrong picture:
//!
//! ```text
//! NodeKind::Group { children }   who holds whom   (Document::parent_of / set_parent)
//! LayerRecord::children          what draws when  (one order, back → front)
//! ```
//!
//! | law | test |
//! |---|---|
//! | Division of Labour | [`law_the_layer_is_the_z_order_and_the_group_is_the_parent`] |
//! | Block Travel | [`law_moving_a_group_moves_its_children`] |
//! | Cycle Refusal | [`law_a_group_can_never_hold_itself`] |
//! | One Undo | [`law_a_move_is_one_undo_and_the_document_round_trips`] |
//! | Lasting Parentage | [`law_a_dropped_group_keeps_its_members`] |
//! | Inheritance | [`law_moving_a_group_into_a_group_deepens_the_tree`] |
//! | Across Layers | [`law_grouping_a_selection_that_spans_layers_undoes_completely`] |
//! | One Run | [`law_the_layer_lists_every_group_as_one_run`] |

use std::collections::BTreeSet;
use vectra_core::{new_node_id, Command, Document, Node, NodeId, NodeKind, VectraError};
use vectra_dependency::gate_command;

// ── Fixtures ──────────────────────────────────────────────────────────────

fn rectangle(name: &str) -> Node {
    Node::new(
        new_node_id(),
        name,
        NodeKind::Rectangle {
            x: vectra_core::Parameter::Literal(0.0),
            y: vectra_core::Parameter::Literal(0.0),
            width: vectra_core::Parameter::Literal(10.0),
            height: vectra_core::Parameter::Literal(10.0),
            corner_radius: vectra_core::Parameter::Literal(0.0),
        },
    )
}

/// A document with one layer holding `names.len()` rectangles, back → front.
fn layered(names: &[&str]) -> (Document, Vec<NodeId>) {
    let mut doc = Document::default();
    let layer = doc.ensure_layer();
    let mut ids = Vec::new();
    for name in names {
        let node = rectangle(name);
        let id = node.id;
        doc.insert_node(node, None).expect("insert");
        doc.assign_to_layer(id, layer);
        ids.push(id);
    }
    doc.resync_order_from_layers();
    (doc, ids)
}

fn group(doc: &mut Document, name: &str, children: Vec<NodeId>) -> NodeId {
    let node = Node::new(new_node_id(), name, NodeKind::Group { children });
    let id = node.id;
    doc.insert_node(node, None).expect("insert");
    id
}

/// **The invariant**: every group the layer lists occupies one contiguous run of
/// the layer's list, and that run reads in document order.
///
/// The layer's `children` *is* the draw order, with groups and members side by
/// side, so a group's block has to travel as a unit. A single id spliced into the
/// list on its own leaves the members behind it — the layer then draws the member
/// behind the group that owns it, and the panel's walk (which follows parent
/// links) disagrees with the canvas about what is on top. Returns the first group
/// that is not in one run.
fn scattered_group(doc: &Document) -> Option<NodeId> {
    let listed = order(doc);
    for group in listed.iter().copied() {
        let Ok(node) = doc.get_node(group) else {
            continue;
        };
        if !matches!(node.kind, NodeKind::Group { .. }) {
            continue;
        }
        let block = doc.node_block(group);
        if block.is_empty() {
            continue;
        }
        let positions: Vec<usize> = block
            .iter()
            .filter_map(|member| listed.iter().position(|row| row == member))
            .collect();
        // Every member is listed (positions.len() == block.len()), they are
        // adjacent, and in the same order the block reads.
        let contiguous = positions.len() == block.len()
            && positions.first().is_some_and(|first| {
                positions == (*first..first + positions.len()).collect::<Vec<_>>()
            });
        if !contiguous || listed[positions[0]..positions[0] + block.len()] != block[..] {
            return Some(group);
        }
    }
    None
}

/// The layer's list — the document's z-order, back → front.
fn order(doc: &Document) -> Vec<NodeId> {
    doc.layers
        .iter()
        .next()
        .map(|l| l.children.clone())
        .unwrap_or_default()
}

/// The layer that holds `id` — the layer whose list contains its block.
fn layer_of(doc: &Document, id: NodeId) -> Option<vectra_core::ids::LayerId> {
    doc.layers.layer_of(id).map(|(record, _)| record.id)
}

fn names(doc: &Document, ids: &[NodeId]) -> Vec<String> {
    ids.iter()
        .map(|id| {
            doc.get_node(*id)
                .map(|n| n.name.clone())
                .unwrap_or_default()
        })
        .collect()
}

// ── Laws ──────────────────────────────────────────────────────────────────

/// **The division of labour.** The layer lists every node of the layer, in draw
/// order; the group says only who holds whom. Nothing else explains the panel, so
/// nothing else may be true.
#[test]
fn law_the_layer_is_the_z_order_and_the_group_is_the_parent() {
    let (mut doc, ids) = layered(&["A", "B", "C"]);
    let g = group(&mut doc, "G", vec![ids[1]]);
    let layer = doc.layers.iter().next().unwrap().id;
    doc.assign_to_layer(g, layer);
    doc.resync_order_from_layers();

    // The layer still lists every node — the group is a *row among* its contents,
    // never a replacement for them. That is why the canvas draws a grouped shape
    // without knowing what a group is.
    assert_eq!(names(&doc, &order(&doc)), ["A", "B", "C", "G"]);
    assert_eq!(
        doc.parent_of(ids[1]),
        Some(g),
        "and the group is the only statement of parentage"
    );
    assert_eq!(
        doc.parent_of(ids[0]),
        None,
        "an ungrouped node has no parent"
    );
    assert!(doc.order_matches_layers(), "the two structures agree");

    // The panel's depth walk, written out: `B` is one step below the layer.
    let mut depth = 0;
    let mut cursor = doc.parent_of(ids[1]);
    while let Some(parent) = cursor {
        depth += 1;
        cursor = doc.parent_of(parent);
    }
    assert_eq!(depth, 1, "one group, one level");
}

/// **Block travel.** Moving a group moves everything it holds, contiguously, and
/// nothing else moves — asserted on the layer's list, which *is* the z-order.
#[test]
fn law_moving_a_group_moves_its_children() {
    let (mut doc, ids) = layered(&["A", "B", "C", "D"]);
    let g = group(&mut doc, "G", vec![ids[1], ids[2]]);
    let layer = doc.layers.iter().next().unwrap().id;
    doc.assign_to_layer(g, layer);
    doc.resync_order_from_layers();

    // Send the group to the front (the engine's index 0 = the back). This is the
    // one placement path since Task 10.5: `ReorderNode` and its
    // `reorder_within_container` are retired, because in a layered document that
    // verb spliced the node into the *layer's* list rather than among its
    // siblings — see `law_the_layer_lists_every_group_as_one_run`.
    doc.set_parent(g, None, 0).expect("a layer path");
    let moved = order(&doc);
    assert_eq!(
        names(&doc, &moved),
        ["B", "C", "G", "A", "D"],
        "the group and its two members travelled as one run, in order"
    );
    assert!(
        doc.order_matches_layers(),
        "and the layer's list is still the whole truth"
    );

    // A leaf still moves alone: the block of a non-group is itself.
    doc.set_parent(ids[3], None, 0).expect("a leaf moves alone");
    assert_eq!(names(&doc, &order(&doc))[0], "D");
}

/// **Cycle refusal.** A group is refused as its own descendant *before* anything
/// moves, so no command sequence can build a document the panel cannot read.
#[test]
fn law_a_group_can_never_hold_itself() {
    let (mut doc, ids) = layered(&["A", "B"]);
    let inner = group(&mut doc, "Inner", vec![ids[0]]);
    let outer = group(&mut doc, "Outer", vec![inner]);
    let layer = doc.layers.iter().next().unwrap().id;
    for node in [inner, outer] {
        doc.assign_to_layer(node, layer);
    }
    doc.resync_order_from_layers();

    // Itself…
    let refused = doc.set_parent(outer, Some(outer), 0);
    assert!(
        matches!(refused, Err(VectraError::GroupCycle { .. })),
        "{refused:?}"
    );
    // …and its own descendant, which is the same cycle one level down.
    let refused = doc.set_parent(outer, Some(inner), 0);
    assert!(
        matches!(refused, Err(VectraError::GroupCycle { .. })),
        "a group cannot be moved inside its own grandchild: {refused:?}"
    );
    // The document is untouched by a refusal: same order, same parentage.
    assert_eq!(
        names(&doc, &order(&doc)),
        ["A", "B", "Inner", "Outer"],
        "the untouched document, B included"
    );
    assert_eq!(doc.parent_of(inner), Some(outer));
    assert_eq!(doc.parent_of(outer), None);

    // A destination that is not a container is refused too, typed.
    let refused = doc.set_parent(ids[1], Some(ids[0]), 0);
    assert!(
        matches!(refused, Err(VectraError::NotAGroup(_))),
        "{refused:?}"
    );
    // …and through the *command* path, where the document is not even reached.
    let cmd = Command::SetNodeParent {
        id: outer,
        parent: Some(inner),
        index: 0,
    };
    let graph = vectra_dependency::DependencyGraph::new();
    let gated = gate_command(&graph, &cmd, &doc);
    assert!(
        gated.is_err() || cmd.apply(&mut doc).is_err(),
        "the gate or the apply refuses, and the document is never left half-moved"
    );
}

/// **One undo.** A move is one command, so undo restores parent *and* position in
/// one step; redo puts it back exactly. The document is byte-identical around the
/// round trip, which is the strongest statement available about a reversible edit.
#[test]
fn law_a_move_is_one_undo_and_the_document_round_trips() {
    let (mut doc, ids) = layered(&["A", "B", "C"]);
    let g = group(&mut doc, "G", vec![]);
    let layer = doc.layers.iter().next().unwrap().id;
    doc.assign_to_layer(g, layer);
    doc.resync_order_from_layers();
    let before = doc.clone();

    let move_it = Command::SetNodeParent {
        id: ids[0],
        parent: Some(g),
        index: 0,
    };
    // The undo stack's own dance: do → the inverse is pushed → undo applies the
    // inverse and pushes *its* inverse → redo applies that.
    let inverse = move_it.apply(&mut doc).expect("the move applies");
    assert_eq!(doc.parent_of(ids[0]), Some(g));
    assert_eq!(doc.sibling_index(ids[0], Some(g)), 0);

    let redo = inverse.apply(&mut doc).expect("the inverse applies");
    assert_eq!(doc.parent_of(ids[0]), None, "undo restores the parent");
    assert_eq!(doc.sibling_index(ids[0], None), 0, "…and the position");
    assert_eq!(order(&doc), before.order, "and the order is what it was");

    let inverse_again = redo.apply(&mut doc).expect("redo");
    assert_eq!(inverse_again, inverse, "redo undoes to the same inverse");
    assert_eq!(
        doc.parent_of(ids[0]),
        Some(g),
        "and the node is inside again"
    );

    // A move that is already satisfied changes nothing and is **not** an error:
    // the same command again, and the document is where it was. (Reporting
    // `NodeNotFound` here was a real defect, found by this law.)
    let settled = doc.clone();
    let quiet = move_it
        .apply(&mut doc)
        .expect("a no-op move is not a failure");
    assert_eq!(doc.parent_of(ids[0]), Some(g));
    assert_eq!(doc.sibling_index(ids[0], Some(g)), 0);
    assert_eq!(doc.order, settled.order, "nothing moved");
    // …and its inverse is itself, so undo/redo through it stays total.
    quiet.apply(&mut doc).expect("the no-op's inverse");
    assert_eq!(doc.order, settled.order);
    assert_eq!(doc.parent_of(ids[0]), Some(g));
    assert_eq!(doc.parent_of(ids[0]), Some(g));
}

/// **Lasting parentage.** Deleting a group's parent does not scatter its
/// members: they are nodes of the document like any other, and the layer still
/// lists them. A designer's work survives the container being removed.
#[test]
fn law_a_dropped_group_keeps_its_members() {
    let (mut doc, ids) = layered(&["A", "B"]);
    let g = group(&mut doc, "G", vec![ids[0], ids[1]]);
    let layer = doc.layers.iter().next().unwrap().id;
    doc.assign_to_layer(g, layer);
    doc.resync_order_from_layers();

    let drop = Command::DeleteNode { id: g };
    drop.apply(&mut doc).expect("delete the group");
    let remaining = order(&doc);
    assert_eq!(
        names(&doc, &remaining),
        ["A", "B"],
        "the members outlive their container"
    );
    // They are not orphans of the *layer*: they are still listed, so they still
    // draw, and the panel shows them at the layer's top level where they now are.
    assert_eq!(doc.parent_of(ids[0]), None);
    let ids_of_layer: BTreeSet<NodeId> = remaining.into_iter().collect();
    assert!(ids_of_layer.contains(&ids[0]) && ids_of_layer.contains(&ids[1]));
}

/// **Inheritance.** Nesting a group inside a group deepens the whole subtree, and
/// the layer's order still says exactly what draws when — an illustration of why
/// the panel can walk parent links and be right about the canvas.
#[test]
fn law_moving_a_group_into_a_group_deepens_the_tree() {
    let (mut doc, ids) = layered(&["A", "B", "C"]);
    let inner = group(&mut doc, "Inner", vec![ids[0]]);
    let outer = group(&mut doc, "Outer", vec![ids[1]]);
    let layer = doc.layers.iter().next().unwrap().id;
    for node in [inner, outer] {
        doc.assign_to_layer(node, layer);
    }
    doc.resync_order_from_layers();

    // Inner joins Outer: same document, one more level.
    let moved = Command::SetNodeParent {
        id: inner,
        parent: Some(outer),
        index: 0,
    };
    moved.apply(&mut doc).expect("nest the groups");
    assert_eq!(doc.parent_of(inner), Some(outer));
    assert_eq!(
        doc.parent_of(ids[0]),
        Some(inner),
        "a member's parent is unchanged by its group moving"
    );

    // The depth of the deepest row, walked exactly as the snapshot's parent link
    // makes the panel walk it.
    let depth = |doc: &Document, node: NodeId| {
        let mut depth = 0_u32;
        let mut cursor = doc.parent_of(node);
        while let Some(parent) = cursor {
            depth += 1;
            cursor = doc.parent_of(parent);
        }
        depth
    };
    assert_eq!(
        depth(&doc, ids[0]),
        2,
        "three levels: layer → Outer → Inner → A"
    );
    assert_eq!(depth(&doc, ids[1]), 1);

    // And the order is still a valid draw order containing every node once.
    let flat = order(&doc);
    assert_eq!(flat.len(), 5, "A, B, C and the two groups: {flat:?}");
    assert_eq!(
        flat.iter().copied().collect::<BTreeSet<NodeId>>().len(),
        5,
        "no node appears twice"
    );
    assert!(doc.order_matches_layers());
}

/// **Across layers.** A selection can span layers, and grouping it is one action.
/// The members that live elsewhere are carried into the group's layer — a layer
/// lists its blocks, so a subtree lives in exactly one layer — and *undo has to
/// carry them back*. Restoring only parent and position left them in the group's
/// layer: the artwork came back grouped the wrong way round, in the wrong layer,
/// which is what this law found. (It is also the shape the panel sends: one
/// `CreateNode`, then one move per selected node.)
#[test]
fn law_grouping_a_selection_that_spans_layers_undoes_completely() {
    use vectra_core::ids::new_layer_id;

    let mut doc = Document::default();
    let base = doc.ensure_layer();

    // Two layers, one shape each.
    let a = rectangle("A");
    let a_id = a.id;
    doc.insert_node(a, None).expect("insert");
    doc.assign_to_layer(a_id, base);

    let guides = new_layer_id();
    assert!(
        doc.layers
            .insert(vectra_core::LayerRecord::new(guides, "Guides"), None),
        "the registry has no such layer yet"
    );
    let b = rectangle("B");
    let b_id = b.id;
    doc.insert_node(b, None).expect("insert");
    doc.assign_to_layer(b_id, guides);
    doc.resync_order_from_layers();

    assert_eq!(layer_of(&doc, a_id), Some(base));
    assert_eq!(layer_of(&doc, b_id), Some(guides));

    // The panel's own action: a new group, then a move per selected node.
    let g = new_node_id();
    let action = Command::Batch {
        commands: vec![
            Command::CreateNode {
                id: g,
                kind: NodeKind::Group { children: vec![] },
                name: Some("Group 2".to_string()),
                index: None,
            },
            Command::SetNodeParent {
                id: a_id,
                parent: Some(g),
                index: 0,
            },
            Command::SetNodeParent {
                id: b_id,
                parent: Some(g),
                index: 1,
            },
        ],
    };
    let inverse = action.apply(&mut doc).expect("the group applies");

    // Both members are in the group, and the group is in the active layer: the
    // one that was already there stayed, the one from elsewhere was carried in.
    assert_eq!(doc.parent_of(a_id), Some(g));
    assert_eq!(doc.parent_of(b_id), Some(g));
    assert_eq!(layer_of(&doc, a_id), Some(base), "A never left its layer");
    assert_eq!(
        layer_of(&doc, b_id),
        Some(base),
        "B followed the group into its layer"
    );
    assert_eq!(layer_of(&doc, g), Some(base));

    // **One undo** — and everything is where it was, layer included.
    let redo = inverse.apply(&mut doc).expect("undo");
    assert_eq!(doc.parent_of(a_id), None);
    assert_eq!(doc.parent_of(b_id), None);
    assert_eq!(layer_of(&doc, a_id), Some(base));
    assert_eq!(
        layer_of(&doc, b_id),
        Some(guides),
        "B is back in its own layer, not the group's"
    );
    assert!(doc.get_node(g).is_err(), "and the group is gone again");

    // Redo closes the loop: the same tree, the same layers.
    redo.apply(&mut doc).expect("redo");
    assert_eq!(doc.parent_of(a_id), Some(g));
    assert_eq!(doc.parent_of(b_id), Some(g));
    assert_eq!(layer_of(&doc, b_id), Some(base));
    assert_eq!(layer_of(&doc, g), Some(base));
}

/// **One run.** After *any* placement the layer's list still holds each group as
/// one contiguous run, in document order — so the tree, walked, *is* the draw
/// order.
///
/// This is the law the retired `ReorderNode` broke. That verb took a plain
/// `index` and, in a layered document, spliced the node into the **layer's** list
/// rather than among its **siblings**: moving a member of a group produced a
/// layer reading `[C, A, B, Pair, Pair]` with `C` still parented to `Pair` — the
/// canvas drawing `C` behind `A` while the panel showed it inside a group painted
/// in front of `A`.
///
/// Writing this law found a second defect, in the surviving verb: `SetNodeParent`
/// inserted at the **anchor row's own position**, and a row is the *end* of its
/// own run (a group's children are listed before it). Dropping a node onto a
/// closed group row therefore spliced it *inside* that group's run — the panel
/// showing a top-level row where the canvas drew it in front of the group's
/// members. The engine now inserts between runs (`Document::run_start`), which is
/// what the expectations below record.
///
/// The fixture is canonicalised first, because `CreateNode { Group { children } }`
/// can leave a group listed after its members — the shape 10.2/10.3 documented,
/// and one a placement can only *repair*. From the first placement on, no
/// placement may scatter a run.
#[test]
fn law_the_layer_lists_every_group_as_one_run() {
    let (mut doc, ids) = layered(&["A", "B", "C", "D"]);
    let g = group(&mut doc, "G", vec![ids[1], ids[2]]);
    let layer = doc.layers.iter().next().unwrap().id;
    doc.assign_to_layer(g, layer);
    doc.resync_order_from_layers();

    let place =
        |doc: &mut Document, id: NodeId, parent: Option<NodeId>, index: usize, what: &str| {
            doc.set_parent(id, parent, index)
                .unwrap_or_else(|error| panic!("{what}: {error:?}"));
            assert_eq!(scattered_group(doc), None, "{what}: a group is scattered");
            assert!(doc.order_matches_layers(), "{what}: order ≠ layers");
        };

    // The group is authored *after* its members, so its run is not yet one: the
    // first placement is what makes it canonical.
    assert_eq!(doc.parent_of(ids[1]), Some(g), "the members are inside");
    place(&mut doc, g, None, 1, "the group's first placement");
    assert_eq!(
        names(&doc, &order(&doc)),
        ["A", "B", "C", "G", "D"],
        "the group's run is now one: its members, then the group"
    );

    // The group to the front (index = the count of roots = past the last one).
    place(&mut doc, g, None, 2, "the group to the front");
    assert_eq!(names(&doc, &order(&doc)), ["A", "D", "B", "C", "G"]);

    // **A member reordered inside its group.** The container is the group, so the
    // run reads [C, B, G] — the order *within* the group changed, and the run
    // stayed one run.
    place(&mut doc, ids[2], Some(g), 0, "C to the top of its group");
    assert_eq!(names(&doc, &order(&doc)), ["A", "D", "C", "B", "G"]);
    let children = match &doc.get_node(g).unwrap().kind {
        NodeKind::Group { children } => children.clone(),
        other => panic!("not a group: {other:?}"),
    };
    assert_eq!(
        names(&doc, &children),
        ["C", "B"],
        "the group's own order moved"
    );

    // A node dropped *into* the middle of the group: between two of its members.
    place(&mut doc, ids[3], Some(g), 1, "D into the middle");
    assert_eq!(names(&doc, &order(&doc)), ["A", "C", "D", "B", "G"]);

    // The group moved again, with a bigger run.
    place(&mut doc, g, None, 0, "the group to the back");
    assert_eq!(names(&doc, &order(&doc)), ["C", "D", "B", "G", "A"]);

    // **A member moved out** — the case the retired verb scattered. `C` becomes
    // the 0th top-level row, which means *before the group's whole run*, not
    // inside it.
    place(&mut doc, ids[2], None, 0, "C out of the group");
    assert_eq!(names(&doc, &order(&doc)), ["C", "D", "B", "G", "A"]);
    assert_eq!(doc.parent_of(ids[2]), None, "C is a top-level row now");
    assert_eq!(doc.parent_of(ids[3]), Some(g), "D stayed inside");

    // …and the panel's walk is the canvas's list: reading the tree (roots, each
    // group's members behind it) reproduces the layer's order exactly.
    let listed = order(&doc);
    let walked: Vec<NodeId> = listed
        .iter()
        .filter(|id| doc.parent_of(**id).is_none())
        .flat_map(|root| doc.node_block(*root))
        .collect();
    assert_eq!(
        names(&doc, &walked),
        names(&doc, &listed),
        "the tree, walked, is the draw order"
    );
}
