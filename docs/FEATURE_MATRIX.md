# VECTRA — FEATURE MATRIX

> **Living document.** Owner: engineering lead · Review trigger: every PR that
> changes a capability's status · Source of truth for *what actually exists*.
>
> **The rule: no status without a witness.** An `IMPLEMENTED` row must name a test
> (or a smoke step) that proves it; a `PARTIAL` row must name what it does *not*
> promise; a `MISSING` row must name the workstream that will create it, **or
> carry `—` and be listed below as deliberately unscheduled.** Exactly two rows
> are in that second class today — `vec.nurbs` and `vec.polygon-star` — because
> no current workstream commits to them. They are real gaps, not oversights: if
> polygonal/star primitives become a product requirement they belong in **1.8**
> (the workstream that widens the path and primitive model), and NURBS is
> unscheduled until a designer asks for it. This matrix was audited on 2026-10-07
> against commit `5422a99` by execution (650 Rust / 146 UI / 79 smoke green) plus
> source inspection — see
> [`VECTRA_AUDIT_AND_ROADMAP.md`](../VECTRA_AUDIT_AND_ROADMAP.md) Part I.

## Status vocabulary (exactly five words, used everywhere)

| status | meaning |
|---|---|
| `IMPLEMENTED` | fully functional against its declared spec, gated by tests, production-ready |
| `PARTIAL` | core logic exists; edge cases, UI integration or performance work missing |
| `SCAFFOLDED` | crate/file exists with basic structs; no real logic or tests |
| `MISSING` | not present in the repository at all |
| `DEPRECATED` | replaced by a newer approach and should be removed |

Evidence abbreviations: `wasm:` = `crates/vectra-wasm/tests/`, `web:` =
`apps/vectra-web/`. Workstream ids (`1.1`, `2.3`, …) are defined in
[`ROADMAP.md`](./ROADMAP.md).

---

## 1. Core Document Graph

| id | capability | status | evidence | limits | phase |
|---|---|---|---|---|---|
| `core.ids` | Stable UUID ids; nodes and operations share one id space | `IMPLEMENTED` | `crates/vectra-core/src/ids.rs`; `crates/vectra-core/tests/undo_redo.rs` | — | — |
| `core.params` | `Parameter<T>` sources: Literal, Variable, Expression, Animated, Procedural, Interaction | `IMPLEMENTED` | `crates/vectra-core/src/param.rs`; `tests/param_resolution.rs` | — | — |
| `core.commands` | 62 command variants, each returning an exact inverse | `IMPLEMENTED` | `crates/vectra-core/src/command.rs`; `tests/undo_redo.rs` | — | — |
| `core.undo` | Byte-exact undo/redo (event-sourced, no full-document clones) | `IMPLEMENTED` | `crates/vectra-core/tests/undo_redo.rs`; `web:tests/…` | — | — |
| `core.graph` | Derived dependency graph, dirty propagation, atomic cycle gate | `IMPLEMENTED` | `crates/vectra-dependency/src/graph.rs`; `tests/dependency_laws.rs`, `incremental_laws.rs`, `workspace_laws.rs` | — | — |
| `core.layers` | Layers with visibility/lock/alpha-lock/clip flags, nesting, reorder, cross-layer moves | `IMPLEMENTED` | `crates/vectra-core/src/layers.rs`; `tests/tree_laws.rs` | group subtree expansion is a panel polish item | — |
| `core.artboards` | Artboard registry, bounds/background editing, active board, scoped export | `IMPLEMENTED` | `layers.rs` (`ArtboardRegistry`); `wasm:tests/export_laws.rs`; smoke 44–46 | — | — |
| `core.summary` | `DocumentSummary` projection (ids, names, kinds, variables, constraints, motion, procedural) | `IMPLEMENTED` | `crates/vectra-core/src/summary.rs`; `tests/summary.rs` | — | — |
| `core.node-kinds` | 6 kinds: Rectangle, Circle, Arc, Path, Text, Group | `PARTIAL` | `crates/vectra-core/src/document.rs` | no Ellipse/Polygon/Star/Line/Frame/Image kinds | 1.1, 2.1 |
| `core.transforms` | Per-node local→parent transforms, rotation/scale/skew | `MISSING` | — | **the #1 blocker**: everything is absolute coordinates | 1.1 |
| `core.hierarchy` | One parent + one ordered child list per node | `MISSING` | three parallel mechanisms today (layer membership, `child_parent`, `Document.order`) | migration risk | 3.1 |
| `core.components` | Component master/instance, props (Scalar+Color), `PropLaw` Direct/Derived, icon-set ladder | `PARTIAL` | `crates/vectra-core/src/component.rs`; `wasm:tests/component_laws.rs` (11) | no variants, no overrides, no libraries, no swap/detach | 3.2 |
| `core.variables` | Document variables (`f64` only) | `PARTIAL` | `param.rs` / `document.rs` | no colour/string/bool/alias types, no modes | 3.3 |
| `core.styles` | Shared paint/text/effect styles, swatches, palettes | `MISSING` | — | every node re-authors its appearance | 3.2, 3.6 |
| `core.resources` | Binary resource table (fonts, raster tiles, grain textures) with content hashes | `MISSING` | fonts are an in-memory library only | blocks raster persistence | 2.4 |

