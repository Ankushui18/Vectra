#!/usr/bin/env node
/**
 * E2E smoke test (Task 1.4 §3, extended Task 2.1 §7).
 *
 * Loads the REAL compiled wasm-bindgen module in Node (no browser, no mocks)
 * and proves the MES §16 loop through the string boundary:
 *   1. WASM loads + instantiates.
 *   2. CreateNode succeeds with `status: ok` + expected events.
 *   3. get_snapshot reflects the new node.
 *   4. Variable bind + resolve flows through the snapshot.
 *   5. Invalid DefineExpression fails typed WITHOUT mutating.
 *   6. Valid define compiles; expression-bound radius evaluates via bytecode.
 *   7. undo unwinds SetParameter → Define → SetParameter → SetVariable →
 *      CreateNode (sync protocol keeps the compiled registry == document).
 *   8. Malformed/unknown commands fail typed (`status: error`), never throw.
 *
 * Task 2.2 additions:
 *   9. The dependency graph is exported: var/expression/property vertices and
 *      `depends_on` edges, acyclic.
 *  10. A variable edit emits `Dirty` with EXACTLY its dependent nodes and
 *      re-evaluates them incrementally (never a full rebuild) — while an
 *      unrelated layer keeps its value.
 *  11. An edit nothing depends on emits an EMPTY dirty set (no evaluation).
 *  12. Undo removes graph vertices/edges exactly; redo restores them.
 *  13. `force_full_evaluation` re-evaluates everything and lands on a scene
 *      byte-identical to the incrementally patched one (`patch ≡ rebuild`).
 *
 * Task 3.1 additions (constraints are hard rules, not guidelines):
 *  14. A `Vertical` constraint solves immediately: B.x snaps to A.x, the
 *      solver's variable count is visible, and the slot's parametric link is
 *      broken loudly (`parametric-link-broken`) rather than writing `$base`.
 *  15. Undo reverts rule + geometry + link in ONE step and releases the
 *      Cassowary variables; redo restores all three.
 *  16. Over-constrained: the weaker rule is dropped, visibly disabled and
 *      reported as `constraint-dropped`; the survivor still holds exactly.
 *  17. Two contradicting `Required` rules are a typed error — nothing applied.
 *
 * Task 3.2 additions (direct manipulation through the drag triad):
 *  19. `BeginDrag` registers the pointer's slots as Cassowary EDIT variables
 *      (RULE 1) and moves nothing; the snapshot names the dragged node.
 *  20. `UpdateDrag` samples are absolute pointer coordinates: the anchor and
 *      its constrained partner both move, incrementally, and every other
 *      command (undo included) is refused — typed — mid-gesture.
 *  21. `EndDrag` finalizes the gesture as one undoable step of literals and
 *      empties the active-edits registry; one undo reverts the whole gesture.
 *
 * Task 4.0 additions (non-destructive operations):
 *  22. `ApplyOperation` creates a VIRTUAL node: the sources stay in the scene
 *      and in `z_order`, the operation's own id joins them with a `path`
 *      primitive, and the registry is visible in the snapshot.
 *  23. Moving a source re-runs only the dependent operation (`Dirty` names it,
 *      the source's path data is byte-identical, the result changed); one undo
 *      removes the operation and leaves both sources exactly as authored.
 *  24. Parking an operation withdraws its geometry but keeps its row; removing
 *      it is ordinary undoable bookkeeping; a stray operand is refused typed.
 *
 * Task 6.0 additions (motion is a source, not a system):
 *  27. `bind_hover_spring` anchors a spring at the slot's *current* value and
 *      the engine clock; `pointer_move` hit-tests, flips the node's `hover:`
 *      flag, and the spring eases from where it was (`from` is the rest value,
 *      not the hover value).
 *  28. Scrubbing the clock is a pure function of time: the same `t` twice gives
 *      identical geometry, a backwards scrub matches the forward sweeps, and
 *      `is_animating` is false at the settle horizon — so the frame loop can
 *      stop instead of spinning (Task 5.0's open item #4).
 *  29. Motion samples are NOT history: hundreds of frames of scrubbing, hovering
 *      and pointer moves leave `undo_depth` untouched, while the binding itself
 *      is exactly one undoable entry.
 *  30. A drag outranks a spring and hands the slot back on release: the spring
 *      survives, re-anchored at the committed value, and redo replays it as a
 *      binding rather than a literal (Interaction Precedence Law).
 *  31. Keyframe tracks are document state: register → sample → move a key →
 *      the sampled value follows → remove → the binding fails loudly. Track CRUD
 *      is undoable; the sampled values never are.
 *
 * Task 9.0 additions (the AI layer emits commands, never geometry):
 *  40. The engine publishes `document_summary` / `ai_prompt` / `ai_phrasings`:
 *      exact ids, the parametric SOURCE beside the current value, the variables
 *      and the full command contract — the grounding, in the engine's own words.
 *  41. `ai_generate_commands` previews a plan that binds `$base * 2` to a slot:
 *      the text becomes COMMANDS (the expression stays an expression, the
 *      `$new:` placeholder becomes a real uuid, the id is the summary's), and
 *      nothing is applied — same document, same history depth.
 *  42. `ai_execute_commands` runs that exact plan through `dispatch_command`:
 *      the engine's events, the dirty set, a live parametric dependency, and one
 *      undo that steps over the expression *and* the rebind at once.
 *  43. `ai_execute_with_retry` self-corrects: the engine refuses
 *      `corner_radius` on a Circle, the refusal goes back as a correction, and
 *      attempt 2 lands `radius = 8` — no API key, no network.
 *  44. Failure is typed and harmless: an incomprehensible prompt is
 *      `unrecognized-prompt`, a hallucinated id is refused by name, a malformed
 *      plan is `invalid-json`, and the document is byte-identical afterwards.
 *
 * Task 5.0 additions (the WebGPU canvas is a consumer, and nothing else):
 *  25. `render_frame` builds the GPU plan from the evaluated scene: the first
 *      frame is a full sync (every node created), a one-node move costs exactly
 *      one 80-byte instance write, and an idle frame costs nothing at all. Node
 *      has no WebGPU, so the frame report says `ready: false` — the *decision*
 *      path is identical to the browser's, only the upload is absent.
 *  26. The pointer path: a client coordinate resolves to a `NodeId` through the
 *      renderer's spatial index (and to nothing over empty space), and that id
 *      drives the BeginDrag/UpdateDrag/EndDrag triad — RULE 3 end to end.
 *
 * Task 7.0 additions (the procedural graph is geometry, not a preview):
 *  32. `procedural_kinds` is the engine's own palette: the panel adds a node by
 *      embedding the operand payload it returns, and the node composes into the
 *      scene as a standard, *nameable* node (RULE 4's other half).
 *  33. Every gate is pre-application: a scalar into a region port, a wire that
 *      would close a chain cycle, and an operand that reads a procedural output
 *      (RULE 3) are all refused typed, with the document unchanged.
 *  34. A slot that reads a value port follows it in the SAME dispatch — the
 *      engine re-reads the readers of a republished port inside one `settle`.
 *  35. A procedural result survives an unrelated operation pass (the silent
 *      eviction trap), parks and re-arms, and a removal is undoable down to the
 *      wire that fed it.
 *  36. Determinism: two engines running the same script produce byte-identical
 *      pictures, a full rebuild equals the incremental scene, and a mutation
 *      nothing depends on re-runs nothing.
 *
 * Task 10.1 additions (the drawing suite:
 * `crates/vectra-draw` + `crates/vectra-wasm/src/draw.rs`):
 *  46. The pen's gestures, as the document sees them afterwards. A click leaves
 *      a corner (`Line`), a drag pulls *mirrored* handles out of the anchor and
 *      turns the segment into a `Cubic`, and an Alt-drag breaks the mirror —
 *      the whole of RULE 2, in the committed path's own control points.
 *  47. A click on the first anchor closes the path with a `Close` segment;
 *      committing without closing does not.
 *  48. The brush: pressure-carrying samples are captured, fitted (21 samples of
 *      a line become a handful of curves, never 21 segments) and expanded to a
 *      closed, fillable ring of Bézier sides (RULE 3).
 *  49. **Quick Shape**: a jagged, ±18 %-wobbly hand-drawn circle becomes four
 *      kappa arcs whose anchors are the *cardinals of the fitted circle to
 *      1e-9* — because the Task 3.1 solver moved them, through eleven rows that
 *      name the primitive first, plus two that hold the seam shut (RULE 3).
 *  50. Direct selection: the spatial hit test (client pixels in, slot out), an
 *      anchor drag that is ONE history entry for twelve samples, a handle drag
 *      that mirrors its partner, and an Alt-drag that writes only the dragged
 *      handle — the boundary-level form of the Handle Independence Law (RULE 4).
 *  51. The overlay's camera: `document_to_client` is the inverse of the pointer
 *      mapping, so handles are drawn where they are — including the y flip that
 *      a DOM overlay would get wrong on its own.
 *
 * Task 10.6 additions (Smart Components, Make Magic, Icon Studio):
 *  65. A selection becomes a component master whose props are variables. The
 *      panel's own view names them (`size`, `stroke_width`, `corner_radius`,
 *      `color`); a placed instance arrives as a *copy* at the design size; and
 *      one write of `size` scales exactly that instance through the dependency
 *      graph — master and siblings untouched, undo restores it in one step
 *      (RULE 1, the Component Prop Law).
 *  66. Make Magic is structural: "make this geometric" and "align perfectly"
 *      become `Vec<Command>` that the engine validates, applies and answers in
 *      a sentence — and a prompt the selection cannot support is refused in a
 *      sentence naming what is missing. No JSON, in or out (RULE 2, RULE 4).
 *  67. The icon macro: one artboard per size on the ladder, and `stroke ÷ size`
 *      equal to the master's ratio at every rung, so a 24px master's 2px stroke
 *      is 1.33px at 16 — optically correct, not a hairline (RULE 3).
 *  68. The five macros the ⌘K bar offers, the selection's prose and the prompt
 *      the model is grounded on all come from the engine, so the bar and the
 *      panel can never drift from it (RULE 4).
 *
 * Steps 65-68 drive the **built** binary in `src/wasm/`, which is what makes
 * this file the end-to-end gate for the four Task 10.6 rules.
 *
 * Task 11.0 additions (advanced parametric typography):
 *  72. A text node creates, shapes and snapshots: the run's metrics are in the
 *      snapshot, and its letterforms are real path geometry (`d`) — the renderer
 *      tessellates text by tessellating a path (RULE 4).
 *  73. **The Parametric Text Law** at the boundary: a `font_size` driven by a
 *      `$variable` re-lays the run out on the next read — the box scales with
 *      the size and the drawn geometry changes with it.
 *  74. **Text on a path** (RULE 2): binding a run to a circle takes it off its
 *      baseline and onto the curve, a non-path source is refused, and an
 *      `offset` slides the run along the arc.
 *  75. **Outline** (RULE 3): the engine mints the letterform nodes, groups them,
 *      hides the type it replaced, draws the same picture — and one undo puts
 *      the document back byte for byte.
 *
 * Task 10.7 additions (the "Procreate" layer — RULE 1 and RULE 2 are the UI's,
 * so the engine's gate is RULE 3 and RULE 4's command half):
 *  69. **Alpha Lock** is a boundary, not a rewrite: the flag costs zero
 *      evaluation, a stroke with nowhere to land is refused in a sentence
 *      naming the layer, and a stroke that crosses the artwork's edge commits
 *      *inside the lines* — every point of the stored geometry is inside, which
 *      is the engine half of the Alpha Lock Law (RULE 3a).
 *  70. **A clipping mask is live geometry**: the upper layer's rect becomes the
 *      shared corner of the two squares, the layer below is untouched, the row
 *      names what it clips to — and clearing the flag gives the *document's*
 *      geometry back through the evaluator, byte for byte (RULE 3b).
 *  71. **ColorDrop's engine half**: the drop is the renderer's own hit test
 *      (empty space answers nothing, so a drop there does nothing) plus one
 *      `SetAppearances` fill — the UI contributes no geometry and no colour
 *      maths (RULE 4).
 *
 * Run: `npm run smoke` (after `bash scripts/build-wasm.sh`).
 */
