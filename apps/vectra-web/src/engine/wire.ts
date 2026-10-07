/**
 * Wire contract mirror of the Rust engine (Task 1.4).
 *
 * These types MUST match the serde shapes in `vectra-core` (Command,
 * EngineEvent, NodeKind, Parameter) and `vectra-wasm` (CommandResponse,
 * SnapshotResponse) exactly. They are hand-maintained by design: any drift
 * breaks loudly at runtime (status:error) instead of silently.
 *
 * Conventions inherited from serde:
 * - `Command` / `EngineEvent` are *internally* tagged: `{type, ...fields}`.
 * - `NodeKind` / `Parameter` / `PathSegment` are *externally* tagged:
 *   `{Variant: payload}`; unit variants (`Close`, `PointerX`) are BARE STRINGS.
 */

// ── Parameters ─────────────────────────────────────────────────────────

export type ParameterFloat =
  | { Literal: number }
  | { Variable: string }
  | { Expression: string }
  | { Animated: unknown }
  | { Procedural: { node: string; port: string } }
  | { Interaction: string };

export interface PointWire {
  x: number;
  y: number;
}

export type ParameterPoint =
  | { Literal: PointWire }
  | { Variable: string }
  | { Expression: string }
  | { Animated: unknown }
  | { Procedural: { node: string; port: string } }
  | { Interaction: string };

export interface ColorWire {
  r: number;
  g: number;
  b: number;
  a: number;
}

export type ParameterColor =
  | { Literal: ColorWire }
  | { Variable: string }
  | { Expression: string }
  | { Animated: unknown }
  | { Procedural: { node: string; port: string } }
  | { Interaction: string };

export type ParamValueWire =
  | { Float: ParameterFloat }
  | { Point: ParameterPoint }
  | { Color: ParameterColor };

// ── Geometry ───────────────────────────────────────────────────────────

export type PathSegmentWire =
  | { Line: { to: ParameterPoint } }
  | { Quadratic: { control: ParameterPoint; to: ParameterPoint } }
  | {
      Cubic: {
        control1: ParameterPoint;
        control2: ParameterPoint;
        to: ParameterPoint;
      };
    }
  | 'Close';

export type NodeKindWire =
  | {
      Rectangle: {
        x: ParameterFloat;
        y: ParameterFloat;
        width: ParameterFloat;
        height: ParameterFloat;
        corner_radius: ParameterFloat;
      };
    }
  | { Circle: { cx: ParameterFloat; cy: ParameterFloat; radius: ParameterFloat } }
  | {
      Arc: {
        cx: ParameterFloat;
        cy: ParameterFloat;
        radius: ParameterFloat;
        start_angle: ParameterFloat;
        end_angle: ParameterFloat;
      };
    }
  | { Path: { start: ParameterPoint; segments: PathSegmentWire[] } }
  | { Group: { children: string[] } };

// ── Commands (Document → mutation) ─────────────────────────────────────

