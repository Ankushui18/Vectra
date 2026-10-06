# TASK 9.0 — THE AI COMMAND API

**Status:** complete. Phase-1 MES execution, Engine-First. Task 9.0's six plan items
and three RULEs are delivered, tested and gated.

> The AI never draws. It emits commands. It is grounded on the engine's own
> summary of the document, every command it emits goes through the same
> `dispatch` path a user's click takes, and when the engine refuses one the
> refusal is handed back as a correction prompt.

| Gate | Result |
| --- | --- |
| `cargo test --workspace --no-fail-fast` | **434 passed / 0 failed** (12 crates) |
| `cargo test -p vectra-ai` | 26 + 1 doctest, 0 failed |
| `cargo test -p vectra-wasm --test ai_laws` | 9 passed |
| `cargo test -p vectra-core --test summary` | 8 passed |
| `cargo clippy --workspace --all-targets` | **0 warnings** |
| `cargo fmt --all -- --check` | clean |
| `npm run smoke` (real wasm module, Node) | **44/44 steps** |
| `npm run test:ui` | 40 passed |
| `npm run typecheck` / `npm run build` | clean / built (33 modules) |

---

## 1. What the task asked for, and where it lives

| # | Plan item | Delivered in |
| --- | --- | --- |
| 1 | `DocumentSummary` in `vectra-core` | `crates/vectra-core/src/summary.rs` (714 lines), `NodeKind::scalar_slots()` in `document.rs` |
| 2 | Command JSON schema + system-prompt template | `crates/vectra-ai/src/schema.rs`, `src/prompt.rs` |
| 3 | WASM boundary `ai_generate_commands` / `ai_execute_with_retry` | `crates/vectra-wasm/src/lib.rs` (+ `document_summary`, `ai_prompt`, `ai_phrasings`, `ai_execute_commands`) |
| 4 | Local executor proving the architecture **without an API key** | `crates/vectra-ai/src/planner.rs` (`HeuristicPlanner`, `ScriptedPlanner`, `ChatPlanner`) + `src/exec.rs` (the loop) |
| 5 | React "AI Assistant" panel (Cmd+K) | `apps/vectra-web/src/App.tsx`, `src/engine/{wire,client,view-model}.ts`, `src/App.css` |
| 6 | Proptests for the three laws | `crates/vectra-ai/tests/ai_laws.rs`, `crates/vectra-wasm/tests/ai_laws.rs`, `crates/vectra-core/tests/summary.rs`, `scripts/smoke.mjs` steps 40–44 |

**RULE 1 (Strict Command Emitter):** the only thing the AI layer can produce is
`Vec<vectra_core::Command>` — deserialized from JSON with `deny_unknown_fields`,
so an invented `type` or a misspelled field is refused before it reaches the
document. There is no code path in `vectra-ai` that writes SVG, a path, a pixel,
or a coordinate that the user did not ask for.

**RULE 2 (Context-Aware Prompting):** `DocumentSummary` is built by the engine,
from the engine's registries, with the engine's evaluators — and it is the *only*
thing the planner is given (`PlanRequest { prompt, summary, correction }`). There
is no `Document` handle in the trait, so a planner *cannot* reach past it.

**RULE 3 (Deterministic Validation & Self-Correction):** every command goes
through the host's `apply`, and the wasm host's `apply` is `dispatch_command` —
the same function the UI's buttons call. Refusals are captured, fed back as a
correction, and up to **two** corrections are attempted (`MAX_ATTEMPTS = 3`
attempts total) before failing with the typed
`AiError::MaxRetriesExceeded { attempts, last_code, plan, corrections, … }`.

---

## 2. RULE 2 — the final `DocumentSummary`