import { strict as assert } from 'node:assert';
import { existsSync, readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const gluePath = join(root, 'src/wasm/vectra_wasm.js');
const wasmPath = join(root, 'src/wasm/vectra_wasm_bg.wasm');

if (!existsSync(gluePath) || !existsSync(wasmPath)) {
  console.error('smoke: missing generated bindings. Run first:');
  console.error('  bash scripts/build-wasm.sh');
  process.exit(2);
}

// 1. WASM loads + instantiates.
const glue = await import(pathToFileURL(gluePath).href);
await glue.default({ module_or_path: readFileSync(wasmPath) });
const engine = new glue.VectraEngine();
console.log('smoke[1/75]: WASM instantiated');

const dispatch = (cmd) => JSON.parse(engine.dispatch_command(JSON.stringify(cmd)));
const snapshot = () => JSON.parse(engine.get_snapshot());
const eventTypes = (res) => res.events.map((e) => e.type);

// 2. CreateNode succeeds with ok + expected events.
const id = crypto.randomUUID();
const created = dispatch({
  type: 'CreateNode',
  id,
  kind: {
    Circle: {
      cx: { Literal: 100.0 },
      cy: { Literal: 100.0 },
      radius: { Literal: 50.0 },
    },
  },
  name: 'smoke-circle',
});
assert.equal(created.status, 'ok', `CreateNode failed: ${JSON.stringify(created)}`);
assert.ok(eventTypes(created).includes('NodesUpdated'), 'missing NodesUpdated');
assert.ok(eventTypes(created).includes('OrderChanged'), 'missing OrderChanged');
console.log('smoke[2/75]: CreateNode ok + NodesUpdated/OrderChanged');

// 3. get_snapshot reflects the new node.
let snap = snapshot();
assert.equal(snap.status, 'ok');
assert.deepEqual(snap.scene.z_order, [id]);
assert.equal(snap.scene.nodes[id].name, 'smoke-circle');
assert.equal(snap.scene.nodes[id].primitive.type, 'circle');
assert.equal(snap.scene.nodes[id].primitive.r, 50.0);
assert.equal(snap.can_undo, true);
assert.equal(snap.can_redo, false);
console.log('smoke[3/75]: snapshot reflects node');

// 4. Variable bind + resolve flows through the snapshot.
let res = dispatch({ type: 'SetVariable', name: 'base', value: 200.0 });
assert.equal(res.status, 'ok');
res = dispatch({
  type: 'SetParameter',
  node_id: id,
  property: 'radius',
  value: { Float: { Variable: 'base' } },
});
assert.equal(res.status, 'ok');
snap = snapshot();
assert.equal(snap.variables.base, 200.0);
assert.equal(snap.scene.nodes[id].primitive.r, 200.0, 'bound radius must resolve');
console.log('smoke[4/75]: $base=200 drives radius → 200');

// 5. Invalid expressions fail typed WITHOUT mutating.
const invalid = dispatch({
  type: 'DefineExpression',
  id: crypto.randomUUID(),
  source: '$base * ',
});
assert.equal(invalid.status, 'error');
assert.match(invalid.message, /invalid expression/);
snap = snapshot();
assert.deepEqual(snap.expressions, {}, 'no record pushed for invalid source');
console.log('smoke[5/75]: invalid define fails typed, no record pushed');

// 6. Valid define compiles; expression-bound radius evaluates via bytecode.
const exprId = crypto.randomUUID();
const defined = dispatch({
  type: 'DefineExpression',
  id: exprId,
  source: '$base * 2 + 10',
});
assert.equal(defined.status, 'ok', `DefineExpression failed: ${JSON.stringify(defined)}`);
assert.ok(eventTypes(defined).includes('ExpressionsUpdated'), 'missing ExpressionsUpdated');
res = dispatch({
  type: 'SetParameter',
  node_id: id,
  property: 'radius',
  value: { Float: { Expression: exprId } },
});
assert.equal(res.status, 'ok');
snap = snapshot();
assert.equal(snap.expressions[exprId], '$base * 2 + 10');
assert.equal(snap.scene.nodes[id].primitive.r, 410.0, 'bytecode eval: 200*2+10');
console.log('smoke[6/75]: define → bind → radius 410 via bytecode');

// 7. undo unwinds the stack with inverse events.
let undone = JSON.parse(engine.undo()); // drop BindExpr → $base bind (200)
assert.equal(undone.status, 'ok');
assert.equal(snapshot().scene.nodes[id].primitive.r, 200.0);
undone = JSON.parse(engine.undo()); // drop Define → no expressions
assert.equal(undone.status, 'ok');
assert.deepEqual(snapshot().expressions, {});
undone = JSON.parse(engine.undo()); // drop BindVar → literal 50
assert.equal(undone.status, 'ok');
assert.equal(snapshot().scene.nodes[id].primitive.r, 50.0);
undone = JSON.parse(engine.undo()); // drop SetVariable → no variables
assert.equal(undone.status, 'ok');
assert.deepEqual(snapshot().variables, {});
undone = JSON.parse(engine.undo()); // drop CreateNode → inverse event
assert.equal(undone.status, 'ok');
assert.ok(eventTypes(undone).includes('NodesRemoved'), 'missing inverse NodesRemoved');
snap = snapshot();
assert.deepEqual(snap.scene.z_order, []);
assert.equal(snap.can_undo, false);
assert.equal(snap.can_redo, true);
console.log('smoke[7/75]: undo ×5 unwinds stack, registry stays in sync');

// 8. Failures are typed envelopes, never throws.
const malformed = dispatch({ type: 'Nope' });
assert.equal(malformed.status, 'error');
assert.match(malformed.message, /invalid command/);
const unknown = dispatch({
  type: 'SetParameter',
  node_id: crypto.randomUUID(),
  property: 'width',
  value: { Float: { Literal: 1.0 } },
});
assert.equal(unknown.status, 'error');
assert.match(unknown.message, /node not found/);
const emptyUndo = JSON.parse(engine.undo());
assert.equal(emptyUndo.status, 'error', 'undo on empty stack must error typed');
assert.match(emptyUndo.message, /nothing to undo/);
console.log('smoke[8/75]: malformed/unknown/empty-undo all fail typed');

// ── Task 10.1 helpers ──────────────────────────────────────────────────────

/**
 * The engine's segment tag as a string.
 *
 * `PathSegment` is *externally* tagged, so a payload-carrying variant is
 * `{"Cubic": {…}}` while a unit variant is the bare string `"Close"` — and
 * `Object.keys("Close")[0]` is `"0"`, which is exactly the trap this helper
 * exists to avoid.
 */
function segmentKind(segment) {
  return typeof segment === 'string' ? segment : Object.keys(segment)[0];
}

/** How many undo steps an arbitrary engine holds (its snapshot's own count). */
function undoDepthOf(engine) {
  // The snapshot's own field name: the engine reports the undo stack depth as
  // `undo_depth` (see `SnapshotResponse`), whatever the Rust accessor is called.
  return JSON.parse(engine.get_snapshot()).undo_depth;
}

/**
 * Does `after` look like `before`'s partner handle mirrored through the anchor?
 *
 * A handle drag mirrors the opposite control point through the anchor, so the
 * *sum* of the two vectors is unchanged: `(h_in − a) + (h_out − a)` is the same
 * before and after. That is the Handle Symmetry Law's identity, checked here at
 * the JSON boundary instead of in Rust.
 */
function mirroredPoint(before, after) {
  const sum = (anchor) => [
    anchor.handle_in[0] - anchor.x + (anchor.handle_out[0] - anchor.x),
    anchor.handle_in[1] - anchor.y + (anchor.handle_out[1] - anchor.y),
  ];
  const [bx, by] = sum(before);
  const [ax, ay] = sum(after);
  return Math.abs(bx - ax) < 1e-6 && Math.abs(by - ay) < 1e-6;
}

// ── Task 2.2: dependency graph + incremental evaluation ─────────────────

const graph = new glue.VectraEngine();
const gsend = (cmd) => JSON.parse(graph.dispatch_command(JSON.stringify(cmd)));
const gsnap = () => JSON.parse(graph.get_snapshot());
const gdeps = () => JSON.parse(graph.dependencies());

/** The `Dirty` event of a response: `{ ids, mode }`. */
/** The dependency graph of an arbitrary engine instance. */
const gdepsOf = (engine) => JSON.parse(engine.dependencies());

const dirtyOf = (res) => {
  assert.equal(res.status, 'ok', `expected ok: ${JSON.stringify(res)}`);
  const ev = res.events.find((e) => e.type === 'Dirty');
  assert.ok(ev, 'every mutation must report a Dirty event');
  return ev;
};
const quoted = (source) => JSON.stringify({
  type: 'DefineExpression', id: source.id, source: source.text,
});

// 9. Build the brief's chain: $base → ƒx → property, plus a direct
//    variable-bound property and one unattached layer.
const rect = crypto.randomUUID();
const idle = crypto.randomUUID();
const fx = crypto.randomUUID();
const steps = [
  { type: 'SetVariable', name: 'base', value: 200.0 },
  {
    type: 'CreateNode',
    id: rect,
    kind: { Rectangle: { x: { Literal: 10.0 }, y: { Literal: 10.0 },
      width: { Literal: 10.0 }, height: { Literal: 40.0 }, corner_radius: { Literal: 0.0 } } },
    name: 'driven-rect',
  },
  { type: 'SetParameter', node_id: rect, property: 'width', value: { Float: { Variable: 'base' } } },
  {
    type: 'CreateNode',
    id: idle,
    kind: { Circle: { cx: { Literal: 0.0 }, cy: { Literal: 0.0 }, radius: { Literal: 5.0 } } },
    name: 'idle-circle',
  },
];
for (const cmd of steps) assert.equal(gsend(cmd).status, 'ok', `setup: ${JSON.stringify(cmd)}`);
assert.equal(gsend(JSON.parse(quoted({ id: fx, text: '$base * 2 + 10' }))).status, 'ok');
assert.equal(
  gsend({ type: 'SetParameter', node_id: idle, property: 'radius', value: { Float: { Expression: fx } } }).status,
  'ok',
);

let view = gdeps();
assert.equal(view.status, 'ok');
assert.equal(view.nodes.length, 4, `vertices: ${JSON.stringify(view.nodes)}`);
assert.equal(view.edges.length, 3, `edges: ${JSON.stringify(view.edges)}`);
assert.equal(view.summary.acyclic, true);
// The exact edge set: rect.width→$base, fx→$base, idle.radius→ƒx.
const keyOf = (node) => node.key;
const byKind = (kind, match) =>
  view.nodes.find((n) => n.kind === kind && (!match || Object.entries(match).every(([k, v]) => n[k] === v)));
const varBase = byKind('variable', { variable: 'base' });
const propRect = byKind('property', { node_id: rect, property: 'width' });
const propIdle = byKind('property', { node_id: idle, property: 'radius' });
const exprFx = byKind('expression', { id: fx });
assert.ok(varBase && propRect && propIdle && exprFx, 'all four vertices present and addressable');
const edgeSet = new Set(view.edges.map((e) => `${e.from}->${e.to}`));
assert.ok(edgeSet.has(`${keyOf(propRect)}->${keyOf(varBase)}`), 'rect.width depends on $base');
assert.ok(edgeSet.has(`${keyOf(exprFx)}->${keyOf(varBase)}`), 'ƒx depends on $base');
assert.ok(edgeSet.has(`${keyOf(propIdle)}->${keyOf(exprFx)}`), 'idle.radius depends on ƒx');
console.log('smoke[9/75]: graph export = 4 vertices / 3 edges, acyclic');

// 10. Edit $base: ONLY its dependents re-evaluate, incrementally.
const before = gsnap();
assert.equal(before.eval.full_evals, 1, 'the cache was warmed once');
let gres = gsend({ type: 'SetVariable', name: 'base', value: 320.0 });
let dirty = dirtyOf(gres);
assert.deepEqual([...dirty.ids].sort(), [rect, idle].sort(), `dirty = dependents only, got ${JSON.stringify(dirty)}`);
assert.equal(dirty.mode, 'incremental', 'a value edit must never trigger a full rebuild');
let after = gsnap();
assert.equal(after.eval.full_evals, 1, 'still no full rebuild after the edit');
assert.equal(after.eval.last_dirty, 2);
assert.equal(after.eval.last_evaluated, 2);
assert.equal(after.eval.last_mode, 'incremental');
assert.equal(after.scene.nodes[rect].primitive.w, 320.0, 'rect.width ← $base patched');
assert.equal(after.scene.nodes[idle].primitive.r, 650.0, 'circle.radius ← ƒx patched (320*2+10)');
assert.equal(after.scene.nodes[rect].primitive.h, 40.0, 'unrelated params untouched');
console.log('smoke[10/75]: $base edit → Dirty = [rect, idle], patched incrementally (full_evals still 1)');

// 11. An edit nothing depends on is a no-op — visibly empty, not "everything".
gres = gsend({ type: 'SetVariable', name: 'lonely', value: 7.0 });
dirty = dirtyOf(gres);
assert.deepEqual(dirty.ids, [], 'no dependents ⇒ empty dirty set');
assert.equal(dirty.mode, 'incremental');
assert.equal(gsnap().eval.last_evaluated, 0, 'nothing was evaluated');
console.log('smoke[11/75]: edit with no dependents → Dirty.ids = [] and 0 evaluations');

// 12. Undo/redo keeps the graph exact — vertices and edges leave and return.
//     Stack at this point (top last): SetVariable base, CreateNode rect,
//     SetParameter rect.width←$base, CreateNode idle, DefineExpression ƒx,
//     SetParameter idle.radius←ƒx, SetVariable base=320, SetVariable lonely=7.
gres = JSON.parse(graph.undo()); // undo SetVariable lonely → nothing depends on it
assert.deepEqual(dirtyOf(gres).ids, [], 'unused variable: still an empty dirty set on undo');
gres = JSON.parse(graph.undo()); // undo $base=320 → both dependents re-evaluate
assert.deepEqual([...dirtyOf(gres).ids].sort(), [rect, idle].sort());
assert.equal(gsnap().scene.nodes[rect].primitive.w, 200.0, 'width back to 200');
assert.equal(gsnap().scene.nodes[idle].primitive.r, 410.0, 'radius back to 200*2+10');
gres = JSON.parse(graph.undo()); // undo idle.radius ← ƒx → one property edge drops
assert.deepEqual(dirtyOf(gres).ids, [idle], 'the unbound node is re-evaluated with its literal');
assert.equal(gsnap().scene.nodes[idle].primitive.r, 5.0, 'radius back to the literal');
view = gdeps();
assert.equal(view.nodes.length, 3, `expression still bound: ${JSON.stringify(view.nodes)}`);
assert.equal(view.edges.length, 2);
gres = JSON.parse(graph.undo()); // undo DefineExpression ƒx → expression vertex drops
assert.equal(gres.status, 'ok');
assert.deepEqual(dirtyOf(gres).ids, [], 'nothing references ƒx any more');
view = gdeps();
assert.equal(view.nodes.length, 2);
assert.equal(view.edges.length, 1, `only rect.width ← $base remains: ${JSON.stringify(view.edges)}`);
assert.equal(view.summary.acyclic, true);
gres = JSON.parse(graph.redo()); // redo DefineExpression → vertex + edge return
assert.equal(gres.status, 'ok');
assert.equal(gdeps().edges.length, 2, 'redo restores the expression vertex + edge');
gres = JSON.parse(graph.redo()); // redo idle.radius ← ƒx
assert.deepEqual(dirtyOf(gres).ids, [idle]);
assert.equal(gdeps().edges.length, 3, 'redo restores the property edge');
assert.equal(gsnap().scene.nodes[idle].primitive.r, 410.0);
gres = JSON.parse(graph.redo()); // redo $base=320
assert.deepEqual([...dirtyOf(gres).ids].sort(), [rect, idle].sort());
gres = JSON.parse(graph.redo()); // redo SetVariable lonely
assert.deepEqual(dirtyOf(gres).ids, []);
assert.equal(gsnap().scene.nodes[idle].primitive.r, 650.0, 'back to the local optimum');
console.log('smoke[12/75]: undo/redo adds and removes graph vertices/edges exactly');

// 13. Full rebuild ≡ incremental patch, and it says so.
const incremental = JSON.stringify(gsnap().scene);
gres = JSON.parse(graph.force_full_evaluation());
dirty = dirtyOf(gres);
assert.equal(dirty.mode, 'full');
assert.equal(gsnap().eval.full_evals, 2, 'the rebuild is counted');
assert.equal(gsnap().eval.last_mode, 'full', 'the snapshot reports the last sweep');
assert.equal(JSON.stringify(gsnap().scene), incremental, 'patch ≡ rebuild, byte for byte');
gres = gsend({ type: 'SetVariable', name: 'base', value: 100.0 });
dirty = dirtyOf(gres);
assert.equal(dirty.mode, 'incremental', 'the cache stays warm after a full sweep');
assert.equal(gsnap().eval.full_evals, 2, 'and no further full rebuild happens');
assert.equal(gsnap().scene.nodes[rect].primitive.w, 100.0);
console.log('smoke[13/75]: force_full_evaluation ≡ incremental scene (patch ≡ rebuild)');

// ── Task 3.1: linear constraints (Cassowary) ────────────────────────────

const solver = new glue.VectraEngine();
const ssend = (cmd) => JSON.parse(solver.dispatch_command(JSON.stringify(cmd)));
const ssnap = () => JSON.parse(solver.get_snapshot());
const sedges = () => JSON.parse(solver.dependencies()).edges;
const nodes = {};
const addRect = (name, x) => {
  const id = crypto.randomUUID();
  nodes[name] = id;
  const response = ssend({
    type: 'CreateNode',
    id,
    name,
    kind: {
      Rectangle: {
        x: { Literal: x }, y: { Literal: 0 }, width: { Literal: 10 },
        height: { Literal: 10 }, corner_radius: { Literal: 0 },
      },
    },
  });
  assert.equal(response.status, 'ok', `create ${name}: ${JSON.stringify(response)}`);
  return id;
};
const xOf = (snap, name) => snap.scene.nodes[nodes[name]].primitive.x;
const constraintOf = (kind, pairs, strength, value) => ({
  type: 'AddConstraint',
  constraint: {
    id: crypto.randomUUID(),
    kind,
    targets: pairs.map(([node, property]) => ({ node_id: node, property })),
    ...(strength ? { strength } : {}),
    ...(value === undefined ? {} : { value }),
  },
});

// 14. `$base` drives B.x; a Vertical rule forces B onto A's column. The solver
//     must NOT write `$base` — it pins B.x and reports the broken link.
assert.equal(ssend({ type: 'SetVariable', name: 'base', value: 300 }).status, 'ok');
addRect('a', 100);
addRect('b', 300);
assert.equal(
  ssend({
    type: 'SetParameter',
    node_id: nodes.b,
    property: 'x',
    value: { Float: { Variable: 'base' } },
  }).status,
  'ok',
);
assert.equal(sedges().length, 1, 'B.x ← $base is graphed');

const vertical = crypto.randomUUID();
const addVertical = {
  type: 'AddConstraint',
  constraint: {
    id: vertical,
    kind: 'vertical',
    targets: [
      { node_id: nodes.a, property: 'x' },
      { node_id: nodes.b, property: 'x' },
    ],
  },
};
let sres = ssend(addVertical);
assert.equal(sres.status, 'ok', `add vertical: ${JSON.stringify(sres)}`);
let sview = ssnap();
assert.equal(xOf(sview, 'b'), 100, 'B.x snapped to A.x');
assert.equal(xOf(sview, 'a'), 100, 'the anchor stayed put');
assert.equal(sview.solver.variables, 2, 'two solver variables (one per slot)');
assert.equal(sview.solver.constraints, 1);
assert.equal(sview.solver.writes, 1, 'exactly one slot moved');
assert.equal(sview.variables.base, 300, 'the solver never writes Document.variables');
const broken = sview.diagnostics.filter((d) => d.code === 'parametric-link-broken');
assert.equal(broken.length, 1, `the broken link is reported: ${JSON.stringify(sview.diagnostics)}`);
assert.equal(broken[0].property, 'x');
assert.equal(sedges().length, 0, 'B.x is a literal slot now');
assert.deepEqual(
  sres.events.find((e) => e.type === 'ConstraintsUpdated').ids,
  [vertical],
  'the constraint change is announced',
);
console.log('smoke[14/75]: Vertical constraint solved B.x ← A.x; link to $base broken and reported');

// 15. A PLAIN write to A.x (the Task 3.1 path — no gesture) still re-solves
//     the rule and moves B to the exact same value.
sres = ssend({
  type: 'SetParameter',
  node_id: nodes.a,
  property: 'x',
  value: { Float: { Literal: 150 } },
});
assert.equal(sres.status, 'ok');
assert.deepEqual(
  [...dirtyOf(sres).ids].sort(),
  [nodes.a, nodes.b].sort(),
  'a drag dirties the anchor and the node the constraint moved',
);
sview = ssnap();
assert.equal(xOf(sview, 'a'), 150);
assert.equal(xOf(sview, 'b'), 150, 'B matches A exactly');
assert.equal(sview.eval.full_evals, 1, 'a constraint solve is never a full re-evaluation');
console.log('smoke[15/75]: a plain write to A.x moves B exactly, incrementally (full_evals still 1)');

// 16. Undo walks back through the solver's writes, then the rule itself: the
//     pair's geometry, the parametric link and the Cassowary variables all
//     revert — one user action per step.
sres = JSON.parse(solver.undo()); // undo the drag
assert.equal(sres.status, 'ok', `${JSON.stringify(sres)}`);
sview = ssnap();
assert.ok(vertical in sview.constraints, 'the rule is still there — we only undid the drag');
assert.equal(xOf(sview, 'a'), 100, 'A is back where the drag started');
assert.equal(
  xOf(sview, 'b'),
  100,
  'B followed A back: the drag and the solve reverted together',
);
sview.diagnostics.length === 0 || assert.equal(sview.diagnostics.length, 0, 'no diagnostics');

sres = JSON.parse(solver.undo()); // undo the constraint
assert.equal(sres.status, 'ok', `${JSON.stringify(sres)}`);
sview = ssnap();
assert.ok(!(vertical in sview.constraints), 'the rule is gone');
assert.equal(xOf(sview, 'b'), 300, 'B.x is back to the unconstrained value');
assert.equal(sview.solver.variables, 0, 'Cassowary variable count decreased to 0');
assert.equal(sedges().length, 1, 'undo re-linked B.x to $base');
assert.equal(sview.diagnostics.length, 0, `the break diagnostic is not re-emitted: ${JSON.stringify(sview.diagnostics)}`);

sres = JSON.parse(solver.redo()); // redo the constraint
assert.equal(sres.status, 'ok');
sview = ssnap();
assert.equal(xOf(sview, 'b'), 100, 'redo re-enforces the rule');
assert.equal(sview.solver.variables, 2, 'and its variables are back');
sres = JSON.parse(solver.redo()); // redo the drag
assert.equal(sres.status, 'ok');
sview = ssnap();
assert.equal(xOf(sview, 'a'), 150);
assert.equal(xOf(sview, 'b'), 150, 'redo replays drag + solve');
console.log('smoke[16/75]: undo reverts (write+solve) then (rule+link); redo replays both');

// 17. Over-constrained: the weaker rule is dropped, visibly, and reported.
// `Distance(c, d) = v` is the signed separation `c - d = v`, so c sits to the
// right of d and the rule already holds.
addRect('c', 100);
addRect('d', 0);
assert.equal(
  ssend(constraintOf('distance', [[nodes.c, 'x'], [nodes.d, 'x']], 'medium', 100)).status,
  'ok',
);
assert.equal(ssnap().solver.writes, 0, 'a rule that already holds moves nothing');
const weak = constraintOf('distance', [[nodes.c, 'x'], [nodes.d, 'x']], 'weak', 200);
assert.equal(ssend(weak).status, 'ok', 'a soft contradiction is not an error');
sview = ssnap();
const dropped = sview.diagnostics.filter((d) => d.code === 'constraint-dropped');
assert.equal(dropped.length, 1, `one drop reported: ${JSON.stringify(sview.diagnostics)}`);
assert.ok(
  dropped[0].message.includes(weak.constraint.id.slice(0, 8)),
  `the diagnostic names the loser: ${dropped[0].message}`,
);
assert.equal(sview.constraints[weak.constraint.id].enabled, false, 'the loser left the active set');
assert.equal(sview.solver.dropped, 1);
assert.equal(xOf(sview, 'c') - xOf(sview, 'd'), 100, 'the survivor still holds exactly');
console.log('smoke[17/75]: 100 vs 200 → weaker dropped, disabled, diagnosed; survivor holds');

// 18. Required vs Required is a typed rejection with nothing applied.
const required = (value) => constraintOf('distance', [[nodes.c, 'x'], [nodes.d, 'x']], 'required', value);
assert.equal(ssend(required(100)).status, 'ok');
const rejected = ssend(required(777));
assert.equal(rejected.status, 'error', `required contradiction must fail: ${JSON.stringify(rejected)}`);
assert.ok(rejected.message.startsWith('unsatisfiable constraints:'), rejected.message);
assert.equal(ssnap().solver.dropped, 0, 'a rejection is not a drop');
console.log('smoke[18/75]: required/required contradiction → typed error, zero mutation');

// 19. BeginDrag registers the pointer's slots as Cassowary EDIT variables
//     (RULE 1: `add_edit_variable` — never a re-solved SetParameter), and
//     registering a gesture moves nothing: the document changes on the first
//     sample. `e`/`f` start on the same column because the rule solved on add.
addRect('e', 100);
addRect('f', 300);
const rule2 = constraintOf('vertical', [[nodes.e, 'x'], [nodes.f, 'x']], 'medium');
assert.equal(ssend(rule2).status, 'ok');
sview = ssnap();
assert.equal(xOf(sview, 'f'), 100, 'the rule solved on add');
assert.equal(sview.solver.active_edits, 0, 'no gesture yet');
sres = ssend({ type: 'BeginDrag', node_id: nodes.e });
assert.equal(sres.status, 'ok', `BeginDrag: ${JSON.stringify(sres)}`);
assert.ok(eventTypes(sres).includes('DragStarted'), 'the gesture is announced');
sview = ssnap();
assert.equal(sview.solver.active_edits, 2, 'x and y are live edit variables');
assert.equal(sview.solver.drag_node, nodes.e, 'the snapshot names the dragged node');
assert.equal(xOf(sview, 'e'), 100, 'BeginDrag moves nothing');
console.log('smoke[19/75]: BeginDrag registers EDIT variables (RULE 1) and moves nothing');

// 20. UpdateDrag samples carry ABSOLUTE pointer coordinates: the anchor and its
//     constrained partner both move, with no full re-evaluation. While the
//     gesture is live every other command is refused — typed, and inert.
sres = ssend({ type: 'UpdateDrag', node_id: nodes.e, x: 250, y: 40 });
assert.equal(sres.status, 'ok', `UpdateDrag: ${JSON.stringify(sres)}`);
sview = ssnap();
assert.equal(xOf(sview, 'e'), 250, 'the anchor reached the pointer exactly');
assert.equal(xOf(sview, 'f'), 250, 'the partner followed it exactly');
assert.equal(sview.solver.active_edits, 2, 'the registry is still open');
assert.equal(sview.eval.full_evals, 1, 'a gesture never triggers a full re-evaluation');
const refused = ssend({
  type: 'SetParameter',
  node_id: nodes.f,
  property: 'x',
  value: { Float: { Literal: 0 } },
});
assert.equal(refused.status, 'error', `mid-gesture write must be refused: ${JSON.stringify(refused)}`);
assert.ok(refused.message.includes('is in progress'), refused.message);
assert.equal(xOf(ssnap(), 'f'), 250, 'the refusal mutated nothing');
console.log('smoke[20/75]: UpdateDrag moves anchor + partner exactly; writes refused mid-gesture');

// 21. EndDrag finalizes the suggested values in the document as literals (the
//     solution is geometry, never a link) and empties the registry. The whole
//     gesture collapses into ONE undoable step.
const evalsBeforeEnd = ssnap().eval.full_evals;
const edgesBeforeEnd = sedges().length;
sres = ssend({ type: 'EndDrag', node_id: nodes.e });
assert.equal(sres.status, 'ok', `EndDrag: ${JSON.stringify(sres)}`);
assert.ok(eventTypes(sres).includes('DragEnded'), 'the gesture ends once');
sview = ssnap();
assert.equal(sview.solver.active_edits, 0, 'the edit registry is empty again');
assert.equal(sview.solver.drag_node, null, 'and no node is being dragged');
assert.equal(xOf(sview, 'e'), 250, 'the pointer value persisted');
assert.equal(xOf(sview, 'f'), 250, 'and so did the partner');
assert.equal(sview.scene.nodes[nodes.e].position.x_source, 'literal', 'stored as a literal');
assert.equal(sview.scene.nodes[nodes.f].position.x_source, 'literal', 'for the partner too');
assert.equal(sedges().length, edgesBeforeEnd, 'ending a gesture invents no link');
assert.ok(rule2.constraint.id in sview.constraints, 'the rule the gesture obeyed is untouched');
assert.equal(ssnap().eval.full_evals, evalsBeforeEnd, 'ending a gesture re-solves incrementally');
sres = JSON.parse(solver.undo()); // one undo: the whole gesture
assert.equal(sres.status, 'ok');
sview = ssnap();
assert.equal(xOf(sview, 'e'), 100, 'one undo restores the anchor');
assert.equal(xOf(sview, 'f'), 100, 'and the partner — the gesture was one step');
assert.ok(rule2.constraint.id in sview.constraints, 'the gesture did not touch the rule');
sres = JSON.parse(solver.redo());
assert.equal(sres.status, 'ok');
assert.equal(xOf(ssnap(), 'e'), 250, 'redo replays the net movement without a solver');
console.log('smoke[21/75]: EndDrag persists literals + clears the registry; 1 undo reverts it all');

// ── Task 4.0: non-destructive operations ────────────────────────────────
// A fresh engine, so the numbers below are the operation layer's own.
const ops = new glue.VectraEngine();
const osend = (cmd) => JSON.parse(ops.dispatch_command(JSON.stringify(cmd)));
const osnap = () => JSON.parse(ops.get_snapshot());

function addShape(name, kind, extra = {}) {
  const node = crypto.randomUUID();
  const created = osend({ type: 'CreateNode', id: node, name, kind, ...extra });
  assert.equal(created.status, 'ok', `CreateNode ${name}: ${JSON.stringify(created)}`);
  return node;
}

// 22. ApplyOperation: the sources keep everything, the result is a scene node.
const plate = addShape('plate', {
  Rectangle: {
    x: { Literal: 0 }, y: { Literal: 0 },
    width: { Literal: 100 }, height: { Literal: 100 },
    corner_radius: { Literal: 0 },
  },
});
const hole = addShape('hole', {
  Rectangle: {
    x: { Literal: 20 }, y: { Literal: 20 },
    width: { Literal: 40 }, height: { Literal: 40 },
    corner_radius: { Literal: 0 },
  },
});
// The sources are rectangles, so "untouched" is asserted on the whole node
// (primitive + style), not on a `d` string only paths have.
const plateBefore = JSON.stringify(osnap().scene.nodes[plate]);
const op = crypto.randomUUID();
let ores = osend({
  type: 'ApplyOperation',
  id: op,
  kind: { type: 'boolean', op: 'subtract' },
  inputs: [plate, hole],
});
assert.equal(ores.status, 'ok', `ApplyOperation: ${JSON.stringify(ores)}`);
assert.ok(eventTypes(ores).includes('OperationsUpdated'), 'OperationsUpdated expected');
assert.ok(eventTypes(ores).includes('Dirty'), 'the result must be evaluated now');
let ov = osnap();
assert.equal(ov.operations[op].kind, 'boolean', 'registered as a boolean');
assert.equal(ov.operations[op].description, 'subtract', 'engine-rendered description');
assert.deepEqual(ov.operations[op].inputs, [plate, hole], 'operands in order');
assert.deepEqual(ov.operations[op].input_names, ['plate', 'hole'], 'operand labels');
assert.equal(ov.operations[op].enabled, true);
assert.equal(ov.scene.nodes[op].primitive.type, 'path', 'a standard scene node (RULE 3)');
assert.ok(ov.scene.nodes[op].primitive.d.length > 0, 'with geometry');
assert.ok(ov.scene.z_order.includes(op), 'and a place in the draw order');
assert.ok(ov.scene.z_order.includes(plate) && ov.scene.z_order.includes(hole), 'sources stay');
const opD = ov.scene.nodes[op].primitive.d;
assert.equal(
  JSON.stringify(ov.scene.nodes[plate]),
  plateBefore,
  'the source is untouched by the operation',
);
assert.ok(opD.includes('M'), `operation geometry looks like a path: ${opD}`);
console.log('smoke[22/75]: ApplyOperation → virtual path node; sources stay in the scene');

// 23. The non-destructive law: moving a source re-runs the operation, never the
//     other way round. `hole` moves 40 to the right → the cut moves with it
//     while `plate`'s own path data is byte-identical.
const plateNode = JSON.stringify(ov.scene.nodes[plate]);
const holeNode = JSON.stringify(ov.scene.nodes[hole]);
ores = osend({
  type: 'SetParameter',
  node_id: hole,
  property: 'x',
  value: { Float: { Literal: 60 } },
});
assert.equal(ores.status, 'ok', `SetParameter: ${JSON.stringify(ores)}`);
const moved = ores.events.filter((e) => e.type === 'Dirty').flatMap((e) => e.ids);
assert.ok(moved.includes(op), `the dependent operation must be dirty: ${JSON.stringify(ores)}`);
ov = osnap();
assert.equal(JSON.stringify(ov.scene.nodes[plate]), plateNode, 'partner node is byte-identical');
assert.notEqual(JSON.stringify(ov.scene.nodes[hole]), holeNode, 'the moved source re-evaluated');
assert.notEqual(ov.scene.nodes[op].primitive.d, opD, 'and so did the operation');
console.log('smoke[23/75]: a source move re-runs only the dependent operation');

// 24. Parking vs removing, and the typed refusal of a stray operand.
ores = osend({ type: 'SetOperationEnabled', id: op, enabled: false });
assert.equal(ores.status, 'ok', `SetOperationEnabled: ${JSON.stringify(ores)}`);
ov = osnap();
assert.equal(ov.operations[op].enabled, false, 'the registry row survives');
assert.equal(ov.scene.nodes[op], undefined, 'but its geometry is withdrawn');
assert.ok(ov.scene.z_order.includes(plate), 'sources are unaffected by a parked operation');
ores = osend({ type: 'SetOperationEnabled', id: op, enabled: true });
assert.equal(ores.status, 'ok');
assert.equal(osnap().scene.nodes[op].primitive.type, 'path', 're-enabling re-evaluates it');
ores = osend({
  type: 'ApplyOperation',
  id: crypto.randomUUID(),
  kind: { type: 'boolean', op: 'union' },
  inputs: [plate],
});
assert.equal(ores.status, 'error', 'arity is enforced before anything is stored');
assert.match(ores.message, /takes 2 input\(s\), got 1/, ores.message);
const opCount = Object.keys(osnap().operations).length;
ores = osend({ type: 'RemoveOperation', id: op });
assert.equal(ores.status, 'ok', `RemoveOperation: ${JSON.stringify(ores)}`);
ov = osnap();
assert.equal(Object.keys(ov.operations).length, opCount - 1, 'the row is gone');
assert.equal(ov.scene.nodes[op], undefined, 'and so is its geometry');
assert.ok(ov.scene.z_order.includes(plate) && ov.scene.z_order.includes(hole), 'sources remain');
assert.ok(ores.events.some((e) => e.type === 'Dirty'), 'the removal re-evaluated the scene');
ores = osend({ type: 'RemoveOperation', id: op });
assert.equal(ores.status, 'error', 'removing it twice is a typed error, not a crash');
console.log('smoke[24/75]: park → re-enable → remove; arity + double-remove are typed');

// ── Task 5.0: the WebGPU canvas (a dumb consumer of EvaluatedScene) ─────
// A fresh engine plus a crenderer on an 800×600 canvas. Node has no WebGPU, so
// `attach` is never called: the crenderer runs its whole *decision* path
// (tessellate, diff, plan, spatial index) and skips only the upload. That is
// exactly the split the browser relies on, minus the pixels.
const cengine = new glue.VectraEngine();
const csend = (cmd) => JSON.parse(cengine.dispatch_command(JSON.stringify(cmd)));
const crenderer = new glue.Renderer();
crenderer.set_viewport(0, 0, 800, 600, 1);
assert.equal(crenderer.gpu_ready(), false, 'no WebGPU in Node — and no pretending');

const shapes = (() => {
  const make = (name, x, y, w, h) => {
    const node = crypto.randomUUID();
    const created = csend({
      type: 'CreateNode', id: node, name,
      kind: { Rectangle: {
        x: { Literal: x }, y: { Literal: y },
        width: { Literal: w }, height: { Literal: h },
        corner_radius: { Literal: 0 },
      }},
    });
    assert.equal(created.status, 'ok', `CreateNode ${name}: ${JSON.stringify(created)}`);
    return node;
  };
  return { a: make('a', 0, 0, 100, 100), b: make('b', 300, 0, 50, 50) };
})();

// 25. The frame: full once, then surgical, then free.
let canvasFrame = JSON.parse(cengine.render_frame(crenderer));
assert.equal(canvasFrame.ready, false, 'the report is honest about the missing device');
assert.equal(canvasFrame.nodes, 2, `two nodes on the canvas: ${JSON.stringify(canvasFrame)}`);
assert.equal(canvasFrame.full, true, 'the first frame reconciles everything');
assert.equal(canvasFrame.created, 2, 'and creates exactly two node buffer sets');
assert.ok(canvasFrame.writes >= 4, `vertices + indices per node: ${JSON.stringify(canvasFrame)}`);

csend({
  type: 'SetParameter', node_id: shapes.a, property: 'x',
  value: { Float: { Literal: 120 } },
});
canvasFrame = JSON.parse(cengine.render_frame(crenderer));
assert.equal(canvasFrame.dirty, 1, `one node was dirty: ${JSON.stringify(canvasFrame)}`);
assert.equal(canvasFrame.moved, 1, 'classified as a move');
assert.equal(canvasFrame.retessellated, 0, 'a move is not a reshape');
assert.equal(canvasFrame.writes, 1, 'ONE buffer write for the whole frame');
assert.equal(canvasFrame.bytes, 80, 'and it is one instance row');
assert.equal(canvasFrame.nodes, 2, 'the other node is still resident');

canvasFrame = JSON.parse(cengine.render_frame(crenderer));
assert.equal(canvasFrame.writes, 0, 'an idle frame writes nothing');
assert.equal(canvasFrame.bytes, 0, 'and costs nothing');

// A gesture: 20 pointer samples, one drawn frame, one write. The ledger
// coalesces; the crenderer never sees the intermediate states.
const cbegin = csend({ type: 'BeginDrag', node_id: shapes.a });
assert.equal(cbegin.status, 'ok', `BeginDrag: ${JSON.stringify(cbegin)}`);
for (let step = 1; step <= 20; step += 1) {
  csend({ type: 'UpdateDrag', node_id: shapes.a, x: 120 + step, y: 10 + step });
}
csend({ type: 'EndDrag', node_id: shapes.a });
canvasFrame = JSON.parse(cengine.render_frame(crenderer));
assert.ok(canvasFrame.writes <= 1, `twenty samples, one upload: ${JSON.stringify(canvasFrame)}`);
assert.equal(canvasFrame.retessellated, 0, 'and not one re-tessellation');
console.log(
  'smoke[25/36]: render_frame → full once, one 80-byte move, idle frames free; 20 samples = 1 upload',
);

// 26. RULE 3: the pointer names a node, and the name drives the triad.
//     Document y is up, screen y is down: node `b` spans doc y 0..50, i.e.
//     screen y 550..600 on this 600-tall canvas.
//     (wasm-bindgen maps `Option::None` to `undefined`, so the helper — like
//     `VectraClient.pointerHit` — normalises the miss to `null`.)
const hitAt = (x, y) => crenderer.pointer_hit(x, y) ?? null;
assert.equal(hitAt(325, 575), shapes.b, 'the centre of b');
assert.equal(hitAt(700, 20), null, 'empty space hits nothing');
assert.equal(hitAt(325, 25), null, 'b is not where the screen says');
const picked = hitAt(325, 575);
assert.equal(picked, shapes.b);
const cdoc = JSON.parse(crenderer.pointer_doc(325, 575));
assert.ok(Math.abs(cdoc.x - 325) < 0.5 && Math.abs(cdoc.y - 25) < 0.5, JSON.stringify(cdoc));
assert.equal(
  csend({ type: 'BeginDrag', node_id: picked }).status,
  'ok',
  'the renderer\'s answer is a valid drag target',
);
assert.equal(csend({ type: 'UpdateDrag', node_id: picked, x: 500, y: 400 }).status, 'ok');
assert.equal(csend({ type: 'EndDrag', node_id: picked }).status, 'ok');
JSON.parse(cengine.render_frame(crenderer));
// The index tracked the edit: the hit moved with the node.
assert.equal(hitAt(525, 175), shapes.b, 'b is where the pointer put it');
assert.equal(hitAt(325, 575), null, 'and its old home is empty');
const cstats = JSON.parse(crenderer.stats());
assert.equal(cstats.ready, false);
assert.equal(cstats.nodes, 2, 'the canvas still holds exactly two nodes');
console.log('smoke[25/75]: pointer → NodeId → BeginDrag/UpdateDrag/EndDrag, and the index follows');

// ── Task 6.0: motion ───────────────────────────────────────────────────
//
// A second engine, so the motion scenarios start from a scene this file
// controls end to end and the canvas sections above keep their own state.
const mengine = new glue.VectraEngine();
const msend = (cmd) => JSON.parse(mengine.dispatch_command(JSON.stringify(cmd)));
const mframe = () => JSON.parse(mengine.render_frame(mrenderer));
const mstatus = () => JSON.parse(mengine.motion_json());
const mrenderer = new glue.Renderer();
mrenderer.set_viewport(0, 0, 800, 600, 1);
const mvalue = (nodeId, property) => {
  const binding = mstatus().bindings.find(
    (b) => b.node_id === nodeId && b.property === property,
  );
  return binding ? binding.value : null;
};
const undoDepth = () => JSON.parse(mengine.get_snapshot()).undo_depth;

const mover = (() => {
  const node = crypto.randomUUID();
  const created = msend({
    type: 'CreateNode', id: node, name: 'hover-me',
    kind: { Rectangle: {
      x: { Literal: 100 }, y: { Literal: 100 },
      width: { Literal: 200 }, height: { Literal: 120 },
      corner_radius: { Literal: 0 },
    } },
  });
  assert.equal(created.status, 'ok', JSON.stringify(created));
  return node;
})();
mframe(); // the hit index describes the drawn scene; draw it

// 27. Binding + hover.
const bindResponse = JSON.parse(
  mengine.bind_hover_spring(mover, 'width', 200, 320, 170, 26),
);
assert.equal(bindResponse.status, 'ok', `bind_hover_spring: ${JSON.stringify(bindResponse)}`);
let mreport = mstatus();
assert.equal(mreport.bindings.length, 1, `one binding: ${JSON.stringify(mreport)}`);
assert.equal(mreport.bindings[0].kind, 'spring');
assert.equal(mreport.bindings[0].from, 200, 'anchored at the slot it is replacing');
assert.equal(mvalue(mover, 'width'), 200, 'at rest at the off value');
assert.equal(mengine.is_animating(), false, 'a spring at its anchor is not moving');

const moutside = JSON.parse(mengine.pointer_move(mrenderer, 700, 500));
assert.equal(moutside.events.length, 0, 'the pointer is nowhere near the shape');
// Document y is UP: the rect spans doc y 100..220, so screen y 380..500.
const moverPoint = JSON.parse(mrenderer.pointer_doc(200, 440));
assert.ok(
  moverPoint.x > 100 && moverPoint.x < 300 && moverPoint.y > 100 && moverPoint.y < 220,
  `the sampled pointer is inside the rect: ${JSON.stringify(moverPoint)}`,
);
const moverHit = mengine.pointer_move(mrenderer, moverPoint.x, moverPoint.y);
const moverEvents = JSON.parse(moverHit);
assert.equal(moverEvents.status, 'ok', moverHit);
assert.ok(
  moverEvents.events.some((e) => e.type === 'Dirty'),
  `a hover flip publishes a dirty set: ${moverHit}`,
);
assert.deepEqual(
  JSON.parse(mengine.motion_json()).states.length,
  1,
  'and the flag is on',
);
assert.equal(mengine.is_animating(), true, 'so the scene is moving');
assert.equal(mvalue(mover, 'width'), 200, 'the transition starts at the off value');
mengine.set_time(0.15);
const midMotion = mvalue(mover, 'width');
assert.ok(midMotion > 200 && midMotion < 320, `easing, not jumping: ${midMotion}`);
console.log('smoke[26/75]: hover flip re-anchors the spring → it eases from where it was');

// 28. Purity + the idle signal.
const sample = (t) => {
  mengine.set_time(t);
  return JSON.stringify(JSON.parse(mengine.get_snapshot()).scene);
};
const forward = [0.2, 0.4, 0.6, 0.8].map((t) => [t, sample(t)]);
let purities = 0;
for (const [t, expected] of [...forward].reverse()) {
  assert.equal(sample(t), expected, `t=${t} replayed identically`);
  purities += 1;
}
assert.equal(purities, 4);
const horizon = mstatus().horizon;
assert.ok(horizon !== null, 'the engine says when it will stop');
mengine.set_time(horizon + 0.05);
assert.equal(mengine.is_animating(), false, 'at the horizon it has stopped');
assert.ok(Math.abs(mvalue(mover, 'width') - 320) < 0.01, 'and it is at the target');
// Two frames: the first pays for the last clock step (the ledger coalesces
// until the next *drawn* frame), the second — with nothing moving in between —
// is the one that must be free.
const settlingFrame = mframe();
assert.ok(settlingFrame.writes > 0, 'the last clock step is drawn once');
const idleFrame = mframe();
assert.equal(idleFrame.writes, 0, `a stopped scene draws for free: ${JSON.stringify(idleFrame)}`);
assert.equal(idleFrame.bytes, 0, 'and costs zero bytes');
assert.equal(idleFrame.dirty, 0, 'and has nothing to reconcile');
console.log('smoke[27/75]: same t ⇒ same scene; idle at the horizon ⇒ the loop can stop');

// 29. Undo isolation.
const depthBefore = undoDepth();
for (let step = 0; step < 60; step += 1) {
  mengine.set_time((step % 10) / 10);
  mengine.is_animating();
  if (step % 20 === 0) {
    mengine.pointer_move(mrenderer, moverPoint.x, moverPoint.y);
  }
  if (step % 20 === 10) {
    mengine.pointer_leave();
  }
}
mframe();
assert.equal(undoDepth(), depthBefore, '60 frames of motion created zero history');
const undoOnce = JSON.parse(mengine.undo());
assert.equal(undoOnce.status, 'ok', JSON.stringify(undoOnce));
assert.equal(undoDepth(), depthBefore - 1, 'while the binding is exactly one entry');
assert.equal(mstatus().bindings.length, 0, 'undo removed the binding');
const redoOnce = JSON.parse(mengine.redo());
assert.equal(redoOnce.status, 'ok', JSON.stringify(redoOnce));
assert.equal(mstatus().bindings.length, 1, 'and redo restored it');
console.log('smoke[28/75]: motion samples are not history (60 frames, 0 entries)');

// 30. Interaction precedence — and note the law is *per slot*: the spring on
//     `width` is not what the pointer touches, so only `x` changes hands.
const bindX = JSON.parse(mengine.bind_spring(mover, 'x', 700, 170, 26));
assert.equal(bindX.status, 'ok', `bind_spring on x: ${JSON.stringify(bindX)}`);
const xBefore = mstatus().bindings.find((b) => b.property === 'x');
assert.equal(xBefore.kind, 'spring');
assert.equal(xBefore.from, 100, 'anchored where the slot is now (its authored x)');
assert.equal(mengine.is_animating(), true, 'and it starts easing toward 700');
const widthBeforeDrag = mstatus().bindings.find((b) => b.property === 'width').from;

assert.equal(msend({ type: 'BeginDrag', node_id: mover }).status, 'ok', 'the gesture opens');
msend({ type: 'UpdateDrag', node_id: mover, x: 620, y: 300 });
assert.equal(
  JSON.parse(mengine.get_snapshot()).scene.nodes[mover].position.x,
  620,
  'the pointer owns the slot while it is down',
);
msend({ type: 'EndDrag', node_id: mover });
mreport = mstatus();
assert.equal(mreport.bindings.length, 2, 'both springs survived the drag');
assert.equal(
  mreport.bindings.find((b) => b.property === 'x').from,
  620,
  'the dragged slot re-anchored at the committed value',
);
assert.equal(
  mreport.bindings.find((b) => b.property === 'width').from,
  widthBeforeDrag,
  'and the untouched slot kept its anchor: precedence is per slot',
);
const mUndone = JSON.parse(mengine.undo());
assert.equal(mUndone.status, 'ok', JSON.stringify(mUndone));
assert.equal(
  mstatus().bindings.find((b) => b.property === 'x').from,
  100,
  'undo restored the pre-drag anchor',
);
const mRedone = JSON.parse(mengine.redo());
assert.equal(mRedone.status, 'ok', JSON.stringify(mRedone));
mreport = mstatus();
assert.equal(mreport.bindings.find((b) => b.property === 'x').from, 620, 'redo replays it');
assert.equal(
  JSON.parse(mengine.get_snapshot()).scene.nodes[mover].position.x_source,
  'animated',
  'and the slot is animated again, not frozen into a literal',
);
// Release the x binding so the track section below starts from a clean slot.
msend({
  type: 'SetParameter', node_id: mover, property: 'x', value: { Float: { Literal: 620 } },
});
console.log('smoke[29/75]: a drag outranks a spring, hands it back, and only on its own slot');

// 31. Keyframe tracks (the other half of MES §12).
const track = {
  id: 'intro',
  name: 'Intro',
  channels: { x: [{ time: 0, value: 0 }, { time: 1, value: 100 }] },
};
const trackAdded = JSON.parse(mengine.set_motion_track(JSON.stringify(track)));
assert.equal(trackAdded.status, 'ok', JSON.stringify(trackAdded));
const trackBound = msend({
  type: 'BindMotion', node_id: mover, property: 'y',
  binding: { KeyframeTrack: { track_id: 'intro', property: 'x' } },
});
assert.equal(trackBound.status, 'ok', JSON.stringify(trackBound));
mengine.set_time(0.5);
assert.equal(mvalue(mover, 'y'), 50, 'sampled mid-segment');
const depthBeforeTrack = undoDepth();
mengine.set_time(0.75);
assert.equal(mvalue(mover, 'y'), 75, 'and the sample follows the clock for free');
assert.equal(undoDepth(), depthBeforeTrack, 'a sample is never an entry');
const edited = JSON.parse(
  mengine.set_motion_track(
    JSON.stringify({
      id: 'intro', name: 'Intro',
      channels: { x: [{ time: 0, value: 0 }, { time: 1, value: 200 }] },
    }),
  ),
);
assert.equal(edited.status, 'ok', JSON.stringify(edited));
assert.equal(mvalue(mover, 'y'), 150, 'a track edit re-samples the bound slot');
const badTrack = JSON.parse(
  mengine.set_motion_track(
    JSON.stringify({
      id: 'nope', name: 'Nope',
      channels: { x: [{ time: 1, value: 0 }, { time: 0, value: 1 }] },
    }),
  ),
);
assert.equal(badTrack.status, 'error', 'non-monotone times are refused typed');
const trackRemoved = JSON.parse(mengine.remove_motion_track('intro'));
assert.equal(trackRemoved.status, 'ok', JSON.stringify(trackRemoved));
assert.equal(mstatus().tracks.length, 0, 'the registry is empty again');
console.log('smoke[30/75]: tracks register, sample, re-sample on edit, and refuse malformed input');

// ── Task 7.0: the procedural graph ─────────────────────────────────────

const pengine = new glue.VectraEngine();
const psend = (cmd) => JSON.parse(pengine.dispatch_command(JSON.stringify(cmd)));
const psnap = () => JSON.parse(pengine.get_snapshot());
const pstatus = () => JSON.parse(pengine.procedural_json());
const pnode = (id) => pstatus().nodes.find((node) => node.id === id);
const pport = (id, port) =>
  pnode(id).outputs.find((output) => output.port === port).value;

// 32. The palette is engine data, and a node added from it draws.
const palette = JSON.parse(pengine.procedural_kinds());
assert.deepEqual(
  palette.map((kind) => kind.tag),
  ['source', 'grid', 'repeat', 'noise', 'smooth'],
);
const gridKind = palette.find((kind) => kind.tag === 'grid');
assert.equal(gridKind.needs_subject, false);
const gridId = crypto.randomUUID();
const gridAdded = psend({
  type: 'AddProceduralNode',
  node: { id: gridId, kind: { type: 'grid', ...gridKind.operands } },
});
assert.equal(gridAdded.status, 'ok', JSON.stringify(gridAdded));
assert.ok(eventTypes(gridAdded).includes('ProceduralUpdated'), 'missing ProceduralUpdated');
let pview = pstatus();
assert.equal(pview.count, 1);
assert.equal(pview.nodes[0].kind, 'grid');
assert.match(pview.nodes[0].name, /grid/, 'the engine named it from its kind');
assert.equal(pport(gridId, 'span'), 'scalar 120', '3 × 40 apart');
let pscene = psnap();
assert.ok(pscene.scene.z_order.includes(gridId), 'the grid is scene geometry');
assert.equal(
  pscene.scene.nodes[gridId].name,
  pview.nodes[0].name,
  'and the inspector can name it (RULE 4)',
);
console.log('smoke[31/75]: the palette is the engine\'s table, and a node from it composes');

// 33. The gates: a type mismatch, a disguised cycle, a real cycle.
const prect = crypto.randomUUID();
psend({
  type: 'CreateNode',
  id: prect,
  name: 'proc-rect',
  kind: {
    Rectangle: {
      x: { Literal: 0 }, y: { Literal: 0 },
      width: { Literal: 40 }, height: { Literal: 30 },
      corner_radius: { Literal: 0 },
    },
  },
});
const sourceId = crypto.randomUUID();
assert.equal(
  psend({
    type: 'AddProceduralNode',
    node: { id: sourceId, kind: { type: 'source', node: prect } },
  }).status,
  'ok',
);
const repeatId = crypto.randomUUID();
assert.equal(
  psend({
    type: 'AddProceduralNode',
    node: { id: repeatId, kind: { type: 'repeat', ...palette.find((k) => k.tag === 'repeat').operands } },
  }).status,
  'ok',
);
const smoothId = crypto.randomUUID();
assert.equal(
  psend({
    type: 'AddProceduralNode',
    node: { id: smoothId, kind: { type: 'smooth', ...palette.find((k) => k.tag === 'smooth').operands } },
  }).status,
  'ok',
);
const wired = psend({
  type: 'ConnectProcedural', node_id: smoothId, port: 'region',
  from: { node: repeatId, port: 'region' },
});
assert.equal(wired.status, 'ok', JSON.stringify(wired));
const beforeGates = JSON.stringify(pstatus());
const mismatch = psend({
  type: 'ConnectProcedural', node_id: smoothId, port: 'region',
  from: { node: gridId, port: 'span' },
});
assert.equal(mismatch.status, 'error', 'a scalar into a region port is refused');
assert.match(mismatch.message, /scalar/i);
const disguised = psend({
  type: 'SetProceduralOperand', node_id: smoothId, port: 'strength',
  value: { Float: { Procedural: { node: gridId, port: 'region' } } },
});
assert.equal(disguised.status, 'error', 'RULE 3: an operand cannot read a port');
const cycle = psend({
  type: 'ConnectProcedural', node_id: repeatId, port: 'region',
  from: { node: smoothId, port: 'region' },
});
assert.equal(cycle.status, 'error', 'a wire that closes the chain is refused');
assert.match(cycle.message, /cycl/i);
assert.equal(JSON.stringify(pstatus()), beforeGates, 'every refusal left the graph as it was');
console.log('smoke[32/75]: typed refusals — port type, disguised cycle, real cycle');

// 34. A slot that reads a port follows it in the same dispatch.
assert.equal(
  psend({
    type: 'SetParameter', node_id: prect, property: 'width',
    value: { Float: { Procedural: { node: gridId, port: 'span' } } },
  }).status,
  'ok',
);
assert.equal(psnap().scene.nodes[prect].primitive.w, 120, 'width reads the grid\'s span');
const respaced = psend({
  type: 'SetProceduralOperand', node_id: gridId, port: 'spacing',
  value: { Float: { Literal: 25 } },
});
assert.equal(respaced.status, 'ok', JSON.stringify(respaced));
assert.equal(
  psnap().scene.nodes[prect].primitive.w,
  75,
  'the reader followed the republished port in the SAME settle',
);
console.log('smoke[33/75]: a republished port reaches its readers within one settle');

// 35. RULE 4: an unrelated operation pass cannot evict the result.
const q1 = crypto.randomUUID();
const q2 = crypto.randomUUID();
for (const [qid, x] of [[q1, 0], [q2, 20]]) {
  psend({
    type: 'CreateNode', id: qid,
    kind: {
      Rectangle: {
        x: { Literal: x }, y: { Literal: 0 },
        width: { Literal: 40 }, height: { Literal: 30 },
        corner_radius: { Literal: 0 },
      },
    },
  });
}
const unionId = crypto.randomUUID();
const union = psend({
  type: 'ApplyOperation', id: unionId,
  kind: { type: 'boolean', op: 'union' }, inputs: [q1, q2],
});
assert.equal(union.status, 'ok', JSON.stringify(union));
pscene = psnap();
assert.ok(pscene.scene.z_order.includes(unionId), 'the union composed');
assert.ok(
  pscene.scene.z_order.includes(gridId),
  'and the procedural result is still drawn — no silent eviction',
);
const parked = psend({ type: 'SetProceduralEnabled', id: gridId, enabled: false });
assert.equal(parked.status, 'ok');
assert.ok(!psnap().scene.z_order.includes(gridId), 'parked ⇒ the geometry left the scene');
psend({ type: 'SetProceduralEnabled', id: gridId, enabled: true });
assert.ok(psnap().scene.z_order.includes(gridId), 're-armed ⇒ it is drawn again');
const wireBefore = pnode(smoothId).wires.region;
psend({ type: 'RemoveProceduralNode', id: repeatId });
assert.ok(!pnode(repeatId), 'the node is gone');
assert.ok(!psnap().scene.z_order.includes(repeatId), 'and so is its geometry');
assert.equal(
  pengine.undo() && JSON.parse(pengine.get_snapshot()).status,
  'ok',
);
assert.equal(pnode(smoothId).wires.region, wireBefore, 'undo restored the node AND its wire');
console.log('smoke[34/75]: RULE 4 holds — survives an operation pass, parks, re-arms, undoes');

// 36. Determinism, patch ≡ rebuild, and a mutation nothing depends on.
const build = () => {
  const local = new glue.VectraEngine();
  const send = (cmd) => JSON.parse(local.dispatch_command(JSON.stringify(cmd)));
  send({
    type: 'CreateNode', id: 'dddddddd-0000-4000-8000-000000000001', name: 'base',
    kind: {
      Rectangle: {
        x: { Literal: 0 }, y: { Literal: 0 },
        width: { Literal: 40 }, height: { Literal: 30 },
        corner_radius: { Literal: 0 },
      },
    },
  });
  send({
    type: 'AddProceduralNode',
    node: {
      id: 'dddddddd-0000-4000-8000-000000000002',
      kind: { type: 'source', node: 'dddddddd-0000-4000-8000-000000000001' },
    },
  });
  send({
    type: 'AddProceduralNode',
    node: {
      id: 'dddddddd-0000-4000-8000-000000000003',
      kind: { type: 'noise', amplitude: { Literal: 1 }, frequency: { Literal: 0.1 }, seed: { Literal: 7 } },
    },
  });
  send({
    type: 'ConnectProcedural',
    node_id: 'dddddddd-0000-4000-8000-000000000003', port: 'region',
    from: { node: 'dddddddd-0000-4000-8000-000000000002', port: 'region' },
  });
  const snap = JSON.parse(local.get_snapshot());
  return JSON.stringify(snap.scene);
};
const pictureA = build();
const pictureB = build();
assert.equal(pictureA, pictureB, 'two engines, one script, one byte-identical picture');
const pIncremental = JSON.stringify(psnap().scene);
const pRebuild = dirtyOf(JSON.parse(pengine.force_full_evaluation()));
assert.equal(pRebuild.mode, 'full', 'the sweep rebuilt everything');
assert.equal(
  JSON.stringify(psnap().scene),
  pIncremental,
  'patch ≡ rebuild with a procedural graph in the picture',
);
const pAgain = dirtyOf(JSON.parse(pengine.force_full_evaluation()));
assert.equal(pAgain.mode, 'full');
const untouched = psend({ type: 'SetVariable', name: 'nobody-reads-me', value: 3 });
assert.equal(untouched.status, 'ok');
const pDirty = untouched.events.find((event) => event.type === 'Dirty');
// A variable nothing reads is not a node: the dirty set names no geometry.
assert.deepEqual(pDirty.ids, [], 'nothing depended on it, so nothing re-ran');
console.log('smoke[35/75]: determinism, patch ≡ rebuild, and an empty dirty set');

// ── Task 8.0: the export boundary ──────────────────────────────────────────
// 37. `export_to_svg` writes a SEMANTIC document: a circle is a `<circle>`, an
//     arc is one `A` command, and nothing is a bezier approximation. The
//     envelope says which format it is, and carries the engine's warnings.
{
  const xengine = new glue.VectraEngine();
  const xsend = (cmd) => JSON.parse(xengine.dispatch_command(JSON.stringify(cmd)));
  xsend({
    type: 'CreateNode', id: 'eeeeeeee-0000-4000-8000-000000000001', name: 'dot',
    kind: { Circle: { cx: { Literal: 40 }, cy: { Literal: 20 }, radius: { Literal: 10 } } },
  });
  xsend({
    type: 'CreateNode', id: 'eeeeeeee-0000-4000-8000-000000000002', name: 'swoosh',
    kind: { Arc: {
      cx: { Literal: 0 }, cy: { Literal: 0 }, radius: { Literal: 50 },
      start_angle: { Literal: 0 }, end_angle: { Literal: 1.5707963267948966 },
    } },
  });
  const envelope = JSON.parse(xengine.export_to_svg());
  assert.equal(envelope.status, 'ok');
  assert.equal(envelope.format, 'svg');
  assert.ok(envelope.code.startsWith('<?xml'), 'an SVG document, not a fragment');
  assert.ok(envelope.code.includes('<circle '), 'a circle stays a circle');
  assert.ok(!envelope.code.includes('<ellipse'), 'and is not a two-radius ellipse');
  assert.ok(/<path [^>]*d="M [^"]* A /.test(envelope.code), 'an arc is one A command');
  assert.ok(!/d="M [^"]*[CcSsQq]/.test(envelope.code), 'no bezier stand-ins');
  assert.deepEqual(envelope.warnings, [], 'nothing to warn about');
  // The same engine, exported twice: identical bytes.
  assert.equal(xengine.export_to_svg(), xengine.export_to_svg());
  console.log('smoke[36/75]: semantic SVG — <circle>, one A command, no approximations');
}