export type CommandWire =
  | {
      type: 'CreateNode';
      id: string;
      kind: NodeKindWire;
      name?: string;
      index?: number;
    }
  | { type: 'DeleteNode'; id: string }
  | {
      type: 'SetParameter';
      node_id: string;
      property: string;
      value: ParamValueWire;
    }
  | { type: 'SetVariable'; name: string; value: number }
  | { type: 'RemoveVariable'; name: string }
  | { type: 'DefineExpression'; id: string; source: string }
  | { type: 'RemoveExpression'; id: string }
  // ── Task 3.1: constraints (hard rules, solved by the engine) ────────
  | { type: 'AddConstraint'; constraint: ConstraintWire }
  | { type: 'RemoveConstraint'; id: string }
  | { type: 'SetConstraintEnabled'; id: string; enabled: boolean }
  // ── Task 3.2: the drag triad. `UpdateDrag` samples carry ABSOLUTE pointer
  //    coordinates in document space; the command produces no history entry.
  | { type: 'BeginDrag'; node_id: string }
  | { type: 'UpdateDrag'; node_id: string; x: number; y: number }
  | { type: 'EndDrag'; node_id: string }
  // ── Task 4.0: non-destructive operations ────────────────────────────
  // The sources stay in the document; the operation is a virtual node that
  // reads their evaluated geometry (RULE 1).
  | { type: 'ApplyOperation'; id: string; kind: OperationKindWire; inputs: string[] }
  | { type: 'RemoveOperation'; id: string }
  | { type: 'SetOperationEnabled'; id: string; enabled: boolean }
  // ── Task 6.0: motion ───────────────────────────────────────────────
  // `BindMotion` carries a whole binding *including its anchor* (`from`, `at`),
  // which is why the UI binds springs through the engine's own binder (it is the
  // only party that can resolve the slot and read the clock) and builds this
  // command directly only for a track, which needs no anchor.
  | { type: 'BindMotion'; node_id: string; property: string; binding: MotionBindingPayload }
  | { type: 'SetMotionTrack'; track: MotionTrackWire }
  | { type: 'RemoveMotionTrack'; track_id: string }
  // ── Task 7.0: the procedural graph ──────────────────────────────────
  // The record carries only what the panel knows: its id and its kind. The
  // engine fills in the rest — the name (from the kind), the style (its own
  // default) and the enabled flag — which is why `ProceduralNodeWire`'s
  // cosmetic fields are optional on the wire.
  | { type: 'AddProceduralNode'; node: ProceduralNodeWire }
  | { type: 'RemoveProceduralNode'; id: string }
  | {
      type: 'ConnectProcedural';
      node_id: string;
      port: string;
      from: { node: string; port: string };
    }
  | { type: 'DisconnectProcedural'; node_id: string; port: string }
  | { type: 'SetProceduralOperand'; node_id: string; port: string; value: ParamValueWire }
  | { type: 'SetProceduralEnabled'; id: string; enabled: boolean }
  // ── Task 10.2: the workspace (layers, artboards, appearances) ───────
  //
  // RULE 4 is visible in this list: the eye and the padlock are ordinary
  // commands that the *engine* turns into a presentation flag. The UI never
  // learns that they are special, and the engine never re-evaluates for them.
  | { type: 'SetAppearances'; node_id: string; appearances: AppearanceLayerWire[] }
  | { type: 'SetNodeVisible'; id: string; visible: boolean }
  | { type: 'SetNodeLocked'; id: string; locked: boolean }
  /** **Copy a node** — the engine's one copy primitive, which Task 10.7's
   *  Copy/Paste gesture (RULE 1) is built on. The new id is the caller's. */
  | { type: 'DuplicateNode'; id: string; source: string; name?: string; index?: number }
  | { type: 'RenameNode'; id: string; name: string }
  | { type: 'CreateLayer'; id: string; name: string; index?: number; artboard?: string }
  | { type: 'DeleteLayer'; id: string }
  | { type: 'RenameLayer'; id: string; name: string }
  | { type: 'SetLayerVisible'; id: string; visible: boolean }
  | { type: 'SetLayerLocked'; id: string; locked: boolean }
  // **Task 10.7 RULE 3** — the two Procreate flags, each a command of its own
  // (one history entry, like the eye), so the designer can undo a lock without
  // undoing the drawing that came after it.
  | { type: 'SetLayerAlphaLocked'; id: string; alpha_locked: boolean }
  | { type: 'SetLayerClippingMask'; id: string; clipping_mask: boolean }
  | { type: 'ReorderLayer'; id: string; index: number }
  | { type: 'AssignNodeToLayer'; node_id: string; layer: string }
  | { type: 'DetachNodeFromLayers'; node_id: string }
  | {
      type: 'CreateArtboard';
      id: string;
      name: string;
      x: number;
      y: number;
      width: number;
      height: number;
      background: ColorWire;
    }
  | { type: 'DeleteArtboard'; id: string }
  | { type: 'RenameArtboard'; id: string; name: string }
  | {
      type: 'SetArtboardBounds';
      id: string;
      x: number;
      y: number;
      width: number;
      height: number;
    }
  | { type: 'SetArtboardBackground'; id: string; background: ColorWire }
  | { type: 'SetActiveArtboard'; id: string }
  | { type: 'SetActiveLayer'; id: string }
  /** Task 10.4: move a node into a group (`parent`), out of every group
   *  (`null`), or to another layer's top level — at a sibling position. */
  | { type: 'SetNodeParent'; id: string; parent: string | null; index: number }
  /** Several commands as one user action: applied in order, undone LIFO, one
   *  entry in the history. */
  | { type: 'Batch'; commands: CommandWire[] };

/**
 * One layer of an appearance stack, as the **document** states it (Task 10.2
 * RULE 3).
 *
 * This is the authored shape — parametric slots included — which is why it is a
 * separate type from `SnapshotAppearanceWire` (the *resolved* shape the panels
 * read). The Appearance Panel edits resolved values and sends them back as
 * literals: the alternative would be a panel that has to understand
 * `Parameter<f64>`, and "the UI resolves no parameters" is a standing rule.
 */
export interface AppearanceLayerWire {
  kind: AppearanceKindWire;
  paint: PaintWire;
  opacity: ParameterFloat;
  blend: BlendModeWire;
  visible: boolean;
}

export type AppearanceKindWire = 'Fill' | { Stroke: { width: ParameterFloat } };

export type BlendModeWire = 'Normal' | 'Multiply' | 'Screen' | 'Overlay';

export type PaintWire =
  | { Solid: ParameterColor }
  | {
      Linear: { start: ParameterPoint; end: ParameterPoint; stops: GradientStopWire[] };
    }
  | {
      Radial: { center: ParameterPoint; radius: ParameterFloat; stops: GradientStopWire[] };
    };

/** A gradient stop on the wire: an offset and a resolved colour literal. */
export interface GradientStopWire {
  offset: number;
  color: ColorWire;
}

/**
 * A procedural node as the UI sends it (Task 7.0).
 *
 * Only `id` and `kind` are required: the engine names the node, styles it and
 * enables it. `wires` and `operands` may be sent to create an already-wired
 * node (the panel does not — it wires with `ConnectProcedural`, one undoable
 * step at a time).
 */
