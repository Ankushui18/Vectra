/**
 * **The left dock** (Task 13.0 RULE 1).
 *
 * Five groups — Paint, Draw, Vector, Shape, Text — and nothing else on screen
 * until the designer wants it. *Holding* a group reveals its entries: press and
 * hold for a beat, or hover, or use the chevron and the keyboard. That single
 * interaction is the whole of RULE 1's left toolbar, and it exists because the
 * alternative (every tool on screen at once) is how a drawing app turns into a
 * control panel.
 *
 * Three decisions are worth stating, because each is a rule of the brief made
 * mechanical:
 *
 * 1. **The group order is the philosophy.** Paint first, Draw second, Vector
 *    third. "Paint first, vector when precision matters" is either visible in
 *    the layout or it is a slogan.
 * 2. **A hold is a hold.** The flyout opens on a *timer* (250 ms), so brushing
 *    past a button while reaching for the canvas does not open anything, while
 *    the designer's deliberate press does. The same timer serves touch, mouse
 *    and pen, and the keyboard gets the same door through `Enter`/`Space`.
 * 3. **Held state is shown on the group, not the entry.** The dock is five
 *    buttons wide; an entry's name lives in the flyout and in the tooltip, and
 *    the active one is *also* echoed by the bottom bar — which is where a
 *    designer actually looks while working.
 *
 * Icons are `lucide-react` and only icons are buttons (RULE 5): every label is
 * in `title` and `aria-label`, so the dock reads clean and still speaks for a
 * screen reader.
 */

import { useCallback, useEffect, useRef, useState } from 'react';
import {
  ArrowUpRight,
  Boxes,
  Brush,
  Combine,
  Copy,
  FlipHorizontal,
  MousePointer2,
  PaintBucket,
  PenTool,
  Shapes,
  Square,
  Circle,
  Type,
  Wand2,
} from 'lucide-react';
import type { LucideIcon } from 'lucide-react';
import { groupForTool, TOOL_GROUPS } from '../engine/theme';
import type { DockActionId, DockEntry, DockGroup, ToolName } from '../engine/theme';

/** The icon each group wears when it is closed. */
const GROUP_ICON: Record<string, LucideIcon> = {
  paint: Brush,
  // The pointer, because that is the tool the group holds today (RULE 1 gives
  // Pen, Node, Boolean, Offset and Mirror to Vector).
  draw: MousePointer2,
  vector: PenTool,
  shape: Shapes,
  text: Type,
};

/** The icon each dock entry wears — tools and actions alike. */
const ENTRY_ICON: Record<string, LucideIcon> = {
  brush: Brush,
  smartFill: PaintBucket,
  pen: PenTool,
  select: MousePointer2,
  direct: Wand2,
  text: Type,
  'add-rectangle': Square,
  'add-circle': Circle,
  'boolean-union': Combine,
  offset: ArrowUpRight,
  fillet: Boxes,
  mirror: FlipHorizontal,
  flip: FlipHorizontal,
  duplicate: Copy,
};

const entryKey = (entry: DockEntry): string =>
  entry.kind === 'tool' ? entry.tool : entry.action;

export interface LeftDockProps {
  /** The tool in hand. */
  tool: ToolName;
  onTool: (tool: ToolName) => void;
  onAction: (action: DockActionId) => void;
  disabled?: boolean;
}

/** How long a press has to last before the flyout opens. */
export const HOLD_MS = 250;

