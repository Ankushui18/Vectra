/**
 * Pure view-model helpers for the remote control (Task 2.2).
 *
 * The React component stays a dumb renderer: everything here is a total,
 * side-effect-free function from engine wire data to display strings. That
 * keeps the incrementality *visible* in the UI (dirty ids in the event log,
 * layed-out dependency rows) without the UI ever holding engine state — and it
 * makes this logic testable without a DOM (`scripts/test-ui.mjs`).
 */
import type {
  AiCallWire,
  AiCorrectionWire,
  AiExecuteWire,
  DocumentSummaryWire,
  ConstraintWire,
  OperationWire,
  DependencyResponseWire,
  EngineEventWire,
  GraphNodeWire,
  SnapshotEvalWire,
  SolverWire,
  MotionBindingWire,
  MotionWire,
  ProceduralReportWire,
  ExportEnvelopeWire,
} from './wire';
import { isAiError, isAiPreview, isAiReport } from './wire';

/** Log severity tags the Log panel styles. */
export type LogKind = 'cmd' | 'ok' | 'error' | 'info' | 'dirty';

export interface LogLine {
  kind: LogKind;
  text: string;
}

/** Short id: enough to join a log line to a layer row by eye. */
const shortId = (id: string): string => (id.length > 8 ? id.slice(0, 8) : id);

/**
 * Human line for one engine event.
 *
 * `Dirty` (Task 2.2) gets prose — it is the whole point of the panel: the log
 * shows *which* nodes the last edit re-evaluated, and an empty id list is
 * spelled out rather than rendered as `[]`. Every other event stays verbatim
 * so the log remains a faithful transcript of the wire.
 */
export function formatEvent(e: EngineEventWire): LogLine {
  if (e.type === 'Dirty') {
    if (e.mode === 'full') {
      return { kind: 'info', text: `⟳ full re-evaluation: ${e.ids.length} node(s)` };
    }
    if (e.ids.length === 0) {
      return { kind: 'info', text: '⛓ dirty: 0 nodes — nothing depends on this edit' };
    }
    return {
      kind: 'dirty',
      text: `⛓ dirty: ${e.ids.length} node(s) re-evaluated — ${e.ids
        .map(shortId)
        .join(', ')}`,
    };
  }
  if (e.type === 'DragStarted') {
    return { kind: 'ok', text: `✋ drag started: ${shortId(e.node_id)}` };
  }
  if (e.type === 'DragEnded') {
    return { kind: 'ok', text: `✊ drag ended: ${shortId(e.node_id)}` };
  }
  if (e.type === 'ConstraintsUpdated' && e.ids.length > 0) {
    return {
      kind: 'ok',
      text: `⊞ constraints: ${e.ids.map(shortId).join(', ')}`,
    };
  }
  if (e.type === 'OperationsUpdated' && e.ids.length > 0) {
    return {
      kind: 'ok',
      text: `⊞ operations: ${e.ids.map(shortId).join(', ')}`,
    };
  }
  return { kind: 'ok', text: `✓ ${JSON.stringify(e)}` };
}

/**
 * The ids of the last `Dirty` event in a response, or `null` when the response
 * carried none (rejections, no-op reads). The UI keeps the ids so it can mark
 * the affected layer rows — the "which nodes moved" half of the requirement.
 */
export function dirtyIdsOf(events: EngineEventWire[]): string[] | null {
  let ids: string[] | null = null;
  for (const e of events) {
    if (e.type === 'Dirty') ids = e.ids;
  }
  return ids;
}

/** Minimal shape of a layer the dependency panel needs for labels. */
export interface LayerName {
  name: string;
}

/**
 * Edge rows for the dependency inspector, already labelled.
 *
 * Property vertices are joined to their layer by `node_id`, so the panel reads
 * `bound-rect • width → depends on → ƒ 3f2a1b7c`, falling back to the short id
 * for layers the scene no longer holds. Vertex order is the engine's, so the
 * panel is a faithful projection — no re-sorting, no graph logic in the UI.
 */
