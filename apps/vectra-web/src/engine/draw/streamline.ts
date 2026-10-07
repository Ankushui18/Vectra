/**
 * **StreamLine — the smooth hand** (Task 10.7 RULE 2).
 *
 * The rule, in the brief's words: *never draw the point immediately*. A hand
 * shakes; a tablet's digitiser reports the shake faithfully; so the stroke the
 * engine receives is the hand plus the noise. StreamLine is the filter between
 * the two, and it is deliberately **not** in the engine: it is the *input* being
 * tidied, so it lives on the input side, in front of `draw_pointer`, where the
 * `PenTool` and the `BrushStrokeBuilder` (`vectra_draw::pen` / `stroke`) cannot
 * see anything else. The engine keeps drawing the stream it is given, exactly as
 * before — which is why this feature needs no command, no document change and no
 * wasm rebuild.
 *
 * ## The mathematics
 *
 * Two stages, in this order, per sample:
 *
 * ```text
 * 1. weighted moving average over the last w samples   (the smoothing)
 *        p̄ = Σ wᵢ·pᵢ / Σ wᵢ ,  wᵢ = 1/(1+i),  i = 0 newest      (a "pull")
 * 2. a lag cap — the smoothed cursor may not fall more than
 *    L = MAX_LAG · s pixels behind the hand              (the "lag")
 * ```
 *
 * The weights `1/(1+i)` are the classic pull profile: the newest sample counts
 * for as much as the previous two combined, so the filtered cursor chases the
 * hand instead of floating away from it, and the window is short enough (`w ≤ 6`)
 * that a deliberate corner is still a corner — the filter softens the *shiver*,
 * not the intent.
 *
 * Without stage 2 a moving average lags without bound on a fast stroke (the
 * average of "where you were" is where you no longer are), which is why
 * StreamLine's lag is clamped: `L = STREAMLINE_MAX_LAG_PX · s` grows linearly
 * with the slider, so **0 % is byte-identical to raw input** (window 1, cap 0)
 * and 100 % is the maximum pull the brief allows.
 *
 * ## The slider
 *
 * `s ∈ [0, 1]`, from a 0–100 % slider. The three knobs it drives are all
 * functions of `s` — window, weights and cap — so there is exactly one number in
 * the UI and exactly one number here, and the StreamLine Law can state the
 * monotonicity property: *more streamline is never less smooth*.
 *
 * ## The tail
 *
 * A lagged cursor stops short: the hand lifts, the smoothed point is still
 * `≤ L` pixels behind. {@link StreamLine.flush} hands back the closing samples —
 * a straight interpolation from where the filter was to where the hand actually
 * stopped — so the stroke ends where the designer's hand ended while the body of
 * the stroke keeps its smoothing. A straight join cannot add jitter: it
 * contributes no new direction changes.
 */

import type { Point } from './pointer';

/** The window at 100 % — `1 + round(s · (MAX_WINDOW - 1))` samples. */
export const STREAMLINE_MAX_WINDOW = 6;

/** How far the smoothed cursor may trail the hand at 100 %, in document units. */
export const STREAMLINE_MAX_LAG_PX = 28;

/** Below this the tail is the same point and is not worth a sample. */
const TAIL_EPSILON = 0.01;

/** The slider's 0–100 → the filter's 0–1, clamped. */
export function streamlineAmount(percent: number): number {
  if (!Number.isFinite(percent)) return 0;
  return Math.min(1, Math.max(0, percent / 100));
}

/**
 * **The filter.** One instance per stroke: the history is the stroke's, and a
 * filter that carried samples between strokes would smooth a line into the
 * previous one.
 */
export class StreamLine {
  private readonly amount: number;
  /** The recent raw samples, oldest first, capped at {@link window}. */
  private history: Array<Point> = [];

  constructor(amount: number) {
    this.amount = Math.min(1, Math.max(0, Number.isFinite(amount) ? amount : 0));
  }

  /** The averaging window this amount selects. 0 % → 1 (no averaging at all). */
  get window(): number {
    return 1 + Math.round(this.amount * (STREAMLINE_MAX_WINDOW - 1));
  }

  /** How far behind the hand the filtered cursor may trail, in document units. */
  get lagCap(): number {
    return this.amount * STREAMLINE_MAX_LAG_PX;
  }

  /** The amount, for the tests and the status line. */
  get strength(): number {
    return this.amount;
  }

