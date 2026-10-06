# Task 6.0 — The Motion Engine: design

**Status: implemented, verified.** This document records the *design decisions* — what the
shape is, why each alternative was rejected, and where the code that carries the decision lives.
The evidence (test counts, gate output, what was actually run) is in `TASK-6.0-REPORT.md`; the
pre-implementation inventory and the authorized forks are in `TASK-6.0-RECON.md`.

---

## §1 The shape: motion is a *source*, not a system

The one structural decision: **motion is a `Parameter` resolver**, not a subsystem that runs
alongside evaluation. `Parameter::Animated(binding)` has existed since Task 1.1; Task 6.0
implements the evaluator behind it.

```text
Parameter<f64>
 ├── Literal(v)          → v
 ├── Variable(name)      → ctx.variables[name]
 ├── Expression(id)      → ExpressionEngine::evaluate
 ├── Animated(binding)   → MotionEngine::evaluate        ← Task 6.0
 └── Procedural(output)  → (Phase 2)
```

Everything the engine already promises therefore applies to numbers that move, with **no new
machinery**:

| the engine's promise | why motion gets it for free |
| --- | --- |
| dirty propagation is minimal | `Parameter::motion_state_flags()` / `motion_track_id()` feed `dependency_targets`, so a flag flip or a track edit dirties exactly its readers |
| incremental evaluation is a patch | an animation changes a value, and a value change is what the scene cache already knows how to patch |
| the renderer is a consumer | a moving node is a `Dirty` id like any other — one 64-byte instance row |
| undo is a document concept | motion *has no document state to change*, so it cannot pollute the stack |

The alternative — a `MotionSystem` that owns a list of animated slots and ticks them — was
rejected because it needs a second dirty-propagation path, a second lifetime for every binding,
and it makes "which value is on screen at `t`?" a question about the system's history rather
than about the document.

---

## §2 The core decision: `value = f(binding, t)` (F1)

**A spring is a closed-form function of time, not a frame-by-frame integrator.**

```rust
// crates/vectra-motion/src/spring.rs
pub fn value_at(&self, target: f64, t: f64) -> f64
```

`MotionEvaluator::evaluate(&binding, ctx)` receives a binding and a context — never a frame
delta, never a previous state. A spring at rest at `from` from time `at` follows the analytic
step response of `m·ẍ + c·ẋ + k·x = 0` with `x(0) = ẋ(0) = 0`, parameterised the way designers
say it (`stiffness`, `damping`) rather than the way textbooks do (`ω`, `ζ`):

```text
ω = √(stiffness)              ζ = damping / (2√stiffness)

ζ < 1 :  1 − e^(−ζωt)·(cos(ω_d t) + (ζω/ω_d)·sin(ω_d t))      ω_d = ω√(1−ζ²)
ζ = 1 :  1 − e^(−ωt)·(1 + ωt)
ζ > 1 :  1 − e^(−ζωt)·[cosh(ω_r t) + (ζω/ω_r)·sinh(ω_r t)]   ω_r = ω√(ζ²−1)
```

### What that buys, concretely

1. **Purity (Motion Purity Law).** Scrubbing backwards, replaying, two clients on the same
   document and a headless test all agree, because there is nothing to disagree about. An
   integrator makes the document a function of the frame rate: the same edit yields different
   pixels on different machines, and the only test possible is a replay — which tests the
   replay.
2. **A computable idle signal (F3).** Because nothing accumulates, "when will this stop moving?"
   has a closed form (`Spring::settle_time`), so `is_animating()` is a question with an answer,
   not a heuristic. That is what lets the React loop stop.
3. **No velocity to remember.** An interrupted animation is a *re-anchor*, and a re-anchor is a
   document edit (`from`, `at`) — visible, undoable, and exactly what the Interaction Precedence
   Law wants. A velocity-based spring would have to keep that velocity somewhere the document
   does not describe.

### The cost, stated plainly

A spring cannot carry momentum across a target change by itself: it is at `from` with zero
velocity and eases to `target`. Overshoot therefore only happens when the damping ratio allows
it (`ζ < 1`), which is a *documented property with a test* — `no_spring_overshoots_at_or_above_critical_damping`
checks a family of ratios including exactly critical and either side of it, and
`an_under_damped_spring_overshoots_by_the_predicted_ratio` checks the peak against
`1 + e^(−ζπ/√(1−ζ²))`.

### Three numerical decisions that the laws forced