// 38. `export_to_react` writes a PARAMETRIC component (Task 8.0 RULE 2): the
//     variable is a required prop and the expression is the arithmetic — the
//     number never appears in the signature's place.
{
  const rengine = new glue.VectraEngine();
  const rsend = (cmd) => JSON.parse(rengine.dispatch_command(JSON.stringify(cmd)));
  rsend({ type: 'SetVariable', name: 'base', value: 40 });
  const expr = 'eeeeeeee-0000-4000-8000-000000000010';
  rsend({ type: 'DefineExpression', id: expr, source: '$base * 2' });
  rsend({
    type: 'CreateNode', id: 'eeeeeeee-0000-4000-8000-000000000011', name: 'box',
    kind: { Rectangle: {
      x: { Literal: 0 }, y: { Literal: 0 },
      width: { Literal: 1 }, height: { Literal: 30 },
      corner_radius: { Literal: 0 },
    } },
  });
  rsend({
    type: 'SetParameter', node_id: 'eeeeeeee-0000-4000-8000-000000000011',
    property: 'width', value: { Float: { Expression: expr } },
  });
  const envelope = JSON.parse(rengine.export_to_react());
  assert.equal(envelope.format, 'react');
  assert.ok(envelope.code.includes('base: number; // 40'), 'the prop is declared, typed, with its value');
  assert.ok(envelope.code.includes('width={base * 2}'), 'the expression is the code');
  assert.ok(!envelope.code.includes('width={80}'), 'and never the resolved number');
  assert.ok(envelope.code.includes('export function Scene('), 'a real component');
  // The picture export of the SAME document is the number: a picture is a
  // picture, code is code.
  const svg = JSON.parse(rengine.export_to_svg());
  assert.ok(svg.code.includes('width="80"'), 'SVG carries the resolved number');
  console.log('smoke[37/75]: parametric React — $base * 2 → width={base * 2} + a required prop');
}

// 39. The export sees the LIVE scene: a spring-driven slot exports the number
//     the canvas is drawing, and the file says so instead of pretending.
{
  const mengine = new glue.VectraEngine();
  const msend = (cmd) => JSON.parse(mengine.dispatch_command(JSON.stringify(cmd)));
  const node = 'eeeeeeee-0000-4000-8000-000000000020';
  msend({
    type: 'CreateNode', id: node, name: 'box',
    kind: { Rectangle: {
      x: { Literal: 0 }, y: { Literal: 0 },
      width: { Literal: 40 }, height: { Literal: 30 }, corner_radius: { Literal: 0 },
    } },
  });
  msend({
    type: 'BindMotion', node_id: node, property: 'height',
    binding: { Spring: { target: { Literal: 100 }, stiffness: 120, damping: 14, from: 0, at: 0 } },
  });
  mengine.set_time(0.05);
  const envelope = JSON.parse(mengine.export_to_svg());
  const drawn = JSON.parse(mengine.get_snapshot()).scene.nodes[node].primitive.h;
  assert.ok(drawn > 0, 'the spring has moved off its anchor');
  assert.ok(
    envelope.warnings.some((w) => w.includes('animated')),
    'the export names the sample as a sample: ' + JSON.stringify(envelope.warnings),
  );
  // The file writes six decimals; the snapshot writes f64. Same number.
  const drawnText = Number(drawn).toFixed(6).replace(/0+$/, '').replace(/\.$/, '');
  assert.ok(
    envelope.code.includes(`height="${drawnText}"`),
    `the file carries the drawn number (${drawnText})`,
  );
  console.log('smoke[38/75]: the export reads the live scene and says when a number is a sample');
}

// ── Task 9.0: the AI command layer ──────────────────────────────────────
//
// The whole point of this task is that an LLM never draws. It emits the same
// JSON a button emits, grounded on a summary of the *live* document, and every
// command it emits goes through the same validation a click does. These steps
// drive that boundary exactly as the React panel does — no API key, no network:
// the planner is local, the validation is the engine's.

