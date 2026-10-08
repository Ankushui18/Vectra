# ENGINE_STATUS — what is verifiably true about VECTRA's engine, today

> **Living document.** Companion to `ROADMAP.md` (what we build next) and
> `FEATURE_MATRIX.md` (what exists, feature by feature). This file answers one
> question only: **what can be proven, right now, by running the code — and what
> is merely claimed.** Every number below is either *measured* in this
> repository, or explicitly marked *(reported)* when it comes from a `TASK-*.md`
> report and was not re-run. There are no other categories; if a claim is
> neither, it does not belong in this file.

**Last full measurement:** 2026-10-08, commit `5422a99` (the only commit).
**Measured by:** `VECTRA_AUDIT_AND_ROADMAP.md` §0 / Appendix C run.
**Environment:** Linux sandbox, `cargo 1.97.0` built from the PyPI toolchain
bundle into `/tmp/rust`, deps from the vendored mirror at `/tmp/vendor`
(247/247 crates), Node v22.22.3. **No GPU, no network, no CI.** Read §6 before
quoting any number as "CI-green".

---

## 1. Gate table — the four green gates

| Gate | Command (see §5 for the environment) | Result | Wall time |
|---|---|---|---|
| Rust tests | `cargo test --workspace --lib --tests --offline --no-fail-fast -j 2` | **650 passed / 0 failed / 0 ignored**, across **54 test targets** | ~9 min cold, ≈1 min warm |
| Clippy | `cargo clippy --workspace --all-targets --offline -j 2 -- -D warnings` | **0 warnings** (with `-D warnings` active) | ~3 min |
| Rustfmt | `cargo fmt --all -- --check` | **clean, no diff** | <5 s |
| Web UI tests | `cd apps/vectra-web && npm run test:ui` | **146 tests / 0 fail** (`node --test` over an esbuild bundle of `tests/**`) | 1.07 s |
| Web smoke | `cd apps/vectra-web && npm run smoke` | **79/79 steps PASS** — loads the *real* compiled `vectra_wasm_bg.wasm` in Node | ~4 s |
| Web typecheck | `cd apps/vectra-web && npm run typecheck` | **clean** (`tsc --noEmit`) | ~6 s |
| Web bundle | `cd apps/vectra-web && npm run build` | **EXIT 0** in 7.72 s (Vite 6) | 7.72 s |

The smoke suite is the strongest end-to-end signal in the repo: it drives the
shipped wasm through 79 named scenarios — document creation, expression
binding, undo, dependency graph, incremental patch ≡ full rebuild, the
constraint drag triad, boolean/clip operations, region graphs, pen/brush
gestures, alpha lock, clipping masks, colour drop, text on a path, procedural
graph, semantic SVG, parametric React export, the AI command layer with
self-correction, and the `.vectra` save path.

### How the 650 splits

- **273** from the 14 `--lib` unit-test targets (in-crate `#[cfg(test)]`
  modules). Densest: `vectra-geometry` 53, `vectra-core` 51, `vectra-draw` 43,
  `vectra-expression` 27, `vectra-constraints` 22, `vectra-render` 20,
  `vectra-motion` 19.
- **377** from 40 integration files. The integration files are the *laws* —
  named `*_laws.rs`, written as invariants rather than examples, e.g.
  `crates/vectra-wasm/tests/` alone holds 14 of them (workspace boundary,
  incremental, constraints, drag, draw, motion, operations, procedural, export,
  components, alpha lock, navigation, render, AI, file).
- Two crates — `vectra-constraints` and `vectra-motion` — have **no
  integration files at all**; they are gated exclusively by unit tests. That is
  a defensible choice for a solver and a spring integrator (the properties are
  internal), but it means no law in the repo pins *their* public API surface.
  Any refactor of `vectra-constraints` should add one.

### What the gates do **not** cover

1. **Doc-tests never ran.** `tools/extract_toolchain.py` installs a toolchain
   **without `rustdoc`**, so `--doc` targets cannot execute here. The reports'
   "650 passed (56 targets)" counts ~2 doc-test targets this sandbox cannot
   reach. The 650/54 figure above is `--lib --tests`.
2. **No GPU path was exercised.** `wgpu` is compiled, the WGSL is
   parsed/type-checked through `naga` in `shader_laws.rs`, GPU buffer layout is
   asserted in `gpu_laws.rs` — but no adapter, no swapchain, no rendered frame.
   Every "renders correctly" statement is a statement about *scene and buffer
   derivation*, not about pixels.
3. **No desktop run.** `apps/vectra-desktop` typechecks and its Rust logic is
   unit-covered, but no Tauri window was launched and no bundle was produced.
