/**
 * **The gesture router** (Task 10.1 RULE 2 / RULE 3).
 *
 * This file is the answer to "where does the *interaction logic* live?", and the
 * answer is: here, in one pure, DOM-free class — not in the React component and
 * not in the engine. The division is a rule of the project (Task 1.4: the UI is a
 * dumb remote; Task 10.1 RULE 1: no geometry maths outside the engine), and it
 * leaves exactly one job for the UI: decide **which engine call a pointer event
 * becomes**, and when a gesture is finished. That job is a state machine, so it
 * is written as one — with no canvas, no WASM and no React in sight, which is why
 * `tests/view-model.test.ts` can drive a whole pen path without a browser.
 *
 * The vocabulary it speaks is the engine's: `draw_pointer(tool, kind, x, y, alt,
 * close, time)` and the two commits. Every decision it makes is one of the
 * gestures the rules name:
 *
 * | rule | gesture | what the router does |
 * | --- | --- | --- |
 * | RULE 2 | click | `down` on the pen, `close: true` — the engine's `closes_at` says whether it closed a path or placed a corner |
 * | RULE 2 | click-drag | the same `down`, then `move`s: the engine builds symmetric handles from the drag |
 * | RULE 2 | Alt-drag | the same, with `alt: true` — the engine breaks symmetry |
 * | RULE 2 | Esc / Enter | `penCommit(close: false)` |
 * | RULE 3 | freehand | `down`/`move`/`up` on the brush; pressure travels with the point if the device has it |
 * | RULE 3 | hold-to-snap | a *timer*, not a gesture: see `quick-shape.ts` |
 *
 * ## Two things the router deliberately does *not* do
 *
 * **It does not decide whether a click closed a path.** `close: true` is an
 * *offer*, sent on every pen down-stroke; `PenSession::closes_at` measures the
 * click against the first anchor and can refuse. Only the engine knows where the
 * anchors are, so anything else would be the UI guessing at geometry.
 *
 * **It does not compute handle vectors.** The drag distance is reported
 * (`dragDistance`) because the *preview* wants to know whether to show handles,
 * but the handles themselves come back in the reply's draft. The Bézier maths is
 * `vectra_draw::pen`'s, and it is exercised by the Handle Symmetry and Handle
 * Independence laws.
 */

import type { DrawReplyWire } from '../wire';

/** The pointer phases the engine's `PointerKind` accepts. */
export type PointerPhase = 'down' | 'move' | 'up' | 'cancel';

/**
 * How far the pointer must travel (in CSS pixels) before a pen press counts as a
 * *drag* rather than a click. Three pixels: below the noise floor of a mouse and
 * above the jitter of a trackpad, so a click that wobbles still places a corner
 * and a deliberate drag always produces handles.
 */
export const DRAG_SLOP_PX = 3;

/** Which drawing tool a gesture belongs to (the engine's two-tool vocabulary). */
export type DrawTool = 'pen' | 'brush';

export interface Point {
  x: number;
  y: number;
}

/** One live gesture. */
export interface Gesture {
  tool: DrawTool;
  pointerId: number;
  /** Where the press happened, in document units. */
  origin: Point;
  /** Where the pointer was the last time the canvas saw it. */
  last: Point;
  /** Press position in *client* pixels — the slop test needs screen distance. */
  originClient: Point;
  /** Alt/Option held **at the press**. RULE 2 reads the modifier once, when the
   *  gesture starts: an Alt pressed halfway through a drag should not retroactively
   *  break a symmetry the user already committed to. */
  alt: boolean;
  /** Raw (client-space, relative to the canvas box) samples of the stroke. */
  samples: Array<{ x: number; y: number; pressure: number | null; clientX: number; clientY: number }>;
  /** True once the pointer has travelled `DRAG_SLOP_PX` from the press. */
  dragged: boolean;
}

/** Construction parameters for a new gesture. */
export interface PressEvent {
  tool: DrawTool;
  pointerId: number;
  point: Point;
  client: Point;
  alt: boolean;
  pressure: number | null;
}

/**
 * Begin a gesture.
 *
 * Note what is stored: the two coordinate systems are kept apart on purpose. The
 * *document* point is what the engine is told; the *client* point is what the
 * slop test is measured in, because "3 pixels" is a statement about a hand on a
 * device, not about a document. A camera that changes scale (Phase 2) then cannot
 * silently turn a click into a drag.
 */
export function beginGesture(press: PressEvent): Gesture {
  return {
    tool: press.tool,
    pointerId: press.pointerId,
    origin: { ...press.point },
    last: { ...press.point },
    originClient: { ...press.client },
    alt: press.alt,
    samples: [
      {
        x: press.point.x,
        y: press.point.y,
        pressure: press.pressure,
        clientX: press.client.x,
        clientY: press.client.y,
      },
    ],
    dragged: false,
  };
}

/**
 * Add a sample to a live gesture, returning the updated gesture.
 *
 * Returns a *new* object rather than mutating: React holds the gesture in state,
 * and a mutation in place is the classic way a pointer sample gets lost (an
 * unchanged reference does not re-render, so the canvas keeps drawing the old
 * stroke). It is also what makes this function testable from a loop.
 */
