/**
 * **The Properties panel** (Task 13.0 RULE 1) — the right panel's contextual face.
 *
 * RULE 1 says the right side "defaults to a clean Layers panel" and *switches*
 * to Properties when an object is selected, showing only what is relevant:
 * **Transform** (X, Y, W, H), **Appearance** (fill, stroke, width) and
 * **Geometry** (corner, offset). This is that switch's destination.
 *
 * Three implementation decisions worth recording:
 *
 * 1. **The appearance stack is not reimplemented.** `AppearancePanel` already
 *    does stacked fills and strokes, blend modes, opacity and the gradient bar,
 *    and it is the *only* place that writes `SetAppearances`. Properties embeds
 *    it rather than growing a second, simpler fill/stroke control that would
 *    disagree with it the first time a gradient is involved. RULE 1's "Fill,
 *    Stroke, Width" is satisfied by the panel that already owns them, one tab
 *    away from Layers instead of always on screen.
 * 2. **Transform rows are the engine's own slot names.** `geometrySlots` maps a
 *    box dimension onto a `SetParameter` property (`width`, `radius`, …), and
 *    the row shows the *resolved* value the snapshot carries. A slot driven by a
 *    `$variable` says so, and editing it writes a literal — which is exactly
 *    what every other numeric field in this app does, and why the source badge
 *    is there.
 * 3. **Where the engine has no slot, there is no row.** A path's W/H are not a
 *    number the document holds; a disabled pair of boxes pretending otherwise
 *    would be the CAD-panel smell the brief asked to remove. Geometry shows only
 *    what exists: corner radius on a rectangle, offset as an operation on
 *    anything closed.
 */

import { useEffect, useState } from 'react';
import { CornerDownRight, Move } from 'lucide-react';
import { setFloatParam } from '../engine/commands';
import { geometrySlots, primitiveBox } from '../engine/theme';
import { isParametricSource } from '../engine/panels';
import type { CommandWire, SnapshotNodeWire } from '../engine/wire';

export interface PropertiesPanelProps {
  node: SnapshotNodeWire | null;
  /** The owner's name, so the header can say what is being edited. */
  nodeName: string | null;
  onCommand: (label: string, command: CommandWire) => void;
  /** Geometry · Offset — an operation on the node, not a parameter. */
  onOffset: (value: number) => void;
  disabled?: boolean;
}

/** One resolved numeric row: the engine's number, editable in place. */
function SlotRow({
  label,
  value,
  source,
  step,
  onCommit,
  disabled,
  testId,
}: {
  label: string;
  value: number | null;
  source?: string;
  step: number;
  onCommit: (value: number) => void;
  disabled?: boolean;
  testId: string;
}) {
  const [draft, setDraft] = useState<string>('');
  useEffect(() => {
    setDraft(value === null ? '' : String(Number(value.toFixed(2))));
  }, [value]);
  const parametric = isParametricSource(source);
  return (
    <label className="slot-row" title={parametric ? `driven by ${source}` : label}>
      <span className="slot-label">{label}</span>
      <input
        className="slot-input"
        type="number"
        step={step}
        value={draft}
        disabled={disabled || value === null}
        data-testid={testId}
        aria-label={`${label} of the selection`}
        onChange={(event) => setDraft(event.target.value)}
        onBlur={() => {
          const parsed = Number.parseFloat(draft);
          if (Number.isFinite(parsed) && parsed !== value) onCommit(parsed);
          else setDraft(value === null ? '' : String(Number(value.toFixed(2))));
        }}
        onKeyDown={(event) => {
          if (event.key === 'Enter') (event.target as HTMLInputElement).blur();
          if (event.key === 'Escape') setDraft(value === null ? '' : String(value));
        }}
      />
      {parametric ? (
        <span className="slot-link" title={`This number is driven by ${source}`}>
          ƒ
        </span>
      ) : null}
    </label>
  );
}