export interface ProceduralNodeWire {
  id: string;
  kind: ProceduralKindWire;
  name?: string;
  enabled?: boolean;
  style?: unknown;
  wires?: Record<string, { node: string; port: string }>;
  operands?: Record<string, ParamValueWire>;
}

/**
 * What a procedural node computes (MES §11), internally tagged exactly like
 * `ProceduralKind` in Rust. A generator carries its operands; a `source` names
 * the layer it reads — its subject is part of the kind, not a parameter.
 */
export type ProceduralKindWire =
  | { type: 'source'; node: string }
  | {
      type: 'grid';
      columns: ParameterFloat;
      rows: ParameterFloat;
      spacing: ParameterFloat;
      origin: ParameterPoint;
    }
  | { type: 'repeat'; count: ParameterFloat; dx: ParameterFloat; dy: ParameterFloat }
  | { type: 'noise'; amplitude: ParameterFloat; frequency: ParameterFloat; seed: ParameterFloat }
  | { type: 'smooth'; iterations: ParameterFloat; strength: ParameterFloat };

/** One kind in the palette, as `procedural_kinds()` reports it. */
export interface ProceduralKindOptionWire {
  tag: string;
  label: string;
  /** True for `source`: its subject is part of the kind, so the panel offers a
   *  layer picker instead of operand fields. */
  needs_subject: boolean;
  /** Operand port → a **kind-level** payload (`Parameter`, not `ParamValue`),
   *  embedded verbatim in the kind the panel builds. The UI never invents a
   *  default, and never reshapes what the engine handed it. */
  operands: Record<string, ParameterFloat | ParameterPoint | ParameterColor>;
  inputs: ProceduralKindPortWire[];
  outputs: ProceduralKindPortWire[];
}

export interface ProceduralKindPortWire {
  port: string;
  ty: string;
  required: boolean;
}

/** The `MotionBinding` variants as they cross the boundary (MES §12). */
export type MotionBindingPayload =
  | {
      KeyframeTrack: { track_id: string; property: string };
    }
  | {
      Spring: {
        target: ParameterFloat;
        stiffness: number;
        damping: number;
        /** The value the spring starts at, and the engine time it held there. */
        from: number;
        at: number;
      };
    }
  | {
      StateDriven: {
        state: string;
        true_value: ParameterFloat;
        false_value: ParameterFloat;
      };
    };

/** The eight Phase-1 constraint kinds (MES §9). */
export type ConstraintKindWire =
  | 'coincident'
  | 'horizontal'
  | 'vertical'
  | 'parallel'
  | 'perpendicular'
  | 'equal_length'
  | 'distance'
  | 'angle';

/** Droppability, in Cassowary terms. */
export type StrengthWire = 'required' | 'strong' | 'medium' | 'weak';

/** One addressed float slot: `(node, property)` — the same vocabulary the
 *  dependency graph keys its property vertices by. */
export interface ConstraintTargetWire {
  node_id: string;
  property: string;
  /** The layer's name, when the engine could resolve it. */
  name?: string | null;
}

/**
 * A constraint as it crosses the boundary.
 *
 * `value` is the operand (`Distance` / `Parallel` / `Angle`); omitting it for
 * those kinds means "capture the value that holds right now", which the engine
 * resolves at dispatch time. `enabled: false` is a rule the solver parked —
 * either the user disabled it or it was the loser of an over-constrained pass.
 */
/** The four boolean set operations, as the engine names them. */
export type BooleanOpWire = 'union' | 'subtract' | 'intersect' | 'exclude';

/**
 * What an operation computes (Task 4.0). Mirrors `OperationKind`'s internal
 * tagging exactly (`{type, …}`), so the UI never reshapes engine data — it
 * picks a kind and forwards it.
 */
export type OperationKindWire =
  | { type: 'boolean'; op: BooleanOpWire }
  | { type: 'offset'; distance: ParameterFloat }
  | { type: 'fillet'; radius: ParameterFloat }
  | { type: 'mirror'; axis: { axis: 'vertical' | 'horizontal'; at: ParameterFloat } };

/** One registered operation, as the Operations panel sees it. */
export interface OperationWire {
  id: string;
  name: string;
  /** `boolean` / `offset` / `fillet` / `mirror`. */
  kind: string;
  /** Engine-rendered one-liner (`subtract`, `fillet r=4`). */
  description: string;
  /** Source node ids, in operand order. */
  inputs: string[];
  /** The sources' display names, parallel to `inputs`. */
  input_names: string[];
  enabled: boolean;
}

/**
 * A procedural node, as the snapshot reports it (Task 7.0). Its *geometry* is in
 * `scene` as a standard node (RULE 4); this is the graph view: what it is, what
 * it reads, and what it last published.
 */
export interface SnapshotProceduralNodeWire {
  id: string;
  name: string;
  /** `source` / `grid` / `repeat` / `noise` / `smooth`. */
  kind: string;
  /** The node's **effective** operands, engine-rendered — never the kind's
   *  template numbers (an operand driven by a variable reads `(variable)`). */
  description: string;
  enabled: boolean;
  /** Input port → the `node:port` it is wired to. */
  wires: Record<string, string>;
  outputs: SnapshotProceduralPortWire[];
  /** Ids this node reads through wires. */
  upstream: string[];
}

