/**
 * Typed command builders (Task 1.4).
 *
 * The UI NEVER hand-writes command JSON: every action goes through these
 * builders, which own the (verbose, exact) wire schema — ids, literal
 * wrapping, externally-tagged enums. If the Rust schema changes, this file
 * (and wire.ts) is the only place that changes.
 */

import type {
  AppearanceLayerWire,
  BooleanOpWire,
  ColorWire,
  CommandWire,
  ConstraintKindWire,
  MotionTrackWire,
  OperationKindWire,
  OutlinePathWire,
  ParamValueWire,
  ParameterFloat,
  ParameterPoint,
  ProceduralKindOptionWire,
  ProceduralKindWire,
  ProceduralNodeWire,
  StrengthWire,
  TextAlignmentWire,
} from './wire';
import { slotFor, type Axis } from './view-model';

/** Wrap a scalar literal. */
export const lit = (v: number): ParameterFloat => ({ Literal: v });

/** Wrap a point literal. */
export const pt = (x: number, y: number): ParameterPoint => ({
  Literal: { x, y },
});

/** Reference a global variable by name. */
export const variable = (name: string): ParameterFloat => ({ Variable: name });

/** Bind to a defined expression by id (Task 2.1). */
export const expr = (id: string): ParameterFloat => ({ Expression: id });

export function createCircle(o: {
  cx: number;
  cy: number;
  r: number;
  name?: string;
}): CommandWire {
  return {
    type: 'CreateNode',
    id: crypto.randomUUID(),
    kind: {
      Circle: { cx: lit(o.cx), cy: lit(o.cy), radius: lit(o.r) },
    },
    ...(o.name ? { name: o.name } : {}),
  };
}

/** **A group node with nothing in it yet** (Task 10.4 RULE 1).
 *
 * Deliberately empty: the children arrive as `SetNodeParent` commands in the
 * same `Batch`, so grouping is one action whose every step is an ordinary
 * command — undo unwinds them together, and nothing needs a special "group these
 * nodes" engine verb. The id is the caller's, because the very next command in
 * the batch has to name it.
 */
export function createGroup(id: string, name: string): CommandWire {
  return {
    type: 'CreateNode',
    id,
    name,
    kind: { Group: { children: [] } },
  };
}

export function createRectangle(o: {
  x: number;
  y: number;
  w: number;
  h: number;
  cornerRadius?: number;
  name?: string;
}): CommandWire {
  return {
    type: 'CreateNode',
    id: crypto.randomUUID(),
    kind: {
      Rectangle: {
        x: lit(o.x),
        y: lit(o.y),
        width: lit(o.w),
        height: lit(o.h),
        corner_radius: lit(o.cornerRadius ?? 0),
      },
    },
    ...(o.name ? { name: o.name } : {}),
  };
}

/**
 * Task 2.2 demo builder: a rectangle created **already bound** to a global
 * variable (`width ← $name`). `CreateNode` carries parametric sources over the
 * wire exactly like `SetParameter` does, so one command produces a node that
 * participates in the dependency graph from its first frame.
 */
export function createBoundRectangle(o: {
  x: number;
  y: number;
  w: number;
  h: number;
  variable: string;
  name?: string;
}): CommandWire {
  return {
    type: 'CreateNode',
    id: crypto.randomUUID(),
    kind: {
      Rectangle: {
        x: lit(o.x),
        y: lit(o.y),
        width: variable(o.variable),
        height: lit(o.h),
        corner_radius: lit(0),
      },
    },
    ...(o.name ? { name: o.name } : {}),
  };
}

/**
 * Task 2.2 demo builder: a circle whose radius reads an expression
 * (`radius ← ƒx`). The expression must already be defined — the engine
 * pre-validates defines and rejects unknown-bind resolution at eval time with
 * a diagnostic, so the UI simply names the id it wants to consume.
 */
export function createBoundCircle(o: {
  cx: number;
  cy: number;
  expressionId: string;
  name?: string;
}): CommandWire {
  return {
    type: 'CreateNode',
    id: crypto.randomUUID(),
    kind: {
      Circle: {
        cx: lit(o.cx),
        cy: lit(o.cy),
        radius: expr(o.expressionId),
      },
    },
    ...(o.name ? { name: o.name } : {}),
  };
}