| decision | reason |
| --- | --- |
| over-damped branch written as **two decaying exponentials**, not `cosh`/`sinh` | `cosh(ω_r t)` overflows to `∞` long before `e^(−ζωt)` underflows to `0`, and `∞ × 0` is `NaN` — at k=170, c=40 that happens around t≈55 s, well inside scrubber range. The two-exponential form is exact everywhere and reaches exactly 1 in the limit. |
| `settle_time` solved by **bisection on the same `envelope` function** that `settled_at` tests | the first version solved a *different* function than it asserted, and predicted arrival times that `settled_at` then rejected. One function, two consumers. |
| damping ratio within `1e-6` of 1.0 uses the **critical** branch | `ζ` is derived from user input, so `ζ = 1` exactly is measure-zero, and the critical form is the limit of both neighbours — evaluating the wrong branch near the boundary amplifies a tiny input difference into visibly different motion. |

---

## §3 Interaction precedence and re-anchoring

**A live drag outranks a spring on the same slot.** Both the drag and a state flip resolve the
same way:

```text
slot is sprung
   │
   ├─ drag:   the pointer writes literals for the duration (the gesture owns the slot)
   │          → on EndDrag, the *committed value* re-anchors the spring, which resumes
   │            from where the finger left it. The gesture records ONE history entry
   │            whose forward command is `BindMotion { …reanchor(committed, now) }`,
   │            so redo replays a re-anchored *binding*, not a frozen literal.
   │
   └─ flip:   `set_state(name, on)` reads every affected slot's **live** value first,
              *then* flips, then writes the re-anchored spring as a **session edit**
              (zero history entries: a hover is an input, not an edit).
```

Both go through `vectra_motion::reanchor(binding, value, now)` — a pure function that builds a new
binding and touches nothing. The caller decides undoability, which is the same separation the
constraint solver uses ("solver proposes, engine commits").

Two implementation traps, both now closed by tests:

* **Read the live value, not the static preview.** `Engine::resolve_float` builds a context from
  the document alone, so an `Animated` slot resolves through core's *static-preview* fallback (a
  spring previews at its target, a state branch at its `false` arm). Re-anchoring on that number
  anchors every transition at the value it is already animating toward — nothing ever moves. The
  engine therefore resolves through `resolve_live`, which wires the expression engine *and* the
  motion engine. Caught by `law_states_are_inputs_and_hover_drives_them`.
* **Read before the flip.** The values must be sampled while the *old* flag still applies;
  reading after would anchor at the destination.

---

## §4 States are inputs (F4)

`StateDriven { state, true_value, false_value }` is a **step function** of a host flag.
Deliberately not smoothed inside the evaluator: smoothing needs a start time and a start value,
and both belong to a spring, which the document holds and undo can reach. So a hover transition
is authored as:

```text
Spring { target: StateDriven { hover:<node>, true: on_value, false: off_value }, from, at }
```

…and the engine re-anchors the spring when the flag flips (§3).

Consequences, all tested:

* flags live in the `MotionEngine`, not the document — `set_state` is not a command and creates
  no history;
* setting a flag to the value it already has is a **no-op** (no events, no dirty set);
* a flag that was never set reads `false`, so the document and the host agree on the default
  without either declaring it;
* the **hover flag names its node** (`hover:<uuid>`), which is what lets `pointer_move` decide
  hover from the document rather than from UI state: React reports a position, the engine
  hit-tests, and the engine flips the flags of the nodes whose bindings read them.

Nested smoothing (a spring inside a branch arm) is *not* re-anchored on a flip: the evaluator
cannot invent an anchor for a binding the document does not name. Recorded as a limitation
(§8), not a silent behaviour.

---

## §5 The dependency fix (the bug found in recon)

`dependency_target` (singular, Task 2.2) classified a parameter by its own variant and stopped:

```rust
Parameter::Animated(_) => Some(GraphNode::clock()),   // correct for literal targets only
```

Correct for a binding whose operands are literals, and **silently wrong for every other
binding**: `Spring { target: $radius }` was dirtied by the clock but never by an edit to
`$radius`, so the scene would keep easing toward a stale target with no error and no diagnostic.

Task 6.0 replaces it with `dependency_targets(param) -> Vec<GraphNode>`, which walks the
binding's parameter tree:

```text
Animated(binding) →
    the clock                              (motion samples ctx.time)
  + GraphNode::State(flag)   for each flag the binding reads
  + GraphNode::Track(id)     if it samples a track
  + recurse into every inner Parameter (a spring's target may itself be a spring)
```

Consequences: the graph grew two vertex kinds (`State`, `Track`), the wire export grew two
`GraphNodeView` variants, and the derive loop emits one edge per dependency instead of one per
parameter. Over-approximation is still sound — an extra vertex costs an evaluation, never a
stale scene.

**The regression test is the deliverable, not the fix:**
`law_a_binding_depends_on_its_inner_sources` builds a spring chasing `$radius`, edits `$radius`,
and asserts the sprung node is dirtied. It was verified to **fail against the old behaviour**
(by temporarily removing the recursion) before being accepted.

