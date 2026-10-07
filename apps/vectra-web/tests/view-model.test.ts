/**
 * UI view-model tests (Task 2.2 §4).
 *
 * The requirement is that the React UI *visibly* demonstrates incremental
 * updates: the event log names the nodes marked dirty and the canvas updates
 * without a full re-evaluation. The rendering half is trivial JSX; the part
 * that can be wrong is the projection of engine wire data into display text —
 * so that is what is tested here, with no DOM and no framework.
 *
 * Run: `npm run test:ui` (bundles with esbuild, executes under `node --test`).
 */
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { beginDrag, endDrag, updateDrag } from '../src/engine/commands';
import { engineIsOlderThanUi, VectraClient } from '../src/engine/client';
import {
  aiCorrectionRows,
  aiPanelState,
  aiPlanRows,
  constraintRows,
  dependencyRows,
  dirtyIdsOf,
  dragStatus,
  evalSummary,
  formatEvent,
  layerRows,
  motionRows,
  motionSummary,
  exportSummary,
  operationRows,
  proceduralPortOptions,
  proceduralRows,
  proceduralSummary,
  shortFlag,
  slotFor,
  solverSummary,
  stateChips,
  summaryCounts,
  summaryLabels,
} from '../src/engine/view-model';
import type {
  AiCallWire,
  AiExecuteWire,
  DocumentSummaryWire,
  ConstraintWire,
  DependencyResponseWire,
  EngineEventWire,
  MotionWire,
  OperationWire,
  ProceduralReportWire,
  SnapshotEvalWire,
} from '../src/engine/wire';

const A = 'aaaaaaaa-1111-4111-8111-aaaaaaaaaaaa';
const B = 'bbbbbbbb-2222-4222-8222-bbbbbbbbbbbb';

test('a Dirty event names the nodes the engine re-evaluated', () => {
  const line = formatEvent({ type: 'Dirty', ids: [A, B], mode: 'incremental' });
  assert.equal(line.kind, 'dirty');
  assert.match(line.text, /2 node\(s\) re-evaluated/);
  // Short ids are enough to join a log line to a Layers row.
  assert.match(line.text, /aaaaaaaa/);
  assert.match(line.text, /bbbbbbbb/);
  assert.doesNotMatch(line.text, /aaaaaaaa-1111/);
});

test('an edit nothing depends on says so instead of printing an empty list', () => {
  const line = formatEvent({ type: 'Dirty', ids: [], mode: 'incremental' });
  assert.equal(line.kind, 'info');
  assert.match(line.text, /0 nodes/);
  assert.match(line.text, /nothing depends/);
});

test('a full sweep is labelled as one, never as an incremental patch', () => {
  const line = formatEvent({ type: 'Dirty', ids: [A, B], mode: 'full' });
  assert.equal(line.kind, 'info');
  assert.match(line.text, /full re-evaluation/);
});

test('other events stay verbatim — the log is a wire transcript', () => {
  const line = formatEvent({ type: 'NodesUpdated', ids: [A] });
  assert.equal(line.kind, 'ok');
  assert.match(line.text, /"type":"NodesUpdated"/);
  assert.match(line.text, new RegExp(A));
});

test('dirtyIdsOf reports the last Dirty event, null when there is none', () => {
  const events: EngineEventWire[] = [
    { type: 'VariablesUpdated', names: ['base'] },
    { type: 'Dirty', ids: [A], mode: 'incremental' },
    { type: 'Dirty', ids: [B], mode: 'incremental' },
  ];
  assert.deepEqual(dirtyIdsOf(events), [B]);
  assert.equal(dirtyIdsOf([{ type: 'OrderChanged' }]), null);
  assert.deepEqual(dirtyIdsOf([]), null);
});

test('dependency rows join property vertices to layer names', () => {
  const deps: DependencyResponseWire = {
    status: 'ok',
    nodes: [
      { kind: 'variable', key: 'var:base', label: '$base', variable: 'base' },
      {
        kind: 'expression',
        key: `expr:${A}`,
        label: `ƒ ${A.slice(0, 8)}`,
        id: A,
      },
      {
        kind: 'property',
        key: `prop:${B}:width`,
        label: `prop ${B.slice(0, 8)} • width`,
        node_id: B,
        property: 'width',
      },
    ],
    edges: [
      { from: `prop:${B}:width`, to: `expr:${A}` },
      { from: `expr:${A}`, to: 'var:base' },
    ],
    summary: { nodes: 3, edges: 2, acyclic: true },
  };

  const rows = dependencyRows(deps, { [B]: { name: 'bound-rect' } });
  assert.equal(rows.length, 2, 'one row per edge, in engine order');
  assert.equal(rows[0].from, 'bound-rect • width');
  assert.equal(rows[0].to, `ƒ ${A.slice(0, 8)}`);
  assert.equal(rows[1].from, `ƒ ${A.slice(0, 8)}`);
  assert.equal(rows[1].to, '$base');
  assert.ok(rows[0].fromKey.includes(B), 'keys stay available for React list keys');
});

