//! Headless engine: document + undo stack + time (MES §14–§16).
//!
//! [`Engine`] is the native (non-WASM) owner of all mutation. `vectra-wasm`
//! wraps exactly this type and exposes [`Engine::dispatch_json`] /
//! [`Engine::snapshot_json`] / [`Engine::set_time`] across `wasm-bindgen`
//! (MES §15). The React app is a remote control: it sends [`Command`] JSON
//! and renders [`EngineEvent`]s — it never mutates the document itself.

use crate::command::{Command, CommandStack, EngineEvent};
use crate::document::Document;
use crate::error::VectraError;
use crate::eval::{EvaluationContext, Resolvable};
use crate::ids::{NodeId, VariableId};
use crate::param::{ParamValue, Parameter};
use serde::{Deserialize, Serialize};

/// JSON envelope returned by [`Engine::dispatch_json`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DispatchResult {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub events: Vec<EngineEvent>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl DispatchResult {
    pub fn ok(events: Vec<EngineEvent>) -> Self {
        Self {
            ok: true,
            events,
            error: None,
        }
    }

    pub fn err(message: impl Into<String>) -> Self {
        Self {
            ok: false,
            events: Vec::new(),
            error: Some(message.into()),
        }
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| {
            r#"{"ok":false,"error":"failed to serialize DispatchResult"}"#.to_string()
        })
    }
}

/// The headless parametric engine.
#[derive(Debug)]
pub struct Engine {
    doc: Document,
    stack: CommandStack,
    time: f64,
}

impl Engine {
    pub fn new() -> Self {
        Self {
            doc: Document::new(),
            stack: CommandStack::with_default_limit(),
            time: 0.0,
        }
    }

    pub fn with_history_limit(limit: usize) -> Self {
        Self {
            doc: Document::new(),
            stack: CommandStack::new(limit),
            time: 0.0,
        }
    }

    pub fn document(&self) -> &Document {
        &self.doc
    }

    pub fn document_mut(&mut self) -> &mut Document {
        &mut self.doc
    }

    pub fn time(&self) -> f64 {
        self.time
    }

    pub fn set_time(&mut self, t: f64) {
        self.time = t;
    }

    pub fn can_undo(&self) -> bool {
        self.stack.can_undo()
    }

    pub fn can_redo(&self) -> bool {
        self.stack.can_redo()
    }

    /// Entries on the undo stack — the exact witness for "this produced no
    /// history" (Task 6.0's Undo Isolation Law: motion samples, state flips and
    /// every other clock-derived change must leave this number untouched).
    pub fn history_depth(&self) -> usize {
        self.stack.undo_len()
    }

    /// Entries on the redo stack, for symmetric assertions.
    pub fn redo_depth(&self) -> usize {
        self.stack.redo_len()
    }

    /// The command `undo()` would apply next (its inverse), without applying
    /// it. The engine owner dry-runs this through the dependency graph before
    /// every undo (Task 2.2), so a cycle can never be *introduced* by
    /// rewinding either.
    pub fn peek_undo(&self) -> Option<&Command> {
        self.stack.peek_undo()
    }

    /// The command `redo()` would re-apply next, without applying it.
    pub fn peek_redo(&self) -> Option<&Command> {
        self.stack.peek_redo()
    }

    /// Label of the next undo entry (undo menus / logs).
    pub fn peek_undo_label(&self) -> Option<&str> {
        self.stack.peek_undo_label()
    }

    /// Execute a typed command.
    pub fn dispatch(&mut self, cmd: Command) -> Result<Vec<EngineEvent>, VectraError> {
        self.stack.execute(&mut self.doc, cmd)
    }

    /// Execute a JSON command (WASM-boundary entry point, MES §15).
    ///
    /// Never panics: transport errors are captured in the [`DispatchResult`].
    pub fn dispatch_json(&mut self, cmd_json: &str) -> String {
        match Command::from_json(cmd_json) {
            Ok(cmd) => match self.dispatch(cmd) {
                Ok(events) => DispatchResult::ok(events).to_json(),
                Err(e) => DispatchResult::err(e.to_string()).to_json(),
            },
            Err(e) => DispatchResult::err(format!("invalid command: {e}")).to_json(),
        }
    }

    pub fn undo(&mut self) -> Result<Vec<EngineEvent>, VectraError> {
        self.stack.undo(&mut self.doc)
    }

    /// Apply a command's document effect **without** pushing a history entry,
    /// returning its exact inverse (Task 3.1).
    ///
    /// The constraint solver runs after the user's command has been applied and
    /// adjusts the geometry the command implies. Those adjustments are not
    /// actions of their own: the caller applies them here and folds the returned
    /// inverses into the entry the user's command created, with
    /// [`Engine::amend_top_backward`]. Prefer [`Engine::dispatch`] for anything
    /// a user actually asked for.
    pub fn apply_untracked(&mut self, cmd: Command) -> Result<Command, VectraError> {
        cmd.apply(&mut self.doc)
    }

