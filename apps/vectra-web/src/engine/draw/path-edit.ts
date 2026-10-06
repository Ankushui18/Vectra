/**
 * **Direct Selection: what the overlay shows and what a click means**
 * (Task 10.1 RULE 4).
 *
 * The white arrow is the tool designers judge a vector editor by, and everything
 * that makes it feel right is in this file:
 *
 * * clicking a path selects the whole path; clicking a *point* selects only that
 *   point;
 * * an anchor's handles are invisible until they are relevant — hovering or
 *   selecting the anchor reveals them;
 * * dragging a handle edits that segment's control point, and dragging an anchor
 *   moves the point and bends the curves attached to it.
 *
 * None of that is geometry (the engine reports anchors, handles and hits in
 * document space; the renderer maps them to pixels) and none of it is state (the
 * document is the state). What it *is* is a **projection**, and the reason it is
 * its own module is that a projection is exactly the kind of thing that is wrong
 * in a way nobody notices until a designer drags the wrong handle.
 *
 * So the split is: this file decides *what is shown and what a click targets*;
 * `client.drawEditCommand` decides *what command that is* (the engine builds the
 * `SetParameter` batch, including the symmetry the Alt key breaks); React decides
 * nothing at all.
 */

import type { AnchorWire, DrawHitWire, DrawOverlayWire } from '../wire';

/** A hit, normalised: `miss` or a named slot with a side. */
export interface Hit {
  kind: 'miss' | 'anchor' | 'handle';
  /** `"start"` or `"segments[i].to"` for an anchor; the control slot for a handle. */
  slot: string;
  /** `"out"` for the handle a segment *leaves* an anchor with, `""` for anchors. */
  side: '' | 'out';
  index: number;
  distance: number;
  x: number;
  y: number;
}

/** `{"kind":"miss"}` is the engine's answer for empty space; everything else is a target. */
export function parseHit(json: DrawHitWire): Hit {
  if (json.kind === 'miss') {
    return { kind: 'miss', slot: '', side: '', index: -1, distance: 0, x: 0, y: 0 };
  }
  return {
    kind: json.kind,
    slot: json.slot,
    side: json.side === 'out' ? 'out' : '',
    index: json.index,
    distance: json.distance,
    x: json.x,
    y: json.y,
  };
}

/** The overlay payload, normalised — a failed read is an empty overlay, not a throw. */
export function parseOverlay(json: DrawOverlayWire): AnchorWire[] {
  return json.ok ? json.anchors : [];
}

/**
 * Is this a hit on the anchor at `slot`?
 *
 * The whole-path case (`RULE 4`: "clicking a path selects the whole path") is the
 * *miss* case of the tool's own hit test: the pointer landed on the shape but not
 * near a vertex, so the path is selected and no individual point is owned. That
 * is the real behaviour of the white arrow, and it falls out of the engine's
 * answer rather than needing a second hit test in the UI.
 */
export function isAnchorHit(hit: Hit, slot: string): boolean {
  return hit.kind === 'anchor' && hit.slot === slot;
}

/**
 * **Which handles are visible** (RULE 4: "hovering a handle reveals it").
 *
 * The rule designers actually expect, and the one implemented here, is: a vertex's
 * handles are drawn when the vertex is *chosen* (clicked) or *hovered*. Showing
 * every handle of every path at all times is the amateur version — it turns a
 * drawing into a thicket — and showing none until a drag starts is the *other*
 * amateur version, because then nobody can find the handle to grab.
 *
 * Returns the anchor slots whose handles should be drawn.
 */
export function revealedHandles(anchors: AnchorWire[], hoveredSlot: string | null, selectedSlot: string | null): string[] {
  const revealed: string[] = [];
  for (const anchor of anchors) {
    const shown = anchor.slot === selectedSlot || anchor.slot === hoveredSlot;
    if (shown) revealed.push(anchor.slot);
  }
  return revealed;
}

/** One thing the overlay draws, in document units. */
export interface OverlayPoint {
  x: number;
  y: number;
  role: 'anchor' | 'handle' | 'draft' | 'sample' | 'cursor';
  /** The slot this point belongs to, when it belongs to one. */
  slot?: string;
  side?: '' | 'in' | 'out';
  /** The anchor's resolution (`"literal"`, `"expression"`, …) — the UI greys the
   *  rest, because an expression owns that number, not the mouse. */
  source?: string;
}

/** A polyline the overlay draws, in document units. */
export interface OverlayLine {
  role: 'handle' | 'draft' | 'sample';
  points: Array<[number, number]>;
}

export interface OverlayModel {
  points: OverlayPoint[];
  lines: OverlayLine[];
  /** Every point, in one list, for a single camera mapping call. */
  shape: Array<[number, number]>;
}

export interface OverlayInput {
  /** The path's anchors, from `draw_overlay`. */
  anchors: AnchorWire[];
  /** The anchors whose handles are revealed (see `revealedHandles`). */
  revealed: string[];
  /** The anchor the direct-selection tool owns, if any. */
  selectedSlot: string | null;
  /** The in-progress pen path, from `draw_pointer`'s draft. */
  draft?: { points: [number, number][]; start: [number, number] } | null;
  /** The in-progress brush stroke, `[x, y, pressure]`. */
  samples?: [number, number, number][];
  /** Where the pointer is, in document units — the tool cursor. */
  cursor?: { x: number; y: number } | null;
}