## 2. Vector Geometry

| id | capability | status | evidence | limits | phase |
|---|---|---|---|---|---|
| `vec.primitives` | Rectangle (clamped corner radius), Circle, Arc, Path, Text, Group | `IMPLEMENTED` | `crates/vectra-geometry/src/{evaluator,paths}.rs`; `tests/geometry_eval.rs` | six kinds only | — |
| `vec.bezier` | Quadratic + cubic segments with parametric control points | `IMPLEMENTED` | `document.rs` `PathSegment`; `crates/vectra-draw/src/bezier.rs` | no conics/arcs in the path model | 1.8 |
| `vec.true-arcs` | Lossless arc canonicalisation (`start ∈ [0,TAU)`, `sweep ∈ [0,TAU]`); one SVG `A` on export | `IMPLEMENTED` | `src/angles.rs`; `src/svg.rs::arc_data`; `tests/geometry_eval.rs` | internal booleans flatten arcs at 64 segs/turn | — |
| `vec.ellipse` | Ellipse / elliptical arcs (rx≠ry) | `MISSING` | — | must be approximated as paths | 1.8 |
| `vec.nurbs` | NURBS / B-splines | `MISSING` | — | — | — |
| `vec.polygon-star` | Polygon / Star / Spiral / Line primitives | `MISSING` | — | — | — |
| `vec.path-edit` | Direct selection: anchors, handle dragging, alt-break, close | `IMPLEMENTED` | `vectra-draw/src/pen.rs`; `wasm:src/draw.rs`; `web:engine/draw/path-edit.ts`; `draw/tests/draw_laws.rs` | — | — |
| `vec.brush-fit` | Freehand → fitted cubics; pressure-else-velocity width; outline expansion | `IMPLEMENTED` | `vectra-draw/src/{fit,stroke,outline}.rs` | vector result only; no tilt/texture | 2.1 |
| `vec.quickshape` | Hold-still snap to circle/rectangle | `IMPLEMENTED` | `vectra-draw/src/quickshape.rs`; `draw_laws.rs` | two shapes | — |
| `vec.curve-refit` | Refit flattened operation results back to cubics | `MISSING` | — | results degrade with each chained op | 1.4 |
| `vec.stroke-to-path` | Outline an authored stroke into a fillable region | `MISSING` | — | blocks Expand / boolean-on-stroke | 1.4 |
| `vec.snap` | Snap providers (grid, points, edges, tangents, guides) with ranked candidates | `MISSING` | only QuickShape + drag rounding exist | — | 1.3 |
| `vec.regions` | Planar arrangement: faces, crossings, spans, holes | `IMPLEMENTED` | `src/regions.rs`; `tests/region_laws.rs`; `prop_two_overlapping_circles_make_exactly_three_regions` | flattened at `FLATTEN_TOLERANCE = 0.05`; O(n²) | 1.5 |

## 3. Constraint Solver

