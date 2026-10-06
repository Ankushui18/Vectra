//! **Vectra's AI command layer** (Task 9.0).
//!
//! The semantic interface between a language model and the parametric graph. A
//! model never touches geometry: it emits commands, the engine decides.
//!
//! ```text
//!   user prompt
//!        │
//!        ▼
//!   ┌─────────┐  prompt + DocumentSummary (vectra-core, RULE 2)
//!   │ Planner │──────────────────────────────────────────────┐
//!   └────┬────┘                                              │
//!        │ reply: JSON array of commands (RULE 1)            │ correction
//!        ▼                                                   │ (RULE 3)
//!   ┌──────────────────┐                                    │
//!   │ schema::compile  │  parse → $new: ids → context check  │
//!   └────────┬─────────┘                                    │
//!            │ Vec<Command>                                  │
//!            ▼                                               │
//!   ┌──────────────────┐   Engine::dispatch (cycle gate,     │
//!   │ exec::execute    │   type check, port gate) ───────────┘
//!   └────────┬─────────┘
//!            ▼
//!     ExecutionReport { attempts, plan, events, dirty, corrections }
//! ```
//!
//! # The three rules this crate exists to keep
//!
//! * **RULE 1 — strict command emitter.** The schema *is*
//!   [`vectra_core::Command`]; a reply is a JSON array of command objects, parsed
//!   by the same `serde` impl the UI's own commands use. No shim language, no
//!   string DSL, no SVG. The single extension is the `$new:<slug>` id token
//!   ([`schema`]), which exists so a model never has to invent a UUID.
//! * **RULE 2 — context-aware prompting.** [`vectra_core::DocumentSummary`] is
//!   the only thing a planner sees, and [`prompt::system_prompt`] injects it into
//!   the instruction text. Ids, names, kinds, variables, constraints, motion and
//!   the procedural graph all come from there — never from the model's memory.
//! * **RULE 3 — deterministic validation and self-correction.** Every command
//!   goes through the engine's own `dispatch`; a typed `VectraError` becomes a
//!   [`Correction`], which becomes the next attempt's correction prompt, up to
//!   [`MAX_ATTEMPTS`]; then the loop stops with a typed
//!   [`AiError::MaxRetriesExceeded`] and *nothing applied*.
//!
//! # Using it
//!
//! ```no_run
//! use vectra_ai::exec::{self, CoreHost};
//! use vectra_ai::planner::HeuristicPlanner;
//! use vectra_core::Engine;
//!
//! let planner = HeuristicPlanner::new();
//! let mut host = CoreHost::new(Engine::new());
//!
//! // Preview: what would the model do? (nothing is applied)
//! let preview = exec::preview(&planner, &host, "add a rectangle named card at 0 0 size 80x60")?;
//! assert_eq!(preview.plan.len(), 1);
//!
//! // Execute: generate, validate, self-correct, report.
//! let report = exec::execute(&planner, &mut host, "add a rectangle named card at 0 0 size 80x60")?;
//! println!("{}", report.headline());
//! # Ok::<(), vectra_ai::AiError>(())
//! ```
//!
//! Swapping the planner for a hosted model is one line: a [`planner::ChatPlanner`]
//! wraps any `Fn(&str) -> Result<String, AiError>`, and everything else — schema,
//! context check, retry loop, report — is unchanged.

pub mod error;
pub mod exec;
pub mod planner;
pub mod prompt;
pub mod schema;

pub use error::{AiError, Correction, MAX_ATTEMPTS};
pub use exec::{
    dirty_ids, execute, execute_plan, preview, CommandHost, CoreHost, ExecutionReport, Preview,
};
pub use planner::{ChatPlanner, HeuristicPlanner, Plan, PlanRequest, Planner, ScriptedPlanner};
pub use prompt::{correction_prompt, system_prompt, SYSTEM_PROMPT};
pub use schema::{
    check_context, compile_plan, parse_reply, plan_json, render_plan, resolve_plan, NEW_ID_PREFIX,
};