// 40. RULE 2: the engine publishes the document summary, and the system prompt
//     is that summary plus the API contract — the hole is gone, and nothing in
//     the prompt is invented by the UI.
{
  const aengine = new glue.VectraEngine();
  const asend = (cmd) => JSON.parse(aengine.dispatch_command(JSON.stringify(cmd)));
  const card = 'dddddddd-0000-4000-8000-000000000040';
  const dot = 'dddddddd-0000-4000-8000-000000000041';
  asend({ type: 'SetVariable', name: 'base', value: 40.0 });
  assert.equal(asend({
    type: 'CreateNode', id: card, name: 'card',
    kind: { Rectangle: { x: { Literal: 0 }, y: { Literal: 0 },
      width: { Literal: 80 }, height: { Literal: 60 }, corner_radius: { Literal: 0 } } },
  }).status, 'ok');
  assert.equal(asend({
    type: 'CreateNode', id: dot, name: 'dot',
    kind: { Circle: { cx: { Literal: 10 }, cy: { Literal: 10 }, radius: { Literal: 5 } } },
  }).status, 'ok');
  // A parametric slot: the summary must show the SOURCE, not the number.
  const exprId = 'dddddddd-0000-4000-8000-0000000000e0';
  assert.equal(asend({ type: 'DefineExpression', id: exprId, source: '$base * 2' }).status, 'ok');
  assert.equal(asend({
    type: 'SetParameter', node_id: card, property: 'width',
    value: { Float: { Expression: exprId } },
  }).status, 'ok');

  const summary = JSON.parse(aengine.document_summary());
  assert.deepEqual(summary.nodes.map((n) => n.label), ["Rectangle 'card'", "Circle 'dot'"]);
  assert.equal(summary.nodes[0].id, card, 'the exact id, unabbreviated');
  assert.deepEqual(summary.variables.map((v) => `${v.name}=${v.value}`), ['base=40']);
  const width = summary.nodes[0].slots.find((s) => s.property === 'width');
  assert.equal(width.source, '$base * 2', 'the source stays parametric in the summary');
  assert.equal(width.value, 80, 'and the current value is reported beside it');
  const height = summary.nodes[0].slots.find((s) => s.property === 'height');
  assert.equal(height.source, '60');

  const prompt = aengine.ai_prompt();
  assert.ok(prompt.includes("Rectangle 'card'"), 'the prompt carries the node list');
  assert.ok(prompt.includes('id=' + card), 'and the exact ids the model must use');
  assert.ok(prompt.includes('$base = 40'), 'and the variables');
  assert.ok(prompt.includes('THE COMMAND API'), 'and the command contract');
  assert.ok(!prompt.includes('{{DOCUMENT}}'), 'the hole was filled');
  const phrasings = JSON.parse(aengine.ai_phrasings());
  assert.ok(phrasings.length > 5 && phrasings.every((line) => typeof line === 'string'),
    'the hint line is engine data: ' + JSON.stringify(phrasings));
  console.log('smoke[39/75]: the engine publishes summary + system prompt + planner hints (RULE 2)');
}

// 41. RULE 1 + the preview path: the AI answers in COMMANDS, resolved against
//     the summary's ids, and a preview applies nothing at all.
{
  const aengine = new glue.VectraEngine();
  const asend = (cmd) => JSON.parse(aengine.dispatch_command(JSON.stringify(cmd)));
  const card = 'dddddddd-0000-4000-8000-000000000050';
  asend({ type: 'SetVariable', name: 'base', value: 40.0 });
  assert.equal(asend({
    type: 'CreateNode', id: card, name: 'card',
    kind: { Rectangle: { x: { Literal: 0 }, y: { Literal: 0 },
      width: { Literal: 80 }, height: { Literal: 60 }, corner_radius: { Literal: 0 } } },
  }).status, 'ok');
  const before = aengine.get_snapshot();
  const depthBefore = JSON.parse(before).undo_depth;

  // An id-free, parametric phrasing: the planner must mint an expression and
  // point the slot at it — with a real uuid, never a `$new:` placeholder.
  const preview = JSON.parse(
    aengine.ai_generate_commands('make the width of card twice $base', ''),
  );
  assert.equal(preview.status, 'ok', JSON.stringify(preview));
  assert.equal(preview.applies, false, 'a preview is not a run');
  assert.equal(preview.attempt, 1);
  assert.equal(preview.plan.length, 2, JSON.stringify(preview.plan));
  assert.equal(preview.plan[0].type, 'DefineExpression');
  // `twice $base` is canonicalized to `2 * $base` — a *parametric expression*,
  // never the number 80 that it happens to evaluate to.
  assert.equal(preview.plan[0].source, '2 * $base', 'the expression is kept as an expression');
  assert.match(
    preview.plan[0].id,
    /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/,
    'a `$new:` placeholder came back as a real id: ' + preview.plan[0].id,
  );
  assert.equal(preview.plan[1].type, 'SetParameter');
  assert.equal(preview.plan[1].node_id, card, 'the Context Law: the id is the summary’s');
  assert.equal(preview.plan[1].property, 'width');
  assert.equal(
    preview.plan[1].value.Float.Expression,
    preview.plan[0].id,
    'the slot binds the expression this very plan defines',
  );

  // Nothing applied: same document, same history depth, nothing to undo.
  assert.equal(aengine.get_snapshot(), before, 'a preview changed the document');
  assert.equal(JSON.parse(aengine.get_snapshot()).undo_depth, depthBefore);
  // The top of the history is still the *creation*: one undo steps over it, not
  // over anything the preview did.
  assert.equal(JSON.parse(aengine.undo()).status, 'ok');
  assert.equal(
    JSON.parse(aengine.get_snapshot()).scene.nodes[card],
    undefined,
    'the preview pushed a history entry',
  );
  console.log('smoke[40/75]: a preview plans resolved commands and applies nothing');
}

// 42. The approved plan runs through `dispatch_command` — the SAME path a click
//     takes — and reports what it cost. One undo reverts the whole plan.
{
  const aengine = new glue.VectraEngine();
  const asend = (cmd) => JSON.parse(aengine.dispatch_command(JSON.stringify(cmd)));
  const card = 'dddddddd-0000-4000-8000-000000000060';
  asend({ type: 'SetVariable', name: 'base', value: 40.0 });
  assert.equal(asend({
    type: 'CreateNode', id: card, name: 'card',
    kind: { Rectangle: { x: { Literal: 0 }, y: { Literal: 0 },
      width: { Literal: 80 }, height: { Literal: 60 }, corner_radius: { Literal: 0 } } },
  }).status, 'ok');
  const depthBefore = JSON.parse(aengine.get_snapshot()).undo_depth;

  const preview = JSON.parse(
    aengine.ai_generate_commands('make the width of card twice $base', ''),
  );
  const receipt = JSON.parse(
    aengine.ai_execute_commands('make the width of card twice $base', JSON.stringify(preview.plan)),
  );
  assert.equal(receipt.status, 'ok', JSON.stringify(receipt));
  assert.equal(receipt.report.attempts, 1);
  assert.equal(receipt.report.plan.length, 2, 'the plan that ran is reported back');
  assert.match(receipt.headline, /2 command\(s\)/, receipt.headline);
  assert.deepEqual(receipt.report.dirty, [card], 'the engine re-evaluated exactly the card');
  assert.ok(
    receipt.report.events.some((e) => e.type === 'NodesUpdated' || e.type === 'Dirty'),
    'the events are the engine’s own: ' + JSON.stringify(receipt.report.events),
  );

  // The slot is genuinely parametric now: 80 through the expression.
  assert.equal(JSON.parse(aengine.get_snapshot()).scene.nodes[card].primitive.w, 80, 'base(40) * 2');

  // Undo behaves exactly as it does for the UI's own multi-command actions: one
  // entry per command, newest first — first the rebind, then the definition.
  // Nothing here is AI-specific, which is the point of RULE 1.
  assert.equal(JSON.parse(aengine.get_snapshot()).undo_depth, depthBefore + 2);
  assert.equal(JSON.parse(aengine.undo()).status, 'ok');
  assert.equal(
    JSON.parse(aengine.get_snapshot()).scene.nodes[card].primitive.w,
    80,
    'the rebind came off first, and the slot fell back to its literal',
  );
  assert.equal(
    Object.keys(JSON.parse(aengine.get_snapshot()).expressions).length,
    1,
    'the definition is still there',
  );
  assert.equal(JSON.parse(aengine.undo()).status, 'ok');
  assert.equal(JSON.parse(aengine.get_snapshot()).undo_depth, depthBefore);
  assert.equal(
    Object.keys(JSON.parse(aengine.get_snapshot()).expressions).length,
    0,
    'the minted expression is undone too — the AI left nothing behind',
  );

  // Redo puts the whole plan back, expression and binding together.
  assert.equal(JSON.parse(aengine.redo()).status, 'ok');
  assert.equal(JSON.parse(aengine.redo()).status, 'ok');
  assert.equal(Object.keys(JSON.parse(aengine.get_snapshot()).expressions).length, 1);
  assert.equal(JSON.parse(aengine.get_snapshot()).scene.nodes[card].primitive.w, 80);

  // And the expression the AI authored is a live dependency like any other:
  // editing the variable moves the slot, without the node being touched.
  const moved = asend({ type: 'SetVariable', name: 'base', value: 100.0 });
  const dirty = moved.events.find((e) => e.type === 'Dirty');
  assert.deepEqual(dirty.ids, [card], 'the AI-authored expression is a live dependency');
  assert.equal(JSON.parse(aengine.get_snapshot()).scene.nodes[card].primitive.w, 200);
  console.log('smoke[41/75]: the approved plan ran through dispatch, undid and redid step by step');
}

// 43. RULE 3: the self-correction loop. A command the engine refuses is fed
//     back, the planner re-plans, and attempt 2 lands — with the engine's own
//     words in the receipt.
{
  const aengine = new glue.VectraEngine();
  const asend = (cmd) => JSON.parse(aengine.dispatch_command(JSON.stringify(cmd)));
  const dot = 'dddddddd-0000-4000-8000-000000000070';
  assert.equal(asend({
    type: 'CreateNode', id: dot, name: 'dot',
    kind: { Circle: { cx: { Literal: 40 }, cy: { Literal: 20 }, radius: { Literal: 15 } } },
  }).status, 'ok');

  // A circle has `radius`, not `corner_radius`: the first plan is wrong on
  // purpose for anything that reasons about shapes rather than slots.
  const receipt = JSON.parse(aengine.ai_execute_with_retry('round the corners of dot by 8', ''));
  assert.equal(receipt.status, 'ok', JSON.stringify(receipt));
  assert.equal(receipt.report.attempts, 2, 'one correction was needed');
  assert.equal(receipt.corrections, 1);
  const correction = receipt.report.corrections[0];
  assert.equal(correction.code, 'engine-rejected');
  assert.match(correction.error, /unknown property 'corner_radius'/, correction.error);
  assert.equal(correction.attempt, 1);
  assert.match(correction.note, /nothing was applied/, correction.note);
  assert.ok(
    receipt.report.notes.some((note) => note.includes('radius')),
    'the planner says what it changed: ' + JSON.stringify(receipt.report.notes),
  );
  assert.equal(
    JSON.parse(aengine.get_snapshot()).scene.nodes[dot].primitive.r,
    8,
    'the corrected command is what landed',
  );
  assert.equal(
    JSON.parse(aengine.get_snapshot()).undo_depth,
    2,
    'one entry for the creation, one for the accepted attempt — a refused plan leaves no trace',
  );
  console.log('smoke[42/75]: the engine’s refusal became a correction and attempt 2 landed');
}

// 44. Graceful failure: an incomprehensible prompt and a hallucinated id both
//     come back typed, and neither touches the document.
{
  const aengine = new glue.VectraEngine();
  const asend = (cmd) => JSON.parse(aengine.dispatch_command(JSON.stringify(cmd)));
  const card = 'dddddddd-0000-4000-8000-000000000080';
  assert.equal(asend({
    type: 'CreateNode', id: card, name: 'card',
    kind: { Rectangle: { x: { Literal: 0 }, y: { Literal: 0 },
      width: { Literal: 80 }, height: { Literal: 60 }, corner_radius: { Literal: 0 } } },
  }).status, 'ok');
  const before = aengine.get_snapshot();

  const nonsense = JSON.parse(aengine.ai_execute_with_retry('reticulate the splines', ''));
  assert.equal(nonsense.status, 'error');
  assert.equal(nonsense.code, 'unrecognized-prompt', 'typed, not a panic: ' + nonsense.message);
  assert.ok(nonsense.message.includes('name a shape'), 'the message says what it does understand');

  const hallucinated = JSON.parse(aengine.ai_execute_commands(
    'delete the purple one',
    JSON.stringify([{ type: 'DeleteNode', id: '99999999-9999-4999-8999-999999999999' }]),
  ));
  assert.equal(hallucinated.status, 'error');
  assert.ok(
    ['engine-rejected', 'unknown-node-id'].includes(hallucinated.code),
    'the invented id is refused by name: ' + hallucinated.code,
  );
  assert.ok(hallucinated.message.includes('99999999'), 'and the message names it');

  // A malformed plan is refused before anything is deserialized.
  const broken = JSON.parse(aengine.ai_execute_commands('whatever', '{"not":"an array"}'));
  assert.equal(broken.status, 'error');
  assert.equal(broken.code, 'invalid-json');

  assert.equal(aengine.get_snapshot(), before, 'nothing above changed the document');
  console.log('smoke[43/75]: refusal is typed, explained, and leaves no trace');
}

// 45. Task 10.0's save path: the engine serializes its **own** document.
//
//     The desktop host asks for exactly this string and gzips it into a
//     `.vectra` file, so what has to hold is that the JSON is the engine's own
//     state — the same nodes, in draw order, with the parametric bindings still
//     bindings — and not a projection some layer assembled. The other half of
//     the law (a fresh engine rebuilding it from the plan) is native, in
//     `crates/vectra-file/tests/roundtrip_laws.rs` and in the desktop host's own
//     `verify` module, because it needs a document-to-commands plan that the
//     window never sees.
{
  const aengine = new glue.VectraEngine();
  const asend = (cmd) => JSON.parse(aengine.dispatch_command(JSON.stringify(cmd)));
  assert.equal(asend({ type: 'SetVariable', name: 'base', value: 40 }).status, 'ok');
  const expr = 'eeeeeeee-0000-4000-8000-000000000001';
  assert.equal(asend({ type: 'DefineExpression', id: expr, source: '$base * 2' }).status, 'ok');
  assert.equal(asend({
    type: 'CreateNode', id: 'dddddddd-0000-4000-8000-0000000000ca', name: 'card',
    kind: { Rectangle: { x: { Literal: 0 }, y: { Literal: 0 },
      width: { Expression: expr }, height: { Literal: 60 }, corner_radius: { Literal: 8 } } },
  }).status, 'ok');

  const document = JSON.parse(aengine.document_json());
  assert.equal(document.version, 1, 'the document carries the engine version');
  assert.equal(document.order.length, 1, 'the draw order is in the document');
  const node = document.nodes[document.order[0]];
  assert.equal(node.name, 'card');
  assert.equal(node.kind.Rectangle.width.Expression, expr, 'the param binding survived as a binding');
  assert.equal(document.expressions[expr].source, '$base * 2');
  assert.deepEqual(document.variables, { base: 40 });

  // …and the runtime count agrees: the summary the AI sees describes the same
  // document the serializer just wrote.
  const summary = JSON.parse(aengine.document_summary());
  assert.equal(summary.nodes.length, document.order.length, 'serializer and summary agree');
  assert.equal(summary.nodes[0].name, 'card');
  assert.equal(summary.nodes[0].id, document.order[0]);
  assert.ok(
    summary.nodes[0].slots.some((slot) => slot.source === '$base * 2'),
    'the parametric source is visible to both surfaces',
  );
  console.log('smoke[44/75]: the engine serializes its own document (the .vectra save path)');
}


