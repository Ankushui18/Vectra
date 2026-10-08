/**
 * **The artist-first shell** (Task 13.0, RULEs 1–5) — the model and the markup.
 *
 * Task 13.0 is a UI overhaul with a hard constraint attached: it must not touch
 * the engine bindings. The way this file holds that line is the way
 * `engine/theme.ts` does — the shell's *decisions* (which group a tool belongs
 * to, which state the bottom bar is in, what a row's type mark is, where the HUD
 * floats, whether the chrome is visible) are pure functions, and they are tested
 * here as such. The markup tests then prove the components render those
 * decisions and nothing else.
 *
 * What each RULE gets:
 *
 * * **RULE 1** — the dock's five groups in the philosophy's order, the one-step
 *   contextual bar, and the drawer as a *door* rather than a dashboard.
 * * **RULE 2** — the type indicator read off the wire, never off the name.
 * * **RULE 3** — the HUD's honesty about what the engine cannot do, the Tab
 *   switch, and the edge reveal from all four edges.
 * * **RULE 5** — one palette (asserted against `App.css` itself, so the two
 *   copies of the theme cannot drift), and icons rather than bare words.
 *
 * Rendering is `react-dom/server`: no DOM, no WebGPU, and the whole shell
 * renders — including `App.tsx` — exactly as it does on a machine with no GPU.
 *
 * Run: `npm run test:ui`.
 */
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';
import { renderToStaticMarkup } from 'react-dom/server';
import { Layers, Sigma } from 'lucide-react';

import App from '../src/App';
import CanvasHUD from '../src/components/CanvasHUD';
import ContextualBottomBar from '../src/components/ContextualBottomBar';
import LeftDock from '../src/components/LeftDock';
import { LayersPanel } from '../src/components/LayersPanel';
import StudioDrawer from '../src/components/StudioDrawer';
import {
  LAYER_TYPES,
  THEME,
  TOOL_GROUPS,
  bottomBarCaption,
  bottomBarMode,
  canvasHud,
  chromeVisible,
  dockEntries,
  edgeReveal,
  groupForTool,
  layerMark,
  layerTypeOf,
  nodeState,
  primitiveBox,
  selectionAnchor,
} from '../src/engine/theme';
import type { SnapshotLayerWire, SnapshotNodeWire, SnapshotWire } from '../src/engine/wire';

const noop = () => {};

/** The whole tag carrying `data-testid` — attributes only, up to its `>`. */
function tagFor(markup: string, testid: string): string {
  const at = markup.indexOf(`data-testid="${testid}"`);
  assert.ok(at >= 0, `no element with data-testid="${testid}"`);
  const open = markup.lastIndexOf('<', at);
  const close = markup.indexOf('>', at);
  return markup.slice(open, close + 1);
}

/** The whole `<button>`, attributes **and** the glyph or icon it draws. */
function buttonFor(markup: string, testid: string): string {
  const at = markup.indexOf(`data-testid="${testid}"`);
  assert.ok(at >= 0, `no element with data-testid="${testid}"`);
  const open = markup.lastIndexOf('<', at);
  const close = markup.indexOf('</button>', at);
  return markup.slice(open, close + '</button>'.length);
}

// ── RULE 1: the left dock ──────────────────────────────────────────────────

test('RULE 1: the dock is five groups, and precision is the third of them', () => {
  assert.deepEqual(
    TOOL_GROUPS.map((group) => group.id),
    ['paint', 'draw', 'vector', 'shape', 'text'],
    'Paint first, Draw second, Vector third: "paint first, vector when precision matters"',
  );
  assert.deepEqual(
    TOOL_GROUPS.map((group) => group.label),
    ['Paint', 'Draw', 'Vector', 'Shape', 'Text'],
  );
});

