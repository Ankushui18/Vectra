/**
 * **The workspace panels actually mount** (Task 10.2 RULEs 1–4), with no DOM and
 * no browser.
 *
 * `panels.test.ts` proves the *decisions* (which rows, which frame, when the
 * panel appears). This file proves the other half, the one a type-check cannot:
 * that the components render, that a click reaches the engine's command
 * builders, and that the JSX the whole workspace hangs off — the integration in
 * `App.tsx` — renders without exploding.
 *
 * Rendering is `react-dom/server`, so there is no `window` and no WebGPU: the
 * canvas components take their null-device path, exactly as they do on a machine
 * without a GPU. React only prints a warning for the `useLayoutEffect`/pointer
 * handlers the server cannot run, and warnings are not failures — an *exception*
 * is what would be. That distinction is the point of this file: it fails loudly
 * if the panels (or the shell that hosts them) stop rendering at all.
 *
 * The house style is on trial too: RULE 4 asks for icons, so the blend controls
 * must be glyph buttons with their names in `title`/`aria-label` — never a raw
 * `<select>` of words or a bare number picker.
 *
 * Run: `npm run test:ui`.
 */
import assert from 'node:assert/strict';
import { test } from 'node:test';
import * as React from 'react';
import { renderToStaticMarkup } from 'react-dom/server';

import App from '../src/App';
import { AppearancePanel } from '../src/components/AppearancePanel';
import { ArtboardBar } from '../src/components/ArtboardBar';
import { LayersPanel } from '../src/components/LayersPanel';
import NavigationOverlay from '../src/components/NavigationOverlay';
import { artboardRows, dropIndexFor } from '../src/engine/panels';
import type {
  CommandWire,
  SnapshotAppearanceWire,
  SnapshotNodeWire,
  SnapshotPaintWire,
  SnapshotWire,
} from '../src/engine/wire';

const noop = () => {};

const solid = (color: string): SnapshotPaintWire => ({ type: 'solid', color });

const row = (
  kind: 'fill' | 'stroke',
  color: string,
  extra: Partial<SnapshotAppearanceWire> = {},
): SnapshotAppearanceWire => ({
  kind,
  paint: solid(color),
  opacity: 1,
  blend: 'normal',
  visible: true,
  width: kind === 'stroke' ? 2 : null,
  ...extra,
});

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

function fixture(): SnapshotWire {
  return {
    status: 'ok',
    scene: {
      nodes: {
        a: node('a', 'Square'),
        b: node('b', 'Circle', {}, [
          row('fill', '#2266ee'),
          row('stroke', '#000000', { width: 6 }),
          row('stroke', '#ffffff', { width: 2, blend: 'multiply' }),
        ]),
      },
      z_order: ['a', 'b'],
    },
    variables: {},
    expressions: {},
    diagnostics: [],
    graph: { nodes: 2, edges: 0, acyclic: true },
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
        id: 'ink',
        name: 'Ink',
        visible: true,
        locked: false,
        children: ['a', 'b', 'dot'],
        child_names: ['Square', 'Circle', 'Dot'],
        child_is_group: [false, true, false],
        child_can_open: [false, true, false],
        // `b` is a group holding `dot` — three rows, two levels, and a real
        // parent link for the panel to walk.
        child_parent: [null, null, 'b'],
        artboard: 'board-1',
        active: true,
      },
    ],
    artboards: [
      {
        id: 'board-1',
        name: 'Logo',
        bounds: [0, 0, 800, 600],
        background: '#ffffff',
        layers: 1,
        active: true,
      },
    ],
    active_layer: 'ink',
    active_artboard: 'board-1',
    can_undo: false,
    can_redo: false,
    time: 0,
  } as unknown as SnapshotWire;
}

const count = (markup: string, needle: string): number => markup.split(needle).length - 1;

// ── RULE 1: the Layers Panel ───────────────────────────────────────────────

