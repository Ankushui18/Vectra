# TASK 8.0 — Export Architecture

**Goal.** Turn Vectra from a design tool into a **code-generation engine**: the
drawing is a document, and the document compiles to code a human can keep.

**Authorization.** "Authorization acknowledged. Proceeding to Task 8.0."

**Status.** Delivered. All gates green:

| Gate | Result |
|---|---|
| `cargo test --workspace` | **390 passed, 0 failed** (was 366 → +16 export laws, +7 wasm boundary laws, +1 doc-test) |
| `cargo clippy --workspace --all-targets` | clean (no warnings) |
| `cargo fmt --all -- --check` | clean |
| `npm run smoke` | **39/39** (was 36/36 → +3 export steps) |
| `npm run typecheck` | clean (`tsconfig.json` + `tsconfig.tests.json`) |
| `npm run test:ui` | **34/34** (was 33 → +1 export-summary test) |
| `npm run build` | ok — `dist/assets/index-*.js` **220.46 kB** (was 217.34 kB), gzip 68.15 kB |

The dev server is running with the new panel: `npm run dev` → `http://localhost:5173/`.

---

## 1. The three rules, and where each one lives

### RULE 1 — semantic first, no bezier approximations

> "A `Circle` exports as `<circle cx cy r />`, an `Arc` must use the SVG `A`
> command. Only true `Path` nodes export as `<path>`."

The semantic shape is carried in the IR, not recovered from triangles:
`ExportGeometry` has `Rect`, `Circle`, `Arc`, `Path`, `Group` variants, and the
compiler **never** collapses them into one another.

* `Circle` → `<circle cx="…" cy="…" r="…" />`. `svg.rs` cannot even emit a path
  for a circle: the renderer for `ExportGeometry::Circle` has one arm.
