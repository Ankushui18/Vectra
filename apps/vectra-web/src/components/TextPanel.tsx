/**
 * **The Text Panel** (Task 11.0 RULE 1, plus the RULE 2 / RULE 3 buttons).
 *
 * ```text
 *   ┌ Text · Headline ─────────────────────────── left │ center │ right ┐
 *   │  "Launch day"                                                     │
 *   │  Font ▾ Vectra Sans        Size 32  ($scale)  Spacing 0           │
 *   │  Leading 1.2                                                      │
 *   │  ── on path ● ──  offset ▓▓▓▓▓░░░░░  40   [Outline]               │
 *   └───────────────────────────────────────────────────────────────────┘
 * ```
 *
 * The three numbers are **parameters**, not fields: the panel sends the same
 * `SetParameter` write a rectangle's width gets, which is what makes a
 * `$variable` or an `ƒexpression` bound to `font_size` re-lay the run out on
 * the next settle with no typographic plumbing anywhere. The `*_source` tags
 * the engine sends are shown as a marker on the label, so a designer can see
 * that a slider is driving a variable before they drag it.
 *
 * The **Outline** button is RULE 3's face in the UI: it calls the boundary's
 * `outline_text`, which shapes the run and dispatches `OutlineText` — new
 * letterform nodes, the type hidden rather than deleted, one undo entry.
 */

import { useState } from 'react';
import {
  lit,
  setFloatParam,
  setFontFamily,
  setText,
  setTextAlignment,
} from '../engine/commands';
import {
  ALIGNMENT_GLYPH,
  fontChoices,
  fontLabel,
  isParametricSource,
  offsetFromSlider,
  sliderFromOffset,
  TEXT_ALIGNMENTS,
} from '../engine/panels';
import type { CommandWire, SnapshotNodeWire, TextAlignmentWire } from '../engine/wire';

export interface TextPanelProps {
  node: SnapshotNodeWire | null;
  /** The families the engine can resolve (the picker's options). */
  fonts: readonly string[];
  /** The bound path's arc length, when the run follows one — the offset
   *  slider's track. `0` (or absent) spans a default track. */
  pathLength?: number;
  onCommand: (label: string, command: CommandWire) => void;
  onOutline: (nodeId: string) => void;
  onBind: (nodeId: string, pathId: string) => void;
  onUnbind: (nodeId: string) => void;
  disabled?: boolean;
}

/** Which slot a numeric field writes, for the label's source marker. */
function sourceOf(source: string | undefined): string | null {
  return isParametricSource(source) ? (source ?? null) : null;
}

