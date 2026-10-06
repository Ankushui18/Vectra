/**
 * **The navigation overlay** (Task 10.3 RULE 2) — what a panned, zoomed canvas
 * needs to stay understandable.
 *
 * Two things live here, and both exist because the camera can now move:
 *
 * 1. **Where the artboards are.** Once the view can pan, a designer can lose the
 *    artwork entirely — frame past it, or zoom out to 1% and not know which of
 *    nine boards is which. Each board is outlined (and tinted with its own
 *    background colour) where the camera says it is, with its name attached.
 *    Every corner comes from `toClient`, i.e. from the renderer's own
 *    `document_to_client`: this component owns no transform, exactly like the
 *    drawing overlay. The y-flip is the part that would silently mirror if it
 *    did.
 * 2. **The zoom controls.** `−`, the readout (which is also the "100%" button),
 *    `+`, and *fit*. Relative steps for the two buttons and one absolute jump
 *    for the readout, because a designer who clicks a label saying 240% means
 *    exactly 240%.
 *
 * It is `pointer-events: none` as a layer and re-enables them only on the
 * controls, so a gesture anywhere else reaches the canvas — an overlay that eats
 * pointer samples is the classic way a pan breaks.
 */

import type { ArtboardRow, NavView } from '../engine/panels';
import { formatZoom } from '../engine/panels';

export interface NavigationOverlayProps {
  /** The document rectangle on screen now, as the renderer reports it. */
  view: NavView | null;
  /** CSS pixels per document unit, as the renderer reports it. */
  zoom: number | null;
  /** The document's artboards, in engine order. */
  boards: ArtboardRow[];
  /** Document points → client pixels: the **engine's** transform, never ours. */
  toClient: (points: [number, number][]) => [number, number][];
  onZoomIn: () => void;
  onZoomOut: () => void;
  onZoomReset: () => void;
  onZoomFit: () => void;
  /** Zoom this far in one press of `+`/`−` (the same value a wheel notch uses). */
  disabled?: boolean;
}

/** One board's outline, in client pixels. */
interface BoardRect {
  ok: boolean;
  x: number;
  y: number;
  w: number;
  h: number;
}

function boardRect(
  board: ArtboardRow,
  toClient: (points: [number, number][]) => [number, number][],
): BoardRect {
  const [x, y, w, h] = board.bounds;
  const corners: [number, number][] = [
    [x, y],
    [x + w, y],
    [x, y + h],
    [x + w, y + h],
  ];
  const mapped = toClient(corners);
  if (mapped.length !== 4) return { ok: false, x: 0, y: 0, w: 0, h: 0 };
  const xs = mapped.map((point) => point[0]);
  const ys = mapped.map((point) => point[1]);
  const minX = Math.min(...xs);
  const minY = Math.min(...ys);
  return {
    ok: true,
    x: minX,
    y: minY,
    w: Math.max(...xs) - minX,
    h: Math.max(...ys) - minY,
  };
}

export default function NavigationOverlay({
  view,
  zoom,
  boards,
  toClient,
  onZoomIn,
  onZoomOut,
  onZoomReset,
  onZoomFit,
  disabled,
}: NavigationOverlayProps) {
  const rects = boards.map((board) => ({ board, rect: boardRect(board, toClient) }));

  return (
    <>
      <svg className="nav-overlay" data-testid="nav-overlay" aria-hidden="true">
        {/* The visible document window, so "where am I" is answerable even when
            every board is off screen. */}
        {view && (
          <rect
            className="nav-window"
            data-testid="nav-window"
            x={0}
            y={0}
            width={Math.max(0, view.w * (zoom ?? 1))}
            height={Math.max(0, view.h * (zoom ?? 1))}
          />
        )}
        {rects.map(({ board, rect }) =>
          rect.ok ? (
            <g key={board.id}>
              <rect
                className={`nav-board${board.active ? ' nav-board-active' : ''}`}
                data-testid={`nav-board-${board.id}`}
                x={rect.x}
                y={rect.y}
                width={rect.w}
                height={rect.h}
                fill={board.background}
              />
              <text className="nav-board-label" x={rect.x + 4} y={rect.y - 4}>
                {board.name} · {board.label}
              </text>
            </g>
          ) : null,
        )}
      </svg>

      <div className="nav-controls" data-testid="nav-controls">
        <button
          className="nav-button"
          data-testid="zoom-out"
          disabled={disabled}
          title="Zoom out (wheel down)"
          aria-label="Zoom out"
          onClick={onZoomOut}
        >
          −
        </button>
        <button
          className="nav-button nav-zoom"
          data-testid="zoom-readout"
          disabled={disabled}
          title="Reset to 100%"
          aria-label={`Zoom ${formatZoom(zoom)} — reset to 100%`}
          onClick={onZoomReset}
        >
          {formatZoom(zoom)}
        </button>
        <button
          className="nav-button"
          data-testid="zoom-in"
          disabled={disabled}
          title="Zoom in (wheel up)"
          aria-label="Zoom in"
          onClick={onZoomIn}
        >
          +
        </button>
        <button
          className="nav-button"
          data-testid="zoom-fit"
          disabled={disabled}
          title="Fit the whole document (0)"
          aria-label="Fit the document"
          onClick={onZoomFit}
        >
          ⤢
        </button>
      </div>
    </>
  );
}
