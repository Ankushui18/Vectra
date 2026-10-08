/**
 * **The Artist-First shell's model** (Task 13.0).
 *
 * Task 13.0 is a UI overhaul, and the way this file keeps that from becoming a
 * rewrite of the *engine bindings* is the same trick Task 10.1 used for the
 * drawing suite: the decisions are pure functions over the snapshot, unit-tested
 * without a DOM, and the components only render what they return.
 *
 * What lives here, and why each one is here rather than in a `.tsx`:
 *
 * * **`THEME`** — the palette as data. The CSS variables in `App.css` are
 *   generated from the same numbers, and the swatch code (a colour well, a
 *   selection ring) needs them in TypeScript. One constant, two consumers.
 * * **`toolGroups`** — RULE 1's left dock: five groups, and the entries that
 *   holding one reveals (Vector's five — Pen, Node, Boolean, Offset, Mirror —
 *   are the brief's list verbatim). The *dock* is a model, not a list of buttons
 *   in JSX.
 * * **`layerType`** — RULE 2's type indicators (`◉ ◇ ▣ ⌁ T`). Which glyph a row
 *   wears is a function of what the engine says the row *is*, never of its name.
 * * **`bottomBarMode`** — RULE 1's dynamic bottom toolbar: one of four states,
 *   decided by the tool in hand and the selection, so the bar cannot show Brush
 *   controls over a path.
 * * **`focusMode` / `edgeReveal`** — RULE 3's Tab key and the soft edge reveal,
 *   as total functions so the shell has no conditional chrome of its own.
 * * **`selectionAnchor`** — where RULE 3's canvas HUD floats: the top-centre of
 *   the selection, in **document** units, for the renderer to project. `null`
 *   when the engine's own numbers do not describe a box (a bare path, a group),
 *   which is what makes the HUD's fallback honest rather than a guess.
 *
 * Nothing in this file computes geometry. Every number it reads is a resolved
 * value the engine already put on the wire, and every number it *returns* is
 * either a slot name for a command or a position for the renderer to map.
 */

import type { SnapshotNodeWire, SnapshotPrimitiveWire, SnapshotWire } from './wire';

// ── RULE 5: the professional dark theme ────────────────────────────────────
//
// The five greys are the whole surface palette. They are named for what they
// *are* rather than for where they are used, because the same `#252526` is the
// hover state of a dock button, a panel row and a bottom-bar control — and a
// theme whose tokens are named after one of its uses always ends up with a
// second, subtly different grey for the next one.

export const THEME = {
  /** The canvas, and the canvas only — everything else is a panel. */
  canvas: '#121212',
  panel: '#1e1e1e',
  hover: '#252526',
  border: '#333333',
  text: '#e0e0e0',
  textDim: '#a0a0a0',
  /** The one accent, used for selection and nothing else. */
  accent: '#4f8cff',
  /** A destructive affordance (delete). */
  danger: '#e5534b',
} as const;

/** The font stack: Inter when the machine has it, the system face otherwise. */
export const FONT_STACK =
  "Inter, system-ui, -apple-system, 'Segoe UI', Roboto, 'Helvetica Neue', sans-serif";

// ── RULE 1: the left dock ──────────────────────────────────────────────────

/** The five groups. Holding one reveals its entries (RULE 1). */
export type DockGroupId = 'paint' | 'draw' | 'vector' | 'shape' | 'text';

/** A dock entry either takes the hand (a tool) or acts at once (a command). */
export type DockEntry =
  | { kind: 'tool'; tool: ToolName; label: string; shortcut: string; hint: string }
  | { kind: 'action'; action: DockActionId; label: string; hint: string };

/**
 * The engine tools the dock can hold.
 *
 * `ToolName` is deliberately the *wire* name — the same string the draw
 * session, `toolForShortcut` and every `draw_pointer` call use. The dock's
 * designer-facing names ("Pen", "Node") live on the entries, where renaming one
 * cannot change which tool the engine is told about.
 */
export type ToolName = 'select' | 'direct' | 'pen' | 'brush' | 'text' | 'smartFill';

