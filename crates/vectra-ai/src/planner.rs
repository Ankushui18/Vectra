//! Planners: the model side of the AI layer.
//!
//! A [`Planner`] answers one question — *given this prompt and this document,
//! what commands?* — and returns `Result<Plan, AiError>`. That is the entire
//! contract, which is why swapping the MVP's local planner for a hosted model
//! later is a one-line change in `vectra-wasm` and nothing else moves.
//!
//! Three implementations ship:
//!
//! * [`HeuristicPlanner`] — the MVP's "model". It parses a natural-language
//!   prompt with a small, documented grammar, grounded *only* on the summary it
//!   is given, and emits **the same JSON an LLM would** (including `$new:`
//!   placeholders), so every prompt exercises the real translation and
//!   validation path rather than a shortcut. It also reads the correction it is
//!   handed — RULE 3's ReAct half: a slot the engine rejected is remapped from
//!   the engine's own message, a node the engine does not know is dropped, and a
//!   command refused for closing a cycle is never repeated.
//! * [`ScriptedPlanner`] — a canned transcript, for tests. It is how the
//!   self-correction law is proven with a plan that is invalid *on purpose*.
//! * [`ChatPlanner`] — the hook for a real model: hand it a callable from prompt
//!   text to reply text and it does the rest, through the identical strict
//!   pipeline. It exists so the boundary has a shape to point at without
//!   pretending an HTTP client belongs in this task.
//!
//! [`Plan`] carries the planner's own `notes` — what it changed after a
//! correction, in its own words. Those land in the execution report, which is
//! how a user sees *why* the second attempt differed from the first.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;

use serde_json::{json, Value};
use vectra_core::{
    BooleanOp, Command, DocumentSummary, MirrorAxis, MotionBinding, NodeKind, OperationKind,
    Parameter, SummaryNode,
};

use crate::error::{AiError, Correction};
use crate::schema::compile_plan;

/// What a planner produced: the commands, plus anything it wants the report to
/// say about them.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Plan {
    pub commands: Vec<Command>,
    /// One line per decision worth surfacing ("the engine said `radius` is not a
    /// slot of Rectangle; using `corner_radius`").
    pub notes: Vec<String>,
}

impl Plan {
    pub fn of(commands: Vec<Command>) -> Self {
        Self {
            commands,
            notes: Vec::new(),
        }
    }

    pub fn with_notes(commands: Vec<Command>, notes: Vec<String>) -> Self {
        Self { commands, notes }
    }
}

/// Everything a planner is told.
pub struct PlanRequest<'a> {
    /// What the user typed.
    pub prompt: &'a str,
    /// The live document, as `vectra-core` summarises it (RULE 2). A planner may
    /// not reach past this: no engine, no scene, no scene graph.
    pub summary: &'a DocumentSummary,
    /// Present on a retry (RULE 3): what the engine said about the last attempt.
    pub correction: Option<&'a Correction>,
}

/// prompt + summary (+ what went wrong last time) → commands.
pub trait Planner {
    fn plan(&self, request: &PlanRequest<'_>) -> Result<Plan, AiError>;
}

/// A planner in front of any text-in/text-out model.
pub struct ChatPlanner<F>
where
    F: Fn(&str) -> Result<String, AiError>,
{
    call: F,
}

impl<F> ChatPlanner<F>
where
    F: Fn(&str) -> Result<String, AiError>,
{
    pub fn new(call: F) -> Self {
        Self { call }
    }
}

impl<F> Planner for ChatPlanner<F>
where
    F: Fn(&str) -> Result<String, AiError>,
{
    fn plan(&self, request: &PlanRequest<'_>) -> Result<Plan, AiError> {
        let full = match request.correction {
            Some(correction) => {
                crate::prompt::correction_prompt(request.prompt, request.summary, correction)
            }
            None => crate::prompt::system_prompt(request.summary),
        };
        let reply = (self.call)(&full)?;
        Ok(Plan::of(compile_plan(&reply, request.summary)?))
    }
}

/// A canned transcript: one reply per attempt, in order.
///
/// The replies go through [`compile_plan`], so a scripted reply that is
/// malformed fails *here*, exactly as a real model's would.
#[derive(Default)]
pub struct ScriptedPlanner {
    replies: RefCell<VecDeque<String>>,
    calls: Cell<usize>,
}

impl ScriptedPlanner {
    pub fn new<I: IntoIterator<Item = impl Into<String>>>(replies: I) -> Self {
        Self {
            replies: RefCell::new(replies.into_iter().map(Into::into).collect()),
            calls: Cell::new(0),
        }
    }

    /// How many times the model was asked — the attempt count a test asserts on.
    pub fn calls(&self) -> usize {
        self.calls.get()
    }
}

impl Planner for ScriptedPlanner {
    fn plan(&self, request: &PlanRequest<'_>) -> Result<Plan, AiError> {
        self.calls.set(self.calls.get() + 1);
        let reply = self
            .replies
            .borrow_mut()
            .pop_front()
            .ok_or_else(|| AiError::Planner {
                detail: format!(
                    "the scripted model ran out of replies after {} attempt(s)",
                    self.calls.get()
                ),
            })?;
        Ok(Plan::of(compile_plan(&reply, request.summary)?))
    }
}

// ── the heuristic planner ──────────────────────────────────────────────

/// The MVP planner: a documented natural-language grammar over the summary.
///
/// It supports the phrasings in [`HeuristicPlanner::PHRASINGS`] and nothing
/// else — an unrecognised prompt is refused with [`AiError::Unrecognized`]
/// rather than guessed at, because a guess would be applied to a real document.
/// The grammar is deliberately small: what this task proves is the *architecture*
/// (schema → ground → validate → correct), and the planner is the one component a
/// hosted model replaces verbatim.
#[derive(Default)]
pub struct HeuristicPlanner;

impl HeuristicPlanner {
    pub fn new() -> Self {
        Self
    }

    /// The phrasings the planner understands — used by the UI hint line and by
    /// the failure message, and asserted in tests so it cannot drift from the
    /// matcher's behaviour.
    pub const PHRASINGS: [&'static str; 15] = [
        "add a rectangle named card at 20 30 size 80x60",
        "add a circle named dot at 40 20 radius 10",
        "add an arc named swoosh at 0 0 radius 50 from 0 to 90",
        "set the width of card to 120",
        "set the radius of card to 8",
        "round the corners of dot by 8",
        "make the width of card twice $base",
        "move dot to 100 50",
        "fill card with #2266ee",
        "delete dot",
        "union card and dot",
        "subtract dot from card",
        "fillet card by 8",
        "align card and dot vertically",
        "animate the height of card to 100 with a spring",
    ];

    /// The **structural macros** (Task 10.6 RULE 2 and 3), the phrases behind the
    /// command bar's one-tap chips.
    ///
    /// They are listed apart from [`HeuristicPlanner::PHRASINGS`] for one honest
    /// reason: every one of them needs a selection, so they are not interchangeable
    /// with the sentences above, and a test can check them against a document
    /// that *has* one. A prompt that names a macro with nothing selected falls
    /// through to the sentence grammar and, if it matches nothing there, is
    /// refused — never guessed at.
    pub const MACRO_PHRASINGS: [&'static str; 5] = [
        "make this geometric",
        "create 4 color variations",
        "align perfectly",
        "make this a component",
        "generate an icon set at 16 32 48",
    ];
}

/// Everything the built-in planner understands, for the UI's hint line and for
/// the failure message. The one list cannot drift from the matcher: the tests in
/// `tests/ai_laws.rs` run every entry through it.
pub fn phrasings() -> Vec<&'static str> {
    HeuristicPlanner::PHRASINGS
        .iter()
        .chain(HeuristicPlanner::MACRO_PHRASINGS.iter())
        .copied()
        .collect()
}

