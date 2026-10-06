/**
 * **The tool palette's model** (Task 10.1).
 *
 * Four tools, and one of them (`select`) is the Task 3.2 drag tool the app
 * already had — the designer surface is additive, not a replacement. What lives
 * here is the part that can be *wrong*: which tool a keystroke selects, what a
 * tool's cursor is, and — the one with a requirement attached — **when the
 * parametric UI must disappear**.
 *
 * RULE 4 is blunt about it: *"during drawing, hide all parametric variables,
 * expression panels and constraint logs — show only the tool cursor, the path
 * being drawn, and the Bézier handles. The math stays hidden."* So the gate
 * below is not a cosmetic preference; it is the rule, expressed once, as a total
 * function over the session state, so no panel can be forgotten in the JSX.
 */

/** The tools a user can hold. `select` is Task 3.2's drag tool. */
export type ToolId = 'select' | 'direct' | 'pen' | 'brush';

export interface ToolSpec {
  id: ToolId;
  /** What the palette button says. */
  label: string;
  /** The keyboard shortcut, as Illustrator's own conventions have it. */
  shortcut: string;
  /** The one-line explanation under the palette. */
  blurb: string;
  /** The CSS cursor while this tool is active. */
  cursor: string;
  /** Does this tool draw or edit paths directly? (RULE 4's gate.) */
  direct: boolean;
}

export const TOOLS: ToolSpec[] = [
  {
    id: 'select',
    label: 'Select',
    shortcut: 'V',
    blurb: 'Move nodes. Every gesture is a BeginDrag/UpdateDrag/EndDrag triad.',
    cursor: 'default',
    direct: false,
  },
  {
    id: 'direct',
    label: 'Direct Selection',
    shortcut: 'A',
    blurb: 'The white arrow: click an anchor to own that point, drag a handle to shape the curve.',
    cursor: 'crosshair',
    direct: true,
  },
  {
    id: 'pen',
    label: 'Pen',
    shortcut: 'P',
    blurb: 'Click for a corner, drag for a smooth point, Alt-drag to break the handles, click the first point to close.',
    cursor: 'crosshair',
    direct: true,
  },
  {
    id: 'brush',
    label: 'Brush',
    shortcut: 'B',
    blurb: 'Draw freehand. Hold still on a rough closed shape and it snaps to a circle or a rectangle.',
    cursor: 'crosshair',
    direct: true,
  },
];

const BY_ID = new Map(TOOLS.map((tool) => [tool.id, tool]));

export function toolSpec(id: ToolId): ToolSpec {
  const spec = BY_ID.get(id);
  if (!spec) throw new Error(`unknown tool ${id}`);
  return spec;
}

/** Tools that draw or edit geometry directly — the ones RULE 4 hides the math for. */
export const DIRECT_TOOLS: ToolId[] = TOOLS.filter((tool) => tool.direct).map(
  (tool) => tool.id,
);

export function isDirectTool(tool: ToolId): boolean {
  return toolSpec(tool).direct;
}

export function toolCursor(tool: ToolId): string {
  return toolSpec(tool).cursor;
}

/**
 * The keystroke → tool mapping, and nothing else: one unmodified letter, no
 * chords. `Escape`/`Enter` are *gestures* (finish the path), not tools, so they
 * are handled by the draw session rather than here.
 */
export function toolForShortcut(key: string): ToolId | null {
  const upper = key.toUpperCase();
  for (const tool of TOOLS) {
    if (tool.shortcut === upper) return tool.id;
  }
  return null;
}

/**
 * **RULE 4's gate.** `true` means the parametric panels (variables, expressions,
 * constraints, procedural nodes, the dependency graph and the event log) may be
 * on screen.
 *
 * "During drawing" is read as *while a drawing tool is held*, for a reason: the
 * panels are React state, and mounting them on the down-stroke and unmounting
 * them on the up-stroke would make the side column flash on every click of a
 * pen path. Holding the tool is the unit of intent a designer actually has —
 * they pick up the pen, they draw, they put it down — and it is the reading that
 * makes the rule testable.
 *
 * `select` deliberately keeps its panels. It is the *parametric* tool: hiding
 * the constraint list while dragging a constrained node would delete the Task
 * 3.2 demonstration (drag a partner and watch the solver write the other one).
 * The rule names drawing, and drawing is what the three direct tools do.
 */
export function showsMathPanels(tool: ToolId): boolean {
  return !isDirectTool(tool);
}

/** The strip that replaces the panels while the math is hidden (RULE 4). */
export function designerHint(tool: ToolId): string {
  switch (tool) {
    case 'pen':
      return 'Pen — click for a corner, drag for a smooth point, Alt-drag for independent handles, click the first point to close, Enter/Esc to finish.';
    case 'brush':
      return 'Brush — draw freehand; hold still on a rough circle or box to snap it to the primitive.';
    case 'direct':
      return 'Direct Selection — click a path, click an anchor to own it, hover to reveal its handles, drag a handle to shape the curve.';
    default:
      return 'Select — drag a node; the engine solves the constraints as you go.';
  }
}
