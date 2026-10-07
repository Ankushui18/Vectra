/**
 * The workspace panels' view-model (Task 10.2) — no DOM, no React.
 *
 * The three panels are the visible half of an "ironclad rules" task, and every
 * one of those rules has a *pure* decision inside it that a component should
 * not be making while it renders:
 *
 *  · RULE 1 — which rows the Layers Panel shows, in which order, and what a
 *    drag downwards in the panel means in the engine's back→front z-order;
 *  · RULE 2 — what the artboard dropdown labels each board, and the rectangle
 *    "jump to artboard" hands to the camera;
 *  · RULE 3 — how a resolved stack becomes a document stack, where a dragged
 *    gradient stop is allowed to land, and what the gradient bar draws;
 *  · RULE 4 — when the Appearance Panel is on screen at all.
 *
 * Those decisions live in `engine/panels.ts`, so they are tested here the same
 * way the rest of the view-model is: as functions, with fixtures shaped exactly
 * like the engine's snapshot (the same JSON the wasm boundary sends).
 *
 * Run: `npm run test:ui`.
 */
import assert from 'node:assert/strict';
import { test } from 'node:test';
import {
  addFill,
  addStroke,
  appearanceCaption,
  appearanceStack,
  artboardFrame,
  artboardRows,
  blendLabel,
  dropIndexFor,
  dropNodeAt,
  dropNodeInto,
  isDescendantOf,
  gradientCss,
  hexToColor,
  formatZoom,
  gridStyle,
  layerOrder,
  layerRows,
  panGestureAllowed,
  moveLayer,
  moveStop,
  recolorStop,
  removeLayerAt,
  selectableNodes,
  selectedNode,
  showsAppearancePanel,
  stackToWire,
  swatchColor,
  togglePaint,
  updateLayerAt,
  wheelZoomFactor,
} from '../src/engine/panels';
import { moveNodeToLayer } from '../src/engine/commands';
import type {
  SnapshotAppearanceWire,
  SnapshotNodeWire,
  SnapshotPaintWire,
  SnapshotWire,
} from '../src/engine/wire';

// ── Fixtures: the engine's own wire shapes ─────────────────────────────────

const solid = (color: string): SnapshotPaintWire => ({ type: 'solid', color });

function row(
  kind: 'fill' | 'stroke',
  color: string,
  extra: Partial<SnapshotAppearanceWire> = {},
): SnapshotAppearanceWire {
  return {
    kind,
    paint: solid(color),
    opacity: 1,
    blend: 'normal',
    visible: true,
    width: kind === 'stroke' ? (extra.width ?? 2) : null,
    ...extra,
  };
}

function node(
  id: string,
  name: string,
  flags: Partial<SnapshotNodeWire> = {},
  appearances: SnapshotAppearanceWire[] = [row('fill', '#2266ee')],
): SnapshotNodeWire {
  return {
    id,
    name,
    primitive: { type: 'rect', x: 0, y: 0, w: 10, h: 10, corner_radius: 0 },
    style: {
      fill: '#2266ee',
      stroke: '#00000000',
      stroke_width: 0,
      opacity: 1,
      appearances,
    },
    position: { x: 0, y: 0, x_source: 'literal', y_source: 'literal' },
    visible: true,
    locked: false,
    own_visible: true,
    own_locked: false,
    ...flags,
  };
}

/**
 * Two layers, three nodes, two boards — the smallest document that exercises
 * every panel at once. `Ink` is the active top layer and holds two shapes; the
 * bottom layer `Guides` is hidden and locked by the layer itself, which is the
 * case the row icons have to describe honestly.
 */