impl Planner for HeuristicPlanner {
    fn plan(&self, request: &PlanRequest<'_>) -> Result<Plan, AiError> {
        let mut clue = Clue {
            summary: request.summary,
            slot_fixes: Vec::new(),
            banned_ids: Vec::new(),
            banned_commands: Vec::new(),
            notes: Vec::new(),
            instruction: request.prompt.to_string(),
        };
        if let Some(correction) = request.correction {
            clue.absorb(correction);
        }
        let json = clue.plan_json()?;
        let text = serde_json::to_string(&json).unwrap_or_else(|_| "[]".to_string());
        // The mock model speaks the same wire format a hosted model does: JSON
        // with `$new:` placeholders, put through the strict compiler.
        let commands = clue.apply_fixes(compile_plan(&text, request.summary)?);
        if commands.is_empty() {
            return Err(AiError::Unrecognized {
                prompt: request.prompt.to_string(),
                detail: "every command in the plan was dropped after the engine's rejection"
                    .to_string(),
            });
        }
        Ok(Plan::with_notes(commands, clue.notes))
    }
}

/// Working state for one heuristic attempt.
struct Clue<'a> {
    summary: &'a DocumentSummary,
    /// `(node id, property the engine rejected, property the node kind has)`.
    slot_fixes: Vec<(String, String, String)>,
    /// Ids the engine said do not exist: never referenced again.
    banned_ids: Vec<String>,
    /// Commands (by JSON) the engine refused for a structural reason: repeating
    /// them cannot help.
    banned_commands: Vec<String>,
    notes: Vec<String>,
    instruction: String,
}

