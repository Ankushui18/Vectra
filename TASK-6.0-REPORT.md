# Task 6.0 — The Motion Engine: report

**Status: complete.** All seven laws hold, at the engine boundary *and* through the compiled wasm
module. The workspace gate is clean, and every number below was produced by a command in §8 on
this machine after the last edit. The design rationale is in `TASK-6.0-DESIGN.md`; the
pre-implementation inventory and the four authorized forks are in `TASK-6.0-RECON.md`.

---

## 1. What was built, and where

| layer | artifact | lines | what it is |
| --- | --- | --- | --- |
| motion | `crates/vectra-motion/src/spring.rs` | 486 | the closed-form step response, `SpringDynamics`, `settle_time`, the settle envelope, 9 unit tests |
| | `crates/vectra-motion/src/track.rs` | 145 | keyframe sampling: bracketing segment, clamping, totality, 4 unit tests |
| | `crates/vectra-motion/src/lib.rs` | 577 | `MotionEngine` (tracks, state flags, epsilon), `impl MotionEvaluator`, `reanchor`, `status`/`is_animating`/`value_of`, `own_spring`, `track_usage`, 6 unit tests |
| graph | `crates/vectra-dependency/src/graph.rs` | — | `dependency_targets` + `State`/`Track` vertices; the recursion into a binding's inner parameters |
| | `crates/vectra-dependency/tests/motion_graph_laws.rs` | 373 | the regression test for the stale-target bug + graph-level laws |
| core | `crates/vectra-core/src/{param,document,command,ids,engine}.rs` | — | `Spring { from, at }`, `MotionTrack` + registry, the three commands, `parse_node_id`, `history_depth`/`peek_undo` |
| wasm | `crates/vectra-wasm/src/lib.rs` | 2 418 total | the motion surface (10 exported methods), `resolve_live`, `flip_state`, `MotionReportWire` |
| | `crates/vectra-wasm/tests/motion_laws.rs` | 623 | the seven laws end-to-end through the frame loop, 7 tests |
| web | `apps/vectra-web/src/engine/*.ts`, `src/App.tsx`, `src/App.css` | — | wire types, client methods, track builders, the inspector projection, the Motion panel and a frame loop that stops |

`vectra-motion` gained no new dependency beyond the workspace; **core stayed the dependency
root** — the motion crate depends on core and implements its trait, never the reverse.

---

## 2. The three decisions that shaped everything

1. **`value = f(binding, t)` (F1).** A spring is the analytic step response, not a frame
   integrator: `MotionEvaluator::evaluate` takes a time and returns a value, so the scene is a
   pure function of the document and the clock. This is what makes the Motion Purity Law
   byte-exact — same `t` ⇒ byte-identical `EvaluatedScene` — and it is what makes the idle
   signal computable rather than guessed.
2. **The anchor is document state; the resolver is not (F2).** `Spring { from, at }` is authored
   intent and therefore lives in the document, where undo reaches it. Nothing else about a
   spring is state: no velocity, no "current value".
3. **The clock is the UI's.** The engine never reads a wall clock; it advances when told and
   answers with dirty ids. `is_animating()` is the idle signal that lets React **stop**
   scheduling frames — which also closes Task 5.0's open item #4.

---

## 3. The seven laws, and where each one is proven

