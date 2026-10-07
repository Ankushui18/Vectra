# VECTRA — ROADMAP

> **Living document.** Owner: engineering lead · Review trigger: end of every phase
> workstream, and any change to a phase's exit criteria · Source of truth for
> *what gets built next*.
>
> Seeded 2026-10-07 from [`VECTRA_AUDIT_AND_ROADMAP.md`](../VECTRA_AUDIT_AND_ROADMAP.md)
> (the audit). Capability-level detail lives in [`FEATURE_MATRIX.md`](./FEATURE_MATRIX.md);
> measured numbers live in [`ENGINE_STATUS.md`](./ENGINE_STATUS.md); the target
> architecture and its invariants live in [`ARCHITECTURE.md`](./ARCHITECTURE.md).

---

## 0. Where the product stands (measured, 2026-10-07)

| gate | result |
|---|---|
| `cargo test --workspace --lib --tests` | 650 passed / 0 failed (54 targets) |
| `cargo clippy --workspace --all-targets -- -D warnings` | clean |
| `cargo fmt --all -- --check` | clean |
| `npm run typecheck` / `test:ui` / `smoke` | clean / 146 passed / 79 steps PASS |

Subsystem status: **1 `IMPLEMENTED`** (Smart Fill & Region Graph), **11 `PARTIAL`**,
**2 `MISSING`** (Raster & Brush, Colour Management), **0 `DEPRECATED`**.
The engine is strong; the *product* is incomplete and the UI still presents itself
as an engineering console. Both facts are the reason this roadmap starts where it
does.

---

## 1. How to read this file

- Phases are **capability milestones, not calendars**. Size classes: `S` ≈ a few
  days · `M` ≈ 1–2 weeks · `L` ≈ 3–6 weeks of focused work.
- A workstream is **done** only when its laws run in CI and its
  `FEATURE_MATRIX.md` rows say `IMPLEMENTED` with a witness.
- Workstream ids (`1.1`, `2.3`, …) are stable; `FEATURE_MATRIX.md` rows reference
  them in their `phase` column.
- Every phase ends with the same gate shape (see §6).

---

## 2. Phase 0 — the audit (done)

| item | status | output |
|---|---|---|
| Repo-level audit & master roadmap | **done** (2026-10-07) | `VECTRA_AUDIT_AND_ROADMAP.md` |
| Verification of the existing gates | **done** | 650 / 146 / 79, clippy + fmt clean |
| Seed documents | **done** | this file · `FEATURE_MATRIX.md` · `ENGINE_STATUS.md` · `ARCHITECTURE.md` |

---

## 3. Phase 1 — Professional Illustrator Core

**Goal.** A designer can produce a real vector illustration — shapes, booleans,
gradients, clipped layers, text on a path, constraints holding proportions — and
hand off a clean SVG that Vectra can also re-open.

| id | workstream | crates touched | size | depends on | status |
|---|---|---|---|---|---|
| **1.0** | Hygiene & CI: LICENSE, `.gitignore`, GitHub Actions (fmt · clippy · tests · wasm build · UI · smoke), `TASK-*.md` → `docs/tasks/`, fix the three offline-tooling defects | repo, `tools/` | S | — | planned |
| **1.1** | **Transform & graph foundation** — `Transform2D`, `parent`, ordered children, world-matrix evaluation, transform slots, group transforms | core, geometry, render, wasm, export, web | L | — | planned |
| **1.2** | **Constraint UX & vocabulary** — Tangent/Symmetric/Concentric/Collinear/Midpoint/EqualRadius/Fix, inequalities, nonlinear pass, on-canvas glyphs, n-ary + click-to-constrain | core, constraints, render, wasm, web, export | L | 1.1 (glyph placement) | planned |
| **1.3** | **Smart Pen & Direct Selection polish** — snapping providers, join/split/simplify, live fit preview, vector eraser, constraint-snapping | draw, constraints, wasm, web | M | 1.1 | planned |
| **1.4** | **Boolean containers & Pathfinder** — n-ary booleans, op-consuming-op, chamfer, expand/outline stroke, divide/trim/merge, fill-rule control, cubic refit of results | core, operations, geometry, wasm, web | L | 1.1 | planned |
| **1.5** | **Smart Fill hardening** — gap tolerance, region cache, worker offload for the >50 ms tier, curve-preserving faces | geometry, operations, wasm, web | M | — | planned |
| **1.6** | **Clipping masks & compositing depth** — mask node refs (invert/feather/alpha-luma), group masks, isolation modes, W3C 16-mode blend set, `<mask>`/`<clipPath>` export | core, render, operations, export, web | M | 1.1 | planned |
| **1.7** | **Gradient system** — conic, gradient-on-stroke, multi-stop editor, on-canvas handles, **linear-light interpolation** | core, geometry, render, export, web | M | — | planned |
| **1.8** | **SVG import/export fidelity** — new `vectra-import` crate, ellipse/arc segments in the path model, `<text>` export option, fidelity report, true round trip | **new** `vectra-import`, export, geometry, wasm, web | L | 1.1 | planned |
| **1.9** | **PNG export** — GPU readback → PNG at 1×/2×/3×, artboard scope, transparency (pulled forward from Phase 2) | export (new `encode` module), render, wasm, web | S | — | planned |