function fixture(): SnapshotWire {
  const nodes: Record<string, SnapshotNodeWire> = {
    a: node('a', 'Square', { layer: 'ink', layer_name: 'Ink' }),
    b: node('b', 'Circle', { layer: 'ink', layer_name: 'Ink' }),
    c: node('c', 'Grid', {
      layer: 'guides',
      layer_name: 'Guides',
      visible: false,
      locked: true,
      own_visible: true,
      own_locked: false,
    }),
  };
  return {
    status: 'ok',
    scene: { nodes, z_order: ['c', 'a', 'b'] },
    variables: {},
    expressions: {},
    diagnostics: [],
    graph: { nodes: 3, edges: 0, acyclic: true },
    eval: {
      last_mode: 'incremental',
      last_dirty: 0,
      last_evaluated: 0,
      full_evals: 1,
      incremental_evals: 1,
      no_ops: 0,
    },
    constraints: {},
    operations: {},
    procedural: {},
    procedural_order: [],
    solver: {
      variables: 0,
      constraints: 0,
      edit_variables: 0,
      writes: 0,
      dropped: 0,
      skipped: 0,
      active_edits: 0,
      drag_node: null,
    },
    layers: [
      {
        id: 'guides',
        name: 'Guides',
        visible: false,
        locked: true,
        // Task 10.7 RULE 3's two flags: this fixture is deliberately plain, so
        // the layer-flag tests read their own states.
        alpha_locked: false,
        clipping_mask: false,
        clipped_to: null,
        children: ['c'],
        child_names: ['Grid'],
        child_is_group: [false],
        child_can_open: [false],
        child_parent: [null],
        artboard: 'board-1',
        active: false,
      },
      {
        id: 'ink',
        name: 'Ink',
        visible: true,
        locked: false,
        alpha_locked: false,
        clipping_mask: false,
        clipped_to: 'guides',
        children: ['a', 'b', 'dot', 'ring', 'speck'],
        child_names: ['Square', 'Circle', 'Dot', 'Ring', 'Speck'],
        child_is_group: [false, true, false, true, false],
        child_can_open: [false, true],
        // `b` is a group holding `dot` and `ring`, and `ring` is itself a group
        // holding `speck` — three levels, which is what tells a real tree walk
        // from Task 10.3's two-level projection.
        // Order is the engine's: back → front, so a member precedes its group and
        // `ring` precedes the `speck` it holds.
        child_parent: [null, null, 'b', 'b', 'ring'],
        artboard: 'board-1',
        active: true,
      },
    ],
    artboards: [
      {
        id: 'board-1',
        name: 'Square',
        bounds: [0, 0, 800, 600],
        background: '#ffffff',
        layers: 2,
        active: false,
      },
      {
        id: 'board-2',
        name: 'Icon 16',
        bounds: [880, 0, 16, 16],
        background: '#101820',
        layers: 1,
        active: true,
      },
    ],
    active_layer: 'ink',
    active_artboard: 'board-2',
    can_undo: false,
    can_redo: false,
    time: 0,
  };
}

// ── RULE 1: the Layers Panel ───────────────────────────────────────────────

test('RULE 1: layers read top-first, and a collapsed layer hides its contents', () => {
  const snap = fixture();
  // Collapsed: two rows, the *top* layer (Ink, last in the engine's back→front
  // list) first — the way every design tool lists them.
  const collapsed = layerRows(snap, new Set());
  assert.deepEqual(
    collapsed.map((line) => [line.kind, line.name, line.depth]),
    [
      ['layer', 'Ink', 0],
      ['layer', 'Guides', 0],
    ],
  );
  assert.equal(collapsed[0].active, true, 'the active layer is marked');

  // Expanded: each layer discloses its contents, also top-first, indented one
  // level, and a group is flagged as a folder (its members stay inside it until
  // the group itself is opened — see the depth test below).
  const expanded = layerRows(snap, new Set(['ink', 'guides']));
  assert.deepEqual(
    expanded.map((line) => [line.kind, line.name, line.depth]),
    [
      ['layer', 'Ink', 0],
      ['node', 'Circle', 1],
      ['node', 'Square', 1],
      ['layer', 'Guides', 0],
      ['node', 'Grid', 1],
    ],
  );
  assert.equal(expanded[1].isGroup, true, 'Circle is a group in this document');
  assert.equal(expanded[2].isGroup, false);
});