impl Clue<'_> {
    /// Read the previous failure and decide what to do differently. This is the
    /// whole of the MVP planner's self-correction, and it is deliberately
    /// *narrow*: read the engine's typed message, consult the summary, change one
    /// thing. It never retries blindly.
    fn absorb(&mut self, correction: &Correction) {
        let error = correction.error.clone();
        if let Some((property, node_id, kind)) = parse_unknown_property(&error) {
            let slot = closest_slot(&kind, &property);
            match slot {
                Some(slot) => {
                    self.notes.push(format!(
                        "the engine said `{property}` is not a slot of {kind}; using `{slot}`"
                    ));
                    self.slot_fixes.push((node_id, property, slot));
                }
                None => self.notes.push(format!(
                    "the engine said `{property}` is not a slot of {kind}, and no close slot exists"
                )),
            }
        }
        if let Some(node_id) = parse_node_not_found(&error) {
            self.notes.push(format!(
                "the engine does not know node {node_id}; dropping references to it"
            ));
            self.banned_ids.push(node_id);
        }
        if error.contains("cyclic dependency") || error.contains("cycle") {
            if let Some(command) = &correction.command {
                self.notes
                    .push("that command would close a cycle; repeating it cannot help".to_string());
                self.banned_commands
                    .push(serde_json::to_string(command).unwrap_or_default());
            }
        }
    }

    /// Apply the fixes from [`Clue::absorb`]: rewrite slots, drop commands that
    /// reference banned nodes or repeat a banned command.
    ///
    /// Dropping is **transitive**. A macro's plan is a chain — a union reads the
    /// union before it, a prop write names the instance the command before it
    /// placed — so dropping one command can orphan the next: the orphan names an
    /// id nothing creates any more, which is a *second* engine refusal in a plan
    /// that was supposed to be fixed. So the filter runs to a fixpoint, each
    /// round collecting the ids the dropped commands would have minted.
    fn apply_fixes(&self, commands: Vec<Command>) -> Vec<Command> {
        let mut dropped_ids: Vec<String> = Vec::new();
        let mut current = commands;
        loop {
            let mut kept: Vec<Command> = Vec::with_capacity(current.len());
            let mut orphaned: Vec<String> = Vec::new();
            for mut command in current.drain(..) {
                if let Command::SetParameter {
                    node_id, property, ..
                } = &mut command
                {
                    if let Some((_, _, good)) = self
                        .slot_fixes
                        .iter()
                        .find(|(node, bad, _)| node == &node_id.to_string() && bad == property)
                    {
                        *property = good.clone();
                    }
                }
                let rendered = serde_json::to_value(&command)
                    .ok()
                    .and_then(|value| serde_json::to_string(&value).ok())
                    .unwrap_or_default();
                let banned = self.banned_commands.contains(&rendered)
                    || self
                        .banned_ids
                        .iter()
                        .any(|id| rendered.contains(id.as_str()))
                    || dropped_ids.iter().any(|id| rendered.contains(id.as_str()));
                if banned {
                    if let Some(id) = crate::schema::created_id(&command) {
                        orphaned.push(id);
                    }
                    continue;
                }
                kept.push(command);
            }
            let stable = orphaned.is_empty();
            dropped_ids.extend(orphaned);
            current = kept;
            if stable {
                return current;
            }
        }
    }

    /// Parse the instruction into command JSON.
    fn plan_json(&mut self) -> Result<Vec<Value>, AiError> {
        let text = self.instruction.trim().to_string();
        if text.is_empty() {
            return Err(AiError::Unrecognized {
                prompt: text,
                detail: "the prompt is empty".to_string(),
            });
        }
        // Pair and chain verbs first: "union card and dot" must not be split.
        if let Some(commands) = self.pair(&text) {
            return Ok(commands);
        }
        // ── Task 10.6: the structural macros (RULE 2) ──────────────────────
        //
        // These are the prompts that talk about *the selection*: "make this
        // geometric", "create 4 colour variations", "align perfectly", "make
        // this a component", "generate an icon set at 16 32 48". They are
        // tried before the sentence grammar because their subject is the
        // canvas, not a name the grammar could look up.
        if let Some(commands) = self.macro_plan(&text)? {
            return Ok(commands);
        }
        let mut all = Vec::new();
        let mut subject: Option<SummaryNode> = None;
        for raw in split_clauses(&text) {
            let clause = raw.trim();
            if clause.is_empty() {
                continue;
            }
            let clause = self.anaphora(clause, subject.as_ref());
            let commands = self.clause(&clause)?;
            if let Some(node) = self.node(&clause) {
                subject = Some(node);
            }
            all.extend(commands);
        }
        if all.is_empty() {
            return Err(self.unrecognized(
                &text,
                &format!(
                    "no rule matched; try a phrasing like \"{}\"",
                    HeuristicPlanner::PHRASINGS[0]
                ),
            ));
        }
        Ok(all)
    }

    /// **The structural macros** (Task 10.6 RULE 2): a prompt about the
    /// selection, answered with strictly typed commands.
    ///
    /// Never pixels, never SVG, never a picture: every arm below returns JSON
    /// commands that go through the same [`crate::schema::compile_plan`] gate as
    /// any other plan, so a macro the engine rejects is corrected (or refused)
    /// exactly like a sentence is.
    ///
    /// `None` means "not a macro", and the sentence grammar gets its turn.
    /// A structural macro, if the prompt names one and the selection supports it.
    ///
    /// `Ok(None)` means "not a macro, try the sentence grammar". `Err` means the
    /// prompt *did* name a macro but the selection cannot support it — a typed
    /// refusal with a sentence a designer can act on is better than falling
    /// through to "I don't know that phrasing", which would be a lie: the macro
    /// was understood perfectly well.
    fn macro_plan(&mut self, text: &str) -> Result<Option<Vec<Value>>, AiError> {
        let lower = text.to_lowercase();
        let selected: Vec<SummaryNode> = self.summary.selected().into_iter().cloned().collect();
        let numbers = numbers_in(text);

        // ── Icon Studio (RULE 3) ────────────────────────────────────────────
        // "generate an icon set at 16 32 48": one master, one instance per size,
        // each on its own artboard, all scaled by the same law — the stroke
        // never thins into a hairline at 16px. This is the JSON face of
        // `vectra_core::component::icon_set_plan`; a macro cannot call that
        // helper directly, because it mints ids up front and a plan's ids have
        // to stay `$new:` tokens until the engine resolves them.
        if !selected.is_empty()
            && (lower.contains("icon set")
                || lower.contains("icon-set")
                || lower.contains("icon sizes"))
        {
            let sizes: Vec<f64> = {
                let picked: Vec<f64> = numbers
                    .iter()
                    .copied()
                    .filter(|size| *size >= 4.0 && *size <= 512.0)
                    .collect();
                if picked.is_empty() {
                    vec![16.0, 32.0, 48.0]
                } else {
                    picked
                }
            };
            let mut commands = vec![self.component_json(&selected)];
            commands.extend(icon_set_json(&selected[0].name, &sizes));
            let smallest = sizes.iter().copied().fold(f64::INFINITY, f64::min);
            self.notes.push(format!(
                "built an icon master from \u{201c}{}\u{201d} at {} \u{2014} stroke and corner \
                 radius scale with each artboard, so nothing thins out at {}px",
                selected[0].name,
                sizes
                    .iter()
                    .map(|size| format!("{}px", trim(*size)))
                    .collect::<Vec<_>>()
                    .join(", "),
                trim(smallest),
            ));
            return Ok(Some(commands));
        }

        // ── Smart Component (RULE 1) ────────────────────────────────────────
        if !selected.is_empty()
            && (lower.contains("make this a component")
                || lower.contains("make it a component")
                || lower.contains("create a component")
                || lower.contains("create component")
                || lower.contains("componentise")
                || lower.contains("componentize"))
        {
            self.notes.push(format!(
                "turned {} into a component \u{2014} Size, Stroke, Corner and Color are now props \
                 you can set per instance",
                self.summary.selection_prose()
            ));
            return Ok(Some(vec![self.component_json(&selected)]));
        }

        // ── Colour variations ───────────────────────────────────────────────
        if lower.contains("variation")
            || lower.contains("colour version")
            || lower.contains("color version")
        {
            if selected.is_empty() {
                // Nothing selected: "this" has no referent, so the sentence
                // grammar gets its turn (it will refuse with its own message).
                return Ok(None);
            }
            let count = numbers
                .iter()
                .copied()
                .find(|count| *count >= 2.0 && *count <= 8.0)
                .map(|count| count.round() as usize)
                .unwrap_or(4);
            let base = selected
                .iter()
                .find_map(|node| slot_source(node, "style.fill"))
                .and_then(|hex| vectra_core::Color::from_hex(&hex))
                .unwrap_or_else(|| vectra_core::Color::rgb(0x22, 0x66, 0xee));
            let palette = hue_palette(base, count);
            let mut commands = Vec::new();
            // Variation 1 *is* the original — the family's reference colour.
            for (index, fill) in palette.iter().enumerate().skip(1) {
                for (order, node) in selected.iter().enumerate() {
                    let copy = format!("$new:variation:{index}:{order}");
                    commands.push(json!({
                        "type": "DuplicateNode",
                        "id": copy,
                        "source": node.id,
                        "name": format!("{} {}", node.name, index + 1),
                    }));
                    if let Some((slot, value)) = position_slot(node) {
                        let step = size_slot(node).unwrap_or(24.0) + 16.0;
                        commands.push(json!({
                            "type": "SetParameter",
                            "node_id": copy,
                            "property": slot,
                            "value": {"Float": {"Literal": value + step * index as f64}},
                        }));
                    }
                    if node.slots.iter().any(|slot| slot.property == "style.fill") {
                        commands.push(json!({
                            "type": "SetParameter",
                            "node_id": copy,
                            "property": "style.fill",
                            "value": {"Color": {"Literal": fill}},
                        }));
                    }
                }
            }
            if commands.is_empty() {
                return Ok(None);
            }
            self.notes.push(format!(
                "made {count} colour variation(s) of {} \u{2014} {} command(s), all of them \
                 editable shapes",
                self.summary.selection_prose(),
                commands.len()
            ));
            return Ok(Some(commands));
        }

        // ── Make it geometric ───────────────────────────────────────────────
        if lower.contains("geometric")
            || lower.contains("geometrise")
            || lower.contains("geometrize")
        {
            if selected.is_empty() {
                // Nothing selected: "this" has no referent, so the sentence
                // grammar gets its turn (it will refuse with its own message).
                return Ok(None);
            }
            let mut commands = Vec::new();
            let mut unified = 0usize;
            for node in &selected {
                // 1. Whole-number geometry: art that sits on the grid.
                for (property, value) in snap_slots(node) {
                    commands.push(json!({
                        "type": "SetParameter",
                        "node_id": node.id,
                        "property": property,
                        "value": {"Float": {"Literal": value}},
                    }));
                }
                // 2. Sharp corners: a radius is a deliberate softening, and this
                //    prompt is the request to remove it.
                if node
                    .slots
                    .iter()
                    .any(|slot| slot.property == "corner_radius")
                {
                    commands.push(json!({
                        "type": "SetParameter",
                        "node_id": node.id,
                        "property": "corner_radius",
                        "value": {"Float": {"Literal": 0.0}},
                    }));
                }
            }
            // 3. One shape, not several overlapping ones: a boolean union of the
            //    first two. Two is the engine's arity — a boolean takes a subject
            //    and a clip — and an operation cannot yet read another operation,
            //    so a chain that folded *all* of the selection in one press would
            //    be a plan the engine refuses. The macro therefore unifies the
            //    pair and says so; the next shape folds in on the next run.
            if let [first, second, ..] = selected.as_slice() {
                commands.push(json!({
                    "type": "ApplyOperation",
                    "id": "$new:op:union:0",
                    "kind": {"type": "boolean", "op": "union"},
                    "inputs": [first.id, second.id],
                }));
                unified += 1;
            }
            if commands.is_empty() {
                return Ok(None);
            }
            let fold = if unified == 0 {
                String::new()
            } else if selected.len() > 2 {
                // Honest about the arity: the designer asked for one shape and
                // gets one union, so the sentence says which pair it joined.
                format!(
                    " and unified \u{201c}{}\u{201d} with \u{201c}{}\u{201d} (a union takes two \
                     shapes; the rest are snapped and ready to fold in next)",
                    selected[0].name, selected[1].name
                )
            } else {
                " and unified them into one shape".to_string()
            };
            self.notes.push(format!(
                "snapped {} to whole numbers{} \u{2014} {} command(s)",
                self.summary.selection_prose(),
                fold,
                commands.len()
            ));
            return Ok(Some(commands));
        }

        // ── Align perfectly ─────────────────────────────────────────────────
        if lower.contains("align") {
            if selected.is_empty() {
                // Nothing selected: "this" has no referent, so the sentence
                // grammar gets its turn (it will refuse with its own message).
                return Ok(None);
            }
            if selected.len() < 2 {
                // Understood, but impossible: aligning a column needs two shapes,
                // and the honest answer says so rather than asking the designer to
                // rephrase a sentence the planner already parsed.
                return Err(AiError::Unrecognized {
                    prompt: text.to_string(),
                    detail: "aligning needs at least two shapes \u{2014} select another one and try again"
                        .to_string(),
                });
            }
            let mut commands = Vec::new();
            let anchor = &selected[0];
            let (anchor_x, _) = axis_slots(anchor);
            // Every other selected node shares the anchor's column …
            for (index, node) in selected.iter().skip(1).enumerate() {
                let (x_slot, _) = axis_slots(node);
                commands.push(json!({
                    "type": "AddConstraint",
                    "constraint": {
                        // One placeholder per constraint: a `$new:` slug mints
                        // **one** id and reuses it, so a shared slug would make
                        // every constraint in the column the same constraint.
                        "id": format!("$new:constraint:column:{index}"),
                        "kind": "vertical",
                        "targets": [
                            {"node_id": anchor.id, "property": anchor_x},
                            {"node_id": node.id, "property": x_slot},
                        ],
                        "strength": "required",
                        "value": Value::Null,
                    },
                }));
            }
            // … and, with three or more, the rows keep the spacing they have:
            // "perfectly" means a column, not a pile.
            if selected.len() >= 3 {
                let ys: Vec<f64> = selected
                    .iter()
                    .filter_map(|node| slot_number(node, axis_slots(node).1))
                    .collect();
                for (index, node) in selected.iter().enumerate().skip(2) {
                    let previous = &selected[index - 1];
                    let (_, y_slot) = axis_slots(node);
                    let (_, y_previous) = axis_slots(previous);
                    let gap = match (ys.get(index - 1), ys.get(index)) {
                        (Some(before), Some(after)) => *before - *after,
                        _ => continue,
                    };
                    commands.push(json!({
                        "type": "AddConstraint",
                        "constraint": {
                            // One placeholder per constraint — see the column
                            // constraints above.
                            "id": format!("$new:constraint:spacing:{index}"),
                            "kind": "parallel",
                            "targets": [
                                {"node_id": previous.id, "property": y_previous},
                                {"node_id": node.id, "property": y_slot},
                            ],
                            "strength": "required",
                            "value": gap,
                        },
                    }));
                }
            }
            if commands.is_empty() {
                return Ok(None);
            }
            self.notes.push(format!(
                "aligned {} into a column \u{2014} {} constraint(s), applied",
                self.summary.selection_prose(),
                commands.len()
            ));
            return Ok(Some(commands));
        }

        Ok(None)
    }

    /// The `CreateComponent` command for a set of nodes, as plan JSON.
    fn component_json(&self, members: &[SummaryNode]) -> Value {
        json!({
            "type": "CreateComponent",
            "id": "$new:component",
            "name": members
                .first()
                .map(|node| format!("{} component", node.name))
                .unwrap_or_else(|| "Component".to_string()),
            "members": members.iter().map(|node| node.id.clone()).collect::<Vec<_>>(),
        })
    }

    /// Give a subject-less clause the shape the previous clause was about.
    ///
    /// "set the width of card to 12 and the height to 20" is one instruction
    /// about one shape; a person resolves "the height" from the context, and so
    /// does this — only when the clause has no shape of its own, names a slot,
    /// and starts like a continuation. Anything else is left alone.
    fn anaphora(&self, clause: &str, subject: Option<&SummaryNode>) -> String {
        let Some(subject) = subject else {
            return clause.to_string();
        };
        if self.node(clause).is_some() {
            return clause.to_string();
        }
        let lower = clause.to_lowercase();
        let Some(slot) = slot_word(&lower) else {
            return clause.to_string();
        };
        if !starts_with_any(&lower, &["the ", "its ", "and the ", "and its "]) {
            return clause.to_string();
        }
        let tail = lower
            .trim_start_matches("and ")
            .trim_start_matches("the ")
            .trim_start_matches("its ");
        let tail = tail
            .split_once(" to ")
            .map(|(_, rest)| rest.to_string())
            .unwrap_or_else(|| tail.to_string());
        format!("set the {slot} of {} to {}", subject.name, tail)
    }

    /// One clause → zero or more commands. Ordered: the first rule that matches
    /// wins, so the table reads top-down like the grammar does.
    fn clause(&mut self, clause: &str) -> Result<Vec<Value>, AiError> {
        let lower = clause.to_lowercase();
        let numbers = numbers_in(clause);

        // variables: "let base = 40"
        if let Some((name, value)) = parse_assignment(clause) {
            return Ok(vec![
                json!({"type": "SetVariable", "name": name, "value": value}),
            ]);
        }

        // delete / remove
        if starts_with_any(&lower, &["delete ", "remove ", "get rid of "]) {
            if lower.contains("variable") {
                let name = words(clause)
                    .into_iter()
                    .find(|word| word.starts_with('$'))
                    .map(|word| word.trim_start_matches('$').to_string())
                    .ok_or_else(|| self.unrecognized(clause, "which variable?"))?;
                return Ok(vec![json!({"type": "RemoveVariable", "name": name})]);
            }
            let node = self
                .node(clause)
                .ok_or_else(|| self.unrecognized(clause, "delete needs a shape that exists"))?;
            return Ok(vec![json!({"type": "DeleteNode", "id": node.id})]);
        }

        // create: "add a rectangle named card at 20 30 size 80x60".
        //
        // Checked *before* the slot rules because a creation names slots too
        // ("... at 10 10 radius 5"), and "which shape's slot?" is the wrong
        // question when the shape does not exist yet. A create verb without a
        // kind word ("add a fillet to card") falls through to the rules below.
        if starts_with_any(&lower, &["add ", "create ", "draw ", "new "]) && names_a_kind(&lower) {
            return Ok(vec![self.create(clause)?]);
        }

        // operations: fillet / offset / mirror
        if let Some(kind) = modifier(&lower, &numbers) {
            let node = self
                .node(clause)
                .ok_or_else(|| self.unrecognized(clause, "which shape?"))?;
            return Ok(vec![json!({
                "type": "ApplyOperation",
                "id": "$new:op:modifier",
                "kind": serde_json::to_value(&kind).unwrap_or(Value::Null),
                "inputs": [node.id],
            })]);
        }

        // paint: "fill card with #2266ee" / "colour card red"
        if let Some(property) = paint_property(&lower) {
            let node = self
                .node(clause)
                .ok_or_else(|| self.unrecognized(clause, "which shape should be painted?"))?;
            let colour = parse_colour(clause).ok_or_else(|| {
                self.unrecognized(clause, "name a colour (red) or a hex value (#ff0000)")
            })?;
            return Ok(vec![json!({
                "type": "SetParameter",
                "node_id": node.id,
                "property": property,
                "value": {"Color": {"Literal": colour}},
            })]);
        }

        // motion: "animate the height of card to 100 with a spring"
        if lower.contains("spring") {
            let node = self
                .node(clause)
                .ok_or_else(|| self.unrecognized(clause, "which shape should animate?"))?;
            let property = slot_word(&lower).unwrap_or("height");
            let target = numbers
                .last()
                .copied()
                .ok_or_else(|| self.unrecognized(clause, "the spring needs a target number"))?;
            let from = node
                .slots
                .iter()
                .find(|slot| slot.property == property)
                .and_then(|slot| slot.value)
                .unwrap_or(0.0);
            let binding = MotionBinding::Spring {
                target: Box::new(Parameter::Literal(target)),
                stiffness: 120.0,
                damping: 14.0,
                from,
                at: 0.0,
            };
            return Ok(vec![json!({
                "type": "BindMotion",
                "node_id": node.id,
                "property": property,
                "binding": serde_json::to_value(&binding).unwrap_or(Value::Null),
            })]);
        }

        // move: "move dot to 100 50"
        if starts_with_any(&lower, &["move ", "place ", "put "]) {
            let node = self
                .node(clause)
                .ok_or_else(|| self.unrecognized(clause, "which shape should move?"))?;
            if numbers.len() < 2 {
                return Err(self.unrecognized(clause, "move needs two coordinates"));
            }
            let (x_slot, y_slot) = position_slots(&node.kind);
            let (x, y) = (numbers[numbers.len() - 2], numbers[numbers.len() - 1]);
            return Ok(vec![
                json!({"type": "SetParameter", "node_id": node.id, "property": x_slot,
                       "value": {"Float": {"Literal": x}}}),
                json!({"type": "SetParameter", "node_id": node.id, "property": y_slot,
                       "value": {"Float": {"Literal": y}}}),
            ]);
        }

        // set a slot: "set the width of card to 120" / "make the width of card twice $base"
        if let Some((property, tail)) = self.slot_of(clause) {
            let node = self
                .node(clause)
                .ok_or_else(|| self.unrecognized(clause, "which shape's slot?"))?;
            if property.starts_with("style.") {
                if let Some(colour) = parse_colour(&tail) {
                    return Ok(vec![json!({
                        "type": "SetParameter", "node_id": node.id, "property": property,
                        "value": {"Color": {"Literal": colour}},
                    })]);
                }
            }
            let mut commands = Vec::new();
            let parameter = self.parameter(&tail, &mut commands)?;
            commands.push(json!({
                "type": "SetParameter",
                "node_id": node.id,
                "property": property,
                "value": {"Float": parameter},
            }));
            return Ok(commands);
        }

        Err(self.unrecognized(clause, "name a shape and a slot, or ask for a new shape"))
    }

    /// A pair request — one that addresses two shapes, so it must be matched
    /// before the clause splitter sees " and ".
    fn pair(&mut self, text: &str) -> Option<Vec<Value>> {
        let lower = text.to_lowercase();
        let refs = self.references(text);
        if refs.len() < 2 {
            return None;
        }
        let (a, b) = (refs[0].clone(), refs[1].clone());

        for (word, op) in [
            ("union", BooleanOp::Union),
            ("intersect", BooleanOp::Intersect),
            ("exclude", BooleanOp::Exclude),
            ("difference", BooleanOp::Exclude),
        ] {
            if lower.contains(word) {
                return Some(vec![json!({
                    "type": "ApplyOperation",
                    "id": "$new:op:boolean",
                    "kind": {"type": "boolean", "op": bool_tag(op)},
                    "inputs": [a.id, b.id],
                })]);
            }
        }

        if lower.contains("subtract") || lower.contains("cut ") {
            // "subtract dot from card" → card \ dot; "cut card with dot" → card \ dot.
            let (subject, clip) = match lower.find(" from ") {
                Some(at) => match (
                    lower.find(&a.name.to_lowercase()),
                    lower.find(&b.name.to_lowercase()),
                ) {
                    (Some(pa), Some(pb)) if pa > at && pb < at => (a.clone(), b.clone()),
                    (Some(pa), Some(pb)) if pb > at && pa < at => (b.clone(), a.clone()),
                    _ => (b.clone(), a.clone()),
                },
                None => (a.clone(), b.clone()),
            };
            return Some(vec![json!({
                "type": "ApplyOperation",
                "id": "$new:op:boolean",
                "kind": {"type": "boolean", "op": "subtract"},
                "inputs": [subject.id, clip.id],
            })]);
        }

        if lower.contains("align")
            || lower.contains("same ")
            || lower.contains("below")
            || lower.contains("above")
            || lower.contains("level with")
        {
            let numbers = numbers_in(text);
            let (kind, value) = if lower.contains("vertical") || lower.contains("column") {
                ("vertical", None)
            } else if lower.contains("horizontal") || lower.contains("row") {
                ("horizontal", None)
            } else if lower.contains("same width")
                || lower.contains("same height")
                || lower.contains("same size")
            {
                ("equal_length", None)
            } else if lower.contains("below") || lower.contains("above") {
                // "keep card 20 below dot" is a signed separation on y.
                let signed = numbers.first().copied().map(|value| {
                    if lower.contains("below") {
                        -value
                    } else {
                        value
                    }
                });
                ("distance", signed)
            } else {
                return None;
            };
            let (x_slot, y_slot) = position_slots(&a.kind);
            let (pa, pb) = match kind {
                "equal_length" => (magnitude_slot(&a.kind), magnitude_slot(&b.kind)),
                "horizontal" => (y_slot, y_slot),
                "distance" => (y_slot, y_slot),
                _ => (x_slot, x_slot),
            };
            return Some(vec![json!({
                "type": "AddConstraint",
                "constraint": {
                    "id": "$new:constraint",
                    "kind": kind,
                    "targets": [
                        {"node_id": a.id, "property": pa},
                        {"node_id": b.id, "property": pb},
                    ],
                    "strength": "required",
                    "value": value,
                },
            })]);
        }
        None
    }

    /// "create an arc named swoosh at 0 0 radius 50 from 0 to 90"
    fn create(&mut self, clause: &str) -> Result<Value, AiError> {
        let lower = clause.to_lowercase();
        let numbers = numbers_in(clause);
        let name = parse_name(clause).unwrap_or_else(|| default_name(&lower));
        let (x, y) = (
            numbers.first().copied().unwrap_or(0.0),
            numbers.get(1).copied().unwrap_or(0.0),
        );
        let kind = if lower.contains("rectangle") || lower.contains("rect") || lower.contains("box")
        {
            let width = numbers.get(2).copied().unwrap_or(100.0);
            let height = numbers.get(3).copied().unwrap_or(60.0);
            let radius = if lower.contains("round") || lower.contains("corner") {
                numbers.get(4).copied().unwrap_or(0.0)
            } else {
                0.0
            };
            rect(x, y, width, height, radius)
        } else if lower.contains("circle") || lower.contains("disc") {
            NodeKind::circle(x, y, numbers.get(2).copied().unwrap_or(50.0))
        } else if lower.contains("arc") || lower.contains("half") {
            NodeKind::arc(
                x,
                y,
                numbers.get(2).copied().unwrap_or(50.0),
                numbers.get(3).copied().unwrap_or(0.0),
                numbers.get(4).copied().unwrap_or(std::f64::consts::PI),
            )
        } else {
            return Err(self.unrecognized(
                clause,
                "the planner can add a rectangle, a circle or an arc",
            ));
        };
        Ok(json!({
            "type": "CreateNode",
            "id": "$new:shape",
            "name": name,
            "kind": serde_json::to_value(&kind).unwrap_or(Value::Null),
        }))
    }

    /// The parameter a slot is being set to: a number, a variable, or an
    /// expression (which becomes a `DefineExpression` first).
    fn parameter(&mut self, tail: &str, commands: &mut Vec<Value>) -> Result<Value, AiError> {
        // Arithmetic first: "twice $base" is an expression, not a variable
        // reference, and which one it is decides whether the document keeps a
        // formula or a copy of a number.
        if let Some(source) = expression_source(tail) {
            commands.push(json!({
                "type": "DefineExpression",
                "id": "$new:expr:value",
                "source": source,
            }));
            return Ok(json!({"Expression": "$new:expr:value"}));
        }
        if let Some(name) = bare_variable(tail) {
            return Ok(json!({"Variable": name}));
        }
        if let Some(value) = numbers_in(tail).first().copied() {
            return Ok(json!({"Literal": value}));
        }
        Err(self.unrecognized(
            tail,
            "that slot needs a number, a $variable or an expression",
        ))
    }

    /// The first node the clause names.
    fn node(&self, clause: &str) -> Option<SummaryNode> {
        self.references(clause).into_iter().next()
    }

    /// All nodes the clause names, in the order they appear.
    fn references(&self, clause: &str) -> Vec<SummaryNode> {
        let mut found: Vec<SummaryNode> = Vec::new();
        let consider = |needle: &str, found: &mut Vec<SummaryNode>| {
            if let Ok(node) = self.summary.find_node(needle) {
                if !found.iter().any(|other| other.id == node.id) {
                    found.push(node.clone());
                }
            }
        };
        for word in words(clause) {
            if word.len() >= 2 {
                consider(&word, &mut found);
            }
        }
        // A quoted name may contain spaces ("the 'blue dot'").
        for quoted in clause.split('\'').skip(1).step_by(2) {
            consider(quoted, &mut found);
        }
        found
    }

    /// `("width", rest)` for "set the width of card to 120".
    fn slot_of(&self, clause: &str) -> Option<(String, String)> {
        let lower = clause.to_lowercase();
        let slot = slot_word(&lower)?;
        let tail = lower
            .split_once(" to ")
            .map(|(_, rest)| rest.to_string())
            .or_else(|| lower.split_once(" of ").map(|(_, rest)| rest.to_string()))
            .unwrap_or(lower);
        Some((slot.to_string(), tail))
    }

    fn unrecognized(&self, clause: &str, detail: &str) -> AiError {
        AiError::Unrecognized {
            prompt: clause.to_string(),
            detail: detail.to_string(),
        }
    }
}

