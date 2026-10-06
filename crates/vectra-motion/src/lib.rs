//! `vectra-motion` — timelines, springs and state machines (MES §12).
//!
//! Motion is **not a system that runs alongside the evaluator**; it is one more
//! [`vectra_core::Parameter`] source. A slot whose value is `Animated` is
//! resolved at `ctx.time` by the [`MotionEvaluator`] implemented here, exactly
//! as a `Variable` slot is resolved from `ctx.variables` or an `Expression` slot
//! from the expression engine. Everything the rest of the engine already does —
//! dirty propagation, incremental evaluation, undo, headless tests — therefore
//! applies to motion with no special cases.
//!
//! # The one rule everything follows: `value = f(binding, t)`
//!
//! [`MotionEngine::evaluate`] is a **function of time**, not a stepper. It never
//! reads a previous frame, never integrates a delta and never writes to the
//! document. Three things fall out of that, and all three are laws with tests
//! (`tests/motion_laws.rs`):
//!
//! 1. **Purity** — the same `t` gives the same value, so scrubbing backwards,
//!    replaying, undoing, and two clients looking at the same document agree.
//! 2. **Idleness is computable** — because nothing accumulates, "when will this
//!    stop moving?" has a closed-form answer ([`MotionStatus::horizon`]), so the
//!    frame loop can stop instead of spinning (the Task 5.0 gap this closes).
//! 3. **Samples are not edits** — a value that exists only as a function of the
//!    clock cannot be undone, and must not appear in the history. Only the
//!    *binding* is document state, and only binding, unbinding, re-anchoring and
//!    track edits are undoable commands.
//!
//! # What is state, and where
//!
//! | thing | lives in | undoable? |
//! |---|---|---|
//! | the binding (target, stiffness, damping, anchor) | the slot, in the document | **yes** |
//! | keyframe tracks | [`vectra_core::Document::motion`] | **yes** |
//! | the clock | `Engine::time` | no (a session cursor) |
//! | state flags (`hover`, `active`, …) | [`MotionEngine`], set by the host | no (an *input*) |
//! | sampled values | nowhere | they are recomputed |
//!
//! The last row is the point. There is no "current value" stored anywhere: it is
//! derived, which is why nothing can drift out of sync with the clock.

pub mod spring;
pub mod track;

use std::collections::{BTreeMap, HashMap};

use thiserror::Error;
use vectra_core::{
    Document, EvaluationContext, MotionBinding, MotionEvaluator, MotionTrackRegistry, Parameter,
    Resolvable, ResolveError, TrackId,
};

pub use spring::{Spring, SpringDynamics};
pub use track::{sample_channel, TrackSample};

/// Errors raised by the motion layer itself (the evaluator reports through
/// [`ResolveError`] so it composes with the rest of parameter resolution).
#[derive(Debug, Error)]
pub enum MotionError {
    #[error("motion: {0}")]
    Other(String),
}

/// Default settle threshold, in the slot's own units.
///
/// Motion drives document-space floats (a 300-unit width, a 0.8 opacity), so the
/// threshold is absolute, not relative. `1e-3` is well below a device pixel at
/// any sane zoom, and it is what the Idle Law uses to decide that a spring has
/// arrived — the loop stops there rather than at float equality, which would
/// never happen.
pub const DEFAULT_SETTLE_EPSILON: f64 = 1e-3;

/// The motion evaluator: springs, state branches and keyframe tracks, resolved
/// at `ctx.time`.
///
/// Holds two pieces of **not-document** state, both deliberately outside the
/// undo stack:
///
/// * a synced copy of the document's [`MotionTrackRegistry`] — a cache the
///   wasm/engine layer refreshes with the same `sync_from_document` call it
///   already makes for the expression engine;
/// * the state flags (`set_state`), which are *inputs* in the same sense as a
///   pointer position: a hover is not an edit.
pub struct MotionEngine {
    tracks: MotionTrackRegistry,
    /// Flag name → value. Absent means `false`, so a binding may read a flag
    /// that has never been set (the document and the host agree on the default
    /// without either having to declare it).
    states: HashMap<String, bool>,
    settle_epsilon: f64,
}

