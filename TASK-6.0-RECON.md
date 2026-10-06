# Task 6.0 — The Motion Engine: reconnaissance (pre-authorization)

**Status: nothing implemented.** This document exists so the authorization prompt can be
written against the code as it is, not against a guess about it. It is the evidence for the
recommendation (Option A), the inventory of what is already soldered, the four decisions the
prompt must settle, and the laws I would hold the work to.

Companion documents: `TASK-5.0-REPORT.md` / `TASK-5.0-DESIGN.md` (the renderer this rides on),
`MES.md` §12 (Motion API).

---

## 0. The verdict, and why

| | task | MES phase label (from the crate's own docs) | verdict |
| --- | --- | --- | --- |
| **A** | 6.0 Motion engine | `vectra-motion`: *"implemented in Phase **1** motion milestone"* | **do this now** |
| C | 7.0 Procedural graph | `vectra-procedural`: *"Phase **2** procedural milestone"* | next frontier, biggest scope |
| B | 8.0 Export / codegen | `vectra-export`: *"Phase **2** export milestone"* | do it **after** the model stops growing |

The MES's own phase labels are decisive: we are still in Phase 1, and Motion is the only one
of the three the spec places there. Beyond that, three engineering arguments:

1. **The socket is already soldered.** `Parameter::Animated`, the §12 `MotionBinding` variants,
   the `MotionEvaluator` trait, the context slot, the static-preview fallbacks, the clock
   vertex in the dependency graph, `set_time`'s dirty propagation, and — since Task 5.0 — a
   renderer that ingests per-frame dirty sets and a React loop that runs per frame. Motion is
   a plug, not a new wing of the building. (Full inventory in §1.)
2. **It is the highest capability jump per unit of new machinery.** Springs and state-driven
   values are what make the engine *interactive* — which is the product thesis, and the thing
   that separates it from a drawing tool that happens to have parameters.
3. **It validates Task 5.0 under real load.** Today the incrementality witness is "40 drag
   samples, 1 write". Motion makes it "60 frames a second of three springs, N bytes a frame",
   and folds in Task 5.0's open item #4 (the unconditional `requestAnimationFrame` loop):
   a settled animation *must* stop the loop, which is the Idle Law below.

**Why not B now (honest case).** Export is cheap, low-risk, ships a tangible artifact, and
touches no engine semantics — if the goal were "maximum demo value this week", it wins. The
argument against it now is timing, not merit: the document model is about to gain two new node
categories (motion bindings, then procedural nodes), and an export IR written before them must
be revisited twice. Export wants to be last so it can be complete.

**Why not C now.** Procedural is the largest scope (node kinds + graph store + evaluation +
editor UI + noise/smoothing math under the "don't hand-roll it" rule), and it composes with
motion — an animated procedural parameter is where the two pay off together. It is a better
Task 6.1 than a Task 6.0.

---

## 1. The socket that is already soldered

| piece | where | state |
| --- | --- | --- |
| `MotionBinding::{KeyframeTrack, Spring, StateDriven}` | `vectra-core/src/param.rs:65` | exactly §12's shape, including `Spring { target: Box<Parameter<f64>>, stiffness, damping }` |
| `Parameter::Animated(MotionBinding)` | `vectra-core/src/param.rs:96` | the parameter variant exists and round-trips (`source_tag() == "animated"`) |
| `trait MotionEvaluator` | `vectra-core/src/eval.rs:53` | `evaluate(time, binding) -> Result<f64, _>` — pure in `time` by signature |
| `EvaluationContext.motion` + `with_motion` | `vectra-core/src/eval.rs:99, 121` | the injection point exists and is unused |
| **static-preview fallbacks** | `vectra-core/src/eval.rs:161–175` | spring → target; state → false arm; keyframe → typed `MissingMotionEvaluator`. **Already implemented and unit-tested** (`eval.rs:274`) |
| typed errors | `vectra-core/src/error.rs:25, 30` | `MissingMotionEvaluator`, `UnsupportedAnimatedTarget` |
| clock vertex | `vectra-dependency/src/graph.rs:76` | `GraphNode::clock()` |
| animated → clock edge | `vectra-dependency/src/graph.rs:843` | `Parameter::Animated(_) => Some(GraphNode::clock())`, with a graph test at `:1111` |
| `Engine::set_time` | `vectra-core/src/engine.rs:88` | absolute time into the document |
| time → dirty → scene | `vectra-wasm/src/lib.rs:501` | `set_time` = clock dirty ids → `patch` → operations → **`dirty_event`**, i.e. already into the Task 5.0 `DirtyLedger` |
| the frame loop | `apps/vectra-web/src/App.tsx` (rAF effect), `src/engine/client.ts` (`setTime`) | a scrubber and a `time` readout already exist |
| the crate | `crates/vectra-motion/src/lib.rs` | 17-line placeholder whose doc names the trait it must implement |

## 2. What is actually missing

1. **The evaluator** — `vectra-motion`'s `impl MotionEvaluator` for springs, state branches and
   keyframe tracks (with the track store), plus the integrator and its determinism story (§3 F1).
2. **A binding command** — nothing can *create* a `Parameter::Animated`. Needs
   `BindMotion` / `UnbindMotion` (or `SetMotion`) with exact inverses, the same undo discipline
   as every other document change.
3. **A clock discipline** — `set_time` accepts an absolute time, but nothing advances it, and
   nothing knows when to *stop* asking for frames (§3 F3).
4. **A state input surface** — hover/active must arrive as an event from the UI, deduped, and
   deliberately **not** undoable (§3 F4). The reserved socket is
   `Parameter::Interaction` / `InteractionProvider` (`eval.rs:76, 181`), currently `None`.
5. **Graph edges for a binding's inner parameters** — see §6.1; this is the first real bug.
6. **Recursion guard** — `Spring { target: Box<Parameter<f64>> }` is recursive by construction
   (`param.rs:63` documents the cycle), so a target that (transitively) reads the spring must be
   refused by the same pre-application cycle gate as expression and constraint edges.

## 3. The four forks the prompt must settle

**F1 — `evaluate(t)` must be pure.** The same `t` must yield a byte-identical scene whether the
user scrubbed forward, scrubbed backward, or jumped. That is what makes scrubbing, undo, and
headless tests possible, and it is why §12's signature takes `time` rather than `dt`. It forces
a choice for springs: a **closed form** (critically damped has a clean one; general ζ is a
decaying oscillation) or a **fixed-step replay from an anchor** recorded when the spring was
bound/re-anchored. Either is defensible; what is not defensible is integrating by frame delta,
which makes the document depend on the frame rate.

**F2 — motion resolves, it never writes.** Unlike `EndDrag` (Task 3.2, which *finalises
literals* — that is what makes a gesture undoable), a spring must not mutate the document:
motion samples are not edits, they add **zero** history entries, and the value exists only in
the `EvaluationContext`. Consequence: interaction precedence — a live drag outranks a spring on
the same slot, and on release the spring **re-anchors at the committed value** so there is no
visual jump (the same "solver proposes, engine commits" discipline, rotated 90°).

**F3 — who ticks, and when it stops.** The engine must not own a wall clock (headless tests,
determinism). The UI advances absolute `t`; the engine answers with dirty ids. What is missing
is the **idle signal**: `is_animating()` must be false when every spring is asleep and no state
transition is in flight, so the React loop can stop scheduling frames. Without it we ship a
permanent 60 fps drain — which is also Task 5.0's open item #4.

**F4 — what `state` is.** `StateDriven { state, true_value, false_value }` names a state; the
input is a small set of UI-driven booleans per node (`hover`, `active`). It arrives as an event
(command), is deduped (setting the state it already has is a no-op and dirties nothing), and is
**not** undoable — a hover is an input, like a pointer sample, not a document edit.

## 4. The laws I would write

| law | statement |
| --- | --- |
| **Motion Purity Law** | same `t` ⇒ byte-identical `EvaluatedScene`, whatever path the clock took to get there (forward, backward, jump, replay) |
| **Spring Convergence Law** | a spring reaches its target within tolerance and stays; for ζ ≥ 1 it never overshoots (monotone in the settling direction) |
| **Idle Law** | a settled animation produces empty dirty sets, **zero** document mutations, zero history entries, `is_animating() == false`, and **zero GPU bytes** |
| **Interaction Precedence Law** | a drag outranks a spring on the same slot; across `EndDrag` the value is continuous (‖Δ‖ ≤ ε, no jump) and the spring resumes toward the target from the committed value |
| **State Law** | `StateDriven` is a step function of the input, deterministic under replay; with no motion crate wired it previews the **false** arm (already true — keep it) |
| **Undo Isolation Law** | N seconds of animation add zero history entries; after "animate, then edit", one undo reverts the **edit**, never a motion sample |
| **Frame Budget Law** | K nodes moving by position alone ⇒ ≤ K × 64 B per frame through Task 5.0's ledger/`MockSink`; settled ⇒ 0 writes; a reshape mid-animation re-tessellates only that node |

## 5. The touch list

* `crates/vectra-motion/src/` — real crate: `spring.rs`, `state.rs`, `track.rs`, `evaluator.rs`, `error.rs`
* `crates/vectra-core/src/param.rs` — (only if the binding needs a field the API lacks; the enum itself is done)
* `crates/vectra-core/src/command.rs` — `BindMotion` / `UnbindMotion` + inverses
* `crates/vectra-dependency/src/graph.rs` — **`dependency_target` must recurse into a binding's
  inner parameters** (§6.1), plus the cycle gate for recursive springs
* `crates/vectra-wasm/src/lib.rs` — wire `with_motion`, add `SetState` / `Tick`-shaped surface,
  `is_animating()`, keep `settle`'s funnel
* `crates/vectra-render` — no changes expected; the Frame Budget Law is asserted through the
  existing ledger and `MockSink`
* `apps/vectra-web/src/{App.tsx,App.css}`, `src/engine/{client,commands,wire}.ts` — bind/unbind a
  spring on a selected slot, a hover/active toggle per layer (to drive states), and a frame loop
  that **stops** when idle
* tests: `crates/vectra-motion/tests/motion_laws.rs`, `crates/vectra-wasm/tests/motion_laws.rs`,
  smoke steps 27+

## 6. Risks, ranked

1. **The silent stale-scene bug (§2.5).** `dependency_target` returns one vertex and does not
   descend into `MotionBinding`'s inner `Parameter<f64>`. So `Spring { target: $radius }` is
   dirtied by the clock but **not** by an edit to `$radius`: the spring would keep easing toward
   the old target until something else dirtied the node. The fix (`dependency_targets -> Vec<_>`,
   or a binding-aware derive) is small, but the failure mode is invisible without a test that
   edits `$radius` while a spring targets it — so that test is part of the deliverable, not a
   nice-to-have.
2. **Determinism vs frame rate (F1)** — the one that can make the result ugly and untestable.
3. **Recursive springs** (`Spring { target: spring-bound parameter }`) — needs the cycle gate
   Task 2.2 established, not a new one.
4. **Idle detection** — a spring that never quite settles ("asleep" threshold) can pin the loop
   at 60 fps forever; the threshold and the snap-to-target rule must be explicit and tested.
5. **Undo interactions** — binding is a document change (undoable); motion samples are not.
   An undo that removes the binding mid-animation must leave no orphaned evaluator state.

## 7. What "done" looks like

* All seven laws green, plus the engine-level and wire-level versions of each.
* `cargo fmt` / `clippy -D warnings` / `test --workspace` clean; 27+ targets.
* Smoke grown past 26 steps (bind a spring → advance time → the node moves incrementally →
  the value settles → the loop reports idle).
* The UI can bind a spring to a selected slot, drive a state, and *show* the frame budget while
  animating.
* `TASK-6.0-DESIGN.md` + `TASK-6.0-REPORT.md`, with the purity decision (F1) argued explicitly.

## 8. What I need to start

The authorized prompt — or "proceed" under the standing rule, in which case I will execute the
above in this order: §6.1 (the graph fix, with its regression test) → the crate and its laws →
the commands and undo → the wasm surface + idle signal → the UI. One request either way: F1 is
the decision that constrains everything else, so if the prompt leaves it open I will choose
**fixed-step replay from an anchor** (cheap, exact for every ζ, and the anchor is exactly the
value `EndDrag` already commits) and document the choice in the design doc.