export function dependencyRows(
  deps: DependencyResponseWire | null,
  layers: Record<string, LayerName>,
): { fromKey: string; from: string; toKey: string; to: string }[] {
  if (!deps || deps.status !== 'ok') return [];
  const byKey = new Map<string, GraphNodeWire>();
  for (const node of deps.nodes) byKey.set(node.key, node);
  const label = (key: string): string => {
    const node = byKey.get(key);
    if (!node) return key;
    if (node.kind !== 'property') return node.label;
    const layer = layers[node.node_id];
    return `${layer ? layer.name : shortId(node.node_id)} • ${node.property}`;
  };
  return deps.edges.map((e) => ({
    fromKey: e.from,
    from: label(e.from),
    toKey: e.to,
    to: label(e.to),
  }));
}

/** Which primitive slot a menu action addresses on an axis. */
export type Axis = 'x' | 'y';

/**
 * The property a constraint on `axis` addresses for a given primitive.
 *
 * This is *wire vocabulary*, not geometry: the same table the engine's
 * `Node::get_param` accepts for rectangles (`x y`) and circles/arcs
 * (`cx cy`). Paths and groups expose no float slots in Phase 1, so they cannot
 * take a constraint — the UI says so instead of sending a doomed command.
 */
export function slotFor(axis: Axis, primitive: string): string | null {
  switch (primitive) {
    case 'rect':
      return axis;
    case 'circle':
    case 'arc':
      return axis === 'x' ? 'cx' : 'cy';
    default:
      return null;
  }
}

/** One constraint, reduced to what the inspector renders. */
export interface ConstraintRow {
  id: string;
  kind: string;
  /** e.g. `vertical · medium` */
  meta: string;
  /** Engine-rendered description of the rule (targets + operand). */
  detail: string;
  enabled: boolean;
  /** True when the solver parked the rule (an over-constrained loser). */
  dropped: boolean;
}

/**
 * Inspector rows for the constraint registry, in the engine's registration
 * order (the map arrives canonically keyed; the UI does not re-sort engine
 * state beyond grouping, so what the panel shows is what the solver holds).
 */
export function constraintRows(constraints: Record<string, ConstraintWire>): ConstraintRow[] {
  return Object.values(constraints)
    .map((c) => ({
      id: c.id,
      kind: c.kind,
      meta: `${c.kind} · ${c.strength ?? 'medium'}`,
      detail: c.description ?? `${c.kind} ${c.targets.map((t) => t.property).join(' ↔ ')}`,
      enabled: c.enabled !== false,
      dropped: c.enabled === false,
    }))
    .sort((a, b) => (a.kind === b.kind ? a.id.localeCompare(b.id) : a.kind.localeCompare(b.kind)));
}

/** The solver line: what the tableau currently holds. */
export function solverSummary(solver: {
  variables: number;
  constraints: number;
  writes: number;
  dropped: number;
}): string {
  return (
    `${solver.variables} var · ${solver.constraints} rule` +
    (solver.constraints === 1 ? '' : 's') +
    ` · ${solver.writes} moved` +
    (solver.dropped > 0 ? ` · ${solver.dropped} dropped` : '')
  );
}

/**
 * The live gesture, as one line — or `null` when no drag is open.
 *
 * `active_edits` is the engine's own count of open Cassowary edit variables, so
 * the panel can prove the gesture really is a native edit session (2 pointer
 * slots) rather than a stream of re-solved writes.
 */
export function dragStatus(
  solver: SolverWire | null | undefined,
  layers: Record<string, LayerName>,
): string | null {
  if (!solver || !solver.drag_node) return null;
  const layer = layers[solver.drag_node];
  const name = layer ? layer.name : shortId(solver.drag_node);
  return `✋ dragging ${name} — ${solver.active_edits} edit variable(s) live`;
}

/**
 * The eval chip: the engine's incrementality bookkeeping as one line.
 *
 * `full ×1` across a session is the claim; `inc ×N` is the evidence that every
 * later edit patched instead of rebuilt.
 */