// ── Motion (Task 6.0) ──────────────────────────────────────────────────
//
// Binding a spring is *not* built here: it needs an anchor (the slot's current
// value and the engine's clock), which only the engine can resolve — see
// `VectraClient.bindSpring` / `bindHoverSpring` (and likewise the track
// registry's own reads/writes, which go through the client's helpers). What the
// UI does build is the two commands that need no anchor: binding a slot to a
// track channel, and releasing it back to a literal.

/**
 * A linear keyframe track (`ramp`): `samples` is `[time, value]` in engine
 * seconds and slot units. Times must strictly increase — the engine validates
 * and refuses a malformed track *before* it enters the document.
 */
export function motionTrack(
  id: string,
  name: string,
  channel: string,
  samples: [number, number][],
): MotionTrackWire {
  return {
    id,
    name,
    channels: {
      [channel]: samples.map(([time, value]) => ({ time, value })),
    },
  };
}

/**
 * Bind a parameter to a track channel (undoable). This one *is* expressible on
 * the wire: unlike a spring it needs no anchor, only the track's id and which
 * of its channels to sample.
 */
export function bindTrack(
  nodeId: string,
  property: string,
  trackId: string,
  channel: string,
): CommandWire {
  return {
    type: 'BindMotion',
    node_id: nodeId,
    property,
    binding: { KeyframeTrack: { track_id: trackId, property: channel } },
  };
}

/**
 * Release a slot from motion by writing a literal — the UI's "unbind" control.
 *
 * The literal is a value the *engine* reported (the slot's live value in the
 * snapshot). The UI never computes it; it forwards what it was told, which is
 * what keeps this a remote-control operation rather than a resolution.
 */
export function unbindParam(
  nodeId: string,
  property: string,
  literal: number,
): CommandWire {
  return {
    type: 'SetParameter',
    node_id: nodeId,
    property,
    value: { Float: { Literal: literal } },
  };
}

export function deleteNode(id: string): CommandWire {
  return { type: 'DeleteNode', id };
}

export function setVariable(name: string, value: number): CommandWire {
  return { type: 'SetVariable', name, value };
}

export function setFloatParam(
  nodeId: string,
  property: string,
  value: ParameterFloat,
): CommandWire {
  return {
    type: 'SetParameter',
    node_id: nodeId,
    property,
    value: { Float: value },
  };
}

/** Define (or redefine) an expression source. The id is minted here so the
 * caller can bind parameters to it; the engine pre-validates the source. */
export function defineExpression(id: string, source: string): CommandWire {
  return { type: 'DefineExpression', id, source };
}

export function removeExpression(id: string): CommandWire {
  return { type: 'RemoveExpression', id };
}

// ── Task 3.1: constraints ───────────────────────────────────────────────

/** A layer as the constraint builders need it: id + what slots it exposes. */
export interface ConstrainedLayer {
  id: string;
  /** Primitive tag from the snapshot (`rect` / `circle` / `arc` / `path`). */
  primitive: string;
  /** For messages only. */
  name?: string;
}

/** Wrap a raw constraint into its command. One place owns the wire shape. */
export function addConstraint(o: {
  kind: ConstraintKindWire;
  targets: { nodeId: string; property: string }[];
  strength?: StrengthWire;
  /** Omit for the operand-capturing kinds — the engine fills in the value that
   *  holds at dispatch time (`Distance`, `Parallel`, `Angle`). */
  value?: number;
}): CommandWire {
  return {
    type: 'AddConstraint',
    constraint: {
      id: crypto.randomUUID(),
      kind: o.kind,
      targets: o.targets.map((t) => ({ node_id: t.nodeId, property: t.property })),
      ...(o.strength ? { strength: o.strength } : {}),
      ...(o.value === undefined ? {} : { value: o.value }),
    },
  };
}