| id | capability | status | evidence | limits | phase |
|---|---|---|---|---|---|
| `con.linear` | Cassowary-backed linear solve, 4 strengths, variable pool with GC | `IMPLEMENTED` | `crates/vectra-constraints/src/{solver,pool,rows}.rs`; `rows_are_unit_coefficient_equalties` | float slots only | — |
| `con.drop` | Deterministic weakest-drop pre-pass + typed `Unsatisfiable` for Required-vs-Required | `IMPLEMENTED` | `src/plan.rs`; `wasm:tests/constraint_laws.rs` (7) | pairwise equality-on-slot only | — |
| `con.kinds` | Coincident, Horizontal, Vertical, Parallel, Perpendicular, EqualLength, Distance, Angle | `PARTIAL` | `crates/vectra-core/src/constraint.rs` | Parallel/Perpendicular are axis-offset forms, not vector relations | 1.2 |
| `con.tangent` `con.symmetric` `con.concentric` `con.collinear` `con.midpoint` `con.equal-radius` | the missing geometric families | `MISSING` | — | needs the nonlinear pass | 1.2 |
| `con.inequalities` | ≥ / ≤ constraints | `MISSING` | — | — | 1.2 |
| `con.nonlinear` | `solve_nonlinear` (MES §9) for true tangency/parallel/symmetry | `MISSING` | — | — | 1.2 |
| `con.drag` | Drag as native Cassowary edit variables; one undo per gesture; re-anchor on release | `IMPLEMENTED` | `wasm:tests/drag_laws.rs` (8); `vectra-constraints/src/solver.rs` | 2 slots, kinds without position slots not draggable | — |
| `con.glyphs` | Constraint glyphs/indicators on the canvas | `MISSING` | UI shows a text list only | the prompt's "UI glyphs" | 1.2 |
| `con.ui-nary` | N-ary selection, click-geometry constrain, strength editor | `MISSING` | 4 buttons + pair-only builders | — | 1.2 |
| `con.inference` | Detect near-coincident/parallel/equal geometry and suggest constraints | `MISSING` | — | — | 4.1 |
| `con.surface` | Dropped/skipped diagnostics + solver stats line in the UI | `IMPLEMENTED` | `web:engine/view-model.ts::solverSummary`; `App.tsx` Constraints panel | — | — |

## 4. Boolean & Operations

| id | capability | status | evidence | limits | phase |
|---|---|---|---|---|---|
| `op.boolean` | Union / Subtract / Intersect / Exclude via `geo::BooleanOps`, even-odd | `IMPLEMENTED` | `crates/vectra-operations/src/ops.rs`; `tests/operation_laws.rs` (12) | exactly two inputs; closed regions only | — |
| `op.offset` | Grow/shrink with round joins (`geo::Buffer`) | `IMPLEMENTED` | `ops.rs`; `operation_laws.rs` | closed regions only | — |
| `op.fillet` | Round convex corners with CAD clamping | `PARTIAL` | `ops.rs::fillet` | reflex corners left sharp | 1.4 |
| `op.mirror` | Reflect across a parametric axis with winding repair | `IMPLEMENTED` | `ops.rs::mirror`; `operation_laws.rs` | one axis per node | — |
| `op.non-destructive` | Sources untouched; result is a first-class virtual node with id + style | `IMPLEMENTED` | `crates/vectra-core/src/operation.rs`; `wasm:tests/operation_laws.rs` | — | — |
| `op.nary` | n-ary operands; an operation may consume another operation | `MISSING` | documented nested-union limitation (Task 10.6) | blocks Pathfinder-class work | 1.4 |
| `op.chamfer` | Chamfer corners | `MISSING` | — | — | 1.4 |
| `op.expand` | Outline Stroke / Expand Appearance | `MISSING` | — | — | 1.4 |
| `op.divide-trim-merge` | Pathfinder Divide / Trim / Merge | `MISSING` | — | — | 1.4 |
| `op.fill-rule` | Per-node nonzero vs even-odd | `MISSING` | even-odd hard-wired | lossy SVG import | 1.4 |
| `op.curve-results` | Curve-preserving boolean/offset/fillet output | `MISSING` | `multi_polygon_to_path` is polyline | — | 1.4 |

## 5. Smart Fills & Region Graph