export function evalSummary(evalWire: SnapshotEvalWire): string {
  return (
    `${evalWire.last_dirty} dirty → ${evalWire.last_evaluated} evaluated · ` +
    `full ×${evalWire.full_evals} · inc ×${evalWire.incremental_evals}` +
    (evalWire.no_ops > 0 ? ` · no-op ×${evalWire.no_ops}` : '')
  );
}

// ── Task 4.0: non-destructive operations ────────────────────────────────
//
// An operation is a VIRTUAL node: the engine mints its id, evaluates it into
// the same `EvaluatedScene` a primitive lives in, and keeps its sources in the
// document untouched (RULE 1, RULE 3). The UI's whole job here is to pick a
// kind, name the operands and render what comes back — every number, caption
// and nested row below is the engine's, not the panel's.

/** The set-operation glyphs, keyed by the engine's own `description`. */
const OPERATION_GLYPHS: Record<string, string> = {
  union: '∪',
  subtract: '−',
  intersect: '∩',
  exclude: '⊕',
};

/** The glyph a row shows: the engine's operator tag for booleans, the kind
 *  tag for modifiers (`offset`, `fillet`, `mirror`). */
export function operationGlyph(op: OperationWire): string {
  return OPERATION_GLYPHS[op.description] ?? op.kind;
}

/** `A − B`, built from the engine's `input_names` in operand order. */
export function operationOperands(op: OperationWire): string {
  const glyph = operationGlyph(op);
  return op.input_names.join(` ${glyph} `);
}

export interface OperationRow {
  id: string;
  /** The engine's registry name (`union 1`). */
  name: string;
  /** `boolean` / `offset` / `fillet` / `mirror`. */
  kind: string;
  glyph: string;
  /** The engine-rendered one-liner (`subtract`, `fillet r=4`). */
  description: string;
  /** `A − B`, from the engine's own operand names. */
  operands: string;
  /** Source ids, in operand order — what the panel's toggle/delete forward. */
  inputs: string[];
  enabled: boolean;
}

/**
 * Rows for the Operations panel, in the engine's registration order.
 *
 * Order comes from the engine's own `inputs` fan-out, so the panel is a
 * projection: no sorting by kind here (unlike constraints, which the engine
 * keys canonically and the inspector groups).
 */
export function operationRows(operations: Record<string, OperationWire>): OperationRow[] {
  return Object.values(operations).map((op) => ({
    id: op.id,
    name: op.name,
    kind: op.kind,
    glyph: operationGlyph(op),
    description: op.description,
    operands: operationOperands(op),
    inputs: op.inputs,
    enabled: op.enabled !== false,
  }));
}

/** One row of the Layers tree. */
export interface LayerRow {
  id: string;
  name: string;
  /** Primitive tag for authored nodes, `kind` for operations — the chip. */
  tag: string;
  /** 0 for a source, 1 for an operation nested beneath the source it reads. */
  depth: number;
  /** True for a virtual `OperationNode` (RULE 3). */
  isOperation: boolean;
  /** For an operation row: `A − B`; `null` for sources. */
  operands: string | null;
  /** For an operation row: the display name of the source it sits under. */
  parent: string | null;
}

/**
 * The Layers tree: authored sources in the panel's order, each operation
 * indented directly beneath the source it reads FIRST (`inputs[0]`).
 *
 * The nesting is *presentation of an engine fact*, not graph logic: the op's
 * inputs are the engine's, and an operation whose first input is missing from
 * the scene is listed at the end rather than invented under a neighbour.
 * `zOrder` arrives top-most-last, so the panel reverses it — the same order the
 * canvas paints in.
 */