**Kickoff order (first two weeks):** 1.0 → the 1.1 design doc (ADR) → 1.1
implementation, with **1.9** and the `FEATURE_MATRIX` seed running in parallel.

**Exit criteria (all must be true):**

1. The logo workflow (n-ary chained booleans, gradient stroke, clipped layers,
   text on a path, a constraint holding a proportion) is authored in-app with no
   JSON by hand.
2. The document saves, exports to SVG, and **imports back** with a fidelity report
   and no structural loss.
3. All new laws are green; the existing 650/146/79 gates do not regress.
4. **CI exists** and runs every gate on every push.
5. Every Phase-1 row of `FEATURE_MATRIX.md` says `IMPLEMENTED` with a witness.

**Explicitly out of scope for Phase 1:** raster painting, colour management,
variant components, auto-layout, PDF, video export.

**Cut first if the phase overruns** (in order): conic gradients →
`<text>`-in-SVG emission → group isolation modes → `Trim`/`Merge` beyond `Divide`
→ eraser/simplify in 1.3. **Never cut:** 1.1, 1.0, 1.8, 1.2's vocabulary, 1.9.

---

## 4. Phase 2 — Procreate Raster Engine

**Goal.** A painter can paint with pressure and tilt, using textures, blend
modes, alpha lock and selections, at 60 fps — and export PNG/JPEG at any scale.

| id | workstream | crates touched | size | depends on | status |
|---|---|---|---|---|---|
| **2.1** | **New crate `vectra-raster`** — tile map, raster layer, brush spec with pressure/tilt dynamics, grain textures, dab pipeline, smudge/blend/eraser, selection algebra, tile-patch undo | **new** `vectra-raster` | L+ | 1.1 (placement), 1.9 | planned |
| **2.2** | **Compositing & GPU** — tile atlas, brush shader passes, layer compositing in the vector frame, MSAA/supersampling, per-frame budgets | render (+ `raster_gpu`), wasm | L | 2.1 | planned |
| **2.3** | **WASM boundary** — `RasterEngine`/`BrushSettings`/`Selection` objects, binary tile channel, pointer-rate event ingestion, `RasterProvider` trait, `EvaluatedPrimitive::Raster` | wasm, core, geometry, render | M | 2.1 | planned |
| **2.4** | **Document container v2** — chunked `.vectra` (JSON + binary resource chunks), content hashes, v1→v2 migration | file, core, desktop | M | 2.1 | planned |
| **2.5** | **UI: the paint surface** — brush palette, dynamics editors, selection tools + marching ants, transform-selection, real colour picker, full-screen canvas | web | L | 2.2 | planned |

**Vertical-slice rule (risk mitigation):** before the full brush system, ship
*one* brush, *one* layer, *one* blend mode, tile undo and PNG out, green.

**Exit criteria:** raster laws green; blend modes match a CPU reference; 60 fps at
4096² on the reference device; PNG/JPEG export; a smoke run that paints, undoes
and compares pixels.

---

## 5. Phase 3 — Unified Design System

**Goal.** One document carries vectors, raster, text, components and variables;
a UI designer builds a component library with variants and themes, and the code
export reflects it.