  /**
   * **Feed one raw sample; take back the point to draw.**
   *
   * At `amount === 0` this is the identity — the same numbers, not a copy with a
   * rounded corner — which is the rule's "0 % = raw input" made literal.
   */
  push(point: Point): Point {
    if (this.amount === 0) return { x: point.x, y: point.y };
    this.history.push({ x: point.x, y: point.y });
    if (this.history.length > this.window) this.history.splice(0, this.history.length - this.window);

    let sx = 0;
    let sy = 0;
    let weight = 0;
    for (let i = 0; i < this.history.length; i += 1) {
      // i counts back from the newest: the last entry has weight 1.
      const w = 1 / (1 + (this.history.length - 1 - i));
      sx += w * this.history[i].x;
      sy += w * this.history[i].y;
      weight += w;
    }
    let x = sx / weight;
    let y = sy / weight;

    // Stage 2 — the lag cap. The pull is a *limit*, not a bonus: the smoothed
    // point is projected back toward the hand when it drifted too far, along the
    // direction it drifted. This is the same clamp whether the pull came from
    // averaging (normal) or from a stylus that jumped (a dropped sample).
    const cap = this.lagCap;
    const dx = x - point.x;
    const dy = y - point.y;
    const distance = Math.hypot(dx, dy);
    if (cap <= 0) {
      x = point.x;
      y = point.y;
    } else if (distance > cap) {
      const k = cap / distance;
      x = point.x + dx * k;
      y = point.y + dy * k;
    }
    return { x, y };
  }

  /**
   * **The closing samples** at release: the straight run from the last filtered
   * point to the last raw one, so the stroke finishes under the hand.
   *
   * Returns an empty list when the two coincide (always, at 0 %), which is the
   * honest answer: nothing to add.
   */
  flush(): Point[] {
    const last = this.history[this.history.length - 1];
    if (!last) return [];
    const tail: Point[] = [];
    // The filter's own last output is re-derived from the history so `flush` is
    // a pure function of what was fed in — no hidden "previous output" state.
    const filtered = this.push({ x: last.x, y: last.y });
    if (Math.hypot(filtered.x - last.x, filtered.y - last.y) <= TAIL_EPSILON) return tail;
    tail.push({ x: filtered.x, y: filtered.y });
    tail.push({ x: last.x, y: last.y });
    return tail;
  }

  /** Forget the stroke (the next one starts clean). */
  reset(): void {
    this.history = [];
  }
}

/**
 * **Does the filter actually smooth?** — the quantity the StreamLine Law reads.
 *
 * The total *turning* of a polyline: the sum of the angles between consecutive
 * segments. Straight input scores 0 no matter how long it is; a jittering hand
 * scores high, once per sample. It is the honest measure of "jagged" for a
 * stroke (a variance of positions would call a perfectly straight diagonal
 * noisy), and it is scale-free, so the law can compare 100 samples of noise
 * against 100 samples of smoothed noise without units.
 *
 * Returns `null` for fewer than three points (there is no turn to measure).
 */
export function pathTurns(points: Point[]): number | null {
  if (points.length < 3) return null;
  let total = 0;
  for (let i = 2; i < points.length; i += 1) {
    const a = points[i - 1];
    const b = points[i];
    const c = points[i - 2];
    const v1x = a.x - c.x;
    const v1y = a.y - c.y;
    const v2x = b.x - a.x;
    const v2y = b.y - a.y;
    const l1 = Math.hypot(v1x, v1y);
    const l2 = Math.hypot(v2x, v2y);
    if (l1 === 0 || l2 === 0) continue;
    const cos = Math.min(1, Math.max(-1, (v1x * v2x + v1y * v2y) / (l1 * l2)));
    total += Math.acos(cos);
  }
  return total;
}

/**
 * **How much does the filter smooth, in the brief's own words?** — the second
 * quantity the StreamLine Law reads.
 *
 * The mean *squared second difference* of a polyline — the discrete curvature,
 * averaged over the samples and over both axes. It is the variance of the
 * hand's *jitter*, and it is the number to reach for when the question is
 * "lower variance", for a reason that matters here: a second difference is
 * blind to any constant offset **and to any linear trend**. The filter trails
 * the hand by design, so a lagged stroke sits tens of units away from the raw
 * one — a plain variance of positions would measure that lag and call a
 * perfectly smooth line noisy. Curvature sees only the shake.
 *
 * Returns `null` for fewer than three points (there is no second difference).
 */
export function pathJitter(points: Point[]): number | null {
  if (points.length < 3) return null;
  let total = 0;
  let count = 0;
  for (let i = 2; i < points.length; i += 1) {
    const dx = points[i - 2].x - 2 * points[i - 1].x + points[i].x;
    const dy = points[i - 2].y - 2 * points[i - 1].y + points[i].y;
    total += dx * dx + dy * dy;
    count += 1;
  }
  return total / count;
}

/**
 * The speed-of-response the status line prints: how far behind the hand the
 * filter is pulling, as a percentage of the maximum. A designer moving the
 * slider reads the number change; the report quotes the same three formulas.
 */
export function streamlineSummary(percent: number): string {
  const s = streamlineAmount(percent);
  if (s === 0) return 'StreamLine off · raw input';
  const window = 1 + Math.round(s * (STREAMLINE_MAX_WINDOW - 1));
  const lag = Math.round(s * STREAMLINE_MAX_LAG_PX);
  return `StreamLine ${Math.round(s * 100)}% · ${window}-sample pull · ≤${lag}px lag`;
}