| law | statement (as authorized) | proof |
| --- | --- | --- |
| **Motion Purity** | same `t` ⇒ byte-identical scene, whatever path the clock took | `law_the_scene_is_a_function_of_the_clock` — one snapshot, then the same `t` replayed after a backwards scrub (+1.5 → −0.75 → +1.5); four layers' `eval` strings compared byte-for-byte |
| **Spring Convergence** | arrives within tolerance and stays; ζ ≥ 1 never overshoots | engine: `law_a_spring_converges_and_stops`; crate: `the_step_response_converges_to_the_target`, `no_spring_overshoots_at_or_above_critical_damping` (ratios 1±1e-9, 1, 1.05, 1.5, 3 at 0.005 s steps), `an_under_damped_spring_overshoots_by_the_predicted_ratio` (peak vs `1 + e^(−ζπ/√(1−ζ²))`), `settle_time_is_an_upper_bound_and_not_pessimistic` |
| **Idle** | settled ⇒ empty dirty sets, zero document writes, zero history, zero bytes | `law_the_frame_loop_can_stop` — a settling frame writes, the **next** frame writes 0/0/0 bytes with an empty dirty set, and `is_animating()` is false while `time − horizon > 0` |
| **Interaction Precedence** | a drag outranks a spring; the hand-back is continuous | `law_a_drag_outranks_a_spring_and_hands_it_back` — the value before release is captured and compared with the value after (|Δ| < 1e-9), the slot is `animated` again, and the gesture left exactly one history entry; a second, differently-parameterised spring on `y` proves the precedence is **per slot** |
| **State** | a step function of the input, deterministic under replay, false arm without the crate | `law_states_are_inputs_and_hover_drives_them` (hover flips exactly the flags its bindings read; the re-anchor starts from the *live* value, not the destination) + `vectra-motion`'s `a_state_branch_reads_the_host_flag` |
| **Undo Isolation** | N seconds of animation add zero entries; one undo reverts the edit | `law_motion_samples_are_not_history` — 60 scrub/hover frames leave `undo_depth` unchanged, then **one** undo removes the binding, redo restores it |
| **Frame Budget** | K nodes moving by position alone ⇒ K × 64 B; settled ⇒ 0; a reshape re-tessellates only that node | `law_an_animating_scene_costs_only_the_slot_that_moves` — one spring ⇒ 1 write / 64 B / 0 re-tessellated; **two** springs ⇒ 2 writes / 128 B; a mid-animation `width` change on one node re-tessellates that node and stays under 4 KiB |

The tests are deliberately written as the *observable* form of each law (a `FrameWire`, a
snapshot, an `undo_depth`), not as assertions about internals, so they keep their meaning if the
implementation is replaced.

---

## 4. The bug the laws caught (found in recon, fixed and regression-tested)

`dependency_target` (singular, Task 2.2) returned one vertex per parameter and never descended
into a `MotionBinding`'s inner parameters. So `Spring { target: $radius }` was dirtied by the
clock but **not** by an edit to `$radius` — the node would keep easing toward a stale target,
with no error, no diagnostic, and a scene that looks *almost* right.

The fix is `dependency_targets(param) -> Vec<GraphNode>` (sorted, deduped), which emits the clock
plus a `State(flag)` per flag read, a `Track(id)` if the binding samples one, and recurses into
every inner `Parameter`.

**The regression test is the deliverable.**
`law_a_binding_depends_on_its_inner_sources` asserts the exact edge set for a nested binding and
that editing `$radius` dirties the sprung node. It was checked to **fail** against the old
behaviour (by temporarily removing the recursion) before being accepted — a law that has never
been red proves nothing.

A second, subtler bug was caught by `law_states_are_inputs_and_hover_drives_them` during
implementation: the re-anchor path originally resolved the slot with `resolve_float`, which builds
a document-only context, so an `Animated` slot resolved to its *target* — the static preview. The
spring would therefore re-anchor at the value it was already heading for, and hover would appear
completely dead while every unit test stayed green. Fixed by resolving through `resolve_live`.

---

## 5. What the wasm surface looks like

```text
bind_spring(node, property, target, stiffness, damping) -> MotionReportWire   # anchors from the live value, at the current time
bind_hover_spring(node, property, on, off, stiffness, damping) -> …           # a spring over StateDriven(hover:<node>)
set_state(node, name, on) -> MotionReportWire                                 # flags; no-ops when unchanged; never history
set_motion_track(json) / remove_motion_track(id) -> MotionReportWire          # undoable: track CRUD is a command
motion_json() -> MotionReportWire                                             # animating, horizon, time, epsilon, states, tracks, bindings
is_animating() -> bool
pointer_move(x, y) / pointer_leave() -> String                                # hit-test ⇒ hover flags ⇒ events + dirty ids
```