// ── small parsers ──────────────────────────────────────────────────────

/// Words that begin a *new* instruction after " and " — so "set the width of
/// card to 12 and set the height to 20" is two clauses, while "union card and
/// dot" (handled by `pair` first) and "add a rectangle and a circle" are not.
const CONTINUATIONS: [&str; 20] = [
    "the", "its", "set", "make", "move", "fill", "colour", "color", "delete", "remove", "add",
    "create", "draw", "animate", "spring", "keep", "round", "offset", "mirror", "fillet",
];

/// Split an instruction into clauses: " and " (when a clause follows), " and
/// then ", " then ", ";" and ". ".
fn split_clauses(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let lower = text.to_lowercase();
    let chars: Vec<char> = text.chars().collect();
    let mut index = 0;
    while index < chars.len() {
        let rest = lower.get(index..).unwrap_or("");
        let skip = if rest.starts_with(" and then ") {
            Some(" and then ".len())
        } else if rest.starts_with(" then ") {
            Some(" then ".len())
        } else if rest.starts_with(". ") {
            Some(". ".len())
        } else if rest.starts_with(';') {
            Some(1)
        } else if let Some(after) = rest.strip_prefix(" and ") {
            let first = after
                .trim_start()
                .split(|ch: char| !ch.is_alphabetic())
                .next()
                .unwrap_or("")
                .to_lowercase();
            if CONTINUATIONS.contains(&first.as_str()) {
                Some(" and ".len())
            } else {
                None
            }
        } else {
            None
        };
        if let Some(skip) = skip {
            index += skip;
            out.push(std::mem::take(&mut current));
            continue;
        }
        if let Some(ch) = chars.get(index) {
            current.push(*ch);
        }
        index += 1;
    }
    out.push(current);
    out
}