| id | capability | status | evidence | limits | phase |
|---|---|---|---|---|---|
| `sf.faces` | Planar faces with holes intact, smallest-face point query | `IMPLEMENTED` | `src/regions.rs`; `tests/region_laws.rs`; `prop_every_face_is_a_closed_path_that_partitions_the_union` | flattened tolerance | — |
| `sf.fill` | Smart Fill as a parametric node keyed by a **seed point**; re-derives when boundaries move | `IMPLEMENTED` | `crates/vectra-operations/tests/smart_fill_laws.rs`; `prop_moving_a_boundary_updates_the_fill_geometry` | seed can leave every face (reported, not silent) | — |
| `sf.retire` | A fill whose seed leaves every face removes its geometry + raises `smart-fill-empty` | `IMPLEMENTED` | `smart_fill_laws.rs`; `prop_a_fill_whose_seed_leaves_every_face_reports_smart_fill_empty` | — | — |
| `sf.spans` | Spans (arc length between crossings) + Break Path at Intersections (non-destructive, one undo) | `IMPLEMENTED` | `regions.rs`; `wasm:src/lib.rs::break_path`; `prop_breaking_a_span_yields_two_closed_paths_with_no_gaps`; smoke 78–79 | `Path` kind only | — |
| `sf.colordrop` | ColorDrop into an enclosed area creates a Smart Fill above the boundaries | `IMPLEMENTED` | `web:engine/draw/{colordrop,regions}.ts`; `RegionOverlay.tsx`; smoke 77 | — | — |
| `sf.gap-tolerance` | Close near-miss gaps (Illustrator-style Smart Fill) | `MISSING` | — | exact closure required today | 1.5 |
| `sf.perf` | Region cache + >50 ms background offload (MES §18 tier 3) | `MISSING` | main-thread pairwise booleans | — | 1.5 |
| `sf.curves` | Curve-preserving faces | `MISSING` | faces are polylines | export quality | 1.5 |

## 6. Procedural Graph

| id | capability | status | evidence | limits | phase |
|---|---|---|---|---|---|
| `proc.graph` | Typed ports (Scalar/Point/Points/Path/Region/Color), derived topology, published table, single cycle gate | `IMPLEMENTED` | `crates/vectra-procedural/src/{engine,nodes}.rs`; `tests/procedural_laws.rs` (12); `wasm:tests/procedural_laws.rs` (10) | no second graph by design | — |
| `proc.noise` | Deterministic splitmix value noise, byte-identical native/wasm | `IMPLEMENTED` | `src/noise.rs`; `procedural_laws.rs` | — | — |
| `proc.kinds` | Source, Grid, Repeat (linear), Noise, Smooth, ComponentMaster, Component | `IMPLEMENTED` | `crates/vectra-core/src/procedural.rs` | Grid emits points+region, not shape copies | — |
| `proc.radial` | Radial/rotational repeat | `MISSING` | — | — | 3.7 |
| `proc.along-path` | Along-path repeat | `MISSING` | — | — | 3.7 |
| `proc.symmetry` | Mirror/rotational symmetry generator | `MISSING` | Mirror exists only as an operation | — | 3.7 |
| `proc.scatter` | Random/scatter placement of copies | `MISSING` | — | — | 3.7 |
| `proc.instancing` | Repeat *nodes* as editable instances | `MISSING` | Repeat duplicates region geometry | — | 3.7 |
| `proc.ui` | Node-graph canvas UI | `MISSING` | dropdown-wired panel only | — | 3.7 |

## 7. Motion & Animation

| id | capability | status | evidence | limits | phase |
|---|---|---|---|---|---|
| `motion.spring` | Closed-form spring (`stiffness`, `damping`, document-stored anchor) — scrub-safe, no integrator | `IMPLEMENTED` | `crates/vectra-motion/src/spring.rs` (9 tests); `wasm:tests/motion_laws.rs` (7) | — | — |
| `motion.track` | Keyframe tracks (named channels), total sampler with segment diagnostics | `IMPLEMENTED` | `src/track.rs`; `the_sampler_is_total_including_outside_the_span`, `a_single_keyframe_track_is_a_constant` | **linear interpolation only** | 1.2→3.x |
| `motion.state` | State-driven branches (`set_state`), spring-smoothed, not undoable by design | `PARTIAL` | `param.rs` `MotionBinding::StateDriven`; `motion_laws.rs` | boolean flags, not a state machine | — |
| `motion.graph` | State/Track vertices, motion-target recursion, dirty propagation | `IMPLEMENTED` | `crates/vectra-dependency/tests/motion_graph_laws.rs` | — | — |
| `motion.easing` | Easing curves / per-key interpolation modes | `MISSING` | deferred in `track.rs` | — | 3.x |
| `motion.timeline-ui` | Keyframe editor, dope sheet, playhead, playback controls | `MISSING` | a time slider and a demo button exist | — | 1.2→3.x |
| `motion.export` | Animation export (CSS/SMIL/Lottie/GIF/MP4/WebM) | `MISSING` | — | animation cannot leave Vectra | 4.2 |
| `motion.non-scalar` | Animate colours, paths, text runs | `MISSING` | bindings are `f64` | — | 3.3 |