```rust
/// A lightweight, serializable, AI-facing view of a document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DocumentSummary {
    pub version: u32,
    /// Nodes in draw order (back → front), the order every other surface uses.
    pub nodes: Vec<SummaryNode>,
    /// Variables, sorted by name.
    pub variables: Vec<SummaryVariable>,
    /// Expression records, sorted by id.
    pub expressions: Vec<SummaryExpression>,
    /// Registered operations, in registry order.
    pub operations: Vec<SummaryOperation>,
    /// Constraints, in registry order.
    pub constraints: Vec<SummaryConstraint>,
    /// The procedural graph, in registry order.
    pub procedural: Vec<SummaryProcedural>,
    /// Motion tracks, sorted by name.
    pub tracks: Vec<SummaryTrack>,
}

pub struct SummaryNode {
    /// The **exact** id a command must carry. Never abbreviated here.
    pub id: String,
    pub name: String,
    /// The kind tag (`Rectangle`, `Circle`, `Arc`, `Path`, `Group`).
    pub kind: String,
    /// `Rectangle 'card'` — the phrase the task names, used as the line label.
    pub label: String,
    /// Every slot, geometry first, then style.
    pub slots: Vec<SummarySlot>,
}

pub struct SummarySlot {
    pub property: String,        // `width`, `style.fill` — what `SetParameter` takes
    pub source: String,          // the parametric source: `40`, `$base`, `$base * 2`
    pub value: Option<f64>,      // resolved now, when a context could evaluate it
}
```

Companions: `SummaryVariable { name, value }`, `SummaryExpression { id, source,
value? }`, `SummaryOperation { id, kind, inputs, enabled, name }`,
`SummaryConstraint { id, kind, targets, strength, value?, enabled }`,
`SummaryProcedural { id, kind, name, enabled, wires, outputs }`,
`SummaryTrack { id, name, channels }`.

Design points that carry weight:

* **The moat survives summarization.** A slot reads `width = $base * 2  (now 80)`
  — the *source* is what a planner is told to keep, and the value is context. A
  summary that printed only `80` would teach a model to flatten parametric
  design; a test (`a_summary_shows_the_expression_not_the_number_it_evaluates_to`)
  pins the opposite.
* **Slots come from the node's own list.** `NodeKind::scalar_slots()` is the
  single source (Rect: x, y, width, height, corner_radius; Circle: cx, cy,
  radius; Arc: cx, cy, radius, start_angle, end_angle; Path: start; Group: none),
  then `STYLE_SLOTS` (`style.fill`, `style.stroke`, `style.stroke_width`,
  `style.opacity`). The summary cannot advertise a slot the engine does not have.
* **Nothing is invented.** With no evaluation context a variable slot reads
  `value: None`; a slot whose expression was deleted reads
  `expr <id> (missing)`. The summary reports, it never repairs.
* **Lookup is generous but never a guess.** `find_node` accepts an exact id, a
  unique id prefix (≥ 4 chars) or a unique name (case-insensitive), and returns
  `NameLookup::Ambiguous(ids)` rather than picking one of two nodes called `dot`.
  `resolve_node_id` is the `Option` form the context check uses.
* **A budget, labelled.** `to_text()` prints at most `DEFAULT_NODE_BUDGET = 60`
  node lines and then says `… N more node(s) not listed; reference those by name`.

The real block (from the test fixture; a live engine adds `(now …)` behind any
source that differs from its resolved value):

```
DOCUMENT v1 — 2 node(s), 1 variable(s), 2 expression(s), 0 operation(s), 1 constraint(s), 0 procedural node(s), 0 track(s)
VARIABLES
  $base = 21
NODES (draw order, back → front)
  Rectangle 'card'  id=fc2dfed0-857c-457c-85fe-7c10bb000110
      x = 0
      y = 0
      width = $base * 2
      height = 30
      corner_radius = 0
      style.fill = #2266ee
      style.stroke = #00000000
      style.stroke_width = 0
      style.opacity = 1
  Circle 'Dot'  id=978cbf7f-0fcd-4134-b263-fe61b0f0ae8f
      cx = 10
      cy = 10
      radius = 5
      …
EXPRESSIONS
  id=018d0eb8-9cf0-4079-a880-1147b0337a53  $base * 4
CONSTRAINTS
  id=9ac4f129-ab2d-4dd9-bbe9-5490c65739f6  [medium] vertical [fc2dfed0-….x, 978cbf7f-….x]
```