/**
 * Everything a dock entry can *do* rather than *become*.
 *
 * Each of these is a real engine call, and each is listed here once so the dock
 * and the canvas HUD cannot offer different versions of the same verb:
 *
 * * `add-rectangle` / `add-circle` → `CreateNode` (the Shape group)
 * * `boolean-*` → an `ApplyOperation` of that kind (the Vector group's Boolean)
 * * `offset` / `fillet` / `mirror` → the same command family's modifiers
 * * `flip` → a `MirrorAxis` operation across the selection's own centre
 * * `duplicate` → `DuplicateNode`
 * * `group` / `ungroup`-shaped actions are on the Layers panel's own header.
 *
 * `rotate` is *not* in this list, and its absence is deliberate: the engine has
 * no transform on a node (audit §3.6, roadmap 1.1), so a Rotate verb here would
 * be a button that does nothing. The HUD renders it disabled, with the reason.
 */
export type DockActionId =
  | 'add-rectangle'
  | 'add-circle'
  | 'boolean-union'
  | 'boolean-subtract'
  | 'boolean-intersect'
  | 'boolean-exclude'
  | 'offset'
  | 'fillet'
  | 'mirror'
  | 'flip'
  | 'duplicate';

export interface DockGroup {
  id: DockGroupId;
  /** The accessible name and the flyout's title. */
  label: string;
  /** The glyph shown when the group is collapsed. */
  glyph: string;
  entries: DockEntry[];
}

const tool = (
  name: ToolName,
  label: string,
  shortcut: string,
  hint: string,
): DockEntry => ({ kind: 'tool', tool: name, label, shortcut, hint });

const action = (id: DockActionId, label: string, hint: string): DockEntry => ({
  kind: 'action',
  action: id,
  label,
  hint,
});

/**
 * **The dock, in the brief's own words** (RULE 1): Paint, Draw, Vector, Shape,
 * Text — with the Vector group holding Pen, Node, Boolean, Offset and Mirror as
 * the brief spells out, and the rest following the same shape.
 *
 * The ordering is the philosophy: Paint, then Draw, then Vector. Precision is
 * the third group, not the first, because "paint first, vector when precision
 * matters" has to be visible in the layout or it is just a slogan.
 */
export const TOOL_GROUPS: readonly DockGroup[] = [
  {
    id: 'paint',
    label: 'Paint',
    glyph: '🖌',
    entries: [
      tool('brush', 'Brush', 'B', 'Draw freehand. Pressure and speed shape the curve.'),
      tool('smartFill', 'Fill', 'F', 'Click an enclosed area to fill it — the fill follows its paths.'),
    ],
  },
  {
    id: 'draw',
    label: 'Draw',
    glyph: '✎',
    // RULE 1 fixes what the *Vector* group holds (Pen, Node, Boolean, Offset,
    // Mirror) and leaves this one to the engine's remaining tool: the pointer.
    // The five verbs a designer reaches for when precision starts are all under
    // Vector, exactly as the brief spells them out, so the dock has no group
    // that contradicts its own label.
    entries: [
      tool('select', 'Move', 'V', 'Move objects. Drag a shape to send it across the canvas.'),
    ],
  },
  {
    id: 'vector',
    label: 'Vector',
    glyph: '⌘',
    entries: [
      tool('pen', 'Pen', 'P', 'Click for corners, drag for curves, click the first point to close.'),
      tool('direct', 'Node', 'A', 'Click a point to own it, drag its handles to shape the curve.'),
      action('boolean-union', 'Boolean · Union', 'Merge two or more shapes into one.'),
      action('offset', 'Offset', 'Grow or shrink the outline by a value.'),
      action('mirror', 'Mirror', 'Reflect a copy across a vertical line.'),
    ],
  },
  {
    id: 'shape',
    label: 'Shape',
    glyph: '▢',
    entries: [
      action('add-rectangle', 'Rectangle', 'Add a rectangle at the canvas centre.'),
      action('add-circle', 'Circle', 'Add a circle at the canvas centre.'),
    ],
  },
  {
    id: 'text',
    label: 'Text',
    glyph: 'T',
    entries: [
      tool('text', 'Text', 'T', 'Click the canvas and type. Set the type in the Properties panel.'),
    ],
  },
];