## 8. Raster & Brush Engine

**Whole subsystem `MISSING`.** The rows below are the checklist Phase 2 must turn
green; none of them exists today.

| id | capability | status | evidence | limits | phase |
|---|---|---|---|---|---|
| `raster.engine` | Pixel/tile model, raster layer, tile map | `MISSING` | no pixel type anywhere in the tree | — | 2.1 |
| `raster.brush` | Dab pipeline with spacing/flow/hardness/jitter/scatter/angle | `MISSING` | brush is vector fitting only | — | 2.1 |
| `raster.dynamics` | Pressure, tilt, azimuth, velocity dynamics with editable curves | `MISSING` | pressure exists for the *vector* brush only (`draw/stroke.rs`) | — | 2.1 |
| `raster.texture` | Grain/texture brushes, stamp brushes | `MISSING` | — | — | 2.1 |
| `raster.smudge` | Smudge / blend / wet mixing | `MISSING` | — | — | 2.1 |
| `raster.eraser` | Raster eraser (and pixel-exact erase undo) | `MISSING` | — | — | 2.1 |
| `raster.blend-modes` | Procreate/Photoshop-class blend set (16+ modes) | `PARTIAL` | 4 modes ship (`style.rs`), verified by `render/tests/gpu_laws.rs` | not a raster engine, but the mode set is shared | 1.6 |
| `raster.selection` | Marquee/lasso/polygon/wand, feather, grow, invert | `MISSING` | — | — | 2.1 |
| `raster.alpha-lock` | Alpha lock over *pixels* | `PARTIAL` | alpha lock exists with vector semantics (`layers.rs`, `ops/clip.rs`) | not pixel-accurate | 2.1 |
| `raster.gpu` | Tiled GPU canvas, per-tile streaming, MSAA | `MISSING` | renderer is vector triangle-list only | — | 2.2 |
| `raster.image-import` | Decode PNG/JPEG/WebP into image nodes | `MISSING` | no image crate in the tree | — | 4.4 |
| `raster.export` | PNG/JPEG export at scale | `MISSING` | SVG + React text only | **Phase-1 exception: 1.9 adds PNG readback→encode** | 1.9, 2.1 |

## 9. Typography

| id | capability | status | evidence | limits | phase |
|---|---|---|---|---|---|
| `text.node` | Parametric text node; every numeric slot a `Parameter<f64>` | `IMPLEMENTED` | `document.rs` `NodeKind::Text`; `src/text.rs`; `tests/text_laws.rs` (21) | — | — |
| `text.shaping` | HarfBuzz-grade shaping (rustybuzz) with GSUB/GPOS: kerning, ligatures, marks | `IMPLEMENTED` | `src/text.rs`; shaping cache + stats | default features only | — |
| `text.on-path` | One-way path binding, arc-length placement, analytic tangents, readability flip, truncation diagnostic | `IMPLEMENTED` | `src/text.rs::layout_text_on_path`; `text_laws.rs`; smoke 71–72 | LTR only | — |
| `text.outline` | Non-destructive Outline Text (per-glyph plans travel with the command) | `IMPLEMENTED` | `Command::OutlineText`; `wasm:tests/motion_laws.rs` neighbours; smoke 73 | — | — |
| `text.fonts` | Bundled face (DejaVu subset, 55 KB) + host registration + fallback diagnostic | `IMPLEMENTED` | `crates/vectra-geometry/assets/vectra-sans.ttf`; `wasm::register_font` | licence file present; no bold/italic variants | — |
| `text.bold-italic` | Weight/style faces and matching | `MISSING` | a family is one file | — | 3.6 |
| `text.variable-fonts` | fvar/gvar axes + named instances | `MISSING` | no axis handling anywhere | — | 3.6 |
| `text.features` | OpenType feature toggles (`smcp`, `onum`, `ssXX`, `frac`, …) | `MISSING` | — | — | 3.6 |
| `text.bidi` | Bidi paragraphs, RTL layout, vertical text | `MISSING` | layout is per-line LTR | — | 3.6 |
| `text.area` | Text frames, wrapping, justification, hyphenation, tabs | `MISSING` | point text only | — | 3.6 |
| `text.styles` | Shared text styles | `MISSING` | — | — | 3.6 |
| `text.decorations` | Underline / strike / overline | `MISSING` | — | — | 3.6 |
| `text.caret-ime` | In-canvas caret, selection, IME composition | `MISSING` | typing happens in the panel | — | 3.6 |