/// Every number in the text, in order. `80x60` counts as two.
fn numbers_in(text: &str) -> Vec<f64> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut current = String::new();
    let mut index = 0;
    while index < chars.len() {
        let ch = chars[index];
        let previous = if index == 0 {
            None
        } else {
            Some(chars[index - 1])
        };
        // A leading `-` is a sign when it starts a token and a digit follows:
        // "move card to -40 20", "offset card by -4".
        let sign = ch == '-'
            && current.is_empty()
            && previous
                .map(|prev| !prev.is_alphanumeric() && prev != '.')
                .unwrap_or(true)
            && chars
                .get(index + 1)
                .map(|next| next.is_ascii_digit() || *next == '.')
                .unwrap_or(false);
        if ch.is_ascii_digit() || ch == '.' || sign {
            current.push(ch);
        } else if !current.is_empty() {
            if let Ok(value) = current.parse::<f64>() {
                out.push(value);
            }
            current.clear();
        }
        index += 1;
    }
    if !current.is_empty() {
        if let Ok(value) = current.parse::<f64>() {
            out.push(value);
        }
    }
    out
}

/// Word tokens, with `$` kept so a variable survives the split.
fn words(text: &str) -> Vec<String> {
    text.split(|ch: char| !(ch.is_alphanumeric() || ch == '_' || ch == '$' || ch == '-'))
        .filter(|word| !word.is_empty())
        .map(str::to_string)
        .collect()
}

