/**
 * **The canvas HUD** (Task 13.0 RULE 3).
 *
 * A small floating bar that appears next to a selection with the actions a
 * designer reaches for while looking at the artwork rather than at a panel:
 * Duplicate, Flip, Rotate, Boolean, More.
 *
 * Two things about it are worth stating, because both are deliberate and both
 * come from the audit rather than from taste:
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
 *
 * The `More` action opens the right panel's Properties tab rather than a popover
 * menu of its own: there is exactly one place in this app where an object's
 * numbers live, and a second one would be a second truth.
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
}

export default function CanvasHUD({
  left,
  top,
  selectionCount,
  primaryName,
  onAction,
  disabled,
}: CanvasHUDProps) {
  const model = canvasHud(selectionCount, primaryName);
  if (selectionCount === 0) return null;
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
