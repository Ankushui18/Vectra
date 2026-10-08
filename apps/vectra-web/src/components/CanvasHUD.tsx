/**
 * **The canvas HUD** (Task 13.0 RULE 3 + Task 14.0 RULE 4).
 *
 * A small floating bar that appears next to a selection with:
 * - **RULE 4 quick edits**: Fill colour, Stroke colour, Stroke width — the
 *   controls a designer reaches for *while looking at the artwork*, not at a
 *   panel. This is the "Procreate feel": inline, contextual, always within
 *   arm's reach of the selection.
 * - **RULE 3 actions**: Duplicate, Flip, Rotate, Boolean, More.
 *
 * The colour and width controls are optional props — when the caller passes
 * them, the well renders; when it does not (tests, or a future simplified
 * view), the HUD falls back to the action buttons alone. That keeps the
 * component honest about what the engine has wired up without forcing every
 * mount site to know about appearance state.
 *
 * Two things about the action buttons are worth stating, because both are
 * deliberate and both come from the audit rather than from taste:
 *
 * 1. **It positions itself from the engine's numbers.** `selectionAnchor`
 *    (pure, tested) reads the *resolved* primitive the snapshot already carries
 *    and the caller maps that document point to the screen through the
 *    renderer's own `documentToClient`. This component receives a finished
 *    `{left, top}` in CSS pixels and does no arithmetic beyond centring itself —
 *    which is why it cannot drift from the artwork when the camera pans.
 * 2. **Unavailable actions are shown and explained, not hidden.** Rotate is
 *    disabled because a node has no transform yet (roadmap 1.1) and Boolean is
 *    disabled unless exactly two objects are selected (the engine's booleans are
 *    binary). The reason is in the tooltip. A control that vanishes teaches
 *    nothing; one that says *why* teaches the model.
 */

import {
  Copy,
  FlipHorizontal,
  Combine,
  MoreHorizontal,
  RotateCw,
} from 'lucide-react';
import type { LucideIcon } from 'lucide-react';
import { canvasHud } from '../engine/theme';
import type { HudActionId } from '../engine/theme';

const HUD_ICON: Record<string, LucideIcon> = {
  Copy,
  FlipHorizontal,
  RotateCw,
  Combine,
  MoreHorizontal,
};

export interface CanvasHUDProps {
  /** Where to draw it, in CSS pixels relative to the canvas stage. */
  left: number;
  top: number;
  /** How many objects are selected. */
  selectionCount: number;
  /** The primary selected object's name, for the caption. */
  primaryName: string | null;
  onAction: (action: HudActionId) => void;
  disabled?: boolean;

  /** Task 14.0 RULE 4: the quick-edit well — when provided, the HUD shows
   *  inline fill/stroke colour and stroke width controls. */
  fillColor?: string | null;
  strokeColor?: string | null;
  strokeWidth?: number | null;
  onFillColor?: (color: string) => void;
  onStrokeColor?: (color: string) => void;
  onStrokeWidth?: (width: number) => void;
}

export default function CanvasHUD({
  left,
  top,
  selectionCount,
  primaryName,
  onAction,
  disabled,
  fillColor,
  strokeColor,
  strokeWidth,
  onFillColor,
  onStrokeColor,
  onStrokeWidth,
}: CanvasHUDProps) {
  const model = canvasHud(selectionCount, primaryName);
  if (selectionCount === 0) return null;

  const hasWell = Boolean(onFillColor || onStrokeColor || onStrokeWidth);

  return (
    <div
      className="canvas-hud"
      data-testid="canvas-hud"
      style={{ left, top }}
      role="toolbar"
      aria-label={`Actions for ${model.caption}`}
      // A HUD press must not reach the canvas: without this, every quick action
      // would also drop a selection (`pointerdown` on the canvas clears it).
      onPointerDown={(event) => event.stopPropagation()}
    >
      {/* Task 14.0 RULE 4: the quick-edit well — fill, stroke, width, inline.
          Only rendered when the caller wires them up. */}
      {hasWell && (
        <div className="hud-well" data-testid="hud-well">
          {onFillColor && (
            <label
              className="hud-color"
              data-testid="hud-fill-color"
              title="Fill colour"
              aria-label="Fill colour"
            >
              <span
                className="hud-color-swatch"
                style={{ background: fillColor ?? '#000000' }}
              />
              <input
                type="color"
                value={fillColor ?? '#000000'}
                disabled={disabled}
                onChange={(e) => onFillColor(e.target.value)}
                aria-label="Pick fill colour"
              />
            </label>
          )}
          {onStrokeColor && (
            <label
              className="hud-color"
              data-testid="hud-stroke-color"
              title="Stroke colour"
              aria-label="Stroke colour"
            >
              <span
                className="hud-color-swatch"
                style={{
                  background: strokeColor ?? 'transparent',
                  border: strokeColor ? 'none' : '1.5px dashed var(--muted)',
                }}
              />
              <input
                type="color"
                value={strokeColor ?? '#000000'}
                disabled={disabled}
                onChange={(e) => onStrokeColor(e.target.value)}
                aria-label="Pick stroke colour"
              />
            </label>
          )}
          {onStrokeWidth && (
            <div className="hud-width" data-testid="hud-stroke-width">
              <input
                type="range"
                min={0}
                max={24}
                step={0.5}
                value={strokeWidth ?? 0}
                disabled={disabled}
                aria-label="Stroke width"
                onChange={(e) => onStrokeWidth(Number(e.target.value))}
              />
              <span className="hud-width-value">
                {strokeWidth != null ? strokeWidth : 0}
              </span>
            </div>
          )}
        </div>
      )}

      {hasWell && <span className="hud-well-divider" aria-hidden="true" />}

      {model.actions.map((action) => {
        const Icon = HUD_ICON[action.icon] ?? Copy;
        const unavailable = disabled || !action.enabled;
        return (
          <button
            key={action.id}
            type="button"
            className={`hud-btn${action.id === 'more' ? ' hud-more' : ''}`}
            data-testid={`hud-${action.id}`}
            disabled={unavailable}
            aria-label={action.label}
            title={action.reason ?? action.label}
            onClick={() => onAction(action.id)}
          >
            <Icon size={15} strokeWidth={1.75} aria-hidden="true" />
          </button>
        );
      })}
    </div>
  );
}