`NodeOutputId`-driven slots read `proc <node>.<port>`; motion bindings read
`spring → $base (from 40 @ 0s, k=120, c=14)`, `state $hover ? 320 : 200` or
`track 'rise'.height` — enough that a model knows a slot is animated and must not
"fix" it with a literal.

---

## 3. The system prompt template

`crates/vectra-ai/src/prompt.rs` holds one `const` string with a single hole,
`{{DOCUMENT}}`, so the prompt is reviewable and diffable as text;
`system_prompt(&summary)` substitutes and a test asserts the hole is gone from
the result. Structure: **the contract** → **the API** → **the document**, then
the machine-readable summary.

```text
You are the command generator for Vectra, a parametric 2D design engine.

You NEVER draw. You never write SVG, paths, coordinates of your own invention, or
code in any language other than the command JSON below. You output commands that
the engine validates and applies; the engine owns all geometry.

OUTPUT CONTRACT
- Reply with ONE JSON array of command objects and nothing else. No prose, no
  markdown fences, no comments, no trailing commas.
- Every command must be one of the types in THE COMMAND API, with exactly the
  field names shown. Unknown types or fields are rejected.
- Prefer the smallest plan that satisfies the request. Do not re-create shapes
  that already exist; edit them.

THE COMMAND API
Coordinates are in document units, y is UP. Angles are radians.

  {"type":"CreateNode","id":"$new:card","name":"card","kind":{KIND}}
      KIND is one of:
        {"Rectangle":{"x":P,"y":P,"width":P,"height":P,"corner_radius":P}}
        {"Circle":{"cx":P,"cy":P,"radius":P}}
        {"Arc":{"cx":P,"cy":P,"radius":P,"start_angle":P,"end_angle":P}}
        {"Group":{"children":[NODE_ID,...]}}
  {"type":"DeleteNode","id":NODE_ID}
  {"type":"SetParameter","node_id":NODE_ID,"property":"width","value":VALUE}
  {"type":"SetVariable","name":"base","value":40.0}
  {"type":"RemoveVariable","name":"base"}
  {"type":"DefineExpression","id":"$new:expr:twice","source":"$base * 2"}
  {"type":"RemoveExpression","id":EXPRESSION_ID}
  {"type":"ApplyOperation","id":"$new:op:cut","kind":OPERATION,"inputs":[NODE_ID,...]}
      OPERATION is one of:
        {"type":"boolean","op":"union"}          (2 inputs: union|subtract|intersect|exclude)
        {"type":"offset","distance":P}           (1 input)
        {"type":"fillet","radius":P}             (1 input)
        {"type":"mirror","axis":{"vertical":{"at":P}}}   (1 input)
  {"type":"RemoveOperation","id":OPERATION_ID}
  {"type":"SetOperationEnabled","id":OPERATION_ID,"enabled":false}
  {"type":"AddConstraint","constraint":{"id":"$new:k1","kind":"vertical",
      "targets":[{"node_id":NODE_ID,"property":"x"},{"node_id":NODE_ID,"property":"x"}],
      "strength":"required","value":null}}
      kind is one of: coincident|horizontal|vertical|parallel|perpendicular|
      equal_length|distance|angle.  strength is one of: required|strong|medium|weak.
      `value` is used by distance|angle|parallel (document units / radians), else null.
  {"type":"RemoveConstraint","id":CONSTRAINT_ID}
  {"type":"BindMotion","node_id":NODE_ID,"property":"height",
      "binding":{"Spring":{"target":P,"stiffness":120.0,"damping":14.0,"from":0.0,"at":0.0}}}

P is a PARAMETER (a number, a variable, or an expression):
  {"Literal":40}       a plain number
  {"Variable":"base"}  a document variable, written $base in Vectra
  {"Expression":"<EXPRESSION_ID>"}   an expression defined earlier in this plan
VALUE is a typed parameter: {"Float":P} for numbers, {"Color":{"Literal":"#ff0000"}} for colour.
Use "style.fill", "style.stroke", "style.stroke_width", "style.opacity" as
`property` to paint; use the slots listed for each node to shape it.

ID RULES — these are absolute
- Existing shapes: copy the `id=` value from THE DOCUMENT verbatim. Never invent,
  never abbreviate, never guess.
- New shapes: use a placeholder "$new:<slug>" (for example "$new:card"), and use
  the same placeholder everywhere you mean the same object in this plan. The
  engine mints the real id. Other namespaces: "$new:expr:<slug>",
  "$new:op:<slug>", "$new:k<slug>".
- If the request names something that is not in THE DOCUMENT, do not invent it.

PARAMETRIC RULES — these are what make the output worth keeping
- If a number in the document is already driven by a variable or an expression,
  keep it that way. Never flatten "$base * 2" into 80.
- To make a new dimension parametric, define a variable with SetVariable and
  reference it ({"Variable":"base"}), or define an expression with
  DefineExpression and reference it ({"Expression":"<id>"}). Expression sources
  use $name for variables and the functions sin, cos, abs, clamp, min, max.
- Prefer one variable and several references over several literals.

EXAMPLES
  (four worked prompt → JSON pairs: create-with-corner, variable + expression +
   binding, round an existing shape, boolean subtract followed by a fill)

THE DOCUMENT
{{DOCUMENT}}
```