/** Every entry in the dock, flattened — the HUD and tests both want this. */
export function dockEntries(): DockEntry[] {
  return TOOL_GROUPS.flatMap((group) => group.entries);
}

/** The group an entry belongs to, or `null`. */
export function groupOf(entry: DockEntry): DockGroup | null {
  const key = entry.kind === 'tool' ? entry.tool : entry.action;
  for (const group of TOOL_GROUPS) {
    for (const candidate of group.entries) {
      const candidateKey = candidate.kind === 'tool' ? candidate.tool : candidate.action;
      if (candidateKey === key) return group;
    }
  }
  return null;
}

/** The tool an entry would select, or `null` for an action. */
export function toolOf(entry: DockEntry): ToolName | null {
  return entry.kind === 'tool' ? entry.tool : null;
}

/**
 * Which group a held tool belongs to, so the dock can show the right group open.
 *
 * `text` and `smartFill` are both in a group whose *label* is not their tool
 * name (Text's group is Text; Fill shares Paint with the brush), which is why
 * this is a lookup rather than a naming convention.
 */
export function groupForTool(name: ToolName): DockGroupId {
  for (const group of TOOL_GROUPS) {
    for (const entry of group.entries) {
      if (entry.kind === 'tool' && entry.tool === name) return group.id;
    }
  }
  return 'vector';
}

// ── RULE 2: layer type indicators ──────────────────────────────────────────

/**
 * The five object types RULE 2 names, and the glyph each one wears.
 *
 * `glyph` is the brief's own character and it is what a layer row *wears*:
 * RULE 2 names these five characters, so a test can assert the indicator itself
 * rather than a picture of it. `icon` names the equivalent `lucide-react` export
 * for the surfaces that need a painted icon instead — a future raster layer's
 * flyout, say — so the two representations of one type cannot drift apart.
 */
export type LayerType = 'raster' | 'vector' | 'group' | 'adjustment' | 'text';

export interface LayerTypeInfo {
  type: LayerType;
  glyph: string;
  label: string;
  /** The `lucide-react` export name, as a string so this module stays pure. */
  icon: string;
}

export const LAYER_TYPES: Record<LayerType, LayerTypeInfo> = {
  raster: { type: 'raster', glyph: '◉', label: 'Paint layer', icon: 'Circle' },
  vector: { type: 'vector', glyph: '◇', label: 'Vector object', icon: 'Squircle' },
  group: { type: 'group', glyph: '▣', label: 'Group', icon: 'Boxes' },
  adjustment: { type: 'adjustment', glyph: '⌁', label: 'Adjustment or mask', icon: 'SlidersHorizontal' },
  text: { type: 'text', glyph: 'T', label: 'Text', icon: 'Type' },
};

/**
 * **What kind of thing a row is** (RULE 2), read off the engine's own wire.
 *
 * The mapping is a total function of the snapshot and never of the name: a node
 * called "text layer" is a vector object, and saying otherwise is exactly the
 * kind of lie the audit's RULE 1 exists to prevent.
 *
 * `isGroup` is not a guess: the engine removes a `NodeKind::Group` from the
 * scene entirely (it is structure, not geometry), so a group has no
 * `SnapshotNode` at all — the layers wire is the only thing that knows a row is
 * one, and it says so.
 */
export function layerTypeOf(
  node: SnapshotNodeWire | undefined,
  isGroup: boolean,
): LayerTypeInfo {
  if (isGroup) return LAYER_TYPES.group;
  if (!node) return LAYER_TYPES.vector;
  if (node.text) return LAYER_TYPES.text;
  return LAYER_TYPES.vector;
}

/**
 * **The mark a row wears**, including the one type that is real today and is
 * not an object kind: `⌁` *(adjustment or mask)*.
 *
 * A layer carrying a clipping mask or an alpha lock *is* a mask — those two
 * flags are the whole of the compositing feature set the engine has (Task 10.7
 * RULE 3), and a panel that drew them as an ordinary layer would be hiding the
 * one place the document says "this layer is not just artwork". So the mark is
 * read from the layer's own record, which is why this function takes it.
 *
 * The two remaining entries — raster and adjustment *nodes* — are unreachable
 * until the engine grows a `NodeKind` for them (roadmap 2.1 and 1.6). They are
 * listed in {@link LAYER_TYPES} so that day is one line, not a redesign.
 */