test('an unknown layer degrades to a short id rather than a hole', () => {
  const deps: DependencyResponseWire = {
    status: 'ok',
    nodes: [
      {
        kind: 'property',
        key: `prop:${B}:radius`,
        label: `prop ${B.slice(0, 8)} • radius`,
        node_id: B,
        property: 'radius',
      },
    ],
    edges: [{ from: `prop:${B}:radius`, to: 'var:base' }],
    summary: { nodes: 1, edges: 1, acyclic: true },
  };
  const rows = dependencyRows(deps, {});
  assert.equal(rows[0].from, 'bbbbbbbb • radius');
  // An unresolved key (graph and snapshot momentarily disagree) is passed
  // through untouched — the UI never invents a label.
  assert.equal(rows[0].to, 'var:base');
  assert.deepEqual(dependencyRows({ status: 'error', message: 'nope' }, {}), []);
  assert.deepEqual(dependencyRows(null, {}), []);
});

test('the eval chip reports the incrementality claim and its cost', () => {
  const stats: SnapshotEvalWire = {
    last_mode: 'incremental',
    last_dirty: 2,
    last_evaluated: 1,
    full_evals: 1,
    incremental_evals: 17,
    no_ops: 3,
  };
  const text = evalSummary(stats);
  assert.match(text, /2 dirty → 1 evaluated/);
  assert.match(text, /full ×1/);
  assert.match(text, /inc ×17/);
  assert.match(text, /no-op ×3/);
  // A pristine session has no no-op suffix to explain.
  assert.doesNotMatch(evalSummary({ ...stats, no_ops: 0 }), /no-op/);
});


// ── Task 3.1: constraints ──────────────────────────────────────────────
//
// The UI never solves anything — the engine owns the tableau. What these
// helpers owe the user is the *vocabulary bridge*: which slot a menu action
// addresses, what the registry looks like, and whether the solver parked a
// rule. Those are the three places the dumb remote can get it wrong.

const C1 = 'cccccccc-3333-4333-8333-cccccccccccc';
const C2 = 'dddddddd-4444-4444-8444-dddddddddddd';

test('slotFor is wire vocabulary: it names the slot the engine accepts', () => {
  // Rectangles carry `x`/`y`; circles and arcs carry `cx`/`cy` — exactly the
  // spellings `Node::get_param` resolves.
  assert.equal(slotFor('x', 'rect'), 'x');
  assert.equal(slotFor('y', 'rect'), 'y');
  assert.equal(slotFor('x', 'circle'), 'cx');
  assert.equal(slotFor('y', 'circle'), 'cy');
  assert.equal(slotFor('x', 'arc'), 'cx');
  assert.equal(slotFor('y', 'arc'), 'cy');
  // Phase-1 paths expose no float slot: `null` makes the caller refuse the
  // action instead of sending a command the engine would reject.
  assert.equal(slotFor('x', 'path'), null);
  assert.equal(slotFor('y', 'path'), null);
  assert.equal(slotFor('x', 'group'), null);
});

test('constraint rows sort by kind and surface the engine description', () => {
  const constraints: Record<string, ConstraintWire> = {
    [C2]: {
      id: C2,
      kind: 'vertical',
      targets: [
        { node_id: A, property: 'x' },
        { node_id: B, property: 'x' },
      ],
      strength: 'medium',
      enabled: true,
      description: `vertical [${C2.slice(0, 8)}]: ${A.slice(0, 8)}.x == ${B.slice(0, 8)}.x`,
    },
    [C1]: {
      id: C1,
      kind: 'distance',
      targets: [
        { node_id: A, property: 'x' },
        { node_id: B, property: 'x' },
      ],
      strength: 'weak',
      value: 100,
      // enabled:false is how the engine reports a rule the solver parked —
      // either the user paused it or it lost an over-constrained pass.
      enabled: false,
      description: `distance [${C1.slice(0, 8)}] = 100`,
    },
  };

  const rows = constraintRows(constraints);
  assert.deepEqual(
    rows.map((r) => r.id),
    [C1, C2],
    'distance sorts before vertical — engine order is not relied on',
  );
  assert.equal(rows[0].meta, 'distance · weak');
  assert.equal(rows[0].detail, `distance [${C1.slice(0, 8)}] = 100`);
  assert.equal(rows[0].enabled, false);
  assert.equal(rows[0].dropped, true, 'a parked rule is shown as dropped');
  assert.equal(rows[1].meta, 'vertical · medium');
  assert.equal(rows[1].dropped, false);
  assert.match(rows[1].detail, /\.x == /);
  assert.deepEqual(constraintRows({}), []);
});

