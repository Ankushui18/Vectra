/**
 * The browser boundary, against the **real binary** (Task 10.6).
 *
 * Three layers are covered elsewhere: the Rust laws (`cargo test`), the engine
 * through its own exports (`scripts/smoke.mjs`, steps 65–68), and the pure
 * view-model projections (`view-model.test.ts`). The seam in between —
 * `VectraClient`'s typed wrappers over `src/wasm/vectra_wasm.js`, which is the
 * *only* thing the ⌘K bar, the component panel and the icon studio ever call —
 * ran nowhere: `client.ts` was imported for its `engineIsOlderThanUi` probe and
 * its `STALE_ENGINE` sentence, but no test had ever driven the wrappers.
 *
 * That gap is exactly where erasure hides bugs. A wrapper that parses JSON into
 * a TS type is checked against that type by the compiler and against *reality*
 * by nothing: `as ComponentViewWire` will happily hand the panel `undefined` for
 * every field if the engine's key names ever drift. So this file loads the wasm
 * the browser loads and walks one designer session through the client's own
 * methods.
 *
 * ## Seeding the module
 *
 * `getEngine()` passes Vite's `?url` import, which the test bundler stubs to
 * `""` (see `scripts/test-ui.mjs`). Instantiating the shared module here first —
 * with the real bytes, read from disk — is therefore not a bypass: wasm-bindgen's
 * `init` returns the module it already has, so `VectraClient.create()` afterwards
 * follows its ordinary path, and a change that made it *not* do so would fail
 * loudly here (a `fetch("")` rejection) rather than quietly pass.
 *
 * Run: `npm run test:ui` (bundles with esbuild, executes under `node --test`).
 */
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';
import { VectraClient } from '../src/engine/client';
import type { CommandWire } from '../src/engine/wire';
import init from '../src/wasm/vectra_wasm.js';

const bytes = readFileSync(new URL('../src/wasm/vectra_wasm_bg.wasm', import.meta.url));
await init({ module_or_path: bytes });

/** A rect the way the app builds one: a `CreateNode` with literal slots. */
function rect(id: string, name: string, size: number, radius: number): CommandWire {
  return {
    type: 'CreateNode',
    id,
    name,
    kind: {
      Rectangle: {
        x: { Literal: 0 },
        y: { Literal: 0 },
        width: { Literal: size },
        height: { Literal: size },
        corner_radius: { Literal: radius },
      },
    },
  };
}

