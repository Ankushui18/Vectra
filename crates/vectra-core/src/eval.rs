//! Incremental evaluation context and resolver traits (MES §3, §6).
//!
//! `core` defines the *interfaces*; the heavy evaluators are implemented by
//! their owner crates and injected here:
//!
//! | Trait | Implemented by | Resolves |
//! |---|---|---|
//! | [`ExpressionEvaluator`] | `vectra-expression` | `Parameter::Expression` → `f64` |
//! | [`MotionEvaluator`] | `vectra-motion` | `Parameter::Animated` → `f64` |
//! | [`ProceduralEvaluator`] | `vectra-procedural` | `Parameter::Procedural` → `f64` / [`Point2`] |
//! | [`InteractionProvider`] | host app / `vectra-wasm` | `Parameter::Interaction` → live input |
//!
//! This keeps every dependency arrow pointing *at* core, never away from it.

use crate::error::ResolveError;
use crate::geom::{Color, Point2};
use crate::ids::{ExpressionId, VariableId};
use crate::param::{InputBinding, MotionBinding, NodeOutputId, Parameter};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Which evaluation path produced the current [`EvaluatedScene`].
///
/// Reported by the engine (Task 2.2, `vectra-dependency::IncrementalScene`)
/// and carried on [`crate::command::EngineEvent::Dirty`] so the remote control
/// can *see* that a change cost one node, not the document.
///
/// [`EvaluatedScene`]: https://docs.rs/vectra-geometry
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EvalMode {
    /// Every node in the document was evaluated.
    Full,
    /// Only the dirty ids were re-evaluated; the rest of the scene was kept.
    Incremental,
}

impl EvalMode {
    pub fn tag(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::Incremental => "incremental",
        }
    }
}

/// Evaluates compiled expressions to scalar values.
pub trait ExpressionEvaluator: Send + Sync {
    fn evaluate(&self, id: ExpressionId, ctx: &EvaluationContext) -> Result<f64, ResolveError>;
}

/// Evaluates motion bindings (timelines, springs, state machines) at `ctx.time`.
pub trait MotionEvaluator: Send + Sync {
    fn evaluate(
        &self,
        binding: &MotionBinding,
        ctx: &EvaluationContext,
    ) -> Result<f64, ResolveError>;
}

/// Evaluates procedural-graph outputs.
///
/// **The implementation reads a published table; it does not evaluate.** A
/// procedural node needs its registry, its upstream values and the evaluated
/// scene to compute anything, and [`EvaluationContext`] deliberately carries
/// none of those (it is core's context, shared by every resolver). So the
/// engine owner runs the procedural *pass* first and the evaluator answers from
/// what that pass published — which keeps resolution pure, and keeps a slot's
/// value a function of the document and the clock, exactly like a motion sample.
pub trait ProceduralEvaluator: Send + Sync {
    fn evaluate_float(
        &self,
        output: &NodeOutputId,
        ctx: &EvaluationContext,
    ) -> Result<f64, ResolveError>;
    fn evaluate_point(
        &self,
        output: &NodeOutputId,
        ctx: &EvaluationContext,
    ) -> Result<Point2, ResolveError>;
    /// The colour door (Task 7.0): a typed `Color` port read by a style slot.
    /// Before this existed, `Parameter::Procedural` in a colour slot was a
    /// permanent [`ResolveError::MissingProceduralEvaluator`].
    fn evaluate_color(
        &self,
        output: &NodeOutputId,
        ctx: &EvaluationContext,
    ) -> Result<Color, ResolveError>;
}

/// Supplies the **font bytes** a text node's family resolves to (Task 11.0).
///
/// The interface is deliberately bytes, not glyphs: `core` knows that a text
/// node names a family and that shaping needs a face, and nothing more. Shaping,
/// outlining and layout are `vectra-geometry`'s business (`rustybuzz` +
/// `ttf-parser`), exactly as path building is — so the font *library* lives
/// there and this trait is the one word core needs to say about it.
///
/// The provider is optional, and an unknown family is not an error: the
/// evaluator falls back to the bundled face and reports a diagnostic, so a
/// document whose fonts are missing still draws its words.
pub trait FontProvider: Send + Sync {
    /// The raw bytes (TTF/OTF) of `family`, case-insensitively, or `None` if
    /// this provider does not have it.
    fn face(&self, family: &str) -> Option<&[u8]>;

    /// The families this provider offers, for the UI's font picker. The default
    /// (no families) is correct for a provider that only ever answers lookups.
    fn families(&self) -> Vec<String> {
        Vec::new()
    }

