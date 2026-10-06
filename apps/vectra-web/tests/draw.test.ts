/**
 * The drawing suite's UI laws (Task 10.1).
 *
 * Everything here runs without a DOM, a canvas or a GPU, and that is the point of
 * the split the task describes: the *interaction logic* (which engine call a
 * pointer becomes, when a hold fires, what the overlay shows) lives in
 * `src/engine/draw/*` as pure functions and one small state machine, so it can be
 * tested like arithmetic. What cannot be tested here — that a real stroke becomes
 * a real `Path` — is tested in Rust, where the mathematics lives
 * (`crates/vectra-draw/tests/draw_laws.rs`, `crates/vectra-wasm/tests/draw_laws.rs`).
 *
 * The four laws, named after the rules they defend:
 *
 * 1. **Rule 2 — the pen's gestures.** A click is a corner, a drag is a smooth
 *    point, Alt breaks the symmetry, Enter finishes and never closes.
 * 2. **Rule 3 — the hold.** The snap fires on a pause, and only on a pause; it
 *    never fires while the hand is still moving, and never fires twice.
 * 3. **Rule 4 — the gate.** A drawing tool hides the math; the select tool does
 *    not; and while drawing, the overlay shows only cursor, path and handles.
 * 4. **The dumb-remote rule.** Every pointer event becomes exactly one engine
 *    call, with the arguments the pointer produced — the UI decides nothing about
 *    geometry.
 */

import assert from 'node:assert/strict';
import { test } from 'node:test';
import {
  DRAG_SLOP_PX,
  beginGesture,
  cancelIntent,
  downIntent,
  extendGesture,
  finishIntent,
  isDrag,
  moveIntent,
  replyHasHandles,
  strokePoints,
  upIntent,
} from '../src/engine/draw/pointer';
import {
  SNAP_HOLD_MS,
  SNAP_STILL_PX,
  SnapHold,
  holdHint,
  snapSummary,
  strokeFromAnchors,
  strokeFromSamples,
} from '../src/engine/draw/quick-shape';
import {
  anchorLabel,
  editTarget,
  overlayModel,
  parseHit,
  parseOverlay,
  placeOverlay,
  revealedHandles,
} from '../src/engine/draw/path-edit';
import { DrawSession, draftStatus, pointerCursor } from '../src/engine/draw/session';
import { DIRECT_TOOLS, TOOLS, designerHint, showsMathPanels, toolForShortcut } from '../src/engine/draw/tools';
import type { AnchorWire, DrawReplyWire } from '../src/engine/wire';

const anchor = (
  slot: string,
  x: number,
  y: number,
  handles: { in?: [number, number]; out?: [number, number]; source?: string } = {},
): AnchorWire => ({
  slot,
  x,
  y,
  handle_in: handles.in ?? null,
  handle_out: handles.out ?? null,
  source: handles.source ?? 'literal',
});

const press = (overrides: Partial<Parameters<typeof beginGesture>[0]> = {}) =>
  beginGesture({
    tool: 'pen',
    pointerId: 1,
    point: { x: 10, y: 20 },
    client: { x: 100, y: 200 },
    alt: false,
    pressure: null,
    ...overrides,
  });

// ── Rule 2: the pen ──────────────────────────────────────────────────────

test('pen: a click is a corner, a drag is a smooth point, and 3px is the line between them', () => {
  const click = press();
  assert.equal(isDrag(click), false, 'no movement yet');
  const nudged = extendGesture(click, { x: 10.1, y: 20.1 }, { x: 101, y: 201 }, null);
  assert.equal(isDrag(nudged), false, 'a hand wobble is not a drag');

  const dragged = extendGesture(
    click,
    { x: 40, y: 50 },
    { x: 100 + DRAG_SLOP_PX, y: 200 },
    null,
  );
  assert.equal(isDrag(dragged), true, 'exactly at the slop is a drag');
  assert.equal(isDrag(null), false);
});

test('pen: the down intent offers "close" and lets the engine refuse it', () => {
  // RULE 2's "click on the first point closes the path" is an *offer*: only the
  // engine knows where the first anchor is, so the UI sends the flag and the
  // engine's `closes_at` decides.
  const down = downIntent(press(), 1000);
  assert.equal(down.call, 'pointer');
  if (down.call !== 'pointer') return;
  assert.equal(down.close, true);
  assert.equal(down.kind, 'down');
  assert.deepEqual([down.x, down.y], [10, 20]);
  // The brush never offers to close: a stroke is one gesture, not a path.
  const brushDown = downIntent(press({ tool: 'brush' }), 1000);
  assert.equal(brushDown.call === 'pointer' && brushDown.close, false);
});

