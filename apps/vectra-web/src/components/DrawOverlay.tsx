/**
 * **The drawing overlay** (Task 10.1 RULE 4).
 *
 * An SVG layer sitting exactly on top of the WebGPU canvas, showing — in the
 * rule's own words — *"only the tool cursor, the path being drawn, and the Bézier
 * handles"*.
 *
 * Two properties make it trustworthy, and both are structural rather than
 * careful:
 *
 * * **Every coordinate is the engine's**, mapped by the renderer's camera
 *   (`client.documentToClient`). This component does not know what a transform is.
 *   The y-axis is the reason it matters: document space is y-up and the DOM is
 *   y-down, so an overlay that positioned itself would be mirrored — correct on
 *   one axis, inside-out on the other, and just plausible enough to ship.
 * * **It draws nothing else.** No dimensions, no variable names, no constraint
 *   badges, no node labels: RULE 4's list is the component's entire vocabulary.
 *   A vertex whose value comes from an expression is drawn *hollow* rather than
 *   annotated — the designer can see it is owned by something else without being
 *   shown the something else.
 *
 * It is `pointer-events: none`: the canvas beneath it owns every gesture, so the
 * overlay can never eat a pointer sample (which is the classic way an overlay
 * breaks a drawing tool).
 */

import type { PlacedOverlay, PlacedPoint } from '../engine/draw/path-edit';

export interface DrawOverlayProps {
  overlay: PlacedOverlay;
  /** The vertex the direct-selection tool owns, if any. */
  selectedSlot: string | null;
  /** The vertex under the pointer, if any. */
  hoveredSlot: string | null;
  /** A Quick Shape hold is pending: the stroke could still become a primitive. */
  holding: boolean;
  /** The last snap's sentence, shown while it is fresh. */
  snapNote: string | null;
}

const ROLE_CLASS: Record<PlacedPoint['role'], string> = {
  anchor: 'ov-anchor',
  handle: 'ov-handle',
  draft: 'ov-draft-dot',
  sample: 'ov-sample-dot',
  cursor: 'ov-cursor',
};

/** A handle is drawn as a small square, an anchor as a square too — the two are
 *  told apart by colour and by the line that joins a handle to its anchor, which
 *  is exactly how a designer reads a Bézier editor. */
function pointClass(point: PlacedPoint, selectedSlot: string | null, hoveredSlot: string | null): string {
  const classes = ['ov-point', ROLE_CLASS[point.role]];
  if (point.role === 'anchor' || point.role === 'handle') {
    if (point.slot === selectedSlot) classes.push('ov-selected');
    else if (point.slot === hoveredSlot) classes.push('ov-hovered');
    // A non-literal vertex is owned by a parameter, not by the mouse.
    if (point.source && point.source !== 'literal') classes.push('ov-driven');
  }
  return classes.join(' ');
}

export default function DrawOverlay({
  overlay,
  selectedSlot,
  hoveredSlot,
  holding,
  snapNote,
}: DrawOverlayProps) {
  const { points, lines } = overlay;
  return (
    <svg className="draw-overlay" data-testid="draw-overlay" aria-hidden="true">
      {lines.map((line, index) => (
        <polyline
          key={`line-${index}`}
          className={`ov-line ov-line-${line.role}`}
          points={line.points.map(([x, y]) => `${x},${y}`).join(' ')}
        />
      ))}
      {points.map((point, index) => (
        <rect
          key={`point-${index}`}
          className={pointClass(point, selectedSlot, hoveredSlot)}
          x={point.cx - 3.5}
          y={point.cy - 3.5}
          width={7}
          height={7}
        />
      ))}
      {holding && (
        <text className="ov-hold" x="12" y="20">
          hold to snap
        </text>
      )}
      {snapNote && (
        <text className="ov-snap" data-testid="snap-note" x="12" y="40">
          {snapNote}
        </text>
      )}
    </svg>
  );
}