* `Arc` → **one `A` command**: `M {x0} {y0} A {r} {r} 0 {large} {sweep 1} {x1} {y1}`,
  angles taken from the document's canonical form (`normalize_arc_angles`:
  sweep ∈ `[0, TAU)`). `large-arc-flag` ⇔ `sweep > π`; `sweep-flag` is always `1`
  (the document's y is up, the group's one `scale(1 -1)` mirror flips it);
  a **full turn** is emitted as two half arcs (one `A` cannot close a circle
  under SVG's endpoint rules); a degenerate sweep falls back to `M … L …` so the
  path is still valid.
* `Path` → lyon's serialization (`path_to_svg_data`), the one place a bezier may
  appear — because a bezier is what the user authored.
* A **computed** region (boolean / procedural output) has no primitive form to
  preserve, so it arrives at the exporter as `Path` with lyon's `d`. That is an
  honest statement about what it is, not an approximation of what it is not.

### RULE 2 — parametric codegen (the moat)

> "`width = $base * 2` exports `<Rect width={base * 2} />`, NOT
> `<Rect width="200" />`; extract all unique variables and declare them as typed
> props."

`ExportParam` carries `code` (the arithmetic), `value` (the number), and `vars`
(what the code reads). `ExportIR::props` is the sorted union of every `vars` in
the picture, so the component signature is derived from the drawing rather than
guessed. Emitted verbatim by `react.rs`:

```tsx
interface SceneProps {
  base: number; // 40
  gap: number;  // 8
}

export function Scene({ base, gap }: SceneProps) {
  return (
    <Group>
      {/* card */}
      <Rect x={0} y={0} width={base * 2} height={60} cornerRadius={gap} fill="#2266ee" opacity={1}/>
      {/* dot */}
      <Circle cx={140} cy={30} r={30} fill="#2266ee" opacity={1}/>
      {/* half */}
      <Arc cx={210} cy={30} r={30} startAngle={0} endAngle={3.141593} fill="#2266ee" opacity={1}/>
    </Group>
  );
}
```

Same document, as a **picture** (a picture is a picture — the number is the
number):

```xml
<?xml version="1.0" encoding="UTF-8"?>
<svg xmlns="http://www.w3.org/2000/svg" width="249.6" height="64.8" viewBox="-4.8 0 249.6 64.8" fill="none">
  <!-- Vectra export: 3 node(s), 2 prop(s) -->
  <g transform="translate(0 64.8) scale(1 -1)">
    <rect data-vectra-node="…" x="0" y="0" width="80" height="60" rx="8" fill="#2266ee"/>
    <circle data-vectra-node="…" cx="140" cy="30" r="30" fill="#2266ee"/>
    <path data-vectra-node="…" d="M 240 30 A 30 30 0 0 1 180 30" fill="#2266ee"/>
  </g>
</svg>
```

Both files are on disk as `TASK-8.0-SAMPLE.svg` / `TASK-8.0-SAMPLE.tsx`,
regenerable with `cargo run -p vectra-export --example samples`. The SVG parses
as well-formed XML (`xml.dom.minidom`; element kinds: `svg`, `g`, `rect`,
`circle`, `path`).

### RULE 3 — the Export IR sits between the document and the exporters

> "Never write string templates directly from the `Document`; first compile into
> `ExportIR`; exporters consume only the IR."

`crates/vectra-export` is the whole of it:

```
Document  (+ the live EvaluatedScene, when there is one)
    │
    ▼   compile_to_ir / compile_to_ir_resolved
┌─────────┐
│ ExportIR│  semantic geometry + parametric slots + props + warnings
└────┬────┘
     ├──────────────► export_svg(ir)      → String   (semantic SVG)
     └──────────────► export_react(ir)    → String   (parametric TSX)
                      (Vue/Svelte = new consumers of the same IR)
```

Neither exporter can see a `Document`, an `Engine`, or a scene: they take
`&ExportIR` and nothing else. `vectra-export` depends on `vectra-core`,
`vectra-geometry`, `vectra-expression`, `lyon` and `serde` — **`vectra-core`
gained no dependency** in this task.

---

## 2. The final `ExportIR`

`crates/vectra-export/src/ir.rs` (938 lines, doc-commented; serializable).

```rust
pub struct ExportIR {
    pub nodes: Vec<ExportNode>,      // draw order, back → front
    pub props: Vec<ExportProp>,      // sorted by name ⇒ byte-stable output
    pub bounds: Option<ExportBounds>,// document space, y up
    pub warnings: Vec<String>,       // every "this is a number, not code"
}

pub struct ExportNode {
    pub id: String,                  // engine id: `data-vectra-node`, JSX key
    pub name: String,
    pub geometry: ExportGeometry,
    pub style: ExportStyle,
}

#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ExportGeometry {
    Rect   { x, y, width, height, corner_radius: ExportParam },
    Circle { cx, cy, radius: ExportParam },
    Arc    { cx, cy, radius, start_angle, end_angle: ExportParam },
    Path   { d: Option<String> },    // lyon's serialization; None ⇔ not resolvable yet
    Group  { children: Vec<String> },
}

pub struct ExportParam {
    pub code: String,        // "40" | "base" | "base * 2" | "" (not expressible)
    pub value: Option<f64>,  // the resolved / drawn number
    pub vars: Vec<String>,   // free variables `code` reads, sorted
}

pub struct ExportColor {  // the colour door, same shape
    pub code: String,        // "#rrggbb" literal, or a prop name
    pub value: Option<Color>,
    pub vars: Vec<String>,
}

pub struct ExportStyle { fill: ExportColor, stroke: ExportColor,
                         stroke_width: ExportParam, opacity: ExportParam }

pub struct ExportProp { pub name: String, pub ty: ExportPropType, pub value: f64 }
pub enum ExportPropType { Number }    // one variant today, a data change tomorrow

pub struct ExportBounds { min_x, min_y, max_x, max_y: f64 }
```

### What compiles into it, and how

| Authored slot | `code` | `value` | `vars` | exporter behaviour |
|---|---|---|---|---|
| `Literal(200.0)` | `"200"` | `200.0` | `[]` | number in both targets |
| `Variable("base")` | `"base"` | `40.0` | `["base"]` | **prop** in TSX, number in SVG |
| `Expression($base * 2)` | `"base * 2"` | `80.0` | `["base"]` | **expression** in TSX |
| `Animated(Spring{…})` | `""` | drawn value | `[]` | number + a warning naming it |
| `Procedural(node, port)` | `""` | published value | `[]` | number + a warning naming it |
| `Interaction(binding)` | `""` | current value | `[]` | number + a warning naming it |

Notes on the two decisions that took the longest to get right:

* **`ExportColor::is_parametric()` keys off `vars`, not off `code.is_empty()`.**
  A colour *literal*'s `code` is still its hex string (SVG needs it); a variable's
  is the prop name. "Has code" is the wrong question — "reads something the
  caller must supply" is the right one. Without this, a literal exported as
  `fill={#ff0000}`, which is not valid JS.
* **Node membership is the document's decision, the numbers are the scene's.**
  `build_ir` walks `doc.geometry_ids()` rather than `scene.z_order`, so a node the
  evaluator skipped — the RULE-2 case, an undefined variable — is still written
  into the code, with its parametric text and a warning. A parametric export that
  silently dropped the node you were mid-way through editing would be worse than
  useless.

`translate_expression` maps the expression language to JavaScript: `$x` → `x`,
`sin`/`cos`/`abs` → `Math.sin`/`Math.cos`/`Math.abs`, and **`clamp` is left as
`clamp(…)`**, declared as a helper at the top of the module — but only when the
emitted code actually calls `clamp(` (a law tests both directions). An expression that cannot be
translated falls back to the resolved number plus a warning; it never emits
broken TypeScript.

### The SVG exporter (`svg.rs`, 321 lines)

* `SvgOptions { include_node_ids: true, pad: 0.02 }` — padding as a fraction of
  the larger dimension, so a stroke has somewhere to land.
* XML declaration, `<svg width height viewBox fill="none">`, a node/prop count
  comment, one `<g transform="translate(0 {max_y}) scale(1 -1)">` — a single
  mirror for the whole picture, so the arc flags stay simple — then one element
  per node.
* Alpha becomes `fill-opacity` / `stroke-opacity` (`128/255` → `0.501961`);
  `opacity` is only written when it is not `1`.
* Numbers are written with six decimals (`fmt_number`), which is what makes a
  round-trip comparison meaningful and the bytes deterministic.

### The React exporter (`react.rs`, 274 lines)

`ReactOptions { component_name: "Scene", include_comments: true }`; the
component name is a knob (tested). Output shape: header comment (node/prop/warning
counts), the `clamp` helper when needed, `interface <Name>Props { … }` **only
when there are props**, then `export function <Name>({ … }: <Name>Props)` whose
body is a `<Group>` of semantic elements. No props ⇒ a props-free signature.
Every number goes through `fmt_number`, every identifier through `sanitize_ident`,
and a warning about a non-parametric slot is written into
the file's header comment — the code documents its own compromises.

---

## 3. The wasm boundary and the UI

`vectra-wasm` (RULE 3 respected: the crate depends on `vectra-export` and passes
the **live scene**, never a string template):

```rust
pub fn export_to_svg(&self) -> String    // {"status","format":"svg","code","warnings"}
pub fn export_to_react(&self) -> String  // {"status","format":"react","code","warnings"}
```

Both call `compile_to_ir_resolved(self.core.document(), self.scene.scene())` and
return one envelope shape, so the panel has one parser and one modal.

**UI.** An `Export` panel (format dropdown: *SVG (semantic)* / *React
(parametric)*, an `Export` button, a summary line) opens a modal with the file,
its warnings above the code, and **Copy to Clipboard** in the footer. The React
side is still a dumb remote — it sends nothing, resolves nothing, and stores only
`{ format, envelope }`; the size and warning text come from `exportSummary()` in
the view-model, which is the only place the projection is tested.

`data-testid`s: `export-format`, `export-run`, `export-summary`, `export-modal`,
`export-warnings`, `export-code`, `export-copy`, `export-close`.

---

## 4. Tests

### `crates/vectra-export/tests/export_laws.rs` — 16 tests (664 lines)

| Law | What it proves |
|---|---|
| Semantic — `law_semantic_circle_stays_a_circle` | proptest, ±500 units: a circle is always `<circle>`, never a path |
| Semantic — `law_semantic_rect_stays_a_rect` | proptest incl. `rx`: a rect is a rect; the **authored** radius is carried (SVG clamps like the evaluator does, the user's number is not rewritten) |
| Semantic — `law_semantic_arc_uses_the_a_command` | proptest: 11-space-separated tokens, one `A`, correct `large-arc-flag`, no `C`/`S`/`Q` anywhere |
| Semantic — `a_full_arc_is_two_half_arcs` | a full turn is two `A`s |
| Prop Mapping — `law_prop_mapping_is_universal` | proptest: every parametric slot in the TSX is a prop in the interface, and vice versa |
| Prop Mapping — `law_a_variable_becomes_a_required_prop` | `$base * 2` → `width={base * 2}` + `base: number; // 40` |
| Prop Mapping — `law_a_direct_variable_is_a_prop_too` | proptest: a bare `$x` is a prop, not a number |
| Prop Mapping — `expression_source_translates_without_evaluating` | `translate_expression` table (`$x`→`x`, `sin`→`Math.sin`, `clamp` preserved) |
| Prop Mapping — `the_clamp_helper_appears_only_when_used` | no dead helper in the output |
| Roundtrip — `law_svg_round_trips_through_a_mock_importer` | proptest: SVG out → parsed by a mock importer → structural equivalence (tags, attributes, count, order) |
| Determinism — `the_same_document_exports_the_same_bytes` | one IR, exported twice, byte-identical (the ids are in the bytes) |
| Colours — `colours_export_faithfully` | hex + `fill-opacity="0.501961"` for `128/255` |
| Knob — `the_component_name_is_a_knob` | `component_name` reaches the signature |
| Empty — `an_empty_document_exports_an_empty_picture` | an empty document is a valid document |

### `crates/vectra-wasm/tests/export_laws.rs` — 7 tests (294 lines)

Through the real JSON seam (`VectraEngine`): one envelope for both formats; a
circle is never approximated; a variable is a required prop **and** the SVG of the
same document carries `width="80"`; the export sees the live scene (a procedural
`noise.tint` reaches the fill, with the warning that says it was read);
a spring-driven slot exports the number the canvas draws, with a warning;
determinism (same engine twice; two engines running one script produce the same
picture once ids are stripped); an empty engine still exports.

### Smoke (3 new steps, 36 → 39) and UI (1 new test, 33 → 34)

* `smoke[37/39]` — semantic SVG: `<circle`, one `A`, no `[CcSsQq]` command in any
  `d`, identical bytes on a second export.
* `smoke[38/39]` — parametric React: `base: number; // 40`, `width={base * 2}`,
  and the SVG of the same document carrying `width="80"`.
* `smoke[39/39]` — the live scene: the drawn spring height appears in the file
  and the envelope names it as a sample.
* `the export summary says the format, the size and the warning count` — the
  panel's only projection.

### Two deliberate non-laws

* **`rx` is not clamped on export.** The evaluator clamps a corner radius to
  `min(w, h)/2`; SVG does the same at draw time. The export carries the authored
  number, because the user's intent is the thing worth keeping, and asserting the
  evaluator's clamp in the exporter would freeze an implementation detail.
* **Determinism is per-engine, not per-process.** `compile_to_ir` mints node ids
  per document and those ids are in the bytes, so two *different* documents
  differ by design; the cross-engine test compares the picture with the
  `data-vectra-node` attributes stripped.

---

## 5. Files touched

| Path | Change |
|---|---|
| `crates/vectra-export/src/ir.rs` | 938 lines: `ExportIR` + `compile_to_ir{,_resolved}` + expression translation |
| `crates/vectra-export/src/svg.rs` | 321 lines: semantic SVG, arcs as `A`, one mirror group |
| `crates/vectra-export/src/react.rs` | 274 lines: TSX component, props interface, clamp helper |
| `crates/vectra-export/src/lib.rs` | crate docs + `export_svg` / `export_react` re-exports (doctest passes) |
| `crates/vectra-export/tests/export_laws.rs` | new, 664 lines, 16 tests |
| `crates/vectra-export/examples/{preview,samples}.rs` | the two generators behind this report's examples |
| `crates/vectra-wasm/src/lib.rs` | `export_to_svg` / `export_to_react` + `export_envelope` |
| `crates/vectra-wasm/Cargo.toml` | `vectra-export` dependency |
| `crates/vectra-wasm/tests/export_laws.rs` | new, 294 lines, 7 tests |
| `apps/vectra-web/src/engine/wire.ts` | `ExportEnvelopeWire` |
| `apps/vectra-web/src/engine/client.ts` | `exportToSvg()` / `exportToReact()` |
| `apps/vectra-web/src/engine/view-model.ts` | `exportSummary()` |
| `apps/vectra-web/src/App.tsx` | Export panel + modal + Copy to Clipboard |
| `apps/vectra-web/src/App.css` | export panel/modal styles, `select` |
| `apps/vectra-web/scripts/smoke.mjs` | steps 37–39 |
| `apps/vectra-web/tests/view-model.test.ts` | export-summary test |

---

## 6. Report-back

* **Final `ExportIR`:** §2 — `ExportIR{ nodes, props, bounds, warnings }`, with
  `ExportGeometry::{Rect,Circle,Arc,Path,Group}` semantic variants and
  `ExportParam{ code, value, vars }` on every scalar slot (colour included, via
  `ExportColor`).
* **React codegen logic:** §1 RULE 2 / §2 — the IR supplies the arithmetic and the
  variable set, `react.rs` only spells it as TypeScript: header comment, `clamp`
  helper when used, `interface` when there are props, one functional component
  whose body is a `<Group>` of semantic elements.
* **Semantic SVG examples:** §1 RULE 2 — `<circle>`, one `A`-command arc, rounded
  `<rect>`; also on disk as `TASK-8.0-SAMPLE.svg`.
* **Test results / gate status:** §4 and the table at the top.
* **Rule compliance:** RULE 1 — no exporter can emit a bezier for a circle, an
  arc or a rect, and the proptests say so; RULE 2 — a parametric slot is a prop
  with its expression intact; RULE 3 — `Document`/`EvaluatedScene` → `ExportIR` →
  `export_svg`/`export_react`, and `vectra-core` gained no dependency.