## 10. Color Management

| id | capability | status | evidence | limits | phase |
|---|---|---|---|---|---|
| `color.model` | `Color { r,g,b,a: u8 }` sRGB bytes, hex parse/serialise | `IMPLEMENTED` | `crates/vectra-core/src/geom.rs` | the *entire* colour model | — |
| `color.spaces` | Linear working space, Display-P3, ICC profiles, gamut handling | `MISSING` | no f32/ICC/P3 code | — | 3.5 |
| `color.oklch` | OKLab/OKLCH editing + interpolation | `MISSING` | — | — | 3.5 |
| `color.variables` | Colour variables/swatches/palettes as resources | `MISSING` | `Document.variables` is `f64` | — | 3.3 |
| `color.gradients` | Linear + radial gradients, document-space frames, `userSpaceOnUse` export | `IMPLEMENTED` | `style.rs` `Paint`; `src/paint.rs`; `svg.rs` | no conic/mesh, no stroke gradients | — |
| `color.gradient-conic` | Conic/angular gradients | `MISSING` | — | SVG has no 1.1 equivalent (documented fallback needed) | 1.7 |
| `color.gradient-stroke` | Gradients along a stroke | `MISSING` | — | — | 1.7 |
| `color.gradient-editor` | Multi-stop editor: add/remove/drag/reverse/alpha per stop | `PARTIAL` | `AppearancePanel.tsx` gradient bar (drag stops) | two-stop oriented | 1.7 |
| `color.interpolation` | Defined interpolation space (sRGB → linear → OKLab) | `MISSING` | stops lerped without a declared space | **silently wrong midpoints today** | 1.7 |
| `color.blend-modes` | W3C compositing set (16+) implemented in WGSL | `PARTIAL` | 4 modes (`Normal/Multiply/Screen/Overlay`) + backdrop pipeline; `render/tests/gpu_laws.rs` | — | 1.6 |
| `color.picker` | Real colour picker (HSV/OKLCH, eyedropper, recents, palette) | `PARTIAL` | browser `<input type="color">` + hex | — | 2.5, 3.5 |
| `color.management-export` | Profiles/interpolation hints in output | `MISSING` | — | — | 3.5 |

## 11. Masks & Compositing

| id | capability | status | evidence | limits | phase |
|---|---|---|---|---|---|
| `mask.alpha-lock` | Alpha lock: new artwork clipped to the layer's existing content | `IMPLEMENTED` | `layers.rs`; `crates/vectra-operations/src/clip.rs`; `wasm:tests/alpha_lock_laws.rs` (5) | vector semantics, at the drawing boundary | — |
| `mask.clipping-mask` | Clip to the layer below (live `region ∩ mask`) | `IMPLEMENTED` | `clip.rs::mask_region`; `wasm:tests/alpha_lock_laws.rs`; smoke 70 | layer-level only | — |
| `mask.holes` | Even-odd nesting → holes across render, regions and export | `IMPLEMENTED` | `regions.rs`; `tests/clip_laws.rs` (8) | nonzero-rule files are lossy | — |
| `mask.nodes` | Mask node references with invert / feather / alpha-luma semantics | `MISSING` | two layer booleans only | — | 1.6 |
| `mask.group` | Group masks, alpha inheritance, knockout, isolation modes | `MISSING` | — | — | 1.6 |
| `mask.export` | `<mask>` / `<clipPath>` emission in SVG | `MISSING` | clipping is resolved geometrically on export | — | 1.6 |
| `mask.raster` | Pixel masks/feather for raster layers | `MISSING` | — | — | 2.1 |