// ── Task 3.2: the drag triad ───────────────────────────────────────────
//
// The UI is a dumb remote: these three builders are the WHOLE drag protocol.
// `BeginDrag` registers the node's slots as Cassowary edit variables,
// `UpdateDrag` suggests absolute pointer coordinates (no history entry), and
// `EndDrag` finalizes the gesture as one undoable step. No builder resolves a
// parameter or consults a constraint — the engine owns every number.

export function beginDrag(id: string): CommandWire {
  return { type: 'BeginDrag', node_id: id };
}

/** `x`/`y` are DOCUMENT-space pointer coordinates (the pointer minus the grab
 *  offset, which is the only arithmetic the UI does — a view offset, not
 *  geometry). */
export function updateDrag(id: string, x: number, y: number): CommandWire {
  return { type: 'UpdateDrag', node_id: id, x, y };
}

export function endDrag(id: string): CommandWire {
  return { type: 'EndDrag', node_id: id };
}

// ── Task 4.0: non-destructive operations ───────────────────────────────
//
// The UI chooses a KIND and names the OPERANDS. Everything else — area, set
// membership, offsetting, rounding, reflecting — happens in the engine (RULE 1,
// RULE 2, RULE 3). The id is minted here only so the panel has something to key
// its rows by, exactly like a constraint id.

export function applyOperation(
  id: string,
  kind: OperationKindWire,
  inputs: string[],
): CommandWire {
  return { type: 'ApplyOperation', id, kind, inputs };
}

export function removeOperation(id: string): CommandWire {
  return { type: 'RemoveOperation', id };
}

export function setOperationEnabled(id: string, enabled: boolean): CommandWire {
  return { type: 'SetOperationEnabled', id, enabled };
}

/**
 * A boolean between two sources — the panel's primary action, bound to the
 * "two nodes selected" state. Operand order is meaningful for `subtract`:
 * `inputs[0]` is the shape that keeps its material.
 */
export function booleanOperation(
  id: string,
  op: BooleanOpWire,
  a: string,
  b: string,
): CommandWire {
  return applyOperation(id, { type: 'boolean', op }, [a, b]);
}

/** A modifier on one source. */
export function modifierOperation(
  id: string,
  kind: 'offset' | 'fillet' | 'mirror',
  target: string,
  value: number,
): CommandWire {
  switch (kind) {
    case 'offset':
      return applyOperation(id, { type: 'offset', distance: lit(value) }, [target]);
    case 'fillet':
      return applyOperation(id, { type: 'fillet', radius: lit(value) }, [target]);
    case 'mirror':
      return applyOperation(
        id,
        { type: 'mirror', axis: { axis: 'vertical', at: lit(value) } },
        [target],
      );
  }
}

// ── Task 7.0: the procedural graph ─────────────────────────────────────
//
// The panel adds a node by naming a KIND from the engine's palette and embeds
// the palette's operand payload verbatim — it never spells a default number, a
// port name, or a style. Everything after that is one command per fact: a wire,
// an operand, a park, a removal. All six are undoable, and each carries its own
// exact inverse in the engine.

/**
 * A node from a palette entry.
 *
 * `subject` is the layer a `source` reads; the palette marks those kinds with
 * `needs_subject`. `id` is minted here only so the panel has a key for its row,
 * exactly like an operation id.
 */
export function proceduralNode(
  option: ProceduralKindOptionWire,
  subject?: string,
): ProceduralNodeWire {
  const kind: ProceduralKindWire = option.needs_subject
    ? { type: 'source', node: subject ?? '' }
    : // The payload the engine handed us, tagged with the kind it came from.
      ({ type: option.tag, ...option.operands } as unknown as ProceduralKindWire);
  return { id: crypto.randomUUID(), kind };
}

/** Add a procedural node (undoable: the inverse removes it and its wires). */
export function addProceduralNode(node: ProceduralNodeWire): CommandWire {
  return { type: 'AddProceduralNode', node };
}

export function removeProceduralNode(id: string): CommandWire {
  return { type: 'RemoveProceduralNode', id };
}

/**
 * Wire an input port to an upstream output port. Both ports are named exactly as
 * the report named them; the engine checks the types and the cycle *before*
 * anything is stored (RULE 1, RULE 3).
 */