export interface SnapshotProceduralPortWire {
  port: string;
  ty: string;
  /** `null` until the pass has published this port (a parked or failing node). */
  value: string | null;
}

export interface ConstraintWire {
  id: string;
  kind: ConstraintKindWire;
  targets: ConstraintTargetWire[];
  strength?: StrengthWire;
  value?: number | null;
  enabled?: boolean;
  /** Engine-rendered one-liner (inspector + log). Never formatted in the UI. */
  description?: string;
}

/** Constraint-solver counters for the last pass (Task 3.1, drag fields 3.2). */
export interface SolverWire {
  /** Live Cassowary variable count — grows and shrinks with the rules. */
  variables: number;
  constraints: number;
  edit_variables: number;
  writes: number;
  dropped: number;
  skipped: number;
  /** Open Cassowary edit variables: 2 while a gesture is live, else 0. */
  active_edits: number;
  /** The node whose slots are registered as edits, or `null` when idle. */
  drag_node: string | null;
}

// ── Events (engine → UI notifications) ─────────────────────────────────

export type EngineEventWire =
  | { type: 'NodesUpdated'; ids: string[] }
  | { type: 'NodesRemoved'; ids: string[] }
  | { type: 'OrderChanged' }
  | { type: 'VariablesUpdated'; names: string[] }
  | { type: 'ExpressionsUpdated'; ids: string[] }
  /** Task 3.1: a constraint was added, removed, enabled or disabled. */
  | { type: 'ConstraintsUpdated'; ids: string[] }
  /** Task 3.2: a drag gesture opened / closed. */
  | { type: 'DragStarted'; node_id: string }
  | { type: 'DragEnded'; node_id: string }
  /** Task 4.0: the operation registry changed. The virtual node's *geometry*
   *  arrives as its own id in the `Dirty` event that follows. */
  | { type: 'OperationsUpdated'; ids: string[] }
  /** Task 7.0: the procedural registry changed. The node's *geometry* arrives
   *  as its own id in the `Dirty` event that follows (RULE 4: the same id
   *  space). */
  | { type: 'ProceduralUpdated'; ids: string[] }
  | { type: 'StackChanged'; can_undo: boolean; can_redo: boolean }
  /**
   * Task 2.2: exactly which geometry nodes the mutation re-evaluated, and
   * whether that was a patch ('incremental') or a full rebuild ('full').
   * `ids: []` is the visible proof that the edit had no dependents.
   */
  | { type: 'Dirty'; ids: string[]; mode: EvalModeWire };

export type EvalModeWire = 'full' | 'incremental';

export type CommandResponseWire =
  | { status: 'ok'; events: EngineEventWire[] }
  | { status: 'error'; message: string };

/**
 * A reply from one of the **Task 10.6 component verbs** (`create_component`,
 * `instantiate_component`, `set_component_prop`, `icon_set`).
 *
 * The engine answers those with the ordinary command envelope *plus* three keys
 * (see `dispatch_side_command` in `crates/vectra-wasm/src/lib.rs`): `prose` — the
 * one-sentence receipt the UI prints verbatim (RULE 4), `label` — the same act in
 * the log's voice, and `created` — the id the engine minted, so a panel can
 * select what it just made instead of guessing which uuid came back. Typed here,
 * once, rather than intersected at each call site: a reply whose `created` is not
 * a string is a wire bug, and this is the type that says so.
 */
export type ComponentReplyWire = CommandResponseWire & {
  prose?: string;
  label?: string;
  created?: string;
};

// ── Snapshot (UI's entire world) ───────────────────────────────────────

export type SnapshotPrimitiveWire =
  | {
      type: 'rect';
      x: number;
      y: number;
      w: number;
      h: number;
      corner_radius: number;
    }
  | { type: 'circle'; cx: number; cy: number; r: number }
  | {
      type: 'arc';
      cx: number;
      cy: number;
      r: number;
      start_angle: number;
      end_angle: number;
    }
  | { type: 'path'; d: string };

/**
 * One resolved paint (Task 10.2 RULE 3): a solid colour, or a gradient with its
 * **document-space** frame and stops. The gradient bar draws exactly this, which
 * is why the frame crosses the boundary at all.
 */
export type SnapshotPaintWire =
  | { type: 'solid'; color: string }
  | { type: 'linear'; start: [number, number]; end: [number, number]; stops: SnapshotStopWire[] }
  | {
      type: 'radial';
      center: [number, number];
      radius: number;
      stops: SnapshotStopWire[];
    };

export interface SnapshotStopWire {
  offset: number;
  color: string;
}

/** One row of the Appearance Panel: kind, paint, opacity, blend, eye, width. */
export interface SnapshotAppearanceWire {
  kind: 'fill' | 'stroke';
  paint: SnapshotPaintWire;
  opacity: number;
  blend: BlendModeLowerWire;
  visible: boolean;
  /** Present exactly when the row is a stroke. */
  width?: number | null;
}

/** The blend tags the engine uses, as the wire spells them. */
export type BlendModeLowerWire = 'normal' | 'multiply' | 'screen' | 'overlay';

