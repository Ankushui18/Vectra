//! Typed parse errors (Task 2.1 §4).
//!
//! Every failure carries a byte position into the source so hosts can
//! underline the exact offense. Runtime failures (missing variables) are NOT
//! here — they surface as [`vectra_core::ResolveError`] at evaluation time,
//! keeping parse-time and run-time concerns strictly separated.

use thiserror::Error;

/// A failure to lex, parse, or type-check an expression source.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ParseError {
    #[error("empty expression")]
    EmptyExpression,

    #[error("invalid token {text:?} at byte {position}")]
    InvalidToken { position: usize, text: String },

    #[error("unexpected {found} at byte {position}; expected {expected}")]
    UnexpectedToken {
        position: usize,
        found: String,
        expected: String,
    },

    #[error("unexpected end of input at byte {position}; expected {expected}")]
    UnexpectedEnd { position: usize, expected: String },

    #[error("unknown function '{name}' at byte {position}; expected sin, cos, abs, or clamp")]
    UnknownFunction { name: String, position: usize },

    #[error("'{name}' expects {expected} argument(s), got {got} (at byte {position})")]
    ArityMismatch {
        name: String,
        expected: usize,
        got: usize,
        position: usize,
    },
}