export function layerMark(
  row: { kind: 'layer' | 'node'; isGroup: boolean },
  layer: { alpha_locked: boolean; clipping_mask: boolean } | undefined,
): LayerTypeInfo {
  if (row.kind === 'layer') {
    // A layer that hides part of itself — by clipping to the layer below or by
    // locking its alpha — *is* the engine's mask vocabulary; only a mask may be
    // painted through. Everything else in the shell is a group or a vector
    // object until the engine grows a raster or an adjustment node to report.
    if (layer && (layer.clipping_mask === true || layer.alpha_locked === true)) {
      return LAYER_TYPES.adjustment;
    }
    return LAYER_TYPES.group;
  }
  return layerTypeOf(undefined, row.isGroup);
}

// ── RULE 1: the dynamic bottom toolbar ─────────────────────────────────────

/**
 * The four states the bottom bar can be in.
 *
 * The brief names three (Brush, Vector path, Node); the fourth is what the bar
 * shows when none of them apply — an empty selection with a Move tool, say. It
 * is not "hide the bar": a control strip that comes and goes is a control strip
 * you cannot learn, and the fourth state is also where Undo/Redo/Zoom live.
 */
export type BottomBarMode = 'brush' | 'path' | 'node' | 'idle';

/**
 * **Which contextual state the bar is in** — RULE 1's whole decision in one
 * function.
 *
 * Precedence is the brief's order and it matters: a *node* selection (the white
 * arrow owns one anchor) is more specific than a path selection, which is more
 * specific than the tool in hand. So holding the pen while a node is selected
 * shows the node controls, because that is what the next gesture will change.
 */
export function bottomBarMode(input: {
  /** The tool the designer is holding. */
  tool: ToolName;
  /** How many objects are selected. */
  selectionCount: number;
  /** Does the single selected object have geometry a path bar can edit? */
  selectedIsGeometry: boolean;
  /** Is one anchor of the selected path under the white arrow? */
  anchorOwned: boolean;
}): BottomBarMode {
  if (input.anchorOwned) return 'node';
  if (input.tool === 'brush') return 'brush';
  if (input.selectionCount > 0 && input.selectedIsGeometry) return 'path';
  return 'idle';
}

/** A one-line caption for the bar, so the state is never a mystery. */
export function bottomBarCaption(mode: BottomBarMode, count: number): string {
  switch (mode) {
    case 'brush':
      return 'Brush';
    case 'path':
      return count === 1 ? '1 object selected' : `${count} objects selected`;
    case 'node':
      return 'Point';
    default:
      return count === 0 ? 'Nothing selected' : `${count} selected`;
  }
}

// ── RULE 3: canvas HUD ─────────────────────────────────────────────────────

/** A HUD action: the ones the brief names, plus the More disclosure. */
export type HudActionId = 'duplicate' | 'flip' | 'rotate' | 'boolean' | 'more';

export interface HudAction {
  id: HudActionId;
  label: string;
  /** The `lucide-react` icon name. */
  icon: string;
  enabled: boolean;
  /** Why it is unavailable, when it is. Shown in the tooltip — never silent. */
  reason?: string;
}

export interface HudModel {
  actions: HudAction[];
  /** The primary caption: what the HUD is acting on. */
  caption: string;
}

/**
 * **The canvas HUD** (RULE 3): Duplicate, Flip, Rotate, Boolean, More.
 *
 * Two of the five are gated by things the engine cannot do yet, and this
 * function *says so* instead of hiding them:
 *
 * * **Rotate** is disabled always — a node has no transform (roadmap 1.1).
 * * **Boolean** needs exactly two objects selected (the engine's booleans are
 *   binary — audit §2.4), so with one selected it is disabled and the tooltip
 *   says why: "select two objects".
 *
 * The rest are live. Flip is a real `MirrorAxis` operation (the engine reflects
 * a copy across a vertical line), which is the honest meaning of "flip" in a
 * document with no transforms: the mirrored copy is a new editable object, and
 * nothing about the original moves.
 */