test('a constraint with no engine description still renders from its targets', () => {
  const rows = constraintRows({
    [C1]: {
      id: C1,
      kind: 'coincident',
      targets: [
        { node_id: A, property: 'cx' },
        { node_id: B, property: 'cx' },
      ],
      // No strength and no description: the defaults the wire promises.
    },
  });
  assert.equal(rows[0].meta, 'coincident · medium');
  assert.equal(rows[0].enabled, true);
  assert.equal(rows[0].detail, 'coincident cx ↔ cx');
});

test('the solver line reports the tableau, singular or plural', () => {
  const base = {
    variables: 2,
    constraints: 1,
    edit_variables: 1,
    writes: 1,
    dropped: 0,
    skipped: 0,
  };
  assert.equal(solverSummary(base), '2 var · 1 rule · 1 moved');
  // After undo the tableau is empty — the undo law's observable.
  assert.equal(
    solverSummary({ ...base, variables: 0, constraints: 0, writes: 0 }),
    '0 var · 0 rules · 0 moved',
  );
  // A dropped rule is called out; zero dropped is not mentioned.
  assert.equal(
    solverSummary({ ...base, constraints: 2, writes: 3, dropped: 2 }),
    '2 var · 2 rules · 3 moved · 2 dropped',
  );
});

test('a ConstraintsUpdated event reads as a constraint line, not raw JSON', () => {
  const line = formatEvent({ type: 'ConstraintsUpdated', ids: [C1, C2] });
  assert.equal(line.kind, 'ok');
  assert.equal(line.text, `⊞ constraints: ${C1.slice(0, 8)}, ${C2.slice(0, 8)}`);
  assert.doesNotMatch(line.text, /cccccccc-3333/);
  // With no ids the event stays a wire transcript rather than a bare ⊞.
  assert.equal(
    formatEvent({ type: 'ConstraintsUpdated', ids: [] }).text,
    '✓ {"type":"ConstraintsUpdated","ids":[]}',
  );
});

// ── Task 3.2: direct manipulation ────────────────────────────────────────

test('the drag triad is three commands — the UI never sends geometry', () => {
  // RULE 3: these three builders are the whole drag protocol. Each one is a
  // verb plus the node it addresses; no builder resolves a parameter, reads a
  // constraint, or does arithmetic on the scene.
  assert.deepEqual(beginDrag(A), { type: 'BeginDrag', node_id: A });
  assert.deepEqual(updateDrag(A, 250, 40), {
    type: 'UpdateDrag',
    node_id: A,
    x: 250,
    y: 40,
  });
  assert.deepEqual(endDrag(A), { type: 'EndDrag', node_id: A });
  // Absolute coordinates go on the wire untouched — no offset is baked in
  // here: the caller already subtracted the grab offset.
  assert.deepEqual(updateDrag(B, -12.5, 0), {
    type: 'UpdateDrag',
    node_id: B,
    x: -12.5,
    y: 0,
  });
});

test('a gesture is announced and closed in prose', () => {
  const started = formatEvent({ type: 'DragStarted', node_id: A });
  assert.equal(started.kind, 'ok');
  assert.equal(started.text, '✋ drag started: aaaaaaaa');
  const ended = formatEvent({ type: 'DragEnded', node_id: A });
  assert.equal(ended.kind, 'ok');
  assert.equal(ended.text, '✊ drag ended: aaaaaaaa');
});

test('the drag indicator reads the engine, and stays silent when idle', () => {
  const solver = {
    variables: 2,
    constraints: 1,
    edit_variables: 3,
    writes: 0,
    dropped: 0,
    skipped: 0,
    active_edits: 0,
    drag_node: null,
  };
  assert.equal(dragStatus(solver, {}), null);
  assert.equal(dragStatus(null, {}), null);
  // Live: the count is the engine's own active-edit registry, and the label
  // comes from the scene (falling back to the short id for a layer it lacks).
  assert.equal(
    dragStatus({ ...solver, active_edits: 2, drag_node: A }, {}),
    '✋ dragging aaaaaaaa — 2 edit variable(s) live',
  );
  assert.equal(
    dragStatus({ ...solver, active_edits: 2, drag_node: A }, { [A]: { name: 'bound-rect' } }),
    '✋ dragging bound-rect — 2 edit variable(s) live',
  );
});