export interface SnapshotStyleWire {
  fill: string;
  stroke: string;
  stroke_width: number;
  opacity: number;
  /** The canonical stack. `fill`/`stroke`/`stroke_width` above are derived from
   *  its first fill and first stroke, for the legacy preview path. */
  appearances: SnapshotAppearanceWire[];
}

/**
 * Where a node's draggable slots resolve right now (Task 3.2).
 *
 * `x_source` / `y_source` are the `Parameter::source_tag()` values
 * (`literal`, `variable:base`, `expression:…`, …) — display data. `null` on the
 * node means Phase 1 has no float slot to drag on it.
 */
export interface SnapshotPositionWire {
  x: number;
  y: number;
  x_source: string;
  y_source: string;
}

export interface SnapshotNodeWire {
  id: string;
  name: string;
  primitive: SnapshotPrimitiveWire;
  style: SnapshotStyleWire;
  /** Present on the wire for every node; `null` ⇒ not draggable. */
  position?: SnapshotPositionWire | null;
  /**
   * Effective presentation (Task 10.2 RULEs 1 and 4): the node's own flags
   * combined with its layer's. `visible` is what the canvas obeys; the `own_*`
   * pair is what the row's icons show, so a node hidden by its layer reads
   * differently from one hidden by itself.
   */
  visible?: boolean;
  locked?: boolean;
  own_visible?: boolean;
  own_locked?: boolean;
  /** The layer that holds this node, and its name (`null` = unassigned). */
  layer?: string | null;
  layer_name?: string | null;
}

/** One row of the Layers Panel (Task 10.2 RULE 1). */
export interface SnapshotLayerWire {
  id: string;
  name: string;
  visible: boolean;
  locked: boolean;
  /** **Task 10.7 RULE 3a**: new artwork on this layer is clipped to what the
   *  layer already holds (the engine's commit path does the intersection). */
  alpha_locked: boolean;
  /** **Task 10.7 RULE 3b**: this layer shows only where it overlaps the layer
   *  below. */
  clipping_mask: boolean;
  /** The layer this one clips to — `null` for the bottom layer, where the
   *  toggle is meaningless and is therefore disabled rather than wrong. */
  clipped_to: string | null;
  /** The layer's contents, back → front. */
  children: string[];
  /** Display names parallel to `children`. */
  child_names: string[];
  /** `true` where a child is a group, so the row shows a group icon. */
  child_is_group: boolean[];
  /** `true` where a child is a group *with contents*, so the row opens. */
  child_can_open: boolean[];
  /**
   * **Which group holds each child**, parallel to `children` (Task 10.4
   * RULE 1). `null` = the layer lists that row directly.
   *
   * The whole tree, in one array of links: a row's contents are the rows that
   * name it, so nesting has no depth limit — this replaced Task 10.3's
   * `child_groups`, whose second level was the projection's whole extent.
   */
  child_parent: (string | null)[];
  /** The artboard that owns this layer, if any. */
  artboard?: string | null;
  active: boolean;
}

/** One row of the artboard dropdown (Task 10.2 RULE 2). */
export interface SnapshotArtboardWire {
  id: string;
  name: string;
  /** `[x, y, width, height]` in document space — what "jump to" frames. */
  bounds: [number, number, number, number];
  background: string;
  layers: number;
  active: boolean;
}

export interface SnapshotDiagnosticWire {
  severity: 'info' | 'warning' | 'error';
  code: string;
  node_id: string | null;
  property: string | null;
  message: string;
}

/** Dependency-graph summary (Task 2.2). */
export interface GraphSummaryWire {
  nodes: number;
  edges: number;
  acyclic: boolean;
}

/** Incrementality bookkeeping (Task 2.2). `full_evals` staying at 1 across a
 *  session is the engine's incrementality claim, made observable. */
export interface SnapshotEvalWire {
  last_mode: EvalModeWire;
  last_dirty: number;
  last_evaluated: number;
  full_evals: number;
  incremental_evals: number;
  no_ops: number;
}

export interface SnapshotWire {
  status: 'ok';
  scene: { nodes: Record<string, SnapshotNodeWire>; z_order: string[] };
  variables: Record<string, number>;
  expressions: Record<string, string>;
  diagnostics: SnapshotDiagnosticWire[];
  graph: GraphSummaryWire;
  eval: SnapshotEvalWire;
  /** Registered constraints, keyed by id (canonical order). */
  constraints: Record<string, ConstraintWire>;
  /** Registered operations, keyed by id in draw order (Task 4.0). */
  operations: Record<string, OperationWire>;
  /** Registered procedural nodes, keyed by id, plus the registry's own order —
   *  a chain has a direction and a JSON object does not (Task 7.0). */
  procedural: Record<string, SnapshotProceduralNodeWire>;
  procedural_order: string[];
  /** Counters from the last solver pass. */
  solver: SolverWire;
  /** The document's layers, back → front (Task 10.2 RULE 1). */
  layers?: SnapshotLayerWire[];
  /** The document's artboards, in registry order (Task 10.2 RULE 2). */
  artboards?: SnapshotArtboardWire[];
  active_layer?: string | null;
  active_artboard?: string | null;
  can_undo: boolean;
  can_redo: boolean;
  time: number;
}

