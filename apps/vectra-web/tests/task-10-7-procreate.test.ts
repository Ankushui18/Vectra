/**
 * **Task 10.7's laws at the UI boundary** — the Gesture Law, the StreamLine Law
 * and ColorDrop's rule, all without a browser.
 *
 * The three modules under test (`engine/draw/touch.ts`, `streamline.ts`,
 * `colordrop.ts`) are deliberately pure: no DOM, no WASM, no React. That is what
 * makes it possible to state the brief's three laws as *executable* properties
 * rather than as a description of a feeling:
 *
 * * **Gesture Law** — a two-finger tap becomes Undo, a three-finger tap becomes
 *   Redo, a three-finger swipe down becomes the copy/paste, and *every* firing
 *   decision — for all three gestures, over a randomized sweep of hands — is
 *   *swallowed* (the caller calls `preventDefault`), so the browser never
 *   scrolls or opens a menu while a designer is undoing.
 * * **StreamLine Law** — a noisy stroke through the filter is measurably
 *   smoother (total turning and the variance of the hand's jitter are both
 *   lower, by a wide margin at high amounts), the
 *   smoothing is monotone in the slider, the filter never lags more than its cap,
 *   and 0 % is byte-identical to raw input.
 * * **ColorDrop Law** — a drop on a shape fills it, a drop in empty space does
 *   nothing at all, and a node with no fill gets one rather than silently
 *   ignoring the drop.
 *
 * Run: `npm run test:ui`.
 */
import assert from 'node:assert/strict';
import { test } from 'node:test';

import {
  modifierClick,
  SWIPE_MIN_PX,
  TAP_MAX_MS,
  TAP_SLOP_PX,
  TouchRouter,
} from '../src/engine/draw/touch';
import {
  pathJitter,
  pathTurns,
  StreamLine,
  STREAMLINE_MAX_LAG_PX,
  STREAMLINE_MAX_WINDOW,
  streamlineAmount,
  streamlineSummary,
} from '../src/engine/draw/streamline';
import {
  dropChangesStack,
  dropStatus,
  isHexColor,
  planColorDrop,
  stackWithColor,
} from '../src/engine/draw/colordrop';
import type { SnapshotAppearanceWire } from '../src/engine/wire';

/** A tiny deterministic generator: the laws must be reproducible, so no RNG. */
function sequence(seed: number): () => number {
  let state = seed >>> 0;
  return () => {
    // xorshift32
    state ^= state << 13;
    state >>>= 0;
    state ^= state >> 17;
    state ^= state << 5;
    state >>>= 0;
    return state / 0x1_0000_0000;
  };
}

// ── RULE 1: the Gesture Law ────────────────────────────────────────────────

/**
 * Feed one whole tap through the router and answer what it decided.
 *
 * `wobble` is the *radius* the finger roams while it is down: a real hand never
 * holds still, and the law has to say that a tap with a hand's own shiver on it
 * is still a tap — while a finger that genuinely travels is a drag.
 */
function tap(fingers: number, router: TouchRouter, wobble: number, held: number): string {
  const start = 100;
  const ids = Array.from({ length: fingers }, (_, index) => index + 1);
  const roam = (index: number) => ({
    pointerId: ids[index],
    clientX: start + index * 40 + wobble,
    clientY: start + wobble * 0.35,
  });
  for (const [index, id] of ids.entries()) {
    router.down({ pointerId: id, clientX: start + index * 40, clientY: start }, 0);
  }
  for (let index = 0; index < ids.length; index += 1) {
    router.move(roam(index), held / 2);
  }
  let action = 'none';
  for (let index = 0; index < ids.length; index += 1) {
    const decision = router.up(roam(index), held);
    if (decision.action !== 'none') action = decision.action;
  }
  router.reset();
  return action;
}

test('Gesture Law: a two-finger tap is Undo, whatever the hand wobbles', () => {
  const random = sequence(0x2f6e2b1);
  const router = new TouchRouter();
  for (let run = 0; run < 200; run += 1) {
    const wobble = random() * (TAP_SLOP_PX - 1);
    const held = random() * TAP_MAX_MS;
    assert.equal(tap(2, router, wobble, held), 'undo', `run ${run} (wobble ${wobble}, held ${held})`);
  }
});