* `correction_prompt(prompt, summary, correction)` re-states the rules, quotes
  the engine's own refusal verbatim (`the engine rejected … : unknown property
  'corner_radius' …`) and asks for a corrected array — this is the ReAct step.
* `failure_message(error)` is the human sentence the UI shows for each typed
  code; a test walks every code and asserts a message exists.
* `HeuristicPlanner::PHRASINGS` (15 entries) is checked by a test against the
  matcher, so the UI's hint line cannot advertise something the planner cannot do.

---

## 4. The WASM boundary

```rust
// crates/vectra-wasm/src/lib.rs
#[wasm_bindgen] pub fn document_summary(&self) -> String;                    // RULE 2
#[wasm_bindgen] pub fn ai_prompt(&self) -> String;                            // the exact text a hosted model gets
#[wasm_bindgen] pub fn ai_phrasings(&self) -> String;                         // the planner's own hints
#[wasm_bindgen] pub fn ai_generate_commands(&self, prompt: &str, summary_json: &str) -> String;
#[wasm_bindgen] pub fn ai_execute_commands(&mut self, prompt: &str, plan_json: &str) -> String;
#[wasm_bindgen] pub fn ai_execute_with_retry(&mut self, prompt: &str, summary_json: &str) -> String;
```

Envelopes (all JSON strings, like every other method at this boundary):

```jsonc
// preview — `applies: false` is on the wire on purpose: the panel renders a plan
// without ever being able to claim it ran.
{"status":"ok","prompt":"…","plan":[…],"notes":[],"attempt":1,"applies":false}

// execution
{"status":"ok","headline":"2 command(s) in 1 attempt, 1 node(s) re-evaluated",
 "report":{ "prompt","attempts","plan","events","dirty","corrections","notes","summary" },
 "corrections":1}