test('RULE 1: holding Vector reveals Pen, Node, Boolean, Offset and Mirror', () => {
  const vector = TOOL_GROUPS.find((group) => group.id === 'vector');
  assert.ok(vector, 'the Vector group exists');
  const labels = vector.entries.map((entry) => entry.label);
  for (const named of ['Pen', 'Node', 'Boolean · Union', 'Offset', 'Mirror']) {
    assert.ok(labels.includes(named), `Vector holds ${named}: ${labels.join(', ')}`);
  }
  // Every entry *does* something: a tool the engine knows, or a real command.
  for (const entry of dockEntries()) {
    if (entry.kind === 'tool') assert.ok(entry.tool.length > 0, entry.label);
    else assert.ok(entry.action.length > 0, entry.label);
    assert.ok(entry.hint.length > 0, `${entry.label} explains itself`);
  }
});

test('RULE 1: a held tool knows which group it belongs to, by lookup not by name', () => {
  assert.equal(groupForTool('brush'), 'paint');
  assert.equal(groupForTool('smartFill'), 'paint', 'Fill shares the Paint group');
  assert.equal(groupForTool('pen'), 'vector');
  assert.equal(groupForTool('direct'), 'vector');
  assert.equal(groupForTool('select'), 'draw', 'the pointer is Draw’s entry');
  assert.equal(groupForTool('text'), 'text', 'whose group label is not its tool name');
});

test('RULE 1: the dock renders a labelled, icon-bearing button per group', () => {
  const markup = renderToStaticMarkup(
    <LeftDock tool="select" onTool={noop} onAction={noop} disabled={false} />,
  );
  assert.ok(markup.includes('data-testid="left-dock"'), 'the dock');
  assert.ok(markup.includes('aria-label="Tools"'), 'and it is named');
  for (const group of TOOL_GROUPS) {
    const opening = tagFor(markup, `dock-${group.id}`);
    assert.ok(opening.includes(`aria-label="${group.label} tools — hold to open`), opening);
    assert.ok(opening.includes('aria-haspopup="true"'), 'the hold is announced');
    assert.ok(
      buttonFor(markup, `dock-${group.id}`).includes('<svg'),
      `an icon, not a word: ${group.label}`,
    );
  }
  assert.ok(!markup.includes('dock-flyout-vector'), 'nothing is held, so nothing is revealed');
  assert.ok(markup.includes('active'), 'the held tool’s group is the one shown as active');
});

// ── RULE 1: the dynamic bottom bar ─────────────────────────────────────────

test('RULE 1: the bar’s state is decided in one place, and the node wins', () => {
  const idle = { tool: 'select' as const, selectionCount: 0, selectedIsGeometry: false, anchorOwned: false };
  assert.equal(bottomBarMode(idle), 'idle');
  assert.equal(bottomBarMode({ ...idle, tool: 'brush' }), 'brush');
  assert.equal(bottomBarMode({ ...idle, selectionCount: 1, selectedIsGeometry: true }), 'path');
  assert.equal(
    bottomBarMode({ ...idle, tool: 'brush', selectionCount: 1, selectedIsGeometry: true, anchorOwned: true }),
    'node',
    'the point in hand is more specific than the tool in hand',
  );
  assert.equal(bottomBarCaption('path', 1), '1 object selected');
  assert.equal(bottomBarCaption('path', 3), '3 objects selected');
  assert.equal(bottomBarCaption('node', 1), 'Point');
  assert.equal(bottomBarCaption('idle', 0), 'Nothing selected');
});

const bar = (over: Partial<Parameters<typeof ContextualBottomBar>[0]> = {}) =>
  renderToStaticMarkup(
    <ContextualBottomBar
      mode="idle"
      selectionCount={0}
      node={null}
      brushName="Brush"
      brushSize={12}
      brushOpacity={1}
      onFillColor={noop}
      onStrokeColor={noop}
      onStrokeWidth={noop}
      onNodeConvert={noop}
      nodeAlign="—"
      onNodeAlign={noop}
      canUndo={false}
      canRedo={false}
      onUndo={noop}
      onRedo={noop}
      zoom={1}
      onZoomIn={noop}
      onZoomOut={noop}
      onZoomFit={noop}
      {...over}
    />,
  );