test('RULE 1: a row reports the effective flags and, when they differ, the own ones', () => {
  const snap = fixture();
  const rows = layerRows(snap, new Set(['ink', 'guides']));
  const grid = rows.find((line) => line.id === 'c');
  assert.ok(grid);
  // The layer's eye hides it and the layer's padlock locks it; the node itself
  // asked for neither, so the row can say *why* the shape is not responding.
  assert.equal(grid.visible, false);
  assert.equal(grid.locked, true);
  assert.equal(grid.ownVisible, true);
  assert.equal(grid.ownLocked, false);

  const square = rows.find((line) => line.id === 'a');
  assert.ok(square);
  assert.equal(square.visible, true);
  assert.equal(square.locked, false);
});

test('RULE 1: a panel drag is the exact inverse of the engine z-order', () => {
  const snap = fixture();
  // Panel order is top-first: [Ink, Guides]; the engine's is back→front.
  assert.deepEqual(layerOrder(snap), ['ink', 'guides']);
  assert.deepEqual(layerOrder(null), []);

  // Dragging the top row down one slot, in a two-layer document, means
  // "become the back-most layer" in the engine.
  assert.equal(dropIndexFor(0, 2), 1);
  assert.equal(dropIndexFor(1, 2), 0);
  // Out-of-range drops clamp instead of throwing: a pointer can leave the list.
  assert.equal(dropIndexFor(9, 3), 0);
  assert.equal(dropIndexFor(-1, 3), 2);
});

test('RULE 1: neither a hidden nor a locked layer contributes a selectable node', () => {
  const snap = fixture();
  // `c` is on the hidden+locked layer, so a click passes through it.
  assert.deepEqual(selectableNodes(snap), ['a', 'b']);
  assert.deepEqual(selectableNodes(null), []);
});

test('RULE 1 (10.4): a tree nests to any depth, siblings in the engine’s order', () => {
  const snap = fixture();

  // Closed, a group is one row with a triangle. `canOpen` is per row: the
  // layer's own triangle is about its contents, a group's about its members.
  const closed = layerRows(snap, new Set(['ink']));
  assert.deepEqual(
    closed.map((line) => [line.name, line.depth, line.canOpen]),
    [
      ['Ink', 0, true],
      ['Circle', 1, true],
      ['Square', 1, false],
      ['Guides', 0, true],
    ],
  );

  // Opened, the tree walks itself: `dot`, `ring` (a group) and `speck` live in
  // the layer's list, but they are rows *inside* the groups that hold them, at
  // the depth the walk reaches them. This is the level Task 10.3 could not show.
  const open = layerRows(snap, new Set(['ink', 'b', 'ring']));
  assert.deepEqual(
    open.map((line) => [line.name, line.depth]),
    [
      ['Ink', 0],
      ['Circle', 1],
      ['Ring', 2],
      ['Speck', 3],
      ['Dot', 2],
      ['Square', 1],
      ['Guides', 0],
    ],
  );

  // A closed group hides its whole subtree, at any depth: `ring` stays a row,
  // `speck` — two levels down — is gone.
  const halfOpen = layerRows(snap, new Set(['ink', 'b']));
  assert.deepEqual(
    halfOpen.map((line) => line.name),
    ['Ink', 'Circle', 'Ring', 'Dot', 'Square', 'Guides'],
  );

  // A group with nothing in it never opens: `canOpen` is not `isGroup`.
  const leaf = layerRows(snap, new Set(['ink', 'b', 'ring', 'speck']));
  assert.equal(leaf.find((line) => line.id === 'speck')?.isGroup, false);
  assert.equal(leaf.find((line) => line.id === 'speck')?.canOpen, false);

  // Every nested row carries the layer it lives in, which is how a drop or a
  // "move to layer" gesture names its destination.
  assert.equal(open.find((line) => line.id === 'speck')?.layerId, 'ink');
  assert.equal(open.find((line) => line.id === 'ink')?.layerId, 'ink');

  // Membership in the open set is per id, not per kind: the same `expanded` set
  // drives layers and groups, and a stale id for a deleted node is inert.
  assert.deepEqual(
    layerRows(snap, new Set(['ink', 'deleted-node'])).map((line) => line.name),
    ['Ink', 'Circle', 'Square', 'Guides'],
  );

  // A snapshot whose links are broken is still a list of rows. Three shapes,
  // each with the reading a designer can act on:
  //
  // 1. a parent the layer does not list — the row is a root here;
  // 2. a deep chain under a closed group — the descendant stays hidden, because
  //    closing the group is the panel *working*, not a gap to paper over;
  // 3. a link that closes a loop — unreachable from any root, so it is shown at
  //    the top level, once.
  const chained: SnapshotWire = {
    ...snap,
    layers: [
      {
        ...snap.layers![1],
        children: ['a', 'b', 'c'],
        child_names: ['Square', 'Circle', 'Deep'],
        child_is_group: [false, true, false],
        child_can_open: [false, true, false],
        child_parent: ['c', null, 'b'], // b → c → a
      },
      snap.layers![0],
    ].reverse() as SnapshotWire['layers'],
  };
  assert.deepEqual(
    layerRows(chained, new Set(['ink', 'b'])).map((line) => line.name),
    ['Ink', 'Circle', 'Deep', 'Guides'],
    'inside the open group, the child row shows; what *it* holds stays closed',
  );
  assert.deepEqual(
    layerRows(chained, new Set(['ink', 'b', 'c'])).map((line) => [line.name, line.depth]),
    [
      ['Ink', 0],
      ['Circle', 1],
      ['Deep', 2],
      ['Square', 3],
      ['Guides', 0],
    ],
    'and it nests to three levels when every ancestor is open',
  );

  const looped: SnapshotWire = {
    ...snap,
    layers: [
      {
        ...snap.layers![1],
        children: ['a', 'b', 'c'],
        child_names: ['Square', 'Circle', 'Orphan'],
        child_is_group: [false, true, false],
        child_can_open: [false, true, false],
        child_parent: ['c', null, 'a'], // a → c → a
      },
      snap.layers![0],
    ].reverse() as SnapshotWire['layers'],
  };
  assert.deepEqual(
    layerRows(looped, new Set(['ink', 'b', 'a'])).map((line) => line.name),
    ['Ink', 'Circle', 'Square', 'Orphan', 'Guides'],
    'a loop leaves every row visible, once each',
  );
});

