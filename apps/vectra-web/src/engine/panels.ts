/**
 * The workspace panels' view-model (Task 10.2) — **pure functions over the
 * snapshot**, and nothing else.
 *
 * The standing rule is that React is a dumb remote: it holds no document, no
 * resolved parameter, no geometry. The workspace panels are the place where that
 * rule could most easily be broken — a Layers Panel wants to *know* the tree, an
 * Appearance Panel wants to *compute* a stack — so the split is drawn
 * explicitly here:
 *
 * ```text
 *   panels.ts   the wire shapes the engine already produced, reshaped for a row
 *   components  pixels and pointers
 *   engine      every fact about the document, every value, every flag
 * ```
 *
 * Everything below is a total function of `SnapshotWire`, which is why it is
 * unit-testable with no DOM and no WASM (`tests/panels.test.ts`).
 */

import type {
  AppearanceLayerWire,
  BlendModeLowerWire,
  BlendModeWire,
  ColorWire,
  GradientStopWire,
  PaintWire,
  SnapshotAppearanceWire,
  SnapshotArtboardWire,
  SnapshotLayerWire,
  SnapshotNodeWire,
  SnapshotPaintWire,
  SnapshotWire,
} from './wire';
import { lit } from './commands';

// ── Layers (RULE 1) ────────────────────────────────────────────────────────

/** One row of the Layers Panel: a layer, or one of its children. */
export interface LayerRow {
  kind: 'layer' | 'node';
  /** Layer id or node id. */
  id: string;
  name: string;
  /** Depth for indentation: 0 for a layer, 1 for its contents, 2 inside a group. */
  depth: number;
  visible: boolean;
  locked: boolean;
  /** The layer new artwork lands in (layers only). */
  active: boolean;
  /** `true` when the child is a group, so the row can be drawn as a folder. */
  isGroup: boolean;
  /**
   * `true` when this row's contents are listed by the engine and the row is
   * open-able — a group with something in it. A folder with nothing inside gets
   * no disclosure triangle, because a triangle that opens onto emptiness is a
   * lie about the document (Task 10.3 RULE 1).
   */
  canOpen: boolean;
  /**
   * The layer this row lives in.
   *
   * For a layer row it is its own id; for a child it is the layer that lists the
   * node — which is *not* always the layer row above it in the tree once groups
   * can own content, and is why the snapshot reports it per row.
   */
  layerId: string;
  /** The node's *own* flags, when they differ from the effective ones. */
  ownVisible?: boolean;
  ownLocked?: boolean;
  /** **Task 10.7 RULE 3a**: the layer's alpha lock (layers only, `false` for
   *  nodes — alpha lock is a property of a layer, not of a shape). */
  alphaLocked?: boolean;
  /** **Task 10.7 RULE 3b**: the layer clips to the one below it. */
  clippingMask?: boolean;
  /** The layer below, when there is one: the mask a clipping layer would use.
   *  `null` = nothing below, so the toggle is disabled. */
  clippedTo?: string | null;
}

/**
 * The Layers Panel's rows, top-first.
 *
 * Layers are listed **top-first** — the way every design tool shows them — so
 * the top level is the reverse of the engine's back→front order. That inversion
 * is the panel's only opinion; the ids, names and flags are the engine's.
 *
 * A *collapsed* layer (RULE 1's "collapsible") hides its children: `expanded` is
 * the set of ids the user opened — driven by layers and groups alike, because
 * "open" means *this id shows its contents* and nothing about what kind of
 * container it is.
 */
export function layerRows(
  snapshot: SnapshotWire | null,
  expanded: ReadonlySet<string>,
): LayerRow[] {
  const layers = snapshot?.layers ?? [];
  const nodes = snapshot?.scene.nodes ?? {};
  const rows: LayerRow[] = [];
  for (const layer of [...layers].reverse()) {
    rows.push({
      kind: 'layer',
      id: layer.id,
      name: layer.name,
      depth: 0,
      visible: layer.visible,
      locked: layer.locked,
      active: layer.active,
      isGroup: false,
      canOpen: layer.children.length > 0,
      layerId: layer.id,
      alphaLocked: layer.alpha_locked,
      clippingMask: layer.clipping_mask,
      clippedTo: layer.clipped_to,
    });
    if (!expanded.has(layer.id)) continue;
    rows.push(...layerTreeRows(layer, expanded, nodes));
  }
  return rows;
}