// ── Canvas (`render_frame()`, Task 5.0) ────────────────────────────────

/** GPU memory totals, as the renderer reports them. */
export interface GpuStatsWire {
  nodes: number;
  buffers: number;
  buffer_bytes: number;
  instance_bytes: number;
  reallocations: number;
  draw_calls: number;
}

/**
 * One drawn frame (Task 5.0 §6).
 *
 * `dirty` is what the engine asked for; `writes`/`bytes` are what the GPU
 * actually paid. The pair is the incrementality witness at the canvas: moving
 * one node is `dirty: 1, writes: 1, bytes: 80` no matter how many pointer
 * samples arrived, and an idle frame is `writes: 0, bytes: 0`.
 */
export interface FrameWire {
  frame: number;
  /** Whether the WebGPU device is up. `false` ⇒ the canvas shows the banner. */
  ready: boolean;
  nodes: number;
  dirty: number;
  full: boolean;
  touched: number;
  writes: number;
  bytes: number;
  created: number;
  retessellated: number;
  moved: number;
  restyled: number;
  removed: number;
  draw_calls: number;
  /**
   * Bytes of ramp atlas re-uploaded by this frame (Task 10.2 RULE 3) — nonzero
   * exactly when a gradient's stops or frame changed. Deliberately not part of
   * `bytes`: that counter is the per-node instance traffic the incrementality
   * witnesses assert.
   */
  ramps: number;
  gpu: GpuStatsWire | null;
  error: string | null;
}

/** The renderer's own status (`renderer.stats()` + `renderer.view()`). */
export interface CanvasStatusWire {
  /** Whether the WebGPU device came up. */
  ready: boolean;
  frames: number;
  /** Drawing-buffer size in device pixels. */
  pixels: [number, number];
  /** Nodes the renderer holds buffers for. */
  nodes: number;
  draw_calls: number;
  error: string | null;
  gpu: GpuStatsWire | null;
}

/** The visible document rectangle (`renderer.view()`), in document units. */
export interface CanvasViewWire {
  x: number;
  y: number;
  w: number;
  h: number;
}

// ── Motion (Task 6.0, `motion_json()`) ─────────────────────────────────

/** One binding, as the inspector sees it. `kind` mirrors `MotionBinding`. */
export interface MotionBindingWire {
  node_id: string;
  property: string;
  kind: 'spring' | 'state' | 'track';
  /** The slot's live value at the current clock — what motion is *doing*. */
  value: number | null;
  from: number | null;
  at: number | null;
  target: number | null;
  stiffness: number | null;
  damping: number | null;
  state: string | null;
  track: string | null;
  channel: string | null;
}

/**
 * The document's motion state: whether anything is moving, when it will stop,
 * and every binding. `horizon === null` means "already at rest".
 */
export interface MotionWire {
  animating: boolean;
  horizon: number | null;
  time: number;
  epsilon: number;
  /** State flags currently on, e.g. `hover:<id>`. Never in the undo stack. */
  states: string[];
  tracks: string[];
  bindings: MotionBindingWire[];
}

/** A keyframe track payload (`SetMotionTrack` / `set_motion_track`). */
export interface MotionTrackWire {
  id: string;
  name: string;
  channels: Record<string, { time: number; value: number }[]>;
}

// ── Procedural graph (`procedural_json()`, Task 7.0) ──────────────────

/** The whole chain, in evaluation order, with per-node published values. */
/**
 * `export_to_svg` / `export_to_react` (Task 8.0).
 *
 * One envelope for both formats, so the panel has one parser and one modal:
 * `code` is the file the user copies, `format` labels it, and `warnings` is the
 * list shown *above* the code — an export that quietly replaced a spring or a
 * procedural read with a number would be worse than no export at all.
 */
export interface ExportEnvelopeWire {
  status: 'ok' | 'error';
  /** `"svg"` or `"react"` — the engine's own label, never the panel's. */
  format: string;
  code: string;
  warnings: string[];
  message?: string;
}

// ── The AI command layer (`ai_*`, Task 9.0) ────────────────────────────
//
// Three envelopes, one shape each, and every one of them is *string in / JSON
// out* like the rest of the boundary. The panel needs three facts the plain
// command response does not carry: the plan it is about to run, how many
// attempts it took, and — when the loop gives up — the correction history that
// explains why.

/** One correction the loop made: what the engine said, and what changed. */
export interface AiCorrectionWire {
  attempt: number;
  /** The typed `AiError` code *or* the engine's own `VectraError` text. */
  code: string;
  error: string;
  /** The command that was refused, as JSON. */
  command?: unknown;
  /** What the planner said it changed on the next attempt. */
  note?: string;
}

/**
 * `ai_generate_commands` — the preview.
 *
 * `applies` is always `false` and is in the wire on purpose: the panel renders
 * the plan without ever being able to claim it ran.
 */
/**
 * A Smart Component prop, as `component_view` publishes it (Task 10.6 RULE 1).
 *
 * Everything the panel needs to draw one *slider* — which is the point: the
 * procedural graph behind it is never shown.
 */