test('RULE 1 (10.4): a drop names the container and the position it lands at', () => {
  const snap = fixture();
  const ink = snap.layers![1]; // children: a, b, dot, ring, speck

  // Onto a layer: the layer's own top level, at the front — the end of its list,
  // because the panel reads top-first. (`b` is the only row left at the top
  // level once `a` is taken out: `dot` and `ring` belong to `b`.)
  assert.deepEqual(dropNodeAt(ink, { id: 'ink', kind: 'layer' }, 'a'), {
    parent: null,
    index: 1,
  });

  // Onto a *closed* group row: a sibling **in front of** the group, not a member.
  assert.deepEqual(dropNodeAt(ink, { id: 'b', kind: 'node', canOpen: false }, 'a'), {
    parent: null,
    index: 0,
  });

  // Onto an *open* group row: inside it, in front of its members.
  assert.deepEqual(dropNodeAt(ink, { id: 'b', kind: 'node', canOpen: true }, 'a'), {
    parent: 'b',
    index: 2, // dot and ring travel with the group
  });

  // Onto a member: a sibling of that member, in front of it.
  assert.deepEqual(dropNodeAt(ink, { id: 'dot', kind: 'node', canOpen: false }, 'a'), {
    parent: 'b',
    index: 0,
  });

  // Onto its own row, or into its own subtree: refused here *and* by the engine
  // (typed, `GroupCycle`) — this check only spares the round trip.
  assert.equal(dropNodeAt(ink, { id: 'a', kind: 'node' }, 'a'), null);
  assert.equal(dropNodeAt(ink, { id: 'dot', kind: 'node', canOpen: true }, 'b'), null);
  assert.equal(isDescendantOf(ink, 'speck', 'b'), true, 'a grandchild is a descendant');
  assert.equal(isDescendantOf(ink, 'a', 'b'), false);

  // A target the layer does not list has no anchor: front of the container.
  assert.deepEqual(dropNodeAt(ink, { id: 'ghost', kind: 'node' }, 'a'), {
    parent: null,
    index: 1,
  });
  assert.equal(dropNodeAt(null, { id: 'ink', kind: 'layer' }, 'a'), null);
});