| id | workstream | crates touched | size | depends on | status |
|---|---|---|---|---|---|
| **3.1** | **One graph (the UDG landing)** — layers/artboards become node kinds, single parent + ordered children, v1/v2→v3 migration through commands | core, dependency, file, wasm, export, web | L | 2.4 | planned |
| **3.2** | **Components v2** — variants (multi-axis), overrides, swap/detach, nested instances, external libraries | core, procedural, wasm, web | L | 3.1 | planned |
| **3.3** | **Variables v2 & modes** — typed variables + aliases, collections, modes (light/dark/density), token export | core, expression, wasm, web | M | 3.1 | planned |
| **3.4** | **Auto-layout** — stack/grid containers, padding/gap/alignment, layout constraints integrated with the solver | core, constraints, wasm, web | L | 3.1, 1.2 | planned |
| **3.5** | **Colour management** — f32 linear working space, sRGB/P3 output, OKLab/OKLCH, ICC import, premultiplied alpha, dithering, colour variables | core, geometry, render, export, wasm, web | L | 1.7 | planned |
| **3.6** | **Typography v2** — variable-font axes, OpenType feature toggles, bold/italic matching, bidi + vertical, area text/wrapping, text styles, in-canvas caret + IME | geometry, core, wasm, web | L | 3.1 | planned |
| **3.7** | **Procedural expansion** — radial, along-path, symmetry, scatter, node instancing, node-graph UI, noise → raster grain | procedural, core, wasm, web | M | 3.1, 2.1 | planned |

**Exit criteria:** a 2-axis variant library with a light/dark mode pair drives a
small UI screen (auto-layout, text styles, colour-managed gradients, a raster
texture); the document exports SVG + PNG ×3 + a parametric React component; the
v1→v3 migration re-saves byte-stable.

---

## 6. Phase 4 — The Zoah Advantage

**Goal.** The engine does work the designer would otherwise do by hand, and the
code export is good enough to be the handoff.

| id | workstream | crates touched | size | depends on | status |
|---|---|---|---|---|---|
| **4.1** | **Real AI, wired honestly** — host-injected provider behind `ChatPlanner`, streaming, tool-use loop, plan editing, **constraint inference**, procedural macros from free text, AI eval corpus in CI | ai, wasm, desktop, web | L | 1.2, 3.1 | planned |
| **4.2** | **Code export polish** — design tokens → CSS vars/Tailwind, React variants/props/slots, Vue/Svelte exporters, SVG→JSX, asset pipeline, `vectra export` CLI | export, wasm | M | 3.2, 4.1 | planned |
| **4.3** | **Performance & instrumentation** — `criterion` benches for the MES §18 tiers, background offload tier, CI perf tracking, in-app perf HUD | all, tools | M | — | planned |
| **4.4** | **Interop expansion** — PDF export, raster import (PNG/JPEG → image nodes), demand-driven formats | export, import, wasm | M | 2.1 | planned |

---

## 7. Gate shape (every phase, every workstream)

```bash
# Rust
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --offline -- -D warnings
cargo test --workspace --lib --tests --offline --no-fail-fast
# Web
cd apps/vectra-web && npm run typecheck && npm run test:ui && npm run smoke
# New laws must be cited in FEATURE_MATRIX.md before the workstream closes.
```

*(Until 1.0 fixes `tools/check.sh`, call cargo directly — see
`ENGINE_STATUS.md` §"Building offline".)*

---

## 8. The critical path

```
1.0 hygiene/CI ─▶ (unblocks every claim)
1.1 transforms ─┬─▶ 1.3 pen/selection
                ├─▶ 1.4 boolean containers
                ├─▶ 1.6 masks & compositing
                ├─▶ 1.8 SVG import
                └─▶ 1.2 constraint glyphs
1.2 vocabulary ══╗
1.5, 1.7, 1.9 independent ─┘
                └─▶ 2.x raster (placement + painterly blend) ─▶ 3.1 one graph ─▶ 3.2/3.3/3.4 ─▶ 4.1 AI advantage
```

---

## 9. Decisions pending ratification (each becomes an ADR in ARCHITECTURE.md §9)

1. The UDG definition (`ARCHITECTURE.md` §5.0, ratification proposal **D1**;
   §4.1 lists the five clauses that are not yet true).
2. Transforms live in `vectra-core`; evaluation in geometry/render; the UI never
   owns transform maths (**D2**).
3. Operations may consume other operations' outputs (workstream 1.4) (**D3**).
4. Masks are node references, not materialised geometry (workstream 1.6) (**D4**).
5. Container v2 lands in Phase 1 or Phase 2 — only Phase 2 needs it; earlier
   avoids a second migration (**D5**, currently proposed: Phase 2).
6. Procedural expansion (3.7) may be pulled forward into Phase 2 if parametric
   pattern work becomes a product priority (**D6**).

---

## 10. Change log

| date | change | reason |
|---|---|---|
| 2026-10-07 | Roadmap created from the audit; Phase 0 closed; Phase 1 sized; four living documents seeded | `VECTRA_AUDIT_AND_ROADMAP.md` |