export function layerRows(
  zOrder: string[],
  nodes: Record<string, { name: string; primitive: { type: string } }>,
  operations: Record<string, OperationWire>,
): LayerRow[] {
  const ops = Object.values(operations);
  const placed = new Set<string>();
  const rows: LayerRow[] = [];
  for (const id of [...zOrder].reverse()) {
    const node = nodes[id];
    const op = operations[id];
    if (op) {
      // Operations are appended to the draw order after every source; the tree
      // claims them under their own source instead, so skip them here.
      continue;
    }
    if (!node) continue;
    rows.push({
      id,
      name: node.name,
      tag: node.primitive.type,
      depth: 0,
      isOperation: false,
      operands: null,
      parent: null,
    });
    for (const candidate of ops) {
      if (candidate.inputs[0] !== id || placed.has(candidate.id)) continue;
      placed.add(candidate.id);
      rows.push({
        id: candidate.id,
        name: candidate.name,
        tag: candidate.kind,
        depth: 1,
        isOperation: true,
        operands: operationOperands(candidate),
        parent: node.name,
      });
    }
  }
  for (const candidate of ops) {
    if (placed.has(candidate.id)) continue;
    rows.push({
      id: candidate.id,
      name: candidate.name,
      tag: candidate.kind,
      depth: 1,
      isOperation: true,
      operands: operationOperands(candidate),
      parent: null,
    });
  }
  return rows;
}

// ── Motion (Task 6.0) ──────────────────────────────────────────────────

/** One binding, as a line in the Motion panel. */
export interface MotionRow {
  nodeId: string;
  property: string;
  kind: MotionBindingWire['kind'];
  /** `spring width → 320 (170 / 26)` — everything a designer needs, at a glance. */
  detail: string;
  /** The slot's live value at the current clock, formatted, or `—`. */
  value: string;
  /** True while this binding is still driving the scene. */
  active: boolean;
}

const num = (v: number | null, digits = 2): string =>
  v === null || !Number.isFinite(v) ? '—' : String(Number(v.toFixed(digits)));

/**
 * The Motion panel's rows.
 *
 * "Active" is a per-row projection: a spring is active while it is further from
 * its target than the engine's own settle epsilon, everything else while the
 * scene as a whole is moving. The UI asks the engine for `animating` as the
 * whole-scene signal; this line is only for display, so it is allowed to be a
 * projection rather than a second opinion.
 */
export function motionRows(motion: MotionWire | null): MotionRow[] {
  if (!motion) return [];
  return motion.bindings.map((b) => {
    let detail: string;
    let active: boolean;
    switch (b.kind) {
      case 'spring': {
        detail = `spring ${b.property} → ${num(b.target)} (${num(b.stiffness, 0)} / ${num(
          b.damping,
          0,
        )})${b.state ? ` on ${shortFlag(b.state)}` : ''}`;
        // A spring is at rest once it is within the engine's own epsilon of its
        // target; the row mirrors that rule rather than inventing one.
        active =
          b.target !== null && b.value !== null
            ? Math.abs(b.value - b.target) > motion.epsilon
            : motion.animating;
        break;
      }
      case 'state':
        detail = `state ${b.property} ← ${shortFlag(b.state ?? '')}`;
        active = motion.animating;
        break;
      case 'track':
      default:
        detail = `track ${b.property} ← ${b.track ?? ''}:${b.channel ?? ''}`;
        active = motion.animating;
        break;
    }
    return {
      nodeId: b.node_id,
      property: b.property,
      kind: b.kind,
      detail,
      value: num(b.value),
      active,
    };
  });
}

// ── Task 7.0: the procedural graph ─────────────────────────────────────
//
// The panel is a projection of `procedural_json()` — the engine's own view of
// the chain: registration order, effective operands, wired inputs, published
// ports. Nothing here computes a geometry value, a port type or a default; the
// only things this file formats are things the engine already said.

/** One row per node, in evaluation order (the registry's own). */
export interface ProceduralRow {
  id: string;
  short: string;
  name: string;
  kind: string;
  /** Engine-rendered effective operands (`grid n=3 m=2 s=10 @(0, 0)`). */
  description: string;
  enabled: boolean;
  /** Operand ports, each with what it holds and its type. */
  operands: { port: string; ty: string; text: string }[];
  /** Input ports that a wire could feed, and whether one already does. */
  inputs: { port: string; ty: string; required: boolean; wired: boolean }[];
  /** Input port → `node:port`, for the row's wiring line. */
  wires: Record<string, string>;
  /** Output ports with their published values (`region 1 rings, 4 pts`). */
  outputs: { port: string; ty: string; value: string | null }[];
  /** True while every required input is fed. */
  ready: boolean;
}