    /// A **stable identity** for the bytes `face(family)` returns, when the
    /// provider can name them (Task 11.0 performance).
    ///
    /// `vectra-geometry` memoizes shaped runs — shaping and glyph outlining are
    /// the expensive half of text, and an `offset` slider must not pay for them
    /// every frame. A memo has to know *which face* it is remembering, and the
    /// family name cannot answer that: a host that re-registers a face under a
    /// name it used before would be served stale outlines. So a provider that
    /// owns its bytes supplies an id here (the [`FontLibrary`] hashes each face
    /// once, at registration); one that does not gets the bytes hashed on the
    /// way in — correct, and merely slower.
    ///
    /// The contract is only "same family, same bytes ⟹ same id": two different
    /// faces must never share an id, and re-registering a face must change it.
    ///
    /// [`FontLibrary`]: https://docs.rs/vectra-geometry
    fn face_id(&self, _family: &str) -> Option<u64> {
        None
    }
}

/// Provides live input values (pointer, viewport, scroll…).
pub trait InteractionProvider: Send + Sync {
    fn evaluate_float(
        &self,
        binding: &InputBinding,
        ctx: &EvaluationContext,
    ) -> Result<f64, ResolveError>;
    fn evaluate_point(
        &self,
        binding: &InputBinding,
        ctx: &EvaluationContext,
    ) -> Result<Point2, ResolveError>;
}

/// Everything a [`Resolvable`] needs to produce a concrete value.
///
/// `variables` + `time` are always present (borrowed from the [`crate::document::Document`]).
/// The four evaluator hooks are `None` until their owner crate wires in —
/// resolving a source with no evaluator is a typed [`ResolveError`], never a panic,
/// except for the documented static-preview fallbacks below.
pub struct EvaluationContext<'a> {
    pub variables: &'a HashMap<VariableId, f64>,
    pub time: f64,
    pub expression: Option<&'a dyn ExpressionEvaluator>,
    pub motion: Option<&'a dyn MotionEvaluator>,
    pub procedural: Option<&'a dyn ProceduralEvaluator>,
    pub interaction: Option<&'a dyn InteractionProvider>,
    /// The font bytes text nodes shape against (Task 11.0). `None` is a normal
    /// state — the evaluator then uses the bundled face for every family.
    pub fonts: Option<&'a dyn FontProvider>,
}

impl<'a> EvaluationContext<'a> {
    pub fn new(variables: &'a HashMap<VariableId, f64>, time: f64) -> Self {
        Self {
            variables,
            time,
            expression: None,
            motion: None,
            procedural: None,
            interaction: None,
            fonts: None,
        }
    }

    pub fn with_expression(mut self, ev: &'a dyn ExpressionEvaluator) -> Self {
        self.expression = Some(ev);
        self
    }

    pub fn with_motion(mut self, ev: &'a dyn MotionEvaluator) -> Self {
        self.motion = Some(ev);
        self
    }

    pub fn with_procedural(mut self, ev: &'a dyn ProceduralEvaluator) -> Self {
        self.procedural = Some(ev);
        self
    }

    pub fn with_interaction(mut self, provider: &'a dyn InteractionProvider) -> Self {
        self.interaction = Some(provider);
        self
    }

    /// Install a font library (Task 11.0). Pure plumbing, like the other four
    /// builders: the context carries the provider, the evaluator asks it.
    pub fn with_fonts(mut self, fonts: &'a dyn FontProvider) -> Self {
        self.fonts = Some(fonts);
        self
    }

    pub fn variable(&self, name: &str) -> Result<f64, ResolveError> {
        self.variables
            .get(name)
            .copied()
            .ok_or_else(|| ResolveError::UndefinedVariable(name.to_string()))
    }
}

/// The only question the engine ever asks a parameter.
///
/// Every numeric value in the document — geometry, style, constraint weight,
/// procedural input, motion target — flows through this trait.
pub trait Resolvable<T> {
    fn resolve(&self, ctx: &EvaluationContext) -> Result<T, ResolveError>;
}