test('RULE 1: the bar *changes content*, it does not come and go', () => {
  // The idle state is where Undo and Zoom live, so the bar is never empty.
  const idle = bar();
  assert.ok(tagFor(idle, 'bottom-bar').includes('data-mode="idle"'), 'the state is on the element');
  assert.ok(idle.includes('data-testid="bar-undo"') && idle.includes('data-testid="bar-redo"'));
  assert.ok(idle.includes('data-testid="bar-zoom"') && idle.includes('100%'), 'the zoom readout');
  assert.ok(
    buttonFor(idle, 'bar-undo').includes('disabled'),
    'and it refuses when the engine has nothing to undo',
  );

  const brush = bar({ mode: 'brush' });
  assert.ok(brush.includes('data-testid="bar-brush"'), 'brush controls');
  assert.ok(!brush.includes('data-testid="bar-node"'), 'and nothing else');
  assert.ok(!brush.includes('data-testid="bar-zoom"'), 'zoom is the idle state’s, not the brush’s');

  const path = bar({ mode: 'path', selectionCount: 1 });
  assert.ok(path.includes('data-testid="bar-fill-color"') && path.includes('data-testid="bar-stroke-color"'));
  assert.ok(path.includes('data-testid="bar-width"'), 'the stroke width is a live control');
  assert.ok(!path.includes('data-testid="bar-node"'));
});

test('RULE 1 + the audit’s honesty: the gaps are named, not hidden', () => {
  // The brush's own width and opacity are Phase 2 (roadmap 2.2): the engine fits
  // every stroke with a fixed profile, so the two sliders show its numbers and
  // say where a settable one is coming from.
  const brush = bar({ mode: 'brush' });
  const size = tagFor(brush, 'bar-size');
  assert.ok(size.includes('disabled'), `the brush size is not settable yet: ${size}`);
  assert.ok(size.includes('readonly') || size.includes('readOnly'), 'and it is not a lie either');
  assert.ok(brush.includes('Phase 2'), 'the tooltip names the roadmap item');
  assert.ok(!brush.includes('onBrushSize'), 'no handler pretends it works');

  // Cap and Join do not exist in the engine (audit §2.2, roadmap 1.4).
  const path = bar({ mode: 'path', selectionCount: 1 });
  for (const id of ['bar-cap', 'bar-join']) {
    const button = tagFor(path, id);
    assert.ok(button.includes('disabled'), `${id} is disabled: ${button}`);
    assert.ok(button.includes('roadmap 1.4'), `${id} says when it arrives`);
    assert.ok(
      buttonFor(path, id).includes('<svg'),
      `${id} is an icon plus a word, not a bare word`,
    );
  }
});

test('RULE 1: the node bar converts, reads the point’s state, and offers Align', () => {
  const markup = bar({ mode: 'node', selectionCount: 1, nodeAlign: 'Symmetric · Smooth' });
  assert.ok(markup.includes('data-testid="bar-node"'));
  assert.ok(buttonFor(markup, 'node-convert').includes('<svg'), 'Convert wears an icon');
  assert.ok(buttonFor(markup, 'node-smooth').includes('<svg'), 'so does Smooth');
  assert.ok(
    markup.includes('Symmetric · Smooth'),
    'the state is reported in the brief’s own word, so no third button repeats it',
  );
  assert.ok(buttonFor(markup, 'node-align-action').includes('<svg'), 'and Align');
});

// ── RULE 2: the type indicators ────────────────────────────────────────────

test('RULE 2: the five glyphs are the brief’s, one per type', () => {
  assert.equal(LAYER_TYPES.raster.glyph, '◉');
  assert.equal(LAYER_TYPES.vector.glyph, '◇');
  assert.equal(LAYER_TYPES.group.glyph, '▣');
  assert.equal(LAYER_TYPES.adjustment.glyph, '⌁');
  assert.equal(LAYER_TYPES.text.glyph, 'T');
  for (const info of Object.values(LAYER_TYPES)) {
    assert.ok(info.label.length > 0, `${info.type} reads as a word in a tooltip`);
    assert.ok(info.icon.length > 0, `${info.type} names its lucide icon`);
  }
});