// ── Task 4.0: operations ────────────────────────────────────────────────
//
// The panel is a projection of the operation registry. These tests pin the two
// things that can rot silently: the operand captions (which come from the
// engine's own `input_names`/`description`, never from the UI's arithmetic) and
// the nesting rule (an operation row is indented under the source it reads
// first — RULE 1's "the sources stay visible" made concrete).

const C = 'cccccccc-3333-4333-8333-cccccccccccc';

function op(over: Partial<OperationWire>): OperationWire {
  return {
    id: C,
    name: 'subtract 1',
    kind: 'boolean',
    description: 'subtract',
    inputs: [A, B],
    input_names: ['disc', 'blade'],
    enabled: true,
    ...over,
  };
}

test('an operation row reads as its operands, in the engine’s order', () => {
  const [row] = operationRows({ [C]: op({}) });
  assert.equal(row.operands, 'disc − blade');
  assert.equal(row.glyph, '−');
  assert.equal(row.kind, 'boolean');
  assert.deepEqual(row.inputs, [A, B]);
  assert.equal(row.enabled, true);
});

test('every boolean gets its operator glyph, modifiers get their kind tag', () => {
  const rows = operationRows({
    a: op({ id: 'a', description: 'union' }),
    b: op({ id: 'b', description: 'intersect' }),
    c: op({ id: 'c', description: 'exclude' }),
    d: op({
      id: 'd',
      kind: 'fillet',
      description: 'fillet r=4',
      inputs: [A],
      input_names: ['disc'],
    }),
  });
  assert.deepEqual(
    rows.map((r) => r.glyph),
    ['∪', '∩', '⊕', 'fillet'],
  );
  assert.equal(rows[3].operands, 'disc');
});

test('a parked operation keeps its row and loses none of its caption', () => {
  const [row] = operationRows({ [C]: op({ enabled: false }) });
  assert.equal(row.enabled, false);
  assert.equal(row.operands, 'disc − blade');
});

test('an operation is nested under the source it reads first, at depth 1', () => {
  const rows = layerRows(
    [A, B, C], // engine draw order: sources first, then the virtual node
    {
      [A]: { name: 'disc', primitive: { type: 'circle' } },
      [B]: { name: 'blade', primitive: { type: 'rect' } },
    },
    { [C]: op({}) },
  );
  assert.deepEqual(
    rows.map((r) => [r.name, r.depth, r.isOperation]),
    [
      ['blade', 0, false], // panel order: top-most source first
      ['disc', 0, false],
      ['subtract 1', 1, true], // nested under disc, its `inputs[0]`
    ],
  );
  assert.equal(rows[2].parent, 'disc');
  assert.equal(rows[2].operands, 'disc − blade');
  assert.equal(rows[2].tag, 'boolean');
});

test('two operations on one source both sit under it, in draw order', () => {
  const rows = layerRows(
    [A, B, C, 'dddddddd-4444-4444-8444-dddddddddddd'],
    {
      [A]: { name: 'disc', primitive: { type: 'circle' } },
      [B]: { name: 'blade', primitive: { type: 'rect' } },
    },
    {
      [C]: op({ id: C, name: 'union 1', description: 'union' }),
      d: op({ id: 'dddddddd-4444-4444-8444-dddddddddddd', name: 'subtract 2' }),
    },
  );
  const nested = rows.filter((r) => r.isOperation);
  assert.deepEqual(
    nested.map((r) => r.name),
    ['union 1', 'subtract 2'],
  );
  assert.deepEqual(
    nested.map((r) => r.depth),
    [1, 1],
  );
});

test('an operation whose source is gone is still listed, never invented', () => {
  // The engine prunes the operation when its source is deleted, so this can
  // only happen if a snapshot disagrees. The panel then shows the orphan row
  // instead of quietly dropping a node the engine believes in — and it does
  // NOT guess a parent: `inputs[0]` is `A`, which the scene no longer holds.
  const rows = layerRows(
    [B],
    { [B]: { name: 'blade', primitive: { type: 'rect' } } },
    { [C]: op({}) },
  );
  const orphan = rows.find((r) => r.isOperation);
  assert.ok(orphan);
  assert.equal(orphan.parent, null);
  assert.equal(orphan.depth, 1);
  assert.deepEqual(
    rows.map((r) => r.name),
    ['blade', 'subtract 1'],
  );
});

