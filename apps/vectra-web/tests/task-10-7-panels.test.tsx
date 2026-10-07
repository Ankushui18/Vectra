/**
 * **Task 10.7's panels render, and say the right thing** — the two surfaces the
 * Procreate layer adds to the workspace, mounted with no DOM and no GPU.
 *
 * This is the half a type-check cannot reach: the StreamLine slider must be a
 * real range input the hand can drag, the colour well must be a *draggable*
 * swatch (not a button that fills on press), and a layer row must carry the two
 * new flags as glyph buttons — with the clipping toggle **disabled** on the
 * bottom layer, because there is nothing under it to clip to.
 *
 * Run: `npm run test:ui`.
 */
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { renderToStaticMarkup } from 'react-dom/server';

import DrawSettingsPanel from '../src/components/DrawSettingsPanel';
import { LayersPanel } from '../src/components/LayersPanel';
import { setLayerAlphaLocked, setLayerClippingMask } from '../src/engine/commands';
import { layerRows } from '../src/engine/panels';
import type { SnapshotLayerWire, SnapshotWire } from '../src/engine/wire';

const noop = () => {};

/**
 * The **whole** tag carrying `data-testid`, not the slice from the test id on.
 * React's server renderer emits attributes in its own order, so a slice that
 * starts at the test id can silently miss the attributes a test means to check.
 */
function tagFor(markup: string, testid: string): string {
  const at = markup.indexOf(`data-testid="${testid}"`);
  assert.ok(at >= 0, `no element with data-testid="${testid}"`);
  const open = markup.lastIndexOf('<', at);
  const close = markup.indexOf('>', at);
  return markup.slice(open, close + 1);
}

/**
 * The whole `<button>`, attributes **and** the glyph it draws.
 *
 * `tagFor` stops at the first `>`, which is right for attributes and wrong for
 * the glyph: these toggles say their state with the character inside them
 * (α/a, ▤/▥), so a test that only looked at the opening tag would pass on a
 * button that rendered nothing at all.
 */
function buttonFor(markup: string, testid: string): string {
  const at = markup.indexOf(`data-testid="${testid}"`);
  assert.ok(at >= 0, `no element with data-testid="${testid}"`);
  const open = markup.lastIndexOf('<', at);
  const close = markup.indexOf('</button>', at);
  return markup.slice(open, close + '</button>'.length);
}

// ── RULE 2 + RULE 4: the pen & brush settings ──────────────────────────────

const settings = (over: Partial<Parameters<typeof DrawSettingsPanel>[0]> = {}) =>
  renderToStaticMarkup(
    <DrawSettingsPanel
      streamline={50}
      onStreamline={noop}
      color="#2266ee"
      onColor={noop}
      onDropStart={noop}
      dragging={false}
      tool="brush"
      {...over}
    />,
  );

test('RULE 2: the StreamLine slider is a real 0–100 range input with a readout', () => {
  const markup = settings();
  assert.ok(markup.includes('data-testid="draw-settings"'), 'the panel');
  const slider = tagFor(markup, 'streamline-slider');
  assert.ok(slider.includes('type="range"'), `a range input, not a number box: ${slider}`);
  assert.ok(slider.includes('min="0"') && slider.includes('max="100"'), 'the full 0–100 span');
  assert.ok(slider.includes('aria-label="StreamLine smoothing"'), 'named for a screen reader');
  assert.ok(slider.includes('value="50"'), 'the slider sits where it was told to');
  assert.ok(
    markup.includes('data-testid="streamline-readout"') && markup.includes('50%'),
    'and the caption reads the same number the filter will use',
  );
  assert.ok(markup.includes('raw') && markup.includes('max'), 'both ends are labelled in words');
  assert.ok(!markup.includes('<select'), 'no word-list picker for a continuous amount');
});

test('RULE 2: the readout is the filter’s own summary, not a second calculation', () => {
  assert.ok(settings({ streamline: 0 }).includes('raw input'), '0 % says it is off');
  assert.ok(settings({ streamline: 100 }).includes('6-sample pull'), '100 % names the window');
  assert.ok(settings({ streamline: 100 }).includes('≤28px lag'), 'and the lag it promises');
});

test('RULE 4: the colour well is a swatch you drag, not a button you press', () => {
  const markup = settings({ color: '#ff8800' });
  const well = tagFor(markup, 'colordrop-well');
  assert.ok(well.includes(`aria-label="Drag colour #ff8800 onto the canvas to fill"`), well);
  assert.ok(well.includes('style="background:#ff8800"'), 'the well *is* the colour');
  assert.ok(!markup.includes('>Fill<'), 'nothing that reads as “press me to fill”');
  assert.ok(
    markup.includes('data-testid="colordrop-color"'),
    'and a plain colour input to change what the well carries',
  );
  assert.ok(
    markup.includes('empty space does nothing'),
    'the rule is stated where the gesture is taught',
  );
  // While the colour is in flight the well says so.
  assert.ok(tagFor(settings({ dragging: true }), 'colordrop-well').includes('held'), 'the held state');
});

