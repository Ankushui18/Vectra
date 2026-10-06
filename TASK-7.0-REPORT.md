# Task 7.0 — The Procedural Graph: report

**Status: complete.** All five rules hold, at the engine boundary *and* through the compiled wasm
module, and the web smoke drives the whole loop from the browser's side of the JSON seam. Every
number below was produced by a command in §6 on this machine after the last edit. The design
rationale is in `TASK-7.0-DESIGN.md`; the pre-implementation inventory is in `TASK-7.0-RECON.md`.

---

## 1. What was built, and where

| layer | artifact | lines | what it is |
| --- | --- | --- | --- |
| core | `crates/vectra-core/src/procedural.rs` | 1 339 | `GeometryData`, `PortType`/`Port`, the five kinds + `default_operands`, `ProceduralNode::{validate, is_wired, upstream}`, `ProceduralRegistry`, `param_procedural_ref`, `RemovedProceduralNode` |
| | `crates/vectra-core/src/document.rs` | 1 164 | the `procedural` registry, widened `geometry_ids`/`is_geometry_id`, `procedural_readers`, `procedural_geometry_closure`, `property_feeds_geometry` |
| | `crates/vectra-core/src/command.rs` | 1 790 | six procedural commands + `EngineEvent::ProceduralUpdated`, the inverse batch, `validate_procedural_cycle` (RULE 3's geometry gate) |
| | `crates/vectra-core/src/eval.rs` | 306 | `ProceduralEvaluator` for float, point **and colour**; `EvaluationContext::procedural`/`with_procedural` |
| procedural | `crates/vectra-procedural/src/engine.rs` | 588 | the chain DAG (topo order, cycle gate, dirty walk), the evaluation pass, the published table, `impl ProceduralEvaluator` |
| | `crates/vectra-procedural/src/nodes.rs` | 640 | per-kind evaluation, `output_type` assertions, the flatten tolerance |
| | `crates/vectra-procedural/src/noise.rs` | 145 | integer hashing (splitmix-style) + value noise — RULE 5 |
| | `crates/vectra-procedural/tests/procedural_laws.rs` | 998 | the seven laws + the settle fixpoint law, 12 tests |
| graph | `crates/vectra-dependency/src/graph.rs` | 1 420 | `GraphNode::Procedural { node, port }`, `procedural_edges`, the `collect_targets` arm, `ProceduralUpdated` |
| | `crates/vectra-dependency/src/prospective.rs` | 357 | the six prospective arms (RULE 3 pre-application in the UI) |
| wasm | `crates/vectra-wasm/src/lib.rs` | 2 953 | `run_procedural`, `run_procedural_fixpoint`, `follow_up_readers`, `procedural_json`, `procedural_kinds`, the snapshot section |
| | `crates/vectra-wasm/tests/procedural_laws.rs` | 795 | 10 laws over the JSON seam |
| web | `apps/vectra-web/src/engine/{wire,commands,view-model,client}.ts` | 2 030 | procedural wire types, 9 builders, 3 projections, `procedural()`/`proceduralKinds()` |
| | `src/App.tsx` + `src/App.css` | 2 858 | the Procedural panel (dropdown-wired, no canvas graph) |
| | `tests/view-model.test.ts` + `scripts/smoke.mjs` | 1 958 | 33 UI tests (4 new) and **36** smoke steps (5 new) |

Core gained no dependency: `GeometryData` is serializable and dependency-free, as RULE 1 requires.
`vectra-procedural` depends on core and implements its trait, never the reverse.

---

## 2. The five rules, and the evidence for each

| rule | where it lives | what proves it |
| --- | --- | --- |
| **1 — `GeometryData` is king** | `core::procedural` | `law_port_types_are_never_coerced` (a `Points` port into a `Region` input is a typed pre-application rejection, document byte-identical afterwards); wasm `law_mismatched_ports_and_disguised_cycles_change_nothing`; smoke step 33 |
| **2 — the pass runs last** | `wasm::settle`, `core::eval` | wasm `law_a_full_rebuild_equals_the_incremental_scene`; smoke steps 34 and 36 (`patch ≡ rebuild` with a procedural graph in the picture); the fixpoint law in `vectra-procedural` |
| **3 — no disguised cycles** | `core::command`, `core::procedural` | `ProceduralNode::validate` for operands; `validate_procedural_cycle` for the geometry path (slot, binding, operation); core `the_cycle_gate_sees_through_geometry_too` proves both the rejection **and** the style exemption; smoke step 33's three refusals |
| **4 — no silent eviction** | `core::document`, `geometry::scene` | `law_a_procedural_result_survives_a_neighbouring_operation` (asserts against `z_order`, then parks/re-arms/removes/undoes); wasm `law_a_procedural_result_survives_an_operation_pass_on_the_wire`; smoke step 35 |
| **5 — deterministic noise** | `vectra-procedural::noise` | `law_the_scene_is_a_function_of_the_document` (full rebuild ≡ incremental, byte-for-byte); smoke step 36 (two engines, one script, one picture) |

---

## 3. Three bugs the gates caught, and what they taught

1. **The name-resolution bug in `apply_partial`.** Task 4.0's composer rebuilt the draw order
   from `Document::geometry_ids()`; the *patch* path rebuilt it from `Document::order` — the
   authored list, which contains neither operations nor procedural results. So the first patch
   after a virtual pass dropped a boolean or a procedural result out of `z_order` while leaving it
   cached: present in `nodes`, invisible to the renderer. Found by probing why a re-armed node
   refused to draw (smoke step 35). Fixed in `crates/vectra-geometry/src/scene.rs` — both paths
   now ask the same question — and the procedural law now asserts against **`z_order`**, not
   `scene.get(id)`, because those are different questions and only one of them is *drawing*.
2. **The settle was missing its second round.** With one pass + one reader re-read, an edit that
   moved a *reader* left every `Source` node downstream of it one edit behind — still drawn, wrong.
   The web smoke found it as `patch ≢ rebuild` (step 36). `settle` now runs the pass to a fixpoint
   (bounded, with a `debug_assert`), and `a_source_reading_a_reader_follows_in_the_same_settle`
   pins it.
3. **The geometry cycle gate over-rejected the colour door.** RULE 3's new boundary check first
   used the *whole* node, so `rect.style.fill ← ⬡noise•tint` with `noise ← source(rect)` was
   refused — but paint feeds no geometry, so that document is legal and the wasm colour-door law
   is exactly that shape. `Document::property_feeds_geometry` now draws the line: geometry slots
   are gated, `style.*` is not. Two failing wasm laws, one fix, and the carve-out is itself a test.

---

## 4. The palette and the panel (step 5)

`procedural_kinds()` is generated from the registry: `{tag, label, needs_subject, operands,
inputs, outputs}` for `["source", "grid", "repeat", "noise", "smooth"]`. Its `operands` are
**kind-level** payloads (`Parameter<T>`, no `ParamValue` wrapper) so the panel spreads them
straight into the kind it is building; the engine fills a missing `name` from `describe()` and a
missing `enabled` from `true`, so the panel sends `{id, kind}` and nothing else.

The panel itself is dropdown-wired — kind, subject, add; per node: outputs, wire pickers per input
port, operand editors, enable and remove; diagnostics from the log. It computes no geometry and
formats no numbers: every string is engine-rendered (`render_param_value`, `geometry_summary`,
`render_procedural_ref`).

---

## 5. Gates

| gate | command | result |
| --- | --- | --- |
| workspace tests | `cargo test --workspace` | **366 passed, 0 failed** (312 at the start of Task 7.0; 363 before this session's two fixes) |
| lints | `cargo clippy --workspace --all-targets` | clean |
| format | `cargo fmt --check` | clean |
| wasm | `bash apps/vectra-web/scripts/build-wasm.sh debug` | rebuilt (18.7 MB debug bundle, bindings regenerated) |
| smoke | `npm run smoke` | **SMOKE PASS 36/36**, `smoke[1/36]`…`smoke[36/36]` |
| UI types | `npm run typecheck` | clean (both tsconfigs) |
| UI tests | `npm run test:ui` | **33 pass, 0 fail** (29 before; +4 procedural projections) |
| web build | `npm run build` | ok — `index-CCMXPLDM.js` 217.34 kB (67.38 kB gzip) |

Per-suite additions this task: `vectra-procedural` 11 lib + **12** laws (new crate);
`vectra-wasm/tests/procedural_laws.rs` **10**; core 44 lib + 10 + 6; `vectra-dependency` 13 + 9 + 8
+ 7; smoke +5 steps; `test:ui` +4 tests.

---

## 6. Open items

* The browser path (real WebGPU canvas + the React panel in a live page) is still unexercised in
  this sandbox — there is no browser tooling here. Everything up to the DOM boundary is covered:
  the wasm module is loaded and driven through its own glue, and the UI is covered by typecheck,
  the projection tests, and the smoke driving the same JSON the panel sends.
* `TASK-7.0-RECON.md`'s "proceed 7" open scope was superseded by the five authorized rules; its
  seven laws remain the test spec and all seven hold.