test('an OperationsUpdated event logs the registry change', () => {
  const line = formatEvent({ type: 'OperationsUpdated', ids: [C] });
  assert.equal(line.kind, 'ok');
  assert.match(line.text, /operations/);
  assert.match(line.text, /cccccccc/);
});

// ── Task 6.0: motion ────────────────────────────────────────────────────

/** A motion report with one spring and one track, as the engine sends it. */
function motionWire(overrides: Partial<MotionWire> = {}): MotionWire {
  return {
    animating: true,
    horizon: 1.5,
    time: 0.5,
    epsilon: 0.001,
    states: [`hover:${A}`],
    tracks: ['intro'],
    bindings: [
      {
        node_id: A,
        property: 'width',
        kind: 'spring',
        value: 260.5,
        from: 200,
        at: 0,
        target: 320,
        stiffness: 170,
        damping: 26,
        state: `hover:${A}`,
        track: null,
        channel: null,
      },
      {
        node_id: B,
        property: 'x',
        kind: 'track',
        value: 40,
        from: null,
        at: null,
        target: null,
        stiffness: null,
        damping: null,
        state: null,
        track: 'intro',
        channel: 'x',
      },
    ],
    ...overrides,
  };
}

test('motion rows describe a spring and a track in one vocabulary', () => {
  const rows = motionRows(motionWire());
  assert.equal(rows.length, 2);
  const spring = rows[0];
  assert.equal(spring.kind, 'spring');
  assert.equal(spring.property, 'width');
  assert.equal(spring.value, '260.5');
  // The flag is shortened to something a panel can show, not a uuid.
  assert.match(spring.detail, /spring width → 320 \(170 \/ 26\) on hover • aaaaaaaa/);
  // 260.5 is 59.5 away from 320 — further than the engine's epsilon, so the row
  // says the spring is still driving the scene.
  assert.equal(spring.active, true);
  const track = rows[1];
  assert.equal(track.kind, 'track');
  assert.match(track.detail, /track x ← intro:x/);
});

test('a settled spring reports itself at rest, not "animating forever"', () => {
  const wire = motionWire({ animating: false, horizon: null });
  wire.bindings = [{ ...wire.bindings[0], value: 320 }];
  const rows = motionRows(wire);
  assert.equal(rows[0].value, '320');
  assert.equal(rows[0].active, false, 'within epsilon of the target ⇒ at rest');
  assert.equal(motionSummary(wire), 'idle · 1 binding at rest');
});

test('a missing value is an em dash, never NaN', () => {
  const wire = motionWire();
  wire.bindings = [{ ...wire.bindings[0], value: null, target: null }];
  const rows = motionRows(wire);
  assert.equal(rows[0].value, '—');
  assert.match(rows[0].detail, /→ —/);
});

test('the motion summary reports the time the loop can stop at', () => {
  assert.equal(motionSummary(motionWire()), 'animating · stops in 1.00s');
  assert.equal(
    motionSummary(motionWire({ horizon: null })),
    'animating · stops in —',
    'a horizon the engine cannot compute yet is not a number the UI invents',
  );
  assert.equal(motionSummary(null), 'motion —');
  assert.equal(
    motionSummary(motionWire({ bindings: [], tracks: [], states: [] })),
    'no bindings — nothing is animated',
  );
});

test('state flags read as chips, and only the ones that are on', () => {
  assert.deepEqual(stateChips(motionWire()), ['hover • aaaaaaaa']);
  assert.deepEqual(stateChips(motionWire({ states: [] })), []);
  assert.deepEqual(stateChips(null), []);
  // A flag with no node id (a hand-set state) is shown whole.
  assert.equal(shortFlag('active'), 'active');
});

test('motion rows survive an empty report', () => {
  assert.deepEqual(motionRows(null), []);
  assert.deepEqual(motionRows(motionWire({ bindings: [] })), []);
});

// ── Task 7.0: the procedural panel's projections ───────────────────────