test('RULE 1 (10.4): a drop onto another layer names that layer, and the move is one undo', () => {
  const snap = fixture();
  const layers = snap.layers!; // back → front: guides, then ink

  // `a` lives in `ink`. Dropped on the **Guides** layer row, it is going to
  // Guides — the target row's layer, not the layer it came from.
  assert.deepEqual(dropNodeInto(layers, 'guides', { id: 'guides', kind: 'layer' }, 'a'), {
    layer: 'guides',
    parent: null,
    index: 1, // `c` is the only root there, so the front of the list is 1
  });

  // Onto a row *inside* the other layer: a sibling of that row, in that layer.
  assert.deepEqual(dropNodeInto(layers, 'guides', { id: 'c', kind: 'node' }, 'a'), {
    layer: 'guides',
    parent: null,
    index: 0,
  });

  // …and onto an open group row of that layer: inside the group.
  assert.deepEqual(
    dropNodeInto(layers, 'ink', { id: 'b', kind: 'node', canOpen: true }, 'a'),
    { layer: 'ink', parent: 'b', index: 2 },
    'the same layer answers exactly as `dropNodeAt` does',
  );

  // The refusals carry over: its own row, and a layer the snapshot lacks.
  assert.equal(dropNodeInto(layers, 'ink', { id: 'a', kind: 'node' }, 'a'), null);
  assert.equal(dropNodeInto(layers, 'ghost', { id: 'ink', kind: 'layer' }, 'a'), null);
  assert.equal(dropNodeInto(null, 'ink', { id: 'ink', kind: 'layer' }, 'a'), null);

  // The command, in the order the engine needs: the node **joins the layer
  // first**, then lands inside the container it was dropped on. One batch, so
  // the designer's single drag is a single undo.
  assert.deepEqual(moveNodeToLayer('a', 'guides', null, 1), {
    type: 'Batch',
    commands: [
      { type: 'AssignNodeToLayer', node_id: 'a', layer: 'guides' },
      { type: 'SetNodeParent', id: 'a', parent: null, index: 1 },
    ],
  });
  assert.deepEqual(moveNodeToLayer('a', 'guides', 'g2', 0), {
    type: 'Batch',
    commands: [
      { type: 'AssignNodeToLayer', node_id: 'a', layer: 'guides' },
      { type: 'SetNodeParent', id: 'a', parent: 'g2', index: 0 },
    ],
  });
});

// ── RULE 2: the artboard dropdown ──────────────────────────────────────────

test('RULE 2: the dropdown labels each board and falls back to the first', () => {
  const snap = fixture();
  const boards = artboardRows(snap);
  assert.deepEqual(
    boards.map((board) => [board.name, board.label, board.active]),
    [
      ['Square', '800×600', false],
      ['Icon 16', '16×16', true],
    ],
  );

  // A document whose boards never chose: the first one is the answer, so the UI
  // has nothing to invent (the engine's `active_id` does the same).
  const noActive = fixture();
  noActive.artboards = (noActive.artboards ?? []).map((board) => ({
    ...board,
    active: false,
  }));
  noActive.active_artboard = null;
  const fallback = artboardRows(noActive);
  assert.equal(fallback.find((board) => board.active), undefined);
  assert.equal(artboardRows(null).length, 0);
});

test('RULE 2: "jump to artboard" frames the board with a margin for its stroke', () => {
  const snap = fixture();
  const [square, icon] = artboardRows(snap);
  assert.deepEqual(artboardFrame(square), { x: -24, y: -24, width: 848, height: 648 });
  assert.deepEqual(artboardFrame(icon), { x: 856, y: -24, width: 64, height: 64 });
  assert.equal(artboardFrame(null), null);
});

// ── RULE 3: the Appearance Panel ───────────────────────────────────────────