export function connectProcedural(
  nodeId: string,
  port: string,
  from: { node: string; port: string },
): CommandWire {
  return { type: 'ConnectProcedural', node_id: nodeId, port, from };
}

export function disconnectProcedural(nodeId: string, port: string): CommandWire {
  return { type: 'DisconnectProcedural', node_id: nodeId, port };
}

/**
 * Set an operand's value. The value is a **literal** the user typed — a
 * `ParamValue` like every other slot write, so the engine sees one shape whether
 * the caller is the panel, a script or a test.
 */
export function setProceduralOperand(
  nodeId: string,
  port: string,
  value: ParamValueWire,
): CommandWire {
  return { type: 'SetProceduralOperand', node_id: nodeId, port, value };
}

/** A scalar operand, from a number the user typed. */
export function scalarOperand(value: number): ParamValueWire {
  return { Float: { Literal: value } };
}

/** A point operand, from the two numbers the user typed. */
export function pointOperand(x: number, y: number): ParamValueWire {
  return { Point: { Literal: { x, y } } };
}

/** Park or re-arm a node. A parked node keeps its wires and its place. */
export function setProceduralEnabled(id: string, enabled: boolean): CommandWire {
  return { type: 'SetProceduralEnabled', id, enabled };
}

export function removeConstraint(id: string): CommandWire {
  return { type: 'RemoveConstraint', id };
}

export function setConstraintEnabled(id: string, enabled: boolean): CommandWire {
  return { type: 'SetConstraintEnabled', id, enabled };
}

/**
 * "Make B vertical to A": one rule on the x-like slot of each layer, anchored
 * on A (the solver moves B — constraints are enforced, not suggested).
 *
 * `null` when either layer exposes no float slot on that axis (Phase-1 paths
 * and groups): the caller reports it instead of sending a command the engine
 * would refuse.
 */
export function verticalConstraint(
  a: ConstrainedLayer,
  b: ConstrainedLayer,
): CommandWire | null {
  const first = slotFor('x', a.primitive);
  const second = slotFor('x', b.primitive);
  if (!first || !second) return null;
  return addConstraint({
    kind: 'vertical',
    targets: [
      { nodeId: a.id, property: first },
      { nodeId: b.id, property: second },
    ],
  });
}

/** Two rules (`cx` and `cy`) make the two layers' centres coincide. */
export function coincidentConstraints(
  a: ConstrainedLayer,
  b: ConstrainedLayer,
): CommandWire[] | null {
  const ax = slotFor('x', a.primitive);
  const bx = slotFor('x', b.primitive);
  const ay = slotFor('y', a.primitive);
  const by = slotFor('y', b.primitive);
  if (!ax || !bx || !ay || !by) return null;
  return [
    addConstraint({
      kind: 'coincident',
      targets: [
        { nodeId: a.id, property: ax },
        { nodeId: b.id, property: bx },
      ],
    }),
    addConstraint({
      kind: 'coincident',
      targets: [
        { nodeId: a.id, property: ay },
        { nodeId: b.id, property: by },
      ],
    }),
  ];
}

/**
 * A separation rule on one axis. Without an explicit `value` the engine
 * captures the current signed separation, so "Add Distance" means *hold this*
 * — which makes the over-constrained demo a two-click story.
 */
export function distanceConstraint(
  a: ConstrainedLayer,
  b: ConstrainedLayer,
  axis: Axis,
  options: { value?: number; strength?: StrengthWire } = {},
): CommandWire | null {
  const first = slotFor(axis, a.primitive);
  const second = slotFor(axis, b.primitive);
  if (!first || !second) return null;
  return addConstraint({
    kind: 'distance',
    targets: [
      { nodeId: a.id, property: first },
      { nodeId: b.id, property: second },
    ],
    ...(options.value === undefined ? {} : { value: options.value }),
    ...(options.strength ? { strength: options.strength } : {}),
  });
}

// ── Task 10.2: the workspace (layers, artboards, appearances) ──────────────
//
// Eighteen builders, one per command the panels emit. They are builders like
// every other one in this file — the panel decides *what* the designer asked
// for, the builder owns the wire shape, the engine decides what it means.

