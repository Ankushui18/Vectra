//! Typed failures of the AI layer (Task 9.0, RULE 3).
//!
//! Two families, and keeping them apart is the point:
//!
//! * **The model's fault** — [`AiError::InvalidJson`], [`AiError::UnknownCommand`],
//!   [`AiError::UnknownNodeId`], [`AiError::PlanConflict`],
//!   [`AiError::InvalidPlaceholder`]. Each is *correctable*: it says what was
//!   wrong with the reply, and [`crate::prompt::correction_prompt`] turns it into
//!   the sentence the model gets on the next attempt.
//! * **The engine's refusal** — [`AiError::Rejected`], which carries the
//!   `VectraError` text verbatim. The AI layer never reinterprets it; the model
//!   is shown the engine's own words.
//!
//! After `MAX_ATTEMPTS` the loop stops and returns
//! [`AiError::MaxRetriesExceeded`] — RULE 3's "fail gracefully", with the last
//! error, the plan that failed, and the whole correction history attached so the
//! user can see what the AI was trying to do rather than just that it failed.

use serde::Serialize;

/// Attempts in total: the first try plus RULE 3's "up to 2 times" of
/// self-correction.
pub const MAX_ATTEMPTS: usize = 3;

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum AiError {
    /// The reply was not JSON, or not the JSON shape the schema asks for.
    #[error("the reply is not a command plan: {detail}")]
    InvalidJson { detail: String, reply: String },

    /// A JSON object whose `type` is not a `Command` tag, or whose payload does
    /// not deserialize into one — "no hallucinated APIs", enforced.
    #[error("unknown or malformed command: {detail}")]
    UnknownCommand {
        detail: String,
        command: serde_json::Value,
    },

    /// An id that is neither in the document summary nor a `$new:` placeholder:
    /// the model referenced a node that does not exist (Context Law).
    #[error("`{id}` is not in the document summary, and `{command}` is not a `$new:` placeholder")]
    UnknownNodeId { id: String, command: String },

    /// A `$new:` token whose namespace cannot be minted where it appears.
    #[error("bad id placeholder `{token}`: {detail}")]
    InvalidPlaceholder { token: String, detail: String },

    /// The plan contradicts the summary it was given — creating a node that
    /// already exists, for instance.
    #[error("the plan contradicts the document summary: {detail}")]
    PlanConflict { detail: String },

    /// The engine refused a command (cycle, type mismatch, unknown property,
    /// port mismatch, arity…). `error` is the engine's own message.
    #[error("engine rejected `{command}`: {error}")]
    Rejected {
        attempt: usize,
        command: String,
        error: String,
    },

    /// RULE 3's ceiling: every attempt failed. Nothing was applied.
    ///
    /// `corrections` is the whole history — what the engine said each time and
    /// what the model replied — so a UI can show *what kept going wrong* rather
    /// than only that it did. The history and the last plan are boxed slices:
    /// this variant is returned from every `Result` in the crate, and the
    /// failure path should not make the success path pay for it.
    #[error("the AI could not produce a valid plan in {attempts} attempt(s): {last_error}")]
    MaxRetriesExceeded {
        attempts: usize,
        last_error: String,
        last_code: String,
        prompt: String,
        plan: Box<[serde_json::Value]>,
        corrections: Box<[Correction]>,
    },

    /// The prompt could not be turned into a plan at all, and asking again would
    /// not change that (a deterministic planner said so). Failing immediately is
    /// kinder than three identical attempts.
    #[error("cannot turn that into commands yet: {detail}")]
    Unrecognized { prompt: String, detail: String },

    /// A planner-side failure that is not a schema mistake (a transport error, a
    /// refusal). Retried like any other correctable failure.
    #[error("planner failed: {detail}")]
    Planner { detail: String },

    /// A host-side failure while applying a command (the engine took the command
    /// but the wrapper could not update its scene).
    #[error("the engine refused to apply the plan: {detail}")]
    Host { detail: String },
}

impl AiError {
    /// A stable, machine-readable tag — what the UI keys on, and what the report
    /// records. Never parse [`Display`](std::fmt::Display) for this.
    pub fn code(&self) -> &'static str {
        match self {
            Self::InvalidJson { .. } => "invalid-json",
            Self::UnknownCommand { .. } => "unknown-command",
            Self::UnknownNodeId { .. } => "unknown-node-id",
            Self::InvalidPlaceholder { .. } => "invalid-placeholder",
            Self::PlanConflict { .. } => "plan-conflict",
            Self::Rejected { .. } => "engine-rejected",
            Self::MaxRetriesExceeded { .. } => "max-retries-exceeded",
            Self::Unrecognized { .. } => "unrecognized-prompt",
            Self::Planner { .. } => "planner-failed",
            Self::Host { .. } => "host-failed",
        }
    }

    /// Whether the model can plausibly fix this by being told about it.
    ///
    /// [`AiError::Unrecognized`] is the exception that proves the rule: it is a
    /// *planner* verdict ("I do not know this phrasing"), and a second identical
    /// attempt would produce the identical verdict.
    pub fn is_correctable(&self) -> bool {
        !matches!(self, Self::Unrecognized { .. })
    }

    /// The correction history, when the failure has one.
    pub fn corrections(&self) -> &[Correction] {
        match self {
            Self::MaxRetriesExceeded { corrections, .. } => corrections,
            _ => &[],
        }
    }

    /// `{"code":…,"message":…}` — the envelope field the wasm boundary returns.
    pub fn as_json(&self) -> serde_json::Value {
        serde_json::json!({ "code": self.code(), "message": self.to_string() })
    }
}

/// The engine's refusal, as the AI layer keeps it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Correction {
    /// 1-based: the attempt that failed.
    pub attempt: usize,
    /// The typed error code (`engine-rejected`, `unknown-node-id`, …).
    pub code: String,
    /// What the engine (or the schema check) said.
    pub error: String,
    /// The command that failed, as JSON.
    pub command: Option<serde_json::Value>,
    /// What the planner did about it, in the retry's own words.
    pub note: String,
}