/**
 * **One layer's rows, top-first, at any depth** (Task 10.4 RULE 1).
 *
 * The engine sends one link per row — the group that holds it — and nothing else
 * about the tree, so the panel's shape is decided here, in one walk with three
 * properties:
 *
 * * **Siblings keep the engine's z-order.** Within a container the rows are the
 *   layer's own back→front list, reversed for the panel's top-first reading —
 *   the same list the canvas draws from, so the panel cannot show an order the
 *   artwork does not have.
 * * **A row nests under its parent, at any depth.** A group's row is followed by
 *   its children, each of which may open in turn; the indent is the walk's own
 *   recursion depth, so there is no level where the tree stops. (Task 10.3
 *   projected two levels and disclosed the cap; the cap is gone.)
 * * **A closed row hides its whole subtree**, and opening is per id.
 *
 * Three kinds of input are handled rather than assumed, because a snapshot is
 * data from outside this module: a link to a row the layer does not list makes
 * the row a **root** here; no row is emitted twice; and a row caught in a
 * **loop** — a link cycle the walk can never reach from a root — is shown at the
 * top level rather than dropped.
 *
 * The distinction matters: a row inside a *closed* group is hidden because the
 * designer closed the group, which is the panel working, so the loop rescue must
 * not resurrect it. Only a chain that revisits a row is unreachable in the sense
 * this fallback means.
 */
function layerTreeRows(
  layer: SnapshotLayerWire,
  expanded: ReadonlySet<string>,
  nodes: Record<string, SnapshotNodeWire>,
): LayerRow[] {
  const out: LayerRow[] = [];
  const parentAt = (index: number): string | null => (layer.child_parent ?? [])[index] ?? null;
  const listed = new Set(layer.children);
  const emitted = new Set<number>();

  const walk = (index: number, depth: number) => {
    if (emitted.has(index)) return;
    emitted.add(index);
    const id = layer.children[index];
    out.push(childRow(layer, index, nodes, depth));
    if (!expanded.has(id)) return;
    // Children, top-first, one level deeper.
    for (let inner = layer.children.length - 1; inner >= 0; inner -= 1) {
      if (parentAt(inner) === id) walk(inner, depth + 1);
    }
  };

  for (let index = layer.children.length - 1; index >= 0; index -= 1) {
    const parent = parentAt(index);
    if (parent === null || !listed.has(parent)) walk(index, 1);
  }
  // **Loops** get a second pass: a row whose ancestors revisit one another can
  // never be reached from a root, and a designer looking at a hand-written
  // document should still see every row it contains — once each.
  for (let index = 0; index < layer.children.length; index += 1) {
    if (!emitted.has(index) && closesLoop(parentAt, listed, layer, index)) walk(index, 1);
  }
  return out;
}

/** Would walking up from `index` revisit a row? The one shape the root pass
 *  cannot reach, and therefore the only shape the rescue pass may add. */
function closesLoop(
  parentAt: (index: number) => string | null,
  listed: ReadonlySet<string>,
  layer: SnapshotLayerWire,
  index: number,
): boolean {
  const seen = new Set<string>();
  let cursor: string | null = layer.children[index];
  let hops = 0;
  while (cursor && hops <= layer.children.length) {
    if (seen.has(cursor)) return true;
    seen.add(cursor);
    const at = layer.children.indexOf(cursor);
    if (at < 0 || !listed.has(cursor)) return false;
    cursor = parentAt(at);
    hops += 1;
  }
  return false;
}