export function proceduralRows(report: ProceduralReportWire | null): ProceduralRow[] {
  if (!report) return [];
  return report.nodes.map((node) => ({
    id: node.id,
    short: node.id.slice(0, 8),
    name: node.name,
    kind: node.kind,
    description: node.description,
    enabled: node.enabled,
    operands: Object.entries(node.operands).map(([port, operand]) => ({
      port,
      ty: operand.ty,
      text: operand.text,
    })),
    inputs: node.inputs.map((port) => ({
      port: port.port,
      ty: port.ty,
      required: port.required,
      wired: port.wired,
    })),
    wires: node.wires,
    outputs: node.outputs.map((port) => ({
      port: port.port,
      ty: port.ty,
      value: port.value,
    })),
    ready: node.inputs.every((port) => !port.required || port.wired),
  }));
}

/**
 * Every output port a wire could read, across the whole chain — the connect
 * control's source list. `label` is what the dropdown shows; `value` is the
 * `node:port` the command sends.
 */
export interface ProceduralPortOption {
  value: string;
  label: string;
  nodeId: string;
  port: string;
  ty: string;
}

export function proceduralPortOptions(report: ProceduralReportWire | null): ProceduralPortOption[] {
  if (!report) return [];
  return report.nodes.flatMap((node) =>
    node.outputs.map((port) => ({
      value: `${node.id}:${port.port}`,
      label: `${node.name} • ${port.port}`,
      nodeId: node.id,
      port: port.port,
      ty: port.ty,
    })),
  );
}

/**
 * The Procedural panel's headline: how many nodes, how many are drawing, and how
 * many the pass could not compute.
 *
 * `drawing` is counted the only way the UI can — a node contributes geometry
 * exactly while it is enabled *and* its geometry port has a published value.
 */
export function proceduralSummary(report: ProceduralReportWire | null): string {
  if (!report) return 'procedural —';
  if (report.count === 0) return 'no nodes — the graph is empty';
  const rows = proceduralRows(report);
  const drawing = rows.filter(
    (row) => row.enabled && row.outputs.some((port) => port.ty === 'region' && port.value !== null),
  ).length;
  const parked = rows.filter((row) => !row.enabled).length;
  const failing = report.diagnostics.length;
  const parts = [`${report.count} node${report.count === 1 ? '' : 's'}`, `${drawing} drawing`];
  if (parked > 0) parts.push(`${parked} parked`);
  if (failing > 0) parts.push(`${failing} failing`);
  return parts.join(' · ');
}

/**
 * A flag's short form for the UI: `hover:1f0c9d2a-…` → `hover • 1f0c9d2a`.
 *
 * The engine names a per-node hover flag `hover:<uuid>`; the panel has no room
 * for a uuid and the reader only needs to tell two flags apart.
 */
/** What the Export section says before the modal is opened. */
export function exportSummary(envelope: ExportEnvelopeWire | null): string {
  if (!envelope) return 'export —';
  if (envelope.status !== 'ok') return `export failed — ${envelope.message ?? 'unknown'}`;
  const bytes = byteSize(envelope.code);
  const warn = envelope.warnings.length
    ? ` · ${envelope.warnings.length} warning${envelope.warnings.length === 1 ? '' : 's'}`
    : '';
  return `${envelope.format} · ${bytes}${warn}`;
}

// ── The AI panel (Task 9.0) ───────────────────────────────────────────
//
// Display only. The panel never interprets a command — it prints the JSON the
// engine returned, one *row* per command so a plan is readable at a glance, and
// it labels each command from the command's own `type` field. No geometry, no
// ids invented, nothing resolved here (Task 1.4's strict boundary).