export function TextPanel({
  node,
  fonts,
  pathLength = 0,
  onCommand,
  onOutline,
  onBind,
  onUnbind,
  disabled,
}: TextPanelProps) {
  const [draft, setDraft] = useState<string | null>(null);
  const text = node?.text;
  if (!node || !text) return null;

  const run = node.primitive.type === 'text' ? node.primitive : null;
  const choices = fontChoices(fonts, text.font_family);
  const bound = Boolean(text.bound_to);
  const offset = text.offset ?? 0;

  const commitText = (value: string) => {
    setDraft(null);
    if (value !== text.text) onCommand('Type', setText(node.id, value));
  };

  return (
    <section className="panel text-panel" data-testid="text-panel">
      <h2>
        Text
        <span className="sub">{node.name}</span>
        <span className="text-align">
          {TEXT_ALIGNMENTS.map((alignment) => (
            <button
              key={alignment}
              className={`mini${text.alignment === alignment ? ' on' : ''}`}
              data-testid={`text-align-${alignment}`}
              disabled={disabled}
              title={`Align ${alignment}`}
              onClick={() =>
                onCommand('Align text', setTextAlignment(node.id, alignment as TextAlignmentWire))
              }
            >
              {ALIGNMENT_GLYPH[alignment]}
            </button>
          ))}
        </span>
      </h2>

      <label className="field text-string">
        <span className="visually-hidden">Text</span>
        <input
          data-testid="text-input"
          value={draft ?? text.text}
          placeholder="Type something"
          disabled={disabled}
          onChange={(event) => setDraft(event.target.value)}
          onBlur={(event) => commitText(event.target.value)}
          onKeyDown={(event) => {
            if (event.key === 'Enter' && !event.shiftKey) {
              event.preventDefault();
              commitText((event.target as HTMLInputElement).value);
            }
            if (event.key === 'Escape') setDraft(null);
          }}
        />
      </label>

      <div className="btn-row">
        <label className="field">
          <span>Font</span>
          <select
            data-testid="text-font"
            value={text.font_family}
            disabled={disabled}
            title={fontLabel(text.font_family, fonts)}
            onChange={(event) =>
              onCommand('Font family', setFontFamily(node.id, event.target.value))
            }
          >
            {choices.map((family) => (
              <option key={family} value={family}>
                {family}
              </option>
            ))}
          </select>
        </label>

        <label className="field">
          <span>
            Size
            {sourceOf(text.font_size_source) && (
              <em className="param-src" data-testid="text-size-source">
                {text.font_size_source}
              </em>
            )}
          </span>
          <input
            type="number"
            data-testid="text-size"
            value={Number(text.font_size.toFixed(2))}
            min={1}
            max={512}
            step={1}
            disabled={disabled}
            onChange={(event) =>
              onCommand(
                'Font size',
                setFloatParam(node.id, 'font_size', lit(Number(event.target.value) || 1)),
              )
            }
          />
        </label>
      </div>

      <div className="btn-row">
        <label className="field">
          <span>
            Spacing
            {sourceOf(text.letter_spacing_source) && (
              <em className="param-src">{text.letter_spacing_source}</em>
            )}
          </span>
          <input
            type="number"
            data-testid="text-spacing"
            value={Number(text.letter_spacing.toFixed(2))}
            step={0.5}
            disabled={disabled}
            onChange={(event) =>
              onCommand(
                'Letter spacing',
                setFloatParam(node.id, 'letter_spacing', lit(Number(event.target.value) || 0)),
              )
            }
          />
        </label>

        <label className="field">
          <span>
            Leading
            {sourceOf(text.line_height_source) && (
              <em className="param-src">{text.line_height_source}</em>
            )}
          </span>
          <input
            type="number"
            data-testid="text-leading"
            value={Number(text.line_height.toFixed(2))}
            min={0}
            step={0.1}
            disabled={disabled}
            onChange={(event) =>
              onCommand(
                'Line height',
                setFloatParam(node.id, 'line_height', lit(Number(event.target.value) || 0)),
              )
            }
          />
        </label>
      </div>

      <div className="btn-row text-path-row">
        {bound ? (
          <>
            <span className="text-bound" data-testid="text-bound">
              on path
            </span>
            <input
              type="range"
              className="text-offset"
              data-testid="text-offset"
              min={0}
              max={1}
              step={0.005}
              value={sliderFromOffset(offset, pathLength)}
              disabled={disabled}
              title="Slide the run along the path"
              onChange={(event) =>
                onCommand(
                  'Path offset',
                  setFloatParam(
                    node.id,
                    'path_offset',
                    lit(offsetFromSlider(Number(event.target.value), pathLength)),
                  ),
                )
              }
            />
            <output data-testid="text-offset-value">{Number(offset.toFixed(1))}</output>
            <button
              className="mini"
              data-testid="text-unbind"
              disabled={disabled}
              title="Let the text return to its own baseline"
              onClick={() => onUnbind(node.id)}
            >
              Unbind
            </button>
          </>
        ) : (
          <button
            className="mini"
            data-testid="text-bind"
            disabled={disabled}
            title="Bind to the selected path (select a path, then the text, then this)"
            onClick={() => {
              const pathId = prompt('Path node id to bind to');
              if (pathId) onBind(node.id, pathId.trim());
            }}
          >
            Bind to path
          </button>
        )}
        <button
          className="mini"
          data-testid="text-outline"
          disabled={disabled || (run !== null && run.glyphs === 0)}
          title="Convert to editable letterform paths (the text node is hidden, not deleted)"
          onClick={() => onOutline(node.id)}
        >
          Outline
        </button>
      </div>

      {run && (
        <p className="sub text-metrics" data-testid="text-metrics">
          {run.glyphs} glyph{run.glyphs === 1 ? '' : 's'}
          {run.lines > 1 ? ` · ${run.lines} lines` : ''} · {Math.round(run.width)}×
          {Math.round(run.height)}
        </p>
      )}
    </section>
  );
}

export default TextPanel;