test('RULE 2: the mark comes from the wire, never from the name', () => {
  const node = (over: Partial<SnapshotNodeWire>): SnapshotNodeWire =>
    ({ id: 'n', name: 'x', primitive: { type: 'rect', x: 0, y: 0, w: 10, h: 10 }, ...over }) as SnapshotNodeWire;

  assert.equal(layerTypeOf(node({}), false).type, 'vector');
  assert.equal(
    layerTypeOf(node({ name: 'text layer' }), false).type,
    'vector',
    'a rectangle called "text" is still a rectangle',
  );
  assert.equal(layerTypeOf(undefined, true).type, 'group', 'a group has no node — the wire says so');
  assert.equal(
    layerTypeOf(node({ text: {} as SnapshotNodeWire['text'] }), false).type,
    'text',
    'a run is a run because the snapshot carries its text',
  );

  // The mask mark is the one type that is real today and is not an object kind:
  // these two flags are the whole of the engine's compositing (Task 10.7 RULE 3).
  assert.equal(
    layerMark({ kind: 'layer', isGroup: false }, { alpha_locked: false, clipping_mask: false }).type,
    'group',
    'a plain paint layer is structure until a raster node exists (roadmap 2.1)',
  );
  assert.equal(
    layerMark({ kind: 'layer', isGroup: false }, { alpha_locked: true, clipping_mask: false }).glyph,
    '⌁',
  );
  assert.equal(
    layerMark({ kind: 'layer', isGroup: false }, { alpha_locked: false, clipping_mask: true }).type,
    'adjustment',
  );
  assert.equal(layerMark({ kind: 'node', isGroup: true }, undefined).type, 'group');
});

test('RULE 2: the row actually wears the mark, with its type in the tooltip', () => {
  const layer = (over: Partial<SnapshotLayerWire> = {}): SnapshotLayerWire => ({
    id: 'ink',
    name: 'Ink',
    visible: true,
    locked: false,
    alpha_locked: false,
    clipping_mask: false,
    clipped_to: null,
    children: [],
    child_names: [],
    child_is_group: [],
    child_can_open: [],
    child_parent: [],
    artboard: null,
    active: true,
    ...over,
  });
  const snapshot = (layers: SnapshotLayerWire[]): SnapshotWire =>
    ({ scene: { nodes: {} }, layers, artboards: [] }) as unknown as SnapshotWire;

  const markup = renderToStaticMarkup(
    <LayersPanel
      snapshot={snapshot([
        layer({ id: 'ink', name: 'Ink', alpha_locked: true }),
        layer({ id: 'sky', name: 'Sky' }),
      ])}
      expanded={new Set<string>()}
      onToggleExpanded={noop}
      onCommand={noop}
      disabled={false}
    />,
  );
  assert.ok(markup.includes('data-testid="layers-panel"'), 'the panel');
  assert.ok(
    markup.includes('type-mark type-adjustment'),
    'the alpha-locked layer wears the mask mark',
  );
  assert.ok(markup.includes('type-mark type-group'), 'and the plain one wears the group mark');
  assert.ok(markup.includes('>⌁<') || markup.includes('>⌁'), 'the glyph itself is in the row');
});

// ── RULE 3: the HUD, focus mode and the edge reveal ────────────────────────

