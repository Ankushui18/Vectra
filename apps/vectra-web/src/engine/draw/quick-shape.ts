/**
 * **Quick Shape: the hold** (Task 10.1 RULE 3).
 *
 * RULE 3's magic moment is a *pause*: the user draws a rough circle, stops moving
 * with the button still down, and the blob becomes a perfect circle. Two things
 * have to be true for that to feel like a tool rather than a glitch:
 *
 * 1. the stroke must actually *be* a primitive — that judgement is the engine's
 *    (`vectra_draw::stroke::is_snap_candidate` and `quickshape::recognize`), and
 *    the reply carries it as `snap_candidate` so the UI never guesses;
 * 2. the user must have stopped — that is *timing*, which is the UI's business,
 *    because only the UI has a clock.
 *
 * So this file owns exactly one policy: **how long is "hold still"**, and what
 * counts as still. It is a pure state machine over timestamps (`performance.now()`
 * values are passed in, never read), which is why the hold can be tested without
 * a browser — and why the constant below is a *decision* someone can disagree
 * with in a code review instead of a magic number buried in a pointer handler.
 *
 * ## Why 260 ms, and why stillness is measured in pixels
 *
 * A hold has to be long enough not to fire on the natural pause at the end of a
 * stroke (people slow down before lifting) and short enough not to feel like a
 * wait. 260 ms sits between a hand's deceleration and a deliberate pause, and it
 * is the number Procreate's own "hold to snap" behaviour lands near. The
 * stillness radius is in **client pixels** — 4 of them — because "did they stop
 * or are they still shaping?" is a question about the hand on a device, not about
 * document units; a zoomed-in document must not make the snap harder to trigger.
 *
 * The hold is *armed* only while the engine says the stroke is a candidate, and
 * *disarmed* the moment the pointer moves more than the radius: a designer who
 * keeps drawing has not finished their shape and must never be interrupted.
 */

import type { DrawReplyWire } from '../wire';

/** How long the pointer must be still, in milliseconds, before the snap fires. */
export const SNAP_HOLD_MS = 260;

/** How far the pointer may wander (client pixels) and still count as still. */
export const SNAP_STILL_PX = 4;

/** Where the hold is in its life. */
export type HoldState = 'idle' | 'waiting' | 'fired';

/**
 * The hold clock.
 *
 * Usage, once per pointer move while the button is down:
 *
 * ```ts
 * const hold = new SnapHold();
 * hold.move(now, client);                     // the hand moved
 * hold.observe(reply.snap_candidate ?? false); // …and the engine's verdict
 * if (hold.tick(now)) requestSnap();           // the pause matured
 * ```
 *
 * `observe` is deliberately separate from `move`: the candidate flag arrives in
 * an *engine reply* (asynchronously, after the move was dispatched), so binding
 * it to the movement would make the timer's start depend on network-shaped
 * timing. The session forwards both, and only `tick` fires an intent.
 */
export class SnapHold {
  private candidate = false;
  private anchor: { time: number; client: { x: number; y: number } } | null = null;
  private state: HoldState = 'idle';

  /** Reset for a new gesture. */
  reset(): void {
    this.candidate = false;
    this.anchor = null;
    this.state = 'idle';
  }

  /** The engine's verdict on the stroke drawn so far. */
  observe(candidate: boolean): void {
    this.candidate = candidate;
    if (!candidate) this.anchor = null;
  }

  /**
   * The pointer moved. Movement within `SNAP_STILL_PX` of the **last anchor**
   * keeps the clock running; anything more re-anchors it (or, if the stroke is no
   * longer a candidate, clears it entirely).
   */
  move(time: number, client: { x: number; y: number }): void {
    if (!this.candidate) {
      this.anchor = null;
      this.state = 'idle';
      return;
    }
    if (this.anchor === null) {
      this.anchor = { time, client: { ...client } };
      this.state = 'waiting';
      return;
    }
    const drift = Math.hypot(client.x - this.anchor.client.x, client.y - this.anchor.client.y);
    if (drift > SNAP_STILL_PX) {
      this.anchor = { time, client: { ...client } };
      this.state = 'waiting';
    }
  }

  /**
   * Has the pause matured? Called on a timer (the session schedules one for
   * `SNAP_HOLD_MS` after each move), and answers `true` **once**: a hold fires a
   * single snap, and the session resets afterwards.
   */
  tick(time: number): boolean {
    if (this.state === 'fired' || this.anchor === null || !this.candidate) return false;
    if (time - this.anchor.time < SNAP_HOLD_MS) return false;
    this.state = 'fired';
    return true;
  }

  /** Milliseconds still to wait, or `null` when nothing is pending. */
  remaining(time: number): number | null {
    if (this.anchor === null || !this.candidate || this.state === 'fired') return null;
    return Math.max(0, SNAP_HOLD_MS - (time - this.anchor.time));
  }

  get holdState(): HoldState {
    return this.state;
  }

  get armed(): boolean {
    return this.anchor !== null && this.candidate && this.state === 'waiting';
  }
}

/**
 * The stroke to hand the engine for a snap: the gesture's samples as plain
 * `[x, y]` document points.
 *
 * A shortcut for the pen case too — a pen path's "rough input" is the anchors the
 * user placed, which is what `strokeFromAnchors` returns.
 */
export function strokeFromSamples(samples: Array<{ x: number; y: number }>): [number, number][] {
  return samples.map((sample) => [sample.x, sample.y]);
}

/** The rough stroke of a pen path: where its anchors are. */
export function strokeFromAnchors(anchors: Array<{ x: number; y: number }>): [number, number][] {
  return anchors.map((anchor) => [anchor.x, anchor.y]);
}

/**
 * The sentence the canvas shows while a hold is pending — RULE 4's "show only
 * the path being drawn", so the hint is a whisper, not a dialog: it appears while
 * the stroke *could* snap and its length is the wait that remains.
 */
export function holdHint(hold: SnapHold, time: number): string | null {
  const remaining = hold.remaining(time);
  if (remaining === null) return null;
  return `hold to snap · ${Math.ceil(remaining)} ms`;
}

/**
 * What to say after a snap (or a refusal).
 *
 * A refusal is worth saying out loud — "the stroke is not a circle or a
 * rectangle" — because the alternative is a designer holding still forever
 * wondering why nothing happens. The error is the *input's* distance from the
 * primitive it was recognised as, which is the honest number: the snapped path
 * is exact, the hand was not.
 */
export function snapSummary(reply: DrawReplyWire): { snapped: boolean; text: string } {
  if (reply.snapped) {
    const error =
      typeof reply.snap_error === 'number' ? ` (±${reply.snap_error.toFixed(1)} units)` : '';
    return { snapped: true, text: `snapped a ${reply.snapped}${error}` };
  }
  return { snapped: false, text: reply.error ?? 'the stroke did not look like a circle or a rectangle' };
}