test('RULE 4: the Appearance Panel is on screen for exactly one unlocked object', () => {
  const snap = fixture();
  assert.equal(showsAppearancePanel(snap, []), false, 'nothing selected');
  assert.equal(showsAppearancePanel(snap, ['a', 'b']), false, 'two objects');
  assert.equal(showsAppearancePanel(snap, ['a']), true);
  assert.equal(showsAppearancePanel(snap, ['missing']), false, 'a stale id');
  // A locked object cannot be edited, so a panel of live controls over it would
  // be a lie; a hidden one still shows — that is usually *why* it is selected.
  snap.scene.nodes.a.locked = true;
  assert.equal(showsAppearancePanel(snap, ['a']), false);
  snap.scene.nodes.a.locked = false;
  snap.scene.nodes.a.visible = false;
  assert.equal(showsAppearancePanel(snap, ['a']), true);
});

test('RULE 3: the brief’s own example is a stack the panel can build', () => {
  let stack: SnapshotAppearanceWire[] = [row('fill', '#2266ee')];
  stack = addStroke(stack, '#000000', 6);
  stack = addStroke(stack, '#ffffff', 2);
  assert.deepEqual(
    stack.map((line) => [line.kind, line.width]),
    [
      ['fill', null],
      ['stroke', 6],
      ['stroke', 2],
    ],
    'thick black under thin white, in draw order',
  );
  // Draw order is the list's order, and the captions say what each row is.
  assert.deepEqual(stack.map(appearanceCaption), ['fill', 'stroke 6', 'stroke 2']);

  // The last row cannot be removed: a node paints something.
  assert.equal(removeLayerAt([stack[0]], 0).length, 1);
  assert.deepEqual(
    removeLayerAt(stack, 1).map((line) => line.width),
    [null, 2],
  );

  // Reordering is the compositing order, and it clamps at the ends.
  assert.deepEqual(
    moveLayer(stack, 2, 1).map((line) => line.width),
    [null, 2, 6],
  );
  assert.deepEqual(
    moveLayer(stack, 0, 99).map((line) => line.width),
    [6, 2, null],
  );
  assert.equal(moveLayer(stack, 7, 0), stack, 'a stale index is a no-op');
});

test('RULE 3: blend modes and paint kinds are labelled, not raw enum spellings', () => {
  const tags = ['normal', 'multiply', 'screen', 'overlay'] as const;
  assert.deepEqual(tags.map(blendLabel), [
    'Normal',
    'Multiply',
    'Screen',
    'Overlay',
  ]);
  const blended = updateLayerAt([row('fill', '#ffffff')], 0, { blend: 'multiply' });
  assert.equal(appearanceCaption(blended[0]), 'fill · Multiply');
  assert.equal(swatchColor({ type: 'solid', color: '#123456' }), '#123456');
  assert.equal(
    swatchColor({
      type: 'linear',
      start: [0, 0],
      end: [1, 0],
      stops: [
        { offset: 0, color: '#111111' },
        { offset: 1, color: '#eeeeeee' },
      ],
    }),
    '#111111',
    'a gradient swatch is its first stop',
  );
});

test('RULE 3: the stack the panel sends back is a document stack of literals', () => {
  const snap = fixture();
  const stack = [
    ...appearanceStack(selectedNode(snap, ['a'])),
    row('stroke', '#000000', { width: 6, blend: 'multiply' }),
  ];
  const wire = stackToWire(stack);
  assert.deepEqual(wire[0], {
    kind: 'Fill',
    paint: { Solid: { Literal: { r: 0x22, g: 0x66, b: 0xee, a: 255 } } },
    opacity: { Literal: 1 },
    blend: 'Normal',
    visible: true,
  });
  assert.deepEqual(wire[1].kind, { Stroke: { width: { Literal: 6 } } });
  assert.equal(wire[1].blend, 'Multiply');
  // A gradient row keeps its stops, as literals, in order.
  const gradient = togglePaint(row('fill', '#ffd200'), 'linear');
  const gradientWire = stackToWire([gradient])[0];
  assert.deepEqual(gradientWire.paint, {
    Linear: {
      start: { Literal: { x: 0, y: 0 } },
      end: { Literal: { x: 100, y: 0 } },
      stops: [
        { offset: 0, color: { r: 0xff, g: 0xd2, b: 0x00, a: 255 } },
        { offset: 1, color: { r: 0xff, g: 0xff, b: 0xff, a: 255 } },
      ],
    },
  });
  // …and a radial row becomes a radial again, not a linear with a centre.
  const radial = togglePaint(row('fill', '#ffd200'), 'radial');
  assert.equal((stackToWire([radial])[0].paint as { Radial?: unknown }).Radial !== undefined, true);
  // Switching back to solid keeps the colour the ramp started from.
  assert.deepEqual(togglePaint(radial, 'solid').paint, { type: 'solid', color: '#ffd200' });
  assert.equal(togglePaint(radial, 'radial'), radial, 'a no-op is a no-op');
});

