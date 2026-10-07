/**
 * The **Task 10.6 panels actually mount** (RULEs 1, 3 and 4 at the rendering
 * level).
 *
 * The other files cover the pieces: `smoke.mjs` drives the engine, and
 * `wasm-client.test.ts` drives the client seam. Neither renders the UI a
 * designer looks at. This file does, with `react-dom/server` — no DOM, no
 * WebGPU, no wasm — which is exactly enough to catch the failure a type-check
 * cannot: a panel that throws (or silently prints the wrong *shape* of thing) on
 * a view the engine really produces.
 *
 * The house rules are on trial here, not just the plumbing:
 *
 * * every prop is a **labelled slider** or a swatch — a designer reads "Stroke ∝
 *   size", not `stroke_width: 2 @ 0.0833…`;
 * * a derived prop says what it follows, because that is the whole idea of the
 *   scaling law;
 * * nothing in the markup is JSON (RULE 4), including the stale-engine sentence;
 * * the ⌘K bar prints the engine's sentence for the selection and offers the
 *   engine's chips, and a chip fills the prompt rather than running it.
 *
 * Run: `npm run test:ui`.
 */
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { renderToStaticMarkup } from 'react-dom/server';

import ComponentPanel from '../src/components/ComponentPanel';
import MagicBar from '../src/components/MagicBar';
import { VectraClient } from '../src/engine/client';
import type { ComponentViewWire, StructuralMacroWire } from '../src/engine/wire';

const noop = () => {};

/** The view the engine produces for a 24px master with a 2px stroke (§65). */
const masterView = (): ComponentViewWire => ({
  status: 'ok',
  role: 'master',
  id: 'f3da711c-6ad2-401f-988b-37c3506b987c',
  name: 'Glyph',
  props: [
    { key: 'size', label: 'Size', ty: 'scalar', law: 'direct', derived: false, value: 24, min: 0, max: 96 },
    {
      key: 'stroke_width',
      label: 'Stroke',
      ty: 'scalar',
      law: 'scaled',
      derived: true,
      from: 'size',
      value: 2,
      min: 0,
      max: 8,
    },
    {
      key: 'corner_radius',
      label: 'Corner radius',
      ty: 'scalar',
      law: 'scaled',
      derived: true,
      from: 'size',
      value: 4,
      min: 0,
      max: 16,
    },
    { key: 'color', label: 'Color', ty: 'color', law: 'direct', derived: false, color: '#2266ee', min: 0, max: 0 },
  ],
  instances: 1,
  can_create: false,
  headline: "Component 'Glyph' — 4 prop(s), 1 instance(s).",
  selection: { count: 0, prose: 'Nothing selected' },
  masters: [{ id: 'f3da711c-6ad2-401f-988b-37c3506b987c', name: 'Glyph', instances: 1 }],
});

const panel = (over: Partial<Parameters<typeof ComponentPanel>[0]> = {}) =>
  renderToStaticMarkup(
    <ComponentPanel
      component={masterView()}
      selectionCount={1}
      ready
      engineIsOlder={false}
      staleSentence={VectraClient.STALE_ENGINE}
      iconSizes="16 32 48"
      onIconSizes={noop}
      onCreate={noop}
      onPlaceInstance={noop}
      onGenerateIconSet={noop}
      onSetProp={noop}
      {...over}
    />,
  );

/**
 * The **whole** tag carrying `data-testid`, not the slice from the test id on.
 *
 * React's server renderer emits attributes in its own order — `type` and `min`/
 * `max` land *before* the test id — so a slice that starts at the test id can
 * silently miss the very attributes a test means to check. Looking backwards to
 * the `<` is what makes these assertions about the element rather than about the
 * accident of attribute order.
 */
function tagFor(markup: string, testid: string): string {
  const at = markup.indexOf(`data-testid="${testid}"`);
  assert.ok(at >= 0, `no element with data-testid="${testid}"`);
  const open = markup.lastIndexOf('<', at);
  const close = markup.indexOf('>', at);
  return markup.slice(open, close + 1);
}

