//! The **system prompt** (Task 9.0, RULE 2): what the model is told, and the
//! correction message it gets when it is wrong.
//!
//! > "Create the 'System Prompt' template that teaches the LLM how to use
//! > Vectra's Command API, including examples of valid JSON payloads."
//!
//! Three sections, in this order, for a reason:
//!
//! 1. **The contract** — you emit commands, never geometry. Permanent.
//! 2. **The API** — the command set with its exact JSON shape, and the id rule.
//!    Permanent, and *complete*: a model that has to guess a field name produces
//!    the very hallucinations RULE 1 forbids, so the surface is spelled out.
//! 3. **The document** — the live [`DocumentSummary`] block from
//!    `vectra-core`. This is the only part that changes between calls, and it is
//!    the only place ids come from.
//!
//! The template is a `const` string with one `{{DOCUMENT}}` hole rather than a
//! bag of `format!` calls, so the prompt can be reviewed, diffed and tested as
//! *text* — [`system_prompt`] only substitutes, and a test asserts the hole is
//! gone from the result.

use vectra_core::DocumentSummary;

use crate::error::Correction;

/// The document-summary placeholder in [`SYSTEM_PROMPT`].
pub const DOCUMENT_HOLE: &str = "{{DOCUMENT}}";

/// The prompt template, verbatim. `{{DOCUMENT}}` is replaced by the summary.
// `r##` (not `r#`): the prompt contains `"#` inside hex colours like "#ff0000",
// which would otherwise close the raw string.
pub const SYSTEM_PROMPT: &str = r##"You are the command generator for Vectra, a parametric 2D design engine.

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

"add a rectangle named card, 80 by 60, rounded by 8, at the origin"
[{"type":"CreateNode","id":"$new:card","name":"card","kind":{"Rectangle":
  {"x":{"Literal":0},"y":{"Literal":0},"width":{"Literal":80},
   "height":{"Literal":60},"corner_radius":{"Literal":8}}}}]

"make the card twice as wide as $base and give it a variable called base = 40"
[{"type":"SetVariable","name":"base","value":40.0},
 {"type":"DefineExpression","id":"$new:expr:twice","source":"$base * 2"},
 {"type":"CreateNode","id":"$new:card","name":"card","kind":{"Rectangle":
  {"x":{"Literal":0},"y":{"Literal":0},"width":{"Expression":"$new:expr:twice"},
   "height":{"Literal":60},"corner_radius":{"Literal":8}}}}]

"round the corners of card by 12"
[{"type":"SetParameter","node_id":"<card's id from THE DOCUMENT>",
  "property":"corner_radius","value":{"Float":{"Literal":12}}}]

"cut dot out of card and fill the result red"
[{"type":"ApplyOperation","id":"$new:op:cut","kind":{"type":"boolean","op":"subtract"},
  "inputs":["<card id>","<dot id>"]},
 {"type":"SetParameter","node_id":"$new:op:cut","property":"style.fill",
  "value":{"Color":{"Literal":"#ff0000"}}}]

STRUCTURAL COMMANDS — how you change *arrangement*, not pixels
- DuplicateNode: {"type":"DuplicateNode","id":"$new:v2","source":"<id>","name":"card 2"}
  copies a shape (parameters and all) so it can be edited independently.
- ApplyOperation: {"type":"ApplyOperation","id":"$new:op:u","kind":{"type":"boolean",
  "op":"union"|"subtract"|"intersect"|"exclude"},"inputs":["<a>","<b>"]} — a boolean
  takes exactly TWO inputs; fold a longer list two at a time.
- CreateComponent: {"type":"CreateComponent","id":"$new:component","name":"card",
  "members":["<id>","<id>"]} — makes the members a *Smart Component*: one master
  with props (size, stroke_width, corner_radius, color) that every instance sets
  for itself. Omit "props" to let the engine infer them.
- InstantiateComponent: {"type":"InstantiateComponent","id":"$new:i1",
  "master":"$new:component","name":"card 24px"} — places an independent copy.
- SetComponentProp: {"type":"SetComponentProp","target":"$new:i1","prop":"size",
  "value":{"Float":{"Literal":32}}} — one instance only; the master and the other
  instances do not move. Colour props take {"Color":{"Literal":"#2266ee"}}.
- CreateArtboard: {"type":"CreateArtboard","id":"$new:board:32","name":"icon 32",
  "x":0,"y":0,"width":32,"height":32,"background":{"r":255,"g":255,"b":255,"a":255}}
  then CreateLayer {"type":"CreateLayer","id":"$new:layer:32","name":"icon 32",
  "artboard":"$new:board:32"} — a frame for a size, and the layer inside it.
- AddConstraint: {"type":"AddConstraint","constraint":{"id":"$new:constraint:c",
  "kind":"vertical"|"horizontal"|"parallel"|"coincident"|"perpendicular"|
  "equal_length"|"distance"|"angle","targets":[{"node_id":"<id>",
  "property":"x"},{"node_id":"<id>","property":"x"}],"strength":"required",
  "value":null}} — the engine keeps it true from now on.

THE SELECTION — prompts that say "this", "these" or "the selection" mean the
nodes under SELECTION in THE DOCUMENT below, never the whole document.

THE DOCUMENT
{{DOCUMENT}}
"##;

/// The full system prompt for a call: the template with the live summary in it.
pub fn system_prompt(summary: &DocumentSummary) -> String {
    SYSTEM_PROMPT.replace(DOCUMENT_HOLE, summary.to_text().trim_end())
}

/// The message a retrying model receives (RULE 3, the ReAct half).
///
/// It is a *correction*, not a restart: the original request, what the engine
/// said, the command that failed, and the rule it broke. The document block is
/// re-injected by the caller because the plan that failed may have been partly
/// about the document as it was — the model is always reasoning about *now*.
pub fn correction_prompt(
    prompt: &str,
    summary: &DocumentSummary,
    correction: &Correction,
) -> String {
    let mut out = String::new();
    out.push_str("Your previous plan was REJECTED by the engine. Fix it and reply with a corrected JSON array.\n\n");
    out.push_str(&format!("Original request: {prompt}\n"));
    out.push_str(&format!("Attempt: {}\n", correction.attempt));
    out.push_str(&format!(
        "Error ({}): {}\n",
        correction.code, correction.error
    ));
    if let Some(command) = &correction.command {
        out.push_str(&format!(
            "Rejected command: {}\n",
            serde_json::to_string(command).unwrap_or_default()
        ));
    }
    out.push_str(
        "\nRules for the correction:\n\
         - Change only what the error is about; keep the rest of the plan.\n\
         - Do not repeat a command the engine has already rejected.\n\
         - Every id must still come from THE DOCUMENT below (or be a $new: placeholder).\n\
         - Nothing was applied: the document is exactly as it is below.\n\n",
    );
    out.push_str("THE DOCUMENT\n");
    out.push_str(summary.to_text().trim_end());
    out
}

/// What a *human* reads when the loop gave up. Short, and it names the door out.
pub fn failure_message(error: &crate::error::AiError) -> String {
    match error {
        crate::error::AiError::Unrecognized { detail, .. } => format!(
            "I cannot turn that into commands yet ({detail}). Try naming a shape and a slot, \
             for example \"round the corners of card by 8\" or \"add a circle named dot at 40 20 radius 10\"."
        ),
        crate::error::AiError::MaxRetriesExceeded { attempts, last_error, .. } => format!(
            "The engine refused the plan after {attempts} attempt(s) — nothing was applied. \
             Last error: {last_error}"
        ),
        other => other.to_string(),
    }
}
