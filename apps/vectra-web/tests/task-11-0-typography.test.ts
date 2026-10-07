/**
 * **Task 11.0's laws at the UI boundary** — the three typography rules as they
 * are *decided* on this side of the wasm boundary, with no browser.
 *
 * The engine owns shaping, layout, tangent sampling and outlining; the UI's job
 * is small and precise, which is exactly why it is testable:
 *
 * * **RULE 1 (parametric text)** — the Text panel is offered exactly when a run
 *   is selected and unlocked, the font picker always shows the family the
 *   *document* names (even when this host cannot resolve it, because silently
 *   rewriting a designer's family is worse than showing it substituted), and
 *   every number the panel sends is a `SetParameter` write — the same command a
 *   rectangle's width gets.
 * * **RULE 2 (text on path)** — the offset slider's track is the engine's own
 *   arc length, its mapping is invertible, and a bound run is *not* draggable
 *   (its origin is the path's, not its own).
 * * **RULE 3 (outline)** — the wire builder sends the engine's letterform plans
 *   unchanged, the Outline button is disabled when there is nothing to outline,
 *   and the panel's own "bind" gesture produces the `BindTextToPath` a designer
 *   would expect.
 *
 * Run: `npm run test:ui`.
 */
import assert from 'node:assert/strict';
import { test } from 'node:test';
import {
  ALIGNMENT_GLYPH,
  TEXT_ALIGNMENTS,
  boundTrack,
  fontChoices,
  fontLabel,
  isParametricSource,
  offsetFromSlider,
  showsTextPanel,
  sliderFromOffset,
} from '../src/engine/panels';
import {
  bindTextToPath,
  createText,
  outlineTextPlan,
  setFontFamily,
  setText,
  setTextAlignment,
} from '../src/engine/commands';
import { TOOLS, isDirectTool, toolCursor, toolForShortcut } from '../src/engine/draw/tools';
import type { SnapshotNodeWire, SnapshotWire } from '../src/engine/wire';

// ── Fixtures (shaped exactly like the engine's snapshot JSON) ────────────

const TEXT_NODE: SnapshotNodeWire = {
  id: 'text-1',
  name: 'text',
  primitive: {
    type: 'text',
    d: 'M0 0L1 1Z',
    glyphs: 4,
    width: 96,
    height: 32,
    lines: 1,
  },
  text: {
    text: 'logo',
    font_family: 'Vectra Sans',
    alignment: 'left',
    font_size: 32,
    font_size_source: 'variable:scale',
    letter_spacing: 0,
    letter_spacing_source: 'literal',
    line_height: 1.2,
    line_height_source: 'literal',
  },
  style: { appearances: [], opacity: 1, blend: 'normal' },
  visible: true,
  locked: false,
  own_visible: true,
  own_locked: false,
} as unknown as SnapshotNodeWire;

const BOUND_NODE: SnapshotNodeWire = {
  ...TEXT_NODE,
  primitive: { ...TEXT_NODE.primitive, width: 628.32 },
  text: { ...TEXT_NODE.text!, bound_to: 'circle-1', offset: 40, offset_source: 'literal' },
} as unknown as SnapshotNodeWire;

function scene(nodes: Record<string, SnapshotNodeWire>): SnapshotWire {
  return { scene: { nodes, fonts: ['Vectra Sans'], z_order: Object.keys(nodes) } } as unknown as SnapshotWire;
}

// ── RULE 1: the panel's decisions ────────────────────────────────────────

test('RULE 1: the Text panel is for one unlocked run and nothing else', () => {
  const snapshot = scene({ 'text-1': TEXT_NODE });
  assert.equal(showsTextPanel(snapshot, ['text-1']), true);

  // Two selected: the panel would be editing "which one?".
  assert.equal(showsTextPanel(snapshot, ['text-1', 'text-1']), false);
  // Nothing selected: nothing to edit.
  assert.equal(showsTextPanel(snapshot, []), false);
  // A locked run cannot be edited, so a live panel over it would be a lie.
  assert.equal(
    showsTextPanel(scene({ 'text-1': { ...TEXT_NODE, locked: true } }), ['text-1']),
    false,
  );
  // A shape is not a run.
  const rect = {
    ...TEXT_NODE,
    primitive: { type: 'rect', x: 0, y: 0, w: 10, h: 10, corner_radius: 0 },
    text: undefined,
  } as unknown as SnapshotNodeWire;
  assert.equal(showsTextPanel(scene({ r: rect }), ['r']), false);
  assert.equal(showsTextPanel(null, ['text-1']), false);
});