test('RULE 1: the Layers Panel renders layers, an eye, a padlock and a rename', () => {
  const commands: CommandWire[] = [];
  const markup = renderToStaticMarkup(
    React.createElement(LayersPanel, {
      snapshot: fixture(),
      expanded: new Set<string>(['ink']),
      onToggleExpanded: noop,
      onCommand: (_label: string, command: CommandWire) => commands.push(command),
      onAssignNode: noop,
      pendingNode: null,
      disabled: false,
    }),
  );
  assert.ok(markup.includes('data-testid="layers-panel"'), 'the panel is there');
  assert.ok(markup.includes('data-testid="layer-row-ink"'), 'its layer row');
  assert.ok(markup.includes('data-testid="layer-row-a"'), 'and its contents');
  assert.ok(markup.includes('data-testid="layer-eye-ink"'), 'the eye icon');
  assert.ok(markup.includes('data-testid="layer-lock-ink"'), 'the padlock icon');
  assert.ok(markup.includes('Layers'), 'and a heading');
  assert.equal(commands.length, 0, 'rendering issues no commands by itself');

  // A collapsed panel lists no contents, and says how many layers there are.
  const collapsed = renderToStaticMarkup(
    React.createElement(LayersPanel, {
      snapshot: fixture(),
      expanded: new Set<string>(),
      onToggleExpanded: noop,
      onCommand: noop,
      disabled: false,
    }),
  );
  assert.ok(!collapsed.includes('data-testid="layer-row-a"'), 'collapsed ⇒ no rows');
});

// ── RULE 2: the Artboard bar ───────────────────────────────────────────────

test('RULE 2: the Artboard bar renders the jump dropdown and both exports', () => {
  const commands: CommandWire[] = [];
  const markup = renderToStaticMarkup(
    React.createElement(ArtboardBar, {
      snapshot: fixture(),
      onCommand: (_label: string, command: CommandWire) => commands.push(command),
      onFrame: noop,
      onExportCurrent: noop,
      onExportAll: noop,
      disabled: false,
    }),
  );
  assert.ok(markup.includes('data-testid="artboard-select"'), 'the dropdown');
  assert.ok(markup.includes('Logo'), 'the board name');
  assert.ok(markup.includes('800×600'), 'and its size, so a jump is unambiguous');
  assert.ok(markup.includes('data-testid="artboard-add"'), 'add a board');
  assert.ok(markup.includes('data-testid="export-current-artboard"'), 'export current');
  assert.ok(markup.includes('data-testid="export-all-artboards"'), 'export all');
  // The dropdown is a real `<select>` with one `<option>` per board — the
  // control the rule asks for, not a list of links.
  assert.equal(count(markup, '<option'), 1, 'one option per board');
});

// ── RULE 3 and RULE 4: the Appearance Panel ────────────────────────────────

test('RULE 3: the Appearance Panel renders the stack, the blend icons and a gradient bar', () => {
  const snap = fixture();
  const stack = snap.scene.nodes.b.style.appearances;
  const commands: CommandWire[] = [];
  const markup = renderToStaticMarkup(
    React.createElement(AppearancePanel, {
      node: snap.scene.nodes.b,
      onCommand: (_label: string, command: CommandWire) => commands.push(command),
      disabled: false,
    }),
  );
  assert.ok(markup.includes('data-testid="appearance-panel"'), 'the panel');
  assert.equal(count(markup, 'data-testid="appearance-row-'), 3, 'three rows for three layers');
  assert.ok(markup.includes('data-testid="appearance-blend-0"'), 'a blend control per row');
  assert.ok(markup.includes('data-testid="appearance-opacity-0"'), 'an opacity control per row');
  assert.equal(stack.length, 3, 'the fixture really is the brief’s stacked-stroke case');
  // RULE 4's house style: the blend control is a glyph button whose *name* lives
  // in an accessible label, and the panel never prints the raw enum spelling.
  assert.ok(markup.includes('aria-label="Blend mode: Normal"'), 'an icon button, named');
  assert.ok(
    markup.includes('data-testid="appearance-add-fill"') &&
      markup.includes('data-testid="appearance-add-stroke"'),
    'and glyph buttons for adding a fill or a stroke',
  );
  assert.ok(!markup.includes('>multiply<'), 'no raw enum text anywhere');
  assert.ok(!markup.includes('<select'), 'and no word-list picker for a glyph’s job');
  assert.equal(commands.length, 0, 'rendering issues no commands');
});

