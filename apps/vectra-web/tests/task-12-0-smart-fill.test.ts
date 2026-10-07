/**
 * **Task 12.0's laws at the UI boundary** — the four rules as they are *decided*
 * on this side of the wasm boundary, with no browser and no engine.
 *
 * The geometry is the engine's: `smart_fill_plan` intersects the paths, refines
 * the arrangement into faces, measures them and reports the spans. What the UI
 * owns is small and precise, which is exactly why it is testable here:
 *
 * * **RULE 1** — the tool highlights the face the engine's probe point hit, and
 *   *only* that one: a pointer over empty space highlights nothing rather than
 *   the nearest face, and every ring it draws is the engine's own coordinate,
 *   mapped by the renderer's camera (never by a transform of the UI's).
 * * **RULE 2 + RULE 4** — a drop inside an enclosed area creates a `SmartFill`
 *   carrying the face's own boundaries and the dropped point, and takes
 *   precedence over recolouring whatever shape is underneath.
 * * **RULE 3** — the panel's rows carry the engine's arc lengths, and Break
 *   sends them back untouched; a path with no crossings offers no break.
 *
 * Run: `npm run test:ui`.
 */
import assert from 'node:assert/strict';
import { test } from 'node:test';
import {
  breakSpans,
  breakStatus,
  placeRegion,
  regionAt,
  regionShape,
  regionStatus,
  sourceRows,
  spanRows,
} from '../src/engine/draw/regions';
import { dropRegion, dropStatus, planColorDrop } from '../src/engine/draw/colordrop';
import { createSmartFill, isSmartFill } from '../src/engine/commands';
import { TOOLS, isDirectTool, showsMathPanels, toolForShortcut } from '../src/engine/draw/tools';
import type { RegionPlanWire } from '../src/engine/wire';

// ── Fixtures (shaped exactly like the engine's plan JSON) ────────────────

const A = 'a-0000-0000-0000-000000000001';
const B = 'b-0000-0000-0000-000000000002';

/** Two overlapping circles: A only, B only and the lens between them. */
const PLAN: RegionPlanWire = {
  sources: [
    {
      id: A,
      name: 'circle A',
      area: 31415.9,
      bounds: [100, 100, 300, 300],
      spans: [
        { index: 0, from: 0, to: 148.3, ring: 0, start: [260, 280], end: [260, 120], length: 148.3, total: 628.3 },
        { index: 1, from: 148.3, to: 628.3, ring: 0, start: [260, 120], end: [260, 280], length: 480, total: 628.3 },
      ],
    },
    {
      id: B,
      name: 'circle B',
      area: 31415.9,
      bounds: [220, 100, 420, 300],
      // Nothing crosses B in this fixture: a source *can* have no spans, and the
      // UI must not invent one.
      spans: [],
    },
  ],
  regions: [
    { index: 0, area: 8946.2, holes: 0, members: [A, B], path: 'M260 280L…Z', rings: [[[260, 280], [260, 120], [300, 200]]] },
    { index: 1, area: 22469.7, holes: 0, members: [A], path: 'M…Z', rings: [[[100, 200], [260, 280], [260, 120]]] },
    { index: 2, area: 22469.7, holes: 0, members: [B], path: 'M…Z', rings: [[[420, 200], [260, 120], [260, 280]]] },
  ],
  crossings: [
    [260, 280],
    [260, 120],
  ],
  hit: 0,
};

const MISS: RegionPlanWire = { ...PLAN, hit: null };

// ── RULE 1: the region graph's UI half ───────────────────────────────────

test('the hit face is the only face the tool highlights', () => {
  assert.equal(regionAt(PLAN)?.members.join(), `${A},${B}`, 'the lens, by signature');
  assert.equal(regionAt(MISS), null, 'no hit, no face — never "the nearest one"');
  assert.equal(regionAt(null), null, 'and no plan at all is not an error');
});

test('the overlay draws the engine coordinates the camera mapped', () => {
  const shape = regionShape(PLAN);
  assert.equal(shape.rings.length, 1, "the hit face's exterior ring");
  assert.deepEqual(shape.rings[0][0], [260, 280], 'the ring point, verbatim');
  assert.equal(shape.crossings.length, 2, 'both crossings, for the markers');

  // The mapper is the renderer's camera: a pure translation here stands in for
  // it, and the overlay must contain exactly what the camera answered.
  const placed = placeRegion(shape, (points) => points.map(([x, y]) => [x + 10, y + 20]));
  assert.deepEqual(placed.rings[0][0], [270, 300]);
  assert.deepEqual(placed.crossings[1], [270, 140]);
  assert.deepEqual(placeRegion(shape, () => []), { rings: [], crossings: [] },
    'a camera with no answer yields an empty overlay, never a misplaced one');
});

test('empty space highlights nothing at all', () => {
  const shape = regionShape(MISS);
  assert.deepEqual(shape.rings, [], 'no face, no rings');
  assert.equal(shape.crossings.length, 2, 'the crossings are still worth showing');
  assert.match(regionStatus(MISS), /no enclosed area/);
});