export interface AiPlanRow {
  /** 1-based position in the plan. */
  index: number;
  /** The command's own tag: `CreateNode`, `SetParameter`, … */
  tag: string;
  /** The id or slot the row addresses, when it names one. */
  target: string;
  /** The command's JSON, verbatim — the row's tooltip. */
  json: string;
}

/** The id-ish field a command carries, for the row's `target` column. */
const AI_TARGET_FIELDS = [
  'node_id',
  'id',
  'property',
  'name',
  'track_id',
  'source',
] as const;

function firstStringField(command: Record<string, unknown>): string {
  for (const field of AI_TARGET_FIELDS) {
    const value = command[field];
    if (typeof value === 'string' && value) return value.length > 12 ? `${value.slice(0, 8)}…` : value;
  }
  // A nested object (a constraint, a procedural node) names its own id.
  for (const field of ['constraint', 'node', 'track']) {
    const nested = command[field];
    if (nested && typeof nested === 'object') {
      const inner = firstStringField(nested as Record<string, unknown>);
      if (inner) return inner;
    }
  }
  const inputs = command.inputs;
  if (Array.isArray(inputs) && inputs.length) return `${inputs.length} input(s)`;
  return '';
}

/**
 * One row per command, in order, straight off the wire.
 *
 * Reads `type` and the first id-ish field it finds — which is *presentation*,
 * not parsing: the JSON below the row is the same JSON the engine will be
 * handed, so a wrong label can never become a wrong command.
 */
export function aiPlanRows(plan: unknown[]): AiPlanRow[] {
  return plan.map((entry, position) => {
    const command = (entry ?? {}) as Record<string, unknown>;
    const tag = typeof command.type === 'string' ? command.type : 'unknown';
    return {
      index: position + 1,
      tag,
      target: firstStringField(command),
      json: JSON.stringify(entry),
    };
  });
}

export interface AiCorrectionRow {
  attempt: number;
  code: string;
  error: string;
  /** The command that was refused, as one line, when the wire carried it. */
  command: string;
  note: string;
}

/** The self-correction history, formatted for the receipt. */
export function aiCorrectionRows(corrections: AiCorrectionWire[]): AiCorrectionRow[] {
  return corrections.map((correction) => ({
    attempt: correction.attempt,
    code: correction.code,
    error: correction.error,
    command:
      correction.command === undefined ? '' : JSON.stringify(correction.command) ?? '',
    note: correction.note ?? '',
  }));
}

export interface AiPanelState {
  /** The section's one-line status. */
  summary: string;
  /** The typed code, when the last call failed. */
  code: string | null;
  /** Rows for the plan under the prompt — the preview, or a failed plan. */
  plan: AiPlanRow[];
  /** The receipt's headline, when something actually ran. */
  headline: string | null;
  /** How many attempts the loop took (1 when nothing needed correcting). */
  attempts: number | null;
  /** The node ids the run re-evaluated — the incrementality, in the panel. */
  dirty: string[];
  corrections: AiCorrectionRow[];
  notes: string[];
}

/**
 * Fold the last two envelopes into what the panel draws.
 *
 * `preview` is the plan on the table; `receipt` is what the engine said after
 * running it (absent while a plan is only being previewed). Both are wire data
 * and nothing else.
 */