export function canvasHud(selectionCount: number, primaryName: string | null): HudModel {
  const one = selectionCount === 1;
  const two = selectionCount === 2;
  return {
    caption: primaryName ?? (selectionCount === 0 ? 'Canvas' : `${selectionCount} selected`),
    actions: [
      {
        id: 'duplicate',
        label: 'Duplicate',
        icon: 'Copy',
        enabled: selectionCount > 0,
        reason: selectionCount === 0 ? 'Select an object first' : undefined,
      },
      {
        id: 'flip',
        label: 'Flip',
        icon: 'FlipHorizontal',
        enabled: one,
        reason: one ? undefined : 'Flip works on one object — mirrors a copy across its centre',
      },
      {
        id: 'rotate',
        label: 'Rotate',
        icon: 'RotateCw',
        enabled: false,
        // The audit's finding, surfaced where a designer would look for it.
        reason: 'Rotation arrives with node transforms (roadmap 1.1) — not available yet',
      },
      {
        id: 'boolean',
        label: 'Boolean',
        icon: 'Combine',
        enabled: two,
        reason: two ? undefined : 'Select two objects to combine them',
      },
      { id: 'more', label: 'More', icon: 'MoreHorizontal', enabled: true },
    ],
  };
}

/**
 * **Where the HUD floats** (RULE 3): the top-centre of the selection's box, in
 * document units, for the renderer to project to the screen.
 *
 * `null` means "the engine's numbers do not describe a box for this object" — a
 * bare `Path`, a `Group`, an `Arc` (whose extents are a function of its sweep).
 * The caller then anchors the HUD to the canvas instead of inventing a position,
 * because a HUD that points at the wrong thing is worse than one that points at
 * nothing.
 *
 * The offsets are *view* offsets in document units (a small nudge above the
 * shape), not geometry: they move the label, never the object.
 */
export function selectionAnchor(
  node: SnapshotNodeWire | undefined,
  /** How many document units one screen pixel is worth at the current zoom. */
  unitsPerPixel: number,
): { x: number; y: number } | null {
  if (!node) return null;
  const gap = 14 * unitsPerPixel;
  const box = primitiveBox(node.primitive);
  if (!box) return null;
  return { x: box.x + box.w / 2, y: box.y - gap };
}

/**
 * The axis-aligned box a **tessellated primitive** occupies, when its own
 * parameters spell one out.
 *
 * This is not a bounding box computation — every number is a resolved parameter
 * the engine already sent (`SnapshotPrimitiveWire`), and the renderer's own
 * spatial index remains the authority on what a hit is. It exists because the
 * HUD has to sit *somewhere* and the alternative is guessing.
 */
export function primitiveBox(
  primitive: SnapshotPrimitiveWire,
): { x: number; y: number; w: number; h: number } | null {
  switch (primitive.type) {
    case 'rect':
      return { x: primitive.x, y: primitive.y, w: primitive.w, h: primitive.h };
    case 'circle':
      return {
        x: primitive.cx - primitive.r,
        y: primitive.cy - primitive.r,
        w: primitive.r * 2,
        h: primitive.r * 2,
      };
    case 'text':
      // A run's own advance box, exactly as the engine measures it.
      return { x: 0, y: 0, w: primitive.width, h: primitive.height };
    // `arc` extents depend on the sweep (a 20° arc of a big circle is short);
    // `path` is an arbitrary outline. Neither is on the wire as a box, so the
    // honest answer is "no box" rather than a wrong one.
    case 'arc':
    case 'path':
      return null;
    default:
      return null;
  }
}

/** The centre of a primitive's box in document units, or `null`. */
export function primitiveCentre(
  primitive: SnapshotPrimitiveWire,
): { x: number; y: number } | null {
  const box = primitiveBox(primitive);
  if (box) return { x: box.x + box.w / 2, y: box.y + box.h / 2 };
  // A circle and an arc both carry their centre as a parameter, so the common
  // case for "flip across this shape's own middle" is always answerable.
  if (primitive.type === 'circle' || primitive.type === 'arc') {
    return { x: primitive.cx, y: primitive.cy };
  }
  return null;
}