// ── Task 10.1: the drawing suite, end to end through the string boundary ────
//
// Everything the four rules promise, driven against the REAL wasm module with
// nothing but JSON — the same door React uses. The pen, the brush, the Quick
// Shape snap and the white arrow are all `draw_*` calls here, and the assertions
// are about the *document* afterwards: what a designer would see if they clicked
// the same way.
{
  // 45. **The pen's gestures** (RULE 2). A click places a corner, a drag pulls
  //     mirrored Bézier handles out of the anchor being placed, Alt-drag breaks
  //     that mirror, and the reply carries the draft the engine actually holds —
  //     the UI never computes a control point.
  {
    const pengine = new glue.VectraEngine();
    const pd = (kind, x, y, alt = false) =>
      JSON.parse(pengine.draw_pointer('pen', kind, x, y, alt, true, 0));

    pd('down', 10, 10);
    pd('up', 10, 10);
    const corner = pd('down', 60, 10);
    assert.equal(corner.ok, true, JSON.stringify(corner));
    assert.equal(corner.draft.anchors.length, 2, 'two anchors placed');
    assert.equal(corner.draft.kinds[0], 'line', 'click ⇒ a corner (LineTo)');

    // …and a drag on the second anchor: smooth, mirrored handles. The drag
    // vector is (30, 0) from the anchor at (60, 10).
    const drag = pd('move', 90, 10);
    assert.equal(drag.draft.kinds[0], 'cubic', 'drag ⇒ a cubic');
    const smooth = drag.draft.anchors[1];
    assert.deepEqual(smooth.handle_in, [30, 10], 'the handle arriving at the anchor');
    assert.deepEqual(smooth.handle_out, [90, 10], 'and the one leaving it — mirrored');
    assert.equal(
      smooth.handle_out[0] - smooth.x,
      smooth.x - smooth.handle_in[0],
      'symmetry: the two handles reflect each other through the point',
    );
    assert.equal(smooth.handle_out[1] - smooth.y, smooth.y - smooth.handle_in[1]);

    // Alt-drag **breaks** the mirror (RULE 2): the dragged handle moves, the
    // other stays where it was. Nothing else in the tool creates a broken anchor.
    const altDrag = pd('move', 100, 40, true);
    const broken = altDrag.draft.anchors[1];
    assert.deepEqual(broken.handle_out, [100, 40], 'the dragged handle followed the pointer');
    assert.deepEqual(broken.handle_in, [30, 10], 'and the partner did not move');
    pd('up', 100, 40);

    // Two more plain clicks, to show what the handles *do* to the segments that
    // touch them: the anchor a drag left handles on curves both of its segments,
    // while two pure corners give a straight line.
    pd('down', 140, 10);
    pd('up', 140, 10);
    pd('down', 180, 10);
    pd('up', 180, 10);

    const committed = JSON.parse(pengine.draw_pen_commit(false, null));
    assert.equal(committed.ok, true, JSON.stringify(committed));
    assert.equal(committed.segments, 3, 'four anchors, three segments');
    const committedPath = JSON.parse(pengine.document_json()).nodes[committed.node_id].kind.Path;
    const committedKinds = committedPath.segments.map(segmentKind);
    assert.deepEqual(committedKinds, ['Cubic', 'Cubic', 'Line'], 'handles curve; two corners do not');
    assert.deepEqual(
      committedPath.segments[0].Cubic.control1.Literal,
      { x: 10, y: 10 },
      'the segment leaves the corner at the corner itself',
    );
    assert.deepEqual(
      committedPath.segments[0].Cubic.control2.Literal,
      { x: 30, y: 10 },
      'and arrives along the mirrored handle (Anchors 2 → 3 are stored as control points)',
    );
    assert.deepEqual(
      committedPath.segments[1].Cubic.control1.Literal,
      { x: 100, y: 40 },
      'the broken handle curves the segment leaving the anchor, unchanged by Alt-breaking',
    );
    assert.deepEqual(committedPath.segments[2].Line.to.Literal, { x: 180, y: 10 });
    console.log('smoke[45/75]: the pen — click, drag, Alt-drag, and what the handles do (RULE 2)');
  }

  // 46. **Closing and finishing** (RULE 2's last two gestures). Clicking the
  //     first anchor closes the path — the engine's `closes_at` measures the
  //     click, because only the engine knows where the first anchor is — and
  //     Enter finishes without closing.
  {
    const cengine = new glue.VectraEngine();
    const cd = (kind, x, y) =>
      JSON.parse(cengine.draw_pointer('pen', kind, x, y, false, true, 0));
    for (const [x, y] of [[0, 0], [40, 0], [40, 40]]) {
      cd('down', x, y);
      cd('up', x, y);
    }
    // A click on the first point: the path closes instead of stacking a
    // duplicate anchor on top of it.
    const closed = cd('down', 1, 1);
    assert.equal(closed.ok, true, JSON.stringify(closed));
    assert.ok(closed.node_id, 'the closing click committed the path');
    assert.equal(closed.draft, undefined, 'and there is no draft left to draw');
    const closedPath = JSON.parse(cengine.document_json()).nodes[closed.node_id].kind.Path;
    const closedKinds = closedPath.segments.map(segmentKind);
    assert.deepEqual(closedKinds, ['Line', 'Line', 'Close'], 'closed by a Close segment');
    assert.deepEqual(closedPath.segments[0].Line.to.Literal, { x: 40, y: 0 });

    // A path finished instead of closed ends at its last anchor.
    const fengine = new glue.VectraEngine();
    const fd = (kind, x, y) =>
      JSON.parse(fengine.draw_pointer('pen', kind, x, y, false, true, 0));
    fd('down', 0, 0);
    fd('up', 0, 0);
    fd('down', 30, 30);
    fd('up', 30, 30);
    const finished = JSON.parse(fengine.draw_pen_commit(false, null));
    const finishedPath = JSON.parse(fengine.document_json()).nodes[finished.node_id].kind.Path;
    assert.equal(finishedPath.segments.length, 1);
    assert.equal(segmentKind(finishedPath.segments[0]), 'Line');
    console.log('smoke[46/75]: click the first point closes; commit without closing does not');
  }

  // 47. **The brush** (RULE 3, first half). A freehand stroke is captured with
  //     pressure, fitted to a Bézier chain and expanded to a *filled* shape: the
  //     sides are curves, the ring closes, and the segment count is a *fit* of the
  //     samples rather than a replay of them.
  {
    const bengine = new glue.VectraEngine();
    const bd = (engine, kind, point) =>
      JSON.parse(engine.draw_pointer('brush', kind, point[0], point[1], false, false, 0));

    // (a) A nearly straight stroke: the fitter's straight-line path collapses
    //     twenty-one samples into a handful of curves. A polyline replay would
    //     have produced twenty segments — this is RULE 3's "never a jagged
    //     polyline", stated as a number.
    const straight = [];
    for (let i = 0; i <= 20; i += 1) straight.push([i * 4, 10 + Math.sin(i) * 0.2]);
    bd(bengine, 'down', straight[0]);
    for (const point of straight.slice(1)) bd(bengine, 'move', point);
    const straightCommit = JSON.parse(bengine.draw_brush_commit(null));
    assert.equal(straightCommit.ok, true, JSON.stringify(straightCommit));
    assert.ok(
      straightCommit.segments < straight.length,
      `fitted, not replayed: ${straightCommit.segments} segments for ${straight.length} samples`,
    );
    const straightPath = JSON.parse(bengine.document_json()).nodes[straightCommit.node_id].kind.Path;
    const straightKinds = straightPath.segments.map(segmentKind);
    assert.ok(straightKinds.includes('Cubic'), 'the sides of the ring are Bézier curves');
    assert.equal(straightKinds[straightKinds.length - 1], 'Close', 'and the ring closes: it is fillable');

    // (b) A hand-drawn arc, captured sample by sample with its pressure channel.
    const arc = [];
    for (let i = 0; i <= 24; i += 1) {
      const t = (i / 24) * Math.PI;
      arc.push([40 + 60 * Math.cos(t), 40 + 20 * Math.sin(t)]);
    }
    bd(bengine, 'down', arc[0]);
    for (const point of arc.slice(1)) bd(bengine, 'move', point);
    const captured = bd(bengine, 'move', arc[arc.length - 1]);
    assert.ok(captured.samples.length >= 8, 'the stroke was captured, sample by sample');
    assert.equal(captured.samples[0].length, 3, 'every sample carries its pressure channel');

    const arcCommit = JSON.parse(bengine.draw_brush_commit(null));
    assert.ok(arcCommit.fit_error > 0 && arcCommit.fit_error < 1.0,
      `the fit tracks the hand to sub-unit accuracy (${arcCommit.fit_error})`);
    const arcPath = JSON.parse(bengine.document_json()).nodes[arcCommit.node_id].kind.Path;
    const cubics = arcPath.segments.map(segmentKind).filter((kind) => kind === 'Cubic').length;
    assert.ok(cubics >= 6, 'the shape is made of curve segments, not a polygon');
    console.log('smoke[47/75]: the brush — fitted to curves, closed ring, pressure captured');
  }

  // 48. **Quick Shape** (RULE 3, second half, and the heart of the task): a
  //     rough *jagged* closed stroke becomes a perfect circle — and the snap is
  //     performed by the Task 3.1 solver, with the primitive's own parameters
  //     constrained to the path's anchors.
  {
    const qengine = new glue.VectraEngine();
    const qd = (kind, point) =>
      JSON.parse(qengine.draw_pointer('brush', kind, point[0], point[1], false, false, 0));
    const center = { x: 100, y: 80 };
    const radius = 50;
    const rough = [];
    for (let i = 0; i < 36; i += 1) {
      const t = (i / 36) * Math.PI * 2;
      // A deliberately bad circle: a periodic ±18 % wobble (periodic so the hand
      // comes back near where it started, which is what a drawn loop does).
      const wobble = 1 + 0.18 * Math.sin(5 * t);
      rough.push([
        center.x + radius * wobble * Math.cos(t),
        center.y + radius * wobble * Math.sin(t),
      ]);
    }
    qd('down', rough[0]);
    for (const point of rough.slice(1)) qd('move', point);
    const qcommitted = JSON.parse(qengine.draw_brush_commit(null));
    assert.equal(qcommitted.ok, true);

    const snapped = JSON.parse(
      qengine.draw_quick_shape(qcommitted.node_id, JSON.stringify(rough)),
    );
    assert.equal(snapped.ok, true, JSON.stringify(snapped));
    assert.equal(snapped.snapped, 'circle', 'the rough stroke was recognised as a circle');
    assert.ok(snapped.snap_error > 0, 'and the hand’s own error is reported, not hidden');

    const document = JSON.parse(qengine.document_json());
    const path = document.nodes[qcommitted.node_id].kind.Path;
    // Four cubic arcs and no `Close`: the fourth arc ends exactly on the start
    // point, so the path closes by geometry — four vertices, not five.
    assert.equal(path.segments.length, 4);
    assert.deepEqual(path.segments.map(segmentKind), ['Cubic', 'Cubic', 'Cubic', 'Cubic']);

    // The primitive is a REAL node, with its own id, named after the snap.
    const primitive = document.order
      .map((id) => document.nodes[id])
      .find((node) => node.kind && node.kind.Circle);
    assert.ok(primitive, 'the snap created a Circle node');
    assert.match(primitive.name, /snapped circle/);
    const cx = primitive.kind.Circle.cx.Literal;
    const cy = primitive.kind.Circle.cy.Literal;
    const r = primitive.kind.Circle.radius.Literal;
    assert.ok(Math.abs(cx - center.x) < 12 && Math.abs(cy - center.y) < 12, 'near the hand’s centre');
    assert.ok(Math.abs(r - radius) < 12, 'and near its radius');

    // …and the rows that hold the path to it are in the document: eleven naming
    // the primitive (primitive slot FIRST in each — the ordering that makes the
    // path follow the circle rather than the other way round) plus the two that
    // hold the seam shut.
    const rows = document.constraints.filter((constraint) =>
      constraint.targets.some((target) => target.node_id === primitive.id),
    );
    assert.equal(rows.length, 11, '3 pins + 4 shared axes + 4 offsets');
    for (const row of rows) {
      assert.equal(row.targets[0].node_id, primitive.id, 'the primitive comes first');
    }
    const seam = document.constraints.filter(
      (constraint) =>
        constraint.targets.length === 2 &&
        constraint.targets.every((target) => target.node_id === qcommitted.node_id),
    );
    assert.equal(seam.length, 2, 'the closure is an invariant, not a coincidence of numbers');

    // The anchors are the cardinals of the fitted circle, to the last decimal.
    const overlay = JSON.parse(qengine.draw_overlay(qcommitted.node_id));
    const cardinals = [
      [cx + r, cy],
      [cx, cy + r],
      [cx - r, cy],
      [cx, cy - r],
    ];
    assert.equal(overlay.anchors.length, 5, 'four positions in five slots (the seam is the fifth)');
    overlay.anchors.forEach((anchor, index) => {
      const expected = index < 4 ? cardinals[index] : cardinals[0];
      assert.ok(
        Math.abs(anchor.x - expected[0]) < 1e-9 && Math.abs(anchor.y - expected[1]) < 1e-9,
        `anchor ${index} is a cardinal: ${anchor.x},${anchor.y} vs ${expected}`,
      );
    });
    console.log('smoke[48/75]: Quick Shape — a jagged stroke became a constrained perfect circle');
  }

  // 49. **Direct selection** (RULE 4). Hit-testing is spatial — anchors win over
  //     handles over the path — the drag is ONE undo step, a handle drag mirrors
  //     its partner, and an Alt-drag does not.
  {
    const eengine = new glue.VectraEngine();
    const ed = (kind, x, y) =>
      JSON.parse(eengine.draw_pointer('pen', kind, x, y, false, false, 0));
    // A smooth middle anchor (a drag), so there is a handle to grab.
    ed('down', 0, 0);
    ed('up', 0, 0);
    ed('down', 50, 0);
    ed('move', 80, 20);
    ed('up', 80, 20);
    ed('down', 100, 0);
    ed('up', 100, 0);
    const pathId = JSON.parse(eengine.draw_pen_commit(false, null)).node_id;

    // A camera, because the grab radius is expressed in *pixels* ÷ the camera's
    // scale — a handle must be as easy to hit at 4× zoom as at 1×.
    const renderer = new glue.Renderer();
    renderer.set_viewport(0, 0, 800, 600, 1);
    // The pointer travels in *client* pixels and the engine converts them with
    // the renderer's own camera — that is the whole of RULE 4's "the handle is
    // where it looks": one camera serves the drawing, the pointer and the overlay.
    const toClient = (x, y) =>
      JSON.parse(renderer.document_to_client(JSON.stringify([[x, y]]))).points[0];
    const hitAt = (x, y) => {
      const [cx, cy] = toClient(x, y);
      return JSON.parse(eengine.draw_hit(renderer, pathId, cx, cy));
    };
    const onAnchor = hitAt(50, 0);
    assert.equal(onAnchor.kind, 'anchor', JSON.stringify(onAnchor));
    assert.equal(onAnchor.slot, 'segments[0].to');
    assert.equal(hitAt(50, 400).kind, 'miss', 'empty space is a miss, not a guess');

    // The overlay reports both handles of the smooth vertex, in absolute
    // coordinates — which is what lets the UI reveal them on hover.
    const before = JSON.parse(eengine.draw_overlay(pathId));
    const smoothIndex = before.anchors.findIndex(
      (anchor) => anchor.handle_in !== null && anchor.handle_out !== null,
    );
    assert.ok(smoothIndex > 0, 'the dragged anchor kept both handles');
    const smooth = before.anchors[smoothIndex];

    // **A handle drag**: the partner mirrors through the anchor, and the whole
    // edit is one `Batch` — one undoable command.
    // `draw_edit_command` answers with a *command* (a `Batch` of component
    // writes), not with a result: the UI puts it through the same
    // `dispatch_command` door as every other edit, so the drag is recorded,
    // gated and reported exactly like a hand-authored change.
    const edit = JSON.parse(
      eengine.draw_edit_command(pathId, smooth.slot, 'out', smooth.handle_out[0], 60, false, false),
    );
    assert.equal(edit.type, 'Batch', JSON.stringify(edit));
    assert.equal(edit.commands.length, 4, 'the dragged handle and its mirror, x and y');
    assert.equal(
      JSON.parse(eengine.dispatch_command(JSON.stringify(edit))).status,
      'ok',
      'and it applies through the ordinary door',
    );
    const afterDrag = JSON.parse(eengine.draw_overlay(pathId));
    const moved = afterDrag.anchors[smoothIndex];
    assert.deepEqual(moved.handle_out, [smooth.handle_out[0], 60], 'the dragged handle moved');
    assert.ok(mirroredPoint(smooth, moved), 'and the partner mirrored it through the anchor');

    // **An Alt-drag breaks the mirror**: only the dragged handle moves.
    const altEdit = JSON.parse(
      eengine.draw_edit_command(pathId, smooth.slot, 'out', moved.handle_out[0], 90, true, false),
    );
    assert.equal(altEdit.commands.length, 2, 'Alt: the dragged handle alone — x and y');
    assert.equal(JSON.parse(eengine.dispatch_command(JSON.stringify(altEdit))).status, 'ok');
    const altAfter = JSON.parse(eengine.draw_overlay(pathId)).anchors[smoothIndex];
    assert.deepEqual(altAfter.handle_in, moved.handle_in, 'Alt held the partner still');
    assert.equal(altAfter.handle_out[1], 90, 'while the dragged handle followed the pointer');

    // **An anchor drag is one history entry, not one per sample.**
    const depthBefore = undoDepthOf(eengine);
    assert.equal(
      JSON.parse(eengine.draw_edit_begin(pathId, 'segments[0].to', '', true)).status,
      'ok',
      'the anchor edit opens',
    );
    for (let i = 1; i <= 12; i += 1) {
      const moved2 = JSON.parse(eengine.draw_edit_update(50 + i, i, false));
      assert.equal(moved2.status, 'ok', JSON.stringify(moved2));
    }
    assert.equal(JSON.parse(eengine.draw_edit_end()).status, 'ok');
    const dragged = JSON.parse(eengine.draw_overlay(pathId));
    assert.equal(dragged.anchors[1].x, 62, 'the anchor followed the pointer');
    assert.equal(dragged.anchors[1].y, 12);
    assert.equal(undoDepthOf(eengine), depthBefore + 1, 'twelve samples, ONE history entry');

    // …and one undo restores the pre-drag geometry exactly.
    assert.equal(JSON.parse(eengine.undo()).status, 'ok');
    const undone = JSON.parse(eengine.draw_overlay(pathId));
    assert.equal(undone.anchors[1].x, 50, 'one undo restored the shape');
    assert.equal(undone.anchors[1].y, 0);
    console.log('smoke[49/75]: direct selection — hit test, solo point, mirror, Alt breaks it, 1 undo');
  }

  // 50. **The overlay's camera** (RULE 4's "show the handles where they are").
  //     Document space is y-up and the DOM is y-down: the mapping is the
  //     renderer's, so a handle cannot be drawn on the wrong pixel — and a canvas
  //     with no box answers honestly instead of inventing one.
  {
    const renderer = new glue.Renderer();
    assert.equal(JSON.parse(renderer.document_to_client('[[0,0]]')).ok, false, 'no box, no pixels');
    renderer.set_viewport(0, 0, 800, 600, 2);
    const mapped = JSON.parse(renderer.document_to_client('[[0,0],[400,300]]'));
    assert.equal(mapped.ok, true);
    assert.equal(mapped.points.length, 2);
    const [a, b] = mapped.points;
    assert.ok(a[1] > b[1], 'document up is screen up: y is flipped exactly once');
    // The inverse is the renderer's own pointer mapping — the very call React
    // makes on every pointer event, so the round trip here is the one a drag
    // actually travels.
    const back = JSON.parse(renderer.pointer_doc(a[0], a[1]));
    assert.ok(Math.abs(back.x) < 1e-3 && Math.abs(back.y) < 1e-3, 'the round trip closes');
    // A canvas with no box has no camera, and the honest answer is `null` rather
    // than a document point invented from an unmeasured window.
    assert.equal(new glue.Renderer().pointer_doc(10, 10), 'null', 'no box ⇒ no coordinate guess');
    console.log('smoke[50/75]: the overlay is placed by the engine’s camera (document y-up ↔ DOM y-down)');
  }

  // 51. **The workspace a new document opens into** (Task 10.2 RULES 1-2).
  //     One board, one layer, and the first shape the designer draws lands on
  //     it — the engine's own answer, not a UI convention.
  {
    const engine = new glue.VectraEngine();
    const snap = JSON.parse(engine.get_snapshot());
    assert.equal(snap.layers.length, 1, 'a new document has a layer to draw on');
    assert.equal(snap.layers[0].name, 'Layer 1');
    assert.equal(snap.artboards.length, 1, 'and a board to draw on it');
    assert.deepEqual(snap.artboards[0].bounds, [0, 0, 800, 600], 'framed like the opening camera');
    assert.equal(snap.active_layer, snap.layers[0].id, 'and it is the active one');
    assert.equal(snap.active_artboard, snap.artboards[0].id);
    console.log('smoke[51/75]: a new document opens on one artboard with one layer');
  }

  // 52. **RULE 4, driven through the JSON the React remote sends.** Hiding a
  //     layer is not an evaluation: the engine reports a no-op, the renderer
  //     writes zero bytes, and the picture loses exactly those draw items. The
  //     eye is the one toggle in the product that costs nothing.
  {
    const engine = new glue.VectraEngine();
    const send = (cmd) => JSON.parse(engine.dispatch_command(JSON.stringify(cmd)));
    const renderer = new glue.Renderer();
    renderer.set_viewport(0, 0, 800, 600, 1);
    const frame = () => JSON.parse(engine.render_frame(renderer));
    const rect = (name, x) => {
      const id = crypto.randomUUID();
      send({
        type: 'CreateNode', id, name,
        kind: { Rectangle: {
          x: { Literal: x }, y: { Literal: 0 },
          width: { Literal: 40 }, height: { Literal: 40 },
          corner_radius: { Literal: 0 },
        }},
      });
      return id;
    };
    rect('a', 0);
    rect('b', 60);
    const guides = crypto.randomUUID();
    send({ type: 'CreateLayer', id: guides, name: 'Guides' });
    send({ type: 'SetActiveLayer', id: guides });
    const c = rect('c', 0);
    rect('d', 60);

    const before = frame();
    assert.equal(before.draw_calls, 4, `one item per node: ${JSON.stringify(before)}`);

    const hidden = send({ type: 'SetLayerVisible', id: guides, visible: false });
    assert.equal(hidden.status, 'ok');
    assert.ok(hidden.events.some((e) => e.type === 'LayersUpdated'), 'the panel hears about it');
    const dirty = hidden.events.find((e) => e.type === 'Dirty');
    assert.deepEqual(dirty.ids, [], 'an eye dirties nothing — not one slot');
    assert.equal(dirty.mode, 'incremental');
    const after = frame();
    assert.equal(after.writes, 0, `zero bytes written: ${JSON.stringify(after)}`);
    assert.equal(after.bytes, 0, 'and zero bytes of traffic');
    assert.equal(after.nodes, 4, 'nothing is released: the shapes are still resident');
    assert.equal(after.draw_calls, 2, 'the GPU simply draws fewer items');
    // The document still holds everything, and the snapshot says so.
    const snap = JSON.parse(engine.get_snapshot());
    assert.equal(snap.scene.nodes[c].visible, false, 'the layer eye reaches the node');
    assert.equal(snap.scene.nodes[c].own_visible, true, 'without editing the node own eye');
    assert.equal(
      snap.layers.find((l) => l.id === guides).children.length,
      2,
      'the layer still lists its contents',
    );
    // A padlock is the same kind of flag: it draws, and costs nothing.
    send({ type: 'SetLayerLocked', id: guides, locked: true });
    const locked = frame();
    assert.equal(locked.writes, 0);
    assert.equal(JSON.parse(engine.get_snapshot()).scene.nodes[c].locked, true, 'locked, not hidden');
    console.log('smoke[52/75]: RULE 4 across the wire — the eye works and writes nothing');
  }

  // 53. **RULE 3: the brief's own example**, end to end. A thick black stroke
  //     with a thinner white stroke on top is *two* strokes drawn in order, and
  //     a blend mode on one of them costs one instance row.
  {
    const engine = new glue.VectraEngine();
    const send = (cmd) => JSON.parse(engine.dispatch_command(JSON.stringify(cmd)));
    const renderer = new glue.Renderer();
    renderer.set_viewport(0, 0, 800, 600, 1);
    const node = crypto.randomUUID();
    send({
      type: 'CreateNode', id: node, name: 'Mark',
      kind: { Rectangle: {
        x: { Literal: 0 }, y: { Literal: 0 },
        width: { Literal: 100 }, height: { Literal: 60 },
        corner_radius: { Literal: 0 },
      }},
    });
    const fill = (r, g, b, blend) => ({
      kind: 'Fill',
      paint: { Solid: { Literal: { r, g, b, a: 255 } } },
      opacity: { Literal: 1 }, blend, visible: true,
    });
    const stroke = (r, g, b, width, blend) => ({
      kind: { Stroke: { width: { Literal: width } } },
      paint: { Solid: { Literal: { r, g, b, a: 255 } } },
      opacity: { Literal: 1 }, blend, visible: true,
    });
    const stack = (blend) => [
      fill(34, 102, 238, 'Normal'),
      stroke(0, 0, 0, 6, 'Normal'),
      stroke(255, 255, 255, 2, blend),
    ];
    const stacked = send({ type: 'SetAppearances', node_id: node, appearances: stack('Normal') });
    assert.equal(stacked.status, 'ok', JSON.stringify(stacked));
    let frame = JSON.parse(engine.render_frame(renderer));
    assert.equal(
      frame.draw_calls,
      3,
      `three appearance layers, three draws: ${JSON.stringify(frame)}`,
    );

    // The stack reaches the panel in order, with each row's own numbers.
    let snap = JSON.parse(engine.get_snapshot());
    let rows = snap.scene.nodes[node].style.appearances;
    assert.deepEqual(rows.map((row) => row.kind), ['fill', 'stroke', 'stroke']);
    assert.deepEqual(rows.map((row) => row.width), [null, 6, 2], 'thick under thin');

    // Re-blending the top stroke: a style change on one row of one node.
    const blended = send({ type: 'SetAppearances', node_id: node, appearances: stack('Multiply') });
    assert.equal(blended.status, 'ok');
    frame = JSON.parse(engine.render_frame(renderer));
    assert.equal(frame.writes, 1, 'one instance row');
    assert.equal(frame.bytes, 80, 'eighty bytes, no retessellation');
    assert.equal(frame.retessellated, 0, 'the outline did not move');
    snap = JSON.parse(engine.get_snapshot());
    rows = snap.scene.nodes[node].style.appearances;
    assert.equal(rows[2].blend, 'multiply', 'and the mode is on the row it was set on');

    // A gradient: three stops, and the ramp reaches the renderer as its own
    // budget line — separate from the 80-byte instance traffic.
    const gradient = send({
      type: 'SetAppearances', node_id: node,
      appearances: [
        {
          kind: 'Fill',
          paint: { Linear: {
            start: { Literal: { x: 0, y: 0 } },
            end: { Literal: { x: 100, y: 0 } },
            stops: [
              { offset: 0, color: { r: 255, g: 210, b: 0, a: 255 } },
              { offset: 0.5, color: { r: 240, g: 90, b: 20, a: 255 } },
              { offset: 1, color: { r: 120, g: 10, b: 60, a: 255 } },
            ],
          }},
          opacity: { Literal: 1 }, blend: 'Normal', visible: true,
        },
        stroke(0, 0, 0, 6, 'Normal'),
        stroke(255, 255, 255, 2, 'Multiply'),
      ],
    });
    assert.equal(gradient.status, 'ok', JSON.stringify(gradient));
    frame = JSON.parse(engine.render_frame(renderer));
    assert.ok(frame.ramps > 0, `the ramp was uploaded: ${JSON.stringify(frame)}`);
    assert.equal(frame.draw_calls, 3);
    snap = JSON.parse(engine.get_snapshot());
    const paint = snap.scene.nodes[node].style.appearances[0].paint;
    assert.equal(paint.type, 'linear');
    assert.equal(paint.stops.length, 3, 'all three stops survive the round trip');
    assert.equal(paint.stops[1].offset, 0.5);
    console.log('smoke[53/75]: RULE 3 — stacked strokes, a blend mode, a gradient ramp');
  }

  // 54. **RULE 2: artboards — jump, frame, export.** The dropdown's jump is the
  //     renderer's camera; the two exports are the engine's own exporter.
  {
    const engine = new glue.VectraEngine();
    const send = (cmd) => JSON.parse(engine.dispatch_command(JSON.stringify(cmd)));
    const renderer = new glue.Renderer();
    renderer.set_viewport(0, 0, 800, 600, 1);
    const rect = (name, x, y, w, h) => {
      const id = crypto.randomUUID();
      send({
        type: 'CreateNode', id, name,
        kind: { Rectangle: {
          x: { Literal: x }, y: { Literal: y },
          width: { Literal: w }, height: { Literal: h },
          corner_radius: { Literal: 0 },
        }},
      });
      return id;
    };
    const icon = crypto.randomUUID();
    send({
      type: 'CreateArtboard', id: icon, name: 'Icon 16',
      x: 0, y: 0, width: 16, height: 16,
      background: { r: 255, g: 255, b: 255, a: 255 },
    });
    send({ type: 'CreateLayer', id: crypto.randomUUID(), name: 'Icon ink' });
    const dot = rect('Dot', 2, 2, 12, 12);
    const logo = crypto.randomUUID();
    send({
      type: 'CreateArtboard', id: logo, name: 'Logo',
      x: 400, y: 0, width: 320, height: 200,
      background: { r: 16, g: 24, b: 32, a: 255 },
    });
    send({ type: 'CreateLayer', id: crypto.randomUUID(), name: 'Logo ink' });
    const mark = rect('Mark', 440, 40, 240, 120);

    const snap = JSON.parse(engine.get_snapshot());
    const board = (name) => snap.artboards.find((candidate) => candidate.name === name);
    assert.equal(snap.artboards.length, 3, 'the seeded board plus the two that were created');
    assert.deepEqual(board('Icon 16').bounds, [0, 0, 16, 16]);
    assert.equal(board('Icon 16').background, '#ffffff');
    assert.equal(board('Icon 16').layers, 1, 'the layer created while standing on it');
    assert.equal(board('Logo').layers, 1, 'and each board owns its own stack');
    assert.equal(snap.active_artboard, logo, 'creating a board moves onto it');

    // **The jump.** The camera is the renderer's, so the panel hands over the
    // board's rectangle and gets the visible window back.
    const framed = JSON.parse(renderer.frame_document(400, 0, 320, 200));
    assert.ok(Math.abs(framed.w - 320) < 1, `framed by width: ${JSON.stringify(framed)}`);
    assert.ok(Math.abs(framed.h - 240) < 1, 'a 4:3 canvas over a 320x200 board');
    const close = JSON.parse(renderer.frame_document(0, 0, 16, 16));
    assert.ok(close.w < framed.w, 'framing the small board zooms in');
    const wide = JSON.parse(renderer.frame_document_default());
    assert.ok(wide.w > framed.w, 'and the default view is the whole picture');

    // **Export current**: the active board only, cropped to its frame, with its
    // own background — and not the other board's artwork.
    const current = JSON.parse(engine.export_current_artboard());
    assert.equal(current.status, 'ok');
    assert.ok(current.code.includes(`data-vectra-artboard="${logo}"`), 'the active board group');
    assert.ok(current.code.includes(mark), 'its own node');
    assert.ok(!current.code.includes(dot), 'and not the other board artwork');
    assert.ok(
      current.code.includes('width="320"') && current.code.includes('height="200"'),
      'cropped to the board, not to the ink',
    );
    // **Export all**: both boards, each with its own frame group.
    const all = JSON.parse(engine.export_all_artboards());
    assert.ok(all.code.includes(`data-vectra-artboard="${logo}"`));
    assert.ok(all.code.includes(`data-vectra-artboard="${icon}"`));
    assert.ok(all.code.includes(dot) && all.code.includes(mark), 'both boards artwork');
    console.log('smoke[54/75]: RULE 2 — boards jump, frame, and export current vs all');
  }

  // 55. **The Layers Panel's own contract, in one gesture.** Reordering a layer
  //     moves its nodes — and costs no evaluation, because the *order* changed
  //     and no value did: the scene re-reads its order from the document.
  {
    const engine = new glue.VectraEngine();
    const send = (cmd) => JSON.parse(engine.dispatch_command(JSON.stringify(cmd)));
    const renderer = new glue.Renderer();
    renderer.set_viewport(0, 0, 800, 600, 1);
    const rect = (name) => {
      const id = crypto.randomUUID();
      send({
        type: 'CreateNode', id, name,
        kind: { Rectangle: {
          x: { Literal: 0 }, y: { Literal: 0 },
          width: { Literal: 100 }, height: { Literal: 100 },
          corner_radius: { Literal: 0 },
        }},
      });
      return id;
    };
    const under = rect('under');
    const art = crypto.randomUUID();
    send({ type: 'CreateLayer', id: art, name: 'Art' });
    send({ type: 'SetActiveLayer', id: art });
    const over = rect('over');
    JSON.parse(engine.render_frame(renderer));

    let snap = JSON.parse(engine.get_snapshot());
    assert.deepEqual(snap.layers.map((l) => l.name), ['Layer 1', 'Art'], 'back to front');
    assert.equal(snap.scene.z_order.indexOf(under), 0, 'the first layer draws first');

    // Drag "Art" to the bottom of the panel: the engine's index 0.
    const moved = send({ type: 'ReorderLayer', id: art, index: 0 });
    assert.equal(moved.status, 'ok', JSON.stringify(moved));
    const dirty = moved.events.find((e) => e.type === 'Dirty');
    assert.deepEqual(dirty.ids, [], 'a reorder changes order, not values');
    snap = JSON.parse(engine.get_snapshot());
    assert.deepEqual(snap.layers.map((l) => l.name), ['Art', 'Layer 1']);
    assert.equal(snap.scene.z_order.indexOf(over), 0, 'and its node travels with it');
    assert.equal(snap.scene.z_order.indexOf(under), 1);
    const reordered = JSON.parse(engine.render_frame(renderer));
    assert.equal(reordered.writes, 0, `nothing is re-uploaded: ${JSON.stringify(reordered)}`);
    assert.equal(reordered.draw_calls, 2, 'the same two items, in the other order');
    console.log('smoke[55/75]: RULE 1 — moving a layer moves its artwork, for free');
  }

  // 56. **Pan and zoom (Task 10.3 RULE 2).** The camera gestures every editor
  //     has, driven through the wasm port exactly as the canvas drives them, and
  //     judged by the one thing that must never drift: the pointer's map to the
  //     document. A wheel anchors the point under the cursor, a pan follows the
  //     pointer, the readout matches what a client pixel means, and a click where
  //     the shape is *drawn* still selects it afterwards.
  {
    const engine = new glue.VectraEngine();
    const send = (cmd) => JSON.parse(engine.dispatch_command(JSON.stringify(cmd)));
    const renderer = new glue.Renderer();
    renderer.set_viewport(0, 0, 800, 600, 2);
    const node = crypto.randomUUID();
    send({
      type: 'CreateNode', id: node, name: 'Rig',
      kind: { Rectangle: {
        x: { Literal: 120 }, y: { Literal: 90 },
        width: { Literal: 160 }, height: { Literal: 120 },
        corner_radius: { Literal: 0 },
      }},
    });
    JSON.parse(engine.render_frame(renderer));

    const doc = (x, y) => JSON.parse(renderer.pointer_doc(x, y));
    const before = doc(610, 145);

    // The wheel: five notches at (610, 145), and the document point under the
    // pointer is the same one it was on the first.
    for (let i = 0; i < 5; i += 1) renderer.nav_zoom(610, 145, 1.2);
    const after = doc(610, 145);
    assert.ok(Math.abs(after.x - before.x) < 0.01 && Math.abs(after.y - before.y) < 0.01,
      `the wheel anchored the point under the pointer: ${JSON.stringify(before)} → ${JSON.stringify(after)}`);
    assert.ok(Math.abs(renderer.nav_scale() - 1.2 ** 5) < 1e-3,
      `the readout is the zoom asked for: ${renderer.nav_scale()}`);

    // The pan: 20 samples of a drag, and the grabbed point ends up exactly
    // under the pointer that grabbed it. The y-flip is the interesting half —
    // the DOM's y grows down, the document's up — so the check is on the client
    // position of the anchor, read back through the engine's own transform.
    const grab = doc(400, 300);
    for (let i = 0; i < 20; i += 1) renderer.nav_pan(-1.5, 4.25);
    const where = JSON.parse(renderer.document_to_client(
      JSON.stringify([[grab.x, grab.y]]),
    ));
    assert.ok(where.ok, JSON.stringify(where));
    assert.ok(Math.abs(where.points[0][0] - (400 - 30)) < 0.01,
      `the grabbed point followed the pointer in x: ${where.points[0][0]}`);
    assert.ok(Math.abs(where.points[0][1] - (300 + 85)) < 0.01,
      `…and in y, downwards: ${where.points[0][1]}`);

    // Selection survives all of it: hit-test the pixel the shape is drawn on.
    const inside = (120 + 160 / 2) * 1;
    const drawn = JSON.parse(renderer.document_to_client(
      JSON.stringify([[inside, 150]]),
    )).points[0];
    assert.equal(renderer.pointer_hit(drawn[0], drawn[1]), node,
      'after a pan and a zoom, clicking the pixel you can see selects the shape you can see');
    // A miss is `undefined` at the raw wasm boundary (`Option::None`); the
    // client normalises it to `null` so callers test one thing.
    assert.equal(renderer.pointer_hit(5000, 5000) ?? null, null, 'and empty space still misses');

    // A preset is absolute, and a hundred percent really is one unit per CSS
    // pixel — on a 2× display, which is where "just divide by dpr" goes wrong.
    renderer.nav_zoom_to(1);
    assert.equal(renderer.nav_scale(), 1, 'nav_zoom_to(1) is 100%');
    const a = doc(300, 300);
    const b = doc(301, 300);
    assert.ok(Math.abs(b.x - a.x - 1) < 1e-6,
      `at 100% a client pixel is a document unit: ${b.x - a.x}`);

    // A canvas that has not been measured refuses the gesture instead of
    // inventing a camera for it.
    const blank = new glue.Renderer();
    const untouched = blank.view();
    blank.nav_zoom(10, 10, 4);
    blank.nav_pan(50, 50);
    assert.equal(blank.view(), untouched, 'an unmeasured canvas has no camera to move');

    console.log('smoke[56/75]: RULE 2 — pan and zoom keep the pointer honest');
  }

  // 57. **A group inside a group is a row the panel can nest (Task 10.4 RULE 1).**
  //     The projection carries each row's **parent** — one link — rather than a
  //     nested structure per level, so the tree has no depth limit: the panel
  //     resolves a row's contents by finding the rows that name it.
  {
    const engine = new glue.VectraEngine();
    const send = (cmd) => JSON.parse(engine.dispatch_command(JSON.stringify(cmd)));
    const leaf = (name, x) => {
      const id = crypto.randomUUID();
      send({
        type: 'CreateNode', id, name,
        kind: { Rectangle: {
          x: { Literal: x }, y: { Literal: 0 },
          width: { Literal: 20 }, height: { Literal: 20 },
          corner_radius: { Literal: 0 },
        }},
      });
      return id;
    };
    const inner = leaf('Dot', 0);
    const outerChild = leaf('Mark', 30);
    const ring = crypto.randomUUID();
    send({
      type: 'CreateNode', id: ring, name: 'Ring',
      kind: { Group: { children: [inner] } },
    });
    const badge = crypto.randomUUID();
    send({
      type: 'CreateNode', id: badge, name: 'Badge',
      kind: { Group: { children: [outerChild, ring] } },
    });
    // Send the outer group to the back of the layer: one placement verb (Task
    // 10.5), and the whole run of three rows travels with it.
    send({ type: 'SetNodeParent', id: badge, parent: null, index: 0 });

    const snap = JSON.parse(engine.get_snapshot());
    const layer = snap.layers[0];
    const at = (id) => layer.children.indexOf(id);
    assert.ok(at(badge) >= 0, 'the outer group is a row');
    assert.equal(layer.child_is_group[at(badge)], true);
    assert.equal(layer.child_can_open[at(badge)], true, 'it holds something, so it opens');
    assert.equal(layer.child_parent[at(badge)], null, 'it sits at the layer’s top level');

    // The member that is a group names its own parent; the leaf inside it names
    // the group. Two links, three levels, no nested structure on the wire.
    assert.equal(layer.child_parent[at(outerChild)], badge);
    assert.equal(layer.child_parent[at(ring)], badge);
    assert.equal(layer.child_parent[at(inner)], ring);
    assert.equal(layer.child_is_group[at(ring)], true);
    assert.equal(layer.child_can_open[at(ring)], true, 'a group holding a group opens too');

    // A leaf has no contents — `can_open` is not "is a group".
    assert.equal(layer.child_is_group[at(outerChild)], false);
    assert.equal(layer.child_can_open[at(outerChild)], false);

    // …and a group with nothing in it never opens, at any level.
    const empty = crypto.randomUUID();
    send({ type: 'CreateNode', id: empty, name: 'Empty', kind: { Group: { children: [] } } });
    const after = JSON.parse(engine.get_snapshot()).layers[0];
    assert.equal(after.child_can_open[after.children.indexOf(empty)], false);

    console.log('smoke[57/75]: RULE 1 — a group inside a group is a row that opens');
  }

  // 58. **A board's box and colour are editable (Task 10.3 RULE 2)** — and
  //     editing them costs no evaluation at all, because neither is a value any
  //     node reads. The jump then frames the new box, and the export writes the
  //     new colour: the two places the edit has to show up.
  {
    const engine = new glue.VectraEngine();
    const send = (cmd) => JSON.parse(engine.dispatch_command(JSON.stringify(cmd)));
    const renderer = new glue.Renderer();
    renderer.set_viewport(0, 0, 800, 600, 1);
    const art = crypto.randomUUID();
    send({
      type: 'CreateNode', id: art, name: 'Art',
      kind: { Rectangle: {
        x: { Literal: 10 }, y: { Literal: 10 },
        width: { Literal: 40 }, height: { Literal: 40 },
        corner_radius: { Literal: 0 },
      }},
    });
    JSON.parse(engine.render_frame(renderer));

    const board = JSON.parse(engine.get_snapshot()).artboards[0].id;
    const framed = send({
      type: 'SetArtboardBounds', id: board,
      x: 300, y: 40, width: 320, height: 200,
    });
    assert.equal(framed.status, 'ok', JSON.stringify(framed));
    const dirty = framed.events.find((e) => e.type === 'Dirty');
    assert.deepEqual(dirty.ids, [], 'a board’s box is not a value any node reads');

    const recoloured = send({
      type: 'SetArtboardBackground', id: board,
      background: { r: 16, g: 24, b: 32, a: 255 },
    });
    assert.equal(recoloured.status, 'ok', JSON.stringify(recoloured));
    assert.deepEqual(
      recoloured.events.find((e) => e.type === 'Dirty').ids,
      [],
      'nor is its colour',
    );

    const snap = JSON.parse(engine.get_snapshot());
    assert.deepEqual(snap.artboards[0].bounds, [300, 40, 320, 200], 'the box is the new one');
    assert.equal(snap.artboards[0].background, '#101820', 'and the colour reads back as hex');

    // The jump frames exactly that box, with the air the dropdown promises.
    const view = JSON.parse(renderer.frame_document(300, 40, 320, 200));
    assert.ok(view.w >= 320 && view.h >= 200, `the board fits: ${JSON.stringify(view)}`);
    const centre = [view.x + view.w / 2, view.y + view.h / 2];
    assert.ok(Math.abs(centre[0] - (300 + 160)) < 0.01 && Math.abs(centre[1] - (40 + 100)) < 0.01,
      `a jump centres the board it was given: ${JSON.stringify(centre)}`);

    // The export carries the new background, and the artwork is still on the
    // board that owns it.
    const svg = JSON.parse(engine.export_current_artboard());
    assert.ok(svg.code.includes('fill="#101820"'), 'the export paints the new background');
    assert.ok(svg.code.includes(`data-vectra-artboard="${board}"`), 'for the board that was edited');
    assert.ok(svg.code.includes(`data-vectra-node="${art}"`), 'and the artwork is still on it');

    console.log('smoke[58/75]: RULE 2 — a board’s box and background, edited for free');
  }

  // 59. **Grouping is one action, and the engine refuses the impossible**
  //     (Task 10.4 RULE 1). A `Batch` creates the group and moves two nodes
  //     inside it: one undo entry restores all three facts (the group, and each
  //     node's parent), because the batch's inverse unwinds last-in-first-out.
  //     A cycle is refused typed, with the document untouched.
  {
    const engine = new glue.VectraEngine();
    const send = (cmd) => JSON.parse(engine.dispatch_command(JSON.stringify(cmd)));
    const rect = (name, x) => {
      const id = crypto.randomUUID();
      send({
        type: 'CreateNode', id, name,
        kind: { Rectangle: {
          x: { Literal: x }, y: { Literal: 0 },
          width: { Literal: 40 }, height: { Literal: 40 },
          corner_radius: { Literal: 0 },
        }},
      });
      return id;
    };
    const first = rect('First', 0);
    const second = rect('Second', 60);
    const third = rect('Third', 120);

    const rules = (snap) => {
      const layer = snap.layers[0];
      return layer.children.map((id, index) => [layer.child_names[index], layer.child_parent[index]]);
    };
    const groupId = crypto.randomUUID();
    const grouped = send({
      type: 'Batch',
      commands: [
        { type: 'CreateNode', id: groupId, name: 'Badge', kind: { Group: { children: [] } } },
        { type: 'SetNodeParent', id: first, parent: groupId, index: 0 },
        { type: 'SetNodeParent', id: second, parent: groupId, index: 1 },
      ],
    });
    assert.equal(grouped.status, 'ok', JSON.stringify(grouped));
    const dirty = grouped.events.find((e) => e.type === 'Dirty');
    assert.deepEqual(
      dirty.ids,
      [groupId],
      'exactly the new group is evaluated — the *moves* dirty nothing',
    );
    assert.ok(
      !dirty.ids.includes(first) && !dirty.ids.includes(second),
      'neither moved node is re-evaluated: membership is presentation',
    );

    let snap = JSON.parse(engine.get_snapshot());
    const rows = rules(snap);
    // A new node is appended to the layer's list, and a moved node's *block*
    // lands where its container is: so the group is at the end, with its members
    // directly in front of it — one contiguous run, which is what makes the
    // group travel as a unit.
    assert.deepEqual(
      rows.map(([name]) => name),
      ['Third', 'First', 'Second', 'Badge'],
      `the layer’s list, with the group’s block at its end: ${JSON.stringify(rows)}`,
    );
    const parentOf = (name) => rows.find(([row]) => row === name)[1];
    assert.equal(parentOf('First'), groupId, 'the first node is inside the group');
    assert.equal(parentOf('Second'), groupId, 'so is the second');
    assert.equal(parentOf('Badge'), null, 'and the group itself is at the layer’s top level');
    assert.equal(parentOf('Third'), null, 'the untouched neighbour is where it was');
    assert.ok(
      rows.map(([name]) => name).indexOf('Badge') === rows.map(([name]) => name).indexOf('Second') + 1,
      'the members sit immediately in front of the group that owns them',
    );

    // The panel's own walk, in three lines, from the wire alone: depth is the
    // number of parent links above a row.
    const depthOf = (id) => {
      let depth = 0;
      let cursor = snap.layers[0].child_parent[snap.layers[0].children.indexOf(id)];
      while (cursor) {
        depth += 1;
        cursor = snap.layers[0].child_parent[snap.layers[0].children.indexOf(cursor)];
      }
      return depth;
    };
    assert.equal(depthOf(first), 1, 'a member is one level down');
    assert.equal(depthOf(groupId), 0, 'the group is at the top');

    // One undo restores all three: the members come back out, the group goes.
    const undone = JSON.parse(engine.undo());
    assert.equal(undone.status, 'ok', JSON.stringify(undone));
    snap = JSON.parse(engine.get_snapshot());
    assert.deepEqual(
      snap.layers[0].child_names,
      ['First', 'Second', 'Third'],
      'one undo, and the document is exactly what it was',
    );
    assert.ok(snap.layers[0].child_parent.every((parent) => parent === null));
    const redone = JSON.parse(engine.redo());
    assert.equal(redone.status, 'ok', JSON.stringify(redone));
    assert.deepEqual(
      rules(JSON.parse(engine.get_snapshot())),
      rows,
      'and one redo restores exactly what the batch had produced',
    );

    // A cycle: a group inside itself, and inside its own descendant. Both are
    // refused *typed*, and the document is untouched by the refusal.
    const self = send({ type: 'SetNodeParent', id: groupId, parent: groupId, index: 0 });
    assert.equal(self.status, 'error', JSON.stringify(self));
    assert.match(self.message, /own descendant/, self.message);
    const intoChild = send({ type: 'SetNodeParent', id: groupId, parent: first, index: 0 });
    assert.equal(intoChild.status, 'error', 'a group cannot move inside its own member');
    assert.deepEqual(
      rules(JSON.parse(engine.get_snapshot())),
      rows,
      'a refused move leaves the document exactly as it was',
    );

    // Out of the group, and the node is a top-level row again.
    const out = send({ type: 'SetNodeParent', id: first, parent: null, index: 2 });
    assert.equal(out.status, 'ok', JSON.stringify(out));
    assert.deepEqual(
      out.events.find((e) => e.type === 'Dirty').ids,
      [],
      'a bare move re-evaluates nothing at all',
    );
    const after = rules(JSON.parse(engine.get_snapshot()));
    assert.equal(after.find(([name]) => name === 'First')[1], null, 'First left the group');
    assert.equal(after.find(([name]) => name === 'Second')[1], groupId, 'Second stayed');

    console.log('smoke[59/75]: RULE 1 — grouping is one undo, and cycles are refused');
  }

  // 60. **The canvas does not know what a group is** (Task 10.4 RULE 1). The
  //     draw order is the layer's list, the group paints nothing, and a grouped
  //     shape is still hit-testable at the pixel it is drawn on: nesting is an
  //     organization, and the engine's rendering path is untouched by it.
  {
    const engine = new glue.VectraEngine();
    const send = (cmd) => JSON.parse(engine.dispatch_command(JSON.stringify(cmd)));
    const renderer = new glue.Renderer();
    renderer.set_viewport(0, 0, 800, 600, 1);
    const under = (() => {
      const id = crypto.randomUUID();
      send({
        type: 'CreateNode', id, name: 'Under',
        kind: { Rectangle: {
          x: { Literal: 0 }, y: { Literal: 0 },
          width: { Literal: 200 }, height: { Literal: 200 },
          corner_radius: { Literal: 0 },
        }},
      });
      return id;
    })();
    const grouped = (() => {
      const id = crypto.randomUUID();
      send({
        type: 'CreateNode', id, name: 'Grouped',
        kind: { Rectangle: {
          x: { Literal: 40 }, y: { Literal: 40 },
          width: { Literal: 60 }, height: { Literal: 60 },
          corner_radius: { Literal: 0 },
        }},
      });
      return id;
    })();
    const groupId = crypto.randomUUID();
    send({
      type: 'Batch',
      commands: [
        { type: 'CreateNode', id: groupId, name: 'Nest', kind: { Group: { children: [] } } },
        { type: 'SetNodeParent', id: grouped, parent: groupId, index: 0 },
      ],
    });
    JSON.parse(engine.render_frame(renderer));

    const snap = JSON.parse(engine.get_snapshot());
    assert.deepEqual(
      snap.scene.z_order,
      [under, grouped],
      'both shapes are drawn, whether grouped or not',
    );
    assert.ok(!snap.scene.z_order.includes(groupId), 'a group paints nothing, so it is not in the scene');
    assert.equal(snap.scene.nodes[grouped].layer, snap.scene.nodes[under].layer, 'same layer');

    // The pointer still finds the shape that is on top at that pixel — the group
    // is transparent to the renderer, exactly as it is to the document.
    // The grouped shape spans document x 40–100, y 40–100; the one beneath it
    // spans 0–200. So (70, 70) is inside both, and the group's member must win.
    const inside = JSON.parse(renderer.document_to_client(JSON.stringify([[70, 70]]))).points[0];
    assert.equal(
      renderer.pointer_hit(inside[0], inside[1]),
      grouped,
      'the grouped shape is still the one under the pointer',
    );
    const outside = JSON.parse(renderer.document_to_client(JSON.stringify([[190, 190]]))).points[0];
    assert.equal(renderer.pointer_hit(outside[0], outside[1]), under, 'and the one beneath it');

    console.log('smoke[60/75]: RULE 1 — the canvas draws the tree without knowing it is one');
  }

  // 61. **Moving a group moves its children, in a layered document too**
  //     (Task 10.4 RULE 1). This is the fix the recon found: the layer's list is
  //     the z-order, so a group's *block* has to travel inside it.
  {
    const engine = new glue.VectraEngine();
    const send = (cmd) => JSON.parse(engine.dispatch_command(JSON.stringify(cmd)));
    const rect = (name) => {
      const id = crypto.randomUUID();
      send({
        type: 'CreateNode', id, name,
        kind: { Rectangle: {
          x: { Literal: 0 }, y: { Literal: 0 },
          width: { Literal: 10 }, height: { Literal: 10 },
          corner_radius: { Literal: 0 },
        }},
      });
      return id;
    };
    const a = rect('A');
    const b = rect('B');
    const c = rect('C');
    const groupId = crypto.randomUUID();
    send({
      type: 'Batch',
      commands: [
        { type: 'CreateNode', id: groupId, name: 'Pair', kind: { Group: { children: [] } } },
        { type: 'SetNodeParent', id: b, parent: groupId, index: 0 },
        { type: 'SetNodeParent', id: c, parent: groupId, index: 1 },
      ],
    });
    const names = () => JSON.parse(engine.get_snapshot()).layers[0].child_names;
    assert.deepEqual(names(), ['A', 'B', 'C', 'Pair'], 'B and C are inside Pair, at the end');

    // Send the group to the back: the whole run travels, contiguously.
    const moved = send({ type: 'SetNodeParent', id: groupId, parent: null, index: 0 });
    assert.equal(moved.status, 'ok', JSON.stringify(moved));
    assert.deepEqual(
      names(),
      ['B', 'C', 'Pair', 'A'],
      `the block stayed a block, and A is behind it: ${JSON.stringify(names())}`,
    );
    assert.deepEqual(
      moved.events.find((e) => e.type === 'Dirty').ids,
      [],
      'and a reorder still costs no evaluation',
    );

    // A leaf still moves alone.
    send({ type: 'SetNodeParent', id: a, parent: null, index: 0 });
    assert.deepEqual(names()[0], 'A', 'a shape is not a block');

    console.log('smoke[61/75]: RULE 1 — a group carries its subtree in the z-order');
  }

  // 62. **A drop into another layer is one batch — and one undo** (Task 10.4
  //     RULE 1). This is the exact pair the panel sends when a row is dragged
  //     onto a row of a different layer: membership first, then the position.
  {
    const engine = new glue.VectraEngine();
    const send = (cmd) => JSON.parse(engine.dispatch_command(JSON.stringify(cmd)));
    const layers = () => JSON.parse(engine.get_snapshot()).layers;
    const layerOf = (id) => layers().find((layer) => layer.children.includes(id));
    const rect = (name) => {
      const id = crypto.randomUUID();
      send({
        type: 'CreateNode', id, name,
        kind: { Rectangle: {
          x: { Literal: 0 }, y: { Literal: 0 },
          width: { Literal: 10 }, height: { Literal: 10 },
          corner_radius: { Literal: 0 },
        }},
      });
      return id;
    };

    const guides = crypto.randomUUID();
    send({ type: 'CreateLayer', id: guides, name: 'Guides' });
    send({ type: 'SetActiveLayer', id: guides });
    const grid = rect('Grid');

    const ink = crypto.randomUUID();
    send({ type: 'CreateLayer', id: ink, name: 'Ink' });
    send({ type: 'SetActiveLayer', id: ink });
    const mark = rect('Mark');
    const badge = crypto.randomUUID();
    send({ type: 'CreateNode', id: badge, name: 'Badge', kind: { Group: { children: [] } } });

    assert.equal(layerOf(grid).id, guides, 'Grid starts in Guides');
    assert.equal(layerOf(mark).id, ink, 'Mark is in Ink');

    // **The drag onto the other layer's row.** The panel's pure decision gives
    // the layer and the index; the command is the pair, in this order.
    // "Front of the container" is the end of the layer's list (the panel reads
    // top-first), so the index the panel computes is the count of the roots it
    // is not: Mark and Badge.
    const inkRoots = layers().find((layer) => layer.id === ink).children.length;
    const moved = send({
      type: 'Batch',
      commands: [
        { type: 'AssignNodeToLayer', node_id: grid, layer: ink },
        { type: 'SetNodeParent', id: grid, parent: null, index: inkRoots },
      ],
    });
    assert.equal(moved.status, 'ok', JSON.stringify(moved));
    assert.deepEqual(
      moved.events.find((e) => e.type === 'Dirty').ids,
      [],
      'a layer move is organization: nothing re-evaluates',
    );

    const inkNow = layers().find((layer) => layer.id === ink);
    assert.deepEqual(
      inkNow.children.map((id, index) => inkNow.child_names[index]),
      ['Mark', 'Badge', 'Grid'],
      `the node landed at the index it was dropped at: ${JSON.stringify(inkNow.child_names)}`,
    );
    assert.equal(
      layers().find((layer) => layer.id === guides).children.length,
      0,
      'and its old layer no longer lists it',
    );

    // **Dropped onto the group inside that other layer**: it joins the group.
    const intoGroup = send({
      type: 'Batch',
      commands: [
        { type: 'AssignNodeToLayer', node_id: grid, layer: ink },
        { type: 'SetNodeParent', id: grid, parent: badge, index: 0 },
      ],
    });
    assert.equal(intoGroup.status, 'ok', JSON.stringify(intoGroup));
    const grouped = layers().find((layer) => layer.id === ink);
    const at = grouped.children.indexOf(grid);
    assert.equal(grouped.child_parent[at], badge, 'Grid is inside Badge');
    assert.equal(grouped.child_parent[grouped.children.indexOf(mark)], null, 'Mark is not');

    // **One undo, one drag.** Undo the group drop, then the layer drop: each is
    // a single step, and the second returns Grid to Guides exactly as it was.
    assert.equal(JSON.parse(engine.undo()).status, 'ok');
    assert.equal(
      layers().find((layer) => layer.id === ink).child_parent[
        layers().find((layer) => layer.id === ink).children.indexOf(grid)
      ],
      null,
      'one undo takes it back out of the group',
    );
    assert.equal(JSON.parse(engine.undo()).status, 'ok');
    const home = layers().find((layer) => layer.id === guides);
    assert.deepEqual(
      home.children.map((id, index) => home.child_names[index]),
      ['Grid'],
      'and one more puts it back in its own layer',
    );
    assert.equal(layerOf(grid).id, guides);

    console.log('smoke[62/75]: RULE 1 — a drop into another layer is one batch, one undo');
  }

  // 63. **The ▣ button, on a selection that spans layers.** The members that live
  //     elsewhere are carried into the group's layer, so one undo has to carry
  //     them *back* — the layer is part of what the move changed, and an inverse
  //     that forgets it leaves the artwork in the wrong layer with the right
  //     parents. That was a real defect; the tree laws found it and this is the
  //     same gesture through the wasm port.
  {
    const engine = new glue.VectraEngine();
    const send = (cmd) => JSON.parse(engine.dispatch_command(JSON.stringify(cmd)));
    const layers = () => JSON.parse(engine.get_snapshot()).layers;
    const layerOf = (id) => layers().find((layer) => layer.children.includes(id));
    const rect = (name) => {
      const id = crypto.randomUUID();
      send({
        type: 'CreateNode', id, name,
        kind: { Rectangle: {
          x: { Literal: 0 }, y: { Literal: 0 },
          width: { Literal: 10 }, height: { Literal: 10 },
          corner_radius: { Literal: 0 },
        }},
      });
      return id;
    };
    const guides = crypto.randomUUID();
    send({ type: 'CreateLayer', id: guides, name: 'Guides' });
    send({ type: 'SetActiveLayer', id: guides });
    const grid = rect('Grid');
    const ink = crypto.randomUUID();
    send({ type: 'CreateLayer', id: ink, name: 'Ink' });
    send({ type: 'SetActiveLayer', id: ink });
    const mark = rect('Mark');

    // **The group action, on a selection that spans layers** (the ▣ button).
    //     The members that live elsewhere are carried into the group's layer, so
    //     one undo has to carry them *back* — the layer is part of what the move
    //     changed, and an inverse that forgets it leaves the artwork in the
    //     wrong layer with the right parents. That was a real defect; the tree
    //     laws found it and this is the same gesture through the wasm port.
    const badge2 = crypto.randomUUID();
    const pairGroup = send({
      type: 'Batch',
      commands: [
        { type: 'CreateNode', id: badge2, name: 'Group 2', kind: { Group: { children: [] } } },
        { type: 'SetNodeParent', id: mark, parent: badge2, index: 0 },
        { type: 'SetNodeParent', id: grid, parent: badge2, index: 1 },
      ],
    });
    assert.equal(pairGroup.status, 'ok', JSON.stringify(pairGroup));
    assert.equal(layerOf(grid).id, ink, 'Grid followed the group into its layer');
    assert.equal(layerOf(mark).id, ink, 'and so did Mark');

    const undone = JSON.parse(engine.undo());
    assert.equal(undone.status, 'ok', JSON.stringify(undone));
    assert.equal(
      layers().find((layer) => layer.id === guides).children.includes(grid),
      true,
      'one undo puts Grid back in its own layer',
    );
    assert.equal(layerOf(mark).id, ink, 'Mark stays where it already was');
    assert.equal(JSON.parse(engine.get_snapshot()).layers.filter((l) => l.children.includes(badge2)).length, 0);
    assert.equal(JSON.parse(engine.redo()).status, 'ok', 'and redo groups them again');

    console.log('smoke[63/75]: RULE 1 — a selection that spans layers undoes layer by layer');
  }

  // 64. **A placement lands *between* runs, never inside one** (Task 10.5). The
  //     panel's decision for "drop onto a closed group row" is `{parent: null,
  //     index: <the group's position among the roots>}` — "be that sibling". In a
  //     flat list a group's children are listed *before* it (a group paints
  //     nothing, so they are drawn behind it), and `SetNodeParent` used to splice
  //     at the group's own row: the dropped node landed *inside* the group's run,
  //     between its last member and the group. The panel — which walks parent
  //     links — called it a top-level row, and the canvas drew it in front of the
  //     group's members: two views of one document, disagreeing.
  {
    const engine = new glue.VectraEngine();
    const send = (cmd) => JSON.parse(engine.dispatch_command(JSON.stringify(cmd)));
    const layer = () => JSON.parse(engine.get_snapshot()).layers[0];
    const rect = (name) => {
      const id = crypto.randomUUID();
      send({
        type: 'CreateNode', id, name,
        kind: { Rectangle: {
          x: { Literal: 0 }, y: { Literal: 0 },
          width: { Literal: 10 }, height: { Literal: 10 },
          corner_radius: { Literal: 0 },
        }},
      });
      return id;
    };
    const a = rect('A');
    const b = rect('B');
    const c = rect('C');
    const pair = crypto.randomUUID();
    send({
      type: 'CreateNode', id: pair, name: 'Pair', kind: { Group: { children: [b, c] } },
    });
    // Canonicalise the group's run: its members, then the group.
    send({ type: 'SetNodeParent', id: pair, parent: null, index: 0 });
    assert.deepEqual(layer().child_names, ['B', 'C', 'Pair', 'A'], 'the run travels as one');

    // **The drop.** A onto the closed Pair row: the panel's own decision, sent
    // through the wire exactly as the panel sends it.
    const roots = layer();
    const index = roots.children
      .filter((id) => (roots.child_parent[roots.children.indexOf(id)] ?? null) === null)
      .filter((id) => id !== a)
      .indexOf(pair);
    const drop = send({ type: 'SetNodeParent', id: a, parent: null, index });
    assert.equal(drop.status, 'ok', JSON.stringify(drop));

    const after = layer();
    assert.deepEqual(
      after.child_names,
      ['A', 'B', 'C', 'Pair'],
      `A is behind the whole run, not inside it: ${JSON.stringify(after.child_names)}`,
    );
    assert.equal(after.child_parent[after.children.indexOf(a)], null, 'and it is a root');

    // The two views agree: walking the tree (roots, each group's members behind
    // it) reproduces the layer's list — which *is* the draw order.
    const snap = JSON.parse(engine.get_snapshot());
    const list = snap.layers[0];
    const walked = [];
    const walk = (id) => {
      const at = list.children.indexOf(id);
      if (at < 0) return;
      for (const child of list.children) {
        if ((list.child_parent[list.children.indexOf(child)] ?? null) === id) walk(child);
      }
      walked.push(id);
    };
    for (const id of list.children) {
      if ((list.child_parent[list.children.indexOf(id)] ?? null) === null) walk(id);
    }
    assert.deepEqual(
      walked.map((id) => list.child_names[list.children.indexOf(id)]),
      list.child_names,
      'the tree, walked, is the draw order',
    );
    assert.deepEqual(
      snap.scene.z_order.map((id) => list.child_names[list.children.indexOf(id)]),
      ['A', 'B', 'C'],
      'and the canvas draws the shapes in that order, the group painting nothing',
    );

    // **One placement verb** (Task 10.5). The retired `ReorderNode` is no longer
    // on the wire at all: a command the engine cannot name is a typed error the
    // log shows, never a silent no-op.
    const retired = send({ type: 'ReorderNode', id: a, index: 0 });
    assert.equal(retired.status, 'error', JSON.stringify(retired));
    assert.match(retired.message, /ReorderNode/, retired.message);

    console.log('smoke[64/75]: RULE 1 — a placement lands between runs, and one verb places');
  }
}

