//! The **validation and self-correction loop** (Task 9.0, RULE 3).
//!
//! > "Before any AI-generated command is applied, it must pass through the
//! > existing `Engine::dispatch` validation (cycle detection, type checking,
//! > port matching). If the engine returns a `VectraError`, the AI layer must
//! > capture this error and feed it back to the LLM as a 'correction prompt'
//! > (ReAct pattern), allowing it to self-correct up to 2 times before failing
//! > gracefully to the user."
//!
//! # The loop
//!
//! ```text
//!   summary = host.summary()                     ← fresh every attempt (RULE 2)
//!   plan    = planner.plan(prompt, summary, last_correction)
//!   apply   = for each command: host.apply(cmd)   ← the engine validates (RULE 3)
//!             on rejection: roll the applied prefix back, remember the error
//!   ── success ──▶ ExecutionReport{ attempts, plan, events, dirty, corrections }
//!   ── failure ──▶ corrections.push(...) ; attempt += 1  (≤ MAX_ATTEMPTS)
//!   ── exhausted ─▶ AiError::MaxRetriesExceeded{ attempts, last_error, plan }
//! ```
//!
//! # Two decisions worth stating
//!
//! **A failed attempt leaves nothing behind.** The commands that *did* apply are
//! rolled back through the host's own undo before the retry runs. Without that, a
//! plan that failed halfway would sit in the document while the model is told
//! "nothing was applied" — and the retry would be reasoning about a document
//! that no longer matches the summary it is shown. The rollback uses ordinary
//! undo entries, so the history stays coherent (a retry that then succeeds leaves
//! exactly one visible action).
//!
//! **The engine's words are what the model sees.** [`Correction::error`] is the
//! `VectraError`'s own `Display` text, not a paraphrase. The AI layer adds no
//! vocabulary of its own about geometry, which is what keeps the correction
//! prompt honest: if the engine learns a new refusal, the model is told about it
//! with no change here.
//!
//! # The host
//!
//! [`CommandHost`] is the seam between this loop and an engine. `vectra-core`'s
//! own [`Engine`] implements it (used by this crate's tests and by any headless
//! caller); `vectra-wasm` implements it over the *boundary* engine, so the AI
//! path runs through exactly the dispatch a user's click does — expression
//! pre-compilation, the dependency-graph cycle gate, the procedural port gate,
//! and the scene/dirty bookkeeping that follows.

use std::collections::BTreeSet;

use serde::Serialize;
use serde_json::Value;
use vectra_core::{Command, DocumentSummary, Engine, EngineEvent, VectraError};

use crate::error::{AiError, Correction, MAX_ATTEMPTS};
use crate::planner::{Plan, PlanRequest, Planner};
use crate::schema::plan_json;

/// An engine the AI layer may apply commands to.
///
/// The three methods are the whole surface. `apply` must push **one** undoable
/// entry per command — that is what makes `rollback(applied)` exact.
pub trait CommandHost {
    /// Dispatch one command through the engine's own validation. The `Err` string
    /// is the engine's message, verbatim.
    fn apply(&mut self, command: &Command) -> Result<Vec<EngineEvent>, String>;
    /// Undo the last `steps` commands — called only for the prefix of a failed
    /// plan.
    fn rollback(&mut self, steps: usize) -> Result<(), String>;
    /// The live document as `vectra-core` summarises it (RULE 2). Called once per
    /// attempt, so a retry always sees the truth.
    fn summary(&self) -> DocumentSummary;
}

/// A `CommandHost` over the plain engine: no scene, no renderer, no boundary
/// gates. Everything a headless caller (a test, a batch job, a CLI) needs.
pub struct CoreHost {
    pub engine: Engine,
}

impl CoreHost {
    pub fn new(engine: Engine) -> Self {
        Self { engine }
    }
}

impl CommandHost for CoreHost {
    fn apply(&mut self, command: &Command) -> Result<Vec<EngineEvent>, String> {
        self.engine
            .dispatch(command.clone())
            .map_err(|e| e.to_string())
    }