test('a designer session through the client: master, prop, instance, icon set', async () => {
  const client = await VectraClient.create();

  // A real engine is not an old engine — the probe, on the artifact that ships.
  assert.equal(client.engineIsOlder, false, 'the built wasm exports the Task 10.6 surface');

  const glyph = 'c1aaaaaa-0000-4000-8000-000000000001';
  const dot = 'c1aaaaaa-0000-4000-8000-000000000002';
  assert.equal(client.dispatch(rect(glyph, 'glyph', 24, 4)).status, 'ok');
  assert.equal(
    client.dispatch({
      type: 'CreateNode',
      id: dot,
      name: 'dot',
      kind: { Circle: { cx: { Literal: 40 }, cy: { Literal: 12 }, radius: { Literal: 12 } } },
    }).status,
    'ok',
  );
  for (const id of [glyph, dot]) {
    // Both halves of a visible stroke: the layer's colour and its width.
    assert.equal(
      client.dispatch({
        type: 'SetParameter',
        node_id: id,
        property: 'style.stroke',
        value: { Color: { Literal: { r: 16, g: 16, b: 16, a: 255 } } },
      }).status,
      'ok',
    );
    assert.equal(
      client.dispatch({
        type: 'SetParameter',
        node_id: id,
        property: 'style.stroke_width',
        value: { Float: { Literal: 2 } },
      }).status,
      'ok',
    );
  }

  // ── What "this" means: the selection, in the engine's own words ──────────
  const selection = client.setSelection([glyph, dot]);
  assert.ok(selection, 'setSelection answers on a current engine');
  assert.equal(selection.count, 2);
  assert.ok(selection.prose.includes('glyph'), selection.prose);
  assert.equal(
    client.setSelection([])?.prose,
    'Nothing selected',
    'and an empty selection is still a sentence, not an empty string',
  );
  assert.equal(client.setSelection([glyph, dot])?.count, 2);

  // ── Before: a selection that *can* become a component ───────────────────
  const before = client.componentView();
  assert.ok(before, 'componentView answers on a current engine');
  assert.equal(before.status, 'ok');
  assert.equal(before.role, 'selection');
  assert.equal(before.can_create, true);
  assert.deepEqual(before.props, [], 'no props yet — there is no component yet');
  assert.equal(before.selection.count, 2);
  assert.ok(!/[{}[\]]/.test(before.headline), `a sentence, not JSON: ${before.headline}`);

  // ── Create Component (RULE 1) ───────────────────────────────────────────
  const made = client.createComponent([glyph, dot], 'Glyph');
  assert.equal(made.status, 'ok', JSON.stringify(made));
  assert.ok(made.prose && made.prose.length > 0, 'the receipt is a sentence');
  assert.ok(!/[{}[\]]/.test(made.prose!), `RULE 4, through the wrapper: ${made.prose}`);

  const master = client.componentView();
  assert.equal(master?.role, 'master');
  assert.equal(master?.instances, 0);
  assert.ok(master?.id, 'the panel reads the master id from the view, not from an envelope');
  assert.deepEqual(
    master!.props.map((prop) => prop.key),
    ['size', 'stroke_width', 'corner_radius', 'color'],
    'the four designer props',
  );
  const size = master!.props.find((prop) => prop.key === 'size')!;
  assert.equal(size.value, 24, 'the master reads at its design size');
  assert.equal(size.law, 'direct');
  const stroke = master!.props.find((prop) => prop.key === 'stroke_width')!;
  assert.equal(stroke.law, 'scaled');
  assert.equal(stroke.derived, true);
  assert.equal(stroke.from, 'size', 'stroke follows size');
  assert.ok(stroke.max >= (stroke.value ?? 0) * 4, 'the slider spans a real range');

  // ── Place an instance, and move *it* alone ─────────────────────────────
  const placed = client.instantiateComponent(master!.id!, 'big');
  assert.equal(placed.status, 'ok', JSON.stringify(placed));
  assert.ok(placed.created, 'the envelope names what it made — and the wire type says so');
  const instance = placed.created!;

  const instanceView = (() => {
    client.setSelection([instance]);
    return client.componentView();
  })();
  assert.equal(instanceView?.role, 'instance');
  assert.equal(instanceView?.master, master!.id);
  assert.equal(
    instanceView?.props.find((prop) => prop.key === 'size')?.value,
    24,
    'a fresh instance is a copy of the master, not a 1-unit speck (§6.1a of the report)',
  );

  // Only the rectangles, narrowed: the scene is a union of primitives, and the
  // props under test (width, corner radius) are the rectangle's own.
  const rects = () => {
    const out: { id: string; w: number; corner_radius: number; stroke: number }[] = [];
    for (const [id, node] of Object.entries(client.snapshot().scene.nodes)) {
      if (node.primitive.type !== 'rect') continue;
      out.push({
        id,
        w: node.primitive.w,
        corner_radius: node.primitive.corner_radius,
        stroke: node.style.stroke_width,
      });
    }
    return out;
  };

  const written = client.setComponentProp(instance, 'size', { Float: { Literal: 40 } });
  assert.equal(written.status, 'ok', JSON.stringify(written));
  assert.ok(written.prose && !/[{}[\]]/.test(written.prose), 'a sentence, not JSON');

  const at = (size: number) => rects().find((rect) => rect.w === size);
  assert.equal(at(24)?.corner_radius, 4, "the master's own artwork did not move");
  assert.equal(at(24)?.stroke, 2);
  assert.ok(at(40), 'the instance took the new size');
  assert.ok(Math.abs(at(40)!.corner_radius - 4 * (40 / 24)) < 1e-6, 'corner scales');
  assert.ok(Math.abs(at(40)!.stroke - 2 * (40 / 24)) < 1e-6, 'stroke scales');

  // One undo, one write: the instance is a copy again.
  assert.equal(client.undo().status, 'ok');
  assert.equal(at(40), undefined);
  assert.equal(at(24)?.corner_radius, 4);

  // ── Icon Studio (RULE 3) ───────────────────────────────────────────────
  const set = client.iconSet(master!.id!, [16, 32, 48]);
  assert.equal(set.status, 'ok', JSON.stringify(set));
  assert.ok(set.prose && !/[{}[\]]/.test(set.prose), 'a sentence, not JSON');
  const boards = client.snapshot().artboards ?? [];
  for (const size of [16, 32, 48]) {
    assert.ok(
      boards.some((board) => board.bounds[2] === size && board.bounds[3] === size),
      `an artboard per size: ${JSON.stringify(boards.map((board) => board.bounds))}`,
    );
  }
  const clones = rects().filter((rect) => [16, 32, 48].includes(rect.w));
  assert.equal(clones.length, 3, `three scaled copies: ${JSON.stringify(clones)}`);
  for (const clone of clones) {
    assert.ok(
      Math.abs(clone.stroke / clone.w - 2 / 24) < 1e-9,
      `the stroke law holds at ${clone.w}px: ${JSON.stringify(clone)}`,
    );
  }
  assert.ok(
    (clones.find((clone) => clone.w === 16)?.stroke ?? 0) >= 1,
    'and 16px is not a hairline',
  );

  // ── The command bar's chips, and the refusal path ──────────────────────
  const macros = client.structuralMacros();
  assert.equal(macros.length, 5, JSON.stringify(macros));
  for (const macro of macros) {
    assert.ok(macro.prompt && macro.label && macro.hint, JSON.stringify(macro));
    assert.ok(!/[{}[\]]/.test(`${macro.label}${macro.hint}`), JSON.stringify(macro));
  }

  const refused = client.aiExecuteWithRetry('make it glitter in the dark');
  assert.equal(refused.status, 'error', JSON.stringify(refused));
  assert.ok(refused.message.length > 0);
  assert.ok(!/[{}[\]]/.test(refused.message), `a refusal is a sentence: ${refused.message}`);
});

