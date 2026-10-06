//! Typed AST (Task 2.1 pipeline stage 2 output).
//!
//! The AST is scalar-typed by construction: there are no strings, no
//! booleans, no nulls — every node evaluates to `f64`. Function names are
//! resolved to [`FunctionName`] during parsing, so unknown functions and
//! arity violations can never reach the compiler.

use vectra_core::VariableId;

/// A fully-resolved expression tree. Scalar-only: every node is `f64`.
#[derive(Debug, Clone, PartialEq)]
pub enum ExprNode {
    Literal(f64),
    Variable(VariableId),
    Neg(Box<ExprNode>),
    Binary {
        op: BinOp,
        left: Box<ExprNode>,
        right: Box<ExprNode>,
    },
    Call {
        name: FunctionName,
        args: Vec<ExprNode>,
    },
}

/// Infix operators, with precedence owned by the parser (pratt levels).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
}

impl BinOp {
    pub fn symbol(&self) -> &'static str {
        match self {
            Self::Add => "+",
            Self::Sub => "-",
            Self::Mul => "*",
            Self::Div => "/",
        }
    }

    /// IEEE-754 application. Division by zero yields ±inf/NaN — deterministic,
    /// never a trap. Non-finite results are handled downstream by the
    /// geometry totality policy (skip + diagnostic).
    pub fn apply(&self, left: f64, right: f64) -> f64 {
        match self {
            Self::Add => left + right,
            Self::Sub => left - right,
            Self::Mul => left * right,
            Self::Div => left / right,
        }
    }
}

/// MVP function set. Extended by adding a variant + [`FunctionName::apply`]
/// arm + [`FunctionName::arity`] — the parser, compiler, and evaluator all
/// key off this enum, so nothing else changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FunctionName {
    Sin,
    Cos,
    Abs,
    Clamp,
}

impl FunctionName {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Sin => "sin",
            Self::Cos => "cos",
            Self::Abs => "abs",
            Self::Clamp => "clamp",
        }
    }

    pub fn arity(&self) -> usize {
        match self {
            Self::Sin | Self::Cos | Self::Abs => 1,
            Self::Clamp => 3,
        }
    }

    /// Total application. `args` always has [`FunctionName::arity`] elements
    /// (enforced at parse time; `debug_assert` documents the contract).
    ///
    /// `clamp` deliberately does NOT use `f64::clamp`: that panics on
    /// `min > max` or NaN bounds. The manual comparisons below are total —
    /// NaN propagates, degenerate bounds collapse deterministically — so
    /// evaluation can never panic on any input.
    pub fn apply(&self, args: &[f64]) -> f64 {
        debug_assert_eq!(args.len(), self.arity());
        match self {
            Self::Sin => args[0].sin(),
            Self::Cos => args[0].cos(),
            Self::Abs => args[0].abs(),
            Self::Clamp => {
                let (x, lo, hi) = (args[0], args[1], args[2]);
                if x < lo {
                    lo
                } else if x > hi {
                    hi
                } else {
                    x
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamp_is_total_where_std_clamp_panics() {
        // Degenerate bounds: deterministic, no panic.
        assert_eq!(FunctionName::Clamp.apply(&[3.0, 5.0, 1.0]), 5.0);
        // NaN propagates (visible to geometry diagnostics) instead of
        // silently collapsing like `f64::max/min`-based clamps.
        assert!(FunctionName::Clamp.apply(&[f64::NAN, 0.0, 1.0]).is_nan());
        // Normal behavior matches intuition.
        assert_eq!(FunctionName::Clamp.apply(&[-5.0, 0.0, 10.0]), 0.0);
        assert_eq!(FunctionName::Clamp.apply(&[7.0, 0.0, 10.0]), 7.0);
        assert_eq!(FunctionName::Clamp.apply(&[99.0, 0.0, 10.0]), 10.0);
    }

    #[test]
    fn division_is_ieee_total() {
        assert_eq!(BinOp::Div.apply(1.0, 0.0), f64::INFINITY);
        assert!(BinOp::Div.apply(0.0, 0.0).is_nan());
    }
}