impl Resolvable<f64> for Parameter<f64> {
    fn resolve(&self, ctx: &EvaluationContext) -> Result<f64, ResolveError> {
        match self {
            Parameter::Literal(v) => Ok(*v),
            Parameter::Variable(name) => ctx.variable(name),
            Parameter::Expression(id) => ctx
                .expression
                .ok_or(ResolveError::MissingExpressionEvaluator(*id))
                .and_then(|ev| ev.evaluate(*id, ctx)),
            Parameter::Animated(binding) => {
                if let Some(ev) = ctx.motion {
                    return ev.evaluate(binding, ctx);
                }
                // Static-preview fallback so documents stay renderable with no
                // motion crate wired (e.g. unit tests, headless export):
                // springs preview at their target, state branches preview the
                // false arm, timelines have no meaningful static value.
                match binding {
                    MotionBinding::Spring { target, .. } => target.resolve(ctx),
                    MotionBinding::StateDriven { false_value, .. } => false_value.resolve(ctx),
                    MotionBinding::KeyframeTrack { .. } => {
                        Err(ResolveError::MissingMotionEvaluator)
                    }
                }
            }
            Parameter::Procedural(output) => ctx
                .procedural
                .ok_or(ResolveError::MissingProceduralEvaluator)
                .and_then(|ev| ev.evaluate_float(output, ctx)),
            Parameter::Interaction(binding) => ctx
                .interaction
                .ok_or(ResolveError::MissingInteractionProvider)
                .and_then(|p| p.evaluate_float(binding, ctx)),
        }
    }
}

impl Resolvable<Point2> for Parameter<Point2> {
    fn resolve(&self, ctx: &EvaluationContext) -> Result<Point2, ResolveError> {
        match self {
            Parameter::Literal(v) => Ok(*v),
            // Phase 1: variables and expressions are scalar-only. A point must
            // be bound component-wise at the property level (e.g. `x`, `y`)
            // or produced by the procedural graph.
            Parameter::Variable(name) => Err(ResolveError::VariableTypeMismatch(name.clone())),
            Parameter::Expression(_) => Err(ResolveError::ExpressionTypeMismatch),
            Parameter::Animated(_) => Err(ResolveError::UnsupportedAnimatedTarget),
            Parameter::Procedural(output) => ctx
                .procedural
                .ok_or(ResolveError::MissingProceduralEvaluator)
                .and_then(|ev| ev.evaluate_point(output, ctx)),
            Parameter::Interaction(binding) => ctx
                .interaction
                .ok_or(ResolveError::MissingInteractionProvider)
                .and_then(|p| p.evaluate_point(binding, ctx)),
        }
    }
}

impl Resolvable<Color> for Parameter<Color> {
    fn resolve(&self, ctx: &EvaluationContext) -> Result<Color, ResolveError> {
        match self {
            Parameter::Literal(v) => Ok(*v),
            Parameter::Procedural(output) => ctx
                .procedural
                .ok_or(ResolveError::MissingProceduralEvaluator)
                .and_then(|ev| ev.evaluate_color(output, ctx)),
            _ => Err(ResolveError::UnsupportedColorSource),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::new_expression_id;

    fn ctx_for(vars: &HashMap<VariableId, f64>) -> EvaluationContext<'_> {
        EvaluationContext::new(vars, 0.0)
    }

    #[test]
    fn literal_resolves() {
        let vars = HashMap::new();
        let ctx = ctx_for(&vars);
        assert_eq!(Parameter::Literal(3.5).resolve(&ctx).unwrap(), 3.5);
    }

    #[test]
    fn variable_resolves_and_missing_errors() {
        let mut vars = HashMap::new();
        vars.insert("base".to_string(), 42.0);
        let ctx = ctx_for(&vars);
        assert_eq!(
            Parameter::<f64>::variable("base").resolve(&ctx).unwrap(),
            42.0
        );
        assert!(matches!(
            Parameter::<f64>::variable("nope").resolve(&ctx),
            Err(ResolveError::UndefinedVariable(_))
        ));
    }

    #[test]
    fn expression_without_evaluator_errors_typed() {
        let vars = HashMap::new();
        let ctx = ctx_for(&vars);
        let id = new_expression_id();
        assert!(matches!(
            Parameter::<f64>::expression(id).resolve(&ctx),
            Err(ResolveError::MissingExpressionEvaluator(e)) if e == id
        ));
    }

    #[test]
    fn spring_falls_back_to_target_without_motion_crate() {
        let vars = HashMap::new();
        let ctx = ctx_for(&vars);
        let p: Parameter<f64> = Parameter::Animated(MotionBinding::spring(
            Parameter::Literal(7.0),
            170.0,
            26.0,
            1.0,
            0.0,
        ));
        assert_eq!(p.resolve(&ctx).unwrap(), 7.0);
    }

    #[test]
    fn point_variable_is_typed_error_not_silent() {
        let mut vars = HashMap::new();
        vars.insert("p".to_string(), 1.0);
        let ctx = ctx_for(&vars);
        assert!(matches!(
            Parameter::<Point2>::variable("p").resolve(&ctx),
            Err(ResolveError::VariableTypeMismatch(_))
        ));
    }
}