export function extendGesture(
  gesture: Gesture,
  point: Point,
  client: Point,
  pressure: number | null,
): Gesture {
  const dx = client.x - gesture.originClient.x;
  const dy = client.y - gesture.originClient.y;
  const dragged = gesture.dragged || Math.hypot(dx, dy) >= DRAG_SLOP_PX;
  return {
    ...gesture,
    last: { ...point },
    dragged,
    samples: [
      ...gesture.samples,
      {
        x: point.x,
        y: point.y,
        pressure,
        clientX: client.x,
        clientY: client.y,
      },
    ],
  };
}

/** The distance travelled since the press, in client pixels. */
export function dragDistance(gesture: Gesture, client: Point): number {
  return Math.hypot(client.x - gesture.originClient.x, client.y - gesture.originClient.y);
}

/**
 * Is this gesture a **drag** — the pen's "smooth point with symmetrical Bézier
 * handles" (RULE 2) rather than a click's corner?
 *
 * Kept as a function rather than a field read because it is the one judgement the
 * UI makes about geometry, and naming it makes the rule visible at the call site:
 * `if (isDrag(gesture))` reads as the rule, not as a distance comparison.
 */
export function isDrag(gesture: Gesture | null): boolean {
  return gesture !== null && gesture.dragged;
}

/**
 * **A pointer event → the engine call it becomes.**
 *
 * The engine offers exactly one door for gestures (`draw_pointer`, plus the two
 * commits), so this is the whole routing table:
 *
 * * `down` — the gesture begins. The pen is offered `close: true` (see the module
 *   comment); the brush sends the first sample.
 * * `move` — the point is forwarded. The engine decides what it means: a pen
 *   move builds or moves handles, a brush move appends a filtered sample.
 * * `up` — the gesture ends: the pen places an anchor (or a smooth point if it
 *   dragged), and the brush's stroke is finite, so the UI asks for the commit.
 * * `cancel` — the pointer was lost (a browser gesture, a window blur): the
 *   session is reset instead of committing a half-drawn path.
 */
export type DrawIntent =
  | {
      call: 'pointer';
      tool: DrawTool;
      kind: PointerPhase;
      x: number;
      y: number;
      alt: boolean;
      close: boolean;
      time: number;
    }
  | { call: 'penCommit'; close: boolean }
  | { call: 'brushCommit' }
  | { call: 'none'; reason: string };

/** What the pen's `up` means: the path ends here (a click), or a segment was drawn. */
export function penUpIntent(): DrawIntent {
  return { call: 'pointer', tool: 'pen', kind: 'up', x: 0, y: 0, alt: false, close: false, time: 0 };
}

/**
 * The engine call for a press, in document units.
 *
 * `close` is `true` on every pen press and the engine refuses it when the click
 * is not near the first anchor — the offer/decision split described at the top of
 * this file.
 */
export function downIntent(gesture: Gesture, time: number): DrawIntent {
  return {
    call: 'pointer',
    tool: gesture.tool,
    kind: 'down',
    x: gesture.origin.x,
    y: gesture.origin.y,
    alt: gesture.alt,
    close: gesture.tool === 'pen',
    time,
  };
}

export function moveIntent(gesture: Gesture, time: number): DrawIntent {
  return {
    call: 'pointer',
    tool: gesture.tool,
    kind: 'move',
    x: gesture.last.x,
    y: gesture.last.y,
    alt: gesture.alt,
    close: false,
    time,
  };
}

/**
 * The engine call for a release.
 *
 * The brush commits here — a brush stroke is finished when the hand lifts, which
 * is the whole difference between the two tools: the pen accumulates anchors until
 * the user says stop, the brush is one gesture from press to release.
 */
export function upIntent(gesture: Gesture, time: number): DrawIntent {
  if (gesture.tool === 'brush') return { call: 'brushCommit' };
  return {
    call: 'pointer',
    tool: 'pen',
    kind: 'up',
    x: gesture.last.x,
    y: gesture.last.y,
    alt: gesture.alt,
    close: false,
    time,
  };
}

export function cancelIntent(gesture: Gesture, time: number): DrawIntent {
  return {
    call: 'pointer',
    tool: gesture.tool,
    kind: 'cancel',
    x: gesture.last.x,
    y: gesture.last.y,
    alt: gesture.alt,
    close: false,
    time,
  };
}

/**
 * **Escape and Enter finish the path** (RULE 2), and neither closes it: a
 * designer pressing Enter is saying "that is the shape I want", and a path that
 * silently joined its ends would be a different shape. Closing is its own
 * gesture — clicking the first anchor.
 */
export function finishIntent(): DrawIntent {
  return { call: 'penCommit', close: false };
}

/**
 * The stroke the Quick Shape recogniser should look at, from a live gesture:
 * the raw document samples without their pressure channel. The recogniser is
 * engine code and takes plain points, and the *rough* stroke is the right input —
 * snapping must see what the hand did, not a tidied-up version of it.
 */
export function strokePoints(gesture: Gesture): [number, number][] {
  return gesture.samples.map((sample) => [sample.x, sample.y]);
}

/**
 * Should the pen's preview show Bézier handles after this reply?
 *
 * A freshly placed anchor has handles only if the press dragged (or if the engine
 * built them for another reason), and the honest answer is to ask the reply:
 * `draft.anchors` carries whatever the engine actually stored. This helper exists
 * so the canvas does not invent a rule of its own.
 */
export function replyHasHandles(reply: DrawReplyWire | null): boolean {
  if (!reply?.draft) return false;
  return reply.draft.anchors.some(
    (anchor) => anchor.handle_in !== null || anchor.handle_out !== null,
  );
}