/**
 * Build the overlay's geometry.
 *
 * Every point comes from the engine: the anchors and handles from `draw_overlay`,
 * the draft from `draw_pointer`, the samples from the brush's own reply. This
 * function *orders* and *labels* them — it never invents or interpolates one —
 * which is what lets the canvas be an honest picture of the document. The
 * returned `shape` is the flattened list the caller maps through
 * `client.documentToClient` in a single call, so the overlay crosses the WASM
 * boundary once per frame rather than once per vertex.
 */
export function overlayModel(input: OverlayInput): OverlayModel {
  const points: OverlayPoint[] = [];
  const lines: OverlayLine[] = [];
  const shape: Array<[number, number]> = [];
  const push = (point: OverlayPoint): void => {
    points.push(point);
    shape.push([point.x, point.y]);
  };

  for (const anchor of input.anchors) {
    push({
      x: anchor.x,
      y: anchor.y,
      role: 'anchor',
      slot: anchor.slot,
      side: '',
      source: anchor.source,
    });
    if (!input.revealed.includes(anchor.slot)) continue;
    // The handle lines: from the anchor to its control point. Two points and a
    // line — the *shape* of the curve between them is the engine's business.
    for (const [side, handle] of [
      ['in', anchor.handle_in],
      ['out', anchor.handle_out],
    ] as const) {
      if (!handle) continue;
      push({ x: handle[0], y: handle[1], role: 'handle', slot: anchor.slot, side });
      lines.push({
        role: 'handle',
        points: [
          [anchor.x, anchor.y],
          [handle[0], handle[1]],
        ],
      });
    }
  }

  if (input.samples && input.samples.length > 0) {
    for (const [x, y] of input.samples) push({ x, y, role: 'sample' });
    lines.push({ role: 'sample', points: input.samples.map(([x, y]) => [x, y]) });
  }

  if (input.draft) {
    for (const [x, y] of input.draft.points) push({ x, y, role: 'draft' });
    lines.push({ role: 'draft', points: input.draft.points });
  }

  if (input.cursor) push({ x: input.cursor.x, y: input.cursor.y, role: 'cursor' });

  return { points, lines, shape };
}

/**
 * The slot a drag should edit, given a hit — or `null` for "not an editable
 * point".
 *
 * The engine's `draw_edit_command` takes `(slot, side, anchor)`: `anchor: true`
 * moves the point itself (and translates the handles attached to it), `false`
 * moves one control point. `side` is `"out"` for the handle a segment leaves an
 * anchor with — the engine's `draw_hit` reports that side, and the *other*
 * handle (`handle_in`) is the one that arrives, which the same anchor's incoming
 * segment owns.
 */
export function editTarget(hit: Hit): { slot: string; side: string; anchor: boolean } | null {
  if (hit.kind === 'anchor') return { slot: hit.slot, side: '', anchor: true };
  if (hit.kind === 'handle') return { slot: hit.slot, side: hit.side, anchor: false };
  return null;
}

/**
 * The label the status strip shows for a selected vertex.
 *
 * Slots are the document's own vocabulary (`segments[2].to`), and a designer
 * should never have to read one — so the strip says "anchor 3 of 5" and keeps the
 * slot for the tooltip. RULE 4's "the math stays hidden" applies to identifiers
 * too, not just to panels.
 */
export function anchorLabel(anchors: AnchorWire[], slot: string | null): string {
  if (slot === null) return 'no point selected';
  const index = anchors.findIndex((anchor) => anchor.slot === slot);
  if (index < 0) return 'no point selected';
  const kind = anchors[index].source === 'literal' ? 'anchor' : `${anchors[index].source}-driven anchor`;
  return `${kind} ${index + 1} of ${anchors.length}`;
}

/** An overlay point, placed on screen. */
export interface PlacedPoint extends OverlayPoint {
  /** Client (viewport) pixels, from the renderer's camera. */
  cx: number;
  cy: number;
}

/** The overlay, ready to render: screen-space points and polylines. */
export interface PlacedOverlay {
  points: PlacedPoint[];
  lines: Array<{ role: OverlayLine['role']; points: Array<[number, number]> }>;
}

/**
 * Put the model on the screen.
 *
 * `mapped` is `client.documentToClient(model.shape)` — one camera call for the
 * whole overlay, in the same order the model produced. This function only pairs
 * them back up, and it is deliberately tolerant: if the renderer has no canvas box
 * yet it answers with an empty list, and an overlay that cannot be placed is
 * *empty* rather than drawn in the wrong place. (A handle a few pixels off is a
 * handle a designer will miss, and a *silently* misplaced one is worse: they will
 * conclude the tool is broken.)
 */
export function placeOverlay(model: OverlayModel, mapped: Array<[number, number]>): PlacedOverlay {
  if (mapped.length !== model.shape.length) return { points: [], lines: [] };
  const placed = model.points.map((point, index) => ({
    ...point,
    cx: mapped[index][0],
    cy: mapped[index][1],
  }));
  // Lines are drawn from the placed points themselves, so a line can never
  // disagree with the vertices it joins.
  const position = new Map<string, [number, number]>();
  model.shape.forEach(([x, y], index) => position.set(`${x},${y}`, mapped[index]));
  const at = (x: number, y: number): [number, number] => position.get(`${x},${y}`) ?? [x, y];
  return {
    points: placed,
    lines: model.lines.map((line) => ({
      role: line.role,
      points: line.points.map(([x, y]) => at(x, y)),
    })),
  };
}
