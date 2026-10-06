# ⚙️ VECTRA — Engine-First, Graph-Oriented, Parametric Vector Design

> Geometry, logic, and motion are the **same editable system**.
> The engine only ever asks `Resolvable::resolve(ctx, time) -> T` —
> it never cares whether `T` came from a literal, a variable, an expression,
> a motion binding, a procedural port, or a live input.

This repo implements the **Master Engineering Specification (MES v1.0)**
(see [`MES.md`](./MES.md)). UI is a thin remote control; the Rust engine owns
all truth.

## Repository layout

```text
vectra/
├── Cargo.toml                  # Workspace (resolver v2, shared deps)
├── MES.md                      # Master Engineering Specification v1.0
├── crates/
│   ├── vectra-core/            # ✅ IDs, Parameter<T>, Document, Command/Undo
│   ├── vectra-geometry/        # ✅ EvaluatedScene, GeometryEvaluator, lyon paths, arcs
│   ├── vectra-constraints/     # ◌ Linear (Cassowary) + nonlinear solvers, dep graph
│   ├── vectra-expression/      # ◌ Lexer/parser/AST/compiler/incremental evaluator
│   ├── vectra-operations/      # ◌ Non-destructive boolean/offset/fillet/mirror/repeat
│   ├── vectra-procedural/      # ◌ Typed node graph, generators, modifiers
│   ├── vectra-motion/          # ◌ Timeline, springs, physics, state machines
│   ├── vectra-render/          # ◌ wgpu scene graph, GPU buffers, hit-testing
│   ├── vectra-export/          # ◌ Export IR, SVG, React/Vue codegen
│   └── vectra-wasm/            # ◐ wasm-bindgen command-bus API (Task 1.4)
├── apps/
│   ├── vectra-web/             # ✅ Dumb React remote + WASM engine (Task 1.4)
│   └── vectra-desktop/         # ◌ Future Tauri shell (reuses vectra-web UI)
└── tests/                      # Cross-crate scenario tests (per-crate proptests live in-crate)
```

✅ done · ◐ scaffolded, in progress · ◌ scaffolded, planned

## Crate dependency DAG (acyclic by construction)

```text
            ┌─────────────┐
            │ vectra-core │  ← dependency root. Owns data model + traits.
            └──────┬──────┘
     ┌────────┬────┴───┬────────┬──────────┬────────┐
     ▼        ▼        ▼        ▼          ▼        ▼
 geometry expression motion constraints procedural ...
     │        │        │        │          │
     └────────┴───┬────┴───┬────┴─────┬─────┘
                  ▼        ▼          ▼
             operations  render    export
                       ╲    │    ╱
                        ▼   ▼  ▼
                      vectra-wasm  ← integration only. No logic.
```

**Law:** owner crates implement core traits (`ExpressionEvaluator`,
`MotionEvaluator`, `ProceduralEvaluator`, `InteractionProvider`) and inject
them into `EvaluationContext`. Core never depends outward.

## Build & test

Prerequisites: stable Rust ≥ 1.75 (`rustup target add wasm32-unknown-unknown`
for the web build).

```bash
# Check everything
cargo check --workspace

# Core engine + property suites (Tasks 1.2 + 1.5)
cargo test -p vectra-core

# Geometry evaluation + property suites (Task 1.3)
cargo test -p vectra-geometry

# Lint + format (CI gates on these)
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
```

## Web remote (Task 1.4)

```bash
# Rebuild the engine for web (wasm-bindgen CLI must match Cargo.lock —
# the script enforces this)
bash apps/vectra-web/scripts/build-wasm.sh

# Run the host
cd apps/vectra-web
npm install
npm run dev     # → http://localhost:5173 (preview, layers, log)
npm run smoke   # Node E2E: real wasm, create → snapshot → bind → undo
npm run build   # tsc + production bundle
```

## Phase-1 task tracker (MES §20)

| Task | Scope | Status |
|------|-------|--------|
| 1.1 | Scaffold workspace + all crate manifests | ✅ done |
| 1.2 | `vectra-core`: `Parameter<T>`, `Document`, `Command`/undo | ✅ done |
| 1.3 | `vectra-geometry`: `NodeKind` eval → `EvaluatedScene` | ✅ done |
| 1.4 | `vectra-wasm` + minimal React/TS remote control | ✅ done |
| 1.5 | Proptest suites: resolution laws + undo laws | ✅ done (16 props) |
| 1.3-tests | Geometry proptests: identity, variable, arc, lyon, incremental | ✅ done (10 props) |
| 1.4-tests | WASM boundary tests + Node E2E smoke (6 steps) | ✅ done |

70 tests green workspace-wide (`cargo test --workspace`) + `npm run smoke` green.

MVP acceptance (MES §19) is tracked in `MES.md`.