4. **No browser run of the web app.** The UI tests are DOM-level (jsdom-class
   harness), the smoke test is Node+wasi-free wasm; nothing clicked a real
   canvas in a real browser in this sandbox.
5. **No CI exists.** There is no `.github/` directory. These gates are green
   *because someone ran them*, not because a robot will run them tomorrow.
   Wiring them is workstream **1.0** in `ROADMAP.md`.

---

## 2. Build artifacts and sizes (measured)

Measured on the build described above. `npm run build` output, byte-exact:

| Artifact | Raw | Compressed |
|---|---|---|
| `apps/vectra-web/dist/assets/vectra_wasm_bg-DsKpeYfC.wasm` | **27,686,250 B (26.40 MiB)** | 3,594.91 kB (Vite gzip) / 3,590,546 B (`gzip -c`) |
| `dist/assets/index-DeeFiym4.js` | 325,089 B | 98.39 kB |
| `dist/assets/index-Cgrqbkem.css` | 25,472 B | — |
| Committed glue `src/wasm/vectra_wasm.js` | 122,999 B | 24,198 B |
| Committed types `src/wasm/vectra_wasm.d.ts` | 43,766 B | — |
| Committed types `src/wasm/vectra_wasm_bg.wasm.d.ts` | 8,770 B | — |

### The wasm is a **debug-profile** build

This is the single most concrete, actionable finding of the audit's measurement
pass, and it is stated as a chain of measurements, not an inference from a name:

| Probe | Result |
|---|---|
| `cargo build --target wasm32-unknown-unknown -p vectra-wasm` (**dev** profile) | **45,634,937 B** |
| `cargo build --release --target wasm32-unknown-unknown -p vectra-wasm` | **10,066,632 B** |
| Committed `apps/vectra-web/src/wasm/vectra_wasm_bg.wasm` (post-`wasm-bindgen`) | **27,686,250 B** |
| `apps/vectra-web/scripts/build-wasm.sh` line 18 | `PROFILE="${1:-debug}"` → **release requires an explicit argument** |
| `Cargo.toml` dev profile | `debug = "line-tables-only"` |
| Byte-probe of the committed wasm for `.debug_info` / `.debug_line` / `target/` strings | **none found** (11 hits for `crates/vectra-wasm/src` — panic paths only) |

Read together: the artifact shipped in `src/wasm/` is the **dev-profile** build
(no DWARF because the dev profile says `line-tables-only`), 2.75× the size of a
release build before `wasm-bindgen` ever runs. `tessellate`/`lyon`/`geo`
unoptimised is most of that difference. **27.7 MB of wasm is served to every
user of the web app today.** Not yet measured: the exact post-`wasm-bindgen`
release size — the `wasm-bindgen` CLI is absent in this sandbox and the in-repo
`scripts/wbgen` driver has its own 42-crate closure that is not vendored. The
release build *file* is 10,066,632 B, which bounds the fix: same 7.7 s build,
roughly a third of the bytes. This belongs in workstream **1.0** as a size
budget gate.

*(Reported, not re-measured: the Tauri desktop binary at ~10.7 MB with
`strip = true`, `lto = "thin"`, `opt-level = "s"`, `codegen-units = 4`. No
desktop release build was run in this environment.)*

---

## 3. Source size, by crate (measured with `wc -l`)

| Crate | src LOC | test LOC | integration files | Role |
|---|---:|---:|---:|---|
| `vectra-core` | 13,262 | 828 | 3 | dependency root: IDs, `Parameter<T>`, `Document`, commands, undo |
| `vectra-wasm` | 8,370 | 8,073 | 14 | the only public API; command bus + snapshots + settle pipeline |
| `vectra-geometry` | 6,537 | 2,368 | 3 | primitives, paths, arcs, regions, tessellation, text shaping |
| `vectra-render` | 4,234 | 2,634 | 6 | wgpu scene graph, buffers, BVH/R-tree hit testing |
| `vectra-ai` | 3,616 | 1,407 | 1 | strict command schema, planner, validation, self-correction |
| `vectra-draw` | 3,458 | 534 | 1 | pen cubic math, brush fitting, Quick Shape, outline |
| `vectra-dependency` | 2,656 | 3,533 | 5 | dirty propagation, incremental evaluation, cycle gate |
| `vectra-constraints` | 2,250 | 0 | 0 | Cassowary linear + nonlinear rows; proposes, engine commits |
| `vectra-export` | 1,818 | 764 | 1 | export IR → SVG, React/Vue codegen |
| `vectra-procedural` | 1,515 | 1,007 | 1 | typed node graph, generators, modifiers |
| `vectra-operations` | 1,309 | 1,819 | 3 | boolean, offset, fillet, mirror, repeat, clip |
| `vectra-expression` | 1,303 | 372 | 1 | lexer, parser, AST, compiler, incremental evaluator |
| `vectra-motion` | 1,208 | 0 | 0 | timeline, springs, physics, tracks as `Parameter` resolvers |
| `vectra-file` | 485 | 483 | 1 | `.vectra` container + Document→Command replay plan |
| **Rust total** | **52,021** | **23,822** | 40 | |
| `apps/vectra-web` | 14,924 TS/TSX | 146 node tests | — | React/Vite host (dumb remote); `App.tsx` alone is 4,233 lines |
| `apps/vectra-desktop` | 34.7 KB Rust (3 files) | in-crate | — | Tauri v2 host: `open/save/verify_document`, `current_path` |