function childRow(
  layer: SnapshotLayerWire,
  index: number,
  nodes: Record<string, SnapshotNodeWire>,
  depth: number,
): LayerRow {
  const id = layer.children[index];
  const node = nodes[id];
  return {
    kind: 'node',
    id,
    name: layer.child_names[index] ?? node?.name ?? id,
    depth,
    // The *effective* flags are what the canvas obeys; when a node's own flags
    // differ, the row says so (a node hidden by its layer is not the same as one
    // hidden by itself, and a designer debugging a missing shape needs to know
    // which).
    visible: node?.visible ?? true,
    locked: node?.locked ?? false,
    active: false,
    isGroup: (layer.child_is_group ?? [])[index] ?? false,
    canOpen: (layer.child_can_open ?? [])[index] ?? false,
    // The layer that lists this node, as the snapshot reports it — which is the
    // row's own layer, not necessarily "the layer row above it".
    layerId: node?.layer ?? layer.id,
    ownVisible: node?.own_visible,
    ownLocked: node?.own_locked,
  };
}

/**
 * **Where a dragged row lands** (Task 10.4 RULE 1): the container it joins and
 * its position among that container's siblings, in the engine's own index
 * convention (`Document::set_parent`: a position in the layer's back→front list,
 * counted *after* the moved block is taken out).
 *
 * Three drop targets, each with the meaning a designer expects from every other
 * tree view:
 *
 * * **a layer row** — the node moves to that layer's top level, in front;
 * * **a group row** — the node goes *inside* the group, in front of its members;
 * * **any other row** — the node becomes a sibling, directly in front of it.
 *
 * `null` means "this drop changes nothing", and there are three honest reasons
 * for it: the drop is on the dragged row itself, on one of its own descendants
 * (which would make the group its own ancestor), or the node is already exactly
 * there. The first two are refusals the *engine* also makes — typed, as
 * `GroupCycle` — and this check exists so the panel does not round-trip a
 * gesture the document will reject. The engine stays the authority: nothing here
 * is trusted to be complete, only to be a subset of it.
 */
export function dropNodeAt(
  layer: SnapshotLayerWire | null,
  target: { id: string; kind: 'layer' | 'node'; canOpen?: boolean },
  dragged: string,
): { parent: string | null; index: number } | null {
  if (!layer || !dragged || target.id === dragged) return null;
  // A drop into the node's own subtree would make a group its own ancestor.
  // Refused here *and* by the engine (`GroupCycle`, typed): this check spares the
  // round trip, the engine's is the authority.
  if (isDescendantOf(layer, target.id, dragged)) return null;
  const parentAt = (id: string): string | null => {
    const index = layer.children.indexOf(id);
    return index < 0 ? null : ((layer.child_parent ?? [])[index] ?? null);
  };
  /** The container's rows, in the engine's order, with the dragged node out. */
  const siblings = (parent: string | null): string[] =>
    layer.children.filter((child) => child !== dragged && parentAt(child) === parent);

  if (target.kind === 'layer') {
    // "In front" is the end of the container: the panel reads top-first, so the
    // last sibling is the row the designer sees at the top of that container.
    return { parent: null, index: siblings(null).length };
  }

  const inside = target.canOpen === true;
  if (inside) {
    return { parent: target.id, index: siblings(target.id).length };
  }

  // A sibling of the target, in front of it.
  const parent = parentAt(target.id);
  const siblingsOfTarget = siblings(parent);
  const at = siblingsOfTarget.indexOf(target.id);
  // A target the layer does not list (a snapshot that has moved on) has no
  // position to anchor to; front of the container is the honest fallback.
  return { parent, index: at < 0 ? siblingsOfTarget.length : at };
}

/** **A drop that may cross layers** (Task 10.4 RULE 1). `dropNodeAt` answers
 *  *where in the tree*; a gesture answers one more question — *whose* tree — and
 *  the panel needs both before it can tell a reorder from a move to another
 *  layer. This wraps the decision with the layer the target row belongs to, so
 *  the caller compares two ids instead of re-deriving membership.
 *
 *  The **target row's own layer** is the authority (it is the layer that lists
 *  the row), never "the layer above the row": a node nested in a group is listed
 *  by the layer the engine says holds it, which is also the layer it is in.
 *
 *  `null` for the same three refusals `dropNodeAt` makes, plus a target layer the
 *  snapshot does not have. */