/// Does the clause name a node kind the planner can create?
fn names_a_kind(lower: &str) -> bool {
    ["rectangle", "rect", "box", "circle", "disc", "arc", "half"]
        .iter()
        .any(|needle| lower.contains(needle))
}

fn starts_with_any(lower: &str, prefixes: &[&str]) -> bool {
    prefixes.iter().any(|prefix| lower.starts_with(prefix))
}

/// `let base = 40` / `add a variable base = 40`.
fn parse_assignment(clause: &str) -> Option<(String, f64)> {
    let (name, value) = clause.split_once('=')?;
    let lower = name.to_lowercase();
    if !(lower.contains("variable") || lower.trim_start().starts_with("let ")) {
        return None;
    }
    let cleaned = words(name).into_iter().rfind(|word| {
        !word.starts_with('$')
            && !["let", "add", "a", "variable", "set"].contains(&word.to_lowercase().as_str())
    })?;
    let value = numbers_in(value).first().copied()?;
    Some((cleaned, value))
}

/// A single `$name` token, and no arithmetic around it.
fn bare_variable(text: &str) -> Option<String> {
    let dollars: Vec<String> = words(text)
        .into_iter()
        .filter(|word| word.starts_with('$'))
        .map(|word| word.trim_start_matches('$').to_string())
        .collect();
    if dollars.len() != 1 {
        return None;
    }
    if is_compound(text) {
        return None;
    }
    dollars.into_iter().next()
}

/// Rebuild an expression source from prose: `twice $base` → `2 * $base`,
/// `$base * 2` → `$base * 2`, `half $base` → `0.5 * $base`.
fn expression_source(text: &str) -> Option<String> {
    let lower = text.to_lowercase();
    let dollar = lower.find('$')?;
    let name: String = lower[dollar + 1..]
        .chars()
        .take_while(|ch| ch.is_alphanumeric() || *ch == '_')
        .collect();
    if name.is_empty() {
        return None;
    }
    let before = lower[..dollar].to_string();
    let after = lower[dollar + 1 + name.len()..].to_string();
    let mut source = format!("${name}");

    let head = if before.contains("twice") || before.contains("double") {
        Some("2".to_string())
    } else if before.contains("half") {
        Some("0.5".to_string())
    } else if before.contains("triple") || before.contains("three times") {
        Some("3".to_string())
    } else if before.contains('*') || before.contains("times") {
        numbers_in(&before).last().copied().map(trim)
    } else {
        None
    };
    if let Some(head) = head {
        source = format!("{head} * {source}");
    }
    if let Some(tail) = tail_operator(&after) {
        source.push_str(&tail);
    }
    if !is_compound(&source) {
        return None;
    }
    Some(source)
}