impl Default for MotionEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl MotionEngine {
    pub fn new() -> Self {
        Self {
            tracks: MotionTrackRegistry::new(),
            states: HashMap::new(),
            settle_epsilon: DEFAULT_SETTLE_EPSILON,
        }
    }

    /// A non-zero threshold below which a spring counts as arrived.
    pub fn with_settle_epsilon(mut self, epsilon: f64) -> Self {
        self.settle_epsilon = if epsilon.is_finite() && epsilon > 0.0 {
            epsilon
        } else {
            DEFAULT_SETTLE_EPSILON
        };
        self
    }

    pub fn settle_epsilon(&self) -> f64 {
        self.settle_epsilon
    }

    pub fn tracks(&self) -> &MotionTrackRegistry {
        &self.tracks
    }

    /// Refresh the cached registry from the document. Cheap and idempotent: the
    /// clone only happens when the registry actually changed.
    pub fn sync_from_document(&mut self, doc: &Document) {
        if self.tracks != doc.motion {
            self.tracks = doc.motion.clone();
        }
    }

    // ── State flags (host inputs, never undoable) ──────────────────────

    /// Set a state flag. Returns the previous value so callers can tell a real
    /// flip (which re-anchors springs) from a redundant write (which must not).
    pub fn set_state(&mut self, name: &str, on: bool) -> Option<bool> {
        self.states.insert(name.to_string(), on)
    }

    pub fn state(&self, name: &str) -> bool {
        self.states.get(name).copied().unwrap_or(false)
    }

    pub fn clear_state(&mut self, name: &str) -> Option<bool> {
        self.states.remove(name)
    }

    /// Every flag that is currently on, sorted — the form the wire reports.
    pub fn active_states(&self) -> Vec<String> {
        let mut on: Vec<String> = self
            .states
            .iter()
            .filter(|(_, v)| **v)
            .map(|(k, _)| k.clone())
            .collect();
        on.sort();
        on
    }

    // ── The pure core ─────────────────────────────────────────────────

    /// The value of a binding at `ctx.time` — the **pure** function every law in
    /// this crate is stated against.
    ///
    /// * `Spring` — closed form from the anchor (see [`spring`]); no velocity is
    ///   carried, so the value depends only on `(target, stiffness, damping,
    ///   from, at, t)`.
    /// * `StateDriven` — a step function of the flag. The branch itself is
    ///   *not* smoothed, because smoothing needs a start time and a start value,
    ///   and those belong to a spring (which the document holds, undoably)
    ///   rather than to hidden evaluator state. A hover transition is therefore
    ///   written `Spring { target: StateDriven { hover, … } }` and the animation
    ///   layer re-anchors the spring on a *flip* ([`reanchor`]).
    /// * `KeyframeTrack` — linear interpolation between keyframes; before the
    ///   first key the first value, after the last the last value, so the
    ///   function is total over `t ∈ (−∞, ∞)` with no error branch in the middle.
    pub fn value_of(
        &self,
        binding: &MotionBinding,
        ctx: &EvaluationContext,
    ) -> Result<f64, ResolveError> {
        match binding {
            MotionBinding::Spring {
                target,
                stiffness,
                damping,
                from,
                at,
            } => {
                let target_value = target.resolve(ctx)?;
                Ok(Spring::new(*stiffness, *damping, *from, *at).value_at(target_value, ctx.time))
            }
            MotionBinding::StateDriven {
                state,
                true_value,
                false_value,
            } => {
                let arm = if self.state(state) {
                    true_value
                } else {
                    false_value
                };
                arm.resolve(ctx)
            }
            MotionBinding::KeyframeTrack { track_id, property } => {
                let track = self.tracks.get(track_id).ok_or_else(|| {
                    ResolveError::Evaluator(format!(
                        "motion track '{track_id}' is not in the document"
                    ))
                })?;
                let samples = track.channels.get(property).ok_or_else(|| {
                    ResolveError::Evaluator(format!(
                        "motion track '{track_id}' has no channel '{property}'"
                    ))
                })?;
                Ok(sample_channel(samples, ctx.time))
            }
        }
    }