Two structural observations from these numbers, both load-bearing for the
roadmap:

1. **Test weight is concentrated where the laws are.** `vectra-wasm` carries
   almost as much test LOC (8,073) as source LOC (8,370) — the boundary is the
   most rigorously specified surface in the repo, which is exactly right for the
   one surface the whole product sits on. `vectra-constraints` and
   `vectra-motion`, by contrast, are gated only internally (§1).
2. **The engine is 52 kLOC of Rust behind a 15 kLOC front end**, and the front
   end is the part that is unbuilt as a product — not the engine. This is the
   quantitative version of the audit's "engine 9/10, product 4.5/10".

---

## 4. Performance: claimed vs measured

**Nothing in the repository measures performance.** There is no `criterion`
dependency anywhere in `Cargo.toml`, no `benches/` directory, no perf test, and
no CI job. `FEATURE_MATRIX.md` row `OPS-7` and audit finding §3.7 record this.

What exists is *(reported)* — numbers written down in task reports that no
check will ever catch drifting:

- `SetVariable` end-to-end **37.5 µs** on a 502-node document *(reported)*.
- One-node incremental patch **190.7 µs** vs **878.1 µs** full rebuild — a 4.6×
  incrementality ratio *(reported)*, consistent with the *measured* structural
  claim that dirty propagation is `O(affected)`.
- MES §18 budgets: **2 ms** immediate / **16 ms** incremental / offload above
  **50 ms**. The third tier — off-main-thread evaluation — **does not exist** in
  the code (no worker, no wasm threads).

Consequence, stated plainly: **the engine's speed is currently a rumour with a
µs unit attached.** The architecture is built for it (dirty sets, incremental
scene patches, `Dirty{ids, mode}` events as an audit trail), and the smoke suite
proves the *incremental* path produces byte-identical results to a full rebuild
— but "fast" is unverified, and a regression today would be invisible.
Workstream **4.3** (Phase 4) owns `criterion` benches, CI-tracked numbers, a
perf HUD, and the >50 ms background tier. Until then, do not cite these µs
figures to a user or an investor without the word *(reported)*.

---

## 5. Reproducing all of this offline (the only supported path here)

The toolchain and dependencies do **not** come from the network in this
environment. Three scripts create a working offline Rust environment in `/tmp`:

```bash
python3 tools/extract_toolchain.py /tmp/rust      # cargo 1.97.0 (+clippy, +rustfmt), ~11 s
export PYTHONPATH=/tmp/pylibs                     # required: tools/vendor_deps.py needs tomli_w
python3 tools/vendor_deps.py                      # vendored mirror → /tmp/vendor, target 247 crates
export PATH=/tmp/rust/prefix/bin:$PATH
export CARGO_HOME=/tmp/cargohome                  # tools/check.sh writes the source-replacement config here
```

`/tmp` does not survive between sessions; re-run all of it. Then the gates are
the Appendix C commands of the audit — run **cargo directly**, not through
`tools/check.sh`.

### Three defects in the offline tooling (all still open)

1. **`tools/check.sh` appends `--offline` *after* the caller's arguments.**
   `tools/check.sh fmt --all -- --check` prints `Unrecognized option: 'offline'`
   and **exits 0 anyway** (the most dangerous of the three: a passing gate that
   did nothing). `tools/check.sh clippy … -- -D warnings` exits 101 because
   `--offline` reaches `clippy-driver`. `test` and `build` are unaffected. One
   line to fix; until then, `fmt` and `clippy` must be invoked as plain cargo
   commands with the environment above.
2. **The first `vendor_deps.py` pass can produce a non-compiling
   `wgpu-types@22.0.0`** (reconstructed manifest drops
   `macro_rules_attribute` while the sources import it). Repair is
   `PYTHONPATH=/tmp/pylibs python3 tools/vendor_deps.py --only wgpu-types --force`
   (~45 s). Do not hand-patch the vendored crate; the initial pass does not
   self-heal without `--force`.
3. **`extract_toolchain.py` installs no `rustdoc`**, so doc-tests are
   impossible (§1). Either ship `rustdoc` in the bundle or delete the doc-test
   targets and stop counting them.