test('Gesture Law: a three-finger tap is Redo', () => {
  const random = sequence(0x51ed1);
  const router = new TouchRouter();
  for (let run = 0; run < 200; run += 1) {
    const wobble = random() * (TAP_SLOP_PX - 1);
    assert.equal(tap(3, router, wobble, random() * TAP_MAX_MS), 'redo', `run ${run}`);
  }
});

test('Gesture Law: a slow two-finger press is not a tap', () => {
  const router = new TouchRouter();
  assert.equal(tap(2, router, 0, TAP_MAX_MS + 1), 'none');
});

test('Gesture Law: two fingers that travel are a drag, not a tap — no undo by accident', () => {
  const router = new TouchRouter();
  assert.equal(tap(2, router, TAP_SLOP_PX + 4, 100), 'none', 'a travelling hand');
});

test('Gesture Law: one finger never fires a gesture (a stroke is not a command)', () => {
  const router = new TouchRouter();
  assert.equal(tap(1, router, 2, 120), 'none');
  // …even a long, wandering one.
  const other = new TouchRouter();
  other.down({ pointerId: 1, clientX: 0, clientY: 0 }, 0);
  for (let step = 0; step < 50; step += 1) {
    other.move({ pointerId: 1, clientX: step * 10, clientY: step * 4 }, step * 10);
  }
  assert.equal(other.up({ pointerId: 1, clientX: 500, clientY: 200 }, 600).action, 'none');
});

test('Gesture Law: three fingers dragged down copy/paste — once, and swallowed', () => {
  const router = new TouchRouter();
  for (const id of [1, 2, 3]) {
    router.down({ pointerId: id, clientX: 100 + id * 30, clientY: 100 }, 0);
  }
  // The swipe fires while the hand is still down: Procreate answers the gesture,
  // not the release.
  const decision = router.move({ pointerId: 2, clientX: 160, clientY: 100 + SWIPE_MIN_PX + 1 }, 40);
  assert.equal(decision.action, 'copyPaste');
  assert.equal(decision.swallow, true, 'the browser must not see this as a scroll');
  // A second move does not fire a second paste.
  assert.equal(router.move({ pointerId: 2, clientX: 160, clientY: 400 }, 60).action, 'none');
});

test('Gesture Law: a sideways three-finger drag is not a copy/paste', () => {
  const router = new TouchRouter();
  for (const id of [1, 2, 3]) {
    router.down({ pointerId: id, clientX: 100 + id * 30, clientY: 100 }, 0);
  }
  const decision = router.move({ pointerId: 2, clientX: 100 + 30 * 3 + 200, clientY: 120 }, 40);
  assert.equal(decision.action, 'none', 'a cancelled tap must never undo');
});

test('Gesture Law: from the second finger on, the canvas owns the event', () => {
  const router = new TouchRouter();
  const first = router.down({ pointerId: 1, clientX: 10, clientY: 10 }, 0);
  assert.equal(first.swallow, false, 'one finger is a stroke: the tool gets it');
  const second = router.down({ pointerId: 2, clientX: 50, clientY: 10 }, 10);
  assert.equal(second.swallow, true);
  assert.equal(second.contacts, 2);
});

test('Gesture Law: a lost contact fires nothing — a cancelled gesture never undoes', () => {
  const router = new TouchRouter();
  router.down({ pointerId: 1, clientX: 10, clientY: 10 }, 0);
  router.down({ pointerId: 2, clientX: 50, clientY: 10 }, 5);
  router.cancel({ pointerId: 2, clientX: 50, clientY: 10 });
  const decision = router.up({ pointerId: 1, clientX: 10, clientY: 10 }, 100);
  assert.equal(decision.action, 'none');
});

test('Gesture Law: Ctrl+Click is Undo and Ctrl+Shift+Click is Redo', () => {
  assert.equal(modifierClick({ ctrlKey: true, shiftKey: false, button: 0 }), 'undo');
  assert.equal(modifierClick({ ctrlKey: true, shiftKey: true, button: 0 }), 'redo');
  assert.equal(modifierClick({ ctrlKey: false, shiftKey: false, button: 0 }), 'none');
  assert.equal(modifierClick({ ctrlKey: true, shiftKey: false, button: 2 }), 'none', 'right-click keeps its menu');
  assert.equal(
    modifierClick({ ctrlKey: true, shiftKey: false, button: 0, dragged: true }),
    'none',
    'Ctrl-drag is not a click (and Alt-drag is the pen’s)',
  );
});

