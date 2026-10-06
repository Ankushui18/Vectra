//! The parametric heart of VECTRA (MES §3, §6).
//!
//! Every numeric (and stylistic) value flows through [`Parameter<T>`].
//! The engine never asks *where* a value comes from — it only calls
//! [`crate::eval::Resolvable::resolve`]. Geometry, logic, and motion are
//! therefore the same editable system, not three subsystems bolted together.
//!
//! ```text
//! ┌──────────┐  ┌──────────┐  ┌────────────┐  ┌──────────┐  ┌─────────────┐
//! │ Literal  │  │ Variable │  │ Expression │  │ Animated │  │ Procedural /│
//! │ (static) │  │ (global  │  │ (formula   │  │ (motion  │  │ Interaction │
//! │          │  │  scalar) │  │  graph)    │  │  graph)  │  │ (live input)│
//! └────┬─────┘  └────┬─────┘  └─────┬──────┘  └────┬─────┘  └──────┬──────┘
//!      └─────────────┴──────────────┴──────────────┴──────────────┘
//!                              │
//!                    Resolvable::resolve(ctx)
//!                              │
//!                              ▼
//!                        concrete T
//! ```

use crate::geom::{Color, Point2};
use crate::ids::{ExpressionId, NodeId, PortId, TrackId, VariableId};
use serde::{Deserialize, Serialize};

/// Address of a procedural-graph output port (MES §11).
///
/// The graph itself lives in `vectra-procedural`; core only carries the address.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeOutputId {
    pub node: NodeId,
    pub port: PortId,
}

impl NodeOutputId {
    pub fn new(node: NodeId, port: impl Into<PortId>) -> Self {
        Self {
            node,
            port: port.into(),
        }
    }
}

/// Live-input binding (pointer, viewport, scroll…). Resolved by an
/// [`crate::eval::InteractionProvider`] at evaluation time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum InputBinding {
    PointerX,
    PointerY,
    PointerDown,
    ScrollX,
    ScrollY,
    ViewportWidth,
    ViewportHeight,
}

/// Motion source for an animated parameter (MES §12).
///
/// > Motion is just another `Parameter` resolver.
///
/// The dynamics themselves live in `vectra-motion`; core carries the binding
/// so `motion → core` stays acyclic. Recursive payloads are boxed because
/// `Parameter<f64>` contains `MotionBinding` contains `Parameter<f64>`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum MotionBinding {
    /// Sample a keyframe track at `ctx.time`.
    ///
    /// `track_id` names a [`crate::document::MotionTrack`] in the document;
    /// `property` selects one of that track's named channels, so a single track
    /// can drive several slots (`x` and `y`, say) with one edit.
    KeyframeTrack { track_id: TrackId, property: String },
    /// Critically-damped (or under/over-damped) spring toward `target`.
    ///
    /// # The anchor (`from`, `at`) — Task 6.0
    ///
    /// A spring is a **pure function of time**: it is at `from` at time `at` (and
    /// stays there for `t < at`), then eases toward `target` with no velocity of
    /// its own to remember. That is what makes scrubbing, replay, undo and
    /// headless tests work: the same `t` yields the same value no matter how the
    /// clock got there.
    ///
    /// The cost is that the *starting point* has to be written down somewhere,
    /// because [`MotionEvaluator`](crate::eval::MotionEvaluator) receives only
    /// the binding and the context — there is no node-id keyed store to hide it
    /// in. Putting the anchor in the document is also what makes re-anchoring
    /// (a drag commits a value; the spring resumes from it) an ordinary,
    /// **undoable** edit rather than invisible session state.
    Spring {
        target: Box<Parameter<f64>>,
        stiffness: f64,
        damping: f64,
        /// Where the spring started, in the slot's units.
        from: f64,
        /// Engine time at which `from` held (seconds). `t < at` ⇒ `from`.
        at: f64,
    },
    /// Branch on a named UI/state-machine flag.
    ///
    /// The flag is an *input*, not document state: it arrives from the host
    /// (`set_state`) and is deliberately not undoable — a hover is not an edit.
    /// Transitions are smoothed by wrapping the branch in a spring, whose anchor
    /// the evaluator re-derives from the flag's own flip time.
    StateDriven {
        state: String,
        true_value: Box<Parameter<f64>>,
        false_value: Box<Parameter<f64>>,
    },
}