/** A two-node chain, exactly as `procedural_json()` renders it. */
function proceduralReport(): ProceduralReportWire {
  return {
    count: 2,
    nodes: [
      {
        id: A,
        name: 'grid n=3 m=2 s=10 @(0, 0)',
        kind: 'grid',
        enabled: true,
        description: 'grid n=3 m=2 s=10 @(0, 0)',
        operands: {
          columns: { ty: 'scalar', text: '3' },
          origin: { ty: 'point', text: '0, 0' },
          rows: { ty: 'scalar', text: '2' },
          spacing: { ty: 'scalar', text: '(variable)' },
        },
        wires: {},
        inputs: [],
        outputs: [
          { port: 'region', ty: 'region', required: false, wired: false, value: 'region 1 rings, 4 pts' },
          { port: 'points', ty: 'points', required: false, wired: false, value: 'points ×12' },
          { port: 'center', ty: 'point', required: false, wired: false, value: 'point (15, 10)' },
          { port: 'span', ty: 'scalar', required: false, wired: false, value: 'scalar 30' },
        ],
        upstream: [],
      },
      {
        id: B,
        name: 'smooth n=2 s=0.5',
        kind: 'smooth',
        enabled: false,
        description: 'smooth n=2 s=0.5 · parked',
        operands: {
          iterations: { ty: 'scalar', text: '2' },
          strength: { ty: 'scalar', text: '0.5' },
        },
        wires: { region: `${A}:region` },
        inputs: [{ port: 'region', ty: 'region', required: true, wired: true, value: null }],
        outputs: [{ port: 'region', ty: 'region', required: false, wired: false, value: null }],
        upstream: [A],
      },
    ],
    diagnostics: [],
  };
}

test('procedural rows project the engine report verbatim', () => {
  const rows = proceduralRows(proceduralReport());
  assert.equal(rows.length, 2);
  assert.equal(rows[0].kind, 'grid');
  assert.equal(rows[0].short, 'aaaaaaaa', 'the row shows a short id, not a uuid');
  assert.equal(rows[0].operands[3].port, 'spacing');
  assert.equal(rows[0].operands[3].text, '(variable)', 'the engine renders it, the UI does not');
  assert.equal(rows[0].operands[3].ty, 'scalar', 'the type decides which control the panel offers');
  assert.equal(rows[0].outputs[3].value, 'scalar 30');
  assert.equal(rows[1].wires['region'], `${A}:region`);
  assert.equal(rows[1].ready, true, 'its one required input is fed');
  assert.equal(rows[1].enabled, false);
  // A parked node publishes nothing — the port line says so with a dash.
  assert.equal(rows[1].outputs[0].value, null);
});

test('procedural rows survive an empty or absent report', () => {
  assert.deepEqual(proceduralRows(null), []);
  assert.deepEqual(proceduralRows({ count: 0, nodes: [], diagnostics: [] }), []);
});

test('the port list is what the connect control offers', () => {
  const options = proceduralPortOptions(proceduralReport());
  assert.equal(options.length, 5, 'four grid ports plus the smoother');
  assert.equal(options[0].value, `${A}:region`);
  assert.equal(options[0].label, 'grid n=3 m=2 s=10 @(0, 0) • region');
  assert.equal(options[0].ty, 'region');
  assert.deepEqual(proceduralPortOptions(null), []);
});

test('the procedural summary counts what is drawing, parked and failing', () => {
  assert.equal(proceduralSummary(proceduralReport()), '2 nodes · 1 drawing · 1 parked');
  const failing = proceduralReport();
  failing.diagnostics = [
    { severity: 'warning', code: 'procedural-missing-input', node_id: B, property: null, message: 'x' },
  ];
  assert.equal(proceduralSummary(failing), '2 nodes · 1 drawing · 1 parked · 1 failing');
  assert.equal(proceduralSummary(null), 'procedural —');
  assert.equal(
    proceduralSummary({ count: 0, nodes: [], diagnostics: [] }),
    'no nodes — the graph is empty',
  );
});

test('the export summary says the format, the size and the warning count', () => {
  assert.equal(exportSummary(null), 'export —');
  assert.equal(
    exportSummary({ status: 'ok', format: 'svg', code: '<svg/>', warnings: [] }),
    'svg · 6 B',
  );
  assert.equal(
    exportSummary({
      status: 'ok',
      format: 'react',
      code: 'x'.repeat(2048),
      warnings: ['one'],
    }),
    'react · 2.0 kB · 1 warning',
  );
  assert.equal(
    exportSummary({ status: 'error', format: 'svg', code: '', warnings: [], message: 'boom' }),
    'export failed — boom',
  );
});

// ── The AI panel (Task 9.0) ─────────────────────────────────────────────
//
// The panel is the one place a reader sees the AI's *translation* — the JSON it
// produced and the engine's verdict on it. These tests pin that projection: the
// plan rows come straight off the wire, a failed call is labelled by its typed
// code (never by parsing a message), and a run reports its attempts, its dirty
// set and its corrections.