test('a stale engine degrades in sentences, through the same wrappers', () => {
  // A checkout whose wasm predates the Rust: the engine object has none of the
  // Task 10.6 exports. `VectraClient`'s constructor is private on purpose (the
  // browser builds it from the singleton), so the client is assembled the way
  // the module system would: an instance of the real class, holding a stand-in
  // engine. The alternative — exporting a test-only constructor — would put a
  // seam in production code to test the *absence* of one.
  const client = Object.create(VectraClient.prototype) as VectraClient;
  // `{}` is the whole point: an engine object with nothing on it, which is what
  // a pre-10.6 wasm produces for every Task 10.6 method.
  Object.assign(client, { engine: {} });

  assert.equal(client.engineIsOlder, true, 'the probe sees the gap');

  for (const reply of [
    client.createComponent(['irrelevant']),
    client.instantiateComponent('irrelevant'),
    client.setComponentProp('irrelevant', 'size', { Float: { Literal: 1 } }),
    client.iconSet('irrelevant', [16]),
  ]) {
    assert.equal(reply.status, 'error');
    assert.equal(reply.message, VectraClient.STALE_ENGINE, 'one sentence, naming the build command');
    assert.ok(!/[{}[\]]/.test(reply.message), 'and no JSON in it');
  }

  // Reads answer "nothing to show" rather than throwing: a panel renders an
  // empty state, it does not crash the studio.
  assert.equal(client.componentView(), null);
  assert.equal(client.setSelection(['x']), null);
  assert.deepEqual(client.structuralMacros(), []);
});
