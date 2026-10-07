# TASK 10.6 — Smart Components & The "Make Magic" AI

Four rules, one idea: **the AI never touches pixels, and the designer never sees
JSON.** Artwork becomes a *component* with props; a prompt becomes a *typed plan*
the engine validates and can refuse; an icon becomes a *law* that scales its own
stroke; and every answer that reaches a human is a sentence.

The work lands in three layers, each with its own laws:

| layer | what it owns | where |
|---|---|---|
| `vectra-core` | the component model, the prop binding, the icon-set macro, the selection/artboard context | `crates/vectra-core/src/component.rs`, `summary.rs`, `command.rs` |
| `vectra-ai` | the structural macros, the sentence grammar, the self-correcting executor | `crates/vectra-ai/src/planner.rs`, `exec.rs`, `schema.rs` |
| `vectra-web` | ⌘K bar, prop inspector, icon studio — and no JSON anywhere | `apps/vectra-web/src/{App.tsx,engine/*.ts}` |

The one environment caveat, stated up front because it is the interesting part:
this sandbox cannot obtain the `wasm-bindgen` **binary**, so **the engine and its
glue were rebuilt by building the CLI's own library** — `wasm-bindgen-cli-support`
— and driving it with the CLI's own `--target web` settings. §6.1 has the
evidence, the driver that now lives in the repo (`scripts/wbgen/`), and the three
vendoring bugs that road produced. The panels are therefore live against the
built binary: the smoke run drives all four rules through it (§5), and §6.1a
records the real bug that only became visible once the new binary was running.

---

## 1. RULE 1 — Smart Components (master / instance)

### 1.1 The data model

Built on what already existed: `ProceduralNode` (Task 7.0) supplies the
registry, the operand ports and the re-evaluation; `Parameter::Variable` (Task
1.2) supplies a named scalar a designer can see in the variable list.

```rust
ProceduralKind::ComponentMaster { members: Vec<usize>, spec: ComponentSpec }
ProceduralKind::Component      { master: NodeId,   group: NodeId, spec: ComponentSpec }
```

* A **master** owns the *prototype*: the member slots, their authored values, and
  the props it exposes. It draws nothing itself — it is the thing instances are
  cut from.
* An **instance** owns a group of ordinary nodes (clones of the members), one
  `Parameter::Variable` per prop, and one compiled expression per *scaled* prop.
  Its artwork is document content like any other; the component is *how the
  numbers in it are bound*, not a second scene.

### 1.2 Props map to `Parameter::Variable`, and scaled props map through one law

| prop | type | law | what it drives |
|---|---|---|---|
| `size` | scalar | direct | every member's geometry, as `value / design size` |
| `stroke_width` | scalar | scaled | every member's `style.stroke_width` |
| `corner_radius` | scalar | scaled | every rectangle member's `corner_radius` |
| `color` | colour | direct | every member's `style.fill` |

Scalars become `Parameter::Variable("<prefix>_<key>")` (`c<hex>` for a master,
`i<hex>` for an instance); colours become `Parameter::Procedural(<owner>.<key>)`
(the Task 7.0 door for a colour a component can be re-coloured through); scaled
slots become `Parameter::Expression("<factor> * $<prefix>_<scale>")`.

**One variable, many fractions.** A scaled prop writes *one* variable and gives
each member's slot its own factor, so `stroke_width: 2` on a 24px master is
`0.0833 × size` on every stroked member — one control, N bindings.
`bind_plan` picks the law's own scale key when a prop owns scaled targets, and
the prop's own key otherwise.

Props no member can carry are **omitted rather than offered as dead controls**: a
circle-only selection has no `corner_radius` slider.

### 1.2a How RULE 1's "the original group becomes an Instance" is read here

RULE 1 says: *"the original group becomes an Instance referencing the Master via
the procedural graph."* This codebase reads that sentence as **the original group
becomes the master's bound artwork** — the prototype the master is cut from — and
"becoming an instance" is what *placing* an instance does. That is stated plainly
because it is a reading, not a translation: what the engine does on
**Create Component** is

1. register a `ComponentMaster` `ProceduralNode` that **names the selection** as
   its members, in draw order (`members: Vec<NodeId>` — membership is a *listing*,
   not a wire: a wire carries a value, and membership is not a value);
2. **rebind** every slot a prop covers, in place — the selected rectangle's
   `width` stops being the literal `24` and becomes
   `Parameter::Expression("1 * $c<slug>_size")`, its stroke becomes
   `"0.0833 * $c<slug>_size"`;