// ── RULE 3: the two flags in the Layers Panel ──────────────────────────────

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
  ({
    scene: { nodes: {} },
    layers,
    artboards: [],
  }) as unknown as SnapshotWire;

test('RULE 3: a layer row carries the alpha lock and the clipping mask', () => {
  // The engine's own order: back → front. `Sky` sits on top of `Ink` and clips
  // to it; `Ink` is the bottom layer, so it has nothing to clip to.
  const layers = [
    layer({ id: 'ink', name: 'Ink', alpha_locked: true, active: true }),
    layer({ id: 'top', name: 'Sky', clipping_mask: true, clipped_to: 'ink' }),
  ];
  const markup = renderToStaticMarkup(
    <LayersPanel
      snapshot={snapshot(layers)}
      expanded={new Set<string>()}
      onToggleExpanded={noop}
      onCommand={noop}
      disabled={false}
    />,
  );
  assert.ok(markup.includes('data-testid="layers-panel"'), 'the panel renders');

  const alpha = tagFor(markup, 'layer-alpha-ink');
  assert.ok(alpha.includes('aria-pressed="true"'), `a pressed toggle: ${alpha}`);
  assert.ok(buttonFor(markup, 'layer-alpha-ink').includes('α'), 'the Greek α when locked — the state is the glyph');
  assert.ok(!alpha.includes('disabled'), 'and it is usable');
  assert.ok(alpha.includes('Unlock alpha'), 'it offers the way back');
  assert.ok(buttonFor(markup, 'layer-alpha-top').includes('>a<'), 'plain a when unlocked');
  assert.ok(tagFor(markup, 'layer-alpha-top').includes('Lock alpha'), 'and the way in');

  const clip = tagFor(markup, 'layer-clip-top');
  assert.ok(clip.includes('aria-pressed="true"'), clip);
  assert.ok(buttonFor(markup, 'layer-clip-top').includes('▤'), 'the clipped-to glyph');
  // The bottom layer has nothing under it: the toggle exists but refuses.
  const bottom = tagFor(markup, 'layer-clip-ink');
  assert.ok(bottom.includes('disabled'), `nothing to clip to ⇒ disabled: ${bottom}`);
  assert.ok(
    bottom.includes('Nothing below this layer to clip to'),
    'and it says why, in the designer’s words',
  );
});

test('RULE 3: the panel computes the clip target from the layer below, not the row above', () => {
  // Three layers: only the two upper ones have a layer below them.
  const layers = [
    layer({ id: 'a', name: 'Base' }),
    layer({ id: 'b', name: 'Mid', clipped_to: 'a' }),
    layer({ id: 'c', name: 'Top', clipped_to: 'b', clipping_mask: true }),
  ];
  const markup = renderToStaticMarkup(
    <LayersPanel
      snapshot={snapshot(layers)}
      expanded={new Set<string>()}
      onToggleExpanded={noop}
      onCommand={noop}
      disabled={false}
    />,
  );
  assert.ok(tagFor(markup, 'layer-clip-a').includes('disabled'), 'the bottom row cannot clip');
  assert.ok(!tagFor(markup, 'layer-clip-b').includes('disabled'), 'the middle row can');
  assert.ok(!tagFor(markup, 'layer-clip-c').includes('disabled'), 'and so can the top row');
  // Rows are listed top-first, the way every design tool shows them.
  assert.ok(
    markup.indexOf('layer-row-c') < markup.indexOf('layer-row-a'),
    'top layer, top row',
  );
});

test('RULE 3: the row carries the flags, and the clip target is the layer below', () => {
  // Engine order: back → front. Only the two upper layers have a layer below.
  const layers = [
    layer({ id: 'a', name: 'Base' }),
    layer({ id: 'b', name: 'Mid', alpha_locked: true, clipped_to: 'a' }),
    layer({ id: 'c', name: 'Top', clipping_mask: true, clipped_to: 'b' }),
  ];
  const rows = layerRows(snapshot(layers), new Set<string>());
  // The panel lists top-first — the inversion is the panel's only opinion.
  assert.deepEqual(
    rows.map((row) => row.id),
    ['c', 'b', 'a'],
  );
  assert.equal(rows[0].clippingMask, true);
  assert.equal(rows[0].clippedTo, 'b');
  assert.equal(rows[1].alphaLocked, true);
  assert.equal(rows[1].clippingMask, false, 'a missing flag reads as off');
  assert.equal(rows[2].clippedTo, null, 'the bottom layer clips to nothing');
});

test('RULE 3: the two toggles are the commands the engine already knows', () => {
  assert.deepEqual(setLayerAlphaLocked('ink', true), {
    type: 'SetLayerAlphaLocked',
    id: 'ink',
    alpha_locked: true,
  });
  assert.deepEqual(setLayerClippingMask('ink', false), {
    type: 'SetLayerClippingMask',
    id: 'ink',
    clipping_mask: false,
  });
});