export interface ComponentPropWire {
  /** `size`, `stroke_width`, `corner_radius`, `color`. */
  key: string;
  /** `Size`, `Stroke`, `Corner`, `Color` — what a designer reads. */
  label: string;
  ty: 'scalar' | 'color';
  /** `direct` (this prop is the value) or `scaled` (derived from another prop). */
  law: 'direct' | 'scaled';
  /** The value now, resolved through the document. */
  value?: number;
  /** `#rrggbb`, for a colour prop. */
  color?: string;
  /** True ⟺ the value follows another prop (a slider still edits it). */
  derived: boolean;
  /** The prop it follows. */
  from?: string;
  min: number;
  max: number;
}

/** What the selection is, and every prop it exposes. */
export interface ComponentViewWire {
  status: 'ok';
  /** `master`, `instance`, `selection` or `none`. */
  role: string;
  id?: string;
  name?: string;
  master?: string;
  props: ComponentPropWire[];
  instances: number;
  can_create: boolean;
  /** The panel's headline, written by the engine. */
  headline: string;
  selection: { count: number; prose: string };
  masters: { id: string; name: string; instances: number }[];
}

/** `set_selection`'s reply: what the prompt will mean by "this". */
export interface SelectionWire {
  status: 'ok';
  count: number;
  prose: string;
}

/** One structural macro the command bar offers as a chip (RULE 2). */
export interface StructuralMacroWire {
  prompt: string;
  label: string;
  hint: string;
}

export interface AiPreviewWire {
  status: 'ok';
  prompt: string;
  /** Untyped on purpose: the panel *shows* commands, it does not read them. */
  plan: unknown[];
  notes: string[];
  attempt: number;
  applies: boolean;
}

/**
 * `DocumentSummary`, as `vectra-core` serializes it (RULE 2).
 *
 * The panel only ever *shows* this — the block a prompt is built from. It is
 * typed enough to count and label, and deliberately not more: the UI is not a
 * second model of the document.
 */
export interface DocumentSummaryWire {
  version: number;
  /** The selected node ids, draw order (Task 10.6 RULE 2). */
  selection?: string[];
  /** The active artboard, when there is one. */
  artboard?: {
    id: string;
    name: string;
    x: number;
    y: number;
    width: number;
    height: number;
  };
  nodes: {
    id: string;
    name: string;
    /** `Rectangle`, `Circle`, … — the kind tag. */
    kind: string;
    /** `Rectangle 'card'` — the label the prompt uses. */
    label: string;
    slots: { property: string; source: string; value?: number | null }[];
  }[];
  variables: { name: string; value: number }[];
  expressions: { id: string; source: string; value?: number }[];
  operations: { id: string; kind: string; inputs: string[]; enabled: boolean; name: string }[];
  constraints: {
    id: string;
    kind: string;
    targets: string[];
    strength: string;
    value?: number;
    enabled: boolean;
  }[];
  procedural: unknown[];
  tracks: unknown[];
}

/** `ExecutionReport`, as the engine builds it (Task 9.0 item 3). */
export interface AiReportInnerWire {
  prompt: string;
  attempts: number;
  plan: unknown[];
  events: EngineEventWire[];
  /** The node ids this plan re-evaluated — the incremental proof, in the panel. */
  dirty: string[];
  corrections: AiCorrectionWire[];
  notes: string[];
  /** The document *after* the plan — what the next prompt would see. */
  summary: DocumentSummaryWire;
}

/** `ai_execute_commands` / `ai_execute_with_retry` — the receipt. */
export interface AiReportWire {
  status: 'ok';
  /** One line: commands, attempts, what re-evaluated. */
  headline: string;
  /**
   * **The designer's sentence** (Task 10.6 RULE 4): "✨ Applied 3 constraints and
   * unified the shape." The panel shows this — never the plan below it.
   */
  prose: string;
  report: AiReportInnerWire;
  corrections: number;
}

/**
 * A failed AI call, typed.
 *
 * `code` is the machine-readable tag (`unrecognized-prompt`,
 * `max-retries-exceeded`, `engine-rejected`, …) — the panel keys off it and
 * never parses the message. `corrections` is present when the loop exhausted
 * its attempts, so the panel can show what kept failing instead of a shrug.
 */
export interface AiErrorWire {
  status: 'error';
  code: string;
  message: string;
  corrections: AiCorrectionWire[];
  plan: unknown[];
}

export type AiCallWire = AiPreviewWire | AiErrorWire;
export type AiExecuteWire = AiReportWire | AiErrorWire;

/** Narrow an AI envelope without repeating the discriminant everywhere. */
export function isAiError(envelope: AiCallWire | AiExecuteWire | null): envelope is AiErrorWire {
  return !!envelope && envelope.status === 'error';
}

export function isAiPreview(envelope: AiCallWire | null): envelope is AiPreviewWire {
  return !!envelope && envelope.status === 'ok' && 'applies' in envelope;
}

export function isAiReport(envelope: AiExecuteWire | null): envelope is AiReportWire {
  return !!envelope && envelope.status === 'ok' && 'report' in envelope;
}