const PLAN = [
  {
    type: 'DefineExpression',
    id: '11111111-1111-4111-8111-111111111111',
    source: '$base * 2',
  },
  {
    type: 'SetParameter',
    node_id: A,
    property: 'width',
    value: { Float: { Expression: '11111111-1111-4111-8111-111111111111' } },
  },
  { type: 'ApplyOperation', id: 'op-1', kind: { type: 'offset', distance: { Literal: 4 } }, inputs: [A, B] },
];

test('a plan row is the command, its tag and the id it addresses', () => {
  const rows = aiPlanRows(PLAN);
  assert.equal(rows.length, 3);
  assert.deepEqual(
    rows.map((row) => row.tag),
    ['DefineExpression', 'SetParameter', 'ApplyOperation'],
  );
  // Ids are short in the row (the rest of the UI does the same) and exact in
  // the JSON beside it — the row is never the thing that gets copied.
  assert.equal(rows[0].target, '11111111…');
  assert.equal(rows[1].target, 'aaaaaaaa…');
  assert.equal(rows[2].target, 'op-1');
  // The last resort is the input count — a row still says *something* about a
  // command that names no id of its own.
  assert.equal(aiPlanRows([{ type: 'ApplyOperation', inputs: [A, B] }])[0].target, '2 input(s)');
  assert.equal(JSON.parse(rows[1].json).node_id, A);
  // The JSON is verbatim — the row is a view, never a re-encoding.
  assert.equal(JSON.parse(rows[1].json).property, 'width');
  assert.deepEqual(aiPlanRows([]), []);
  assert.equal(aiPlanRows([null])[0].tag, 'unknown');
});

test('an idle panel says so, and a preview never claims to have applied', () => {
  const idle = aiPanelState(null, null);
  assert.match(idle.summary, /idle/);
  assert.deepEqual(idle.plan, []);

  const preview: AiCallWire = {
    status: 'ok',
    prompt: 'make the width of card twice $base',
    plan: PLAN,
    notes: ['defined an expression for the width'],
    attempt: 1,
    applies: false,
  };
  const state = aiPanelState(preview, null);
  assert.match(state.summary, /3 command\(s\) ready — nothing applied/);
  assert.equal(state.plan.length, 3);
  assert.equal(state.dirty.length, 0, 'a preview evaluates nothing');
  assert.equal(state.attempts, 1);
  assert.deepEqual(state.notes, ['defined an expression for the width']);
});

test('an unrecognized prompt is labelled by its typed code', () => {
  const failed: AiCallWire = {
    status: 'error',
    code: 'unrecognized-prompt',
    message: 'I cannot turn that into commands yet …',
    corrections: [],
    plan: [],
  };
  const state = aiPanelState(failed, null);
  assert.equal(state.code, 'unrecognized-prompt');
  assert.match(state.summary, /cannot plan — unrecognized-prompt/);
});

test('a run reports its headline, attempts, dirty ids and corrections', () => {
  const receipt: AiExecuteWire = {
    status: 'ok',
    headline: '1 command(s) in 2 attempts, 1 node(s) re-evaluated',
    // RULE 4: what the panel shows is the sentence, so the wire carries it.
    prose: '✨ Adjusted 1 value.',
    corrections: 1,
    report: {
      prompt: 'round the corners of dot by 8',
      attempts: 2,
      plan: [PLAN[1]],
      events: [{ type: 'Dirty', ids: [A], mode: 'incremental' }],
      dirty: [A],
      corrections: [
        {
          attempt: 1,
          code: 'engine-rejected',
          error: "unknown property 'corner_radius' for node …",
          command: PLAN[1],
          note: 'the engine wanted radius; retrying with radius',
        },
      ],
      notes: ['set radius'],
      summary: {
        version: 1,
        nodes: [],
        variables: [],
        expressions: [],
        operations: [],
        constraints: [],
        procedural: [],
        tracks: [],
      },
    },
  };
  const state = aiPanelState(null, receipt);
  assert.equal(state.headline, receipt.headline);
  assert.equal(state.prose, receipt.prose, 'the sentence survives the panel');
  assert.equal(state.attempts, 2);
  assert.deepEqual(state.dirty, [A]);
  assert.equal(state.corrections.length, 1);
  assert.equal(state.corrections[0].code, 'engine-rejected');
  assert.match(state.corrections[0].error, /corner_radius/);
  assert.match(state.corrections[0].note, /retrying with radius/);
  assert.match(state.corrections[0].command, /SetParameter/);
});