export function dropNodeInto(
  layers: SnapshotLayerWire[] | null | undefined,
  targetLayerId: string,
  target: { id: string; kind: 'layer' | 'node'; canOpen?: boolean },
  dragged: string,
): { layer: string; parent: string | null; index: number } | null {
  const layer = (layers ?? []).find((candidate) => candidate.id === targetLayerId) ?? null;
  const drop = dropNodeAt(layer, target, dragged);
  return layer && drop ? { layer: layer.id, parent: drop.parent, index: drop.index } : null;
}

/** Is `candidate` the dragged row or one of its descendants? The panel's own
 *  cheap check for "this drop would make a group its own ancestor"; the engine
 *  refuses the same move typed (`GroupCycle`) whether or not this ran. */
export function isDescendantOf(
  layer: SnapshotLayerWire | null,
  candidate: string,
  ancestor: string,
): boolean {
  if (!layer || candidate === ancestor) return candidate === ancestor;
  let cursor: string | null = candidate;
  let hops = 0;
  while (cursor && hops <= layer.children.length) {
    if (cursor === ancestor) return true;
    const index = layer.children.indexOf(cursor);
    cursor = index < 0 ? null : ((layer.child_parent ?? [])[index] ?? null);
    hops += 1;
  }
  return false;
}

/**
 * The layers as the panel lists them, top-first, for drag-and-drop indices. */
export function layerOrder(snapshot: SnapshotWire | null): string[] {
  return [...(snapshot?.layers ?? [])].reverse().map((layer) => layer.id);
}

/**
 * Where a dragged layer lands: the engine's back→front index for a drop on
 * `targetIndex` in the panel's top-first list.
 *
 * A drag in the panel moves a row *down* the screen to send it *back* in the
 * picture, so the two orders are inverses. Keeping the flip here — once, tested
 * — is what stops it from being re-derived (and re-broken) in the component.
 */
export function dropIndexFor(topFirstIndex: number, layerCount: number): number {
  return Math.max(0, Math.min(layerCount - 1, layerCount - 1 - topFirstIndex));
}

/** The nodes a locked or hidden layer contributes: none are selectable. */
export function selectableNodes(snapshot: SnapshotWire | null): string[] {
  return (snapshot?.scene.z_order ?? []).filter((id) => {
    const node = snapshot?.scene.nodes[id];
    if (!node) return false;
    return (node.visible ?? true) && !(node.locked ?? false);
  });
}

// ── Artboards (RULE 2) ─────────────────────────────────────────────────────

export interface ArtboardRow {
  id: string;
  name: string;
  /** `[x, y, width, height]`, document space. */
  bounds: [number, number, number, number];
  background: string;
  layers: number;
  active: boolean;
  /** The size label every design tool shows next to a board: `1024×768`. */
  label: string;
}

export function artboardRows(snapshot: SnapshotWire | null): ArtboardRow[] {
  return (snapshot?.artboards ?? []).map((board: SnapshotArtboardWire) => ({
    id: board.id,
    name: board.name,
    bounds: board.bounds,
    background: board.background,
    layers: board.layers,
    active: board.active,
    label: `${round(board.bounds[2])}×${round(board.bounds[3])}`,
  }));
}

/** The active board, or `null` for a document with none. */
export function activeArtboard(snapshot: SnapshotWire | null): ArtboardRow | null {
  const rows = artboardRows(snapshot);
  return rows.find((row) => row.active) ?? rows[0] ?? null;
}

/**
 * The document rectangle "jump to artboard" should show: the board's frame with
 * a little air around it, so a stroke on the boundary is visible.
 */
export function artboardFrame(
  board: ArtboardRow | null,
  margin = 24,
): { x: number; y: number; width: number; height: number } | null {
  if (!board) return null;
  const [x, y, width, height] = board.bounds;
  return {
    x: x - margin,
    y: y - margin,
    width: width + margin * 2,
    height: height + margin * 2,
  };
}