`MotionReportWire` sorts by node then property, so the report is stable under JSON comparison —
which is what makes the smoke assertions readable. `set_time` stays funnelled through the existing
`dirty_event`; **no second dirty path was added** for motion.

One ABI-shaped bug was fixed here too: `MotionBindingWire::of` reported a keyframe track's
*channel* as the binding's `property`, so a bound track slot could not be found by
`motion_json()`'s own lookup (`property` = the slot, `channel` = the track channel). It was
invisible in Rust tests and showed up only as a `null` in the smoke's value probe.

---

## 6. The UI (Task 1.4's rule kept: a dumb remote control)

`App.tsx` gained a Motion panel and a hover driver; `view-model.ts` gained `motionRows` /
`motionSummary` / `stateChips` (pure projections, covered by the view-model tests) and **no
geometry, no resolution, no state duplication** crossed into TypeScript.

* **Frame loop.** `kick()` schedules a frame only when the engine says it is animating; the
  callback stops scheduling when `isAnimating()` goes false. A settled scene costs zero frames —
  the fix for Task 5.0's open item #4.
* **Hover.** `pointermove` → `pointer_move(x, y)` → the engine hit-tests and flips the
  hover-bound flags; the value that moves is the spring's, not the UI's. `pointerleave` flips
  every hover flag off.