These three are quantified as ~an hour of work each in audit §3.9. Until they
land, "the gate is green locally" is *configuration-dependent* — which is
exactly the class of problem CI (workstream 1.0) exists to remove.

---

## 6. Verified-where matrix

Where each subsystem's claims are actually exercised. This table is the
honest answer to "has this been tested?" — and the reason `FEATURE_MATRIX.md`
carries an evidence column per row.

| Surface | Rust unit | Rust laws | Web UI | Node smoke | Real GPU | Real browser | Desktop |
|---|:--:|:--:|:--:|:--:|:--:|:--:|:--:|
| Document / commands / undo | ✅ | ✅ | ✅ | ✅ | — | — | ✅ |
| Geometry, arcs, regions | ✅ | ✅ | ✅ | ✅ | — | — | — |
| Constraint solver | ✅ | ✅ (boundary) | ✅ (rows) | ✅ (drag triad) | — | — | — |
| Expression language | ✅ | ✅ | ✅ | ✅ | — | — | — |
| Dependency / incremental | ✅ | ✅ | ✅ | ✅ (patch ≡ rebuild) | — | — | — |
| Boolean / clip / smart fill | ✅ | ✅ (area identities) | ✅ | ✅ | — | — | — |
| Procedural graph | ✅ | ✅ | ✅ | ✅ | — | — | — |
| Motion / springs / tracks | ✅ | ✅ | ✅ | ✅ | — | — | — |
| Text shaping → outlines | ✅ | ✅ | ✅ | ✅ | — | — | — |
| Export (SVG / React) | ✅ | ✅ | ✅ | ✅ | — | — | — |
| AI command layer | ✅ | ✅ | ✅ (panel) | ✅ | — | — | — |
| Renderer / buffers | ✅ | ✅ | — | ✅ (frames/hits) | ❌ | ❌ | — |
| Raster / brush engine | — | — | — | — | — | — | — |
| Colour management | — | — | — | — | — | — | — |

The two empty bottom rows are the audit's two `MISSING` subsystems, and they
are empty for a reason: there is nothing to test. Everything above them is
verified at least at the law level, and the renderer is the one place where the
verification stops at the GPU boundary.

---

## 7. Risk register — what is green *and fragile*

Green gates can still hide the things most likely to break Phase 1. Ranked by
"how much damage if it is discovered late":

| # | Risk | Why it matters now | Evidence |
|---|---|---|---|
| R1 | **No transforms in the document model.** | Every geometry consumer — renderer, exporters, region graph, hit testing — is written assuming a node's coordinates *are* its position. Adding transforms later is a cross-cutting rewrite, not a feature. | `FEATURE_MATRIX.md` CORE rows; audit §3.6 |
| R2 | **Three parallel hierarchies** (layer membership, `Document.order`, per-layer lists) instead of one child list. | Grouping/reordering laws pass, but each new "container" (boolean, mask, clip group) multiplies the ways they can disagree. | audit §3.2; `tree_laws.rs`, `workspace_laws.rs` |
| R3 | **The shipped wasm is a debug build** (§2). | 26 MiB to first user; also means all quoted timings are dev-profile timings. | §2 of this file |
| R4 | **Performance is unmeasured** (§4). | Reports' µs figures can regress silently; the MES §18 50 ms tier is unimplemented. | §4; audit §3.7 |
| R5 | **No CI** (no `.github/`). | The four green gates depend on a sandbox that will not exist for the next contributor. | §1; audit §3.8 |
| R6 | **Two crates have zero integration laws** (constraints, motion). | Public-API regressions are invisible to the gate. | §1 |
| R7 | **`vectra-wasm` is the only public API and is ~47 methods wide** hand-written in `lib.rs`/`draw.rs`/`render.rs`. | Any Phase-2 raster boundary change touches the widest, most-tested surface in the repo — which is why the 8,073 lines of law there are an asset, not overhead. | audit §3.5 |

---

## 8. Update rules

This document is only worth anything if it is *re-measured*, not edited.

1. **Every gate run in CI must overwrite §1** with the new numbers, and the
   reference to `VECTRA_AUDIT_AND_ROADMAP.md` replaced by a link to the run.
2. **§2 must be regenerated by the build**, not typed. Add
   `tools/wasm_size_report.py` (Phase 1, workstream 1.0) that emits the table;
   a size budget that is not machine-checked is a wish.
3. **§4 may not gain a number unless a `criterion` bench produced it.** When
   benches land, move rows from *(reported)* to measured in the same commit.
4. **§6 gains a column only when a gate for it exists** — never a checkmark for
   intent.
5. **§7 loses a row only when the fix is in `main`**, and the row records the
   commit that closed it.