function round(value: number): number {
  return Math.round(value * 100) / 100;
}

// ── Navigation (Task 10.3 RULE 2) ──────────────────────────────────────────
//
// Pan and zoom are the renderer's: it owns the projection, and every one of
// these functions is *presentation* — the zoom label, the wheel's arithmetic,
// which pointer buttons start a pan, and where the document grid sits. None of
// them converts a coordinate; the engine answers that (`document_to_client`).

/** The document rectangle the canvas is showing, as the renderer reports it. */
export interface NavView {
  x: number;
  y: number;
  w: number;
  h: number;
}

/**
 * The zoom label: `240%`, `100%`, `12.5%`, `8.3%`.
 *
 * One decimal **below 100%**, integers at and above it — the rule every other
 * editor uses, and the one a designer's eye expects: 12.5% and 66.7% are views
 * someone deliberately chose, while a trailing `.0` on 240 would be noise. A
 * whole number prints without the decimal (`25%`, not `25.0%`), because the
 * label should not look more precise than the camera is.
 *
 * The *value* is the renderer's; only the rounding is decided here, and a camera
 * that is not there yet says so rather than claiming 100%.
 */
export function formatZoom(scale: number | null): string {
  if (scale === null || !Number.isFinite(scale) || scale <= 0) return '—';
  const percent = scale * 100;
  if (percent >= 100) return `${Math.round(percent)}%`;
  const rounded = Math.round(percent * 10) / 10;
  return Number.isInteger(rounded) ? `${rounded}%` : `${rounded.toFixed(1)}%`;
}

/**
 * One press of `+` (or `Ctrl`+`=`) zooms by this much; `−` divides by it. A
 * quarter either way is the step every editor uses — big enough to see, small
 * enough to land on the zoom you meant.
 */
export const ZOOM_STEP = 1.25;

/** Wheel deltas arrive in three units; the engine's zoom is unitless. */
const WHEEL_UNIT_PIXELS = [1, 16, 400];
/** One full wheel notch in pixels: 400 px of scroll is one doubling. */
const WHEEL_PIXELS_PER_DOUBLING = 400;
/** No single event may zoom more than this, however violent the trackpad. */
const WHEEL_FACTOR_LIMIT = 4;

/**
 * A wheel event's zoom factor, `> 1` for zoom in.
 *
 * Scroll *up* (negative `deltaY`) zooms in, which is what every design tool does
 * and what a designer's hand expects. The factor is exponential in the scroll
 * distance, so N small events and one big one land on the same zoom — a
 * trackpad's stream of 2-pixel deltas must not behave differently from a mouse's
 * single notch.
 */
export function wheelZoomFactor(deltaY: number, deltaMode = 0): number {
  if (!Number.isFinite(deltaY) || deltaY === 0) return 1;
  const unit = WHEEL_UNIT_PIXELS[deltaMode] ?? 1;
  const pixels = deltaY * unit;
  const factor = 2 ** (-pixels / WHEEL_PIXELS_PER_DOUBLING);
  if (!Number.isFinite(factor) || factor <= 0) return 1;
  const limit = WHEEL_FACTOR_LIMIT;
  return Math.min(limit, Math.max(1 / limit, factor));
}

/**
 * Does this pointer press start a **pan**? Middle button, or space held with the
 * primary button — the two conventions every editor shares, and the reason
 * navigation does not need a tool of its own (a mode you must enter is a mode
 * you must remember to leave).
 */
export function panGestureAllowed(button: number, spaceHeld: boolean): boolean {
  return button === 1 || (button === 0 && spaceHeld);
}

/** The document grid's cell, in document units. */
export const GRID_UNITS = 50;

/**
 * Where the grid sits, for the camera that is showing `view` at `scale`.
 *
 * The grid is a CSS overlay, so a pan or a zoom that moved the artwork but not
 * the grid would be a *mismeasurement* — the designer would count cells and get
 * the wrong number. Sized and offset from the same view rectangle the shader
 * projects with, the two cannot drift apart.
 */