const MACROS: StructuralMacroWire[] = [
  { prompt: 'make this geometric', label: 'Geometric', hint: 'Snap the corners' },
  { prompt: 'create 4 color variations', label: '4 Color Variations', hint: 'Four takes' },
  { prompt: 'align perfectly', label: 'Align Perfectly', hint: 'Constraints, not nudges' },
];

test('RULE 1: a master renders its props as labelled sliders, one of them derived', () => {
  const markup = panel();

  assert.ok(markup.includes('data-testid="component-panel"'), 'the panel mounted');
  // React escapes the apostrophes in the headline (`&#x27;`), so the assertion
  // is on the sentence's words rather than on its markup.
  assert.ok(
    markup.includes('4 prop(s), 1 instance(s).') && markup.includes('Glyph'),
    'the headline is the engine’s sentence, printed verbatim',
  );
  for (const key of ['size', 'stroke_width', 'corner_radius', 'color']) {
    assert.ok(markup.includes(`data-testid="prop-${key}"`), `${key} has a row`);
  }

  // The three scalars are range inputs, each with the engine's bounds…
  for (const [key, max] of [
    ['size', 96],
    ['stroke_width', 8],
    ['corner_radius', 16],
  ] as const) {
    const tag = tagFor(markup, `prop-input-${key}`);
    assert.ok(tag.includes('type="range"'), `${key} is a slider: ${tag}`);
    assert.ok(tag.includes(`max="${max}"`), `${key} spans the engine's range: ${tag}`);
  }
  // …and a colour swatch, not a text field, for the colour.
  assert.ok(tagFor(markup, 'prop-input-color').includes('type="color"'), 'the swatch is a swatch');

  // The scaling law is *visible*: the two derived props say what they follow.
  assert.equal((markup.match(/∝ size/g) ?? []).length, 2, markup);
  assert.ok(markup.includes('follows size'), 'with the tooltip a designer can hover');

  // And the slider shows the number, not the ratio behind it.
  assert.ok(markup.includes('data-testid="prop-value-size"'), 'the value is printed');
  assert.ok(markup.includes('>24.0<'), 'twenty-four, one decimal, as written');

  // RULE 4, at the rendering level: no braces, no brackets, no raw keys. The
  // attributes may carry them (a test id *is* the key); the text a designer
  // reads may not.
  assert.ok(!/[{}[\]]/.test(markup), 'no JSON in the markup');
  const text = markup.replace(/<[^>]+>/g, ' ');
  assert.ok(!text.includes('stroke_width'), `the wire key never reaches the screen: ${text}`);
  assert.ok(!text.includes('__'), 'nor any other machine spelling');
});

test('RULE 1: an instance panel names its master, and the buttons reflect the role', () => {
  const view = masterView();
  const markup = panel({
    component: { ...view, role: 'instance', id: 'inst-1', master: view.id, can_create: false },
  });
  assert.ok(markup.includes('data-testid="component-master"'), 'the master is named');
  assert.ok(markup.includes('referencing Glyph'), 'by its name, not its uuid');
  // "Place Instance" belongs to a master; on an instance it is disabled, and
  // "Create Component" is live only when something is selected to group.
  assert.ok(tagFor(markup, 'component-instantiate').includes('disabled'), 'no instance of an instance');
  assert.ok(!tagFor(markup, 'component-create').includes('disabled'), 'one node is selected');
});

test('RULE 1: nothing selected is an empty state, not an empty panel', () => {
  const markup = panel({ component: null, selectionCount: 0 });
  assert.ok(markup.includes('select artwork'), 'the subtitle says what to do');
  assert.ok(markup.includes('Props appear here as sliders'), 'and the empty state explains itself');
  assert.ok(!markup.includes('data-testid="component-props"'), 'there is no list to render');
  for (const id of ['component-create', 'component-instantiate', 'icon-set']) {
    assert.ok(tagFor(markup, id).includes('disabled'), `${id} is disabled`);
  }
});