// failure — typed, with the history when the loop exhausted its attempts
{"status":"error","code":"max-retries-exceeded","message":"…","corrections":[…],"plan":[…]}
```

**The host seam** is what makes RULE 3 true rather than aspirational:

```rust
impl CommandHost for EngineHost<'_> {
    fn apply(&mut self, command: &Command) -> Result<Vec<EngineEvent>, String> {
        let response = self.engine.dispatch_command(&serde_json::to_string(command)?);
        match serde_json::from_str::<CommandResponse>(&response) { … }
    }
    fn rollback(&mut self, steps: usize) -> Result<(), String> { /* engine.undo() × steps */ }
    fn summary(&self) -> DocumentSummary { self.engine.document_summary_value() }
}
```

`apply` is `dispatch_command` — the dependency-graph cycle gate, the expression
pre-compile, the procedural port gate and the **drag guard** all run first, in
the same order they run for a click; `rollback` is `undo`, so a failed plan
unwinds through the ordinary history. The engine's refusal text (a `VectraError`
`Display`) is the string that goes back to the planner.

`ai_generate_commands` runs the same loop against a `AiPreviewHost` with **no
engine behind it** (`apply` returns `Err("preview only: nothing is applied")`),
so "the preview cannot mutate the document" is structural, not a promise. A test
in `vectra-wasm/tests/ai_laws.rs` proves it: same snapshot, same history depth,
and one `undo` still steps over the *creation*.

---

## 5. RULE 3 — the retry loop

```
attempt 1..=MAX_ATTEMPTS (3):
    plan = planner.plan(prompt, summary_live, correction?)      // compile_plan + context check
    for command in plan.commands:                               // in order
        host.apply(command)                                     // → dispatch_command
        on Err: roll back the *applied prefix*, record a Correction
                (attempt, code, error, the command's JSON, what was rolled back),
                hand the error back as the correction prompt, retry
    on Ok: return ExecutionReport { attempts, plan, events, dirty, corrections, notes, summary }
exhausted → AiError::MaxRetriesExceeded { attempts, last_code, last_error, plan, corrections }
             with **nothing applied**.
```

* `MAX_ATTEMPTS = 3` ⇒ **two** self-corrections, as the brief specifies.
* Only `AiError::Unrecognized` (a deterministic planner saying "I do not
  understand this prompt") fails immediately — retrying an identical
  deterministic refusal three times helps nobody. Everything else, including a
  hallucinated node id from the context check, is correctable and is retried.
* A partial attempt never survives: the applied prefix is rolled back *before*
  the correction is requested, so the planner re-plans against a clean document.
* The `Correction` record carries `{attempt, code, error, command, note}` — the
  UI's "what kept going wrong" and the report's evidence.
* Typed codes: `invalid-json`, `unknown-command`, `unknown-node-id`,
  `invalid-placeholder`, `plan-conflict`, `engine-rejected`,
  `max-retries-exceeded`, `unrecognized-prompt`, `planner-failed`, `host-failed`.

---

## 6. Item 4 — the executor that needs no API key

The architecture's only model-shaped part is behind one trait:

```rust
pub struct PlanRequest<'a> { pub prompt: &'a str, pub summary: &'a DocumentSummary,
                             pub correction: Option<&'a Correction> }
pub trait Planner { fn plan(&self, request: &PlanRequest<'_>) -> Result<Plan, AiError>; }
```

* **`HeuristicPlanner`** — the MVP: a small grammar over 15 advertised phrasings
  (`set the width of card to 120`, `make the width of card twice $base`, `round
  the corners of dot by 8`, `fill card with #2266ee`, `union card and dot`,
  `align card and dot vertically`, `animate the height of card to 100 with a
  spring`, `delete dot`, …). It resolves names against the *summary*, keeps
  parametric slots parametric (`twice $base` ⇒ `2 * $base`, never `80`), and
  refuses anything else with a message that names what it does understand. Focus
  is the translation layer + validation loop, exactly as the brief says.
* **`ScriptedPlanner`** — a canned transcript, one reply per attempt, compiled
  through the *real* schema: this is how the retry laws are tested deterministically.
* **`ChatPlanner<F>`** — `F: Fn(&str) -> Result<String, AiError>`: point it at
  any hosted model (the prompt it sends is `ai_prompt()` + the correction text)
  and nothing else in the task changes. No key is needed to *run* or *test* the
  task; the seam is where one would be used.

`$new:` placeholders are how a static reply can still be a coherent plan: the
schema walk mints one real id per placeholder per plan (in the right namespace —
Node, Expression, Operation, Constraint, Track), reuses it everywhere the reply
means the same object, and refuses a placeholder that is used but never created
(`plan-conflict`).

---

## 7. The UI — the AI Assistant panel (Cmd+K)

Section in `apps/vectra-web/src/App.tsx`, styled in `src/App.css`, projected by
pure functions in `src/engine/view-model.ts` (`aiPanelState`, `aiPlanRows`,
`aiCorrectionRows`, `summaryCounts`, `summaryLabels`) — the React component still
holds no engine state and computes no geometry (Task 1.4's boundary holds: it
sends prose and renders envelopes).

| Control | Calls | Shows |
| --- | --- | --- |
| prompt `<input>` (`ai-prompt`) | — | the request; **Enter** = Generate |
| **Generate** (`ai-generate`) | `ai_generate_commands` | one row per command (index, tag, target, verbatim JSON), `applies: false`, "nothing applied yet" |
| **Execute** (`ai-execute`) | `ai_execute_commands` with the previewed plan JSON | the engine's own events in the shared log, the headline, the dirty ids |
| **Auto-correct** (`ai-autocorrect`) | `ai_execute_with_retry` | the same, plus the correction rows — `attempt 1 · engine-rejected · unknown property 'corner_radius' for node … (Circle)` with the loop's note (`nothing was applied; the error went back as a correction prompt`) — and the planner's own answer in the notes line: ``the engine said `corner_radius` is not a slot of Circle; using `radius` `` |
| **Show system prompt** (`ai-prompt-toggle`) | `ai_prompt` | the grounding: node labels + counts, then the full prompt text |
| hint line (`ai-phrasings`) | `ai_phrasings` | what the built-in planner understands |
| ⌘K / Ctrl+K | — | focuses and selects the prompt input, scrolls it into view |

Everything the panel displays is a wire fact: the plan rows print the same JSON
the Execute button hands back to the engine, the dirty list is the engine's
`Dirty` event, and a failure is labelled by its typed code (`refused —
max-retries-exceeded`) with the correction history beneath it. A refused plan
logs "the document is unchanged — a failed plan is rolled back" because that is
what the engine's rollback guarantees.

Cmd+K is a *jump*, not a mode: it never swallows keys from a text field.

---

## 8. Tests — the three laws and where they are proven

**Validity Law** (AI-generated commands parse and execute without a `VectraError`)
* `law_validity_every_supported_prompt_produces_commands_that_run` — all 15
  advertised phrasings: non-empty plan, `attempts == 1`, a mutation event, and a
  document JSON that actually changed.
* `law_validity_any_number_lands` *(proptest)* — any width/height in 1..1000
  lands, exactly the number asked for, in one attempt.
* smoke 40–42 + ui tests: the same through the wasm boundary and the panel's
  projection.

**Context Law** (the AI references existing `NodeId`s, never hallucinating one)
* `law_context_refuses_every_unknown_id` *(proptest)* — for any id-shaped string
  not in the summary, `compile_plan` returns `unknown-node-id` and nothing is
  passed to the engine; a `$new:` placeholder in the same slot is accepted.
* `law_context_the_ai_uses_the_models_ids_and_never_hallucinates_one` and
  `node_lookup_accepts_ids_prefixes_and_names_never_guesses` (exact/prefix/name,
  `Ambiguous` for duplicates, `None` for a miss).
* smoke 41 asserts the planned `node_id` is the id from the summary; smoke 44
  asserts a hallucinated id comes back typed with the id in the message.

**Self-Correction Law** (an invalid command's error is caught and attempt 2
succeeds — or the flow fails gracefully with typed `MaxRetriesExceeded`)
* `law_self_correction_the_engine_refusal_is_fed_back_and_the_retry_succeeds` —
  `round the corners of dot by 8` on a Circle: `attempts == 2`,
  `corrections[0].code == "engine-rejected"`, the error text contains
  `unknown property 'corner_radius'`, the notes say `radius`, and `dot.radius == 8`.
* `law_self_correction_gives_up_typed_after_two_corrections` and
  `law_self_correction_a_hallucinated_id_is_retried_and_then_reported` — three
  planner calls, `max-retries-exceeded`, `last_code == "unknown-node-id"`.
* `law_self_correction_a_partial_plan_is_rolled_back_before_the_retry`.
* smoke 43 (wasm boundary: attempts 2, radius 8, a refused plan leaves no history)
  and smoke 44 (`invalid-json`, `unknown-node-id`/`engine-rejected`,
  `unrecognized-prompt`, document byte-identical afterwards).

RULE 1's strictness is tested beside them: `a_misspelled_or_invented_field_is_refused_not_ignored`
(a typo of a *required* field, a typo of an *optional* one that serde would have
silently dropped, a stray key nested inside `kind`, and a legitimate `null` that
must still be accepted).

Also: `crates/vectra-core/tests/summary.rs` (8 tests — labels, the source/now
split, missing expressions, lookup and ambiguity, constraint target strings, the
empty canvas, the budget's `… N more`, the prompt block's section order) and
`crates/vectra-wasm/tests/ai_laws.rs` (9 tests — summary/prompt/phrasings,
preview-applies-nothing, approved-plan execution with dirty ids, the cycle gate
refusing an AI plan, the drag guard mid-gesture, the ReAct loop, the
hallucination path, determinism up to minted ids, the empty document).

**Four findings worth carrying forward**

1. **`serde_json`'s default float parser is 1 ULP off.** `475.81511995683564`
   parsed to `475.8151199568357` (bit patterns `…649` → `…650`), which broke the
   Validity Law's exact-number assertions. `Display`/`to_string`/`json!` were
   already exact — only parsing was lossy. The workspace now sets
   `serde_json = { version = "1", features = ["float_roundtrip"] }`. **Keep it.**
2. **`$new:` placeholders appear inside `ParamValue`s, not only in id fields.**
   The planner emits `{"Float":{"Expression":"$new:expr:value"}}`; the schema
   walk now resolves an `Expression`-shaped parameter exactly as it resolves a
   top-level id (`{"Variable":"base"}` stays a name and is left alone). Without
   this, the engine rejected the plan with `UUID parsing failed: invalid
   character: found '$' at 0` — a *good* failure (the boundary refused to
   guess), which is why the fix went into the resolver rather than the engine.
3. **Never assert on a command's *text*.** A plan assertion that searched a
   command's JSON for the substring `"80"` (proving a parametric slot had not been
   flattened to the number it evaluates to) failed about one run in eight — a node
   id is a random UUID and `80` is two hex digits. The assertion now reads the
   field (`value.Float.Expression` is set, `Literal`/`Variable` are null). The
   flake was caught by repeating the workspace gate; the fix is strictly stronger.
4. **An executed plan is one history entry per command**, like every other
   multi-command action in the UI (`runSequence`). Atomicity is provided by the
   AI layer's rollback, not by the history: a plan that fails half-way is fully
   unwound before the correction, while a successful plan undoes step by step
   (newest first). `Command::Batch` remains available if a single-entry gesture
   is ever wanted for AI edits.

---

## 9. Files

**Added**

* `crates/vectra-core/src/summary.rs`, `crates/vectra-core/tests/summary.rs`
* `crates/vectra-ai/` — `Cargo.toml`, `src/{lib,schema,error,prompt,planner,exec}.rs`
  (3 639 lines), `tests/ai_laws.rs` (~1 020 lines)
* `crates/vectra-wasm/tests/ai_laws.rs`

**Changed**

* `Cargo.toml` (workspace: member 12, `vectra-ai` dep, `serde_json` `float_roundtrip`)
* `crates/vectra-core/src/{lib,document}.rs` (`pub mod summary`, re-exports,
  `NodeKind::scalar_slots()`)
* `crates/vectra-wasm/{Cargo.toml,src/lib.rs}` (the six methods, `EngineHost`,
  `AiPreviewHost`, the three envelopes, `CommandResponse: Deserialize`)
* `apps/vectra-web/src/{App.tsx,App.css}`, `src/engine/{wire,client,view-model}.ts`,
  `tests/view-model.test.ts`, `scripts/smoke.mjs`, regenerated `src/wasm/*`

---

## 10. Open items / next

* A hosted model changes exactly one line: `HeuristicPlanner::new()` →
  `ChatPlanner::new(fetch_prompt)`. The schema, grounding, validation, retry and
  UI are unchanged, and `ai_prompt()` already returns the exact text to send.
* Multi-turn conversation, streaming and tool-calls are **not** in Task 9.0's
  scope; the envelope set (`preview` / `report` / `error`) is what a streaming
  UI would extend.
* The panel's Execute runs a previewed plan verbatim; a "one-undo AI edit"
  (wrapping a plan in `Command::Batch`) is a UI decision left open on purpose.