    /// Is anything bound to this binding still moving at `ctx.time`?
    ///
    /// Conservative by construction (a state whose two arms are equal still
    /// counts while a *nested* spring is in flight), because the cost of being
    /// wrong once is a frame loop that never stops, while the cost of being
    /// conservative is a few idle frames.
    pub fn binding_horizon(&self, binding: &MotionBinding, ctx: &EvaluationContext) -> Option<f64> {
        match binding {
            MotionBinding::Spring {
                target,
                stiffness,
                damping,
                from,
                at,
            } => {
                let Ok(target_value) = target.resolve(ctx) else {
                    return Some(ctx.time); // unresolvable now; don't claim idleness
                };
                let spring = Spring::new(*stiffness, *damping, *from, *at);
                if spring.settled_at(target_value, ctx.time, self.settle_epsilon) {
                    // Fully at rest once the clock reaches the anchor.
                    return if ctx.time < *at { Some(*at) } else { None };
                }
                let horizon = *at + spring.settle_time(target_value, self.settle_epsilon);
                Some(horizon.max(ctx.time))
            }
            MotionBinding::StateDriven {
                true_value,
                false_value,
                ..
            } => {
                // A branch is a step; its own horizon is whatever its live arm
                // still owes the clock. `StateDriven` is deliberately stepped, so
                // an un-sprung branch contributes nothing — the horizon of a
                // *future* flip is unknowable, and this reports now, not never.
                let arm = if self.state(if let MotionBinding::StateDriven { state, .. } = binding {
                    state.as_str()
                } else {
                    unreachable!()
                }) {
                    true_value
                } else {
                    false_value
                };
                self.parameter_horizon(arm, ctx)
            }
            MotionBinding::KeyframeTrack { track_id, property } => {
                let track = self.tracks.get(track_id)?;
                let _ = property;
                if ctx.time < track.end_time() {
                    Some(track.end_time())
                } else {
                    None
                }
            }
        }
    }

    /// Deepest motion inside a parameter tree: a spring may have a state branch
    /// inside it, whose arm may hold another spring, and the frame loop has to
    /// know about all of them.
    fn parameter_horizon(&self, param: &Parameter<f64>, ctx: &EvaluationContext) -> Option<f64> {
        let mut horizon: Option<f64> = None;
        match param {
            Parameter::Animated(binding) => {
                horizon = self.binding_horizon(binding, ctx);
                for inner in binding.inner_parameters() {
                    horizon = merge_horizon(horizon, self.parameter_horizon(inner, ctx));
                }
            }
            _ => {
                for nested in {
                    let mut out = Vec::new();
                    param.collect_nested_motion(&mut out);
                    out
                } {
                    horizon = merge_horizon(horizon, self.parameter_horizon(nested, ctx));
                }
            }
        }
        horizon
    }

    /// When the whole document is at rest, and whether it is now.
    pub fn status(&self, doc: &Document, ctx: &EvaluationContext) -> MotionStatus {
        let mut horizon: Option<f64> = None;
        for node in doc.nodes.values() {
            node.for_each_float_param(|_, param| {
                horizon = merge_horizon(horizon, self.parameter_horizon(param, ctx));
            });
        }
        // A horizon in the future means "still moving"; a horizon behind the
        // clock means "nothing left to do".
        let animating = horizon.map(|h| h > ctx.time).unwrap_or(false);
        MotionStatus { animating, horizon }
    }

    /// Whether the document is still moving at `ctx.time` — the idle signal the
    /// frame loop keys off (`is_animating` on the wasm boundary).
    pub fn is_animating(&self, doc: &Document, ctx: &EvaluationContext) -> bool {
        self.status(doc, ctx).animating
    }
}

fn merge_horizon(a: Option<f64>, b: Option<f64>) -> Option<f64> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (Some(a), None) => Some(a),
        (None, b) => b,
    }
}

/// The motion status of a scene at a moment: whether anything is moving, and
/// (if so) the time at which nothing will be.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MotionStatus {
    pub animating: bool,
    /// Engine time at which every binding is at rest, if that is knowable. `None`
    /// means "already at rest for every `t ≥ now`".
    pub horizon: Option<f64>,
}