impl MotionBinding {
    /// Tag used in diagnostics and the inspector (`"spring"`, `"state"`, `"track"`).
    pub fn tag(&self) -> &'static str {
        match self {
            Self::KeyframeTrack { .. } => "track",
            Self::Spring { .. } => "spring",
            Self::StateDriven { .. } => "state",
        }
    }

    /// A spring at rest at `from` from time `at`, easing toward `target`.
    pub fn spring(
        target: Parameter<f64>,
        stiffness: f64,
        damping: f64,
        from: f64,
        at: f64,
    ) -> Self {
        Self::Spring {
            target: Box::new(target),
            stiffness,
            damping,
            from,
            at,
        }
    }

    /// A step driven by a named flag.
    pub fn state(
        state: impl Into<String>,
        true_value: Parameter<f64>,
        false_value: Parameter<f64>,
    ) -> Self {
        Self::StateDriven {
            state: state.into(),
            true_value: Box::new(true_value),
            false_value: Box::new(false_value),
        }
    }

    /// A keyframe track sampled at the engine clock.
    pub fn track(track_id: impl Into<TrackId>, property: impl Into<String>) -> Self {
        Self::KeyframeTrack {
            track_id: track_id.into(),
            property: property.into(),
        }
    }

    /// The flags this binding reads, in declaration order (used for dirty
    /// propagation when a flag flips).
    pub fn state_flags(&self) -> Vec<&str> {
        match self {
            Self::StateDriven { state, .. } => vec![state.as_str()],
            Self::Spring { target, .. } => target.motion_state_flags(),
            Self::KeyframeTrack { .. } => Vec::new(),
        }
    }

    /// The track this binding samples, if any.
    pub fn track_id(&self) -> Option<&TrackId> {
        match self {
            Self::KeyframeTrack { track_id, .. } => Some(track_id),
            Self::Spring { target, .. } => target.motion_track_id(),
            Self::StateDriven { .. } => None,
        }
    }

    /// Every parameter nested inside this binding, however deep.
    ///
    /// This is what makes dependency derivation *correct* for motion: the graph
    /// must see that `Spring { target: $radius }` depends on `$radius` as well as
    /// on the clock (see `vectra-dependency::dependency_targets`).
    pub fn inner_parameters(&self) -> Vec<&Parameter<f64>> {
        let mut out = Vec::new();
        self.collect_inner(&mut out);
        out
    }

    fn collect_inner<'a>(&'a self, out: &mut Vec<&'a Parameter<f64>>) {
        match self {
            Self::KeyframeTrack { .. } => {}
            Self::Spring { target, .. } => {
                out.push(target);
                target.collect_nested_motion(out);
            }
            Self::StateDriven {
                true_value,
                false_value,
                ..
            } => {
                out.push(true_value);
                out.push(false_value);
                true_value.collect_nested_motion(out);
                false_value.collect_nested_motion(out);
            }
        }
    }
}

/// A source-agnostic parameter. `T` is the resolved value type
/// (`f64`, [`Point2`], [`Color`], …).
///
/// All variants are [`Clone`] + serializable so commands, undo entries, and
/// WASM messages can carry them by value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Parameter<T> {
    /// A static, literal value.
    Literal(T),
    /// A reference to a global scalar variable by name.
    Variable(VariableId),
    /// A reference to a compiled expression by ID.
    Expression(ExpressionId),
    /// A motion-graph binding (timeline, spring, state machine).
    Animated(MotionBinding),
    /// The output of a procedural node-graph port.
    Procedural(NodeOutputId),
    /// A live input binding (pointer, viewport, …).
    Interaction(InputBinding),
}

impl<T> Parameter<T> {
    pub fn literal(value: T) -> Self {
        Self::Literal(value)
    }

    pub fn variable(name: impl Into<VariableId>) -> Self {
        Self::Variable(name.into())
    }

