/**
 * **The Region overlay** (Task 12.0 RULE 1's visible half).
 *
 * An SVG layer over the canvas that draws the face under the pointer, the
 * crossings that bound it, and nothing else — the same discipline the drawing
 * overlay keeps: RULE 4's list is its whole vocabulary.
 *
 * The coordinates are the engine's, twice over: the *region* comes from the
 * region graph as document-space rings, and the mapping to client pixels is the
 * renderer's camera (`client.documentToClient`, threaded in as `toClient`). This
 * component has no transform of its own, which is the only way the highlight can
 * be guaranteed to sit exactly on the geometry the fill will adopt — a highlight
 * half a pixel off is a fill in the wrong place.
 *
 * `pointer-events: none`, like every overlay here: the canvas beneath owns every
 * gesture.
 */

import { useEffect, useState } from 'react';
import { placeRegion, type PlacedRegion, type RegionShape } from '../engine/draw/regions';

export interface RegionOverlayProps {
  /** The face to highlight, in document units (`regionShape`'s answer). */
  shape: RegionShape;
  /** The renderer's camera: document points in, client pixels out. */
  toClient: (points: Array<[number, number]>) => Array<[number, number]>;
  /** One line for the strip: which region this is. */
  status?: string | null;
}

export default function RegionOverlay({ shape, toClient, status }: RegionOverlayProps) {
  const [placed, setPlaced] = useState<PlacedRegion>({ rings: [], crossings: [] });

  // One camera call per change, exactly like the draw overlay. A shape that
  // cannot be placed is empty rather than wrong (`placeRegion`).
  useEffect(() => {
    setPlaced(placeRegion(shape, toClient));
    // `toClient` is a fresh closure per render in the host; the *shape* is what
    // this depends on, and the host rebuilds both together.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [shape]);

  if (placed.rings.length === 0) return null;

  return (
    <svg className="draw-overlay" data-testid="region-overlay" aria-hidden="true">
      {placed.rings.map((ring, index) =>
        ring.length < 3 ? null : (
          <polygon
            key={`ring-${index}`}
            className={index === 0 ? 'ov-region' : 'ov-region ov-region-hole'}
            points={ring.map(([x, y]) => `${x},${y}`).join(' ')}
          />
        ),
      )}
      {placed.crossings.map(([x, y], index) => (
        <circle key={`crossing-${index}`} className="ov-crossing" cx={x} cy={y} r={3.5} />
      ))}
      {status ? (
        <text className="ov-region-note" data-testid="region-note" x="12" y="20">
          {status}
        </text>
      ) : null}
    </svg>
  );
}
