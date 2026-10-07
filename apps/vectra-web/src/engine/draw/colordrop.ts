/**
 * **ColorDrop** (Task 10.7 RULE 4) — drag a colour onto the canvas, fill what is
 * under it.
 *
 * The rule is three sentences and this file is all three, as a plan:
 *
 * 1. *find the enclosed area with the existing hit test* — the drop point goes
 *    through `client.pointerHit`, the same renderer-side index a click uses. The
 *    UI computes no region of its own, and it never guesses from coordinates.
 * 2. *dropped inside an enclosed area → **a new Smart Fill*** (Task 12.0 RULE 4)
 *    — the region is identified by the engine's region graph and the fill is
 *    pinned to it: the boundaries and the drop point, not a recoloured shape.
 *    This is checked *before* the shape test, because a face is what a drop into
 *    an arrangement means, even when a path also happens to be under the point.
 * 3. *dropped on an existing shape → fill that shape* — the shape's **first
 *    fill** in the resolved appearance stack is recoloured. That is the fill a
 *    designer sees the drop land on (the bottom of the stack, the one Procreate's
 *    ColorDrop replaces), and every other row is echoed back verbatim, so a
 *    gradient, a stroke and a blend mode survive the drop untouched.
 * 4. *dropped in empty space → nothing* — no hit, no plan, no command, no
 *    history entry. Reported as {@link DropPlan} `noop` with the reason, so the
 *    status line can say why nothing happened instead of looking broken.
 *
 * A node with **no** fill at all gets one (a solid fill with the dropped
 * colour). That is not a special case for its own sake: dropping paint on a
 * shape that has none is the commonest thing a designer does with the feature,
 * and "fill the shape" is the rule's own wording.
 *
 * The plan is a value, not an action — {@link planColorDrop} decides, the caller
 * dispatches. That is what lets the law test run headlessly, and what keeps the
 * command construction in `engine/commands.ts` with its siblings.
 */

import { regionDrop, type RegionDrop } from './regions';
import type { RegionPlanWire, SnapshotAppearanceWire, SnapshotPaintWire } from '../wire';

/**
 * The three facts a drop can lead to: a **region** to fill (Task 12.0 RULE 4), a
 * shape to recolour, or nothing.
 *
 * `smartFill` is listed first because that is the rule's order: a drop inside an
 * enclosed area creates a fill *pinned to that region*, and only a drop that
 * lands in no enclosed area falls through to recolouring the shape beneath it.
 */
export type DropPlan =
  | { kind: 'smartFill'; boundaries: string[]; seed: [number, number]; color: string; name: string }
  | { kind: 'fill'; nodeId: string; color: string }
  | { kind: 'noop'; reason: string };

/**
 * **The rule, as a function.**
 *
 * `nodeId` is the renderer's hit test answer (or `null` for empty space) and
 * `color` is the hex the picker produced. A malformed colour is a no-op too: the
 * engine's colour parser is strict, and a drop that would send it nonsense should
 * not reach the wire.
 */
export function planColorDrop(
  nodeId: string | null,
  color: string,
  region: RegionDrop | null = null,
): DropPlan {
  if (!isHexColor(color)) return { kind: 'noop', reason: `not a colour: ${color}` };
  // RULE 4 first: an enclosed area *is* the answer when the drop is inside one.
  // The boundaries and the seed are the engine's (see `regionDrop`), so the new
  // fill lands on the very region the tool would have highlighted.
  if (region) {
    return {
      kind: 'smartFill',
      boundaries: region.boundaries,
      seed: region.seed,
      color,
      name: region.name,
    };
  }
  if (!nodeId) return { kind: 'noop', reason: 'empty space — nothing to fill' };
  return { kind: 'fill', nodeId, color };
}

/**
 * The same rule, one step earlier: **which region did this drop land in?**
 *
 * `plan` is the engine's answer for the drop point and `point` is the point
 * itself; a plan with no hit is a drop in no enclosed area, which is what the
 * shape test then gets to answer. Exported so the caller reads as the rule does
 * — region first, shape second — instead of assembling the order itself.
 */
export function dropRegion(
  plan: RegionPlanWire | null,
  point: [number, number] | null,
): RegionDrop | null {
  return regionDrop(plan, point);
}

/** `#rrggbb` or `#rrggbbaa` — the two shapes the engine's colour takes. */
export function isHexColor(color: string): boolean {
  return /^#[0-9a-fA-F]{6}([0-9a-fA-F]{2})?$/.test(color);
}

/**
 * **The stack with the dropped colour in it.**
 *
 * The bottom-most fill is recoloured if it is a solid paint — a gradient's
 * identity is its stops, and rewriting a stop is `panels.recolorStop`'s job, not
 * a drop's: dropping a flat colour on a gradient in Procreate fills the *shape*,
 * which is the same choice made differently. When the first fill is a gradient,
 * a solid fill is inserted *under* it rather than replacing it, so the drop adds
 * a colour instead of destroying one; when there is no fill at all, one is added
 * on top of the stack.
 */
export function stackWithColor(
  stack: SnapshotAppearanceWire[],
  color: string,
): SnapshotAppearanceWire[] {
  const index = stack.findIndex((row) => row.kind === 'fill');
  const solid: SnapshotPaintWire = { type: 'solid', color };
  if (index === -1) {
    return [
      ...stack,
      { kind: 'fill', paint: solid, opacity: 1, blend: 'normal', visible: true, width: null },
    ];
  }
  const row = stack[index];
  if (row.paint.type === 'solid') {
    const next = stack.slice();
    next[index] = { ...row, paint: solid, visible: true };
    return next;
  }
  const next = stack.slice();
  next.splice(index, 0, {
    kind: 'fill',
    paint: solid,
    opacity: 1,
    blend: 'normal',
    visible: true,
    width: null,
  });
  return next;
}

/**
 * Does the drop change anything? A drop of the colour a shape already has is a
 * no-op the *engine* would still record as a history entry, so the check lives
 * here: an undo step that restores an identical stack is a trap for the designer
 * pressing undo twice.
 */
export function dropChangesStack(stack: SnapshotAppearanceWire[], color: string): boolean {
  const first = stack.find((row) => row.kind === 'fill');
  if (!first) return true;
  if (first.paint.type !== 'solid') return true;
  return first.paint.color.toLowerCase() !== color.toLowerCase();
}

/** One line for the status strip: what the drop did, in the designer's words. */
export function dropStatus(plan: DropPlan, name: string | null): string {
  if (plan.kind === 'noop') return `◧ ColorDrop · ${plan.reason}`;
  if (plan.kind === 'smartFill') {
    const boundaries = plan.boundaries.length === 1 ? '1 path' : `${plan.boundaries.length} paths`;
    return `◧ ColorDrop · new smart fill in the region between ${boundaries} · ${plan.color}`;
  }
  return `◧ ColorDrop · filled ${name ?? plan.nodeId} with ${plan.color}`;
}