test('RULE 3: a dragged gradient stop clamps between its neighbours and never crosses', () => {
  const stops = [
    { offset: 0, color: '#ffd200' },
    { offset: 0.5, color: '#f05a14' },
    { offset: 1, color: '#780a3c' },
  ];
  assert.equal(moveStop(stops, 1, 0.85)[1].offset, 0.85);
  // Clamped by the neighbour below and the one above, and by the ends.
  assert.equal(moveStop(stops, 1, -3)[1].offset, 0);
  assert.equal(moveStop(stops, 1, 9)[1].offset, 1);
  // The ends may move *inwards* — a gradient may start at 40% —
  // but never past their neighbour.
  assert.equal(moveStop(stops, 0, 0.4)[0].offset, 0.4);
  assert.equal(moveStop(stops, 0, 0.9)[0].offset, 0.5, 'clamped to the next stop');
  assert.equal(moveStop(stops, 2, 0.2)[2].offset, 0.5, 'and the same at the other end');
  // Moving one stop leaves the others exactly where they were.
  assert.deepEqual(
    moveStop(stops, 1, 0.25).map((stop) => stop.offset),
    [0, 0.25, 1],
  );
  assert.deepEqual(
    recolorStop(stops, 1, '#00ff00').map((stop) => stop.color),
    ['#ffd200', '#00ff00', '#780a3c'],
  );
  // Out-of-range indices are ignored rather than throwing mid-drag.
  assert.deepEqual(moveStop(stops, 9, 0.5), stops);
  assert.deepEqual(recolorStop(stops, -1, '#000000'), stops);
});

test('RULE 3: the gradient bar paints the ramp the engine will sample', () => {
  const linear: SnapshotPaintWire = {
    type: 'linear',
    start: [0, 0],
    end: [100, 0],
    stops: [
      { offset: 0, color: '#ffd200' },
      { offset: 0.5, color: '#f05a14' },
      { offset: 1, color: '#780a3c' },
    ],
  };
  assert.equal(
    gradientCss(linear),
    'linear-gradient(90deg, #ffd200 0%, #f05a14 50%, #780a3c 100%)',
  );
  assert.equal(gradientCss(solid('#2266ee')), null, 'a solid has no bar');

  // The colour codec round-trips the values the engine sent, alpha included.
  assert.deepEqual(hexToColor('#2266ee'), { r: 0x22, g: 0x66, b: 0xee, a: 255 });
  assert.deepEqual(hexToColor('#2266ee80'), { r: 0x22, g: 0x66, b: 0xee, a: 0x80 });
  // Malformed input is opaque black, never invented digits.
  assert.deepEqual(hexToColor('not a colour'), { r: 0, g: 0, b: 0, a: 255 });
  assert.deepEqual(hexToColor('#2266e'), { r: 0, g: 0, b: 0, a: 255 });
});

test('RULE 3: adding a fill or a stroke appends on top and never mutates the input', () => {
  const stack = [row('fill', '#2266ee')];
  const filled = addFill(stack, '#ffffff');
  const stroked = addStroke(stack, '#000000', 4);
  assert.equal(stack.length, 1, 'the panel owns no state it did not ask for');
  assert.deepEqual(
    filled.map((line) => line.kind),
    ['fill', 'fill'],
  );
  assert.equal(filled[1].paint.type === 'solid' ? filled[1].paint.color : '', '#ffffff');
  assert.deepEqual(stroked[1].width, 4);
  assert.equal(stroked[1].blend, 'normal');
});


// ── RULE 2 (10.3): navigation ──────────────────────────────────────────────