// ── Task 10.6: Smart Components, Make Magic, Icon Studio ────────────────────
//
// The four rules, driven against the *browser artifact* — the glue and binary
// `apps/vectra-web/src/wasm/` ships and `client.ts` loads. Nothing here reaches
// into Rust: this is exactly the surface the panels call, so a green run is the
// statement "the ⌘K bar and the component sliders have an engine behind them".

// 65. **RULE 1.** A selection becomes a master with designer props; setting one
//     prop on one instance re-evaluates *that* instance through the dependency
//     graph — and the master's own artwork does not move. The scaling law shows
//     up in the snapshot: corner radius and stroke are fractions of `size`, so
//     growing the instance to 40 scales a 4px corner to 6.667 and a 2px stroke
//     to 3.333.
{
  const sengine = new glue.VectraEngine();
  const ssend = (cmd) => JSON.parse(sengine.dispatch_command(JSON.stringify(cmd)));
  const glyph = 'deeeeeee-0000-4000-8000-000000000065';
  const badge = 'deeeeeee-0000-4000-8000-000000000066';
  assert.equal(ssend({
    type: 'CreateNode', id: glyph, name: 'glyph',
    kind: { Rectangle: { x: { Literal: 0 }, y: { Literal: 0 },
      width: { Literal: 24 }, height: { Literal: 24 }, corner_radius: { Literal: 4 } } },
  }).status, 'ok');
  assert.equal(ssend({
    type: 'CreateNode', id: badge, name: 'badge',
    kind: { Circle: { cx: { Literal: 40 }, cy: { Literal: 12 }, radius: { Literal: 12 } } },
  }).status, 'ok');
  for (const id of [glyph, badge]) {
    // A visible stroke needs a colour *and* a width: the legacy projection drops
    // a stroke layer whose colour is transparent (the engine's own rule).
    assert.equal(ssend({ type: 'SetParameter', node_id: id, property: 'style.stroke',
      value: { Color: { Literal: { r: 16, g: 16, b: 16, a: 255 } } } }).status, 'ok');
    assert.equal(ssend({ type: 'SetParameter', node_id: id, property: 'style.stroke_width',
      value: { Float: { Literal: 2 } } }).status, 'ok');
  }

  // The panel's own view, before anything is a component: a selection that can
  // *become* one, with no props to show yet.
  const selection = JSON.parse(sengine.set_selection(JSON.stringify([glyph, badge])));
  assert.equal(selection.status, 'ok');
  assert.equal(selection.count, 2);
  const before = JSON.parse(sengine.component_view());
  assert.equal(before.role, 'selection', JSON.stringify(before));
  assert.equal(before.can_create, true);
  assert.deepEqual(before.props, []);

  const made = JSON.parse(sengine.create_component(JSON.stringify([glyph, badge]), 'Glyph'));
  assert.equal(made.status, 'ok', JSON.stringify(made));
  assert.ok(typeof made.prose === 'string' && made.prose.length > 0, 'RULE 4: prose');

  const master = JSON.parse(sengine.component_view());
  assert.equal(master.role, 'master', JSON.stringify(master));
  assert.equal(master.instances, 0);
  const keys = master.props.map((prop) => prop.key);
  // The props a designer gets, and the ones a 24px glyph cannot carry are
  // simply absent — no dead sliders.
  assert.deepEqual(keys, ['size', 'stroke_width', 'corner_radius', 'color'], JSON.stringify(master.props));
  const size = master.props.find((prop) => prop.key === 'size');
  assert.equal(size.value, 24, 'the master reads at its design size');
  const stroke = master.props.find((prop) => prop.key === 'stroke_width');
  assert.equal(stroke.law, 'scaled');
  assert.equal(stroke.derived, true);
  assert.equal(stroke.from, 'size', 'stroke follows size — that is the whole law');
  assert.ok(stroke.max >= stroke.value * 4, 'and the slider spans a real range');

  // A second copy of the artwork, bound to the same law.
  const placed = JSON.parse(sengine.instantiate_component(master.id, 'big'));
  assert.equal(placed.status, 'ok', JSON.stringify(placed));
  const instance = placed.created;
  assert.ok(instance, 'the envelope names what it made');
  sengine.set_selection(JSON.stringify([instance]));
  const view = JSON.parse(sengine.component_view());
  assert.equal(view.role, 'instance');
  assert.equal(view.master, master.id);
  assert.equal(view.props.find((prop) => prop.key === 'size').value, 24, JSON.stringify(view.props));

  // Placing an instance places a **copy**: before anything is written it already
  // reads the master's numbers — a 1-unit speck would be the engine reading its
  // own expression-bound clones as literals.
  const rectsOf = () => Object.entries(JSON.parse(sengine.get_snapshot()).scene.nodes)
    .filter(([, node]) => node.primitive.type === 'rect');
  // The master's member is the one we authored; everything else that is a rect
  // came out of the instance.
  const cloneRect = () => rectsOf().find(([id]) => id !== glyph)[1];
  const numbers = (node) => ({ w: node.primitive.w, r: node.primitive.corner_radius,
    s: node.style.stroke_width });
  assert.deepEqual(numbers(cloneRect()), { w: 24, r: 4, s: 2 },
    `a fresh instance is the master at its design size: ${JSON.stringify(rectsOf().map(([id, n]) => [id.slice(-3), numbers(n)]))}`);

  // **The Prop Law, through the panel's own button.** One write.
  const written = JSON.parse(sengine.set_component_prop(
    instance, 'size', JSON.stringify({ Float: { Literal: 40 } }),
  ));
  assert.equal(written.status, 'ok', JSON.stringify(written));
  assert.ok(typeof written.prose === 'string' && written.prose.length > 0, 'RULE 4: prose');

  const scaled = numbers(cloneRect());
  assert.ok(Math.abs(scaled.w - 40) < 1e-6, `the instance took the new size: ${JSON.stringify(scaled)}`);
  assert.ok(Math.abs(scaled.r - 4 * (40 / 24)) < 1e-6, `corner scales: ${JSON.stringify(scaled)}`);
  assert.ok(Math.abs(scaled.s - 2 * (40 / 24)) < 1e-6, `stroke scales: ${JSON.stringify(scaled)}`);
  assert.deepEqual(numbers(JSON.parse(sengine.get_snapshot()).scene.nodes[glyph]), { w: 24, r: 4, s: 2 },
    "and the master's own artwork did not move");

  // One undo takes the size back, and the instance is a copy again.
  assert.equal(JSON.parse(sengine.undo()).status, 'ok');
  assert.deepEqual(numbers(cloneRect()), { w: 24, r: 4, s: 2 }, 'one undo, and it is a copy');
  console.log('smoke[65/75]: RULE 1 — a master, a prop, one instance, and the scaling law');
}