test('Gesture Law: every firing decision is swallowed — this is the no-scroll law', () => {
  // The brief's CRITICAL clause, as a universal over hands rather than over two
  // hand-picked examples: **any** decision the router ever returns carrying an
  // action also carries `swallow`, which is what App.tsx turns into
  // `event.preventDefault()` (three call sites: the key handler, the canvas
  // pointer-down, and the draw move). Covering every gesture matters most for
  // the three-finger swipe — that is the one the browser would otherwise read
  // as a page scroll, and it fires mid-drag rather than on release.
  //
  // A hand is generated per trial: 2-4 contacts, a random spread of positions,
  // and either a tap (stays inside the slop) or a swipe (travels past the
  // threshold, with sideways drift that stays legal). Every decision the router
  // produces is inspected, and the sweep counts its firings so it cannot pass
  // vacuously — the assertion that all three gestures fired proves the
  // universal actually covered each of them.
  const random = sequence(0x9e5c);
  const firings = new Map<string, number>();
  let decisions = 0;
  for (let trial = 0; trial < 300; trial += 1) {
    const router = new TouchRouter();
    const fingers = 2 + Math.floor(random() * 3); // 2, 3 or 4
    const swipe = fingers >= 3 && random() < 0.5;
    const originX = 100 + random() * 400;
    const originY = 100 + random() * 400;
    const ids = Array.from({ length: fingers }, (_, i) => i + 1);
    const spread = () =>
      (random() - 0.5) * 2 * (swipe ? 10 : 4); // sideways drift, legal either way
    const at = (id: number, dy: number) => ({
      pointerId: id,
      clientX: originX + spread(),
      clientY: originY + dy,
    });
    const feed = (decision: { action: string; swallow: boolean }) => {
      decisions += 1;
      if (decision.action !== 'none') {
        assert.equal(
          decision.swallow,
          true,
          `a firing ${decision.action} must be swallowed (trial ${trial})`,
        );
        firings.set(decision.action, (firings.get(decision.action) ?? 0) + 1);
      }
    };
    // The hand lands, staggered by a few milliseconds.
    for (const id of ids) feed(router.down(at(id, 0), random() * 15));
    if (swipe) {
      // …and drags down past the threshold, in a few steps.
      const travel = SWIPE_MIN_PX + 4 + random() * 120;
      for (let step = 1; step <= 4; step += 1) {
        const dy = (travel * step) / 4;
        for (const id of ids) feed(router.move(at(id, dy), 40 + step * 20));
      }
    } else {
      // A tap wobbles inside the slop and lifts before the clock runs out.
      // A wobble well inside TAP_SLOP_PX: two draws of ±4 plus ±3 of travel
      // stay under the 12 px the recogniser allows, so the tap does fire.
      for (const id of ids) feed(router.move(at(id, (random() - 0.5) * 6), 60));
    }
    const liftAt = swipe ? 400 : 60 + random() * 180; // < TAP_MAX_MS
    for (const id of ids) feed(router.up(at(id, swipe ? 200 : 0), liftAt + id));
    assert.ok(decisions > 0, 'the sweep must actually feed the router');
  }
  for (const gesture of ['undo', 'redo', 'copyPaste']) {
    assert.ok(
      (firings.get(gesture) ?? 0) > 0,
      `the sweep must have produced a ${gesture} to test the swallow contract`,
    );
  }
  assert.ok(decisions > 1000, `the sweep must be substantial (${decisions} decisions)`);
});

// ── RULE 2: the StreamLine Law ─────────────────────────────────────────────

/** A straight line with the hand's own shiver on top of it. */
function noisyLine(samples: number, amplitude: number, seed: number): Array<{ x: number; y: number }> {
  const random = sequence(seed);
  const points = [];
  for (let step = 0; step < samples; step += 1) {
    points.push({
      x: step * 4,
      y: 200 + (random() - 0.5) * 2 * amplitude,
    });
  }
  return points;
}