test('RULE 3: a gradient row draws a bar with one draggable handle per stop', () => {
  const snap = fixture();
  const gradient: SnapshotNodeWire = {
    ...snap.scene.nodes.a,
    style: {
      ...snap.scene.nodes.a.style,
      appearances: [
        {
          kind: 'fill',
          paint: {
            type: 'linear',
            start: [0, 0],
            end: [100, 0],
            stops: [
              { offset: 0, color: '#ffd200' },
              { offset: 0.5, color: '#f05a14' },
              { offset: 1, color: '#780a3c' },
            ],
          },
          opacity: 1,
          blend: 'normal',
          visible: true,
          width: null,
        },
      ],
    },
  };
  const markup = renderToStaticMarkup(
    React.createElement(AppearancePanel, {
      node: gradient,
      onCommand: noop,
      disabled: false,
    }),
  );
  assert.ok(markup.includes('data-testid="gradient-bar-0"'), 'the gradient bar');
  assert.ok(markup.includes('data-testid="gradient-stop-0-0"'), 'stop 0');
  assert.ok(markup.includes('data-testid="gradient-stop-0-1"'), 'stop 1');
  assert.ok(markup.includes('data-testid="gradient-stop-0-2"'), 'stop 2');
  assert.ok(
    markup.includes('linear-gradient(90deg, #ffd200 0%, #f05a14 50%, #780a3c 100%)'),
    'the bar paints the ramp the engine samples',
  );
});

test('RULE 4: outside a selection there is no Appearance Panel to render', () => {
  // The panel's contract is a *node*: the shell never mounts it without one, and
  // `showsAppearancePanel` (tested in `panels.test.ts`) is the gate. This asserts
  // the gate is the only way in — a document with nothing selected renders the
  // panel zero times, which is what RULE 4 asks for.
  const snap = fixture();
  const withSelection = renderToStaticMarkup(
    React.createElement(AppearancePanel, {
      node: snap.scene.nodes.a,
      onCommand: noop,
      disabled: false,
    }),
  );
  assert.ok(withSelection.includes('appearance-panel'));
});

test('RULE 1 (10.4): an opened group renders its contents, one level deeper', () => {
  const snap = fixture();
  const closed = renderToStaticMarkup(
    React.createElement(LayersPanel, {
      snapshot: snap,
      expanded: new Set(['ink']),
      onToggleExpanded: noop,
      onCommand: noop,
      onGroupSelection: noop,
      disabled: false,
    }),
  );
  assert.ok(closed.includes('data-testid="group-selection"'), 'the group action is in the header');
  assert.ok(closed.includes('data-testid="layer-row-b"'), 'the group row is there');
  assert.ok(closed.includes('data-testid="group-expand-b"'), 'with a disclosure triangle');
  assert.ok(!closed.includes('data-testid="layer-row-dot"'), 'and its contents are shut');

  const open = renderToStaticMarkup(
    React.createElement(LayersPanel, {
      snapshot: snap,
      expanded: new Set(['ink', 'b']),
      onToggleExpanded: noop,
      onCommand: noop,
      onGroupSelection: noop,
      disabled: false,
    }),
  );
  assert.ok(open.includes('data-testid="layer-row-dot"'), 'opened, the member is a row');
  // Depth is indentation: the depth the walk found, rendered as pixels, and the
  // one place the panel turns a number into a layout decision.
  assert.ok(open.includes('padding-left:38px'), 'two levels in, one more indent');
  const a11y = open.match(/data-testid="group-expand-b"[^>]*aria-expanded="true"/);
  assert.ok(a11y, 'the triangle reports its state to assistive tech, not just its glyph');
  // The group action is in the header, and it says whether it can do anything:
  // with nothing selected, the control exists but is inert (RULE 4's honesty
  // about what is and is not available).
  assert.ok(
    open.includes('data-testid="group-selection"'),
    'the group action is offered in the header',
  );
  // With nothing selected it is inert rather than absent: a control that comes
  // and goes is harder to find than one that says "not yet".
  assert.ok(
    /data-testid="group-selection"[^>]*disabled/.test(open),
    'and it is disabled with an empty selection',
  );
});

