//! Compiled-expression registry + evaluator (Task 2.1 pipeline stage 4).
//!
//! [`ExpressionEngine`] owns `ExpressionId → CompiledExpression` and
//! implements [`vectra_core::ExpressionEvaluator`], replacing core's
//! previously-unwired hook. Evaluation is a tight loop over flat [`Instr`]
//! with one `Vec` operand stack and zero `HashMap` contact after the
//! single up-front slot-materialization pass.
//!
//! ## `$time`
//! `$time` is a reserved variable name bound to
//! [`EvaluationContext::time`](vectra_core::EvaluationContext::time) —
//! the engine clock. It IS reported by dependency extraction (the Phase-4
//! graph treats the clock as a global dep: time advance invalidates `$time`
//! expressions). A user variable literally named `time` is shadowed by the
//! clock; `define_variable("time")` remains legal but unreadable from
//! expressions — document, don't forbid.

use crate::ast::FunctionName;
use crate::compiler::{compile, CompiledExpression, Instr};
use crate::error::ParseError;
use crate::parser::parse;
use std::collections::{HashMap, HashSet};
use vectra_core::{
    Document, EvaluationContext, ExpressionEvaluator, ExpressionId, ResolveError, VariableId,
};

/// Reserved variable name bound to the engine clock (see module docs).
pub const TIME_VARIABLE: &str = "time";

/// What one [`ExpressionEngine::sync_from_document`] pass did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RegistrySync {
    /// Registrations dropped because the document no longer defines them.
    pub dropped: usize,
    /// Sources (re)compiled because the document's source changed.
    pub compiled: usize,
}

impl RegistrySync {
    pub fn changed(&self) -> bool {
        self.dropped + self.compiled > 0
    }
}

/// Registry of compiled expressions. The evaluator half of
/// `vectra_core::ExpressionEvaluator`.
#[derive(Debug, Default)]
pub struct ExpressionEngine {
    compiled: HashMap<ExpressionId, CompiledExpression>,
    sources: HashMap<ExpressionId, String>,
}

impl ExpressionEngine {
    pub fn new() -> Self {
        Self::default()
    }

    /// Parse, compile, and store `src` under `id`, returning dependencies.
    /// Overwrites any previous registration (redefine).
    pub fn compile_source(
        &mut self,
        id: ExpressionId,
        src: &str,
    ) -> Result<HashSet<VariableId>, ParseError> {
        let compiled = compile(&parse(src)?, src);
        let deps = compiled.dependencies();
        self.sources.insert(id, src.to_string());
        self.compiled.insert(id, compiled);
        Ok(deps)
    }

    /// Validate a source without storing it (pre-dispatch checks).
    pub fn check_source(src: &str) -> Result<HashSet<VariableId>, ParseError> {
        Ok(compile(&parse(src)?, src).dependencies())
    }

    pub fn get(&self, id: ExpressionId) -> Option<&CompiledExpression> {
        self.compiled.get(&id)
    }

    pub fn source_of(&self, id: ExpressionId) -> Option<&str> {
        self.sources.get(&id).map(String::as_str)
    }

    pub fn contains(&self, id: ExpressionId) -> bool {
        self.compiled.contains_key(&id)
    }

    pub fn remove(&mut self, id: ExpressionId) -> Option<CompiledExpression> {
        self.sources.remove(&id);
        self.compiled.remove(&id)
    }

    /// Drop every registration where `keep(id)` is false.
    pub fn retain(&mut self, mut keep: impl FnMut(ExpressionId) -> bool) {
        self.compiled.retain(|id, _| keep(*id));
        self.sources.retain(|id, _| keep(*id));
    }

    pub fn len(&self) -> usize {
        self.compiled.len()
    }

    pub fn is_empty(&self) -> bool {
        self.compiled.is_empty()
    }

