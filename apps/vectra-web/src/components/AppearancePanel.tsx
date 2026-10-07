/**
 * **The Appearance Panel** (Task 10.2 RULE 3) — stacked fills and strokes, blend
 * modes, per-layer opacity, and a gradient bar with draggable stops.
 *
 * ```text
 *   ┌ Appearance · badge ─────────────────── + fill  + stroke ┐
 *   │  ⬤ ▨ fill      ◐ multiply   ▓▓▓▓▓░░ 100%   👁          │  ← blend is an icon
 *   │  ⬤ ─ stroke 12 ◐ screen     ▓▓▓░░░░  80%   👁          │  ← width on the row
 *   │  ⬤ ─ stroke  4 ◐ normal     ▓▓▓▓▓▓▓ 100%   👁          │
 *   │  ░░ gradient bar with two draggable stops ░░            │
 *   └──────────────────────────────────────────────────────────┘
 * ```
 *
 * **RULE 4's other half:** this panel is *only* on screen when exactly one
 * unlocked object is selected (`showsAppearancePanel`), and the controls are
 * icon-based — the blend modes are a row of glyph buttons with names in their
 * tooltips, not a `<select>` of words; the gradient is a bar you drag, not four
 * number fields.
 *
 * The panel edits the **resolved** stack the engine sent and returns the whole
 * stack as one `SetAppearances` command. One gesture, one undo entry, and no
 * parameter resolution anywhere in the UI: the resolved values *are* the truth
 * about what is on screen.
 */

import { useState } from 'react';
import { setAppearances } from '../engine/commands';
import {
  addFill,
  addStroke,
  appearanceCaption,
  appearanceStack,
  BLEND_MODES,
  blendLabel,
  gradientCss,
  moveLayer,
  moveStop,
  recolorStop,
  removeLayerAt,
  stackToWire,
  swatchColor,
  togglePaint,
  updateLayerAt,
} from '../engine/panels';
import type { SpanRow } from '../engine/draw/regions';
import type { SnapshotAppearanceWire, SnapshotNodeWire, SnapshotPaintWire } from '../engine/wire';
import type { CommandWire } from '../engine/wire';

export interface AppearancePanelProps {
  node: SnapshotNodeWire | null;
  onCommand: (label: string, command: CommandWire) => void;
  disabled?: boolean;
  /**
   * **Task 12.0 RULE 3**: the spans of this node's outline, as the engine's
   * region graph measured them — empty when nothing crosses the path, which is
   * exactly when there is nothing to break.
   *
   * The rows come from the plan, so this panel formats no arc lengths of its own.
   */
  spans?: SpanRow[];
  /**
   * *Break Path at Intersections*: split the path at the crossings. The rows
   * carry the engine's own `[from, to]` pairs back to it — the panel chooses
   * *which* cuts, never *where* they are.
   */
  onBreakPath?: (rows: SpanRow[]) => void;
}

/** The glyph each blend mode shows: a filled circle in the mode's own idiom. */
const BLEND_GLYPH: Record<string, string> = {
  normal: '◑',
  multiply: '◐',
  screen: '◒',
  overlay: '◓',
};