function filter(points: Array<{ x: number; y: number }>, amount: number) {
  const line = new StreamLine(amount);
  return points.map((point) => line.push(point));
}

test('StreamLine Law: the filter smooths a shaky hand, and more is smoother', () => {
  const raw = noisyLine(120, 6, 0x5eed);
  const rawTurns = pathTurns(raw);
  assert.ok(rawTurns !== null && rawTurns > 0, 'the noisy line must actually turn');
  let previous = Number.POSITIVE_INFINITY;
  for (const amount of [0.25, 0.5, 0.75, 1]) {
    const turns = pathTurns(filter(raw, amount));
    assert.ok(turns !== null, 'the filtered line has enough points');
    assert.ok(
      turns <= rawTurns * 0.75,
      `amount ${amount} must be significantly smoother (${turns} vs ${rawTurns})`,
    );
    assert.ok(turns <= previous, `more streamline must never be less smooth (${amount})`);
    previous = turns;
  }
  // The headline number the report quotes: at full strength the path's turning
  // is a small fraction of the hand's.
  const full = pathTurns(filter(raw, 1))!;
  assert.ok(full < rawTurns * 0.5, `${full} should be far below ${rawTurns}`);
});

test('StreamLine Law: 0 % is byte-identical to raw input', () => {
  const raw = noisyLine(40, 9, 0xabc);
  assert.deepEqual(filter(raw, 0), raw);
  const line = new StreamLine(0);
  line.push({ x: 1, y: 2 });
  assert.deepEqual(line.flush(), [], 'nothing to add when nothing lagged');
  assert.equal(line.window, 1);
  assert.equal(line.lagCap, 0);
});

test('StreamLine Law: the lag never exceeds the cap the slider promises', () => {
  for (const amount of [0.2, 0.5, 1]) {
    const line = new StreamLine(amount);
    const cap = amount * STREAMLINE_MAX_LAG_PX;
    for (const point of noisyLine(80, 30, 0x1234 + Math.round(amount * 10))) {
      const filtered = line.push(point);
      const lag = Math.hypot(filtered.x - point.x, filtered.y - point.y);
      assert.ok(lag <= cap + 1e-9, `lag ${lag} exceeds the cap ${cap}`);
    }
    assert.equal(line.lagCap, cap);
  }
});

test('StreamLine Law: the smoothed path has lower variance than the hand', () => {
  // The brief's literal wording. Turning (§ above) is the better proxy for
  // "jagged"; this is the variance itself — the mean squared second difference,
  // which is blind to the filter's lag and reads only the shake.
  //
  // Three different hands, because a law that only holds for one seed is a
  // coincidence: the filter has no idea which noise it is being fed.
  for (const seed of [0x5eed, 0x1234, 0xbeef]) {
    const raw = noisyLine(120, 6, seed);
    const rawJitter = pathJitter(raw);
    assert.ok(rawJitter !== null && rawJitter > 0, 'the noisy line must actually shake');
    let previous = Number.POSITIVE_INFINITY;
    for (const amount of [0.25, 0.5, 0.75, 1]) {
      const jitter = pathJitter(filter(raw, amount));
      assert.ok(jitter !== null, 'the filtered line has enough points');
      assert.ok(
        jitter <= rawJitter * 0.5,
        `amount ${amount} must significantly lower the variance (${jitter} vs ${rawJitter})`,
      );
      assert.ok(
        jitter <= previous,
        `more streamline must never raise the variance (${amount})`,
      );
      previous = jitter;
    }
    // At full strength the hand's shake is largely gone: ~9.7 % of the raw
    // variance on all three seeds, so the bound is stated with room to spare.
    const full = pathJitter(filter(raw, 1))!;
    assert.ok(full <= rawJitter * 0.2, `full-strength variance ${full} vs raw ${rawJitter}`);
  }
});

test('StreamLine Law: a tap is a tap — one sample passes through untouched', () => {
  for (const amount of [0, 0.3, 1]) {
    const line = new StreamLine(amount);
    assert.deepEqual(line.push({ x: 42, y: 7 }), { x: 42, y: 7 });
  }
});