export interface ProceduralReportWire {
  count: number;
  nodes: ProceduralNodeViewWire[];
  /** One note per node that could not compute (the Containment Law's report). */
  diagnostics: SnapshotDiagnosticWire[];
}

export interface ProceduralNodeViewWire {
  id: string;
  name: string;
  kind: string;
  enabled: boolean;
  description: string;
  /** Operand port → what it holds now, and its type. */
  operands: Record<string, ProceduralOperandViewWire>;
  /** Input port → the `node:port` it is wired to. */
  wires: Record<string, string>;
  inputs: ProceduralPortViewWire[];
  outputs: ProceduralPortViewWire[];
  upstream: string[];
}

export interface ProceduralOperandViewWire {
  /** `scalar` / `point` / `color` — what the Set control must offer. */
  ty: string;
  /** The effective value, engine-rendered (`12`, `(variable)`, `40, 20`). */
  text: string;
}

export interface ProceduralPortViewWire {
  port: string;
  ty: string;
  required: boolean;
  wired: boolean;
  /** The published value summary, for an output port. */
  value: string | null;
}

// ── Dependency inspector (`dependencies()`) ────────────────────────────

/** One graph vertex. Discriminated by `kind`; `key` is the edge reference. */
export type GraphNodeWire =
  | { kind: 'variable'; key: string; label: string; variable: string }
  | { kind: 'expression'; key: string; label: string; id: string }
  | {
      kind: 'property';
      key: string;
      label: string;
      node_id: string;
      property: string;
    }
  /* Task 6.0: motion sources are vertices too. A spring depends on the clock
     *and* on whatever its target reads; a `hover:` flag and a keyframe track get
     their own vertices, so the dirty set names them. */
  | { kind: 'state'; key: string; label: string; state: string }
  | { kind: 'track'; key: string; label: string; track: string }
  /* Task 7.0: one vertex per procedural *output port*, so a slot reading a port
     depends on it in the graph and the dirty set names it. */
  | {
      kind: 'procedural';
      key: string;
      label: string;
      node_id: string;
      port: string;
    };

/** One edge: `from` depends on `to` (both are vertex `key`s). */
export interface GraphEdgeWire {
  from: string;
  to: string;
}

export type DependencyResponseWire =
  | {
      status: 'ok';
      nodes: GraphNodeWire[];
      edges: GraphEdgeWire[];
      summary: GraphSummaryWire;
    }
  | { status: 'error'; message: string };

// ── Drawing tools (Task 10.1) ───────────────────────────────────────────
//
// The pen, the brush and the direct-selection tool are *engine* tools: every
// gesture is a `draw_*` call and every answer is one of these records. The UI
// holds no geometry — it plots the points it is given and sends the pointers it
// receives (Task 1.4's dumb remote, with the canvas as its sensor).

/** One anchor of a path, in document units. Handles are **absolute** points. */
export interface AnchorWire {
  /** `"start"` or `"segments[i].to"` — the document's own slot vocabulary. */
  slot: string;
  x: number;
  y: number;
  /** The handle arriving at this anchor (`segments[i].control2`), if any. */
  handle_in: [number, number] | null;
  /** The handle leaving this anchor (`segments[i+1].control1`), if any. */
  handle_out: [number, number] | null;
  /** `"literal"`, `"variable"`, `"expression"`, … — non-literals are owned by
   *  an expression, so the UI greys them rather than pretending a drag sticks. */
  source: string;
}

/** The pen's or brush's work in progress: exactly the geometry the engine has. */
export interface DraftWire {
  start: [number, number];
  /** `"line"`, `"quadratic"`, `"cubic"`, `"close"` — one per segment. */
  kinds: string[];
  /** Every segment's points in slot order: the preview polyline. */
  points: [number, number][];
  anchors: AnchorWire[];
  closed: boolean;
  /** Whether this draft could become a primitive **right now** (`is_snap_candidate`). */
  snap_candidate: boolean;
}

/** The reply to any `draw_*` gesture. */
export interface DrawReplyWire {
  ok: boolean;
  error?: string;
  draft?: DraftWire;
  /** Brush samples so far, `[x, y, pressure]`, for the live preview. */
  samples?: [number, number, number][];
  node_id?: string;
  segments?: number;
  fit_error?: number;
  /** Set by a Quick Shape snap: which primitive the stroke turned out to be. */
  snapped?: string;
  /** How far the *input* was from the primitive, in document units. */
  snap_error?: number;
  snap_candidate?: boolean;
}

/** `draw_overlay`: the anchors and handles of a path, for the direct-selection UI. */
export interface DrawOverlayWire {
  ok: boolean;
  anchors: AnchorWire[];
  error?: string;
}

/** `draw_hit`: what is under the pointer, in the tool's own vocabulary. */
export type DrawHitWire =
  | { kind: 'miss' }
  | {
      kind: 'anchor' | 'handle';
      slot: string;
      /** `"out"` for the handle leaving an anchor, `null` for an anchor. */
      side: 'out' | null;
      index: number;
      distance: number;
      x: number;
      y: number;
    };

/** `document_to_client`: document points in, viewport pixels out. */
export interface DocToClientWire {
  ok: boolean;
  points: [number, number][];
}