// 66. **RULE 2 + RULE 4.** A prompt becomes commands the *engine* validates:
//     "make this geometric" snaps, unifies and answers in a sentence; "align
//     perfectly" adds the constraints; and a prompt the selection cannot support
//     is refused in a sentence that says what is missing — never in JSON.
{
  const aengine = new glue.VectraEngine();
  const asend = (cmd) => JSON.parse(aengine.dispatch_command(JSON.stringify(cmd)));
  const ids = [
    'ae000000-0000-4000-8000-000000000061',
    'ae000000-0000-4000-8000-000000000071',
    'ae000000-0000-4000-8000-000000000081',
  ];
  ids.forEach((id, index) => {
    assert.equal(asend({
      type: 'CreateNode', id, name: `shape ${index + 1}`,
      kind: { Rectangle: { x: { Literal: index * 40 }, y: { Literal: 0 },
        width: { Literal: 24 }, height: { Literal: 24 }, corner_radius: { Literal: 6 } } },
    }).status, 'ok');
  });
  aengine.set_selection(JSON.stringify(ids));

  const run = JSON.parse(aengine.ai_execute_with_retry('make this geometric', ''));
  assert.equal(run.status, 'ok', JSON.stringify(run));
  assert.ok(run.prose.startsWith('✨'), `the receipt is a sentence: ${run.prose}`);
  assert.ok(!run.prose.includes('{') && !run.prose.includes('['), `and never JSON: ${run.prose}`);
  assert.ok(run.report.events.length > 0, 'something was actually applied');
  const snapped = Object.values(JSON.parse(aengine.get_snapshot()).scene.nodes)
    .filter((node) => node.primitive.type === 'rect')
    .map((node) => node.primitive.corner_radius);
  assert.deepEqual(snapped, [0, 0, 0], `corners were squared: ${JSON.stringify(snapped)}`);

  // Align: two constraints on the x slot (a column), and the spacing rule the
  // third shape earns.
  const aligned = JSON.parse(aengine.ai_execute_with_retry('align perfectly', ''));
  assert.equal(aligned.status, 'ok', JSON.stringify(aligned));
  assert.ok(aligned.prose.startsWith('✨'), aligned.prose);
  const doc = JSON.parse(aengine.document_json());
  const constraints = doc.constraints.constraints ?? doc.constraints;
  assert.ok(constraints.length >= 2, `the constraints are in the document: ${constraints.length}`);

  // **A refusal is a sentence too.** One shape cannot form a column; the engine
  // says what is missing instead of pretending not to understand the phrase.
  const lonely = new glue.VectraEngine();
  lonely.dispatch_command(JSON.stringify({
    type: 'CreateNode', id: 'af000000-0000-4000-8000-000000000001', name: 'only',
    kind: { Circle: { cx: { Literal: 0 }, cy: { Literal: 0 }, radius: { Literal: 8 } } },
  }));
  lonely.set_selection(JSON.stringify(['af000000-0000-4000-8000-000000000001']));
  const refused = JSON.parse(lonely.ai_execute_with_retry('align perfectly', ''));
  assert.equal(refused.status, 'error', JSON.stringify(refused));
  assert.match(refused.message, /two shapes/, refused.message);
  assert.ok(!refused.message.includes('{'), `no JSON in a refusal either: ${refused.message}`);
  console.log('smoke[66/75]: RULE 2 — prompts become commands, and refusals are sentences (RULE 4)');
}

// 67. **RULE 3.** "Generate Icon Set": one artboard per size, and the 2px stroke
//     of a 24px master is 1.33px at 16 — optically correct, not a hairline.
{
  const iengine = new glue.VectraEngine();
  const isend = (cmd) => JSON.parse(iengine.dispatch_command(JSON.stringify(cmd)));
  const glyph = '1c000000-0000-4000-8000-000000000067';
  assert.equal(isend({
    type: 'CreateNode', id: glyph, name: 'glyph',
    kind: { Rectangle: { x: { Literal: 0 }, y: { Literal: 0 },
      width: { Literal: 24 }, height: { Literal: 24 }, corner_radius: { Literal: 4 } } },
  }).status, 'ok');
  assert.equal(isend({ type: 'SetParameter', node_id: glyph, property: 'style.stroke',
    value: { Color: { Literal: { r: 16, g: 16, b: 16, a: 255 } } } }).status, 'ok');
  assert.equal(isend({ type: 'SetParameter', node_id: glyph, property: 'style.stroke_width',
    value: { Float: { Literal: 2 } } }).status, 'ok');
  iengine.set_selection(JSON.stringify([glyph]));
  const made = JSON.parse(iengine.create_component(JSON.stringify([glyph]), 'Glyph'));
  assert.equal(made.status, 'ok', JSON.stringify(made));
  const master = made.created;
  assert.ok(master, 'the macro needs a master, and the envelope names it');

  const set = JSON.parse(iengine.icon_set(master, JSON.stringify([16, 32, 48])));
  assert.equal(set.status, 'ok', JSON.stringify(set));
  const snap = JSON.parse(iengine.get_snapshot());
  const sizes = [16, 32, 48];
  for (const size of sizes) {
    assert.ok(snap.artboards.some((board) => board.bounds[2] === size && board.bounds[3] === size),
      `an artboard per size: ${JSON.stringify(snap.artboards.map((b) => b.bounds))}`);
  }
  const clones = Object.values(snap.scene.nodes)
    .filter((node) => node.primitive.type === 'rect' && sizes.some((s) => Math.abs(node.primitive.w - s) < 1e-9))
    .map((node) => ({ size: node.primitive.w, stroke: node.style.stroke_width }));
  assert.equal(clones.length, 3, `three scaled copies: ${JSON.stringify(clones)}`);
  for (const clone of clones) {
    assert.ok(Math.abs(clone.stroke / clone.size - 2 / 24) < 1e-9,
      `the stroke law holds at every size: ${JSON.stringify(clones)}`);
  }
  const sixteen = clones.find((clone) => clone.size === 16);
  assert.ok(sixteen.stroke >= 1.0, `no hairline at 16px: ${JSON.stringify(sixteen)}`);
  assert.ok(!set.prose.includes('{'), set.prose);
  console.log('smoke[67/75]: RULE 3 — the icon ladder, and the stroke law at every size');
}

// 68. **RULE 4's other half: what the panels are fed.** The engine publishes the
//     macros the ⌘K bar offers as chips, names the selection in prose, and
//     describes the plan in designer language — the wire carries no JSON to
//     print, because the plan is described, not dumped.
{
  const pengine = new glue.VectraEngine();
  const words = JSON.parse(pengine.structural_macros());
  assert.equal(words.length, 5, JSON.stringify(words));
  for (const macro of words) {
    assert.ok(macro.prompt && macro.label && macro.hint, JSON.stringify(macro));
    assert.ok(!/[{}]/.test(macro.label + macro.hint), JSON.stringify(macro));
  }
  const empty = JSON.parse(pengine.set_selection('[]'));
  assert.equal(empty.count, 0);
  assert.equal(empty.prose, 'Nothing selected');
  const view = JSON.parse(pengine.component_view());
  assert.equal(view.role, 'none');
  assert.ok(!/[{}]/.test(view.headline), view.headline);

  // An empty selection is *absent* from the summary's struct, not a list of
  // blanks — and the grounding says so in words rather than omitting the section.
  const summary = JSON.parse(pengine.document_summary());
  assert.ok(summary.selection === undefined || summary.selection.length === 0);
  assert.ok(pengine.ai_prompt().includes('SELECTION: none'), 'the grounding names the subject');

  // With a node selected, both surfaces carry it: the summary by id, the prompt
  // by the words "this" and "these" point at.
  const only = '1c000000-0000-4000-8000-000000000068';
  pengine.dispatch_command(JSON.stringify({
    type: 'CreateNode', id: only, name: 'subject',
    kind: { Circle: { cx: { Literal: 0 }, cy: { Literal: 0 }, radius: { Literal: 6 } } },
  }));
  assert.equal(JSON.parse(pengine.set_selection(JSON.stringify([only]))).count, 1);
  const grounded = JSON.parse(pengine.document_summary());
  assert.deepEqual(grounded.selection, [only]);
  const prompt = pengine.ai_prompt();
  assert.ok(prompt.includes(only) && prompt.includes('SELECTION (1 node(s)'),
    'the prompt the model is grounded on carries the exact id it must use');
  console.log('smoke[68/75]: RULE 4 — chips, prose and headlines come from the engine');
}

// 69. **RULE 3a — Alpha Lock is a boundary, not a rewrite.** The flag round-trips
//     through the snapshot and re-evaluates nothing; the *drawing door* is what
//     obeys it. A stroke with nowhere to land is refused in a sentence, and a
//     stroke that crosses the artwork's edge keeps only the part inside it.
{
  const aengine = new glue.VectraEngine();
  const send = (cmd) => JSON.parse(aengine.dispatch_command(JSON.stringify(cmd)));
  const snap = () => JSON.parse(aengine.get_snapshot());
  const layer = snap().active_layer;
  assert.ok(layer, 'open_workspace seeds the layer the flag lives on');

  // The layer's existing artwork: a 200×200 square at (100, 100).
  const art = crypto.randomUUID();
  send({
    type: 'CreateNode', id: art, name: 'art',
    kind: { Rectangle: {
      x: { Literal: 100 }, y: { Literal: 100 },
      width: { Literal: 200 }, height: { Literal: 200 },
      corner_radius: { Literal: 0 },
    }},
  });

  const locked = send({ type: 'SetLayerAlphaLocked', id: layer, alpha_locked: true });
  assert.equal(locked.status, 'ok', JSON.stringify(locked));
  assert.ok(eventTypes(locked).includes('LayersUpdated'), 'the panel hears about the toggle');
  const lockDirty = locked.events.find((e) => e.type === 'Dirty');
  assert.deepEqual(lockDirty.ids, [], 'a lock resolves no parameter: zero evaluation');
  assert.equal(snap().layers.find((l) => l.id === layer).alpha_locked, true, 'and it round-trips');

  // Sweep the brush from `from` to `to`, sample by sample — through the real door.
  const sweep = (from, to, steps = 10) => {
    for (let i = 0; i <= steps; i += 1) {
      const t = i / steps;
      const reply = JSON.parse(aengine.draw_pointer(
        'brush', i === 0 ? 'down' : 'move',
        from[0] + (to[0] - from[0]) * t, from[1] + (to[1] - from[1]) * t,
        false, false, 0,
      ));
      assert.equal(reply.ok, true, JSON.stringify(reply));
    }
  };
  // Every point the stored path passes through, from the snapshot's own SVG data
  // (absolute `M`/`L`/`Q`/`C`/`Z` commands, straight coordinate pairs).
  const pathPoints = (d) => {
    const numbers = (d.match(/-?\d+(?:\.\d+)?(?:e-?\d+)?/gi) ?? []).map(Number);
    const points = [];
    for (let i = 0; i + 1 < numbers.length; i += 2) points.push([numbers[i], numbers[i + 1]]);
    return points;
  };

  // (a) A stroke in empty space: refused, and the refusal is about alpha lock.
  sweep([400, 500], [700, 560]);
  const refused = JSON.parse(aengine.draw_brush_commit(null));
  assert.equal(refused.ok, false, JSON.stringify(refused));
  assert.ok(/alpha lock/.test(refused.error ?? ''), `a sentence, not a stack trace: ${refused.error}`);
  assert.ok(!refused.node_id, 'and nothing was written');

  // (b) A stroke that starts inside the artwork and runs 200 units past its right
  //     edge. It commits — and the geometry it stores is inside the lines.
  sweep([150, 200], [500, 200]);
  const committed = JSON.parse(aengine.draw_brush_commit(null));
  assert.equal(committed.ok, true, JSON.stringify(committed));
  const geometry = snap().scene.nodes[committed.node_id].primitive;
  assert.equal(geometry.type, 'path', 'a clipped stroke is a path, whatever the draft was');
  const points = pathPoints(geometry.d);
  const beyond = points.filter(([x]) => x > 300 + 1e-6);
  assert.equal(beyond.length, 0,
    `not one point of the stored geometry is past the artwork's edge: ${JSON.stringify(beyond.slice(0, 4))}`);
  assert.ok(points.length >= 2, 'and it is still a stroke');
  assert.ok(points.every(([x, y]) => x >= 100 - 1e-6 && y >= 100 - 1e-6),
    'the alpha lock is a box, not an edge: nothing escaped sideways either');
  console.log('smoke[69/75]: RULE 3a — alpha lock refuses the miss and clips the crossing');
}

// 70. **RULE 3b — a clipping mask is live geometry.** The upper layer shows only
//     where it overlaps the layer below; the layer below is untouched; and
//     clearing the flag restores the *document's* answer, not a saved copy.
{
  const cengine = new glue.VectraEngine();
  const send = (cmd) => JSON.parse(cengine.dispatch_command(JSON.stringify(cmd)));
  const snap = () => JSON.parse(cengine.get_snapshot());
  const bottom = snap().active_layer;
  const under = crypto.randomUUID();
  send({
    type: 'CreateNode', id: under, name: 'under',
    kind: { Rectangle: {
      x: { Literal: 0 }, y: { Literal: 0 },
      width: { Literal: 100 }, height: { Literal: 100 },
      corner_radius: { Literal: 0 },
    }},
  });

  const top = crypto.randomUUID();
  send({ type: 'CreateLayer', id: top, name: 'Sky' });
  send({ type: 'SetActiveLayer', id: top });
  const over = crypto.randomUUID();
  send({
    type: 'CreateNode', id: over, name: 'over',
    kind: { Rectangle: {
      x: { Literal: 50 }, y: { Literal: 50 },
      width: { Literal: 300 }, height: { Literal: 300 },
      corner_radius: { Literal: 0 },
    }},
  });

  const before = snap();
  assert.equal(before.scene.nodes[over].primitive.type, 'rect', 'an ordinary shape to start');
  assert.equal(before.layers.find((l) => l.id === top).clipped_to, bottom,
    'the row names the layer it would clip to');
  assert.equal(before.layers.find((l) => l.id === bottom).clipped_to, null,
    'and the bottom layer names nothing — there is nothing under it');

  const on = send({ type: 'SetLayerClippingMask', id: top, clipping_mask: true });
  assert.equal(on.status, 'ok', JSON.stringify(on));
  assert.equal(on.events.find((e) => e.type === 'Dirty').ids.includes(over), true,
    'the reshaped node is in the dirty set — the renderer has to hear about it');
  assert.equal(snap().layers.find((l) => l.id === top).clipping_mask, true);

  const clipped = snap();
  assert.equal(clipped.scene.nodes[over].primitive.type, 'path',
    'the shape is now a derived region, not the authored rect');
  assert.equal(clipped.scene.nodes[under].primitive.type, 'rect', 'the mask itself is not reshaped');
  assert.equal(clipped.scene.nodes[under].primitive.w, 100, 'and not resized either');
  const corner = (clipped.scene.nodes[over].primitive.d.match(/-?\d+(?:\.\d+)?/g) ?? []).map(Number);
  const xs = corner.filter((_, i) => i % 2 === 0);
  const ys = corner.filter((_, i) => i % 2 === 1);
  assert.ok(xs.every((x) => x >= 50 - 1e-6 && x <= 100 + 1e-6), `only the shared column: ${xs}`);
  assert.ok(ys.every((y) => y >= 50 - 1e-6 && y <= 100 + 1e-6), `and the shared row: ${ys}`);

  // Clearing the flag: the restore path asks the evaluator for the document's
  // own geometry again — which is why it comes back exactly.
  send({ type: 'SetLayerClippingMask', id: top, clipping_mask: false });
  const restored = snap();
  assert.deepEqual(restored.scene.nodes[over].primitive, before.scene.nodes[over].primitive,
    'the document geometry is back, byte for byte');
  assert.equal(restored.layers.find((l) => l.id === top).clipping_mask, false);
  console.log('smoke[70/75]: RULE 3b — a layer shows only over the layer below, non-destructively');
}

// 71. **RULE 4's engine half — ColorDrop.** The drop is decided by the renderer's
//     own hit test (the same index a click uses) and written as one fill on the
//     node it found; empty space answers nothing, so a drop there does nothing.
{
  const dengine = new glue.VectraEngine();
  const send = (cmd) => JSON.parse(dengine.dispatch_command(JSON.stringify(cmd)));
  const renderer = new glue.Renderer();
  renderer.set_viewport(0, 0, 800, 600, 1);

  const square = crypto.randomUUID();
  send({
    type: 'CreateNode', id: square, name: 'Square',
    kind: { Rectangle: {
      x: { Literal: 100 }, y: { Literal: 100 },
      width: { Literal: 200 }, height: { Literal: 200 },
      corner_radius: { Literal: 0 },
    }},
  });
  JSON.parse(dengine.render_frame(renderer));

  // Document y is up, screen y is down: the square spans doc y 100..300, i.e.
  // screen y 300..500 on this 600-tall canvas.
  const inside = JSON.parse(renderer.document_to_client(JSON.stringify([[200, 200]]))).points[0];
  const found = renderer.pointer_hit(inside[0], inside[1]) ?? null;
  assert.equal(found, square, 'the drop lands on the shape under the pointer');
  const empty = JSON.parse(renderer.document_to_client(JSON.stringify([[700, 560]]))).points[0];
  assert.equal(renderer.pointer_hit(empty[0], empty[1]) ?? null, null,
    'and empty space hits nothing — the drop does nothing');

  // The fill the UI would send: one solid appearance layer, written as a literal.
  const filled = send({
    type: 'SetAppearances', node_id: square,
    appearances: [{
      kind: 'Fill',
      paint: { Solid: { Literal: { r: 232, g: 98, b: 44, a: 255 } } },
      opacity: { Literal: 1 },
      blend: 'Normal',
      visible: true,
    }],
  });
  assert.equal(filled.status, 'ok', JSON.stringify(filled));
  const stack = JSON.parse(dengine.get_snapshot()).scene.nodes[square].style.appearances;
  assert.equal(stack.length, 1, JSON.stringify(stack));
  assert.equal(stack[0].kind, 'fill');
  assert.deepEqual(stack[0].paint, { type: 'solid', color: '#e8622c' },
    'and the colour that comes back is the one that was dropped');
  console.log('smoke[71/75]: RULE 4 — the drop fills what the hit test found, and nothing else');
}

// ── Task 11.0: parametric typography, end to end ───────────────────────────
//
// The four rules driven through the REAL module with nothing but JSON: place
// type, bind it to a path, slide it, outline it, undo it. The assertions are
// about what a designer would see — the run's box, where its glyphs are drawn,
// what the layers panel would list afterwards.

/// The Text tool's own payload, used by the three typography steps: every
/// typographic number is a `Parameter`, exactly as the UI's `createText` sends
/// them, so nothing below is a special case for the test's benefit.
const runPayload = (text, fontSize) => ({
  Text: {
    text,
    font_family: 'Vectra Sans',
    font_size: { Literal: fontSize },
    letter_spacing: { Literal: 0 },
    line_height: { Literal: 1.2 },
    alignment: 'left',
    x: { Literal: 40 },
    y: { Literal: 300 },
  },
});