## 12. Export & Interoperability

| id | capability | status | evidence | limits | phase |
|---|---|---|---|---|---|
| `io.svg-export` | Semantic SVG (rect/circle/arc-`A`/path/group), artboard scope, gradients, y-flip, warnings, `data-vectra-*` ids | `IMPLEMENTED` | `crates/vectra-export/src/svg.rs`; `tests/export_laws.rs` (17); smoke 32–33 | text exports as paths only | — |
| `io.react` | Parametric React component (`width={base * 2}`, required props from variables) | `IMPLEMENTED` | `src/react.rs`; `export_laws.rs`; smoke 34 | React only | — |
| `io.vectra` | `.vectra` container v1 (header + gzip, pinned header), replay-based load, atomic write, verify-by-replay | `IMPLEMENTED` | `crates/vectra-file/src/{format,plan}.rs`; `tests/roundtrip_laws.rs` (14); `apps/vectra-desktop/src-tauri/src/verify.rs` | desktop only; web refuses honestly | — |
| `io.svg-import` | SVG import with fidelity report and true round trip | `MISSING` | `ExportError` reserves "a future importer" | — | 1.8 |
| `io.text-as-text` | `<text>` emission with font embedding/subsetting | `MISSING` | — | — | 1.8 |
| `io.png` | PNG export (1×/2×/3×, transparency, artboard scope) | `MISSING` | — | **no pixel output at all today** | 1.9 |
| `io.pdf` | PDF/EPS export | `MISSING` | — | — | 4.4 |
| `io.vue-svelte` | Vue/Svelte exporters (IR consumers) | `MISSING` | named in docs, not built | — | 4.2 |
| `io.tokens` | Design-token export (CSS vars, Tailwind) | `MISSING` | — | — | 3.3 |
| `io.cli` | `vectra export` CLI / MCP surface | `MISSING` | — | — | 4.2 |
| `io.desktop-files` | Native open/save dialogs, atomic writes, roundtrip verification | `IMPLEMENTED` | `apps/vectra-desktop/src-tauri/src/{main,file_io,verify}.rs` | `bundle.active: false` (no installers) | — |

## 13. AI Command Interface

| id | capability | status | evidence | limits | phase |
|---|---|---|---|---|---|
| `ai.schema` | The AI's schema *is* `Command`; `$new:<slug>` id tokens; no shim DSL | `IMPLEMENTED` | `crates/vectra-ai/src/schema.rs`; `tests/ai_laws.rs` (34) | — | — |
| `ai.grounding` | `DocumentSummary` + system prompt as the only context | `IMPLEMENTED` | `crates/vectra-core/src/summary.rs`; `src/prompt.rs`; `wasm:tests/ai_laws.rs` (9) | — | — |
| `ai.self-correction` | Typed `VectraError` → `Correction` → retry (`MAX_ATTEMPTS`); exhaustion = nothing applied | `IMPLEMENTED` | `src/exec.rs`; `ai_laws.rs` | two retries, single-shot plans | — |
| `ai.preview-execute` | Preview without mutation; execute/reject wholesale | `IMPLEMENTED` | `exec::preview` / `execute`; `web` AI panel | no per-command accept/reject | 4.1 |
| `ai.macros` | Five one-tap structural macros + prose receipts in the designer's voice | `IMPLEMENTED` | `wasm::structural_macros`; `MagicBar.tsx`; smoke 55–57 | hard-coded prompts | 4.1 |
| `ai.model` | A real language model behind `ChatPlanner` | `MISSING` | the shipped planner is `HeuristicPlanner` (a phrase matcher); `ChatPlanner` wraps a caller `Fn` with no implementation in-tree | **"AI" today = keyword table** | 4.1 |
| `ai.constraint-inference` | Geometric inference → suggested real constraints | `MISSING` | — | — | 4.1 |
| `ai.plan-editing` | Streaming, tool-use loop, per-command edit UI | `MISSING` | — | — | 4.1 |
| `ai.eval-harness` | Prompt→expected-plan corpus in CI | `MISSING` | — | — | 4.1 |
| `ai.macro-generation` | Macros generated as plans over procedural + constraints | `MISSING` | — | — | 4.1 |