    pub fn expression(id: ExpressionId) -> Self {
        Self::Expression(id)
    }

    pub fn is_literal(&self) -> bool {
        matches!(self, Self::Literal(_))
    }

    /// Short source tag for diagnostics (`"literal"`, `"variable"`, …).
    pub fn source_tag(&self) -> &'static str {
        match self {
            Self::Literal(_) => "literal",
            Self::Variable(_) => "variable",
            Self::Expression(_) => "expression",
            Self::Animated(_) => "animated",
            Self::Procedural(_) => "procedural",
            Self::Interaction(_) => "interaction",
        }
    }

    /// The motion binding behind this parameter, if it is animated.
    pub fn motion(&self) -> Option<&MotionBinding> {
        match self {
            Self::Animated(binding) => Some(binding),
            _ => None,
        }
    }

    /// Flags read by this parameter's motion binding, or nothing.
    pub fn motion_state_flags(&self) -> Vec<&str> {
        self.motion().map(|m| m.state_flags()).unwrap_or_default()
    }

    /// The track this parameter samples, if it is animated by a track.
    pub fn motion_track_id(&self) -> Option<&TrackId> {
        self.motion().and_then(|m| m.track_id())
    }

    /// Continue a walk *through* this parameter into a nested binding (motion is
    /// recursive: a spring's target may itself be a spring chasing a flag).
    /// The caller has already recorded `self`; this records what lies inside it.
    pub fn collect_nested_motion<'a>(&'a self, out: &mut Vec<&'a Parameter<f64>>) {
        if let Some(binding) = self.motion() {
            binding.collect_inner(out);
        }
    }

    /// Map the literal payload, preserving non-literal sources.
    ///
    /// Note: only `Literal` values are mapped; every other source is cloned
    /// unchanged (for `f64` payloads inside [`MotionBinding`] this is exact
    /// because bindings are only valid on `Parameter<f64>` in Phase 1).
    pub fn map_literal<U>(self, f: impl FnOnce(T) -> U) -> Parameter<U>
    where
        T: Clone,
    {
        match self {
            Parameter::Literal(v) => Parameter::Literal(f(v)),
            Parameter::Variable(v) => Parameter::Variable(v),
            Parameter::Expression(e) => Parameter::Expression(e),
            Parameter::Animated(m) => Parameter::Animated(m),
            Parameter::Procedural(o) => Parameter::Procedural(o),
            Parameter::Interaction(i) => Parameter::Interaction(i),
        }
    }
}

/// Type-erased parameter payload for the [`crate::command::Command::SetParameter`]
/// command, which addresses properties by string path (`"width"`, `"start"`,
/// `"style.fill"`, …) and must carry a correctly-typed value over the WASM/JSON boundary.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ParamValue {
    Float(Parameter<f64>),
    Point(Parameter<Point2>),
    Color(Parameter<Color>),
}

impl ParamValue {
    /// Static type tag used in [`crate::error::VectraError::PropertyTypeMismatch`].
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Float(_) => "float",
            Self::Point(_) => "point",
            Self::Color(_) => "color",
        }
    }

    pub fn float_literal(v: f64) -> Self {
        Self::Float(Parameter::Literal(v))
    }

    pub fn point_literal(x: f64, y: f64) -> Self {
        Self::Point(Parameter::Literal(Point2::new(x, y)))
    }

    pub fn color_literal(c: Color) -> Self {
        Self::Color(Parameter::Literal(c))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_tags_are_stable() {
        assert_eq!(Parameter::<f64>::literal(1.0).source_tag(), "literal");
        assert_eq!(Parameter::<f64>::variable("w").source_tag(), "variable");
    }

    #[test]
    fn param_value_kinds() {
        assert_eq!(ParamValue::float_literal(1.0).kind(), "float");
        assert_eq!(ParamValue::point_literal(1.0, 2.0).kind(), "point");
    }

    #[test]
    fn parameter_json_roundtrip() {
        let p = Parameter::<f64>::variable("base");
        let json = serde_json::to_string(&p).unwrap();
        let back: Parameter<f64> = serde_json::from_str(&json).unwrap();
        assert_eq!(p, back);
    }
}