// 72. **A run draws.** A text node creates with every number parametric, shapes
//     through the bundled face (no host font library involved), and reaches the
//     snapshot as metrics *plus* real path geometry: the renderer tessellates
//     text by tessellating a path, so a run's letterforms and a path's segments
//     are the same kind of thing.
{
  const tengine = new glue.VectraEngine();
  const send = (cmd) => JSON.parse(tengine.dispatch_command(JSON.stringify(cmd)));
  const snap = () => JSON.parse(tengine.get_snapshot());

  const run = crypto.randomUUID();
  const created = send({
    type: 'CreateNode',
    id: run,
    name: 'logo',
    kind: {
      Text: {
        text: 'Vectra',
        font_family: 'Vectra Sans',
        font_size: { Literal: 64 },
        letter_spacing: { Literal: 0 },
        line_height: { Literal: 1.2 },
        alignment: 'left',
        x: { Literal: 40 },
        y: { Literal: 300 },
      },
    },
  });
  assert.equal(created.status, 'ok', JSON.stringify(created));

  const scene = snap().scene;
  const node = scene.nodes[run];
  assert.ok(node, 'the run is in the scene');
  assert.equal(node.primitive.type, 'text', JSON.stringify(node.primitive).slice(0, 200));
  assert.ok(node.primitive.glyphs >= 6, `six letters shaped: ${node.primitive.glyphs}`);
  assert.ok(node.primitive.d.includes('M'), 'the run carries real outline geometry');
  assert.ok(node.primitive.width > 0 && node.primitive.height > 0, 'and a box');

  // The typography row the panel edits, resolved and tagged with its sources.
  assert.equal(node.text.text, 'Vectra');
  assert.equal(node.text.font_size, 64);
  assert.equal(node.text.font_size_source, 'literal');
  assert.equal(node.text.bound_to, null);

  // The picker's options come from the engine's own font library.
  assert.ok(scene.fonts.includes('Vectra Sans'), JSON.stringify(scene.fonts));
  console.log('smoke[72/79]: RULE 1 — a parametric run shapes, snapshots and draws');
}

// 73. **The Parametric Text Law.** Bind `font_size` to a `$variable` and change
//     the variable: the run is *already* re-laid out on the next read. The box
//     scales with the size and the drawn geometry changes with it, because the
//     variable write is the same one that moves a rectangle's width.
{
  const pengine = new glue.VectraEngine();
  const send = (cmd) => JSON.parse(pengine.dispatch_command(JSON.stringify(cmd)));
  const snap = () => JSON.parse(pengine.get_snapshot());
  const run = crypto.randomUUID();
  send({ type: 'CreateNode', id: run, name: 'scaled', kind: runPayload('Vectra', 64) });
  const box = () => snap().scene.nodes[run].primitive;

  send({ type: 'SetVariable', name: 'scale', value: 32 });
  send({
    type: 'SetParameter',
    node_id: run,
    property: 'font_size',
    // A geometry slot's value is a `ParamValue`, not a bare parameter: the
    // engine needs to know *which* of a node's three value kinds it is editing
    // (a float, a point, a colour) before it can route it to a slot.
    value: { Float: { Variable: 'scale' } },
  });
  const small = box();
  assert.equal(snap().scene.nodes[run].text.font_size, 32, 'the slot resolves through the variable');
  // The tag is the parameter's own `source_tag()` — the same vocabulary a
  // rectangle's `x_source` speaks — not a name written for this step.
  assert.equal(snap().scene.nodes[run].text.font_size_source, 'variable');

  const events = send({ type: 'SetVariable', name: 'scale', value: 96 });
  assert.ok(eventTypes(events).includes('Dirty'), 'a size change dirties the run');
  const big = box();

  const ratio = big.width / small.width;
  assert.ok(Math.abs(ratio - 3) < 0.02, `the box scaled with the size: ${ratio}`);
  assert.notEqual(big.d, small.d, 'and the letterforms were laid out again');
  assert.ok(big.height > small.height, 'type grows upwards too');
  console.log('smoke[73/79]: THE PARAMETRIC TEXT LAW — a variable re-lays out a run');
}

// 74. **Text on a path** (RULE 2). Bound to a circle the run leaves its baseline
//     and rides the curve — the drawn geometry is somewhere else entirely — and
//     an `offset` slides it along the arc. A source that is not path-shaped is
//     refused *before* anything is written.
{
  const cengine = new glue.VectraEngine();
  const send = (cmd) => JSON.parse(cengine.dispatch_command(JSON.stringify(cmd)));
  const snap = () => JSON.parse(cengine.get_snapshot());

  const circle = crypto.randomUUID();
  send({
    type: 'CreateNode',
    id: circle,
    kind: { Circle: { cx: { Literal: 0 }, cy: { Literal: 0 }, radius: { Literal: 200 } } },
  });
  const bound = crypto.randomUUID();
  send({ type: 'CreateNode', id: bound, name: 'around', kind: runPayload('around', 40) });
  const straight = snap().scene.nodes[bound].primitive.d;

  const bind = send({
    type: 'BindTextToPath',
    node_id: bound,
    path: circle,
    offset: { Literal: 0 },
  });
  assert.equal(bind.status, 'ok', JSON.stringify(bind));
  const onCircle = snap().scene.nodes[bound];
  assert.equal(onCircle.text.bound_to, circle, 'the snapshot names the source');
  assert.notEqual(onCircle.primitive.d, straight, 'the run is no longer straight');
  assert.equal(onCircle.primitive.glyphs, 6, 'same six glyphs, different placement');

  // A rectangle is not a path to ride: refused, and nothing was written.
  const rect = crypto.randomUUID();
  send({
    type: 'CreateNode',
    id: rect,
    kind: {
      Rectangle: {
        x: { Literal: 0 },
        y: { Literal: 0 },
        width: { Literal: 10 },
        height: { Literal: 10 },
        corner_radius: { Literal: 0 },
      },
    },
  });
  const refused = send({
    type: 'BindTextToPath',
    node_id: bound,
    path: rect,
    offset: { Literal: 0 },
  });
  assert.equal(refused.status, 'error', 'a rectangle is not a path to ride');
  assert.equal(snap().scene.nodes[bound].text.bound_to, circle, 'and nothing changed');

  // The offset slides the run: the geometry moves, the string does not.
  send({
    type: 'SetParameter',
    node_id: bound,
    property: 'path_offset',
    value: { Float: { Literal: 120 } },
  });
  const slid = snap().scene.nodes[bound];
  assert.notEqual(slid.primitive.d, onCircle.primitive.d, 'the run slid along the arc');
  assert.equal(slid.text.offset, 120);
  assert.equal(slid.text.text, 'around');
  console.log('smoke[74/79]: RULE 2 — a run rides the curve and slides along it');
}

// 75. **Outline** (RULE 3). The engine shapes the run itself, mints the
//     letterform nodes, groups them and hides the type it replaced — and one
//     undo puts the document back, byte for byte.
{
  const oengine = new glue.VectraEngine();
  const send = (cmd) => JSON.parse(oengine.dispatch_command(JSON.stringify(cmd)));
  const snap = () => JSON.parse(oengine.get_snapshot());

  const text = crypto.randomUUID();
  send({ type: 'CreateNode', id: text, name: 'word', kind: runPayload('out', 80) });
  const before = snap();
  const drawn = before.scene.nodes[text].primitive.d;
  const beforeIds = new Set(Object.keys(before.scene.nodes));
  const glyphs = before.scene.nodes[text].primitive.glyphs;

  const reply = JSON.parse(oengine.outline_text(text));
  assert.equal(reply.status, 'ok', JSON.stringify(reply));
  const group = reply.created;
  assert.ok(group, 'the reply names the group it made');

  const after = snap();
  // 1. The type is hidden, not deleted — and still holds every authored value.
  assert.equal(after.scene.nodes[text].visible, false, 'the source run is hidden');
  assert.equal(after.scene.nodes[text].text.text, 'out', 'and still says what it said');
  // 2. One **new** node per shaped glyph, each a drawable path, and the group
  //    that holds them is a real row of the layer tree.
  const fresh = Object.keys(after.scene.nodes).filter((id) => !beforeIds.has(id));
  assert.equal(fresh.length, glyphs, `one letterform per glyph: ${fresh.length} vs ${glyphs}`);
  assert.ok(
    fresh.every((id) => after.scene.nodes[id].primitive.type === 'path'),
    'every letterform is a path node',
  );
  assert.ok(
    fresh.every((id) => after.scene.nodes[id].visible && after.scene.nodes[id].primitive.d.includes('M')),
    'and each one carries geometry it can be filled with',
  );
  const row = after.layers.flatMap((layer) => layer.children ?? []).length
    ? after.layers.find((layer) => (layer.children ?? []).includes(fresh[0]))
    : null;
  assert.ok(row, 'the letterforms are children of a group in the layer tree');
  assert.equal(row.children.filter((id) => fresh.includes(id)).length, fresh.length,
    'and the group holds exactly the new letterforms');
  // 3. One undo, and the document is back exactly as it was.
  assert.equal(JSON.parse(oengine.undo()).status, 'ok');
  const undone = snap();
  assert.equal(undone.scene.nodes[text].visible, true, 'the type is back on screen');
  assert.deepEqual(
    Object.keys(undone.scene.nodes).sort(),
    [...beforeIds].sort(),
    'the letterforms are gone',
  );
  assert.equal(undone.scene.nodes[text].primitive.d, drawn, 'the run is byte for byte');
  console.log('smoke[75/79]: RULE 3 — outlining is non-destructive, grouped, and one undo');
}


// ── Task 12.0: smart fills, regions and broken paths ───────────────────────
//
// Four rules driven through the REAL engine with nothing but JSON: build an
// arrangement, read its regions, fill one, move a boundary, break a path at its
// crossings. Every assertion is about what a designer would see — which faces
// exist, where the fill sits in the draw order, whether the geometry followed
// the shape, and whether the pieces add back up to what they came from.

// 76. **The Region Graph** (RULE 1). Two overlapping circles make exactly three
//     faces — A only, B only, A∩B — the crossing points are the engine's, and
//     each circle's outline is partitioned by the spans between them. A probe
//     point identifies the face it is in; empty space identifies nothing.
{
  const rengine = new glue.VectraEngine();
  const send = (cmd) => JSON.parse(rengine.dispatch_command(JSON.stringify(cmd)));
  const snap = () => JSON.parse(rengine.get_snapshot());
  const circle = (cx, cy, r, name) => {
    const id = crypto.randomUUID();
    const created = send({
      type: 'CreateNode',
      id,
      name,
      kind: { Circle: { cx: { Literal: cx }, cy: { Literal: cy }, radius: { Literal: r } } },
    });
    assert.equal(created.status, 'ok', JSON.stringify(created));
    return id;
  };

  const a = circle(200, 200, 100, 'A');
  const b = circle(320, 200, 100, 'B');
  const plan = JSON.parse(rengine.smart_fill_plan(JSON.stringify([a, b]), JSON.stringify([260, 200])));

  assert.deepEqual(
    plan.sources.map((source) => source.id),
    [a, b],
    'the boundaries are listed in the order they were named',
  );
  assert.equal(plan.regions.length, 3, `exactly three regions: ${JSON.stringify(plan.regions)}`);
  const lens = plan.regions.find((region) => region.members.length === 2);
  const onlyA = plan.regions.find((region) => region.members.join() === a);
  const onlyB = plan.regions.find((region) => region.members.join() === b);
  assert.ok(lens && onlyA && onlyB, 'A only, B only and A∩B, one each');
  // The lens is the smallest face, and the probe point is inside it.
  assert.equal(plan.hit, lens.index, 'the probe point identifies the lens');
  assert.ok(onlyA.area > lens.area && onlyB.area > lens.area, 'the lens is the smallest face');
  // The circles are r=100 with centres 120 apart: the lens area has a closed
  // form, and the engine's own number must match it — 2r²cos⁻¹(d/2r) − (d/2)√(4r²−d²).
  const analytic = 2 * 1e4 * Math.acos(0.6) - 60 * Math.sqrt(4e4 - 1.44e4);
  assert.ok(
    Math.abs(lens.area - analytic) / analytic < 0.02,
    `lens area ${lens.area} ≈ ${analytic}`,
  );
  assert.ok(lens.path.includes('M') && lens.path.includes('Z'), 'a region is a clean closed path');

  // The crossing points themselves: two circles crossing at x = 260, y = 200 ± 80.
  // A crossing is where the two *outlines the renderer draws* meet, and those
  // outlines are flattened polygons whose chords cut the corner by up to the
  // flattening tolerance — so the honest check is that each crossing lies on
  // both circles, near the analytic point, not that it lands on it exactly.
  assert.equal(plan.crossings.length, 2, JSON.stringify(plan.crossings));
  for (const [x, y] of plan.crossings) {
    assert.ok(Math.abs(Math.hypot(x - 200, y - 200) - 100) < 0.25, `crossing lies on A: ${x},${y}`);
    assert.ok(Math.abs(Math.hypot(x - 320, y - 200) - 100) < 0.25, `crossing lies on B: ${x},${y}`);
    assert.ok(Math.abs(x - 260) < 0.5, `crossing on the midline: ${x}`);
    assert.ok(Math.abs(Math.abs(y - 200) - 80) < 0.5, `crossing at ±80: ${y}`);
  }
  // Each outline is cut into the arcs between those crossings, and the arcs are
  // the whole outline — the spans partition it.
  for (const source of plan.sources) {
    assert.equal(source.spans.length, 2, `two spans on ${source.name}`);
    const covered = source.spans.reduce((sum, span) => sum + span.length, 0);
    assert.ok(Math.abs(covered - source.spans[0].total) < 1e-6, 'spans cover the outline exactly');
    assert.ok(
      source.spans[0].start.every((v, i) => Math.abs(v - source.spans[1].end[i]) < 1e-6),
      'one span ends where the other begins',
    );
  }
  // Empty space is in no face at all — not in the nearest one.
  const outside = JSON.parse(rengine.smart_fill_plan(JSON.stringify([a, b]), JSON.stringify([10, 10])));
  assert.equal(outside.hit, null, 'a point outside every path identifies no region');
  console.log('smoke[76/79]: RULE 1 — two circles, three regions, spans and crossings from the engine');
}

// 77. **A Smart Fill is parametric** (RULE 2) and **ColorDrop makes one**
//     (RULE 4). The drop creates a *node* pinned to the region — above the
//     boundaries in the draw order, wearing the dropped colour — and moving a
//     boundary re-reads the arrangement instead of replaying stored geometry.
//     Move it far enough and the seed is in no face: the fill is empty and says
//     so with a diagnostic of its own.
{
  const fengine = new glue.VectraEngine();
  const send = (cmd) => JSON.parse(fengine.dispatch_command(JSON.stringify(cmd)));
  const snap = () => JSON.parse(fengine.get_snapshot());
  const circle = (cx, cy, r, name) => {
    const id = crypto.randomUUID();
    send({
      type: 'CreateNode',
      id,
      name,
      kind: { Circle: { cx: { Literal: cx }, cy: { Literal: cy }, radius: { Literal: r } } },
    });
    return id;
  };

  const a = circle(200, 200, 100, 'A');
  const b = circle(320, 200, 100, 'B');
  const fill = crypto.randomUUID();
  const created = send({
    type: 'CreateSmartFill',
    id: fill,
    boundaries: [a, b],
    seed: [260, 200],
    fill: { r: 232, g: 98, b: 44, a: 255 },
    name: 'orange',
  });
  assert.equal(created.status, 'ok', JSON.stringify(created));

  let scene = snap();
  assert.equal(scene.operations[fill].kind, 'smart-fill', 'registered as a region operation');
  assert.equal(scene.operations[fill].description, 'smart fill · 2 boundaries');
  assert.deepEqual(scene.operations[fill].inputs, [a, b], 'the boundaries it reads, not itself');
  const node = scene.scene.nodes[fill];
  assert.ok(node, 'the fill is a real scene node');
  assert.equal(node.name, 'orange');
  assert.equal(node.primitive.type, 'path');
  assert.ok(node.primitive.d.includes('M'), 'with geometry');
  // **RULE 4's paint**: the dropped colour is the fill's own appearance stack.
  assert.equal(node.style.appearances[0].kind, 'fill');
  assert.equal(node.style.appearances[0].paint.color, '#e8622c');
  assert.equal(node.style.fill, '#e8622c', 'and it is the fill the renderer uses');
  // **Above the boundaries** (RULE 4): an operation's result draws after the
  // shapes it reads, which is what "sitting above the boundary paths" means in
  // a painter's-algorithm draw order.
  const order = scene.scene.z_order;
  assert.ok(
    order.indexOf(fill) > order.indexOf(a) && order.indexOf(fill) > order.indexOf(b),
    `the fill draws above its boundaries: ${JSON.stringify(order)}`,
  );

  // Parametric: move B and the *region* moves with it — the fill's geometry is
  // recomputed, not translated.
  const before = node.primitive.d;
  const moved = send({
    type: 'SetParameter',
    node_id: b,
    property: 'cx',
    value: { Float: { Literal: 340 } },
  });
  assert.equal(moved.status, 'ok');
  const dirty = moved.events.filter((e) => e.type === 'Dirty').flatMap((e) => e.ids);
  assert.ok(dirty.includes(fill), `the dependent fill must be dirty: ${JSON.stringify(moved)}`);
  const afterMove = snap().scene.nodes[fill].primitive.d;
  assert.notEqual(afterMove, before, 'the fill re-evaluated');
  // The new lens is *smaller*: the circles are further apart, so a translation
  // would have been wrong — this is the region, re-derived.
  const planNow = JSON.parse(
    fengine.smart_fill_plan(JSON.stringify([a, b]), JSON.stringify([270, 200])),
  );
  const lensNow = planNow.regions.find((region) => region.members.length === 2);
  assert.ok(lensNow && lensNow.area < 8946, `narrower lens after the move: ${lensNow.area}`);

  // Pull them far apart: the *lens* is gone, but the seed is still inside A, and
  // the fill keeps to its seed — it is A's face now, re-derived again, rather
  // than a stale copy of the old lens.
  send({ type: 'SetParameter', node_id: b, property: 'cx', value: { Float: { Literal: 900 } } });
  scene = snap();
  const followed = scene.scene.nodes[fill];
  assert.ok(followed, 'the seed is still enclosed — the fill follows the seed');
  assert.notEqual(followed.primitive.d, afterMove, 'and it is not the old lens');
  const planApart = JSON.parse(
    fengine.smart_fill_plan(JSON.stringify([a, b]), JSON.stringify([260, 200])),
  );
  assert.equal(
    planApart.regions[planApart.hit].members.join(),
    a,
    'the seed resolves to A alone in the new arrangement',
  );
  // Now take *both* boundaries off the seed: no face encloses it any more, and
  // the fill is *empty and says so*.
  send({ type: 'SetParameter', node_id: a, property: 'cx', value: { Float: { Literal: -1000 } } });
  scene = snap();
  assert.equal(scene.scene.nodes[fill], undefined, 'with no region, there is no fill');
  const note = scene.diagnostics.find((d) => d.code === 'smart-fill-empty');
  assert.ok(note, `a diagnostic names the missing region: ${JSON.stringify(scene.diagnostics)}`);
  assert.equal(note.severity, 'warning');
  console.log('smoke[77/79]: RULE 2 + RULE 4 — a drop makes a parametric fill above its boundaries');
}

// 78. **Break Path at Intersections** (RULE 3). A path crossed by another shape
//     has spans; breaking at them mints closed `Path` nodes, hides the source
//     (non-destructively), keeps the areas adding up — and one undo puts the
//     document back exactly.
{
  const pengine = new glue.VectraEngine();
  const send = (cmd) => JSON.parse(pengine.dispatch_command(JSON.stringify(cmd)));
  const snap = () => JSON.parse(pengine.get_snapshot());

  // A square path, and a rectangle overlapping its right half.
  const square = crypto.randomUUID();
  const line = (x, y) => ({ Line: { to: { Literal: { x, y } } } });
  send({
    type: 'CreateNode',
    id: square,
    name: 'square',
    kind: {
      Path: {
        start: { Literal: { x: 0, y: 0 } },
        segments: [line(200, 0), line(200, 200), line(0, 200), 'Close'],
      },
    },
  });
  const bar = crypto.randomUUID();
  send({
    type: 'CreateNode',
    id: bar,
    name: 'bar',
    kind: {
      Rectangle: {
        x: { Literal: 100 },
        y: { Literal: 50 },
        width: { Literal: 200 },
        height: { Literal: 100 },
        corner_radius: { Literal: 0 },
      },
    },
  });

  const before = snap();
  const drawn = before.scene.nodes[square].primitive.d;
  const beforeIds = new Set(Object.keys(before.scene.nodes));
  // The bar is a *participant*: a path's spans are its arcs between crossings
  // with the other paths in the graph, so both are named.
  const plan = JSON.parse(pengine.smart_fill_plan(JSON.stringify([square, bar]), 'null'));
  const source = plan.sources.find((s) => s.id === square);
  assert.ok(source, 'the square is a boundary');
  assert.equal(source.spans.length, 2, 'the bar crosses it twice, so it has two spans');
  assert.ok(Math.abs(source.area - 40000) < 1e-6, `the square's own area: ${source.area}`);
  const all = source.spans.map((span) => [span.from, span.to]);

  // Every span at once: the path becomes its arcs between crossings.
  const reply = JSON.parse(pengine.break_path(square, JSON.stringify(all)));
  assert.equal(reply.status, 'ok', JSON.stringify(reply));
  assert.ok(reply.created, 'the reply names the piece it made');
  const after = snap();
  assert.equal(after.scene.nodes[square].visible, false, 'the source is hidden, not deleted');
  const fresh = Object.keys(after.scene.nodes).filter((id) => !beforeIds.has(id));
  assert.equal(fresh.length, 2, `two pieces from two cuts: ${fresh.length}`);
  for (const id of fresh) {
    const piece = after.scene.nodes[id];
    assert.equal(piece.primitive.type, 'path', 'a piece is an ordinary path node');
    assert.ok(piece.visible && piece.primitive.d.includes('M'), 'with real geometry');
    assert.ok(piece.primitive.d.includes('Z'), `and closed: ${piece.primitive.d}`);
    assert.equal(piece.style.fill, before.scene.nodes[square].style.fill, 'wearing the source paint');
  }
  // **No gaps and no overlap**: the pieces' areas add back up to the source's.
  const piecesPlan = JSON.parse(pengine.smart_fill_plan(JSON.stringify(fresh), 'null'));
  const total = piecesPlan.sources.reduce((sum, s) => sum + s.area, 0);
  assert.ok(
    Math.abs(total - source.area) < 1e-6,
    `the pieces tile the square: ${total} vs ${source.area}`,
  );
  // One undo, and the document is exactly what it was.
  assert.equal(JSON.parse(pengine.undo()).status, 'ok');
  const undone = snap();
  assert.equal(undone.scene.nodes[square].visible, true, 'the source is back on screen');
  assert.deepEqual(Object.keys(undone.scene.nodes).sort(), [...beforeIds].sort(), 'pieces gone');
  assert.equal(undone.scene.nodes[square].primitive.d, drawn, 'byte for byte');
  console.log('smoke[78/79]: RULE 3 — breaking at the crossings yields closed pieces that tile');
}

// 79. **Breaking one span**, and the empty case. A single span splits the path
//     at its two crossings into the span itself and the rest of the ring — the
//     "user selects a span and breaks it" gesture. A path nothing crosses has no
//     span at all, so the engine refuses the break rather than inventing one.
{
  const sengine = new glue.VectraEngine();
  const send = (cmd) => JSON.parse(sengine.dispatch_command(JSON.stringify(cmd)));
  const snap = () => JSON.parse(sengine.get_snapshot());
  const line = (x, y) => ({ Line: { to: { Literal: { x, y } } } });

  const square = crypto.randomUUID();
  send({
    type: 'CreateNode',
    id: square,
    name: 'square',
    kind: {
      Path: {
        start: { Literal: { x: 0, y: 0 } },
        segments: [line(200, 0), line(200, 200), line(0, 200), 'Close'],
      },
    },
  });
  const bar = crypto.randomUUID();
  send({
    type: 'CreateNode',
    id: bar,
    name: 'bar',
    kind: {
      Rectangle: {
        x: { Literal: 100 },
        y: { Literal: 50 },
        width: { Literal: 200 },
        height: { Literal: 100 },
        corner_radius: { Literal: 0 },
      },
    },
  });

  const plan = JSON.parse(sengine.smart_fill_plan(JSON.stringify([square, bar]), 'null'));
  const span = plan.sources.find((s) => s.id === square).spans[0];
  const reply = JSON.parse(sengine.break_path(square, JSON.stringify([[span.from, span.to]])));
  assert.equal(reply.status, 'ok', JSON.stringify(reply));
  const split = snap();
  const fresh = Object.keys(split.scene.nodes).filter((id) => id !== square && id !== bar);
  assert.equal(fresh.length, 2, 'a span splits the path into the span and the rest');
  const areas = JSON.parse(sengine.smart_fill_plan(JSON.stringify(fresh), 'null')).sources.map(
    (s) => s.area,
  );
  assert.ok(
    Math.abs(areas.reduce((x, y) => x + y, 0) - 40000) < 1e-6,
    `the two arcs are the whole square: ${JSON.stringify(areas)}`,
  );

  // **A path nothing crosses has no span at all** — the two cuts are the only
  // reason to cut. The lone circle shares a graph with the crossed square, so
  // the zero is about the circle and not about an empty question.
  const lone = crypto.randomUUID();
  send({
    type: 'CreateNode',
    id: lone,
    name: 'lone',
    kind: {
      Circle: {
        cx: { Literal: 600 },
        cy: { Literal: 600 },
        radius: { Literal: 50 },
      },
    },
  });
  const whole = JSON.parse(
    sengine.smart_fill_plan(JSON.stringify([square, bar, lone]), 'null'),
  );
  const spansOf = (id) => whole.sources.find((s) => s.id === id)?.spans.length ?? -1;
  assert.equal(spansOf(lone), 0, 'nothing crosses the lone circle');
  assert.equal(spansOf(square), 2, 'and in the same graph the square keeps its two');

  // A break at no spans is refused: "nothing to break" is an answer, not an
  // invented cut.
  const refused = JSON.parse(sengine.break_path(lone, '[]'));
  assert.equal(refused.status, 'error', JSON.stringify(refused));
  console.log('smoke[79/79]: RULE 3 — one span, two arcs; nothing to break is refused');
}

console.log(
  'SMOKE PASS: wasm → create → snapshot → bind → expr → undo → typed errors → graph → incremental → patch ≡ rebuild → constraints → drag triad → operations → canvas frames → pointer hits → motion → springs → idle stop → tracks → procedural graph → semantic SVG → parametric React → live-scene export → AI summary → AI commands → self-correction → document serialization (.vectra save path) → pen gestures → brush fitting → Quick Shape snap → direct selection → overlay camera → workspace seed → eye/lock flags → stacked appearances + blends + gradients → artboard jump/frame/export → layer reorder → pan/zoom navigation → nested groups → artboard box editing → arbitrary-depth trees → grouping as one undo → cycle refusal → group blocks in the z-order → cross-layer drops → cross-layer grouping undone → runs, not rows → one placement verb → smart components → make magic → icon studio → one-sentence answers → alpha lock → clipping masks → colordrop fill → parametric text → text on a path → outline to paths → region graph (3 faces) → a parametric smart fill from a drop → break at the crossings',
);