test('RULE 1: the picker offers the engine’s families and never loses the document’s', () => {
  // The engine's list is the options…
  assert.deepEqual(fontChoices(['Vectra Sans', 'Mono'], 'Mono'), ['Vectra Sans', 'Mono']);
  // …and a family this host cannot resolve is shown *first*, so the select
  // displays what the document says instead of silently switching it.
  assert.deepEqual(fontChoices(['Vectra Sans'], 'Baskerville'), ['Baskerville', 'Vectra Sans']);
  // No double entry when it is already there.
  assert.deepEqual(fontChoices(['Vectra Sans'], 'Vectra Sans'), ['Vectra Sans']);
  assert.equal(fontLabel('Baskerville', ['Vectra Sans']), 'Baskerville (substituted)');
  assert.equal(fontLabel('Vectra Sans', ['Vectra Sans']), 'Vectra Sans');
});

test('RULE 1: the panel can tell a parametric slot from a typed one', () => {
  assert.equal(isParametricSource('variable:scale'), true);
  assert.equal(isParametricSource('expression:e1'), true);
  assert.equal(isParametricSource('procedural:p:scalar'), true);
  // `literal` and a missing tag both mean "nothing is driving this".
  assert.equal(isParametricSource('literal'), false);
  assert.equal(isParametricSource(undefined), false);
});

test('RULE 1: the panel writes a run through the ordinary parameter command', () => {
  const created = createText({ x: 10, y: 20, name: 'headline' });
  assert.equal(created.type, 'CreateNode');
  const kind = (
    created as unknown as {
      kind: {
        Text: {
          text: string;
          font_family: string;
          font_size: unknown;
          letter_spacing: unknown;
          line_height: unknown;
          alignment: string;
          x: unknown;
          y: unknown;
        };
      };
    }
  ).kind.Text;
  assert.equal(kind.text, '');
  assert.equal(kind.font_family, 'Vectra Sans');
  assert.equal(kind.alignment, 'left');
  // Every number is a **parameter**, not a bare number: the wire's shape is
  // explicit so a variable or an expression can be written into the same slot
  // later, which is what makes RULE 1 parametric rather than merely numeric.
  assert.deepEqual(kind.font_size, { Literal: 32 });
  assert.deepEqual(kind.letter_spacing, { Literal: 0 });
  assert.deepEqual(kind.line_height, { Literal: 1.2 });
  assert.deepEqual(kind.x, { Literal: 10 });
  assert.deepEqual(kind.y, { Literal: 20 });

  assert.deepEqual(setText('t', 'new words'), {
    type: 'SetText',
    node_id: 't',
    text: 'new words',
  });
  assert.deepEqual(setFontFamily('t', 'Mono'), {
    type: 'SetFontFamily',
    node_id: 't',
    family: 'Mono',
  });
  // The wire spelling is the document's own tag (`TextAlign`'s lowercase serde
  // form), which is also what the snapshot reports back — one spelling end to
  // end, so a panel can round-trip an alignment it never wrote.
  assert.deepEqual(setTextAlignment('t', 'center'), {
    type: 'SetTextAlignment',
    node_id: 't',
    alignment: 'center',
  });
  assert.deepEqual(bindTextToPath('t', 'c', { Literal: 12 }), {
    type: 'BindTextToPath',
    node_id: 't',
    path: 'c',
    offset: { Literal: 12 },
  });
});

test('RULE 1: the alignment toggle shows all three and nothing else', () => {
  assert.deepEqual([...TEXT_ALIGNMENTS], ['left', 'center', 'right']);
  for (const alignment of TEXT_ALIGNMENTS) {
    assert.equal(typeof ALIGNMENT_GLYPH[alignment], 'string');
    assert.ok(ALIGNMENT_GLYPH[alignment].length > 0);
  }
});

// ── RULE 2: the offset slider ────────────────────────────────────────────