/** The exact inverse of `SetAppearances` is the same command carrying the
 *  previous stack, so the panel always sends the **whole** stack: one gesture,
 *  one undo entry. */
export function setAppearances(nodeId: string, appearances: AppearanceLayerWire[]): CommandWire {
  return { type: 'SetAppearances', node_id: nodeId, appearances };
}

export function setNodeVisible(id: string, visible: boolean): CommandWire {
  return { type: 'SetNodeVisible', id, visible };
}

// ── Task 11.0: parametric typography ───────────────────────────────────────
//
// The Text tool, the typography inspector and the two text buttons send exactly
// these builders and nothing else. The *numbers* (`font_size`,
// `letter_spacing`, `line_height`, `path_offset`) are parameters, so they go
// through the ordinary `setFloatParam` write and can be bound to a variable or
// an expression like any other slot — a type-driven composition needs no
// text-specific plumbing. These builders cover what a parameter cannot carry:
// a string, a family name, an alignment tag, a binding, and the outline plan.

/** A new text node. `text` starts empty (a caret waiting for a word) unless
 *  the caller has one; the origin is the first line's baseline start. */
export function createText(o: {
  x: number;
  y: number;
  text?: string;
  fontFamily?: string;
  fontSize?: number;
  name?: string;
}): CommandWire {
  return {
    type: 'CreateNode',
    id: crypto.randomUUID(),
    kind: {
      Text: {
        text: o.text ?? '',
        font_family: o.fontFamily ?? 'Vectra Sans',
        font_size: lit(o.fontSize ?? 32),
        letter_spacing: lit(0),
        line_height: lit(1.2),
        alignment: 'left',
        x: lit(o.x),
        y: lit(o.y),
      },
    },
    ...(o.name ? { name: o.name } : {}),
  };
}

/** Rewrite a run's string — the only writer of `text`. */
export function setText(nodeId: string, text: string): CommandWire {
  return { type: 'SetText', node_id: nodeId, text };
}

/** Choose a family. An unknown one is *diagnosed* by the engine (the run falls
 *  back to the bundled face) rather than refused — the words stay. */
export function setFontFamily(nodeId: string, family: string): CommandWire {
  return { type: 'SetFontFamily', node_id: nodeId, family };
}

export function setTextAlignment(
  nodeId: string,
  alignment: TextAlignmentWire,
): CommandWire {
  return { type: 'SetTextAlignment', node_id: nodeId, alignment };
}

/** **Bind to a path** (RULE 2). `offset` is a parameter, so a slider, a
 *  `$variable` or a spring can slide the run along the curve. */
export function bindTextToPath(
  nodeId: string,
  pathId: string,
  offset: ParameterFloat = lit(0),
): CommandWire {
  return { type: 'BindTextToPath', node_id: nodeId, path: pathId, offset };
}

/** Unbind: the run returns to its own baseline with every typographic
 *  property intact. */
export function unbindTextFromPath(nodeId: string): CommandWire {
  return { type: 'UnbindTextFromPath', node_id: nodeId };
}

/** **Outline to paths** (RULE 3), carrying an engine-minted plan.
 *
 *  The UI cannot build one itself — shaping lives in `vectra-geometry`, and a
 *  browser has no font and no glyph ids — so the Outline button goes through
 *  the boundary's `outline_text`, which shapes the run and dispatches this
 *  command. The builder exists for the callers that *do* hold a plan (tests,
 *  the boundary's own dispatch, a future host with its own shaper) and for the
 *  one invariant that matters: the group id is the caller's, so undo and redo
 *  address the same nodes. */
export function outlineTextPlan(o: {
  nodeId: string;
  groupId: string;
  paths: OutlinePathWire[];
  name?: string;
}): CommandWire {
  return {
    type: 'OutlineText',
    node_id: o.nodeId,
    group_id: o.groupId,
    ...(o.name ? { name: o.name } : {}),
    paths: o.paths,
  };
}

// ── Task 12.0: smart fills and broken paths ───────────────────────────────
//
// Two builders, one per rule. Both are *thin* on purpose: the region graph
// decided everything upstream (which boundaries, which seed, where the cuts
// are), so this file's job is to spell the decision the way the engine's serde
// expects and stop.