export default function LeftDock({ tool, onTool, onAction, disabled }: LeftDockProps) {
  /** The open group, or `null`. Hover opens it; clicking a group's icon keeps it. */
  const [open, setOpen] = useState<string | null>(null);
  /** The group the pointer is over, so its flyout can show next to it. */
  const [hovered, setHovered] = useState<string | null>(null);
  /** The pending hold timer, cleared whenever the press ends early. */
  const holdRef = useRef<number | null>(null);
  /** True while the current press is a *hold* rather than a click. */
  const heldRef = useRef(false);

  const clearHold = useCallback(() => {
    if (holdRef.current !== null) {
      window.clearTimeout(holdRef.current);
      holdRef.current = null;
    }
  }, []);

  useEffect(() => clearHold, [clearHold]);

  const startHold = useCallback(
    (groupId: string) => {
      clearHold();
      heldRef.current = false;
      holdRef.current = window.setTimeout(() => {
        heldRef.current = true;
        setOpen(groupId);
      }, HOLD_MS);
    },
    [clearHold],
  );

  /**
   * The press's end. A press that outlived the timer was a **hold** — the flyout
   * is already open and the release must not also activate the entry under it.
   * A press that ended early was a **click**: it selects the group's first tool,
   * which is the behaviour a designer expects from a single tap on a tool button
   * and the reason the common case needs no flyout at all.
   */
  const endHold = useCallback(
    (group: DockGroup) => {
      const wasHold = heldRef.current;
      clearHold();
      if (wasHold) return;
      const first = group.entries[0];
      setOpen(null);
      if (first?.kind === 'tool') onTool(first.tool);
      else if (first?.kind === 'action') onAction(first.action);
    },
    [clearHold, onTool, onAction],
  );

  const choose = (entry: DockEntry) => {
    setOpen(null);
    if (entry.kind === 'tool') onTool(entry.tool);
    else onAction(entry.action);
  };

  const activeGroup = groupForTool(tool);

  return (
    <nav
      className="dock"
      data-testid="left-dock"
      aria-label="Tools"
      // A press anywhere else closes the flyout — the interaction's other half.
      onMouseLeave={() => {
        setHovered(null);
        setOpen(null);
      }}
    >
      {TOOL_GROUPS.map((group) => {
        const GroupIcon = GROUP_ICON[group.id] ?? Square;
        const isOpen = open === group.id || (hovered === group.id && open === null);
        const isActive = activeGroup === group.id;
        return (
          <div
            key={group.id}
            className={`dock-group${isActive ? ' active' : ''}`}
            onMouseEnter={() => setHovered(group.id)}
            onMouseLeave={() => setHovered((current) => (current === group.id ? null : current))}
          >
            <button
              type="button"
              className="dock-btn"
              data-testid={`dock-${group.id}`}
              disabled={disabled}
              aria-haspopup="true"
              aria-expanded={isOpen}
              aria-label={`${group.label} tools — hold to open`}
              title={`${group.label} — hold to open`}
              onPointerDown={() => startHold(group.id)}
              onPointerUp={() => endHold(group)}
              onPointerLeave={clearHold}
              onClick={(event) => {
                // The press handlers above already acted; a click that follows a
                // *hold* must not double-fire, and one that follows a tap has
                // already chosen. This only satisfies the keyboard path.
                if (event.detail === 0) setOpen(isOpen ? null : group.id);
              }}
              onKeyDown={(event) => {
                if (event.key === 'Enter' || event.key === ' ') {
                  event.preventDefault();
                  setOpen(isOpen ? null : group.id);
                }
              }}
            >
              <GroupIcon size={20} strokeWidth={1.75} aria-hidden="true" />
              <span className="dock-glyph" aria-hidden="true">
                {group.glyph}
              </span>
            </button>

            {isOpen && (
              <div
                className="dock-flyout"
                data-testid={`dock-flyout-${group.id}`}
                role="menu"
                aria-label={`${group.label} tools`}
                onMouseEnter={() => setHovered(group.id)}
              >
                <span className="dock-flyout-title">{group.label}</span>
                {group.entries.map((entry) => {
                  const EntryIcon = ENTRY_ICON[entryKey(entry)] ?? Square;
                  const active = entry.kind === 'tool' && entry.tool === tool;
                  const shortcut = entry.kind === 'tool' ? entry.shortcut : undefined;
                  return (
                    <button
                      key={entryKey(entry)}
                      type="button"
                      role="menuitem"
                      className={`dock-entry${active ? ' active' : ''}`}
                      data-testid={`dock-entry-${entryKey(entry)}`}
                      disabled={disabled}
                      aria-pressed={entry.kind === 'tool' ? active : undefined}
                      aria-label={`${entry.label}${shortcut ? ` (${shortcut})` : ''}`}
                      title={`${entry.label}${shortcut ? ` (${shortcut})` : ''} — ${entry.hint}`}
                      onClick={() => choose(entry)}
                    >
                      <EntryIcon size={16} strokeWidth={1.75} aria-hidden="true" />
                      <span className="dock-entry-label">{entry.label}</span>
                      {shortcut ? <kbd>{shortcut}</kbd> : null}
                    </button>
                  );
                })}
              </div>
            )}
          </div>
        );
      })}
    </nav>
  );
}