test('RULE 3: the HUD is the brief’s five actions, gated only by what the engine lacks', () => {
  const one = canvasHud(1, 'Ship');
  assert.equal(one.caption, 'Ship', 'it says what it is acting on');
  const byId = (model: ReturnType<typeof canvasHud>, id: string) => {
    const action = model.actions.find((candidate) => candidate.id === id);
    assert.ok(action, `the HUD offers ${id}`);
    return action;
  };
  assert.deepEqual(
    one.actions.map((action) => action.id),
    ['duplicate', 'flip', 'rotate', 'boolean', 'more'],
  );
  assert.equal(byId(one, 'duplicate').enabled, true);
  assert.equal(byId(one, 'flip').enabled, true, 'Flip is a real MirrorAxis operation');
  assert.equal(byId(one, 'rotate').enabled, false, 'a node has no transform (roadmap 1.1)');
  assert.match(byId(one, 'rotate').reason ?? '', /roadmap 1\.1/, 'and it says so');
  assert.equal(byId(one, 'boolean').enabled, false, 'the engine’s booleans are binary (audit §2.4)');
  assert.match(byId(one, 'boolean').reason ?? '', /two objects/);

  const two = canvasHud(2, null);
  assert.equal(byId(two, 'boolean').enabled, true, 'with two, Boolean is live');
  assert.equal(byId(two, 'flip').enabled, false, 'and Flip steps aside');
  assert.match(byId(two, 'flip').reason ?? '', /one object/);

  const none = canvasHud(0, null);
  assert.equal(none.caption, 'Canvas');
  assert.equal(byId(none, 'duplicate').enabled, false);
});

test('RULE 3: the HUD draws nothing when nothing is selected, and five buttons when', () => {
  const empty = renderToStaticMarkup(
    <CanvasHUD left={0} top={0} selectionCount={0} primaryName={null} onAction={noop} />,
  );
  assert.equal(empty, '', 'no selection, no HUD — not a HUD with dead buttons');

  const markup = renderToStaticMarkup(
    <CanvasHUD left={120} top={40} selectionCount={1} primaryName="Ship" onAction={noop} />,
  );
  const hud = tagFor(markup, 'canvas-hud');
  assert.ok(hud.includes('style="left:120px;top:40px"'), `placed by the renderer’s camera: ${hud}`);
  assert.ok(hud.includes('aria-label="Actions for Ship"'), 'named for what it acts on');
  for (const id of ['duplicate', 'flip', 'rotate', 'boolean', 'more']) {
    const button = buttonFor(markup, `hud-${id}`);
    assert.ok(button.includes('<svg'), `${id} wears an icon`);
    assert.ok(button.includes('aria-label='), `${id} is named`);
  }
  assert.ok(tagFor(markup, 'hud-rotate').includes('disabled'), 'and Rotate is refusable, not absent');
  assert.ok(tagFor(markup, 'hud-more').includes('hud-more'), 'More keeps its divider');
});

test('RULE 3: Tab hides the chrome; an edge brings it back', () => {
  assert.equal(chromeVisible(false, false), true, 'the normal state');
  assert.equal(chromeVisible(true, false), false, 'focus mode: the artwork only');
  assert.equal(chromeVisible(true, true), true, 'the edge reveal');

  const box = { width: 1440, height: 900 };
  const middle = { x: 720, y: 450 };
  assert.equal(edgeReveal(middle, box), false, 'the middle of the canvas reveals nothing');
  assert.equal(edgeReveal({ x: 4, y: 450 }, box), true, 'the left edge wakes the dock');
  assert.equal(edgeReveal({ x: 1436, y: 450 }, box), true, 'the right edge the inspector');
  assert.equal(edgeReveal({ x: 720, y: 3 }, box), true, 'the top edge the file bar');
  assert.equal(edgeReveal({ x: 720, y: 897 }, box), true, 'the bottom edge Undo');
  assert.equal(edgeReveal(middle, { width: 0, height: 0 }), false, 'a box with no size has no edges');
});