test('pen: Alt is read once, at the press — a modifier pressed mid-drag does not rewrite history', () => {
  const gesture = press({ alt: true });
  const moved = extendGesture(gesture, { x: 30, y: 30 }, { x: 160, y: 260 }, null);
  assert.equal(moved.alt, true, 'the gesture keeps the modifier it started with');
  const move = moveIntent(moved, 1234);
  assert.equal(move.call === 'pointer' && move.alt, true);
});

test('pen: Enter/Escape finish without closing; the brush commits on release', () => {
  const finish = finishIntent();
  assert.deepEqual(finish, { call: 'penCommit', close: false });

  const penUp = upIntent(press(), 2000);
  assert.equal(penUp.call, 'pointer');
  assert.equal(penUp.call === 'pointer' && penUp.kind, 'up');
  assert.equal(penUp.call === 'pointer' && penUp.close, false);

  const brushUp = upIntent(press({ tool: 'brush' }), 2000);
  assert.deepEqual(brushUp, { call: 'brushCommit' });

  const cancel = cancelIntent(press(), 2000);
  assert.equal(cancel.call === 'pointer' && cancel.kind, 'cancel');
});

test('pen: the draft is whatever the engine answered — the UI never builds handles', () => {
  const reply: DrawReplyWire = {
    ok: true,
    draft: {
      start: [0, 0],
      kinds: ['cubic'],
      points: [
        [0, 0],
        [10, 0],
      ],
      anchors: [
        anchor('start', 0, 0, { out: [10, 0] }),
        anchor('segments[0].to', 40, 0, { in: [30, 0] }),
      ],
      closed: false,
      snap_candidate: false,
    },
  };
  assert.equal(replyHasHandles(reply), true);
  assert.equal(replyHasHandles({ ok: true }), false);
  // The stroke the engine gets for a snap is the raw samples, pressure dropped.
  const gesture = extendGesture(press(), { x: 12, y: 4 }, { x: 130, y: 210 }, 0.7);
  assert.deepEqual(strokePoints(gesture), [
    [10, 20],
    [12, 4],
  ]);
});

// ── Rule 3: the hold ─────────────────────────────────────────────────────

test('quick shape: the hold fires on a pause and only on a pause', () => {
  const hold = new SnapHold();
  hold.observe(true);
  hold.move(0, { x: 0, y: 0 });
  assert.equal(hold.tick(SNAP_HOLD_MS - 1), false, 'not yet');
  assert.equal(hold.tick(SNAP_HOLD_MS), true, 'the pause matured');
  assert.equal(hold.tick(SNAP_HOLD_MS + 500), false, 'and it fires exactly once');

  const moving = new SnapHold();
  moving.observe(true);
  moving.move(0, { x: 0, y: 0 });
  moving.move(SNAP_HOLD_MS - 10, { x: SNAP_STILL_PX + 1, y: 0 });
  assert.equal(moving.tick(SNAP_HOLD_MS + 5), false, 'the hand was still moving');

  const refused = new SnapHold();
  refused.observe(false);
  refused.move(0, { x: 0, y: 0 });
  assert.equal(refused.tick(SNAP_HOLD_MS * 10), false, 'the engine says it is not a shape');
});

test('quick shape: the hint counts down, and a refusal is said out loud', () => {
  const hold = new SnapHold();
  hold.observe(true);
  hold.move(0, { x: 0, y: 0 });
  assert.equal(holdHint(hold, 0), `hold to snap · ${SNAP_HOLD_MS} ms`);
  assert.equal(holdHint(hold, SNAP_HOLD_MS), 'hold to snap · 0 ms');
  assert.equal(holdHint(hold, SNAP_HOLD_MS + 1), 'hold to snap · 0 ms');

  const snapped = snapSummary({ ok: true, snapped: 'circle', snap_error: 3.14159 });
  assert.equal(snapped.snapped, true);
  assert.equal(snapped.text, 'snapped a circle (±3.1 units)');

  const refused = snapSummary({ ok: false, error: 'the stroke is not a circle or a rectangle' });
  assert.equal(refused.snapped, false);
  assert.match(refused.text, /not a circle/);
});