test('StreamLine Law: the tail reaches where the hand stopped', () => {
  const line = new StreamLine(1);
  const raw = noisyLine(60, 8, 0x77);
  for (const point of raw) line.push(point);
  const tail = line.flush();
  assert.ok(tail.length >= 2, 'a lagging filter owes the stroke a tail');
  const last = raw[raw.length - 1];
  assert.deepEqual(tail[tail.length - 1], last, 'the stroke must end under the hand');
});

test('StreamLine Law: the summary names the same numbers the mathematics uses', () => {
  assert.match(streamlineSummary(0), /raw input/);
  const summary = streamlineSummary(100);
  assert.match(summary, new RegExp(`${STREAMLINE_MAX_WINDOW}-sample pull`));
  assert.match(summary, new RegExp(`≤${STREAMLINE_MAX_LAG_PX}px lag`));
  assert.equal(streamlineAmount(-5), 0);
  assert.equal(streamlineAmount(500), 1);
  assert.equal(streamlineAmount(Number.NaN), 0);
});

// ── RULE 4: ColorDrop ──────────────────────────────────────────────────────

const fillRow = (color: string): SnapshotAppearanceWire => ({
  kind: 'fill',
  paint: { type: 'solid', color },
  opacity: 1,
  blend: 'normal',
  visible: true,
  width: null,
});

test('ColorDrop Law: a drop on a shape fills it, a drop in space does nothing', () => {
  assert.deepEqual(planColorDrop('node-1', '#ff0000'), {
    kind: 'fill',
    nodeId: 'node-1',
    color: '#ff0000',
  });
  assert.equal(planColorDrop(null, '#ff0000').kind, 'noop');
  assert.equal(planColorDrop('node-1', 'not a colour').kind, 'noop');
  assert.equal(planColorDrop('node-1', '#ff0').kind, 'noop', 'three-digit hex is not the wire shape');
  assert.equal(isHexColor('#00ff0080'), true);
  assert.match(dropStatus({ kind: 'noop', reason: 'empty space — nothing to fill' }, null), /empty space/);
  assert.match(dropStatus({ kind: 'fill', nodeId: 'n', color: '#123456' }, 'Square'), /filled Square/);
});

test('ColorDrop Law: the drop recolours the fill the designer sees', () => {
  const stack = [fillRow('#111111'), { ...fillRow('#222222'), visible: false }];
  const dropped = stackWithColor(stack, '#abcdef');
  assert.equal(dropped[0].paint.type, 'solid');
  assert.deepEqual(
    dropped[0].paint,
    { type: 'solid', color: '#abcdef' },
    'the bottom fill is the one a drop replaces',
  );
  assert.deepEqual(dropped[1], stack[1], 'the rest of the stack is echoed back verbatim');
  assert.deepEqual(stack[0].paint, { type: 'solid', color: '#111111' }, 'the input is not mutated');
});

test('ColorDrop Law: a shape with no fill gets one', () => {
  const dropped = stackWithColor([], '#00ff00');
  assert.equal(dropped.length, 1);
  assert.equal(dropped[0].kind, 'fill');
  const empty = dropChangesStack([], '#00ff00');
  assert.equal(empty, true);
});

test('ColorDrop Law: a drop of the colour the shape already has is not a history entry', () => {
  assert.equal(dropChangesStack([fillRow('#ABCDEF')], '#abcdef'), false);
  assert.equal(dropChangesStack([fillRow('#111111')], '#abcdef'), true);
  // A gradient keeps its identity: the drop adds a colour underneath instead.
  const gradient: SnapshotAppearanceWire = {
    kind: 'fill',
    paint: {
      type: 'linear',
      start: [0, 0],
      end: [10, 10],
      stops: [
        { offset: 0, color: '#000000' },
        { offset: 1, color: '#ffffff' },
      ],
    },
    opacity: 1,
    blend: 'normal',
    visible: true,
    width: null,
  };
  const withGradient = stackWithColor([gradient], '#123456');
  assert.equal(withGradient.length, 2);
  assert.deepEqual(withGradient[1], gradient, 'the gradient survives the drop');
});