---

## §6 What is document state, and what is not

| thing | lives in | undoable? | why |
| --- | --- | --- | --- |
| the binding (target, stiffness, damping, anchor `from`/`at`) | the slot, in `Document` | **yes** | it is authored intent |
| keyframe tracks | `Document.motion` (`MotionTrackRegistry`) | **yes** | a track is a value; CRUD is a command |
| the clock | `Engine::time` | no | a session cursor |
| state flags | `MotionEngine` | no | an *input*, like a pointer position |
| sampled values | nowhere | — | recomputed; there is no "current value" to drift |

This table is the Undo Isolation Law in one screen: `undo_depth` is unchanged by 60 frames of
scrubbing, hovering and pointer moves (`law_motion_samples_are_not_history`), and a binding costs
exactly one entry.

---

## §7 Keyframe tracks

* **Linearity is a dependency decision, not an aesthetic one.** A linear segment is determined by
  the two keys that bracket it, so editing one key moves exactly one segment and the dirty
  propagation stays honest. A Catmull-Rom or Bézier track derives tangents from *neighbouring*
  keys — editing a key silently changes segments it does not touch, which is a materially
  different dependency graph. Deferred to Phase 2 with the track-editing UI and its own vertices.
* **The sampler is total**: before the first key it holds the first value, after the last the
  last, and it never returns an error. A scrubber routinely lands before `t=0` of a clip, and
  clamping the *clock* instead would make the scene depend on where the UI thinks the timeline
  starts.
* Validation happens at the boundary (`MotionTrack::validate`: finite, strictly increasing times,
  non-empty channels, non-empty id), so the hot path has no defensive branches.
* A track's id is what the graph keys on, so *editing* a track dirties its readers precisely
  (`law_a_track_edit_dirties_exactly_its_readers`).

---

## §8 Known limitations

1. **Nested springs are not re-anchored** on a state flip (§4). The top-level spring of a slot is
   the re-anchorable unit; a spring inside a branch arm re-anchors only when the document says so.
2. **One interpolation curve** (linear). Keys carry no easing handles yet; the shape of a curve is
   a Phase-2 track-editor concern with its own graph vertices (§7).
3. **`dependency_target` (singular) remains** as a convenience for callers that need "is this
   parametric, and by what?" — with a doc comment saying it must never be used to derive edges.
4. **The settle epsilon is an absolute number** (`1e-3` slot units, configurable), not a fraction
   of the travel. Motion drives document-space floats (a 300-unit width, a 0.8 opacity), so an
   absolute threshold is honest; a 1e9-unit shape would settle "instantly" by this measure.
5. **`StateDriven` has no transition table.** It is a boolean branch; multi-state machines,
   transitions and triggers are MES §12's later half and arrive with the procedural/state work.
6. **The hover driver needs a drawn frame.** The hit index belongs to the renderer's last drawn
   scene (RULE 3), so a canvas that has never drawn has nothing to hover. This is the real mount
   order in the app (draw, then follow the pointer) and is tested.

---

## §9 Code map

| file | what it carries |
| --- | --- |
| `crates/vectra-motion/src/spring.rs` | the three closed forms, `SpringDynamics`, `settle_time`, the envelope, 9 unit tests |
| `crates/vectra-motion/src/track.rs` | `sample_channel` / `sample_with_segment`, totality, 4 unit tests |
| `crates/vectra-motion/src/lib.rs` | `MotionEngine` (tracks + states + epsilon), `impl MotionEvaluator`, `reanchor`, `status`/`is_animating`, `own_spring`, `track_usage`, 6 unit tests |
| `crates/vectra-motion/tests/motion_laws.rs` | the seven laws at the engine boundary |
| `crates/vectra-dependency/src/graph.rs` | `dependency_targets` + the walk + `State`/`Track` vertices |
| `crates/vectra-dependency/tests/motion_graph_laws.rs` | the regression test and the graph-level laws |
| `crates/vectra-core/src/{param,document,command,ids,engine}.rs` | the anchor, `MotionTrack`/registry, the three commands, `parse_node_id`, `history_depth` |
| `crates/vectra-wasm/src/lib.rs` | `motion` field, `with_motion` on every context, `resolve_live`, `flip_state`, the motion surface, `MotionReportWire` |
| `crates/vectra-wasm/src/render.rs` | `Renderer::hit_test` (document-space, for the hover driver) |
| `crates/vectra-wasm/tests/motion_laws.rs` | the same seven laws through the wasm boundary and the real frame loop |
| `apps/vectra-web/src/engine/{wire,client,commands,view-model}.ts` | motion wire types, client methods, track builders, the inspector projection |
| `apps/vectra-web/src/App.tsx` | hover driver, motion panel, and a frame loop that stops |
