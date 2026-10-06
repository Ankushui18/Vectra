//! `vectra-expression` — deterministic formula language (Task 2.1, MES §7).
//!
//! Compiles `$base * 2 + 10` or `sin($time) * 50` into flat stack-machine
//! bytecode, then evaluates it in a tight, allocation-free loop. Implements
//! [`vectra_core::ExpressionEvaluator`], replacing core's previously-unwired
//! hook — `Parameter::Expression` now resolves for real.
//!
//! Pipeline (never interpret the AST per-frame):
//!
//! ```text
//! source ──[lexer: logos]──▶ tokens ──[parser: chumsky pratt]──▶ ExprNode
//!     ──[resolve: names+arity]──▶ typed AST ──[compiler: fold+emit]──▶
//!     CompiledExpression { code, slots } ──[engine: stack VM]──▶ f64
//! ```
//!
//! Scalar-only, side-effect free, total: parse failures are [`ParseError`],
//! runtime failures are [`vectra_core::ResolveError`], and nothing panics.

pub mod ast;
pub mod compiler;
pub mod engine;
pub mod error;
pub mod lexer;
pub mod parser;

pub use ast::{BinOp, ExprNode, FunctionName};
pub use compiler::{compile, fold_constants, CompiledExpression, Instr};
pub use engine::{ExpressionEngine, RegistrySync, TIME_VARIABLE};
pub use error::ParseError;
pub use lexer::{lex, Token};
pub use parser::parse;
