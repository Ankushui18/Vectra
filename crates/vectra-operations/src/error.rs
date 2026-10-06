//! Typed failures for the operations pass (Task 4.0).
//!
//! Everything here is *local*: an operation that cannot be computed reports one
//! of these, becomes a diagnostic, and contributes no geometry. None of them
//! unwind a pass, and none of them are reachable by panicking.

use thiserror::Error;
use vectra_core::{NodeId, OperationId};

#[derive(Debug, Error, PartialEq, Eq)]
pub enum OperationError {
    /// The operation id is not in the registry (removed between the pass being
    /// scheduled and running).
    #[error("operation not found: {0}")]
    MissingOperation(OperationId),

    /// One of the operation's sources has no evaluated geometry in the scene —
    /// deleted, or skipped by the evaluator (e.g. an unresolvable parameter).
    #[error("operation {operation} reads node {input}, which has no evaluated geometry")]
    MissingInput {
        operation: OperationId,
        input: NodeId,
    },

    /// The source evaluates to something that encloses no area (an open sliver,
    /// a zero-size shape). There is no region to operate on.
    #[error("operation {operation} reads node {input}, which encloses no area")]
    EmptyInput {
        operation: OperationId,
        input: NodeId,
    },

    /// A scalar operand (radius, distance, axis) could not be resolved or is not
    /// finite.
    #[error("operation {operation} could not resolve its operand: {message}")]
    UnresolvableOperand {
        operation: OperationId,
        message: String,
    },

    /// A scalar operand resolved to a non-finite value.
    #[error("operation operand is not finite: {which}")]
    NonFiniteOperand { which: &'static str },

    /// The operand list does not match the operation's arity. The command
    /// validates this before storing the node, so reaching it means the
    /// registry was mutated out of band.
    #[error("operation takes {expected} input(s), got {got}")]
    Arity { expected: usize, got: usize },
}