test('quick shape: the stroke that is recognised is the hand’s, not a tidied version', () => {
  assert.deepEqual(
    strokeFromSamples([
      { x: 1, y: 2 },
      { x: 3, y: 4 },
    ]),
    [
      [1, 2],
      [3, 4],
    ],
  );
  assert.deepEqual(strokeFromAnchors([{ x: 5, y: 6 }]), [[5, 6]]);
});

// ── Rule 4: the gate, and the overlay ────────────────────────────────────

test('RULE 4: a drawing tool hides the math, and Select keeps it', () => {
  assert.equal(showsMathPanels('select'), true, 'the parametric tool keeps its panels');
  for (const tool of DIRECT_TOOLS) {
    assert.equal(showsMathPanels(tool), false, `${tool} must hide the math`);
  }
  assert.deepEqual(DIRECT_TOOLS, ['direct', 'pen', 'brush']);
  // Every tool has a hint, and the hints are sentences rather than names.
  for (const spec of TOOLS) {
    assert.ok(designerHint(spec.id).length > 20, spec.id);
    assert.equal(toolForShortcut(spec.shortcut), spec.id);
  }
  assert.equal(toolForShortcut('z'), null, 'no tool on Z');
  assert.equal(toolForShortcut('v'), 'select', 'shortcuts are case-insensitive');
});

test('RULE 4: handles are revealed by the vertex that owns them, not all at once', () => {
  const anchors = [
    anchor('start', 0, 0, { out: [10, 0] }),
    anchor('segments[0].to', 40, 0, { in: [30, 0], out: [50, 0] }),
    anchor('segments[1].to', 80, 0, { in: [70, 0] }),
  ];
  assert.deepEqual(revealedHandles(anchors, null, null), [], 'nothing is chosen: no handles');
  assert.deepEqual(revealedHandles(anchors, 'segments[0].to', null), ['segments[0].to']);
  assert.deepEqual(revealedHandles(anchors, null, 'start'), ['start']);
  assert.deepEqual(revealedHandles(anchors, 'start', 'segments[1].to'), ['start', 'segments[1].to']);
});

test('RULE 4: the overlay is engine geometry only — cursor, path and handles', () => {
  const anchors = [
    anchor('start', 0, 0, { out: [10, 0] }),
    anchor('segments[0].to', 40, 0, { in: [30, 0] }),
  ];
  const model = overlayModel({
    anchors,
    revealed: ['start'],
    selectedSlot: 'start',
    draft: { points: [[40, 0], [50, 10]], start: [40, 0] },
    samples: undefined,
    cursor: { x: 60, y: 20 },
  });
  const roles = model.points.map((point) => point.role);
  assert.deepEqual(roles, ['anchor', 'handle', 'anchor', 'draft', 'draft', 'cursor']);
  // Two anchors + one revealed handle + two draft points + the cursor.
  assert.equal(model.shape.length, model.points.length);
  // One handle line, drawn from the anchor to its control point.
  const handleLine = model.lines.find((line) => line.role === 'handle');
  assert.deepEqual(handleLine?.points, [
    [0, 0],
    [10, 0],
  ]);

  // No draft, no samples, no cursor: nothing but the path's own vertices.
  const bare = overlayModel({ anchors, revealed: [], selectedSlot: null });
  assert.deepEqual(
    bare.points.map((p) => p.role),
    ['anchor', 'anchor'],
  );
  assert.equal(bare.lines.length, 0);
});

test('RULE 4: the overlay is placed by the camera, and stays empty without one', () => {
  const model = overlayModel({
    anchors: [anchor('start', 0, 0)],
    revealed: [],
    selectedSlot: null,
  });
  const placed = placeOverlay(model, [[100, 200]]);
  assert.equal(placed.points.length, 1);
  assert.deepEqual([placed.points[0].cx, placed.points[0].cy], [100, 200]);
  // A renderer with no canvas box answers with an empty list: an overlay that
  // cannot be placed is *empty*, never drawn in the wrong place.
  assert.deepEqual(placeOverlay(model, []), { points: [], lines: [] });
});

