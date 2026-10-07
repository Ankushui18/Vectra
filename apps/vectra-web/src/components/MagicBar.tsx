/**
 * **Make Magic** — the ⌘K command bar (Task 10.6, RULE 2 + RULE 4).
 *
 * The whole interface to the AI layer is one text field and a row of chips. The
 * chips are *engine data* (`structural_macros`), not a UI list, so a macro the
 * planner learns about shows up here without a frontend change; the hint under
 * the field is the engine's own sentence about what "this" currently means.
 *
 * What is deliberately absent is as important as what is here: there is no
 * preview of JSON, no plan inspector, no raw command list. What comes back from
 * a run is `magicReceipt` — the engine's one-line sentence — and it is printed
 * verbatim. A designer reads "✨ Applied 3 constraints and unified the shape.",
 * never a `Vec<Command>`.
 *
 * Dumb view, like every other panel: the prompt text and the focus live here
 * (they are the widget's business), and every intent leaves through a callback.
 * The bar focuses itself when it opens, which is why `App` no longer needs a ref
 * into the JSX it hosts.
 */
import { useEffect, useRef } from 'react';

import type { StructuralMacroWire } from '../engine/wire';

export interface MagicBarProps {
  /** Mounted or not. `false` renders nothing at all, not a hidden bar. */
  open: boolean;
  /** The engine is loaded and the document is stable. */
  ready: boolean;
  /** The prompt being typed. */
  text: string;
  onText: (value: string) => void;
  /** The engine's prose for the selection — what "this" means right now. */
  selectionProse: string;
  /** The macros the engine publishes (`structural_macros`). */
  macros: StructuralMacroWire[];
  /** The engine's sentence for the last run, or `null`. */
  receipt: string | null;
  onRun: (prompt: string) => void;
  onClose: () => void;
}

export function MagicBar({
  open,
  ready,
  text,
  onText,
  selectionProse,
  macros,
  receipt,
  onRun,
  onClose,
}: MagicBarProps) {
  const input = useRef<HTMLInputElement>(null);
  // Focus on open: the bar is summoned by a keystroke, so the designer's hands
  // are already on the keyboard and the next thing they type is the prompt.
  useEffect(() => {
    if (open) input.current?.focus();
  }, [open]);

  if (!open) return null;

  const run = () => {
    onRun(text);
    onText('');
  };

  return (
    <div
      className="magic-backdrop"
      data-testid="magic-backdrop"
      onClick={(event) => {
        if (event.target === event.currentTarget) onClose();
      }}
    >
      <div className="magic-bar" role="dialog" aria-label="Make Magic">
        <div className="magic-row">
          <span className="magic-spark" aria-hidden="true">
            ✨
          </span>
          <input
            ref={input}
            className="magic-input"
            data-testid="magic-input"
            placeholder="Make this geometric, create 4 color variations, align perfectly…"
            value={text}
            onChange={(event) => onText(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === 'Enter') run();
            }}
            aria-label="Make Magic prompt"
          />
          <button data-testid="magic-run" disabled={!ready || !text.trim()} onClick={run}>
            Make Magic
          </button>
        </div>
        {/* What "this" means, in the engine's words (RULE 2's subject). */}
        <p className="magic-hint" data-testid="magic-selection">
          {selectionProse || 'select artwork, then describe what it should become'}
        </p>
        <div className="magic-chips">
          {macros.map((macro) => (
            <button
              key={macro.prompt}
              className="magic-chip"
              data-testid={`magic-chip-${macro.label.toLowerCase().replace(/\s+/g, '-')}`}
              title={macro.hint}
              onClick={() => {
                onText(macro.prompt);
                input.current?.focus();
              }}
            >
              {macro.label}
            </button>
          ))}
        </div>
        {receipt ? (
          <p className="magic-prose" data-testid="magic-receipt">
            {receipt}
          </p>
        ) : null}
        <p className="magic-foot">
          The AI edits the document's structure — constraints, parameters, operations. It
          never draws pixels. ⌘K opens this bar; Enter runs it.
        </p>
      </div>
    </div>
  );
}

export default MagicBar;