export function gridStyle(
  view: NavView | null,
  scale: number | null,
): { backgroundSize: string; backgroundPosition: string } {
  const cell = GRID_UNITS * (scale && Number.isFinite(scale) && scale > 0 ? scale : 1);
  const v = view ?? { x: 0, y: 0, w: 0, h: 0 };
  // Document y grows upward, the overlay grows downward: the top edge of the
  // canvas is `view.y + view.h`, which is where the pattern's origin is pinned.
  const top = (v.y + v.h) * (scale && scale > 0 ? scale : 1);
  const left = -v.x * (scale && scale > 0 ? scale : 1);
  return {
    backgroundSize: `${cell}px ${cell}px`,
    backgroundPosition: `${left}px ${top}px`,
  };
}

/** The artboard rectangles the navigation overlay outlines. `artboardRows`
 *  already carries the bounds; this is the one-line adapter the overlay takes,
 *  so the component imports no artboard logic. */
export function overlayBoards(snapshot: SnapshotWire | null): ArtboardRow[] {
  return artboardRows(snapshot);
}

// ── Appearances (RULE 3) ───────────────────────────────────────────────────

/** The blend modes the engine implements, in the order the menu lists them. */
export const BLEND_MODES: { tag: BlendModeLowerWire; label: string; wire: BlendModeWire }[] = [
  { tag: 'normal', label: 'Normal', wire: 'Normal' },
  { tag: 'multiply', label: 'Multiply', wire: 'Multiply' },
  { tag: 'screen', label: 'Screen', wire: 'Screen' },
  { tag: 'overlay', label: 'Overlay', wire: 'Overlay' },
];

export function blendLabel(tag: BlendModeLowerWire): string {
  return BLEND_MODES.find((mode) => mode.tag === tag)?.label ?? tag;
}

/** The hex colour a row paints with, for the row's swatch: a gradient reports
 *  its first stop, the way the canvas's average would. */
export function swatchColor(paint: SnapshotPaintWire): string {
  switch (paint.type) {
    case 'solid':
      return paint.color;
    case 'linear':
    case 'radial':
      return paint.stops[0]?.color ?? '#00000000';
  }
}

/** A row's short caption: `fill`, `stroke 4`, `fill · multiply`. */
export function appearanceCaption(row: SnapshotAppearanceWire): string {
  const width = row.kind === 'stroke' ? ` ${round(row.width ?? 0)}` : '';
  const blend = row.blend === 'normal' ? '' : ` · ${blendLabel(row.blend)}`;
  return `${row.kind}${width}${blend}`;
}

/**
 * The stack the panel shows for a node — the engine's resolved stack, in draw
 * order (back → front).
 */
export function appearanceStack(node: SnapshotNodeWire | null): SnapshotAppearanceWire[] {
  return node?.style.appearances ?? [];
}

/**
 * **Resolved stack → document stack.** The panel edits *resolved* values (the
 * numbers the engine already computed) and hands them back as literals.
 *
 * This is the one conversion in the panel, and it is deliberate: the alternative
 * is a UI that understands `Parameter<f64>`, `Expression` slots and motion
 * bindings, which is exactly the layer of intelligence a dumb remote must not
 * have. A slot driven by `$base * 2` becomes the number it evaluated to when the
 * designer touches *that row* — and stays parametric everywhere the designer
 * does not touch it, because the rest of the stack is echoed back verbatim.
 *
 * `edits` carries the rows the designer changed; untouched rows keep their
 * engine-side source by being sent as literals of their resolved value **or**,
 * for a parametric row the panel never touches, as the wire shape it came back
 * as. Phase 1 takes the literal route and says so in the panel's hint text.
 */
