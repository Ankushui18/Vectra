/**
 * **The pen and brush settings** (Task 10.7 RULE 2 + RULE 4's source).
 *
 * Two controls, and both are deliberately physical rather than numeric:
 *
 * * the **StreamLine slider** — 0 % draws exactly what the hand did, 100 % is the
 *   maximum pull the filter allows, and the caption under it reads the same
 *   three numbers the mathematics uses (window, lag), so the slider never hides
 *   what it is doing. The slider is the *only* place the amount is set; the
 *   filter itself (`engine/draw/streamline.ts`) is pure and is what the tests
 *   attack.
 * * the **colour well** — the colour ColorDrop carries. It is a swatch you drag
 *   onto the canvas, not a button you press: press it and a chip follows the
 *   pointer, release over a shape and the shape takes the colour
 *   (`engine/draw/colordrop.ts` decides what that means, `App` asks the engine
 *   what is under the drop). The small picker beside it is the plain colour
 *   input — one gesture to change the colour, one to use it.
 *
 * Neither control knows anything about geometry, layers or commands. The panel
 * reports an amount in percent and a colour in hex, and that is the whole
 * contract — which is what keeps the "dumb remote" rule (Task 1.4) true even for
 * a feature this tactile.
 */

import { streamlineAmount, streamlineSummary } from '../engine/draw/streamline';

export interface DrawSettingsPanelProps {
  /** 0–100, straight from the slider. */
  streamline: number;
  onStreamline: (percent: number) => void;
  /** The colour the well carries, `#rrggbb` or `#rrggbbaa`. */
  color: string;
  onColor: (color: string) => void;
  /** Start the drag-to-fill: the pointer's client position. */
  onDropStart: (clientX: number, clientY: number) => void;
  /** True while a colour is in flight (the well shows it is held). */
  dragging: boolean;
  disabled?: boolean;
}

/** The active tool's name, so the caption can say who the settings are for. */
function targetLabel(tool: string): string {
  if (tool === 'pen') return 'Pen';
  if (tool === 'brush') return 'Brush';
  return 'Pen & Brush';
}

export default function DrawSettingsPanel({
  streamline,
  onStreamline,
  color,
  onColor,
  onDropStart,
  dragging,
  disabled,
  tool,
}: DrawSettingsPanelProps & { tool: string }) {
  const amount = streamlineAmount(streamline);
  return (
    <section className="panel draw-settings" data-testid="draw-settings">
      <h2>
        {targetLabel(tool)} settings{' '}
        <span className="sub">the hand, filtered — and the paint, in hand</span>
      </h2>

      <label className="streamline" data-testid="streamline-control">
        <span className="streamline-label">
          StreamLine
          <span className="sub" data-testid="streamline-readout">
            {streamlineSummary(streamline)}
          </span>
        </span>
        <input
          type="range"
          min={0}
          max={100}
          step={1}
          data-testid="streamline-slider"
          value={Math.round(amount * 100)}
          disabled={disabled}
          aria-label="StreamLine smoothing"
          onChange={(event) => onStreamline(Number(event.target.value))}
        />
        <span className="streamline-ends">
          <span>raw</span>
          <span>{Math.round(amount * 100)}%</span>
          <span>max</span>
        </span>
      </label>

      <div className="colordrop" data-testid="colordrop">
        <button
          type="button"
          className={`well${dragging ? ' held' : ''}`}
          data-testid="colordrop-well"
          style={{ background: color }}
          title="Drag onto the canvas to fill what is under the pointer"
          aria-label={`Drag colour ${color} onto the canvas to fill`}
          disabled={disabled}
          onPointerDown={(event) => {
            if (disabled) return;
            event.preventDefault();
            onDropStart(event.clientX, event.clientY);
          }}
        >
          <span className="well-grip" aria-hidden="true">
            ⋮⋮
          </span>
        </button>
        <label className="swatch well-picker" title="Colour">
          <input
            type="color"
            data-testid="colordrop-color"
            disabled={disabled}
            value={color.slice(0, 7)}
            onChange={(event) => onColor(event.target.value)}
          />
        </label>
        <span className="sub colordrop-hint">
          drag the swatch onto a shape to fill it — empty space does nothing
        </span>
      </div>
    </section>
  );
}