## 14. UI/UX Shell

| id | capability | status | evidence | limits | phase |
|---|---|---|---|---|---|
| `ui.canvas` | WebGPU canvas driven by wasm snapshots; camera/pan/zoom; nav overlay; grid; artboard box; diagnostics strip | `IMPLEMENTED` | `crates/vectra-wasm/src/render.rs`; `wasm:tests/navigation_laws.rs`, `render_laws.rs`; smoke 27–31 | no MSAA; no full-screen mode | — |
| `ui.tools` | 6 tools (Select, Direct, Pen, Brush, Text, Smart Fill) with shortcuts + RULE-4 panel gate | `IMPLEMENTED` | `web:engine/draw/tools.ts`; `ToolPalette.tsx`; smoke 62 | palette is in-panel row, not a floating toolbar | 1.3, 2.5 |
| `ui.panels` | Layers (tree, drag/drop, ops rows), Appearance (stacks/blends/gradient bar), Text, Components, AI, Export, Constraints, Procedural, Motion, Dependencies, Event log | `IMPLEMENTED` | `web:src/components/*`; `web:tests/panels*.test.tsx` | the last two are debug chrome in the default layout | 1.0 |
| `ui.gestures` | Touch recognizer (paint vs pan vs zoom), no context menu/scroll, StreamLine slider, ColorDrop | `IMPLEMENTED` | `engine/draw/{touch,streamline,colordrop}.ts`; smoke 60–61, 75 | — | — |
| `ui.theme` | Dark theme (CSS-variable console theme) | `PARTIAL` | `App.css` | **no light mode, no theme switch** | 1.0 |
| `ui.iconography` | Proper icon set | `MISSING` | emoji/unicode glyphs as chrome | — | 1.0 |
| `ui.menus-settings` | Menu bar, preferences, command palette | `MISSING` | ⌘K is the AI bar | — | 4.1 |
| `ui.immersive-canvas` | Canvas-first layout, floating toolbars, collapsible/resizable panes | `MISSING` | canvas is one panel in a scroll column | — | 1.0, 2.5 |
| `ui.a11y` | Accessibility pass (focus, roles, contrast, keyboard coverage) | `PARTIAL` | some `aria-*` + `aria-pressed`; shortcut coverage V/A/P/B/T/F only | — | 1.0 |
| `ui.dev-chrome-gate` | Debug panels behind an explicit developer flag | `MISSING` | Dependencies + Engine events ship by default | — | 1.0 |
| `ui.desktop` | Tauri v2 host: native dialogs, `.vectra` on disk, verify-by-replay | `IMPLEMENTED` | `apps/vectra-desktop/**` | no installers bundled | — |

---

## 15. Summary counts (this matrix)

151 audited rows:

| status | rows |
|---|---|
| `IMPLEMENTED` | 58 |
| `PARTIAL` | 13 |
| `MISSING` | 80 |
| `SCAFFOLDED` | 0 |
| `DEPRECATED` | 0 |

Reading the counts honestly: the 58 `IMPLEMENTED` rows are concentrated in the
engine (graph, geometry, regions, springs, shaping, export, command spine); the
80 `MISSING` rows are concentrated in the *product* (raster 10, typography 8,
colour 7, interop 7, constraints 6, operations 6, procedural 6, UI 4). That
asymmetry — strong engine, missing surface — is the audit's central finding and
the reason Phase 1 starts with transforms and CI rather than with features.

## 16. How to update this file

1. A PR that changes a capability updates its row **in the same PR** (status,
   evidence, limits, phase).
2. `IMPLEMENTED` requires a witness: a test name, or a smoke step number, that
   the PR's CI run executed.
3. When a workstream from `ROADMAP.md` closes, every row it listed in its `phase`
   column must be moved off `MISSING`/`PARTIAL` or the workstream is not done.
4. `SCAFFOLDED` and `DEPRECATED` are legal states; entering `DEPRECATED` requires
   a deletion date in the PR description.