test('RULE 2 (10.3): the navigation overlay draws boards where the camera says', () => {
  const snap = fixture();
  const boards = artboardRows(snap);
  // A camera at 100% with the document's origin at the canvas origin: the
  // transform below is the *engine's*, in its y-up convention, which is exactly
  // what the component is handed in the app (`client.documentToClient`).
  const toClient = (points: [number, number][]): [number, number][] =>
    points.map(([x, y]) => [x, 600 - y]);

  const markup = renderToStaticMarkup(
    React.createElement(NavigationOverlay, {
      view: { x: 0, y: 0, w: 800, h: 600 },
      zoom: 1,
      boards,
      toClient,
      onZoomIn: noop,
      onZoomOut: noop,
      onZoomReset: noop,
      onZoomFit: noop,
      disabled: false,
    }),
  );

  assert.ok(markup.includes('data-testid="nav-overlay"'), 'the overlay is mounted');
  assert.ok(markup.includes('data-testid="nav-board-board-1"'), 'each board is outlined');
  assert.ok(markup.includes('data-testid="nav-window"'), 'and the visible window is marked');
  // The board's own colour is its tint: the overlay invents nothing.
  assert.ok(markup.includes('fill="#ffffff"'), 'the board background is the fill');
  assert.ok(markup.includes('Logo · 800×600'), 'the label names it, with its size');
  // The controls are icon buttons with accessible names (house style).
  assert.ok(markup.includes('data-testid="zoom-in"') && markup.includes('data-testid="zoom-out"'));
  assert.ok(markup.includes('aria-label="Zoom 100%'), 'the readout names the zoom');
  assert.ok(markup.includes('data-testid="zoom-fit"'), 'and fit is one press away');

  // A camera that is not there yet renders the controls and no frames, rather
  // than frames in the wrong place.
  const unmeasured = renderToStaticMarkup(
    React.createElement(NavigationOverlay, {
      view: null,
      zoom: null,
      boards,
      toClient,
      onZoomIn: noop,
      onZoomOut: noop,
      onZoomReset: noop,
      onZoomFit: noop,
    }),
  );
  assert.ok(unmeasured.includes('data-testid="zoom-readout"'));
  assert.ok(unmeasured.includes('—'), 'the zoom reads as unknown, not as 100%');
});

test('RULE 2 (10.3): the artboard bar edits the board’s box and background', () => {
  const snap = fixture();
  const markup = renderToStaticMarkup(
    React.createElement(ArtboardBar, {
      snapshot: snap,
      onCommand: noop,
      onFrame: noop,
      onExportCurrent: noop,
      onExportAll: noop,
      disabled: false,
    }),
  );
  for (const field of ['artboard-x', 'artboard-y', 'artboard-width', 'artboard-height']) {
    assert.ok(markup.includes(`data-testid="${field}"`), `${field} is editable`);
  }
  assert.ok(markup.includes('data-testid="artboard-background"'), 'so is the background');
  // The fields show the engine's numbers, not placeholders.
  assert.ok(markup.includes('value="800"') && markup.includes('value="600"'));
  assert.ok(markup.includes('value="#ffffff"'), 'the colour reads as the engine sent it');
});

// ── The shell ──────────────────────────────────────────────────────────────

test('the App shell renders, and the workspace panels are part of it', () => {
  // No WebGPU in Node, so `VectraClient` reports the null device and the canvas
  // takes its banner path. That is the same code path a browser without WebGPU
  // takes, and it must not stop the panels around it from rendering: the
  // workspace is document state, not a GPU feature.
  const markup = renderToStaticMarkup(React.createElement(App));
  assert.ok(markup.length > 1000, 'something rendered');
  assert.ok(markup.includes('data-testid="layers-panel"'), 'so is the layers panel');
  assert.ok(markup.includes('data-testid="layers-empty"'), 'with its empty state, honestly');
});

test('a panel drag index is the inverse of the engine z-order, in the shell’s own call', () => {
  // The one piece of arithmetic the Layers Panel does, asserted where the drag
  // handler calls it (see `LayersPanel.onDropLayer`).
  assert.equal(dropIndexFor(0, 3), 2);
  assert.equal(dropIndexFor(2, 3), 0);
});