/**
 * **A Smart Fill** (RULE 2 + RULE 4): a fill pinned to one region of an
 * arrangement.
 *
 * `boundaries` and `seed` are the node's identity — the paths around the region
 * and the point inside it — never a copy of the geometry. Move a boundary and
 * the fill follows, because the engine re-reads the arrangement rather than
 * redrawing a stored path.
 *
 * `fill` is the colour RULE 4 dropped in; omitting it lets the engine's default
 * stack stand (RULE 2's own appearance stack, with its own fill, stroke and
 * blend). The id is minted here, like every other node the UI creates.
 */
export function createSmartFill(o: {
  boundaries: string[];
  seed: [number, number];
  fill?: ColorWire;
  name?: string;
}): CommandWire {
  return {
    type: 'CreateSmartFill',
    id: crypto.randomUUID(),
    boundaries: o.boundaries,
    seed: o.seed,
    ...(o.fill ? { fill: o.fill } : {}),
    ...(o.name ? { name: o.name } : {}),
  };
}

/**
 * **Did this command make a virtual node?** The Operations panel's question.
 *
 * A Smart Fill is not a `CreateNode`: it arrives as its own command, so anything
 * that wants to know whether the last action produced a region op asks here
 * rather than listing command types in three places.
 */
export function isSmartFill(command: CommandWire): boolean {
  return command.type === 'CreateSmartFill';
}

// ── Task 10.7: the "Procreate" layer (gestures, alpha lock, clipping) ──────
//
// Four builders: the two layer flags RULE 3 adds, the copy primitive the
// three-finger swipe needs (there is no `Copy`/`Paste` command in the engine —
// `DuplicateNode` *is* the copy, and duplicating is what a paste produces), and
// nothing else. A gesture must never grow a wire shape of its own.

/** **Copy / Paste**, as the engine's one copy primitive: a `DuplicateNode` for
 *  the selection, placed just above its source.
 *
 *  The id is generated here because `DuplicateNode` carries the *new* node's id
 *  (`crypto.randomUUID()`, the same UUIDv4 the engine's own `new_node_id()`
 *  allocates). The name is left to the engine, which suffixes the source's. */
export function duplicateNode(source: string, name?: string): CommandWire {
  return {
    type: 'DuplicateNode',
    id: crypto.randomUUID(),
    source,
    ...(name ? { name } : {}),
  };
}

/** **Alpha Lock** (RULE 3a): new artwork on this layer is clipped to what the
 *  layer already holds. A flag with its own history entry, like the eye. */
export function setLayerAlphaLocked(id: string, alpha_locked: boolean): CommandWire {
  return { type: 'SetLayerAlphaLocked', id, alpha_locked };
}

/** **Clipping Mask** (RULE 3b): this layer shows only where it overlaps the
 *  layer below. */
export function setLayerClippingMask(id: string, clipping_mask: boolean): CommandWire {
  return { type: 'SetLayerClippingMask', id, clipping_mask };
}

export function setNodeLocked(id: string, locked: boolean): CommandWire {
  return { type: 'SetNodeLocked', id, locked };
}

export function renameNode(id: string, name: string): CommandWire {
  return { type: 'RenameNode', id, name };
}

export function createLayer(name: string, artboard?: string): CommandWire {
  return {
    type: 'CreateLayer',
    id: crypto.randomUUID(),
    name,
    ...(artboard ? { artboard } : {}),
  };
}

export function deleteLayer(id: string): CommandWire {
  return { type: 'DeleteLayer', id };
}

export function renameLayer(id: string, name: string): CommandWire {
  return { type: 'RenameLayer', id, name };
}

export function setLayerVisible(id: string, visible: boolean): CommandWire {
  return { type: 'SetLayerVisible', id, visible };
}

export function setLayerLocked(id: string, locked: boolean): CommandWire {
  return { type: 'SetLayerLocked', id, locked };
}

/** Z-position among layers: `index` is where the layer lands, and its nodes
 *  travel with it (RULE 1). */
export function reorderLayer(id: string, index: number): CommandWire {
  return { type: 'ReorderLayer', id, index };
}