    /// Record an already-applied action as one history entry (Task 3.2).
    ///
    /// `forward` must re-apply the net effect, `backward` must restore the exact
    /// pre-action state — both are commands, so undo/redo need no re-solve.
    /// Returns the stack-change event, like every other mutation.
    pub fn record(
        &mut self,
        forward: Command,
        backward: Command,
        label: impl Into<String>,
    ) -> EngineEvent {
        self.stack.record(forward, backward, label);
        EngineEvent::StackChanged {
            can_undo: self.stack.can_undo(),
            can_redo: self.stack.can_redo(),
        }
    }

    /// top history entry, so undoing the user's action also reverts it.
    pub fn amend_top_backward(&mut self, extra: Command) {
        self.stack.amend_top_backward(extra);
    }

    /// Discard the most recently dispatched command: restore the document and
    /// drop the entry, leaving the stack as if it had never been dispatched.
    ///
    /// Used when a post-apply stage rejects the result (a deep contradiction
    /// between required constraints, which only the solver can detect).
    pub fn rollback_last(&mut self) -> Result<(), VectraError> {
        self.stack.rollback_last(&mut self.doc)
    }

    pub fn redo(&mut self) -> Result<Vec<EngineEvent>, VectraError> {
        self.stack.redo(&mut self.doc)
    }

    /// Serialize the document for UI snapshots / persistence (Phase 1).
    ///
    /// Phase 2 replaces the canvas path with `EvaluatedScene` (resolved
    /// polygons/curves) while keeping this for the inspector + file format.
    pub fn snapshot_json(&self) -> String {
        serde_json::to_string(&self.doc)
            .unwrap_or_else(|_| r#"{"error":"snapshot failed"}"#.to_string())
    }

    /// Borrow an evaluation context over the live document.
    ///
    /// Owner crates (`vectra-expression`, `vectra-motion`, …) inject their
    /// evaluators via the `with_*` builders before resolving.
    pub fn evaluation_context(&self) -> EvaluationContext<'_> {
        self.doc.evaluation_context(self.time)
    }

    // ── Resolution helpers (inspector / tests / export) ────────────────

    /// Resolve a float property (`"width"`, `"radius"`, `"style.opacity"`, …).
    pub fn resolve_float(&self, node_id: NodeId, property: &str) -> Result<f64, VectraError> {
        let node = self.doc.get_node(node_id)?;
        match node.get_param(property)? {
            ParamValue::Float(p) => Ok(p.resolve(&self.evaluation_context())?),
            other => Err(VectraError::PropertyTypeMismatch {
                property: property.to_string(),
                expected: "float",
                got: other.kind(),
            }),
        }
    }

    /// Resolve a variable-bound float parameter directly (tests, constraints).
    pub fn resolve_param(&self, param: &Parameter<f64>) -> Result<f64, VectraError> {
        Ok(param.resolve(&self.evaluation_context())?)
    }

    pub fn get_variable(&self, name: &str) -> Option<f64> {
        self.doc.variables.get(name).copied()
    }

    pub fn set_variable_direct(
        &mut self,
        name: VariableId,
        value: f64,
    ) -> Result<Option<f64>, VectraError> {
        self.doc.set_variable(name, value)
    }
}

impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::NodeKind;
    use crate::ids::new_node_id;

    #[test]
    fn mvp_variable_drives_width() {
        // MES §19: change Rectangle.width via Literal and Variable; changing
        // the variable resolves a new width through the same path.
        let mut engine = Engine::new();
        let id = new_node_id();
        engine
            .dispatch(Command::CreateNode {
                id,
                kind: NodeKind::rectangle(0.0, 0.0, 100.0, 50.0),
                name: Some("rect".to_string()),
                index: None,
            })
            .unwrap();
        assert_eq!(engine.resolve_float(id, "width").unwrap(), 100.0);

        engine
            .dispatch(Command::SetVariable {
                name: "base".to_string(),
                value: 200.0,
            })
            .unwrap();
        engine
            .dispatch(Command::SetParameter {
                node_id: id,
                property: "width".to_string(),
                value: ParamValue::Float(Parameter::variable("base")),
            })
            .unwrap();
        assert_eq!(engine.resolve_float(id, "width").unwrap(), 200.0);

        engine
            .dispatch(Command::SetVariable {
                name: "base".to_string(),
                value: 320.0,
            })
            .unwrap();
        assert_eq!(engine.resolve_float(id, "width").unwrap(), 320.0);
    }

    #[test]
    fn dispatch_json_envelope_never_panics() {
        let mut engine = Engine::new();
        let ok = engine.dispatch_json(r#"{"type":"SetVariable","name":"x","value":1.0}"#);
        let parsed: DispatchResult = serde_json::from_str(&ok).unwrap();
        assert!(parsed.ok);

        let bad = engine.dispatch_json(r#"{"type":"Nope"}"#);
        let parsed: DispatchResult = serde_json::from_str(&bad).unwrap();
        assert!(!parsed.ok);
        assert!(parsed.error.is_some());
    }
}