test('the status line names the region, its boundaries and its holes', () => {
  assert.equal(regionStatus(PLAN), '◲ Smart Fill · region 1 of 3 · 2 boundaries');
  // The engine numbers a face by its place in the list, so a one-face plan
  // carries `index: 0` — the fixture says so too.
  const holed: RegionPlanWire = {
    ...PLAN,
    regions: [{ ...PLAN.regions[1], index: 0, holes: 1 }],
    hit: 0,
  };
  assert.equal(regionStatus(holed), '◲ Smart Fill · region 1 of 1 · 1 boundary · 1 hole');
  assert.match(regionStatus(null), /unavailable/);
});

// ── RULE 4 (and RULE 2's identity): the drop ─────────────────────────────

test('a drop inside an enclosed area creates a SmartFill, not a recolour', () => {
  const region = dropRegion(PLAN, [280, 200]);
  assert.ok(region, 'the engine identified a region');
  assert.deepEqual(region.boundaries, [A, B], 'the face holds both boundaries');
  assert.deepEqual(region.seed, [280, 200], 'pinned to the dropped point');

  // Even when a shape is under the pointer — the drop is inside a *face*, and
  // that is the answer RULE 4 wants.
  const plan = planColorDrop(A, '#e8622c', region);
  assert.equal(plan.kind, 'smartFill');
  if (plan.kind !== 'smartFill') return;
  assert.deepEqual(plan.boundaries, [A, B]);
  assert.deepEqual(plan.seed, [280, 200]);
  assert.equal(plan.color, '#e8622c');
  assert.match(dropStatus(plan, null), /new smart fill in the region between 2 paths/);

  // The wire it produces is a `CreateSmartFill` with those numbers.
  const command = createSmartFill({
    boundaries: plan.boundaries,
    seed: plan.seed,
    fill: { r: 232, g: 98, b: 44, a: 255 },
    name: plan.name,
  });
  assert.equal(command.type, 'CreateSmartFill');
  assert.equal(isSmartFill(command), true);
  assert.deepEqual(command.boundaries, [A, B]);
  assert.deepEqual(command.seed, [280, 200]);
  assert.deepEqual(command.fill, { r: 232, g: 98, b: 44, a: 255 });
});

test('a drop outside every region still recolours the shape beneath it', () => {
  assert.equal(dropRegion(MISS, [10, 10]), null, 'no face, no region');
  const plan = planColorDrop(B, '#112233', dropRegion(MISS, [10, 10]));
  assert.deepEqual(plan, { kind: 'fill', nodeId: B, color: '#112233' });
  // And with no shape either, nothing happens — with a reason.
  const nothing = planColorDrop(null, '#112233', null);
  assert.equal(nothing.kind, 'noop');
  // A malformed colour is refused before either branch runs.
  assert.equal(planColorDrop(null, 'not a colour', null).kind, 'noop');
});

// ── RULE 3: spans and the break ──────────────────────────────────────────

test('the panel lists the engine spans and sends them back untouched', () => {
  const rows = spanRows(PLAN, A);
  assert.equal(rows.length, 2, 'the source has two spans');
  assert.equal(rows[0].label, 'span 1 of 2 · ring 1 · 148.3 of 628.3');
  assert.deepEqual(breakSpans(rows), [[0, 148.3], [148.3, 628.3]],
    'the arc lengths go back exactly as they came');
  // One row is "break this span"; the id is what the engine reported.
  assert.deepEqual(breakSpans([rows[1]]), [[148.3, 628.3]]);
  assert.equal(breakStatus(rows, 'circle A'), '✂ Broke circle A at 2 spans → 4 paths');
  assert.equal(breakStatus([rows[0]], 'circle A'), '✂ Broke circle A at 1 span → 2 paths');
});

test('a path with no crossings offers no break', () => {
  assert.deepEqual(spanRows(PLAN, B), [], 'nothing crosses B, so there is no span to break');
  assert.deepEqual(spanRows(PLAN, 'not-a-source'), []);
  assert.deepEqual(spanRows(null, A), []);
  assert.deepEqual(breakSpans([]), []);
  assert.deepEqual(sourceRows(PLAN).map((s) => s.name), ['circle A', 'circle B']);
  assert.deepEqual(sourceRows(null), []);
});

// ── the tool itself ──────────────────────────────────────────────────────

test('the Smart Fill tool exists, is reachable, and keeps its panels', () => {
  const spec = TOOLS.find((tool) => tool.id === 'smartFill');
  assert.ok(spec, 'a Smart Fill tool is in the palette');
  assert.equal(spec.shortcut, 'F');
  assert.equal(toolForShortcut('f'), 'smartFill', 'the shortcut is the palette’s');
  assert.equal(toolForShortcut('F'), 'smartFill');
  // Shortcuts are unique — two tools on one letter would be a bug the palette
  // could not report.
  const shortcuts = TOOLS.map((tool) => tool.shortcut);
  assert.equal(new Set(shortcuts).size, shortcuts.length, JSON.stringify(shortcuts));
  // It is not a drawing tool: what it makes is a node with an appearance stack
  // the designer then edits, so RULE 4's gate must keep the panels.
  assert.equal(isDirectTool('smartFill'), false);
  assert.equal(showsMathPanels('smartFill'), true);
});