/// Re-anchor a spring at a new start, in the slot's own units.
///
/// This is the **only** way a spring learns that its target changed, and it is a
/// pure function — it makes a new binding, it does not touch the document. The
/// caller decides whether that becomes an undoable edit (`EndDrag` finalising a
/// drag) or a session write (a `hover` flip re-anchoring a spring).
///
/// Non-spring bindings and `Spring`s whose anchor is already correct are
/// returned unchanged, so callers can call this unconditionally.
pub fn reanchor(binding: &MotionBinding, value: f64, now: f64) -> MotionBinding {
    match binding {
        MotionBinding::Spring {
            target,
            stiffness,
            damping,
            from,
            at,
        } => {
            if *from == value && *at == now {
                return binding.clone();
            }
            MotionBinding::Spring {
                target: target.clone(),
                stiffness: *stiffness,
                damping: *damping,
                from: value,
                at: now,
            }
        }
        other => other.clone(),
    }
}

/// The slot's own spring, if it is one — the shape the drag and hover paths
/// re-anchor. Nested springs (a spring inside a state branch inside a spring)
/// are intentionally *not* found: the evaluator cannot re-anchor what the
/// document does not name, and inventing an anchor for an arm that is not
/// currently live would be a guess. Recorded as a limitation in the design doc.
pub fn own_spring(param: &Parameter<f64>) -> Option<&MotionBinding> {
    match param {
        Parameter::Animated(binding @ MotionBinding::Spring { .. }) => Some(binding),
        _ => None,
    }
}

/// Which flags a parameter tree reads, deduped and sorted.
pub fn state_flags_of(param: &Parameter<f64>) -> Vec<String> {
    let mut flags: Vec<String> = param
        .motion_state_flags()
        .into_iter()
        .map(|s| s.to_string())
        .collect();
    flags.sort();
    flags.dedup();
    flags
}

/// Track ids a parameter tree samples, deduped and sorted.
pub fn tracks_of(param: &Parameter<f64>) -> Vec<TrackId> {
    let mut ids: Vec<TrackId> = Vec::new();
    collect_tracks(param, &mut ids);
    ids.sort();
    ids.dedup();
    ids
}

fn collect_tracks(param: &Parameter<f64>, ids: &mut Vec<TrackId>) {
    if let Some(id) = param.motion_track_id() {
        ids.push(id.clone());
    }
    let mut inner: Vec<&Parameter<f64>> = Vec::new();
    param.collect_nested_motion(&mut inner);
    for nested in inner {
        collect_tracks(nested, ids);
    }
}

/// The track-to-slot reference count, for the "is this track still used?"
/// question the RemoveMotionTrack path and the inspector both ask.
pub fn track_usage(doc: &Document) -> BTreeMap<TrackId, usize> {
    let mut usage: BTreeMap<TrackId, usize> = BTreeMap::new();
    for node in doc.nodes.values() {
        node.for_each_float_param(|_, param| {
            for id in tracks_of(param) {
                *usage.entry(id).or_default() += 1;
            }
        });
    }
    usage
}