    /// Reunite the registry with the document's sources after a mutation
    /// (Task 2.1 protocol, shared with the WASM engine and the test harnesses
    /// so there is exactly one implementation of it).
    ///
    /// Drops ids the document no longer defines (remove/undo) and
    /// (re)compiles sources that changed (define/redefine/undo/redo). Total: a
    /// source that fails to compile is dropped rather than panicking — only
    /// reachable through out-of-band mutation, since commands are
    /// pre-validated at the boundary.
    pub fn sync_from_document(&mut self, doc: &Document) -> RegistrySync {
        let mut report = RegistrySync::default();
        let before = self.compiled.len();
        self.retain(|id| doc.expressions.contains_key(&id));
        report.dropped = before - self.compiled.len();
        for (id, record) in &doc.expressions {
            let stale = self.source_of(*id) != Some(record.source.as_str());
            if stale {
                if self.compile_source(*id, &record.source).is_ok() {
                    report.compiled += 1;
                } else {
                    self.remove(*id);
                }
            }
        }
        report
    }

    /// Evaluate one compiled expression. Total: the only failures are a
    /// missing variable ([`ResolveError::UndefinedVariable`]) or, for the
    /// by-id path, an unknown id. IEEE edge values (±inf/NaN) flow through
    /// as data — the geometry totality policy decides their fate.
    pub fn eval_compiled(
        compiled: &CompiledExpression,
        ctx: &EvaluationContext,
    ) -> Result<f64, ResolveError> {
        // Single materialization pass: N DISTINCT variables → N lookups.
        // Everything after this is map-free arithmetic.
        let mut slots = Vec::with_capacity(compiled.slots.len());
        for name in &compiled.slots {
            if name == TIME_VARIABLE {
                slots.push(ctx.time);
            } else {
                slots.push(ctx.variable(name)?);
            }
        }

        let mut stack: Vec<f64> = Vec::with_capacity(32);
        for instr in &compiled.code {
            match *instr {
                Instr::Push(v) => stack.push(v),
                Instr::Load(i) => stack.push(slots[i as usize]),
                Instr::Neg => {
                    let a = pop_or_nan(&mut stack);
                    stack.push(-a);
                }
                Instr::Add => {
                    let (a, b) = pop2_or_nan(&mut stack);
                    stack.push(a + b);
                }
                Instr::Sub => {
                    let (a, b) = pop2_or_nan(&mut stack);
                    stack.push(a - b);
                }
                Instr::Mul => {
                    let (a, b) = pop2_or_nan(&mut stack);
                    stack.push(a * b);
                }
                Instr::Div => {
                    let (a, b) = pop2_or_nan(&mut stack);
                    stack.push(a / b);
                }
                Instr::Sin => {
                    let a = pop_or_nan(&mut stack);
                    stack.push(a.sin());
                }
                Instr::Cos => {
                    let a = pop_or_nan(&mut stack);
                    stack.push(a.cos());
                }
                Instr::Abs => {
                    let a = pop_or_nan(&mut stack);
                    stack.push(a.abs());
                }
                Instr::Clamp => {
                    let hi = pop_or_nan(&mut stack);
                    let lo = pop_or_nan(&mut stack);
                    let x = pop_or_nan(&mut stack);
                    stack.push(FunctionName::Clamp.apply(&[x, lo, hi]));
                }
            }
        }
        debug_assert_eq!(stack.len(), 1, "compiler emits balanced code");
        Ok(stack.pop().unwrap_or(f64::NAN))
    }
}

/// Stack underflow is impossible by construction (the compiler emits
/// balanced code for a well-formed AST), but evaluation must stay total
/// even against a hand-built `CompiledExpression` — hence NaN, never panic.
#[inline]
fn pop_or_nan(stack: &mut Vec<f64>) -> f64 {
    debug_assert!(!stack.is_empty(), "bytecode stack underflow");
    stack.pop().unwrap_or(f64::NAN)
}

#[inline]
fn pop2_or_nan(stack: &mut Vec<f64>) -> (f64, f64) {
    let b = pop_or_nan(stack);
    let a = pop_or_nan(stack);
    (a, b)
}