test('RULE 2: the slider’s track is the engine’s own arc length', () => {
  assert.equal(boundTrack(BOUND_NODE), 628.32);
  // A run that follows nothing has no track to draw…
  assert.equal(boundTrack(TEXT_NODE), 0);
  assert.equal(boundTrack(null), 0);
  // …and neither has a shape.
  const rect = {
    ...TEXT_NODE,
    primitive: { type: 'rect', x: 0, y: 0, w: 1, h: 1, corner_radius: 0 },
    text: undefined,
  } as unknown as SnapshotNodeWire;
  assert.equal(boundTrack(rect), 0);
});

test('RULE 2: the slider maps both ways, over the whole path and no further', () => {
  const length = 628.32;
  // The ends of the track are the ends of the path.
  assert.ok(Math.abs(offsetFromSlider(0, length) + Math.round(length * 10) / 10) < 1e-9);
  assert.ok(Math.abs(offsetFromSlider(1, length) - Math.round(length * 10) / 10) < 1e-9);
  // The middle is zero — the run sits where it was authored.
  assert.equal(offsetFromSlider(0.5, length), 0);
  // Round-trip: a slider position survives a trip through the offset and back.
  for (const fraction of [0, 0.1, 0.25, 0.5, 0.75, 0.9, 1]) {
    const offset = offsetFromSlider(fraction, length);
    assert.ok(Math.abs(sliderFromOffset(offset, length) - fraction) < 0.01);
  }
  // Values are snapped: a pointer is not a micrometer.
  assert.equal(offsetFromSlider(0.50001, 1000), 0);
  assert.equal(offsetFromSlider(0.6, 1000), 200);
  // A missing length (nothing bound yet) still spans a usable track.
  assert.equal(offsetFromSlider(0.5, 0), 0);
  assert.ok(offsetFromSlider(1, 0) > 0);
  // Out-of-range fractions clamp instead of producing nonsense.
  assert.ok(Math.abs(offsetFromSlider(-5, length) + Math.round(length * 10) / 10) < 1e-9);
  assert.ok(Math.abs(offsetFromSlider(5, length) - Math.round(length * 10) / 10) < 1e-9);
  assert.equal(sliderFromOffset(-10_000, length), 0);
  assert.equal(sliderFromOffset(10_000, length), 1);
});

// ── RULE 3: the outline's wire shape ─────────────────────────────────────

test('RULE 3: outlining sends the engine’s letterforms unchanged', () => {
  const plan = outlineTextPlan({
    nodeId: 'text-1',
    groupId: 'group-1',
    name: 'logo (outlined)',
    paths: [
      {
        id: 'p-1',
        name: 'l',
        start: { Literal: { x: 0, y: 0 } },
        segments: [
          { Line: { to: { Literal: { x: 4, y: 0 } } } },
          { Line: { to: { Literal: { x: 4, y: 9 } } } },
          'Close',
        ],
      },
    ],
  });
  assert.equal(plan.type, 'OutlineText');
  const wire = plan as unknown as {
    node_id: string;
    group_id: string;
    name: string;
    paths: { id: string; name: string }[];
  };
  assert.equal(wire.node_id, 'text-1');
  assert.equal(wire.group_id, 'group-1');
  assert.equal(wire.name, 'logo (outlined)');
  assert.equal(wire.paths.length, 1);
  assert.equal(wire.paths[0].id, 'p-1');
  // The letterform is *named after the character it draws*, which is what makes
  // the layers panel read `l` instead of a uuid.
  assert.equal(wire.paths[0].name, 'l');
});

// ── The Text tool (RULE 1's canvas half) ─────────────────────────────────

test('the Text tool is bound to T, is no drawing tool, and has a text cursor', () => {
  const text = TOOLS.find((tool) => tool.id === 'text');
  assert.ok(text, 'the palette must offer a Text tool');
  assert.equal(text.shortcut, 'T');
  assert.equal(toolForShortcut('t'), 'text');
  assert.equal(toolForShortcut('T'), 'text');
  // RULE 4 of Task 11.0's brief: typing is not drawing, so the parametric panels
  // (and the Text panel itself) stay on screen.
  assert.equal(isDirectTool('text'), false);
  assert.equal(toolCursor('text'), 'text');
  // The palette's letters stay unique — `T` must not have been taken twice.
  const shortcuts = TOOLS.map((tool) => tool.shortcut);
  assert.equal(new Set(shortcuts).size, shortcuts.length);
});