test('a refusal after every attempt carries the history, not just the verdict', () => {
  const capped: AiExecuteWire = {
    status: 'error',
    code: 'max-retries-exceeded',
    message: 'the AI could not produce a valid plan in 3 attempt(s): …',
    plan: PLAN,
    corrections: [
      { attempt: 1, code: 'engine-rejected', error: 'first', note: '' },
      { attempt: 2, code: 'engine-rejected', error: 'second', note: '' },
      { attempt: 3, code: 'engine-rejected', error: 'third', note: '' },
    ],
  };
  const state = aiPanelState(null, capped);
  assert.equal(state.code, 'max-retries-exceeded');
  assert.equal(state.corrections.length, 3, 'every attempt is on show');
  assert.deepEqual(
    aiCorrectionRows(capped.corrections).map((row) => row.attempt),
    [1, 2, 3],
  );
  assert.equal(state.plan.length, 3, 'the plan that kept failing is shown too');
});

test('the grounding line counts the engine summary it was built from', () => {
  const summary: DocumentSummaryWire = {
    version: 1,
    nodes: [
      {
        id: A,
        name: 'card',
        kind: 'Rectangle',
        label: "Rectangle 'card'",
        slots: [{ property: 'width', source: '$base * 2', value: 80 }],
      },
    ],
    variables: [{ name: 'base', value: 40 }],
    expressions: [],
    operations: [],
    constraints: [],
    procedural: [],
    tracks: [],
  };
  assert.deepEqual(summaryLabels(summary), ["Rectangle 'card'"]);
  assert.equal(summaryCounts(summary), '1 node · 1 variable · 0 expressions · 0 constraints');
  assert.equal(summaryCounts(null), 'no summary');
  assert.equal(summaryCounts({ ...summary, nodes: [], variables: [] }).startsWith('0 nodes · 0 variables'), true);
});

// ── Task 10.6 RULE 4: the plan is described, never dumped ────────────────────

test('every plan row reads as a phrase a designer would say, never as JSON', () => {
  const rows = aiPlanRows([
    { type: 'AddConstraint', constraint: { id: 'c1', kind: 'vertical' } },
    { type: 'ApplyOperation', id: 'op1', kind: 'union', inputs: ['a', 'b'] },
    { type: 'SetParameter', node_id: 'a', property: 'corner_radius', value: 0 },
    { type: 'CreateComponent', id: 'm1', name: 'Badge' },
    { type: 'SomethingBrandNew', node_id: 'a' },
  ]);
  assert.deepEqual(
    rows.map((row) => row.label),
    [
      'Add a constraint',
      'Unify the shapes',
      'Adjust',
      'Create a component',
      'something brand new',
    ],
  );
  // The tag is still on the row for anyone who wants it; the *reading* is not.
  assert.equal(rows[0].tag, 'AddConstraint');
  for (const row of rows) {
    assert.ok(!row.label.includes('{'), row.label);
    assert.ok(!row.label.includes('_'), row.label);
  }
});

test('no panel text ever contains a brace — RULE 4 is checkable', () => {
  const state = aiPanelState(
    {
      status: 'ok',
      applies: false,
      prompt: 'align perfectly',
      plan: [
        { type: 'AddConstraint', constraint: { id: 'c1' } },
        { type: 'SetParameter', node_id: 'a', property: 'x' },
      ],
      notes: [],
      attempt: 1,
    },
    null,
  );
  const shown = [state.summary, state.headline ?? '', ...state.plan.map((row) => row.label)];
  for (const line of shown) {
    assert.ok(!/[{}\[\]"]/.test(line), `${line} looks like JSON`);
  }
});

// ── Task 10.6: an engine older than the UI degrades in one sentence ──────────

test('the Task 10.6 surface is probed, not assumed', () => {
  assert.equal(engineIsOlderThanUi({}), true, 'no methods at all');
  const half = {
    set_selection: () => '{}',
    component_view: () => '{}',
    // structural_macros missing: the chips would vanish silently otherwise.
  };
  assert.equal(engineIsOlderThanUi(half), true, 'a partial surface is an old surface');
  const full = {
    set_selection: () => '{}',
    component_view: () => '{}',
    structural_macros: () => '[]',
  };
  assert.equal(engineIsOlderThanUi(full), false);
  assert.match(VectraClient.STALE_ENGINE, /build-wasm\.sh/);
  assert.ok(!VectraClient.STALE_ENGINE.includes('{'));
});