/// `" times 2"` → `" * 2"`, `" divided by 2"` → `" / 2"`, `" * 2"` → `" * 2"`.
fn tail_operator(after: &str) -> Option<String> {
    let (operator, rest): (&str, &str) =
        if let Some(rest) = after.split_once(" times ").map(|(_, rest)| rest) {
            ("*", rest)
        } else if let Some(rest) = after.split_once(" multiplied by ").map(|(_, rest)| rest) {
            ("*", rest)
        } else if let Some(rest) = after.split_once(" divided by ").map(|(_, rest)| rest) {
            ("/", rest)
        } else if let Some(rest) = after.split_once(" plus ").map(|(_, rest)| rest) {
            ("+", rest)
        } else if let Some(rest) = after.split_once(" minus ").map(|(_, rest)| rest) {
            ("-", rest)
        } else {
            let rest = after.trim_start();
            match rest.chars().next() {
                Some('*') => ("*", &rest[1..]),
                Some('/') => ("/", &rest[1..]),
                Some('+') => ("+", &rest[1..]),
                Some('-') => ("-", &rest[1..]),
                _ => return None,
            }
        };
    let value = numbers_in(rest).first().copied()?;
    Some(format!(" {operator} {}", trim(value)))
}

fn trim(value: f64) -> String {
    vectra_core::trim_number(value)
}

/// Whether a source is arithmetic rather than a single token.
fn is_compound(source: &str) -> bool {
    let lower = source.to_lowercase();
    [
        "*", "+", "/", "(", "sin(", "cos(", "abs(", "clamp(", "min(", "max(",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

/// The slot word a clause names, if any.
fn slot_word(lower: &str) -> Option<&'static str> {
    for (needles, slot) in [
        (
            &[
                "corner radius",
                "corner_radius",
                "round the corner",
                "rounding",
                "rounded",
                "corner",
            ][..],
            "corner_radius",
        ),
        (&["stroke width", "border width"][..], "style.stroke_width"),
        (&["stroke"][..], "style.stroke"),
        (&["fill", "colour", "color"][..], "style.fill"),
        (&["opacity", "alpha"][..], "style.opacity"),
        (&["width", "wide"][..], "width"),
        (&["height", "tall"][..], "height"),
        (&["radius"][..], "radius"),
        (&["position"][..], "x"),
        (&[" x "][..], "x"),
        (&[" y "][..], "y"),
    ] {
        if needles.iter().any(|needle| lower.contains(needle)) {
            return Some(slot);
        }
    }
    None
}

fn paint_property(lower: &str) -> Option<&'static str> {
    if lower.starts_with("fill ") || lower.starts_with("colour ") || lower.starts_with("color ") {
        return Some("style.fill");
    }
    None
}

fn parse_colour(text: &str) -> Option<vectra_core::Color> {
    let lower = text.to_lowercase();
    if let Some(hash) = lower.find('#') {
        // Skip the `#` itself — the digits start one character after it.
        let hex: String = lower[hash + 1..]
            .chars()
            .take_while(|ch| ch.is_ascii_hexdigit())
            .collect();
        if hex.len() >= 6 {
            let component =
                |range: std::ops::Range<usize>| u8::from_str_radix(&hex[range], 16).unwrap_or(0);
            return Some(vectra_core::Color::rgba(
                component(0..2),
                component(2..4),
                component(4..6),
                255,
            ));
        }
    }
    let (r, g, b) = match () {
        _ if lower.contains("red") => (255, 0, 0),
        _ if lower.contains("green") => (0, 128, 0),
        _ if lower.contains("blue") => (0, 0, 255),
        _ if lower.contains("black") => (0, 0, 0),
        _ if lower.contains("white") => (255, 255, 255),
        _ if lower.contains("grey") || lower.contains("gray") => (128, 128, 128),
        _ if lower.contains("orange") => (255, 165, 0),
        _ if lower.contains("yellow") => (255, 255, 0),
        _ if lower.contains("purple") || lower.contains("violet") => (128, 0, 128),
        _ if lower.contains("teal") || lower.contains("cyan") => (0, 128, 128),
        _ => return None,
    };
    Some(vectra_core::Color::rgba(r, g, b, 255))
}

fn parse_name(clause: &str) -> Option<String> {
    let lower = clause.to_lowercase();
    for marker in [" named ", " called ", " name "] {
        if let Some(index) = lower.find(marker) {
            let rest = &clause[index + marker.len()..];
            let word = rest
                .trim_start_matches(['\'', '"'])
                .split(|ch: char| !(ch.is_alphanumeric() || ch == '_' || ch == '-'))
                .next()
                .unwrap_or("");
            if !word.is_empty() {
                return Some(word.to_string());
            }
        }
    }
    None
}

fn default_name(lower: &str) -> String {
    if lower.contains("rectangle") || lower.contains("rect") || lower.contains("box") {
        "rect".to_string()
    } else if lower.contains("circle") {
        "circle".to_string()
    } else {
        "arc".to_string()
    }
}

/// A rectangle with a corner radius — `NodeKind::rectangle` is 4-ary, and the
/// radius is the one slot a prompt usually supplies separately.
fn rect(x: f64, y: f64, width: f64, height: f64, radius: f64) -> NodeKind {
    match NodeKind::rectangle(x, y, width, height) {
        NodeKind::Rectangle {
            x,
            y,
            width,
            height,
            ..
        } => NodeKind::Rectangle {
            x,
            y,
            width,
            height,
            corner_radius: Parameter::Literal(radius),
        },
        other => other,
    }
}

/// The canonical position slots for a node, by kind tag.
fn position_slots(kind: &str) -> (&'static str, &'static str) {
    match kind {
        "Rectangle" => ("x", "y"),
        _ => ("cx", "cy"),
    }
}

fn magnitude_slot(kind: &str) -> &'static str {
    match kind {
        "Circle" | "Arc" => "radius",
        _ => "width",
    }
}

fn bool_tag(op: BooleanOp) -> &'static str {
    match op {
        BooleanOp::Union => "union",
        BooleanOp::Subtract => "subtract",
        BooleanOp::Intersect => "intersect",
        BooleanOp::Exclude => "exclude",
    }
}

/// `fillet card by 8` / `offset card by -4` / `mirror card horizontally at 0`.
fn modifier(lower: &str, numbers: &[f64]) -> Option<OperationKind> {
    if lower.contains("fillet") || lower.contains("round its") {
        let radius = numbers.last().copied().unwrap_or(4.0);
        return Some(OperationKind::Fillet {
            radius: Parameter::Literal(radius),
        });
    }
    if lower.contains("offset") {
        let distance = numbers.last().copied().unwrap_or(4.0);
        return Some(OperationKind::Offset {
            distance: Parameter::Literal(distance),
        });
    }
    if lower.contains("mirror") || lower.contains("reflect") {
        let at = numbers.first().copied().unwrap_or(0.0);
        let axis = if lower.contains("horizontal") {
            MirrorAxis::Horizontal {
                at: Parameter::Literal(at),
            }
        } else {
            MirrorAxis::Vertical {
                at: Parameter::Literal(at),
            }
        };
        return Some(OperationKind::Mirror { axis });
    }
    None
}

/// `unknown property 'radius' for node <uuid> (Rectangle)` → the pieces.
fn parse_unknown_property(error: &str) -> Option<(String, String, String)> {
    let property = between(error, "unknown property '", "'")?;
    let node_id = between(error, "for node ", " ")?;
    let kind = between(error, "(", ")")?;
    Some((property, node_id, kind))
}