3. leave the artwork **exactly where it was**, value-preserving.

From that moment the original group references the master through the dependency
graph, which is the property the rule is really about: the edges exist, so a prop
write is one variable edit and a settle, and there is no second copy of the
design values to drift.

**Why not literally replace the group with an instance?** Because a master has to
keep its design values somewhere, and in this engine they live in the artwork
itself. The alternative — a master that stores numbers of its own and a clone set
that replaces the selection — would duplicate every member's geometry into the
spec, and would make "Create Component" silently *move* a designer's artwork from
one node identity to another (breaking undo granularity, layer membership, and
every selection the designer had). The behaviour that matters is preserved
verbatim, and the panel is honest about the roles: the selection reads
**master**, a placed copy reads **instance**.

`the_original_group_becomes_the_masters_bound_artwork` pins all three steps — the
membership list, the rebound slots (asserting the expression's source names the
master's own `size` variable, for both the direct `width` and the scaled
`stroke_width`), `owner_of` answering with the master, and the snapshot before
and after being identical — so the reading is enforced rather than narrated.

### 1.3 The Prop Law (`crates/vectra-wasm/tests/component_laws.rs`)

`law_the_component_prop_law` (proptest, 1..=4 members × 4 prop writes): setting a
prop on **one instance** changes **only that instance's** variables, its nodes
re-evaluate in the same dispatch, the master and the sibling instances are
untouched, and the other instance's value is *not* what was just written. Undo
restores the instance exactly.

The wasm boundary is the UI's contract, so the same file drives it natively:
`set_selection`, `component_view`, `create_component`, `instantiate_component`,
`set_component_prop`, `icon_set`, `structural_macros` — eleven laws, including
`the_original_group_becomes_the_masters_bound_artwork` (§1.2a),
`law_a_fresh_instance_is_the_master_at_its_design_size` (§6.1a),
`every_component_verb_answers_with_prose` and
`the_panel_view_tracks_the_selection_through_its_roles` (a loose shape → "shape",
a member → "part of a component", a master → "master", an instance →
"instance", each with the verb that applies).

---

## 2. RULE 2 — Make Magic: structural, typed, self-correcting

### 2.1 A prompt becomes `Vec<Command>` — never pixels, never SVG

`DocumentSummary` (Task 9.0) gained what a *selection* prompt needs:

* `selection` — the ids, in **draw order** (a prompt that says "the first one"
  means what the designer sees back-to-front), with stale ids dropped rather than
  dangling;
* `selection_prose()` — the engine's own sentence for "this" ("card, Dot"), which
  the ⌘K bar shows so the UI and the planner cannot disagree about the subject;
* `artboard` — the active board's name and bounds, i.e. the *scale context* of
  every icon prompt (`active_id`, so a board created by a command counts too).

The planner answers a macro prompt with plan JSON that resolves to exactly the
`Command` enum, and `the_schema_is_the_command_enum_and_nothing_else` still
holds. Five macros ship in `HeuristicPlanner::MACRO_PHRASINGS`, and the UI's chips
come from the engine (`structural_macros` → `phrasings()`), so the advertisement
cannot drift from the matcher:

| prompt | plan |
|---|---|
| `align perfectly` | `AddConstraint{vertical}` on every pair, plus `parallel` spacing from the third shape on |
| `make this geometric` | whole-number snap of every geometry slot, `corner_radius → 0`, **one** union of the first two |
| `create 4 color variations` | `hue_palette` (2..=8, default 4), one `DuplicateNode` + position + `style.fill` per copy |
| `make this a component` | `CreateComponent` over the selection |
| `generate an icon set at 16 32 48` | `CreateComponent`, then 4 commands per size (see RULE 3) |

### 2.2 What the engine refused, and why the engine was right