test('RULE 4: a hit is a slot and a side, and the drag target follows from it', () => {
  const miss = parseHit({ kind: 'miss' });
  assert.equal(miss.kind, 'miss');
  assert.equal(editTarget(miss), null, 'empty space is not an edit');

  const anchorHit = parseHit({
    kind: 'anchor',
    slot: 'segments[1].to',
    side: null,
    index: 1,
    distance: 2,
    x: 5,
    y: 6,
  });
  assert.deepEqual(editTarget(anchorHit), {
    slot: 'segments[1].to',
    side: '',
    anchor: true,
  });

  const handleHit = parseHit({
    kind: 'handle',
    slot: 'segments[2].control1',
    side: 'out',
    index: 2,
    distance: 3,
    x: 7,
    y: 8,
  });
  assert.deepEqual(editTarget(handleHit), {
    slot: 'segments[2].control1',
    side: 'out',
    anchor: false,
  });

  // An empty overlay is an empty list, never a throw.
  assert.deepEqual(parseOverlay({ ok: false, anchors: [] }), []);
});

test('RULE 4: vertex labels are the designer’s, and a driven vertex says so', () => {
  const anchors = [
    anchor('start', 0, 0),
    anchor('segments[0].to', 40, 0, { source: 'expression' }),
  ];
  assert.equal(anchorLabel(anchors, 'start'), 'anchor 1 of 2');
  assert.equal(anchorLabel(anchors, 'segments[0].to'), 'expression-driven anchor 2 of 2');
  assert.equal(anchorLabel(anchors, null), 'no point selected');
  assert.equal(anchorLabel(anchors, 'segments[9].to'), 'no point selected');
});

// ── The session: the plan's named state ──────────────────────────────────

test('the session tracks the plan’s fields, and a commit hands the node to the tools', () => {
  const session = new DrawSession('pen');
  assert.equal(session.drawTool, 'pen');
  assert.equal(session.state.isDrawing, false);
  assert.equal(session.state.isAltPressed, false);
  assert.equal(session.state.isHoldingForSnap, false);
  assert.equal(session.state.activeHandle, null);

  session.press({
    tool: 'pen',
    pointerId: 7,
    point: { x: 1, y: 2 },
    client: { x: 10, y: 20 },
    alt: true,
    pressure: null,
  });
  assert.equal(session.state.isDrawing, true);
  assert.equal(session.state.isAltPressed, true);
  assert.equal(session.state.gesture?.pointerId, 7);

  session.absorbReply({
    ok: true,
    draft: {
      start: [1, 2],
      kinds: ['line'],
      points: [
        [1, 2],
        [3, 4],
      ],
      anchors: [anchor('start', 1, 2)],
      closed: false,
      snap_candidate: true,
    },
  });
  assert.equal(session.state.draft?.kinds.length, 1);
  assert.match(session.state.status, /hold to snap/);

  const committed = session.absorbCommit({ ok: true, node_id: 'node-1', segments: 4 });
  assert.equal(committed.isDrawing, false);
  assert.equal(committed.nodeId, 'node-1');
  assert.equal(committed.gesture, null);
  assert.match(committed.status, /path committed · 4 segments/);

  // A snap says what it did, in the engine's own measurement.
  const snapped = session.absorbCommit({ ok: true, node_id: 'n', snapped: 'circle', snap_error: 2.5 });
  assert.equal(snapped.snapNote, 'snapped a circle (±2.5 units)');
});

test('the session drops a selection the document no longer has', () => {
  const session = new DrawSession('direct');
  session.mutate({ selectedSlot: 'segments[2].to' });
  session.absorbOverlay('path-1', [anchor('start', 0, 0)]);
  assert.equal(session.state.selectedSlot, null, 'the vertex is gone: nothing is owned');
  // …and the selection comes back the moment the document has such a vertex
  // again (an undo, say): the session re-reads what the engine reports.
  session.mutate({ selectedSlot: 'segments[2].to' });
  session.absorbOverlay('path-1', [anchor('start', 0, 0), anchor('segments[2].to', 1, 1)]);
  assert.equal(session.state.selectedSlot, 'segments[2].to');
  assert.equal(session.state.nodeId, 'path-1');
  assert.equal(session.state.anchors.length, 2);
});

test('the tool cursor and status lines are the tool’s, and never the engine’s maths', () => {
  assert.equal(pointerCursor('select', false), 'default');
  assert.equal(pointerCursor('pen', false), 'crosshair');
  assert.equal(pointerCursor('pen', true), 'grabbing');
  assert.equal(draftStatus(0, false), 'drawing — click to place the first point');
  assert.equal(draftStatus(2, false), '✎ 2 segments · Enter to finish');
  assert.equal(draftStatus(1, true), '✎ 1 segment · hold to snap');
});