export default function PropertiesPanel({
  node,
  nodeName,
  onCommand,
  onOffset,
  disabled,
}: PropertiesPanelProps) {
  const [offset, setOffset] = useState('4');

  if (!node) {
    return (
      <section className="panel properties-panel" data-testid="properties-panel">
        <p className="hint" data-testid="properties-empty">
          Select an object to see its properties.
        </p>
      </section>
    );
  }

  const slots = geometrySlots(node);
  const box = slots?.box ?? primitiveBox(node.primitive);
  // The engine's resolved position wins when it has one: it is the value the
  // solver, an expression or a spring produced, not a re-derivation here.
  const x = node.position?.x ?? box?.x ?? null;
  const y = node.position?.y ?? box?.y ?? null;
  const w = box?.w ?? null;
  const h = box?.h ?? null;
  const isGroup = node.primitive.type === undefined;

  const write = (property: string, value: number) =>
    onCommand(`Set ${property}`, setFloatParam(node.id, property, { Literal: value }));

  return (
    <section className="panel properties-panel" data-testid="properties-panel">
      <header className="panel-head">
        <Move size={15} strokeWidth={1.75} aria-hidden="true" />
        <h2>{nodeName ?? 'Properties'}</h2>
      </header>

      {!isGroup && slots ? (
        <>
          <div className="panel-section" data-testid="properties-transform">
            <span className="section-label">Transform</span>
            <div className="slot-grid">
              <SlotRow
                label="X"
                value={x}
                source={node.position?.x_source}
                step={1}
                testId="prop-x"
                disabled={disabled}
                onCommit={(value) => write(slots.x.property, value)}
              />
              <SlotRow
                label="Y"
                value={y}
                source={node.position?.y_source}
                step={1}
                testId="prop-y"
                disabled={disabled}
                onCommit={(value) => write(slots.y.property, value)}
              />
              {slots.w && w !== null ? (
                <SlotRow
                  label="W"
                  value={w}
                  step={1}
                  testId="prop-w"
                  disabled={disabled}
                  onCommit={(value) => write(slots.w!.property, value * slots.w!.scale)}
                />
              ) : null}
              {slots.h && h !== null ? (
                <SlotRow
                  label="H"
                  value={h}
                  step={1}
                  testId="prop-h"
                  disabled={disabled}
                  onCommit={(value) => write(slots.h!.property, value * slots.h!.scale)}
                />
              ) : null}
            </div>
          </div>

          {slots.corner ? (
            <div className="panel-section" data-testid="properties-geometry">
              <span className="section-label">Geometry</span>
              <div className="slot-grid">
                <SlotRow
                  label="Corner"
                  value={cornerOf(node)}
                  step={1}
                  testId="prop-corner"
                  disabled={disabled}
                  onCommit={(value) => write(slots.corner!, value)}
                />
              </div>
              <div className="slot-row">
                <span className="slot-label">Offset</span>
                <input
                  className="slot-input"
                  type="number"
                  step={0.5}
                  value={offset}
                  disabled={disabled}
                  data-testid="prop-offset"
                  aria-label="Offset amount"
                  onChange={(event) => setOffset(event.target.value)}
                />
                <button
                  type="button"
                  className="slot-action"
                  data-testid="prop-offset-apply"
                  disabled={disabled}
                  title="Grow or shrink the outline — a live operation, not a permanent change"
                  onClick={() => onOffset(Number.parseFloat(offset) || 0)}
                >
                  <CornerDownRight size={13} strokeWidth={1.75} aria-hidden="true" />
                  <span>Apply</span>
                </button>
              </div>
            </div>
          ) : (
            <div className="panel-section" data-testid="properties-geometry">
              <span className="section-label">Geometry</span>
              <div className="slot-row">
                <span className="slot-label">Offset</span>
                <input
                  className="slot-input"
                  type="number"
                  step={0.5}
                  value={offset}
                  disabled={disabled}
                  data-testid="prop-offset"
                  aria-label="Offset amount"
                  onChange={(event) => setOffset(event.target.value)}
                />
                <button
                  type="button"
                  className="slot-action"
                  data-testid="prop-offset-apply"
                  disabled={disabled}
                  onClick={() => onOffset(Number.parseFloat(offset) || 0)}
                >
                  <CornerDownRight size={13} strokeWidth={1.75} aria-hidden="true" />
                  <span>Apply</span>
                </button>
              </div>
            </div>
          )}
        </>
      ) : (
        <p className="hint" data-testid="properties-group">
          A group has no numbers of its own. Select one of its objects to edit
          position, size or paint.
        </p>
      )}
    </section>
  );
}

/** The rectangle's rounded-corner radius as the wire reports it. */
function cornerOf(node: SnapshotNodeWire): number | null {
  return node.primitive.type === 'rect' ? node.primitive.corner_radius : null;
}