export function AppearancePanel({
  node,
  onCommand,
  disabled,
  spans = [],
  onBreakPath,
}: AppearancePanelProps) {
  const [openBlend, setOpenBlend] = useState<number | null>(null);
  const [dragRow, setDragRow] = useState<number | null>(null);
  const [dragStop, setDragStop] = useState<{ row: number; stop: number } | null>(null);

  if (!node) return null;

  const stack = appearanceStack(node);
  const apply = (label: string, next: SnapshotAppearanceWire[]) => {
    onCommand(label, setAppearances(node.id, stackToWire(next)));
  };

  const onBarPointer = (
    row: number,
    index: number,
    paint: Extract<SnapshotPaintWire, { type: 'linear' | 'radial' }>,
    element: HTMLElement,
    clientX: number,
  ) => {
    const box = element.getBoundingClientRect();
    if (box.width <= 0) return;
    const offset = Math.max(0, Math.min(1, (clientX - box.left) / box.width));
    const stops = moveStop(paint.stops, index, offset);
    const next = updateLayerAt(stack, row, {
      paint: { ...paint, stops } as SnapshotPaintWire,
    });
    apply('Move gradient stop', next);
  };

  return (
    <section className="panel appearance-panel" data-testid="appearance-panel">
      <h2>
        Appearance
        <span className="sub">{node.name}</span>
        <span className="appearance-add">
          <button
            className="mini"
            data-testid="appearance-add-fill"
            disabled={disabled}
            title="Add a fill layer (top of the stack)"
            onClick={() => apply('Add fill', addFill(stack))}
          >
            + fill
          </button>
          <button
            className="mini"
            data-testid="appearance-add-stroke"
            disabled={disabled}
            title="Add a stroke layer (drawn on top)"
            onClick={() => apply('Add stroke', addStroke(stack))}
          >
            + stroke
          </button>
        </span>
      </h2>

      {node.primitive.type === 'path' && onBreakPath && (
        <div className="region-block" data-testid="region-break">
          <button
            className="mini"
            data-testid="break-path"
            disabled={disabled || spans.length === 0}
            title={
              spans.length === 0
                ? 'Nothing crosses this path — there is no intersection to break at'
                : 'Split this path at every intersection with the shapes around it'
            }
            onClick={() => onBreakPath(spans)}
          >
            ✂ Break Path at Intersections
          </button>
          {spans.length === 0 ? (
            <p className="hint" data-testid="region-none">
              no intersections on this path
            </p>
          ) : (
            <ul className="span-list" data-testid="span-list">
              {spans.map((row) => (
                <li className="span-row" key={row.index} data-testid={`span-row-${row.index}`}>
                  <span className="span-label">{row.label}</span>
                  <button
                    className="mini"
                    data-testid={`break-span-${row.index}`}
                    disabled={disabled}
                    title="Break this span: two paths, split at its two crossings"
                    onClick={() => onBreakPath([row])}
                  >
                    break
                  </button>
                </li>
              ))}
            </ul>
          )}
        </div>
      )}

      <ul className="appearance-list" data-testid="appearance-list">
        {stack.map((row, index) => {
          const paint = row.paint;
          const gradient =
            paint.type === 'solid'
              ? null
              : (paint as Extract<SnapshotPaintWire, { type: 'linear' | 'radial' }>);
          return (
            <li
              key={`${row.kind}-${index}`}
              className={[
                'appearance-row',
                row.visible ? '' : 'hidden',
                dragRow === index ? 'dragging' : '',
              ]
                .filter(Boolean)
                .join(' ')}
              data-testid={`appearance-row-${index}`}
              draggable
              onDragStart={() => setDragRow(index)}
              onDragOver={(event) => event.preventDefault()}
              onDrop={(event) => {
                event.preventDefault();
                if (dragRow !== null && dragRow !== index) {
                  apply('Reorder appearance layers', moveLayer(stack, dragRow, index));
                }
                setDragRow(null);
              }}
            >
              <span className="appearance-grip" title="Drag to restack">
                ⠿
              </span>

              {/* paint kind: solid / linear / radial, all as one control */}
              <span className="paint-kind" role="group" aria-label="Paint kind">
                {(
                  [
                    ['solid', '⬤'],
                    ['linear', '▤'],
                    ['radial', '◎'],
                  ] as const
                ).map(([kind, glyph]) => (
                  <button
                    key={kind}
                    className={`icon ${paint.type === kind ? 'on' : ''}`}
                    data-testid={`appearance-paint-${index}-${kind}`}
                    disabled={disabled}
                    title={
                      kind === 'solid'
                        ? 'Solid colour'
                        : kind === 'linear'
                          ? 'Linear gradient'
                          : 'Radial gradient'
                    }
                    onClick={() => apply(`Paint ${kind}`, updateLayerAt(stack, index, {
                      paint: togglePaint(row, kind).paint,
                    }))}
                  >
                    {glyph}
                  </button>
                ))}
              </span>

              <label
                className="swatch"
                title="Colour"
                style={{ background: swatchColor(paint) }}
              >
                <input
                  type="color"
                  data-testid={`appearance-color-${index}`}
                  disabled={disabled}
                  value={hexInput(swatchColor(paint))}
                  onChange={(event) =>
                    apply(
                      'Appearance colour',
                      updateLayerAt(stack, index, {
                        paint:
                          paint.type === 'solid'
                            ? { type: 'solid', color: event.target.value }
                            : ({
                                ...paint,
                                stops: recolorStop(paint.stops, 0, event.target.value),
                              } as SnapshotPaintWire),
                      }),
                    )
                  }
                />
              </label>

              <span className="appearance-caption" data-testid={`appearance-caption-${index}`}>
                {appearanceCaption(row)}
                {row.kind === 'stroke' && (
                  <input
                    className="width-input"
                    type="number"
                    min={0}
                    step={0.5}
                    value={row.width ?? 1}
                    data-testid={`appearance-width-${index}`}
                    disabled={disabled}
                    onChange={(event) =>
                      apply(
                        'Stroke width',
                        updateLayerAt(stack, index, { width: Number(event.target.value) }),
                      )
                    }
                  />
                )}
              </span>

              {/* blend mode: an icon with the mode's name in its tooltip */}
              <span className="blend">
                <button
                  className="icon blend-icon"
                  data-testid={`appearance-blend-${index}`}
                  disabled={disabled}
                  title={`Blend: ${blendLabel(row.blend)}`}
                  aria-label={`Blend mode: ${blendLabel(row.blend)}`}
                  aria-haspopup="menu"
                  onClick={() => setOpenBlend(openBlend === index ? null : index)}
                >
                  {BLEND_GLYPH[row.blend] ?? '◑'}
                </button>
                {openBlend === index && (
                  <span className="blend-menu" data-testid={`appearance-blend-menu-${index}`}>
                    {BLEND_MODES.map((mode) => (
                      <button
                        key={mode.tag}
                        className={`icon ${row.blend === mode.tag ? 'on' : ''}`}
                        data-testid={`appearance-blend-${index}-${mode.tag}`}
                        title={mode.label}
                        aria-label={`Blend mode: ${mode.label}`}
                        onClick={() => {
                          apply('Blend mode', updateLayerAt(stack, index, { blend: mode.tag }));
                          setOpenBlend(null);
                        }}
                      >
                        {BLEND_GLYPH[mode.tag]} {mode.label}
                      </button>
                    ))}
                  </span>
                )}
              </span>

              <label className="opacity" title="Layer opacity">
                <input
                  type="range"
                  min={0}
                  max={1}
                  step={0.01}
                  value={row.opacity}
                  data-testid={`appearance-opacity-${index}`}
                  disabled={disabled}
                  onChange={(event) =>
                    apply(
                      'Layer opacity',
                      updateLayerAt(stack, index, { opacity: Number(event.target.value) }),
                    )
                  }
                />
                <span className="opacity-value">{Math.round(row.opacity * 100)}%</span>
              </label>

              <button
                className="icon"
                data-testid={`appearance-eye-${index}`}
                disabled={disabled}
                title={row.visible ? 'Hide this layer' : 'Show this layer'}
                aria-pressed={!row.visible}
                onClick={() =>
                  apply('Layer visibility', updateLayerAt(stack, index, { visible: !row.visible }))
                }
              >
                {row.visible ? '👁' : '⃠'}
              </button>

              <button
                className="icon danger"
                data-testid={`appearance-remove-${index}`}
                disabled={disabled || stack.length <= 1}
                title={
                  stack.length <= 1
                    ? 'A shape always paints something — add a layer before removing the last'
                    : 'Remove this layer'
                }
                onClick={() => apply('Remove appearance layer', removeLayerAt(stack, index))}
              >
                ✕
              </button>

              {/* the gradient bar: the ramp, with a handle per stop */}
              {gradient && (
                <div
                  className="gradient-bar"
                  data-testid={`gradient-bar-${index}`}
                  ref={(element) => {
                    if (!element) return;
                    // Pointer capture on the whole bar: a stop drag keeps
                    // tracking outside the element, which is what makes a
                    // gradient feel like a physical object.
                    element.onpointerdown = (event) => {
                      const stopIndex = nearestStop(gradient.stops, element, event.clientX);
                      setDragStop({ row: index, stop: stopIndex });
                      onBarPointer(index, stopIndex, gradient, element, event.clientX);
                      element.setPointerCapture(event.pointerId);
                    };
                    element.onpointermove = (event) => {
                      if (!dragStop || dragStop.row !== index) return;
                      onBarPointer(index, dragStop.stop, gradient, element, event.clientX);
                    };
                    element.onpointerup = () => setDragStop(null);
                  }}
                  style={{ backgroundImage: gradientCss(paint) ?? undefined }}
                >
                  {gradient.stops.map((stop, stopIndex) => (
                    <button
                      key={stopIndex}
                      className="gradient-stop"
                      data-testid={`gradient-stop-${index}-${stopIndex}`}
                      style={{ left: `${stop.offset * 100}%`, background: stop.color }}
                      title={`Stop ${stopIndex + 1}: ${Math.round(stop.offset * 100)}%`}
                      onPointerDown={(event) => {
                        event.stopPropagation();
                        setDragStop({ row: index, stop: stopIndex });
                      }}
                    />
                  ))}
                  <span className="gradient-hint">
                    {paint.type === 'linear' ? 'linear' : 'radial'} · drag a stop
                  </span>
                </div>
              )}
            </li>
          );
        })}
      </ul>

      <p className="hint">
        Fills and strokes draw back → front in this list: a thick stroke under a thinner one is
        two rows, and the one nearer the bottom is the one beneath.
      </p>
    </section>
  );
}

function nearestStop(
  stops: { offset: number }[],
  element: HTMLElement,
  clientX: number,
): number {
  const box = element.getBoundingClientRect();
  const width = box.width || 1;
  const t = (clientX - box.left) / width;
  let best = 0;
  let bestDistance = Number.POSITIVE_INFINITY;
  stops.forEach((stop, index) => {
    const distance = Math.abs(stop.offset - t);
    if (distance < bestDistance) {
      bestDistance = distance;
      best = index;
    }
  });
  return best;
}

/** `<input type="color">` speaks `#rrggbb`; the engine's hex may carry alpha. */
function hexInput(hex: string): string {
  const clean = hex.replace('#', '');
  return `#${clean.slice(0, 6).padEnd(6, '0')}`;
}

export default AppearancePanel;