export function aiPanelState(
  preview: AiCallWire | null,
  receipt: AiExecuteWire | null,
): AiPanelState {
  // The plan on the table is the preview's; when the *loop* gives up, the plan
  // it kept failing on arrives in the error envelope instead — show that rather
  // than an empty panel.
  const fromPreview = isAiError(preview)
    ? preview.plan
    : isAiPreview(preview)
      ? preview.plan
      : null;
  const fromReceipt = isAiError(receipt)
    ? receipt.plan
    : isAiReport(receipt)
      ? receipt.report.plan
      : null;
  const plan = aiPlanRows(fromPreview?.length ? fromPreview : (fromReceipt ?? []));
  const corrections = aiCorrectionRows(
    isAiError(receipt)
      ? receipt.corrections
      : isAiReport(receipt)
        ? receipt.report.corrections
        : isAiError(preview)
          ? preview.corrections
          : [],
  );

  if (isAiError(receipt)) {
    return {
      summary: `refused — ${receipt.code}`,
      code: receipt.code,
      plan,
      headline: null,
      attempts: null,
      dirty: [],
      corrections,
      notes: [],
    };
  }
  if (isAiReport(receipt)) {
    return {
      summary: receipt.headline,
      code: null,
      plan: aiPlanRows(receipt.report.plan),
      headline: receipt.headline,
      attempts: receipt.report.attempts,
      dirty: receipt.report.dirty,
      corrections,
      notes: receipt.report.notes,
    };
  }
  if (isAiError(preview)) {
    return {
      summary: `cannot plan — ${preview.code}`,
      code: preview.code,
      plan,
      headline: null,
      attempts: null,
      dirty: [],
      corrections,
      notes: [],
    };
  }
  if (isAiPreview(preview)) {
    return {
      summary: `${plan.length} command(s) ready — nothing applied`,
      code: null,
      plan,
      headline: null,
      attempts: preview.attempt,
      dirty: [],
      corrections,
      notes: preview.notes,
    };
  }
  return {
    summary: 'idle — describe an edit, then Generate',
    code: null,
    plan: [],
    headline: null,
    attempts: null,
    dirty: [],
    corrections: [],
    notes: [],
  };
}

/**
 * `Rectangle 'card'`, `Circle 'dot'` … — the node labels in the summary block.
 *
 * The panel lists them under *Show the system prompt*, so a reader can see the
 * grounding the model was given without reading 200 lines of prompt prose.
 */
export function summaryLabels(summary: DocumentSummaryWire | null): string[] {
  return summary ? summary.nodes.map((node) => node.label) : [];
}

/** `3 node(s) · 1 variable(s) · 0 constraint(s)` — the panel's context line. */
export function summaryCounts(summary: DocumentSummaryWire | null): string {
  if (!summary) return 'no summary';
  const part = (count: number, noun: string) =>
    `${count} ${noun}${count === 1 ? '' : 's'}`;
  return [
    part(summary.nodes.length, 'node'),
    part(summary.variables.length, 'variable'),
    part(summary.expressions.length, 'expression'),
    part(summary.constraints.length, 'constraint'),
  ].join(' · ');
}

/** Human byte size — the modal footer, not a measurement. */
function byteSize(text: string): string {
  const bytes = new TextEncoder().encode(text).length;
  if (bytes < 1024) return `${bytes} B`;
  return `${(bytes / 1024).toFixed(1)} kB`;
}

export function shortFlag(flag: string): string {
  const [head, ...rest] = flag.split(':');
  const tail = rest.join(':');
  if (!tail) return head;
  return `${head} • ${tail.slice(0, 8)}`;
}

/**
 * The Motion panel's headline: is the engine moving, and until when.
 *
 * `horizon` is a *time*, so the answer to "how long will this take?" is
 * `horizon − now` — the number the frame loop would otherwise have to discover
 * by drawing frames until something stops changing.
 */
export function motionSummary(motion: MotionWire | null): string {
  if (!motion) return 'motion —';
  if (motion.bindings.length === 0) return 'no bindings — nothing is animated';
  if (!motion.animating) {
    return `idle · ${motion.bindings.length} binding${
      motion.bindings.length === 1 ? '' : 's'
    } at rest`;
  }
  const remaining = motion.horizon === null ? null : Math.max(0, motion.horizon - motion.time);
  return `animating · stops in ${remaining === null ? '—' : `${remaining.toFixed(2)}s`}`;
}

/**
 * The state flags currently on, as chips. They are inputs, never history, so
 * this list is the honest answer to "what is the UI currently telling the
 * engine?" — and it is what a hover looks like from the engine's side.
 */
export function stateChips(motion: MotionWire | null): string[] {
  if (!motion) return [];
  return motion.states.map(shortFlag);
}