impl MotionEvaluator for MotionEngine {
    fn evaluate(
        &self,
        binding: &MotionBinding,
        ctx: &EvaluationContext,
    ) -> Result<f64, ResolveError> {
        self.value_of(binding, ctx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use vectra_core::{Keyframe, MotionTrack, Parameter};

    /// An empty variable map that outlives every context built from it.
    fn vars() -> &'static HashMap<String, f64> {
        static VARS: std::sync::OnceLock<HashMap<String, f64>> = std::sync::OnceLock::new();
        VARS.get_or_init(HashMap::new)
    }

    /// Evaluate one binding through the engine, with a context built *inside* the
    /// call so the engine is not borrowed across a later `set_state`.
    fn eval(
        engine: &MotionEngine,
        binding: &MotionBinding,
        time: f64,
    ) -> Result<f64, ResolveError> {
        let mut ctx = EvaluationContext::new(vars(), time);
        ctx.motion = Some(engine);
        engine.evaluate(binding, &ctx)
    }

    #[test]
    fn a_spring_previews_at_its_target_without_an_evaluator() {
        // The static-preview fallback in `vectra-core` and this engine must agree
        // on the limit: a settled spring is its target.
        let engine = MotionEngine::new();
        let binding = MotionBinding::spring(Parameter::Literal(300.0), 170.0, 26.0, 0.0, 0.0);
        let value = eval(&engine, &binding, 50.0).expect("resolves");
        assert!((value - 300.0).abs() < 1e-9, "{value}");
        // …and at the anchor it is exactly `from`, not the target.
        assert_eq!(eval(&engine, &binding, 0.0).expect("resolves"), 0.0);
    }

    #[test]
    fn a_track_samples_before_between_and_after_its_keyframes() {
        let mut engine = MotionEngine::new();
        let mut doc = Document::default();
        doc.motion
            .insert(MotionTrack {
                id: "intro".to_string(),
                name: "Intro".to_string(),
                channels: [(
                    "x".to_string(),
                    vec![Keyframe::new(1.0, 10.0), Keyframe::new(2.0, 30.0)],
                )]
                .into_iter()
                .collect(),
            })
            .expect("valid");
        engine.sync_from_document(&doc);

        let binding = MotionBinding::track("intro", "x");
        for (t, expected) in [
            (0.0, 10.0),
            (1.0, 10.0),
            (1.5, 20.0),
            (2.0, 30.0),
            (9e3, 30.0),
        ] {
            let value = eval(&engine, &binding, t).expect("resolves");
            assert!(
                (value - expected).abs() < 1e-12,
                "t={t}: {value} ≠ {expected}"
            );
        }
    }

    #[test]
    fn a_missing_track_is_an_error_not_a_zero() {
        let engine = MotionEngine::new();
        let binding = MotionBinding::track("ghost", "x");
        assert!(eval(&engine, &binding, 0.0).is_err());
    }

    #[test]
    fn a_state_branch_reads_the_host_flag() {
        let mut engine = MotionEngine::new();
        let binding =
            MotionBinding::state("hover", Parameter::Literal(2.0), Parameter::Literal(1.0));
        assert_eq!(eval(&engine, &binding, 0.0).unwrap(), 1.0, "default off");
        engine.set_state("hover", true);
        assert_eq!(eval(&engine, &binding, 0.0).unwrap(), 2.0);
        assert_eq!(engine.active_states(), vec!["hover".to_string()]);
        engine.set_state("hover", false);
        assert_eq!(eval(&engine, &binding, 0.0).unwrap(), 1.0);
        assert_eq!(engine.active_states(), Vec::<String>::new());
    }

    #[test]
    fn the_horizon_of_a_settled_spring_is_the_present() {
        let engine = MotionEngine::new();
        let doc = Document::default();
        let mut ctx = EvaluationContext::new(vars(), 10.0);
        ctx.motion = Some(&engine);
        let status = engine.status(&doc, &ctx);
        assert!(!status.animating);
        assert_eq!(
            status.horizon, None,
            "an empty document owes the clock nothing"
        );
    }

    #[test]
    fn a_bound_spring_is_reported_as_moving_until_it_arrives() {
        use vectra_core::{new_node_id, Node, NodeKind};
        let engine = MotionEngine::new();
        let mut doc = Document::default();
        let id = new_node_id();
        let mut node = Node::new(id, "rect", NodeKind::rectangle(0.0, 0.0, 10.0, 10.0));
        node.set_param(
            "width",
            vectra_core::ParamValue::Float(Parameter::Animated(MotionBinding::spring(
                Parameter::Literal(300.0),
                170.0,
                26.0,
                10.0,
                0.0,
            ))),
        )
        .expect("bind");
        doc.nodes.insert(id, node);

        for (t, moving) in [(0.0, true), (0.05, true), (0.5, true), (3.0, false)] {
            let mut ctx = EvaluationContext::new(vars(), t);
            ctx.motion = Some(&engine);
            let status = engine.status(&doc, &ctx);
            assert_eq!(status.animating, moving, "t={t}");
        }
    }
}