test('RULE 3: the HUD floats above the selection’s own box, or not at all', () => {
  const rect = {
    id: 'r',
    name: 'Ship',
    primitive: { type: 'rect', x: 100, y: 200, w: 80, h: 40 },
  } as unknown as SnapshotNodeWire;
  assert.deepEqual(primitiveBox(rect.primitive), { x: 100, y: 200, w: 80, h: 40 });
  // 2 document units to the pixel ⇒ the 14px gap is 28 units.
  assert.deepEqual(selectionAnchor(rect, 2), { x: 140, y: 172 });

  const circle = {
    id: 'c',
    name: 'Moon',
    primitive: { type: 'circle', cx: 50, cy: 60, r: 20 },
  } as unknown as SnapshotNodeWire;
  // The circle's box top is cy − r = 40, and the 14px gap is 14 units at 1:1.
  assert.deepEqual(selectionAnchor(circle, 1), { x: 50, y: 26 });

  // A bare path has no box on the wire, so the honest answer is "no anchor"
  // rather than a guess that points the HUD at the wrong thing.
  const path = {
    id: 'p',
    name: 'Ink',
    primitive: { type: 'path', anchors: [] },
  } as unknown as SnapshotNodeWire;
  assert.equal(primitiveBox(path.primitive), null);
  assert.equal(selectionAnchor(path, 1), null);
  assert.equal(selectionAnchor(undefined, 1), null);
});

test('RULE 1 + 3: a point’s state is read from its own handles', () => {
  const anchor = (handle_in: [number, number] | null, handle_out: [number, number] | null) => ({
    slot: 'a0',
    x: 100,
    y: 100,
    handle_in,
    handle_out,
  });
  // Mirrored through the anchor: in = 2·anchor − out.
  assert.equal(nodeState([anchor([80, 100], [120, 100])], 'a0'), 'smooth');
  assert.equal(nodeState([anchor([80, 90], [120, 100])], 'a0'), 'corner');
  assert.equal(nodeState([anchor(null, [120, 100])], 'a0'), 'corner', 'a half-handle is a corner');
  assert.equal(nodeState([anchor([80, 100], null)], 'a0'), 'corner');
  assert.equal(nodeState([], 'a0'), 'corner', 'an anchor the overlay does not carry is a corner');
  assert.equal(nodeState([anchor([80.05, 100], [120, 100])], 'a0'), 'smooth', 'within tolerance');
});

// ── RULE 5: the theme and the house style ──────────────────────────────────

test('RULE 5: one palette — the CSS and the constant are the same numbers', () => {
  const css = readFileSync(fileURLToPath(new URL('../src/App.css', import.meta.url)), 'utf8');
  const token = (name: string): string => {
    const match = new RegExp(String.raw`--${name}:\s*([^;]+);`).exec(css);
    assert.ok(match, `App.css declares --${name}`);
    return match[1].trim();
  };
  for (const [name, expected] of [
    ['canvas', THEME.canvas],
    ['panel', THEME.panel],
    ['hover', THEME.hover],
    ['border', THEME.border],
    ['text', THEME.text],
    ['muted', THEME.textDim],
    ['accent', THEME.accent],
    ['danger', THEME.danger],
  ] as const) {
    assert.equal(token(name), expected, `--${name} matches THEME`);
  }
  assert.equal(THEME.canvas, '#121212');
  assert.equal(THEME.panel, '#1e1e1e');
  assert.equal(THEME.hover, '#252526');
  assert.equal(THEME.border, '#3e3e42');
  assert.equal(THEME.text, '#e0e0e0');
  assert.equal(THEME.textDim, '#a0a0a0');
  assert.match(token('font'), /^Inter,/, 'RULE 5’s type: Inter when the machine has it');
  assert.ok(css.includes('system-ui'), 'and the system face when it does not');
});