* **Panel.** A status badge from the report, hover off/on inputs, *bind a spring to the selected
  slot*, *bind a hover spring*, the hover toggle, track registration and removal, state chips,
  and per-binding rows with a per-property unbind. Every action logs to the event log and
  refreshes, so incrementality stays visible (Task 2.2's rule), and "Re-evaluate all" is the
  pre-existing `runFullReeval`.

---

## 7. Test results

Every suite ran on this machine after the final edit.

| suite | result | what it proves |
| --- | --- | --- |
| **workspace** (`cargo test --workspace`) | **312 passed / 0 failed / 40 targets** | ran three times consecutively, identical; no flakes |
| `vectra-motion` unit (`--lib`) | **19 / 19** | the closed forms (9), the sampler (4), the engine + evaluator (6) |
| `vectra-dependency/tests/motion_graph_laws.rs` | **7 / 7** | the recursion, the edge set, the track/state vertices, prospective cycles |
| `vectra-wasm/tests/motion_laws.rs` | **7 / 7** | the seven laws end-to-end (frame loop, snapshot, history, hit-test) |
| `vectra-wasm/tests/{drag,constraint,operation,render}_laws.rs` | **8+7+10+9 / 34** | the earlier laws, unchanged and still green under the motion engine |
| `cargo clippy --workspace --all-targets -- -D warnings` | **0 findings** | (one `manual_clamp` in the over-damped branch, fixed with `clamp`) |
| `cargo fmt --all -- --check` | clean | |
| `apps/vectra-web` — `npm run typecheck` | clean | both `tsconfig.json` and `tsconfig.tests.json` |
| `apps/vectra-web` — `npm run test:ui` | **29 / 29** | view-model projections incl. 5 new motion tests |
| `apps/vectra-web` — `npm run build` | clean | JS 209.59 kB (gzip 65.45), CSS 9.78 kB (gzip 2.64) |
| `apps/vectra-web/scripts/smoke.mjs` | **31 / 31 steps** | the whole engine through the compiled wasm module in Node |

**Smoke steps 27–31** are the Task 6.0 ones, and they are the acceptance demo:

| step | what it does |
| --- | --- |
| 27 | hover flips a flag; the spring **eases from where it was** (asserted ≠ the `on` value at the first sample) |
| 28 | the same `t` replays identically 4×; the scene settles and `is_animating()` goes false; the next frame does **0** writes / 0 bytes |
| 29 | 60 scrub + hover frames move `undo_depth` by **zero**; the binding is exactly one entry; undo removes it, redo restores it |
| 30 | per-slot precedence: drag `x` to 620 → the `x` spring re-anchors at 620, the `width` spring is untouched; undo 100, redo 620, `x_source == "animated"` |
| 31 | a track registers → samples (50 → 150 at t = 0.75) → **re-samples after a key edit** → a non-monotone track is refused typed → removal |

---

## 8. How to reproduce

```bash
export PATH=/usr/local/cargo/bin:$PATH

# 1. the whole workspace (includes the lavapipe GPU laws when a Vulkan adapter exists)
cd /home/user/vectra && cargo test --workspace          # 312 passed / 0 failed

# 2. the motion laws, alone
cargo test -p vectra-motion                              # 19
cargo test -p vectra-dependency --test motion_graph_laws # 7
cargo test -p vectra-wasm --test motion_laws             # 7

# 3. gates
cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings

# 4. the web app
cd apps/vectra-web && npm install                        # node_modules is not snapshotted
bash scripts/build-wasm.sh debug                         # rebuild after any Rust edit
npm run typecheck && npm run test:ui && npm run build
node scripts/smoke.mjs                                   # 31/31
npm run dev                                              # http://0.0.0.0:5173
```

---

## 9. Honest gaps

1. **The browser path is still unexercised in this sandbox** — no browser tooling here (recorded
   in Task 5.0 and still true). The hover driver is proven at the wasm boundary
   (`Renderer::hit_test` ⇒ flags ⇒ dirty ids ⇒ one 64-byte row) and in smoke step 27, but no
   human has watched a spring move in the canvas in this environment. The dev server is up and
   the mount order is the tested one (draw, then follow the pointer).
2. **Nested springs are not re-anchored** on a state flip; the top-level spring of a slot is the
   re-anchorable unit. Documented in `TASK-6.0-DESIGN.md` §8.
3. **Tracks are linear.** One interpolation curve; per-key easing is a Phase-2 track-editor
   concern with its own dependency vertices.
4. **The settle epsilon is an absolute 1e-3 slot units**, configurable, not proportional to the
   travel. For document-space floats in the ranges the document actually carries (a 300-unit
   width, a 0.8 opacity) that is honest; a 1e9-unit shape would settle "instantly" by this
   measure.
5. **`StateDriven` is a boolean branch**, not a state machine. Multi-state machines, transitions
   and triggers are the other half of MES §12 and belong with the procedural work.
6. **MES §12 is one field behind the code.** The spec sketches
   `Spring { target, stiffness, damping }`; the implementation carries the anchor as well
   (`Spring { target, stiffness, damping, from, at }`), per fork F1/F2 — the anchor is document
   state and the resolver is pure. `MES.md` was left untouched (it is the standing spec, not the
   tracker); the rationale is in `TASK-6.0-DESIGN.md` §2 and the module docs of
   `crates/vectra-motion/src/spring.rs`. Every other §12 shape is implemented exactly as written,
   including the static-preview fallbacks (spring → target, state → false arm, timeline → typed
   error).

---

## 10. Verdict against the acceptance criteria

| criterion (RECON §8 order) | status |
| --- | --- |
| 1. `dependency_target` recursion **with** the regression test | ✅ `dependency_targets`; test verified red-before-green |
| 2. `vectra-motion`: springs, state branches, tracks, `impl MotionEvaluator` | ✅ 19 unit tests; closed form supersedes the RECON's fixed-step wording, rationale recorded |
| 3. Commands + undo: binding undoable, samples not; track CRUD | ✅ three commands, `Undo Isolation` proven through wasm |
| 4. wasm surface: `with_motion`, `set_state`, `is_animating`; `set_time` keeps its funnel | ✅ 10 methods; no second dirty path |
| 5. UI: bind a spring, drive hover/active, a loop that **stops** when idle | ✅ Motion panel + `kick()`; closes Task 5.0's open item #4 |
| 6. The seven laws; gates clean; smoke past 26; design + report | ✅ 312/0; clippy + fmt clean; 29/29 UI; **31/31 smoke**; this document + `TASK-6.0-DESIGN.md` |

**Task 6.0 is complete.** The engine is interactive: numbers can move, the document stays the
only place intent lives, animation cannot pollute the undo stack, and a settled scene costs
nothing.