    fn rollback(&mut self, steps: usize) -> Result<(), String> {
        for _ in 0..steps {
            self.engine.undo().map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    fn summary(&self) -> DocumentSummary {
        DocumentSummary::capture(self.engine.document())
    }
}

/// What the UI previews before anything runs.
#[derive(Debug, Clone, Serialize)]
pub struct Preview {
    pub prompt: String,
    /// The resolved plan, one JSON command per entry — ids already minted, so
    /// what the user reads is exactly what will be dispatched.
    pub plan: Vec<Value>,
    /// The planner's own remarks (empty on a first attempt).
    pub notes: Vec<String>,
    /// Which attempt this preview came from.
    pub attempt: usize,
}

/// What happened when a plan ran.
#[derive(Debug, Clone, Serialize)]
pub struct ExecutionReport {
    pub prompt: String,
    /// 1 = the first plan worked. `MAX_ATTEMPTS` = it took every correction.
    pub attempts: usize,
    /// The commands that were applied, as JSON.
    pub plan: Vec<Value>,
    /// The engine's events for the whole plan, in order.
    pub events: Vec<EngineEvent>,
    /// Every node id the engine marked dirty — the visible proof of what the
    /// plan cost (MES §16).
    pub dirty: Vec<String>,
    /// The corrections that were needed, oldest first.
    pub corrections: Vec<Correction>,
    /// The planner's notes across attempts.
    pub notes: Vec<String>,
    /// The document summary *after* the plan (what the next prompt would see).
    pub summary: DocumentSummary,
}

impl ExecutionReport {
    /// A one-line summary for the log: `2 command(s) in 1 attempt, 3 node(s) dirty`.
    pub fn headline(&self) -> String {
        let dirty = if self.dirty.is_empty() {
            "nothing re-evaluated".to_string()
        } else {
            format!("{} node(s) re-evaluated", self.dirty.len())
        };
        let attempts = if self.attempts == 1 {
            "first attempt".to_string()
        } else {
            format!("{} attempts", self.attempts)
        };
        format!(
            "{} command(s) applied in {attempts}, {dirty}",
            self.plan.len()
        )
    }
}

/// Generate a plan without applying it — the AI panel's *preview*.
pub fn preview<P: Planner, H: CommandHost>(
    planner: &P,
    host: &H,
    prompt: &str,
) -> Result<Preview, AiError> {
    let summary = host.summary();
    let request = PlanRequest {
        prompt,
        summary: &summary,
        correction: None,
    };
    let plan = planner.plan(&request)?;
    Ok(preview_of(prompt, &plan, 1))
}

/// Generate a plan and apply it, self-correcting up to [`MAX_ATTEMPTS`] times.
pub fn execute<P: Planner, H: CommandHost>(
    planner: &P,
    host: &mut H,
    prompt: &str,
) -> Result<ExecutionReport, AiError> {
    let mut corrections: Vec<Correction> = Vec::new();
    let mut notes: Vec<String> = Vec::new();
    let mut last: Option<(AiError, Vec<Value>)> = None;

    for attempt in 1..=MAX_ATTEMPTS {
        let summary = host.summary();
        let correction = corrections.last().cloned();
        let request = PlanRequest {
            prompt,
            summary: &summary,
            correction: correction.as_ref(),
        };

        let plan = match planner.plan(&request) {
            Ok(plan) => plan,
            Err(error) => {
                if !error.is_correctable() {
                    return Err(error);
                }
                last = Some((error.clone(), Vec::new()));
                corrections.push(Correction {
                    attempt,
                    code: error.code().to_string(),
                    error: error.to_string(),
                    command: None,
                    note: "the model was asked again with the error".to_string(),
                });
                continue;
            }
        };
        notes.extend(plan.notes.iter().cloned());
        let json = plan_json(&plan.commands);

        let mut events = Vec::new();
        let mut applied = 0usize;
        let mut failure: Option<(String, Option<Value>)> = None;
        for command in &plan.commands {
            match host.apply(command) {
                Ok(mut produced) => {
                    applied += 1;
                    events.append(&mut produced);
                }
                Err(error) => {
                    failure = Some((error, serde_json::to_value(command).ok()));
                    break;
                }
            }
        }

        match failure {
            None => {
                return Ok(ExecutionReport {
                    prompt: prompt.to_string(),
                    attempts: attempt,
                    plan: json,
                    dirty: dirty_ids(&events),
                    events,
                    corrections,
                    notes,
                    summary: host.summary(),
                })
            }
            Some((error, command)) => {
                // Nothing of a failed attempt survives (see the module docs).
                if applied > 0 {
                    host.rollback(applied)
                        .map_err(|detail| AiError::Host { detail })?;
                }
                let rejected = AiError::Rejected {
                    attempt,
                    command: command
                        .as_ref()
                        .map(describe_value)
                        .unwrap_or_else(|| "<plan>".to_string()),
                    error: error.clone(),
                };
                last = Some((rejected.clone(), json.clone()));
                corrections.push(Correction {
                    attempt,
                    code: rejected.code().to_string(),
                    error: error.clone(),
                    command,
                    // The note records the *loop's* half of the exchange (what
                    // became of the commands, and that the engine's own words
                    // were handed back); the planner's answer arrives on the
                    // next attempt and lands in `notes`.
                    note: if applied == 0 {
                        "nothing was applied; the error went back as a correction prompt".to_string()
                    } else {
                        format!(
                            "{applied} applied command(s) were rolled back; the error went back as a correction prompt"
                        )
                    },
                });
            }
        }
    }

    let (error, plan) = last.unwrap_or_else(|| {
        (
            AiError::Planner {
                detail: "the loop produced neither a plan nor an error".to_string(),
            },
            Vec::new(),
        )
    });
    Err(AiError::MaxRetriesExceeded {
        attempts: MAX_ATTEMPTS,
        last_error: error.to_string(),
        last_code: error.code().to_string(),
        prompt: prompt.to_string(),
        plan: plan.into_boxed_slice(),
        corrections: corrections.into_boxed_slice(),
    })
}

/// Apply a plan that was already generated and approved (the AI panel's
/// *Execute* for a previewed plan). No planner, so no retry: a rejection is
/// final, and it is rolled back.
pub fn execute_plan<H: CommandHost>(
    host: &mut H,
    prompt: &str,
    commands: &[Command],
) -> Result<ExecutionReport, AiError> {
    let mut events = Vec::new();
    let mut applied = 0usize;
    for command in commands {
        match host.apply(command) {
            Ok(mut produced) => {
                applied += 1;
                events.append(&mut produced);
            }
            Err(error) => {
                if applied > 0 {
                    host.rollback(applied)
                        .map_err(|detail| AiError::Host { detail })?;
                }
                return Err(AiError::Rejected {
                    attempt: 1,
                    command: describe_command(command),
                    error,
                });
            }
        }
    }
    Ok(ExecutionReport {
        prompt: prompt.to_string(),
        attempts: 1,
        plan: plan_json(commands),
        dirty: dirty_ids(&events),
        events,
        corrections: Vec::new(),
        notes: Vec::new(),
        summary: host.summary(),
    })
}

fn preview_of(prompt: &str, plan: &Plan, attempt: usize) -> Preview {
    Preview {
        prompt: prompt.to_string(),
        plan: plan_json(&plan.commands),
        notes: plan.notes.clone(),
        attempt,
    }
}

/// Every id the engine reported as dirty, in order, de-duplicated.
pub fn dirty_ids(events: &[EngineEvent]) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for event in events {
        if let EngineEvent::Dirty { ids, .. } = event {
            for id in ids {
                let id = id.to_string();
                if seen.insert(id.clone()) {
                    out.push(id);
                }
            }
        }
    }
    out
}

fn describe_command(command: &Command) -> String {
    serde_json::to_string(command).unwrap_or_else(|_| command.label())
}

fn describe_value(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "null".to_string())
}

/// A `CommandHost` that refuses everything — for callers that only want to
/// generate and preview a plan, with no engine attached yet.
pub struct DryRunHost {
    pub summary: DocumentSummary,
}

impl DryRunHost {
    pub fn new(summary: DocumentSummary) -> Self {
        Self { summary }
    }
}

impl Default for DryRunHost {
    fn default() -> Self {
        Self::new(DocumentSummary::capture(&vectra_core::Document::new()))
    }
}

impl CommandHost for DryRunHost {
    fn apply(&mut self, _command: &Command) -> Result<Vec<EngineEvent>, String> {
        Err("dry run: the AI layer is not attached to an engine".to_string())
    }

    fn rollback(&mut self, _steps: usize) -> Result<(), String> {
        Ok(())
    }

    fn summary(&self) -> DocumentSummary {
        self.summary.clone()
    }
}

/// Turn a `VectraError` into the text the model is shown. One function, so the
/// correction prompt and the report never disagree about what the engine said.
pub fn engine_message(error: &VectraError) -> String {
    error.to_string()
}