// ── RULE 3: focus mode ─────────────────────────────────────────────────────

/** RULE 3's Tab key: every panel and toolbar hidden, artwork on `#121212`. */
export const FOCUS_CANVAS = THEME.canvas;

/**
 * The chrome a focus-mode canvas still owes the designer.
 *
 * Not "nothing": the canvas needs a way back, and RULE 3 asks for one — moving
 * the cursor to the edge softly reveals the UI. So `chromeVisible` is
 * `!focusMode || peek`, and the *canvas* itself is untouched either way: the
 * same scene, the same camera, the same gestures. Focus mode is a change of
 * dress, never a change of behaviour.
 */
export function chromeVisible(focusMode: boolean, peek: boolean): boolean {
  return !focusMode || peek;
}

/** How close to the edge (in CSS px) the pointer has to be to wake the UI. */
export const EDGE_REVEAL_PX = 56;

/**
 * **The soft edge reveal** (RULE 3): is the pointer close enough to an edge to
 * bring the chrome back?
 *
 * The left and right edges are where the dock and the panel live, so those are
 * the two that matter; the top and bottom get the same treatment because the
 * top bar holds Save and the bottom bar holds Undo, and reaching for either of
 * them should not require leaving focus mode by hand.
 */
export function edgeReveal(
  point: { x: number; y: number },
  box: { width: number; height: number },
  margin: number = EDGE_REVEAL_PX,
): boolean {
  if (box.width <= 0 || box.height <= 0) return false;
  return (
    point.x <= margin ||
    point.y <= margin ||
    point.x >= box.width - margin ||
    point.y >= box.height - margin
  );
}

// ── The canvas HUD's and bottom bar's shared vocabulary ────────────────────

/** A swatch string for a paint on the wire — the bottom bar's colour dots. */
export function paintSwatch(node: SnapshotNodeWire | undefined): string | null {
  if (!node) return null;
  const stack = node.style.appearances;
  const fill = stack.find((row) => row.kind === 'fill' && row.visible);
  if (fill) return swatchOf(fill.paint);
  if (node.style.fill) return node.style.fill;
  return null;
}

/** The first visible stroke's colour, or `null`. */
export function strokeSwatch(node: SnapshotNodeWire | undefined): string | null {
  if (!node) return null;
  const stack = node.style.appearances;
  const stroke = stack.find((row) => row.kind === 'stroke' && row.visible);
  if (stroke) return swatchOf(stroke.paint);
  return node.style.stroke || null;
}

/** The first visible stroke's width, or `0`. */
export function strokeWidthOf(node: SnapshotNodeWire | undefined): number {
  if (!node) return 0;
  const stroke = node.style.appearances.find(
    (row) => row.kind === 'stroke' && row.visible && row.width != null,
  );
  return stroke?.width ?? node.style.stroke_width ?? 0;
}

function swatchOf(paint: SnapshotNodeWire['style']['appearances'][number]['paint']): string {
  switch (paint.type) {
    case 'solid':
      return paint.color;
    case 'linear':
    case 'radial':
      return paint.stops[0]?.color ?? THEME.textDim;
    default:
      return THEME.textDim;
  }
}

/**
 * **The geometry slots a node's box maps onto** (RULE 1's Properties tab).
 *
 * The engine has no transform on a node, so "X, Y, W, H" is not one thing it can
 * answer uniformly — a rectangle has four scalars, a circle has a centre and a
 * radius, a path has neither. This table is that translation, and it is honest
 * about the third case: `null` means the Properties tab shows no numeric row
 * rather than a disabled one, because there is no slot to write.
 *
 * `scale` is how a *box* dimension maps to the engine's own parameter: a
 * circle's width is `2 × radius`, so writing `W` writes `radius = W / 2`. That is
 * a unit conversion on the way in and out, not a geometry computation.
 */