The first version of the geometric macro folded a 3-shape selection into
`ApplyOperation{inputs: [left, union, right]}` — a union of a union. The engine
refused it, correctly: `OperationKind::Boolean` is binary and `ApplyOperation`
reads its inputs from the *evaluated primitives scene*, so an operation cannot be
an operand of another operation ("Phase 2" in `vectra-operations`). The macro now
unifies **the first two** shapes and says so honestly ("unified \"A\" with \"B\"
(a union takes two shapes; the rest are snapped and ready to fold in next)").
A nested union is a documented engine limitation, not something to paper over in
the evaluator.

Two further defects were the planner's, and are fixed:

* **One placeholder, one id.** `$new:` slugs mint an id on first use and reuse it;
  the align macro gave every column constraint the same slug, so the second
  constraint was "already registered". Constraints now carry their index
  (`$new:constraint:column:{index}`, `…:spacing:{index}`).
* **A refusal must be about the prompt, not the phrase.** `align perfectly` with
  one shape selected now returns a typed `unrecognized-prompt` whose message is
  *"aligning needs at least two shapes — select another one and try again"*,
  instead of falling through to "I cannot turn that into commands yet".
  `a_macro_that_cannot_run_says_why_in_a_sentence` pins both that and the empty
  selection.

### 2.3 Self-correction (Task 9.0 retry), made transitive

`apply_fixes` runs to a **fixpoint**: a fix that drops a command also drops
everything that named the ids that command would have minted (`created_id`, now
`pub(crate)`), so one ban cannot cascade into a second refusal on the next
attempt. `law_a_macro_self_corrects_around_a_stale_selection` proves the loop:
prompt → plan → engine refuses (a member was deleted) → feedback → repaired plan
→ applied, with the sentence on the report.

### 2.4 The sentence (RULE 4's other half)

`ExecutionReport::prose()` composes from per-command phrases —
"applied 1 constraint", "unified the shape(s)", "turned the selection into a
component", "placed 3 instance(s)", "made 3 copies", "added 3 artboards",
"adjusted 4 values" — into `"✨ Applied 3 constraints and unified the shape."`
`every_macro_answers_in_a_sentence_a_designer_can_read` runs all five macros
against **one** live document and asserts every answer starts with `✨`, names no
JSON, and that each macro's plan still applies.

---

## 3. RULE 3 — Icon Studio: the scaling law

`generate an icon set at 16 32 48` builds the master from the selection, then for
each size: `CreateArtboard` (a square board of exactly that size, laid out left
to right with a 16-unit gutter), `CreateLayer` (which makes it the active layer,
so the instance lands on *its* board), `InstantiateComponent`, and
`SetComponentProp{prop: "size"}`.

The last command is the whole trick: `stroke_width` and `corner_radius` are
*scaled* props, so writing one variable re-derives both through their compiled
expressions. A 2px stroke on a 24px glyph is `1.333` at 16px — never a hairline.

* `law_the_icon_scaling_law` (proptest): for any master and any 1..=4 sizes, each
  board is `size × size`, each instance is a real instance of *that* master, and
  every instance's stroke/radius is the scaled expression, not a copied number.
* `a_default_icon_set_is_the_16_32_48_ladder` — the classic ladder when nothing is
  said; `8.0..=512.0` is the range the UI accepts (planner 4..=512).
* `an_icon_set_refuses_a_selection_that_is_not_a_master` — a typed refusal, not a
  silently empty sheet.
* `the_icon_macro_builds_the_ladder_with_a_scaled_size_prop` — the AI path emits
  `1 + 3×4` commands, in order, and applying them yields a master with three
  instances whose stroke/radius props are `is_scaled()` and whose variables and
  expressions are per-instance (no shared ids).

---

## 4. RULE 4 — Designer-first UI

* **The plan is described, not dumped.** `aiPlanRows` maps each command to a
  label via `AI_LABELS` ("Add a constraint", "Unify the shapes", "Place an
  instance", …), falling back to a de-capitalised tag — and the row's `title` is
  the label, so hovering shows prose too. The old `<code>{row.json}</code>` is
  gone from the panel.
* **One sentence per run.** `AiReportWire.prose` travels from the engine to the
  panel (`ai-prose`); the panel shows the sentence, the headline stays as the
  receipt's title.
* **The component panel is a property inspector.** Sliders and a colour input
  keyed by prop (`prop-{key}`, `prop-input-{key}`, `prop-value-{key}`), a master
  line for instances, and the procedural graph nowhere in sight.
* **⌘K / Ctrl-K** opens the Make Magic bar anywhere; Escape closes it; chips come
  from the engine; the bar shows the engine's sentence for the current selection.
* `no panel text ever contains a brace — RULE 4 is checkable` asserts the *panel
  strings* (summary, headline, every row label) contain no `{`, `[`, `]` or `"`.

---

## 5. Tests

### Rust — **576 passed, 0 failed** (Task 10.5 closed at 549)

`cargo test --offline --workspace --all-targets` (doctests excluded — §6.2).

Task 10.6's own suites:

| suite | tests | what it pins |
|---|---|---|
| `vectra-wasm component_laws` | **11** | RULE 1 (Prop Law, Instance Seed Law, the original group's binding), RULE 3 (Icon Scaling Law), RULE 4 (prose), undo |
| `vectra-ai ai_laws` | **34** | RULE 2: structural law over 5 macros on one live document, the icon ladder, sentences, self-correction |
| `vectra-core summary` | **12** | selection capture (draw order, dropped ids), empty selection, artboard context |
| `vectra-core lib` | 51 | the component model + commands (`CreateComponent`, `InstantiateComponent`, `SetComponentProp`, `CreateArtboard`, `CreateLayer`) |

### UI — **99 passed, 0 failed** (`npm run test:ui`)

Three tests in `view-model.test.ts`: every plan row reads as a phrase (and the tag
survives for anyone who wants it); no panel string contains JSON; the Task 10.6
surface is *probed*, not assumed (`engineIsOlderThanUi({})` → true, a partial
surface → true, the full one → false, and `VectraClient.STALE_ENGINE` names the
build command in a sentence with no braces).

Two more in **`tests/wasm-client.test.ts`** (new) close the one seam nothing
covered: `VectraClient`'s typed wrappers over the glue — the only thing the ⌘K
bar, the component panel and the icon studio ever call. They load the wasm the
browser loads and walk a designer session through the client's own methods:
selection prose, the master's four props, the fresh instance being a copy, one
`setComponentProp` scaling that instance alone, one undo, the icon ladder with the
stroke law at every rung, the five chips, a refusal in a sentence. The second test
hands the same wrappers an engine object with *none* of the Task 10.6 methods and
pins the degrade path: four typed `status: 'error'` replies carrying
`VectraClient.STALE_ENGINE`, and reads answering `null`/`[]` instead of throwing.
Type-level, the reply shape those verbs share is now `ComponentReplyWire` in
`wire.ts` (envelope + `prose`/`label`/`created`) rather than an intersection
retyped at each call site.

Seven more in **`tests/task-10-6-panels.test.tsx`** (new, with the two panels
they cover extracted from `App.tsx` into `ComponentPanel.tsx` and `MagicBar.tsx`
— the house shape of every other panel, and the reason they can be rendered in a
test at all). Rendered with `react-dom/server`, they pin the parts of RULEs 1–4 a
type-check cannot see: the engine's headline printed verbatim; exactly three
range inputs with the engine's own bounds and a colour *swatch* for the colour;
the scaling law **visible** as "∝ size" on both derived props and the wire key
`stroke_width` never reaching the screen text; the empty state explaining itself;
the stale-engine sentence on screen and still brace-free; the Icon Studio's
editable ladder; the ⌘K bar's chips (labels and hints straight from the engine),
its receipt printed verbatim, a closed bar rendering *nothing* at all, and the
run button disabled exactly while the prompt is empty.

### Smoke — **68 steps, `SMOKE PASS`** (`npm run smoke`)

Every earlier feature, end to end, then Task 10.6's four steps drive **the binary
in the repo** — the artifact `client.ts` loads, not a native stand-in:

* `[65/68] RULE 1` — two shapes become a master (`Glyph`) with exactly
  `size, stroke_width, corner_radius, color`; a placed instance is a *copy* at
  24 units; one write of `size = 40` moves the instance to 40, its corner to
  4 × 40/24 and its stroke to 2 × 40/24 while the master's member stays 24/4/2;
  one undo puts it back.
* `[66/68] RULE 2` — *"make this geometric"* squares three corners and answers
  `✨ …`; *"align perfectly"* leaves ≥ 2 constraints in the document; a
  one-shape selection refuses with a sentence containing "two shapes" and no
  brace or bracket anywhere in it.
* `[67/68] RULE 3` — `icon_set` on a 24px master produces a 16/32/48 ladder: an
  artboard per size, three scaled clones, `stroke ÷ size` equal to the master's
  2/24 at every rung, and the 16px stroke ≥ 1px.
* `[68/68] RULE 4` — the five macros the ⌘K bar offers come from the engine, the
  selection is described in prose ("Nothing selected"), and the prompt the model
  is grounded on names the selected id.

The step count is part of the assertion: the labels run `[1/68]`…`[68/68]`, so a
new step that forgets to renumber is a diff a reviewer can see.

### Gate status

Every gate below was re-run **twice**: once after the last edit, and again from a
**cold sandbox** (`/tmp` reclaimed — toolchain re-extracted, 256 crates
re-vendored, `node_modules` reinstalled), where the counts came out identical.
The last two rows are new: the engine was rebuilt from source this time (§6.1),
so the smoke run is now a statement about Task 10.6 as well.

| gate | result |
|---|---|
| `tools/check.sh check --workspace --all-targets` | **exit 0** |
| `cargo test --offline --workspace --all-targets` | **576 passed, 0 failed** |
| `cargo fmt --all --check` | clean |
| `cargo clippy --workspace --all-targets` | **no warnings** |
| `npm run typecheck` (app + tests) | clean |
| `npm run test:ui` | **99 / 99** |
| `npm run smoke` | **`SMOKE PASS`** (68 steps, all four rules; three consecutive runs) |
| `npm run build` (production) | 47 modules, `vectra_wasm_bg-*.wasm` 24 178.80 kB, gzip 2 835.02 kB |
| `cargo build --target wasm32-unknown-unknown -p vectra-wasm` + the glue driver | 4 artifacts regenerated (§6.1) |
| `npm run dev` + wasm fetch | HTML 200, `vectra_wasm.js` 200 (7 Task 10.6 methods present), `vectra_wasm_bg.wasm` 200 (24 178 797 B), preview `Host` header accepted |

---

## 6. Deviations and environment, stated plainly

### 6.1 The wasm **was** rebuilt — by building the CLI's own library

`apps/vectra-web/scripts/build-wasm.sh` still needs the `wasm-bindgen` CLI at the
lockfile's version (0.2.129), and this sandbox still cannot obtain *the binary*:
the released artifacts live on `release-assets.githubusercontent.com` (blocked:
`302` then `OpenSSL SSL_connect: SSL_ERROR_SYSCALL`), no reachable package host
carries a CLI (PyPI: no `wasm-bindgen*`; npm: the name is a security-holder stub
and `wasm-pack`'s tarball is an 8 KB `install.js` that downloads that same blocked
binary), and no published `wasm-bindgen-cli` crate exists anywhere reachable.

What *is* reachable is the CLI's **source**, and the CLI is a thin argument
parser in front of `wasm-bindgen-cli-support` — an ordinary library crate with a
small closure. So this task built the CLI without the CLI:

| piece | where | what it is |
|---|---|---|
| the driver | `apps/vectra-web/scripts/wbgen/` (**new**, in-repo) | a ~70-line crate that calls `Bindgen::web(true)`, sets the flags the CLI's `rmain` sets, and generates |
| its closure | 42 packages, vendored into `/tmp/cli-vendor` from the same sources the workspace uses | `wasm-bindgen-cli-support 0.2.129`, `walrus 0.27.2`, `wasmparser`/`wasm-encoder 0.245.1`, `wasm-bindgen-shared 0.2.129`, `serde`, `rayon`, … |
| the module | `cargo build --offline --target wasm32-unknown-unknown -p vectra-wasm` (**debug**, the profile HEAD shipped) | `target/wasm32-unknown-unknown/debug/vectra_wasm.wasm` (40 327 873 B raw) |
| the artifacts | `apps/vectra-web/src/wasm/` | `vectra_wasm.js` (115 271 B), `vectra_wasm.d.ts` (38 996 B), `vectra_wasm_bg.wasm` (24 178 797 B), `vectra_wasm_bg.wasm.d.ts` (8 226 B) |

Two of the CLI's flags are *not* the library's defaults, and both matter here, so
the driver sets them explicitly: `typescript(true)` (the app's types come from
`vectra_wasm.d.ts`) and `omit_default_module_path(false)` (the glue's
`new URL('vectra_wasm_bg.wasm', import.meta.url)` fallback that `client.ts`'s
`?url` import and `scripts/smoke.mjs` both rely on). `reference_types` and
`multi_value` are deliberately left alone: the library reads the module's own
`target_features` and enables each transform only if the module already uses it.
The driver is *not* a general CLI replacement (no node target, no test runner),
it is not a workspace member (empty `[workspace]` table — it resolves against the
CLI's closure, not `Vectra/Cargo.lock`), and the seven methods are in the built
glue: `set_selection`, `component_view`, `create_component`,
`instantiate_component`, `set_component_prop`, `icon_set`, `structural_macros`.

Reproducing it needs a vendor tree for the CLI's closure; that is sandbox
machinery, not repo content, but the shape of it is:

```bash
tools/vendor_deps.py /tmp/cli-vendor --lock /tmp/cli-lock.toml     # 42 crates
printf '[source.crates-io]\nreplace-with = "cli"\n\n[source.cli]\ndirectory = "/tmp/cli-vendor"\n' \
    > /tmp/cli-cargohome/config.toml
CARGO_HOME=/tmp/cli-cargohome cargo run --release \
    --manifest-path apps/vectra-web/scripts/wbgen/Cargo.toml -- \
    target/wasm32-unknown-unknown/debug/vectra_wasm.wasm \
    apps/vectra-web/src/wasm vectra_wasm
```

Vendoring *that* closure found three more bugs in `tools/vendor_deps.py` (§6.3),
all of the same kind — a source that looks right and is not:

* `LOCK_OVERRIDE` was honoured by `checksums_from_lock()` but not by
  `read_lock()`, so a foreign-lock run vendored the **workspace's** closure while
  claiming to vendor the CLI's (42 crates reported, 256 directories written);
* `wasm-tools` tags its repository `v1.245.1` while publishing crates as
  `0.245.1`, and the crate's manifest inherits its version from the workspace
  root (`version.workspace = true`) — so the tag never matched and the script
  fell back to **`main`**, whose `Name` enum has two variants `walrus` 0.27.2 has
  never heard of (`error[E0004]`, 17 errors). The fix is the tag form *and*
  reading the inherited version; the first build of the CLI library failed on
  exactly this, which is how it was found.

The earlier plan — hand-writing the glue and shimming the committed binary — was
abandoned on evidence, and the evidence is worth keeping: the compiler's raw ABI
and the CLI's differ *everywhere*, not just in names (raw `JsValue` parameters
are `i32` handles where the CLI emits `externref`; string-returning exports
compile as sret calls where the glue calls the multi-value form), so a
hand-patched 23 MB binary would have had to reproduce the CLI's whole transform —
in exactly the paths (canvas surface creation, promise-returning `attach`,
closures) that nothing in this sandbox can test. Building the library that *is*
that transform is the shorter road, and it leaves a reproducible one behind.

What it does *not* claim is byte-identity with the CLI's output — there is no CLI
to diff against here. It claims the same library, the same module and the same
settings, and the evidence that it is enough is the suite below: the generated
glue and the module it loads pass all 68 smoke steps and the 90 UI tests, and the
`.d.ts` it emits typechecks against `client.ts`.

The UI's degrade path stays: a browser loaded against an older engine still gets
the sentence naming `build-wasm.sh` (`data-testid="engine-outdated"`) instead of
a stack trace — the probe is not dead code, it is the standing contract for
anyone whose engine predates the interface.

### 6.1a The bug the rebuilt engine exposed

The old binary could not run these paths at all, so the first smoke run against
the new one is also the first time a *fresh instance* was ever evaluated — and it
came out **1 × 1**. `InstantiateComponent` planned the instance's bindings with
`bind_plan(…, seed: None)`, which seeds the scale variable from
`design_size(doc, &clones)`; a clone's geometry is by then expression-bound
(`$size × factor`), and `design_size` reads **literals**, so it read 0 and took
its documented `1.0` floor. The consequences were user-visible: placing an
instance gave a 1-unit speck, and its Size slider spanned `0..4`.

The fix seeds the instance from the **master's** own scale variable — the number
the designer sees on the master — and the rule for "which prop is the scale"
(`scale_key`) now lives in one place, `vectra_core::component::scale_key`, used by
both the binder and the instantiate path. `law_a_fresh_instance_is_the_master_at_
its_design_size` (proptest, 24 cases over design size / stroke / radius) pins it:
the clone carries the master's numbers before any prop is written, and the panel's
Size slider reads them and spans at least `2 ×` design. It was verified to fail
against the unfixed code (the proptest failure, with
`component_laws.proptest-regressions` deleted again afterwards).

### 6.2 Doctests

`cargo test --workspace` ends in `error: doctest failed … could not execute
process rustdoc`: the toolchain extracted from the PyPI bundle ships `rustc`,
`cargo`, `rustfmt` and `clippy` but no `rustdoc`. `--all-targets` runs
everything else (576 tests). On a machine with rustdoc, plain
`cargo test --workspace` runs them.

### 6.3 `Cargo.lock`

The delivered lock is `HEAD` **plus one line**: `proptest` as a dev-dependency of
`vectra-wasm` (the new proptest laws need it). Two notes for a reader who runs
cargo inside a sandbox like this one:

* any cargo run *here* re-prunes the lock's `glam 0.15…0.19` entries (120 lines),
  because the vendored `nalgebra` manifest is version-patched to the single
  vendored `glam 0.14.0`, which makes those packages unreachable. This is an
  artifact of the offline vendor tree, not of the change: on a checkout that can
  reach crates.io the lock is self-consistent and cargo leaves it alone;
* `tools/` (untracked) holds the sandbox's toolchain/vendor machinery
  (`extract_toolchain.py`, `vendor_deps.py`, `check.sh`). This task fixed **nine**
  things in `vendor_deps.py`, each one a way the vendor tree silently produced an
  unbuildable crate — two from the workspace's cold re-runs, three from the CLI
  closure's (§6.1), and one more from a third cold run:

  1. a published registry tarball beats a repository tree (a GitHub tag can be
     the wrong API or lack a feature the lock needs — `glow` 0.13.1, `glam`
     0.14.0's missing `f64`). A *checked-in cargo registry copy* is that same
     evidence, so `VECTOR_PREFER_REGISTRY=1` promotes it over any tree — which is
     how `glow` is sourced here: the AOSP mirror's `crates/glow` is the right
     version and the wrong contents (it has no `build.rs`, so its `src/gl46.rs`
     lacks `GLchar` and the host build dies with 30 errors), while
     `Eldarismailovee/EmulatorGB`'s `.cargo/registry/cache` holds the crate as
     published;
  2. a feature request whose *every* feature was dropped is removed with the
     dependency entry, not left as a no-op (`convert-glam033` naming a dependency
     that no longer exists);
  3. the dependency-feature prune runs **before** the feature-table prune, in both
     the install and the repair path;
  4. `prune_dependency_features` iterates a copy, because it may delete the entry
     it is looking at;
  5. the post-pool pass that re-reconciles the finished tree runs on **every**
     invocation, not only `--repair-only` — the prune consults the tree, so it
     cannot run while the tree is half-built;
  6. `--lock PATH` is honoured by `read_lock()` as well as by
     `checksums_from_lock()` (without this, a foreign-lock run vendors the
     *workspace's* closure under the other lock's name — 42 crates reported, 256
     directories written);
  7. a manifest that inherits its version (`version.workspace = true`, which TOML
     reads as `version = {workspace = true}`) is read through to the workspace
     root, so a correct release is not mistaken for a version mismatch — the
     mistake that sent `wasmparser` to `wasm-tools`' `main` branch;
  8. `wasm-tools`' tag form is `v1.<minor>.<patch>` for crate `0.<minor>.<patch>`
     (`v1.245.1` ↔ `wasmparser 0.245.1`), and the crate-name set is compared
     through the same normalisation as every other name in the file
     (`wasm-encoder` → `wasm_encoder`, which a hyphenated literal set never
     matched).

  9. `REGISTRY_FIRST` — a set, not a flag, **and** a hard stop rather than a
     preference. `glow` is the case: the AOSP mirror's `crates/glow` has no
     `build.rs`, so its `gl46.rs` is a stub and the host build dies with 30
     `GLchar` errors. As an env-var-gated behaviour (`VECTOR_PREFER_REGISTRY=1`)
     the same lockfile produced a working vendor tree once and a broken one twice,
     depending on how the tool happened to be called; the third cold run caught
     that, and the set made it a property of the lockfile. The fourth cold run
     then showed the second half of the problem: both registry lookups go through
     GitHub's *code search*, which is rate-limited — in a 256-crate pass they can
     both come back empty for reasons unrelated to the crate, and the mirror walk
     silently wins again. So a registry-first crate now takes a published copy
     *or the run fails, naming the crate*: the fallback is refused instead of
     taken. On the run after the fix, `glow 0.13.1` came from
     `Eldarismailovee/EmulatorGB`'s `.cargo/registry/cache` copy — checksum-
     verified against the lock — with `gl46.rs` complete.

  Two of the first five were found by a cold-sandbox re-run, which is the point of
  doing it: `glow` had been re-selected from the AOSP mirror, and
  `logos`/`logos-derive` had been taken from two different commits of the `logos`
  repository, so the derive macro and its runtime disagreed about
  `CallbackResult`. Three more (6–8) were found by building the CLI closure, where
  the failure mode is loud (`error[E0004]` × 17) but the cause is three rules
  away. And the ninth is the one that repeats: the same `glow` selection came
  back on the third cold run because the fix had been an environment variable —
  the report's own gate table would have been green twice and red once, for a
  reason invisible in the diff. The fourth run found why even a set was not
  enough: the lookup that honours it is the one that gets rate-limited.

### 6.4 `scripts/test-ui.mjs` (a gate that was silently broken)

`node --test dist/` no longer expands a directory on Node 22.22 — the runner
treated `dist` as a file and reported `MODULE_NOT_FOUND` (which is how a suite of
87 tests becomes "1 failed"). The bundles are now named explicitly, so the same
command finds every test on every Node.

---

## 7. Files touched

| file | change |
|---|---|
| `crates/vectra-core/src/component.rs` | **new** — master/instance kinds, prop inference and binding, the icon-set plan, `inspect` (the panel view) |
| `crates/vectra-core/src/{command,lib,ids}.rs` | `CreateComponent`, `InstantiateComponent` (and its scale seed — §6.1a), `SetComponentProp`, `CreateArtboard`, `CreateLayer`, id constructors |
| `crates/vectra-core/src/summary.rs` | selection (draw order, live ids), `selection_prose`, `artboard`, `to_text`'s `SELECTION` section |
| `crates/vectra-core/src/procedural.rs` | operand access a component binding needs |
| `crates/vectra-procedural/src/nodes.rs` | the master/instance kinds' evaluation and diagnostics |
| `crates/vectra-dependency/src/prospective.rs` | dry-run edges for the three component commands |
| `crates/vectra-ai/src/planner.rs` | five structural macros, `MACRO_PHRASINGS`, the honest geometric union, per-constraint placeholders, typed macro refusals |
| `crates/vectra-ai/src/{exec,schema,prompt,lib}.rs` | the transitive `apply_fixes` cascade, `created_id`, the macro arms' id minting, the macro phrasings in the system prompt |
| `crates/vectra-wasm/src/lib.rs`, `Cargo.toml` | the seven engine methods, and `proptest` for the new laws |
| `crates/vectra-wasm/tests/component_laws.rs` | **new** — the Prop Law, the Instance Seed Law, the original-group binding (§1.2a), the Icon Scaling Law, prose and undo |
| `crates/vectra-ai/tests/ai_laws.rs` | the structural law (all macros, one live document), the icon ladder, sentences |
| `crates/vectra-core/tests/summary.rs` | selection and artboard capture |
| `apps/vectra-web/src/App.tsx`, `App.css` | ⌘K bar, component panel, icon studio, prose rows |
| `apps/vectra-web/src/engine/{client,wire,view-model}.ts` | typed Task 10.6 surface + probe, the new wire shapes, label rows |
| `apps/vectra-web/tests/view-model.test.ts` | the three RULE-4/probe tests |
| `apps/vectra-web/tests/wasm-client.test.ts` | **new** — a designer session through `VectraClient` against the real binary, and the stale-engine degrade path |
| `apps/vectra-web/src/components/{ComponentPanel,MagicBar}.tsx` | **new** — the Task 10.6 panels, extracted from `App.tsx` so they can be mount-tested like every other panel |
| `apps/vectra-web/tests/task-10-6-panels.test.tsx` | **new** — those two panels rendered: props, laws, chips, empty and stale states |
| `apps/vectra-web/src/engine/wire.ts` | `ComponentReplyWire` — the shape the four component verbs share |
| `apps/vectra-web/scripts/test-ui.mjs` | explicit bundles (Node 22.22) |
| `apps/vectra-web/scripts/smoke.mjs` | steps 65–68: the four rules against the **built** engine, documented in the header like every earlier task's (and the `[n/68]` renumber) |
| `apps/vectra-web/scripts/build-wasm.sh` | points at `scripts/wbgen/` when the CLI cannot be installed (comment + error text only) |
| `apps/vectra-web/README.md` | the driver's invocation, beside the CLI's version rule |
| `apps/vectra-web/scripts/wbgen/` | **new** — the CLI's library, driven with the CLI's `--target web` settings (§6.1) |
| `apps/vectra-web/src/wasm/*` | **regenerated** — glue + `.d.ts` + the 24 178 797 B wasm (the seven Task 10.6 methods) |
| `Cargo.lock` | `proptest` for `vectra-wasm` |

---

## 8. What a designer can do now

1. Select a shape, press **⌘K**, type *"make this geometric"*, press Enter: the
   corner radius snaps to 0, the geometry lands on whole numbers, the first two
   shapes are unified — and the receipt reads **"✨ Snapped 2 shapes to whole
   numbers and unified them into one shape."** No JSON anywhere.
2. Drag the **Size** slider in the Component panel: only that instance moves,
   instantly, through the dependency graph. Drag **Stroke** and the artwork's
   stroke follows the *size law*, not the pixels.
3. Type *"generate an icon set at 16 32 48"* (or press the button): three
   artboards, three instances, one law — and the 16px glyph keeps its stroke.
   A freshly placed instance arrives as a *copy* of the master (§6.1a), so the
   slider starts where the designer left the master, not at 1 px.
4. `git checkout` nothing: every one of those plans is a `Vec<Command>` a
   click could have produced, so undo takes each back in one step.