export function stackToWire(stack: SnapshotAppearanceWire[]): AppearanceLayerWire[] {
  return stack.map((row) => ({
    kind:
      row.kind === 'stroke'
        ? { Stroke: { width: lit(row.width ?? 0) } }
        : ('Fill' as const),
    paint: paintToWire(row.paint),
    opacity: lit(row.opacity),
    blend: (BLEND_MODES.find((mode) => mode.tag === row.blend)?.wire ?? 'Normal') as BlendModeWire,
    visible: row.visible,
  }));
}

export function paintToWire(paint: SnapshotPaintWire): PaintWire {
  switch (paint.type) {
    case 'solid':
      return { Solid: { Literal: hexToColor(paint.color) } };
    case 'linear':
      return {
        Linear: {
          start: { Literal: { x: paint.start[0], y: paint.start[1] } },
          end: { Literal: { x: paint.end[0], y: paint.end[1] } },
          stops: paint.stops.map(stopToWire),
        },
      };
    case 'radial':
      return {
        Radial: {
          center: { Literal: { x: paint.center[0], y: paint.center[1] } },
          radius: lit(paint.radius),
          stops: paint.stops.map(stopToWire),
        },
      };
  }
}

function stopToWire(stop: { offset: number; color: string }): GradientStopWire {
  return { offset: stop.offset, color: hexToColor(stop.color) };
}

/**
 * `#rrggbb[aa]` → the engine's colour wire.
 *
 * The engine's own `Color::to_hex` is the inverse and is what every value here
 * came from, so this is a decode of a format this UI never invents: a swatch
 * the designer picks in an `<input type="color">` arrives as `#rrggbb` and the
 * alpha stays opaque, which is the honest reading of "I picked this colour".
 */
export function hexToColor(hex: string): ColorWire {
  // Validated, not sliced-and-hoped: a malformed string must not turn into
  // *digits* (slicing `"not a colour"` yields `#0a0c00`), because those would be
  // sent to the engine as a colour the designer never chose. Anything that is
  // not `#rrggbb[aa]` reads as opaque black — the swatch's own default — so a
  // bad call site is visible as a black fill instead of a random one.
  const match = /^#?([0-9a-f]{6})([0-9a-f]{2})?$/i.exec(hex.trim());
  const clean = match ? match[1] + (match[2] ?? '') : '000000';
  const parse = (from: number, to: number): number =>
    Number.parseInt(clean.slice(from, to), 16);
  return {
    r: parse(0, 2),
    g: parse(2, 4),
    b: parse(4, 6),
    a: clean.length >= 8 ? parse(6, 8) : 255,
  };
}

/** The selected node, or `null` (the Appearance Panel's whole condition). */
export function selectedNode(
  snapshot: SnapshotWire | null,
  selection: readonly string[],
): SnapshotNodeWire | null {
  if (!snapshot || selection.length !== 1) return null;
  return snapshot.scene.nodes[selection[0]] ?? null;
}

/**
 * **Should the Appearance Panel be on screen?** (RULE 4: "only when an object is
 * selected").
 *
 * Exactly one object, and not a lock: a locked thing cannot be edited, so a
 * panel of live controls over it would be a lie. A hidden node still shows its
 * panel — the designer is usually there *because* it is hidden.
 */
export function showsAppearancePanel(
  snapshot: SnapshotWire | null,
  selection: readonly string[],
): boolean {
  const node = selectedNode(snapshot, selection);
  if (!node) return false;
  return !(node.locked ?? false);
}

// ── Stack edits (pure: new stack in, no side effects out) ───────────────────

/** Add a fill on top of the stack (RULE 3: fills stack, back → front). */
export function addFill(
  stack: SnapshotAppearanceWire[],
  color = '#2266ee',
): SnapshotAppearanceWire[] {
  return [
    ...stack,
    {
      kind: 'fill',
      paint: { type: 'solid', color },
      opacity: 1,
      blend: 'normal',
      visible: true,
      width: null,
    },
  ];
}

/** Add a stroke on top of the stack — the second half of the brief's own
 *  example ("a thick black stroke with a thinner white stroke on top"). */