export interface GeometrySlots {
  /** The parameter that holds the box's left edge. */
  x: { property: string; scale: number; offset: number };
  y: { property: string; scale: number; offset: number };
  /** The parameter the box's width maps to, or `null` when it has none. */
  w: { property: string; scale: number; offset: number } | null;
  h: { property: string; scale: number; offset: number } | null;
  /** The corner-radius slot, when the primitive has one. */
  corner: string | null;
  /** The box itself, in the engine's numbers — what the rows display. */
  box: { x: number; y: number; w: number; h: number } | null;
}

const direct = (property: string) => ({ property, scale: 1, offset: 0 });

export function geometrySlots(node: SnapshotNodeWire | undefined): GeometrySlots | null {
  if (!node) return null;
  const box = primitiveBox(node.primitive);
  switch (node.primitive.type) {
    case 'rect':
      return {
        x: direct('x'),
        y: direct('y'),
        w: direct('width'),
        h: direct('height'),
        corner: 'corner_radius',
        box,
      };
    case 'circle':
      // `W` is the diameter: writing it writes the radius, exactly as the
      // engine's own `radius` parameter defines it.
      return {
        x: direct('cx'),
        y: direct('cy'),
        w: { property: 'radius', scale: 2, offset: 0 },
        h: { property: 'radius', scale: 2, offset: 0 },
        corner: null,
        box,
      };
    case 'arc':
      return {
        x: direct('cx'),
        y: direct('cy'),
        w: { property: 'radius', scale: 2, offset: 0 },
        h: { property: 'radius', scale: 2, offset: 0 },
        corner: null,
        box,
      };
    case 'text':
      // A run's position is a parameter; its size is typography, and the Text
      // panel owns it. So X/Y are live here and W/H are absent — not disabled.
      return { x: direct('x'), y: direct('y'), w: null, h: null, corner: null, box };
    case 'path':
    default:
      return { x: direct('start'), y: direct('start'), w: null, h: null, corner: null, box };
  }
}

/** Is this node something the appearance stack can paint? (Not a group.) */
export function paintable(node: SnapshotNodeWire | undefined): boolean {
  return Boolean(node) && node?.primitive.type !== undefined;
}

/**
 * **What a point *is*** (RULE 1's node bar): `smooth` when its two handles
 * mirror each other, `corner` when they do not.
 *
 * The third word in the brief's list — *symmetric* — is a **state**, not an
 * action: a symmetric point is exactly a smooth one, and the bar reports it
 * beside the two verbs (Corner, Smooth) rather than offering a third button that
 * would do what one of them already does. Both handles absent is a plain corner;
 * one absent is a half-handle, which the engine treats as a corner too.
 *
 * The test is the engine's own definition of symmetry — the in-handle is the
 * out-handle reflected through the anchor — evaluated here on *resolved*
 * coordinates the overlay already carries, so this is a reading of engine data,
 * not a second opinion about the geometry.
 */
export function nodeState(
  anchors: ReadonlyArray<{
    slot: string;
    x: number;
    y: number;
    handle_in: [number, number] | null;
    handle_out: [number, number] | null;
  }>,
  slot: string,
  /** How far apart the mirrored handles may be and still count as equal.
   *  A tenth of a document unit is below what a hand can place. */
  tolerance = 0.1,
): 'smooth' | 'corner' {
  const anchor = anchors.find((candidate) => candidate.slot === slot);
  if (!anchor || !anchor.handle_in || !anchor.handle_out) return 'corner';
  const [ix, iy] = anchor.handle_in;
  const [ox, oy] = anchor.handle_out;
  // Mirror through the anchor: in = 2·anchor − out.
  const mirroredX = 2 * anchor.x - ox;
  const mirroredY = 2 * anchor.y - oy;
  return Math.hypot(ix - mirroredX, iy - mirroredY) <= tolerance ? 'smooth' : 'corner';
}

/** The node id a snapshot has, or `null` — the shell's own narrow helper. */
export function primarySelected(
  snapshot: SnapshotWire | null,
  selection: readonly string[],
): SnapshotNodeWire | null {
  const id = selection[0];
  if (!id || !snapshot) return null;
  return snapshot.scene.nodes[id] ?? null;
}
