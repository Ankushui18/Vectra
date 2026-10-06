//! Typed failures of the procedural pass (MES §11, Task 7.0).

use thiserror::Error;

/// What can stop one procedural node from producing its outputs.
///
/// Every variant is **local**: the pass turns it into exactly one
/// [`vectra_geometry::Diagnostic`] naming the node, contributes no geometry for
/// it, and carries on with its siblings (the Containment Law — the same
/// discipline `OperationError` follows).
#[derive(Debug, Clone, PartialEq, Error)]
pub enum ProceduralError {
    #[error("required input port '{0}' is not wired")]
    MissingInput(String),

    #[error("operand '{0}' has no value")]
    MissingOperand(String),

    #[error("operand '{port}' carries {found}, but {expected} was required")]
    OperandType {
        port: String,
        expected: &'static str,
        found: &'static str,
    },

    #[error("operand '{0}' is not a finite number")]
    NonFinite(String),

    #[error("operand '{port}' is {value}, outside the supported range [{min}, {max}]")]
    OutOfRange {
        port: String,
        value: f64,
        min: f64,
        max: f64,
    },

    #[error("the node would produce {cells} items, past the guard rail of 1e6")]
    TooLarge { cells: usize },

    #[error("the result encloses no area (a degenerate region, or an empty input)")]
    EmptyResult,

    #[error("a node kind produced port '{0}', which it does not declare")]
    UnknownPort(String),

    #[error("operand '{port}' could not be resolved: {message}")]
    OperandUnresolved { port: String, message: String },

    #[error("source node {0} is not live geometry (deleted, or not yet evaluated)")]
    SourceUnavailable(vectra_core::NodeId),
}

impl ProceduralError {
    /// The diagnostic code this failure reports as, and whether it is the
    /// routine "still wiring it up" case. Missing inputs are by far the most
    /// common state while authoring a graph, and the UI shows them differently.
    pub fn is_missing_input(&self) -> bool {
        matches!(self, Self::MissingInput(_) | Self::MissingOperand(_))
    }
}
