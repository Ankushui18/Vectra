/**
 * **Task 12.0 at the UI boundary** — regions, spans and the two gestures that
 * use them: the Smart Fill tool (hover a face, click to fill it) and RULE 3's
 * "Break Path at Intersections".
 *
 * The engine owns every number. `smart_fill_plan` intersects the paths, refines
 * the arrangement into faces, measures each face's area, finds the crossings and
 * cuts each outline into its spans; this module *reads* that plan and decides
 * three small things, each of which is a function you can call without a
 * browser:
 *
 * * **what the overlay draws** — the face under the pointer, as rings in
 *   document units, mapped to the screen by the renderer's camera (never by a
 *   transform here: the y-axis would be inside-out);
 * * **what the drop does** — RULE 4's precedence, in {@link regionDrop}: a drop
 *   inside an enclosed area *is* a new Smart Fill, which is why the region test
 *   comes before the shape test in `planColorDrop`;
 * * **what the panel offers** — the spans of the selected path, and the
 *   `[from, to]` pairs "Break" hands back to the engine verbatim.
 *
 * That last point is the one worth stating plainly: the UI never computes where
 * an intersection is. It echoes the numbers the engine measured, which is the
 * only way a break can be guaranteed to land exactly on the crossings the
 * region graph found — the same points, not points that agree to five decimals.
 */

import type { PlanRegionWire, PlanSourceWire, RegionPlanWire } from '../wire';

/** What a plan says about the *hit* face, in document units. */
export interface RegionShape {
  /** The hit face's rings: exterior first, then holes. */
  rings: Array<Array<[number, number]>>;
  /** Every crossing the plan found, for the markers. */
  crossings: Array<[number, number]>;
}

/** The same thing, placed on the screen by the renderer's camera. */
export interface PlacedRegion {
  rings: Array<Array<[number, number]>>;
  crossings: Array<[number, number]>;
}

/** The face the probe point fell in, or `null` — the tool's whole hover state. */
export function regionAt(plan: RegionPlanWire | null): PlanRegionWire | null {
  if (!plan || plan.hit === null || plan.hit === undefined) return null;
  return plan.regions[plan.hit] ?? null;
}

/**
 * What the Smart Fill tool highlights: the hit face, plus the crossings.
 *
 * An empty hit is an empty shape rather than a fallback to some other face: the
 * pointer is over empty space, and highlighting the nearest region would put a
 * fill under a click the designer did not aim at.
 */
export function regionShape(plan: RegionPlanWire | null): RegionShape {
  const face = regionAt(plan);
  return {
    rings: face ? face.rings : [],
    crossings: plan ? plan.crossings : [],
  };
}

/**
 * Put a {@link RegionShape} on the screen.
 *
 * `map` is `client.documentToClient(points)` over the flattened shape, in order
 * — one camera call for the whole overlay, exactly as `placeOverlay` does it for
 * the drawing tools. A mapper that answers with a different number of points
 * (no canvas box yet) yields an *empty* overlay rather than a misplaced one.
 */
export function placeRegion(
  shape: RegionShape,
  map: (points: Array<[number, number]>) => Array<[number, number]>,
): PlacedRegion {
  const flat: Array<[number, number]> = [];
  for (const ring of shape.rings) flat.push(...ring);
  flat.push(...shape.crossings);
  const mapped = map(flat);
  if (mapped.length !== flat.length) return { rings: [], crossings: [] };
  let at = 0;
  const rings = shape.rings.map((ring) => {
    const placed = mapped.slice(at, at + ring.length);
    at += ring.length;
    return placed;
  });
  return { rings, crossings: mapped.slice(at) };
}

/** One sentence under the canvas: what the pointer is over. */
export function regionStatus(plan: RegionPlanWire | null): string {
  if (!plan) return '◲ Smart Fill · regions unavailable';
  const face = regionAt(plan);
  if (!face) return '◲ Smart Fill · no enclosed area here';
  const boundaries = face.members.length === 1 ? '1 boundary' : `${face.members.length} boundaries`;
  const holes = face.holes === 1 ? ' · 1 hole' : face.holes > 1 ? ` · ${face.holes} holes` : '';
  return `◲ Smart Fill · region ${face.index + 1} of ${plan.regions.length} · ${boundaries}${holes}`;
}

/**
 * **RULE 4's drop, as a value**: what a colour dropped at `point` does.
 *
 * The rule's order is the whole of it — *"dropped inside an enclosed area
 * identifies the region and creates a new SmartFill node pinned to that exact
 * region"* — so a region hit wins over a shape hit, and a drop that lands on a
 * shape that is also *inside* an arrangement made by several paths creates the
 * fill rather than recolouring one of them. The returned `boundaries` are the
 * face's own signature, in the plan's order, and `seed` is the very point that
 * was dropped: that pair is the new node's whole identity.
 */
export interface RegionDrop {
  boundaries: string[];
  seed: [number, number];
  name: string;
}

export function regionDrop(
  plan: RegionPlanWire | null,
  point: [number, number] | null,
): RegionDrop | null {
  if (!plan || !point) return null;
  const face = regionAt(plan);
  if (!face || face.members.length === 0) return null;
  return {
    boundaries: face.members,
    seed: point,
    name: `smart fill ${face.index + 1}`,
  };
}

/** One row of the Region panel: a span of the selected path's outline. */
export interface SpanRow {
  /** Its position in the plan's span list — the row key and what Break sends. */
  index: number;
  /** `span 2 of 3 · ring 1`. */
  label: string;
  /** Arc length, formatted by the engine's number (no UI arithmetic). */
  length: string;
  /** The `[from, to]` pair to hand back — the engine's own numbers. */
  span: [number, number];
}

/** The plan's sources as rows: what took part, and how much it spans. */
export function sourceRows(plan: RegionPlanWire | null): PlanSourceWire[] {
  return plan ? plan.sources : [];
}

const num = (v: number, digits = 2): string =>
  Number.isFinite(v) ? String(Number(v.toFixed(digits))) : '—';

/**
 * The spans of one source, as the panel's rows.
 *
 * A path with no crossings has an empty list — not a single span covering the
 * whole outline. That is the honest answer: "break this at its intersections"
 * has no meaning without an intersection, and a row for a break that cannot be
 * made would be a button that lies.
 */
export function spanRows(plan: RegionPlanWire | null, sourceId: string): SpanRow[] {
  const source = plan?.sources.find((candidate) => candidate.id === sourceId) ?? null;
  if (!source) return [];
  return source.spans.map((span) => ({
    index: span.index,
    label: `span ${span.index + 1} of ${source.spans.length} · ring ${span.ring + 1} · ${num(
      span.length,
    )} of ${num(span.total)}`,
    length: num(span.length),
    span: [span.from, span.to] as [number, number],
  }));
}

/**
 * **What "Break Path at Intersections" sends** (RULE 3): the spans' arc-length
 * pairs, unedited.
 *
 * `rows` is the panel's list — every span of one path, or the single row a
 * designer picked. Whichever it is, the numbers went from the engine to the
 * panel and back untouched, so the cut lands on the crossing the region graph
 * found rather than near it.
 */
export function breakSpans(rows: SpanRow[]): Array<[number, number]> {
  return rows.map((row) => row.span);
}

/** One sentence for the log: what the break did. */
export function breakStatus(rows: SpanRow[], name: string): string {
  const pieces = rows.length * 2;
  const spans = rows.length === 1 ? '1 span' : `${rows.length} spans`;
  return `✂ Broke ${name} at ${spans} → ${pieces} paths`;
}