export function assignNodeToLayer(nodeId: string, layer: string): CommandWire {
  return { type: 'AssignNodeToLayer', node_id: nodeId, layer };
}

export function detachNodeFromLayers(nodeId: string): CommandWire {
  return { type: 'DetachNodeFromLayers', node_id: nodeId };
}

/** **Move a node into a group, out of every group, or to another layer's top
 *  level** (Task 10.4 RULE 1). `parent: null` means "listed directly by the
 *  layer", not "unassigned". The engine refuses a cycle or a non-group parent,
 *  typed, before anything moves. */
export function setNodeParent(
  id: string,
  parent: string | null,
  index: number,
): CommandWire {
  return { type: 'SetNodeParent', id, parent, index };
}

/** Several commands as **one** user action: applied in order, undone
 *  last-in-first-out, one entry in the history. A drop into another layer is
 *  exactly this — membership and parentage are two facts. */
export function batch(commands: CommandWire[]): CommandWire {
  return { type: 'Batch', commands };
}

/** **A dropped node lands in another layer** (Task 10.4 RULE 1) — the two facts
 *  are membership and parentage, so it is one batch and one undo entry.
 *
 *  The order is not decoration: `SetNodeParent` says "at `index` in *the layer
 *  that lists me*", so the node has to be in that layer before it can be placed
 *  inside a group of it. (`parent: null` = that layer's top level.) The engine
 *  still refuses a cycle typed, and the panel's pure `dropNodeInto` has already
 *  decided which layer and which position this is. */
export function moveNodeToLayer(
  nodeId: string,
  layer: string,
  parent: string | null,
  index: number,
): CommandWire {
  return batch([assignNodeToLayer(nodeId, layer), setNodeParent(nodeId, parent, index)]);
}

export function createArtboard(o: {
  name: string;
  x: number;
  y: number;
  width: number;
  height: number;
  background: ColorWire;
}): CommandWire {
  return {
    type: 'CreateArtboard',
    id: crypto.randomUUID(),
    name: o.name,
    x: o.x,
    y: o.y,
    width: o.width,
    height: o.height,
    background: o.background,
  };
}

export function deleteArtboard(id: string): CommandWire {
  return { type: 'DeleteArtboard', id };
}

export function renameArtboard(id: string, name: string): CommandWire {
  return { type: 'RenameArtboard', id, name };
}

export function setArtboardBounds(
  id: string,
  bounds: { x: number; y: number; width: number; height: number },
): CommandWire {
  return { type: 'SetArtboardBounds', id, ...bounds };
}

export function setArtboardBackground(id: string, background: ColorWire): CommandWire {
  return { type: 'SetArtboardBackground', id, background };
}

/** A pointer, not structure — but undoable like everything else (RULE 2). */
export function setActiveArtboard(id: string): CommandWire {
  return { type: 'SetActiveArtboard', id };
}

export function setActiveLayer(id: string): CommandWire {
  return { type: 'SetActiveLayer', id };
}

// ── appearance-stack helpers (pure data shaping, no geometry) ──────────────

/** A solid fill layer, the stack's most common row. */
export function fillLayer(color: ColorWire): AppearanceLayerWire {
  return {
    kind: 'Fill',
    paint: { Solid: { Literal: color } },
    opacity: lit(1),
    blend: 'Normal',
    visible: true,
  };
}

/** A solid stroke layer. */
export function strokeLayer(color: ColorWire, width: number): AppearanceLayerWire {
  return {
    kind: { Stroke: { width: lit(width) } },
    paint: { Solid: { Literal: color } },
    opacity: lit(1),
    blend: 'Normal',
    visible: true,
  };
}

/** A linear gradient fill between two stops. */
export function gradientFill(
  from: ColorWire,
  to: ColorWire,
  start: { x: number; y: number },
  end: { x: number; y: number },
): AppearanceLayerWire {
  return {
    kind: 'Fill',
    paint: {
      Linear: {
        start: { Literal: start },
        end: { Literal: end },
        stops: [
          { offset: 0, color: from },
          { offset: 1, color: to },
        ],
      },
    },
    opacity: lit(1),
    blend: 'Normal',
    visible: true,
  };
}
