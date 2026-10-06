/**
 * The tool palette (Task 10.1).
 *
 * Four buttons and a status strip. It holds no state and makes no decisions: the
 * tool lives in `App`'s session, the vocabulary lives in `engine/draw/tools.ts`,
 * and this component renders both. That is the whole reason it is a component
 * instead of more JSX in `App.tsx` — the file that routes pointers should not
 * also be the file that decides what a palette button looks like.
 */

import { TOOLS } from '../engine/draw/tools';
import type { ToolId } from '../engine/draw/tools';

export interface ToolPaletteProps {
  tool: ToolId;
  onSelect: (tool: ToolId) => void;
  disabled: boolean;
  /** What the status strip says — the active tool's blurb, or the engine's answer. */
  status: string;
}

export default function ToolPalette({ tool, onSelect, disabled, status }: ToolPaletteProps) {
  return (
    <div className="tool-palette" data-testid="tool-palette">
      <div className="btn-row tool-row">
        {TOOLS.map((spec) => (
          <button
            key={spec.id}
            type="button"
            data-testid={`tool-${spec.id}`}
            className={spec.id === tool ? 'tool active' : 'tool'}
            aria-pressed={spec.id === tool}
            title={`${spec.label} (${spec.shortcut}) — ${spec.blurb}`}
            disabled={disabled}
            onClick={() => onSelect(spec.id)}
          >
            {spec.label}
            <kbd>{spec.shortcut}</kbd>
          </button>
        ))}
      </div>
      <div className="tool-status" data-testid="tool-status">
        {status}
      </div>
    </div>
  );
}

/**
 * **The designer rail** (Task 10.1 RULE 4).
 *
 * What stands in the side column while the parametric panels are hidden. It is
 * not a summary of the document — that would be the math again, wearing a
 * different hat. It is a *tool*: which tool is held, how to use it, and how much
 * of the path exists so far. Nothing here is a number the engine computes.
 *
 * The distinction matters and is the rule's whole point: a designer mid-stroke
 * needs the canvas, the cursor and their handles — every extra panel is a
 * distraction from the shape, and every *parametric* panel is an invitation to
 * stop drawing and start editing numbers. The panels come back the moment they
 * put the pen down.
 */
export interface DesignerRailProps {
  tool: ToolId;
  /** How many anchors the selected path has (from the engine's overlay). */
  anchors: number;
  /** How many segments the draft has, or `null` when nothing is being drawn. */
  segments: number | null;
  /** The vertex the white arrow owns, in the designer's language. */
  selected: string;
  status: string;
}

export function DesignerRail({ tool, anchors, segments, selected, status }: DesignerRailProps) {
  const spec = TOOLS.find((candidate) => candidate.id === tool);
  return (
    <section className="panel designer-rail" data-testid="designer-rail">
      <h2>
        {spec?.label ?? 'Drawing'}{' '}
        <span className="sub">the math is hidden while you draw</span>
      </h2>
      <p className="tool-blurb">{spec?.blurb}</p>
      <ul className="designer-facts">
        <li>
          <span className="fact-k">anchors</span>
          <span className="fact-v">{anchors}</span>
        </li>
        {segments !== null ? (
          <li>
            <span className="fact-k">segments in progress</span>
            <span className="fact-v">{segments}</span>
          </li>
        ) : null}
        {tool === 'direct' ? (
          <li>
            <span className="fact-k">selection</span>
            <span className="fact-v">{selected}</span>
          </li>
        ) : null}
      </ul>
      <p className="tool-status-line" data-testid="designer-status">
        {status}
      </p>
      <p className="empty">
        Parameters, expressions, constraints and the event log are hidden while a
        drawing tool is held (RULE 4). Switch to <strong>Select</strong> (V) to get
        them back.
      </p>
    </section>
  );
}