test('RULE 4: a stale engine explains itself in a sentence, and still mounts', () => {
  const markup = panel({ engineIsOlder: true, component: null, selectionCount: 1 });
  assert.ok(markup.includes('data-testid="engine-outdated"'), 'the panel says so');
  assert.ok(markup.includes(VectraClient.STALE_ENGINE), 'in the engine layer’s own words');
  assert.ok(!/[{}[\]]/.test(markup), 'and still no JSON, anywhere');
  assert.ok(markup.includes('data-testid="component-panel"'), 'the rest of the panel renders');
});

test('RULE 3: the Icon Studio is a size field and one button', () => {
  const markup = panel();
  assert.ok(tagFor(markup, 'icon-sizes').includes('value="16 32 48"'), 'the ladder is editable');
  assert.ok(markup.includes('Generate Icon Set'), 'and the macro has a button');
  assert.ok(
    markup.includes('One artboard per size; stroke and corners scale with it'),
    'with the law in the tooltip',
  );
});

test('RULE 2: the ⌘K bar offers the engine’s chips and prints the engine’s sentence', () => {
  const markup = renderToStaticMarkup(
    <MagicBar
      open
      ready
      text=""
      onText={noop}
      selectionProse="glyph, badge on “Artboard 1”"
      macros={MACROS}
      receipt="✨ Snapped 2 shapes to whole numbers and unified them into one shape."
      onRun={noop}
      onClose={noop}
    />,
  );

  assert.ok(markup.includes('data-testid="magic-backdrop"'), 'the bar is up');
  assert.ok(markup.includes('data-testid="magic-input"'), 'with one prompt field');
  // The subject of "this", in the engine's words — the thing a prompt acts on.
  assert.ok(markup.includes('glyph, badge on “Artboard 1”'), 'the selection is named');
  // The chips are engine data: one per macro, labelled by the engine, hinting
  // with the engine's hint.
  assert.equal((markup.match(/magic-chip-/g) ?? []).length, 3, markup);
  assert.ok(markup.includes('data-testid="magic-chip-4-color-variations"'), 'labels become test ids');
  assert.ok(markup.includes('Constraints, not nudges'), 'the hint travels with the chip');
  // The receipt is a sentence (RULE 4) and it is the *only* thing shown back.
  assert.ok(markup.includes('data-testid="magic-receipt"'), 'the receipt is on screen');
  assert.ok(markup.includes('✨ Snapped 2 shapes'), 'verbatim');
  assert.ok(!/[{}[\]]/.test(markup), 'no JSON in the bar');
  // An empty prompt cannot run: the button is disabled until there is something
  // to say.
  assert.ok(tagFor(markup, 'magic-run').includes('disabled'), 'nothing to run yet');
  assert.ok(markup.includes('It never draws pixels'), 'the bar says what the AI is');
});

test('RULE 2: a closed bar renders nothing, and an empty selection invites one', () => {
  assert.equal(
    renderToStaticMarkup(
      <MagicBar
        open={false}
        ready
        text=""
        onText={noop}
        selectionProse=""
        macros={MACROS}
        receipt={null}
        onRun={noop}
        onClose={noop}
      />,
    ),
    '',
    'closed is absent, not hidden',
  );
  const markup = renderToStaticMarkup(
    <MagicBar
      open
      ready
      text="align perfectly"
      onText={noop}
      selectionProse=""
      macros={[]}
      receipt={null}
      onRun={noop}
      onClose={noop}
    />,
  );
  assert.ok(markup.includes('select artwork, then describe what it should become'), 'the invitation');
  assert.ok(!markup.includes('data-testid="magic-receipt"'), 'no receipt before a run');
  assert.ok(!tagFor(markup, 'magic-run').includes('disabled'), 'a prompt can run');
});