test('RULE 5: the studio drawer is a door, and the Developer tab is behind it', () => {
  const tabs = [
    { id: 'parameters', label: 'Parameters', icon: Sigma, content: <p>variables</p> },
    { id: 'engine', label: 'Engine', icon: Layers, content: <p>diagnostics</p>, dev: true },
  ];
  assert.equal(
    renderToStaticMarkup(
      <StudioDrawer
        open={false}
        onClose={noop}
        active="parameters"
        onActive={noop}
        dev={false}
        onDev={noop}
        disabled={false}
        tabs={tabs}
      />,
    ),
    '',
    'closed means not in the document at all',
  );

  const closed = renderToStaticMarkup(
    <StudioDrawer
      open
      onClose={noop}
      active="parameters"
      onActive={noop}
      dev={false}
      onDev={noop}
      disabled={false}
      tabs={tabs}
    />,
  );
  assert.ok(closed.includes('data-testid="studio-drawer"'), 'the drawer');
  assert.ok(closed.includes('data-testid="drawer-tab-parameters"'), 'the designer’s tab');
  assert.ok(!closed.includes('drawer-tab-engine'), 'the developer’s tab is not offered');
  assert.ok(closed.includes('data-testid="drawer-dev"'), 'but its switch is, visibly off');

  const dev = renderToStaticMarkup(
    <StudioDrawer
      open
      onClose={noop}
      active="engine"
      onActive={noop}
      dev
      onDev={noop}
      disabled={false}
      tabs={tabs}
    />,
  );
  assert.ok(dev.includes('data-testid="drawer-tab-engine"'), 'switched on, the tab appears');
  assert.ok(tagFor(dev, 'drawer-dev').includes('aria-pressed="true"'), 'and the switch says so');
  assert.ok(dev.includes('<svg'), 'every tab wears an icon');
});

// ── The shell itself ───────────────────────────────────────────────────────

test('RULE 1–5: the shell renders, and its default state has no debug surface', () => {
  const markup = renderToStaticMarkup(<App />);
  const shell = tagFor(markup, 'app-shell');
  assert.ok(shell.includes('data-focus="off"'), `the focus state is on the shell: ${shell}`);
  assert.ok(!shell.includes('focus-mode'), 'and focus mode is off');
  assert.ok(!shell.includes('drawer-open'), 'nothing is open that was not asked for');

  // RULE 1's three edges of the layout, plus the rails inside them.
  assert.ok(markup.includes('data-testid="top-bar"'), 'the top bar');
  assert.ok(markup.includes('data-testid="left-dock"'), 'the dock');
  assert.ok(markup.includes('data-testid="right-panel"'), 'the inspector');
  assert.ok(markup.includes('data-testid="bottom-bar"'), 'the bottom bar');
  assert.ok(markup.includes('data-testid="tab-layers"') && markup.includes('data-testid="tab-properties"'));
  assert.ok(markup.includes('data-testid="panel-collapse"'), 'and a way to give the canvas the room');
  assert.ok(markup.includes('data-testid="layers-panel"'), 'Layers is the default tab');
  assert.ok(!markup.includes('data-testid="properties-panel"'), 'Properties waits for a selection');
  assert.ok(markup.includes('data-testid="drag-status"'), 'the stage still reports the gesture');

  // Task 13.0 step 5: the debug surface is behind the Developer switch, and the
  // switch is off. No raw JSON, no evaluation chip, no dependency graph, no HUD
  // with nothing selected.
  for (const gone of [
    'studio-drawer',
    'diagnostics',
    'eval-chip',
    'deps-panel',
    'log-panel',
    'drawer-dev',
    'canvas-hud',
    'focus-edge-left',
  ]) {
    assert.ok(!markup.includes(`data-testid="${gone}"`), `${gone} is not in the default layout`);
  }
  assert.ok(!markup.includes('{"'), 'and no command JSON is printed as text');
  assert.ok(!markup.includes('eval.'), 'nor the engine’s evaluation vocabulary');
});

test('RULE 1: with no selection the bar is idle and the inspector is Layers', () => {
  const markup = renderToStaticMarkup(<App />);
  assert.ok(tagFor(markup, 'bottom-bar').includes('data-mode="idle"'), 'idle, not hidden');
  assert.ok(markup.includes('Nothing selected'), 'and it says so in words');
  assert.ok(!markup.includes('data-testid="bar-brush"'), 'no brush controls without a brush');
  assert.ok(!markup.includes('data-testid="bar-node"'), 'and no node controls without a node');
  assert.ok(markup.includes('data-testid="tab-layers"'), 'the Layers tab is there to be chosen');
  assert.ok(
    tagFor(markup, 'tab-layers').includes('aria-selected="true"'),
    'and it is the one selected',
  );
});