export function addStroke(
  stack: SnapshotAppearanceWire[],
  color = '#000000',
  width = 2,
): SnapshotAppearanceWire[] {
  return [
    ...stack,
    {
      kind: 'stroke',
      paint: { type: 'solid', color },
      opacity: 1,
      blend: 'normal',
      visible: true,
      width,
    },
  ];
}

/** Remove one row. The stack is never emptied by the UI: a node paints
 *  *something*, so the last row cannot be deleted. */
export function removeLayerAt(
  stack: SnapshotAppearanceWire[],
  index: number,
): SnapshotAppearanceWire[] {
  if (stack.length <= 1) return stack;
  return stack.filter((_, row) => row !== index);
}

/** Move a row within the stack (drag in the Appearance Panel). */
export function moveLayer(
  stack: SnapshotAppearanceWire[],
  from: number,
  to: number,
): SnapshotAppearanceWire[] {
  if (from === to || from < 0 || from >= stack.length) return stack;
  const next = [...stack];
  const [row] = next.splice(from, 1);
  next.splice(Math.max(0, Math.min(next.length, to)), 0, row);
  return next;
}

/** Replace one row (colour, opacity, blend, width, eye, paint kind). */
export function updateLayerAt(
  stack: SnapshotAppearanceWire[],
  index: number,
  patch: Partial<SnapshotAppearanceWire>,
): SnapshotAppearanceWire[] {
  return stack.map((row, at) => (at === index ? { ...row, ...patch } : row));
}

/** Move a gradient stop, clamped to `[0, 1]` and to its neighbours — stops
 *  cannot cross, which is what keeps the ramp monotonic (the sampler in
 *  `paint.rs` sorts and collapses, so a crossing stop would silently reorder). */
export function moveStop(
  stops: { offset: number; color: string }[],
  index: number,
  offset: number,
): { offset: number; color: string }[] {
  // A drag can outlive the row it started on (a stop removed while the pointer
  // is down, a stale index from the render before last): ignore it rather than
  // reading past the end mid-gesture.
  if (index < 0 || index >= stops.length) return stops;
  const lower = index === 0 ? 0 : stops[index - 1].offset;
  const upper = index === stops.length - 1 ? 1 : stops[index + 1].offset;
  const clamped = Math.max(lower, Math.min(upper, offset));
  return stops.map((stop, at) => (at === index ? { ...stop, offset: clamped } : stop));
}

/** Recolour a stop, keeping its offset. */
export function recolorStop(
  stops: { offset: number; color: string }[],
  index: number,
  color: string,
): { offset: number; color: string }[] {
  return stops.map((stop, at) => (at === index ? { ...stop, color } : stop));
}

/** Switch a fill row between solid and a two-stop linear gradient, or back. */
export function togglePaint(
  row: SnapshotAppearanceWire,
  kind: 'solid' | 'linear' | 'radial',
): SnapshotAppearanceWire {
  if (kind === row.paint.type) return row;
  const base = swatchColor(row.paint);
  // The second stop is the ramp's other end: white, the way every design tool
  // opens a gradient until the designer picks the second colour.
  const second = '#ffffff';
  switch (kind) {
    case 'solid':
      return { ...row, paint: { type: 'solid', color: base } };
    case 'linear':
      return {
        ...row,
        paint: {
          type: 'linear',
          start: [0, 0],
          end: [100, 0],
          stops: [
            { offset: 0, color: base },
            { offset: 1, color: second },
          ],
        },
      };
    case 'radial':
      return {
        ...row,
        paint: {
          type: 'radial',
          center: [0, 0],
          radius: 100,
          stops: [
            { offset: 0, color: base },
            { offset: 1, color: second },
          ],
        },
      };
  }
}

/** A CSS `linear-gradient(…)` for the gradient bar, from the engine's stops. */
export function gradientCss(paint: SnapshotPaintWire): string | null {
  if (paint.type === 'solid') return null;
  const stops = paint.stops
    .map((stop) => `${stop.color} ${Math.round(stop.offset * 1000) / 10}%`)
    .join(', ');
  return `linear-gradient(90deg, ${stops})`;
}