fn parse_node_not_found(error: &str) -> Option<String> {
    error
        .split("node not found: ")
        .nth(1)
        .map(|rest| rest.split_whitespace().next().unwrap_or("").to_string())
        .filter(|id| !id.is_empty())
}

fn between(text: &str, start: &str, end: &str) -> Option<String> {
    let from = text.find(start)? + start.len();
    let rest = &text[from..];
    let to = rest.find(end)?;
    Some(rest[..to].to_string())
}

/// The slot on `kind` the model most likely meant by `property`.
///
/// The engine's message names the kind; the summary knows the kind's slots, so
/// this is a lookup plus a small alias table — never a guess about geometry.
fn closest_slot(kind: &str, property: &str) -> Option<String> {
    let candidates: &[&str] = match kind {
        "Rectangle" => &["x", "y", "width", "height", "corner_radius"],
        "Circle" => &["cx", "cy", "radius"],
        "Arc" => &["cx", "cy", "radius", "start_angle", "end_angle"],
        "Path" => &["start"],
        _ => &[],
    };
    if candidates.contains(&property) {
        return Some(property.to_string());
    }
    for (from, to) in [
        ("radius", "corner_radius"),
        ("corner_radius", "radius"),
        ("r", "radius"),
        ("corner", "corner_radius"),
        ("rounding", "corner_radius"),
        ("color", "style.fill"),
        ("colour", "style.fill"),
    ] {
        if from == property && (candidates.contains(&to) || to.starts_with("style.")) {
            return Some(to.to_string());
        }
    }
    None
}

// ── Task 10.6 macro helpers ─────────────────────────────────────────────────

/// The numeric value of a slot, whether the summary recorded it or only wrote
/// its source (`"24"`).
fn slot_number(node: &SummaryNode, property: &str) -> Option<f64> {
    let slot = node.slots.iter().find(|slot| slot.property == property)?;
    if let Some(value) = slot.value {
        return Some(value);
    }
    let source = slot.source.trim();
    if let Some((_, tail)) = source.rsplit_once('*') {
        if let Ok(value) = tail.trim().parse::<f64>() {
            return Some(value);
        }
    }
    source.parse::<f64>().ok()
}

/// The raw source of a slot (`"#2266ee"` for a colour, `"$base"` for a
/// variable) — the summary's own spelling, never a re-render.
fn slot_source(node: &SummaryNode, property: &str) -> Option<String> {
    node.slots
        .iter()
        .find(|slot| slot.property == property)
        .map(|slot| slot.source.trim().to_string())
}

/// Whether the node's position lives in `x/y` or `cx/cy`.
fn axis_slots(node: &SummaryNode) -> (&'static str, &'static str) {
    if node.kind == "Circle" || node.kind == "Arc" {
        ("cx", "cy")
    } else {
        ("x", "y")
    }
}

/// `(slot, current value)` for the node's first position axis.
fn position_slot(node: &SummaryNode) -> Option<(&'static str, f64)> {
    let (x_slot, y_slot) = axis_slots(node);
    if let Some(value) = slot_number(node, x_slot) {
        return Some((x_slot, value));
    }
    slot_number(node, y_slot).map(|value| (y_slot, value))
}

/// The magnitude a variation is laid out by: width, height or diameter.
fn size_slot(node: &SummaryNode) -> Option<f64> {
    for property in ["width", "height", "radius"] {
        if let Some(value) = slot_number(node, property) {
            return Some(if property == "radius" {
                value * 2.0
            } else {
                value
            });
        }
    }
    None
}

/// Every position slot, snapped to whole numbers (only the ones the node has).
fn snap_slots(node: &SummaryNode) -> Vec<(&'static str, f64)> {
    let (x_slot, y_slot) = axis_slots(node);
    let mut out = Vec::new();
    for slot in [x_slot, y_slot] {
        if let Some(value) = slot_number(node, slot) {
            out.push((slot, value.round()));
        }
    }
    out
}

/// `count` colours from the same family as `base`: even hue steps around the
/// wheel at the same saturation and lightness (RULE 2's "Create 4 color
/// variations" asks for a palette, not four random colours).
fn hue_palette(base: vectra_core::Color, count: usize) -> Vec<vectra_core::Color> {
    let (h, s, l) = rgb_to_hsl(
        base.r as f64 / 255.0,
        base.g as f64 / 255.0,
        base.b as f64 / 255.0,
    );
    (0..count)
        .map(|index| {
            let hue = (h + index as f64 * 360.0 / count.max(1) as f64).rem_euclid(360.0);
            let (r, g, b) = hsl_to_rgb(hue, s, l);
            vectra_core::Color::rgba(
                (r * 255.0).round() as u8,
                (g * 255.0).round() as u8,
                (b * 255.0).round() as u8,
                base.a,
            )
        })
        .collect()
}

fn rgb_to_hsl(r: f64, g: f64, b: f64) -> (f64, f64, f64) {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let lightness = (max + min) / 2.0;
    if (max - min).abs() < 1e-12 {
        return (0.0, 0.0, lightness);
    }
    let delta = max - min;
    let saturation = if lightness > 0.5 {
        delta / (2.0 - max - min)
    } else {
        delta / (max + min)
    };
    let hue = if max == r {
        60.0 * (((g - b) / delta) % 6.0)
    } else if max == g {
        60.0 * ((b - r) / delta + 2.0)
    } else {
        60.0 * ((r - g) / delta + 4.0)
    };
    (hue.rem_euclid(360.0), saturation, lightness)
}

fn hsl_to_rgb(hue: f64, saturation: f64, lightness: f64) -> (f64, f64, f64) {
    let c = (1.0 - (2.0 * lightness - 1.0).abs()) * saturation;
    let h = hue / 60.0;
    let x = c * (1.0 - (h % 2.0 - 1.0).abs());
    let (r, g, b) = match h as i32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = lightness - c / 2.0;
    (r + m, g + m, b + m)
}

/// The **Icon Studio** body of a macro: one instance per size, each on its own
/// square artboard, each scaled through the master's `size` prop (RULE 3).
///
/// Laid out left to right with the same 16px gutter `icon_set_plan` uses, so the
/// macro and the panel button produce identical documents.
fn icon_set_json(master_name: &str, sizes: &[f64]) -> Vec<Value> {
    let mut commands = Vec::new();
    let mut cursor = 0.0f64;
    for size in sizes {
        let board = format!("$new:board:{}", trim(*size));
        let layer = format!("$new:layer:{}", trim(*size));
        let instance = format!("$new:instance:{}", trim(*size));
        let label = format!("{} {}", master_name, trim(*size));
        commands.push(json!({
            "type": "CreateArtboard",
            "id": board,
            "name": label,
            "x": cursor,
            "y": 0.0,
            "width": size,
            "height": size,
            "background": {"r": 255, "g": 255, "b": 255, "a": 255},
        }));
        commands.push(json!({
            "type": "CreateLayer",
            "id": layer,
            "name": label,
            "index": Value::Null,
            "artboard": board,
        }));
        commands.push(json!({
            "type": "InstantiateComponent",
            "id": instance,
            "master": "$new:component",
            "name": format!("{}px", trim(*size)),
            "index": Value::Null,
        }));
        commands.push(json!({
            "type": "SetComponentProp",
            "target": instance,
            "prop": "size",
            "value": {"Float": {"Literal": size}},
        }));
        cursor += size + 16.0;
    }
    commands
}