impl ExpressionEvaluator for ExpressionEngine {
    fn evaluate(&self, id: ExpressionId, ctx: &EvaluationContext) -> Result<f64, ResolveError> {
        match self.compiled.get(&id) {
            Some(compiled) => Self::eval_compiled(compiled, ctx),
            None => Err(ResolveError::Evaluator(format!("unknown expression {id}"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn ctx_at<'a>(vars: &'a HashMap<VariableId, f64>, time: f64) -> EvaluationContext<'a> {
        EvaluationContext::new(vars, time)
    }

    fn eval_src(src: &str, vars: &HashMap<VariableId, f64>, time: f64) -> f64 {
        let compiled = compile(&parse(src).unwrap(), src);
        ExpressionEngine::eval_compiled(&compiled, &ctx_at(vars, time)).unwrap()
    }

    #[test]
    fn brief_examples_evaluate() {
        let vars: HashMap<VariableId, f64> = [("base", 21.0), ("width", 150.0)]
            .map(|(k, v)| (k.to_string(), v))
            .into();
        assert_eq!(eval_src("$base * 2 + 10", &vars, 0.0), 52.0);
        assert_eq!(eval_src("clamp($width, 0, 100)", &vars, 0.0), 100.0);
        assert_eq!(eval_src("sin($time) * 50", &vars, 0.0), 0.0);
    }

    #[test]
    fn time_variable_reads_clock() {
        let vars = HashMap::new();
        assert_eq!(eval_src("$time * 2", &vars, 21.0), 42.0);
        // ...and is reported as a dependency (clock invalidation).
        let compiled = compile(&parse("$time + 1").unwrap(), "");
        assert!(compiled.dependencies().contains("time"));
    }

    #[test]
    fn missing_variable_is_typed_not_panic() {
        let vars = HashMap::new();
        let compiled = compile(&parse("$ghost + 1").unwrap(), "");
        assert_eq!(
            ExpressionEngine::eval_compiled(&compiled, &ctx_at(&vars, 0.0)),
            Err(ResolveError::UndefinedVariable("ghost".to_string()))
        );
    }

    #[test]
    fn unknown_id_is_typed() {
        let engine = ExpressionEngine::new();
        let vars = HashMap::new();
        let id = vectra_core::new_expression_id();
        assert!(matches!(
            ExpressionEvaluator::evaluate(&engine, id, &ctx_at(&vars, 0.0)),
            Err(ResolveError::Evaluator(_))
        ));
    }

    #[test]
    fn sync_from_document_recompiles_and_drops() {
        use vectra_core::{Command, Engine};
        let mut doc = vectra_core::Document::new();
        let id = vectra_core::new_expression_id();
        doc.define_expression(id, "$a".to_string());

        let mut engine = ExpressionEngine::new();
        let first = engine.sync_from_document(&doc);
        assert_eq!(first.compiled, 1);
        assert!(engine.contains(id));

        // Unchanged source → nothing to do (idempotent).
        assert!(!engine.sync_from_document(&doc).changed());

        // Redefine → recompile; the compiled artifact follows the source.
        doc.define_expression(id, "$b * 2".to_string());
        let redefined = engine.sync_from_document(&doc);
        assert_eq!(redefined.compiled, 1);
        assert_eq!(engine.source_of(id), Some("$b * 2"));

        // Removal → dropped.
        doc.remove_expression(id).unwrap();
        assert_eq!(engine.sync_from_document(&doc).dropped, 1);
        assert!(!engine.contains(id));

        // An unparsable source (out-of-band only) is dropped, never a panic.
        let bad = vectra_core::new_expression_id();
        doc.define_expression(bad, "$a * ".to_string());
        engine.sync_from_document(&doc);
        assert!(!engine.contains(bad));

        // The engine path (dispatch → sync) is the same code.
        let mut core = Engine::new();
        let live = vectra_core::new_expression_id();
        core.dispatch(Command::DefineExpression {
            id: live,
            source: "$z + 1".to_string(),
        })
        .unwrap();
        let mut registry = ExpressionEngine::new();
        registry.sync_from_document(core.document());
        assert!(registry.contains(live));
        core.dispatch(Command::RemoveExpression { id: live })
            .unwrap();
        registry.sync_from_document(core.document());
        assert!(!registry.contains(live));
    }

    #[test]
    fn registry_round_trip_and_redefine() {
        let mut engine = ExpressionEngine::new();
        let id = vectra_core::new_expression_id();
        let deps = engine.compile_source(id, "$a * 2 + $b").unwrap();
        assert_eq!(deps, HashSet::from(["a".to_string(), "b".to_string()]));
        assert_eq!(engine.source_of(id), Some("$a * 2 + $b"));

        // Redefine overwrites.
        engine.compile_source(id, "$c").unwrap();
        assert_eq!(engine.source_of(id), Some("$c"));

        engine.remove(id);
        assert!(!engine.contains(id));
        assert_eq!(engine.source_of(id), None);
    }
}