test('the zoom label rounds the way a designer reads it', () => {
  assert.equal(formatZoom(1), '100%');
  assert.equal(formatZoom(2.4), '240%');
  assert.equal(formatZoom(0.125), '12.5%', 'a real view keeps its decimal');
  assert.equal(formatZoom(0.25), '25%', 'a whole number drops the decimal');
  assert.equal(formatZoom(64), '6400%');
  // Below 10% the integer would erase the difference between two real views of
  // a large poster, so one decimal survives there and only there.
  assert.equal(formatZoom(0.083), '8.3%');
  assert.equal(formatZoom(0.00741), '0.7%');
  // A camera that is not there yet says so instead of inventing 100%.
  assert.equal(formatZoom(null), '—');
  assert.equal(formatZoom(0), '—');
  assert.equal(formatZoom(Number.NaN), '—');
});

test('a wheel zooms in when it scrolls up, out when it scrolls down', () => {
  assert.ok(wheelZoomFactor(-100) > 1, 'scroll up zooms in');
  assert.ok(wheelZoomFactor(100) < 1, 'scroll down zooms out');
  assert.equal(wheelZoomFactor(0), 1, 'a still wheel is not a gesture');
  assert.equal(wheelZoomFactor(Number.NaN), 1);

  // Exponential, so a trackpad's stream and a mouse's notch agree: two hundred
  // 2-pixel events are one 400-pixel event, and 400 pixels is exactly a
  // doubling.
  assert.ok(Math.abs(wheelZoomFactor(2) ** 200 - wheelZoomFactor(400)) < 1e-9);
  assert.ok(Math.abs(wheelZoomFactor(400) - 0.5) < 1e-12);
  assert.ok(Math.abs(wheelZoomFactor(-400) - 2) < 1e-12);

  // Sign symmetry: scrolling back exactly what you scrolled forward restores the
  // view, which is what makes the wheel feel reversible rather than lossy.
  assert.ok(Math.abs(wheelZoomFactor(37) * wheelZoomFactor(-37) - 1) < 1e-12);

  // Units: a line-mode event is 16 pixels, a page-mode event 400.
  assert.ok(Math.abs(wheelZoomFactor(25, 1) - wheelZoomFactor(400)) < 1e-12);
  assert.ok(Math.abs(wheelZoomFactor(1, 2) - wheelZoomFactor(400)) < 1e-12);

  // One violent event cannot teleport the view: the per-event factor is capped,
  // and the cap is symmetric.
  assert.equal(wheelZoomFactor(100000), 1 / 4);
  assert.equal(wheelZoomFactor(-100000), 4);
});

test('a pan starts on the middle button or on space', () => {
  assert.equal(panGestureAllowed(1, false), true, 'middle drag pans');
  assert.equal(panGestureAllowed(0, true), true, 'space + drag pans');
  assert.equal(panGestureAllowed(0, false), false, 'a plain drag belongs to the tool');
  assert.equal(panGestureAllowed(2, true), false, 'the right button is a context menu');
});

test('the document grid follows the camera', () => {
  const opening = gridStyle({ x: 0, y: 0, w: 800, h: 600 }, 1);
  assert.equal(opening.backgroundSize, '50px 50px');
  // At the opening view the pattern's origin sits at the document's top edge —
  // the same place the fixed grid has always been.
  assert.equal(opening.backgroundPosition, '0px 600px');

  const zoomed = gridStyle({ x: 0, y: 0, w: 800, h: 600 }, 2);
  assert.equal(zoomed.backgroundSize, '100px 100px', 'cells are document units');
  assert.equal(zoomed.backgroundPosition, '0px 1200px');

  // Panning slides the grid by the pan itself: the artwork and the ruler move
  // together or the grid is a wrong ruler.
  const panned = gridStyle({ x: 100, y: 0, w: 800, h: 600 }, 2);
  assert.equal(panned.backgroundPosition, '-200px 1200px');
  assert.equal(panned.backgroundSize, zoomed.backgroundSize);

  // A canvas that has not been measured yet still yields a usable style.
  assert.equal(gridStyle(null, null).backgroundSize, '50px 50px');
});
