//! `vectra-wasm` — `wasm-bindgen` command-bus API (MES §15–§16, Tasks 1.4/2.1/2.2).
//!
//! The host (React, Tauri, tests) is a **dumb remote control**: it sends
//! [`vectra_core::Command`] JSON through [`VectraEngine::dispatch_command`]
//! and renders the [`CommandResponse`] events plus [`SnapshotResponse`]
//! snapshots. No geometry, no resolution, no dependency logic, no state lives
//! outside the engine.
//!
//! # The mutation protocol (one place, in order)
//!
//! ```text
//! dispatch_command(json)
//!   1. parse                                    → typed error on garbage
//!   2. pre-validate expression sources          → InvalidExpression, no mutation
//!   3. dry-run the dependency graph delta       → CyclicDependency, no mutation
//!   4. core.dispatch                            → the ONLY document mutation
//!   5. settle: registry sync → graph sync → dirty propagation → scene patch
//!   6. events + Dirty{ids, mode}                → the UI sees what it cost
//! ```
//!
//! Steps 2–3 are why the engine can promise that a refused command leaves the
//! document, the history, and the graph bit-identical: nothing happened yet.
//!
//! # Wire protocol (all methods take/return plain JSON strings)
//!
//! | Method | Returns |
//! |---|---|
//! | `dispatch_command(cmd)` | `{"status":"ok","events":[…]}` or `{"status":"error","message":"…"}` |
//! | `get_snapshot()` | `{"status":"ok","scene":{…},"variables":{…},"expressions":{…},"diagnostics":[…],"graph":{…},"eval":{…},"can_undo":…,"can_redo":…,"time":…}` |
//! | `undo()` / `redo()` | same envelope as `dispatch_command` |
//! | `set_time(t)` | same envelope (dirty propagation from the clock) |
//! | `dependencies()` | `{"status":"ok","nodes":[…],"edges":[…],"summary":{…}}` |
//! | `force_full_evaluation()` | same envelope as `dispatch_command`, `mode:"full"` |
//!
//! The `Dirty` event is the incrementality witness: `ids` are exactly the
//! geometry nodes this mutation re-evaluated (empty ⟺ nothing depended on it),
//! and `mode` says whether that was a patch or a full rebuild.

mod draw;
mod render;
mod snapshot;

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use snapshot::{
    build_snapshot, PlanRegionWire, PlanSourceWire, PlanSpanWire, RegionPlanWire,
    SnapshotDiagnostic, SnapshotInputs, SnapshotPosition, SnapshotSolver, SnapshotText,
};
use std::collections::BTreeSet;
use vectra_ai::exec::{self, CommandHost, ExecutionReport, Preview};
use vectra_ai::planner::HeuristicPlanner;
use vectra_ai::{AiError, Correction};
use vectra_constraints::{
    capture_value, rows, ConstraintError, ConstraintSolver, Dropped, SolveWrite,
};
use vectra_core::summary::DocumentSummary;
use vectra_core::{
    parse_node_id, Command, ConstraintTarget, Engine, EngineEvent, EvalMode, GeometryData,
    MotionBinding, MotionTrack, NodeId, NodeKind, NodeOutputId, OperationId, ParamValue, Parameter,
    ProceduralKind, Resolvable, VectraError,
};
use vectra_dependency::{gate_command, DependencyGraph, GraphExport, IncrementalScene};
use vectra_geometry::EvaluatedPrimitive;

use vectra_export::{
    compile_to_ir_resolved, export_artboards_svg, export_react, export_svg, ArtboardScope,
    ArtboardView, ExportBounds, ExportColor,
};
use vectra_expression::ExpressionEngine;
use vectra_geometry::{Diagnostic, RegionGraph};
use vectra_motion::{own_spring, reanchor, MotionEngine};
use vectra_operations::{ClipState, OperationsEvaluator};
use vectra_procedural::{ProceduralEngine, ProceduralEvaluation};
use wasm_bindgen::prelude::*;

pub use render::{CanvasRect, DirtyLedger, FrameWire, Renderer};

/// The artboard a new document opens with, in document units. 800×600 is the
/// window the renderer's opening camera already fits (`Camera::fit(800, 600)`),
/// so the paper and the view agree before the designer touches anything.
const WORKSPACE_WIDTH: f64 = 800.0;
const WORKSPACE_HEIGHT: f64 = 600.0;

/// Command-method envelope: `{"status":"ok","events":[…]}` or
/// `{"status":"error","message":"…"}`.
///
/// This envelope lives at the WASM boundary on purpose — core's native
/// `DispatchResult` keeps its own shape, and neither side is coupled to the
/// other's transport needs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "lowercase")]
enum CommandResponse {
    Ok { events: Vec<EngineEvent> },
    Error { message: String },
}

impl CommandResponse {
    fn ok_json(events: Vec<EngineEvent>) -> String {
        serde_json::to_string(&Self::Ok { events }).unwrap_or_else(|_| {
            r#"{"status":"error","message":"failed to serialize events"}"#.to_string()
        })
    }

    fn err_json(message: impl Into<String>) -> String {
        serde_json::to_string(&Self::Error {
            message: message.into(),
        })
        .unwrap_or_else(|_| {
            r#"{"status":"error","message":"failed to serialize error"}"#.to_string()
        })
    }
}

/// Dependency-inspector payload: the graph, plus a status tag.
#[derive(Debug, Clone, Serialize)]
struct DependencyResponse {
    status: &'static str,
    #[serde(flatten)]
    graph: GraphExport,
}

impl DependencyResponse {
    fn ok_json(graph: GraphExport) -> String {
        serde_json::to_string(&Self {
            status: "ok",
            graph,
        })
        .unwrap_or_else(|_| {
            r#"{"status":"error","message":"failed to serialize graph"}"#.to_string()
        })
    }
}

/// The engine instance hosted in the browser / Tauri webview / Node smoke harness.
///
/// Three derived structures follow the document through [`VectraEngine::settle`]:
/// the compiled expression registry (Task 2.1), the dependency graph, and the
/// incrementally patched scene cache (Task 2.2).
/// How many times one `settle` may run the procedural pass.
///
/// Two is the number a legal document can use (see
/// [`VectraEngine::run_procedural_fixpoint`]); the two extra rounds exist so a
/// registry that reached the engine some other way is *reported* by the
/// `debug_assert` instead of silently truncating a longer legal chain.
const MAX_PASS_ROUNDS: usize = 4;

#[wasm_bindgen]
pub struct VectraEngine {
    core: Engine,
    /// The host's font faces (Task 11.0). Empty by default: every family
    /// then resolves to the **bundled** face (`vectra_geometry::BUNDLED_FACE`),
    /// so a fresh engine can set type with no host cooperation at all. A
    /// host that ships faces registers them here and every text node in
    /// every document sees them.
    fonts: vectra_geometry::FontLibrary,
    expressions: ExpressionEngine,
    graph: DependencyGraph,
    scene: IncrementalScene,
    /// The constraint solver (Task 3.1). Owns the Cassowary variable pool, so
    /// the variable count tracks the live rules across solves.
    solver: ConstraintSolver,
    /// Diagnostics produced by the last solver pass; merged into the snapshot's
    /// single diagnostics channel.
    solver_diagnostics: Vec<Diagnostic>,
    /// The non-destructive operations pass (Task 4.0): composes virtual nodes
    /// on top of the primitive scene.
    operations: OperationsEvaluator,
    /// Operation ids whose *registry entry* changed in the command being
    /// applied, so `settle` recomputes them even when no source moved (a new
    /// operation, a disabled one, a removed one).
    pending_operations: Vec<OperationId>,
    /// The procedural pass (Task 7.0). Runs **last** (RULE 2), after primitives
    /// and operations, because a `Source` node reads the evaluated scene.
    procedural: ProceduralEngine,
    /// Procedural nodes this command touched: the pass recomputes exactly these
    /// plus whatever the dependency graph dirtied.
    pending_procedural: Vec<NodeId>,
    /// Diagnostics from the last operations pass, merged into the snapshot.
    operation_diagnostics: Vec<Diagnostic>,
    /// The nodes the designer has selected (Task 10.6 RULE 2). The command bar
    /// sends this before running a prompt, so "make this geometric" has a
    /// "this"; the panel reads it to decide what it is inspecting.
    selection: std::cell::RefCell<Vec<NodeId>>,
    /// The motion evaluator (Task 6.0): springs, state flags and keyframe
    /// tracks, resolved at the clock. Holds a synced copy of
    /// `Document::motion` and the host-set state flags — *inputs*, never history.
    motion: MotionEngine,
    /// The live drag gesture (Task 3.2), if the pointer is down.
    drag: Option<DragTxn>,
    /// Task 5.0: the dirty ids the renderer has not drawn yet. Every mutation
    /// notes here (through `dirty_event`); `render_frame` drains it. Coalescing
    /// is the point: a gesture settles many times per displayed frame, and the
    /// canvas must upload once per *frame*, not once per pointer sample.
    render_dirty: DirtyLedger,
    /// Task 10.1: the drawing tools' gesture state — the pen's anchors and the
    /// brush's samples. It holds *input*, never document state: nothing here is
    /// undoable until `draw_*_commit` turns it into commands.
    draw: crate::draw::DrawTools,
    /// **Task 10.7 RULE 3b**: the clipping masks' bookkeeping — each clipped
    /// node's *document* geometry (its baseline) and what the scene currently
    /// shows. A derivation cache, not document state: see
    /// [`vectra_operations::ClipState`] for why recomputing from a baseline,
    /// rather than from the clipped result, is the difference between a mask and
    /// a shrink.
    clip: ClipState,
}

/// `hover:` prefix on the per-node state flag a hover spring reads.
const HOVER_PREFIX: &str = "hover:";

/// The flag a node's hover spring branches on.
fn hover_flag(node_id: NodeId) -> String {
    format!("{HOVER_PREFIX}{node_id}")
}

/// One binding, as the inspector sees it (Task 6.0).
#[derive(Debug, Clone, Serialize)]
struct MotionBindingWire {
    node_id: String,
    property: String,
    kind: String,
    /// The live value of the slot at the current clock: what the spring is
    /// currently at, or what the track samples — the number a designer wants
    /// next to the binding.
    value: Option<f64>,
    from: Option<f64>,
    at: Option<f64>,
    target: Option<f64>,
    stiffness: Option<f64>,
    damping: Option<f64>,
    state: Option<String>,
    track: Option<String>,
    channel: Option<String>,
}

impl MotionBindingWire {
    fn of(
        node_id: NodeId,
        property: &str,
        binding: &MotionBinding,
        ctx: &vectra_core::EvaluationContext<'_>,
    ) -> Self {
        // The live value, sampled through whatever evaluator the context wires
        // (this is the same call the evaluator dispatch makes), falling back to
        // the value the binding is aiming at when resolution fails — a missing
        // track should still show *something* in the inspector.
        let value = ctx
            .motion
            .and_then(|m| m.evaluate(binding, ctx).ok())
            .or_else(|| binding_target(binding, ctx));
        match binding {
            MotionBinding::Spring {
                target,
                stiffness,
                damping,
                from,
                at,
            } => Self {
                node_id: node_id.to_string(),
                property: property.to_string(),
                kind: "spring".to_string(),
                value,
                from: Some(*from),
                at: Some(*at),
                target: target.resolve(ctx).ok(),
                stiffness: Some(*stiffness),
                damping: Some(*damping),
                state: binding.state_flags().first().map(|flag| flag.to_string()),
                track: None,
                channel: None,
            },
            MotionBinding::StateDriven { state, .. } => Self {
                node_id: node_id.to_string(),
                property: property.to_string(),
                kind: "state".to_string(),
                value,
                from: None,
                at: None,
                target: None,
                stiffness: None,
                damping: None,
                state: Some(state.clone()),
                track: None,
                channel: None,
            },
            MotionBinding::KeyframeTrack {
                track_id,
                property: channel,
            } => Self {
                node_id: node_id.to_string(),
                // The *slot* this binding drives — the same field every other
                // binding reports. The channel it samples is a different name
                // (a track can drive `y` from its `x` channel, and the inspector
                // has to show both or neither).
                property: property.to_string(),
                kind: "track".to_string(),
                value,
                from: None,
                at: None,
                target: None,
                stiffness: None,
                damping: None,
                state: None,
                track: Some(track_id.clone()),
                channel: Some(channel.clone()),
            },
        }
    }
}

/// The value a binding eases toward / ends at, used when the live resolution is
/// unavailable (a missing track, a missing flag): the inspector shows the
/// *intended* value and lets the diagnostic explain the gap.
fn binding_target(
    binding: &MotionBinding,
    ctx: &vectra_core::EvaluationContext<'_>,
) -> Option<f64> {
    match binding {
        MotionBinding::Spring { target, .. } => target.resolve(ctx).ok(),
        MotionBinding::StateDriven { false_value, .. } => false_value.resolve(ctx).ok(),
        MotionBinding::KeyframeTrack { .. } => None,
    }
}

/// The whole motion state of the document, for the inspector and the harness.
#[derive(Debug, Clone, Serialize)]
struct MotionReportWire {
    animating: bool,
    horizon: Option<f64>,
    time: f64,
    epsilon: f64,
    states: Vec<String>,
    tracks: Vec<String>,
    bindings: Vec<MotionBindingWire>,
}

/// One drag gesture, engine-side (Task 3.2).
///
/// The solver owns the tableau for the gesture; this owns the **history**: the
/// parameter each touched slot had before the gesture's first write (so undo
/// restores links, not just values), the last value each ended at (so redo
/// replays the net movement without a solver), and the diagnostics the gesture
/// produced, so a broken link stays visible while the pointer is down.
struct DragTxn {
    node_id: NodeId,
    label: String,
    /// slot label → `(node, property, parameter before the first write)`.
    first_source: BTreeMap<String, (NodeId, String, ParamValue)>,
    /// slot label → the last solved value.
    last_value: BTreeMap<String, f64>,
    /// Diagnostics accumulated across the gesture, deduplicated.
    diagnostics: Vec<Diagnostic>,
    /// Pointer samples applied so far.
    updates: u32,
    /// Constraints the gesture parked (an over-constrained list that lost while
    /// the pointer was down). Recorded so undo *and* redo reproduce the flag.
    parked: Vec<vectra_core::ConstraintId>,
    /// Slot label → the spring the slot was animated by before the first write
    /// (**Task 6.0**, Interaction Precedence Law).
    ///
    /// A live drag outranks motion: the first pointer sample replaces the spring
    /// with a literal, and the pointer owns the slot until release. On release
    /// the slot does not stay a literal — the committed value **re-anchors the
    /// spring it came from**, so the spring resumes from wherever the finger let
    /// go. Capturing the binding here (rather than looking it up afterwards) is
    /// what makes the gesture exactly invertible: `forward()` writes the same
    /// re-anchored binding it will write on redo.
    spring_before: BTreeMap<String, MotionBinding>,
}

impl DragTxn {
    fn new(node_id: NodeId, label: String) -> Self {
        Self {
            node_id,
            label,
            first_source: BTreeMap::new(),
            last_value: BTreeMap::new(),
            spring_before: BTreeMap::new(),
            diagnostics: Vec::new(),
            updates: 0,
            parked: Vec::new(),
        }
    }

    /// Remember that the gesture parked `id` (deduplicated).
    fn park(&mut self, id: vectra_core::ConstraintId) {
        if !self.parked.contains(&id) {
            self.parked.push(id);
        }
    }

    /// Add diagnostics, keeping the first occurrence of each.
    fn merge_diagnostics(&mut self, more: Vec<Diagnostic>) {
        for diagnostic in more {
            if !self.diagnostics.contains(&diagnostic) {
                self.diagnostics.push(diagnostic);
            }
        }
    }

    /// The net movement as one command (deterministic slot order) — the drag
    /// entry's *forward*.
    fn forward(&self, now: f64) -> Command {
        let mut commands: Vec<Command> = self
            .parked
            .iter()
            .map(|id| Command::SetConstraintEnabled {
                id: *id,
                enabled: false,
            })
            .collect();
        let reanchored: BTreeMap<&str, &MotionBinding> = self
            .spring_before
            .iter()
            .map(|(label, binding)| (label.as_str(), binding))
            .collect();
        commands.extend(self.last_value.iter().map(|(label, value)| {
            let (node_id, property) = self
                .slot_of(label)
                .expect("every written slot has a recorded pre-state");
            match reanchored.get(label.as_str()) {
                // Animated before, animated after: redo must replay the same
                // re-anchor the gesture applied, or redo would leave a literal
                // where undo restored a spring (the invertibility law).
                Some(binding) => Command::BindMotion {
                    node_id,
                    property: property.to_string(),
                    binding: reanchor(binding, *value, now),
                },
                None => Command::SetParameter {
                    node_id,
                    property: property.to_string(),
                    value: ParamValue::Float(Parameter::Literal(*value)),
                },
            }
        }));
        Command::batch_commands(commands)
    }

    /// The exact pre-gesture state as one command — the drag entry's *backward*.
    fn backward(&self) -> Command {
        let mut commands: Vec<Command> = self
            .parked
            .iter()
            .map(|id| Command::SetConstraintEnabled {
                id: *id,
                enabled: true,
            })
            .collect();
        commands.extend(
            self.first_source
                .values()
                .map(|(node_id, property, source)| Command::SetParameter {
                    node_id: *node_id,
                    property: property.clone(),
                    value: source.clone(),
                }),
        );
        Command::batch_commands(commands)
    }

    fn slot_of(&self, label: &str) -> Option<(NodeId, &str)> {
        self.first_source
            .get(label)
            .map(|(node_id, property, _)| (*node_id, property.as_str()))
    }

    /// Record a solve's writes without applying them (the end-of-gesture pass
    /// has already written them through the ordinary command path).
    fn record_writes(&mut self, writes: &[SolveWrite]) {
        for write in writes {
            self.record_one(write);
        }
    }

    fn record_one(&mut self, write: &SolveWrite) {
        let label = write.target.label();
        let fresh = !self.first_source.contains_key(&label);
        self.first_source.entry(label.clone()).or_insert_with(|| {
            (
                write.target.node_id,
                write.target.property.clone(),
                write.source.clone(),
            )
        });
        // …and if that pre-gesture source was a spring, remember it: the release
        // hands the slot back to motion instead of freezing a literal.
        if fresh {
            if let ParamValue::Float(param) = &write.source {
                if let Some(binding) = own_spring(param) {
                    self.spring_before.insert(label.clone(), binding.clone());
                }
            }
        }
        self.last_value.insert(label, write.value);
    }

    /// The re-anchor writes this gesture owes on release: for every slot that was
    /// animated by a spring, the spring's new anchor is the committed value.
    fn reanchor_plan(&self, now: f64) -> Vec<(NodeId, String, MotionBinding)> {
        let mut plan = Vec::new();
        for (label, binding) in &self.spring_before {
            let (Some((node_id, property)), Some(value)) =
                (self.slot_of(label), self.last_value.get(label))
            else {
                continue;
            };
            plan.push((
                node_id,
                property.to_string(),
                reanchor(binding, *value, now),
            ));
        }
        plan
    }
}

#[wasm_bindgen]
impl VectraEngine {
    /// Initialize a fresh engine.
    #[wasm_bindgen(constructor)]
    pub fn new() -> Self {
        console_error_panic_hook::set_once();
        let mut core = Engine::new();
        // **The workspace a new document opens into** (Task 10.2 RULES 1–2): one
        // artboard, one layer, with fixed ids. It is document state, not a
        // history entry — a designer does not undo the paper they were given —
        // and it lives on the document type rather than in this constructor so
        // the UI's engine and any future host share one definition. RULE 1 says
        // every node belongs to a layer; this is what makes that true for the
        // *first* node the designer draws, without a special case in
        // `insert_node`.
        core.document_mut()
            .open_workspace(WORKSPACE_WIDTH, WORKSPACE_HEIGHT);
        Self {
            core,
            expressions: ExpressionEngine::new(),

            selection: std::cell::RefCell::new(Vec::new()),
            graph: DependencyGraph::new(),
            scene: IncrementalScene::new(),
            operations: OperationsEvaluator::new(),
            pending_procedural: Vec::new(),
            pending_operations: Vec::new(),
            operation_diagnostics: Vec::new(),
            procedural: ProceduralEngine::new(),
            motion: MotionEngine::new(),
            solver: ConstraintSolver::new(),
            solver_diagnostics: Vec::new(),
            drag: None,
            render_dirty: DirtyLedger::new(),
            draw: crate::draw::DrawTools::new(),
            clip: ClipState::new(),
            // The bundled face answers every family until a host registers
            // more, so text draws out of the box on any machine.
            fonts: vectra_geometry::FontLibrary::bundled(),
        }
    }

    /// Execute a [`Command`] JSON string. Returns a [`CommandResponse`] JSON string.
    ///
    /// `DefineExpression` sources are pre-validated (test-compiled) and every
    /// edge-adding command is dry-run through the dependency graph BEFORE
    /// dispatch: an invalid expression or a cycle fails with a typed error and
    /// *zero* mutation — no record, no undo entry, no graph edge.
    pub fn dispatch_command(&mut self, cmd_json: &str) -> String {
        let mut command = match Command::from_json(cmd_json) {
            Ok(cmd) => cmd,
            Err(e) => return CommandResponse::err_json(format!("invalid command: {e}")),
        };
        // ── Task 3.2: the drag protocol ───────────────────────────────────
        //
        // A live gesture owns the tableau, so while the pointer is down the only
        // commands that may touch the document are the gesture's own; everything
        // else is refused with a typed error *before* anything is applied.
        match &command {
            Command::BeginDrag { node_id } => return self.begin_drag(*node_id),
            Command::UpdateDrag { node_id, x, y } => return self.update_drag(*node_id, *x, *y),
            Command::EndDrag { node_id } => return self.end_drag(*node_id),
            _ => {
                if let Some(drag) = &self.drag {
                    return CommandResponse::err_json(
                        VectraError::drag_active(drag.node_id).to_string(),
                    );
                }
            }
        }

        if let Command::DefineExpression { source, .. } = &command {
            if let Err(e) = ExpressionEngine::check_source(source) {
                return CommandResponse::err_json(format!("invalid expression: {e}"));
            }
        }
        // A constraint that can never hold is refused here: nothing is applied,
        // nothing is pushed, no edge is added — the same discipline as the
        // cycle gate below.
        if let Err(e) = ConstraintSolver::preflight(self.core.document(), &command) {
            return CommandResponse::err_json(constraint_error_message(e));
        }
        if let Err(e) = self.capture_operands(&mut command) {
            return CommandResponse::err_json(e);
        }
        if let Err(e) = gate_command(&self.graph, &command, self.core.document()) {
            return CommandResponse::err_json(e.to_string());
        }
        // Task 4.0: remember which operation-registry entries this command
        // touches, so `settle` recomputes exactly those virtual nodes (a
        // removed or disabled operation has no geometry to compute — the
        // registry patch retires it).
        for id in operation_ids_of(&command) {
            if !self.pending_operations.contains(&id) {
                self.pending_operations.push(id);
            }
        }
        // Task 7.0: the same for the procedural registry — a node this command
        // touched is recomputed by the pass, whatever the graph says.
        for id in procedural_ids_of(&command) {
            if !self.pending_procedural.contains(&id) {
                self.pending_procedural.push(id);
            }
        }
        match self.core.dispatch(command.clone()) {
            Ok(events) => {
                match self.constraint_pass(&command, true) {
                    Ok(pass) => {
                        self.solver_diagnostics = pass.diagnostics;
                        let mut events = events;
                        events.extend(pass.events);
                        CommandResponse::ok_json(self.settle(events))
                    }
                    Err(e) => {
                        // The tableau refused the system (a contradiction the
                        // pre-pass cannot see): rewind the command so the
                        // document, stack and solver are exactly as they were.
                        let _ = self.core.rollback_last();
                        self.solver_diagnostics.clear();
                        CommandResponse::err_json(constraint_error_message(e))
                    }
                }
            }
            Err(e) => CommandResponse::err_json(e.to_string()),
        }
    }

    /// Project the live engine into a [`snapshot::SnapshotResponse`] JSON string.
    ///
    /// The first call warms the scene cache (one full pass); every later call is
    /// a pure projection — evaluation already happened during the mutation that
    /// dirtied it.
    pub fn get_snapshot(&mut self) -> String {
        let positions = self.positions();
        let texts = self.texts();
        {
            let Self {
                core,
                expressions,
                scene,
                motion,
                procedural,
                ..
            } = self;
            let ctx = core
                .evaluation_context()
                .with_expression(expressions)
                .with_motion(motion)
                .with_procedural(procedural)
                .with_fonts(&self.fonts);
            scene.refresh_if_cold(core.document(), &ctx);
        }
        let Self {
            core,
            scene,
            graph,
            solver,
            solver_diagnostics,
            operation_diagnostics,
            procedural,
            ..
        } = self;
        let mut diagnostics: Vec<Diagnostic> = scene.diagnostics().to_vec();
        diagnostics.extend(operation_diagnostics.iter().cloned());
        diagnostics.extend(procedural.diagnostics().iter().cloned());
        let published = published_table(procedural, core.document());
        build_snapshot(SnapshotInputs {
            engine: core,
            scene: scene.scene(),
            diagnostics: &diagnostics,
            solver_diagnostics,
            positions: &positions,
            texts: &texts,
            // The picker's options, sorted by the projection: the library's own
            // keys, never a hardcoded list — a document naming a family this
            // host lacks still shows *its* family (the panel adds it).
            fonts: {
                let mut families = self.fonts.families();
                families.sort();
                families.dedup();
                families
            },
            published: &published,
            solver: SnapshotSolver::from_stats(
                solver.stats(),
                solver.drag_node().map(|node| node.to_string()),
            ),
            stats: scene.stats(),
            graph: graph.summary(),
        })
        .to_json()
    }

    /// The dependency graph, for the inspector panel.
    pub fn dependencies(&self) -> String {
        DependencyResponse::ok_json(self.graph.export())
    }

    /// **The palette**: every kind the engine can build, with its ports and the
    /// operand payload an `AddProceduralNode` must carry.
    ///
    /// The panel owns no table of node kinds, port names, or default numbers: it
    /// lists what this returns and forwards the operand payload **verbatim**.
    /// That is the same discipline as the command builders — if the engine
    /// changes a default, the panel changes with it, because the panel never had
    /// a copy.
    ///
    /// `needs_subject` marks the kind whose subject is *part of the kind* rather
    /// than an operand (`Source { node }`): the panel offers a layer picker for
    /// those, and sends `{"type": "source", "node": "…"}`.
    #[wasm_bindgen]
    pub fn procedural_kinds(&self) -> String {
        let samples = [
            (
                ProceduralKind::Source {
                    node: NodeId::nil(),
                },
                true,
            ),
            (
                ProceduralKind::grid(3.0, 2.0, 40.0, vectra_core::Point2::ZERO),
                false,
            ),
            (ProceduralKind::repeat(3.0, 60.0, 0.0), false),
            (ProceduralKind::noise(1.0, 0.1, 1.0), false),
            (ProceduralKind::smooth(2.0, 0.5), false),
        ];
        let kinds: Vec<ProceduralKindWire> = samples
            .into_iter()
            .map(|(kind, needs_subject)| ProceduralKindWire {
                tag: kind.tag().to_string(),
                label: kind.describe(),
                needs_subject,
                operands: kind
                    .default_operands()
                    .into_iter()
                    .map(|(port, value)| {
                        // The *kind's* fields are `Parameter<T>`, not `ParamValue`
                        // — the wrapper that says which kind of slot it is lives
                        // one level up. Unwrap it here so the panel can spread
                        // this payload straight into the kind it is building:
                        // a `{"Float": …}` wrapper at that level is exactly the
                        // bug this function exists to prevent.
                        let field = match value {
                            ParamValue::Float(parameter) => serde_json::to_value(parameter),
                            ParamValue::Point(parameter) => serde_json::to_value(parameter),
                            ParamValue::Color(parameter) => serde_json::to_value(parameter),
                        };
                        (port.clone(), field.unwrap_or(serde_json::Value::Null))
                    })
                    .collect(),
                inputs: kind
                    .inputs()
                    .iter()
                    .map(|port| ProceduralKindPortWire {
                        port: port.name.clone(),
                        ty: port.ty.tag().to_string(),
                        required: port.required,
                    })
                    .collect(),
                outputs: kind
                    .outputs()
                    .iter()
                    .map(|port| ProceduralKindPortWire {
                        port: port.name.clone(),
                        ty: port.ty.tag().to_string(),
                        required: false,
                    })
                    .collect(),
            })
            .collect();
        serde_json::to_string(&kinds).unwrap_or_else(|e| format!("{{\"error\":{e:?}}}"))
    }

    /// The drawing as a **semantic SVG document** (Task 8.0, RULE 1).
    ///
    /// The picture is compiled from the document *and* the scene the engine is
    /// currently drawing (`compile_to_ir_resolved`), so motion is exported at the
    /// instant the user clicked, and a procedural result exports as the region it
    /// computed — never as an approximation of a primitive it is not.
    ///
    /// Returns `{"status","format","code","warnings"}`: the UI shows `code`, and
    /// `warnings` says why an exported number is not the authored one (a clamp, a
    /// spring, an unresolved variable).
    #[wasm_bindgen]
    pub fn export_to_svg(&self) -> String {
        let ir = compile_to_ir_resolved(self.core.document(), self.scene.scene());
        export_envelope("svg", export_svg(&ir), &ir.warnings)
    }

    /// **Export current artboard** (Task 10.2 RULE 2): the SVG of the board the
    /// designer is on, cropped to its frame and painted with its background.
    ///
    /// The document decides what belongs to the board (its layers' nodes); the
    /// exporter decides what the file looks like. Nothing here filters geometry
    /// by hand — the same IR, the same element table, one extra frame.
    #[wasm_bindgen]
    pub fn export_current_artboard(&self) -> String {
        let boards = self.artboard_views();
        let ir = compile_to_ir_resolved(self.core.document(), self.scene.scene());
        // The active board first (and, for `Current`, only it): a document with
        // no artboards at all still exports its whole picture rather than
        // nothing.
        let active: Vec<ArtboardView> = {
            let target = self.core.document().artboards.active_id();
            let found: Vec<ArtboardView> = boards
                .iter()
                .filter(|board| Some(board.id.clone()) == target.map(|id| id.to_string()))
                .cloned()
                .collect();
            if found.is_empty() {
                boards.clone()
            } else {
                found
            }
        };
        export_envelope(
            "svg",
            export_artboards_svg(&ir, &active, ArtboardScope::Current),
            &ir.warnings,
        )
    }

    /// **Export all artboards** (Task 10.2 RULE 2): every board, laid out where
    /// it sits in document space, so the file reads as one canvas with frames on
    /// it — the same arrangement the GPU canvas shows.
    #[wasm_bindgen]
    pub fn export_all_artboards(&self) -> String {
        let ir = compile_to_ir_resolved(self.core.document(), self.scene.scene());
        let boards = self.artboard_views();
        export_envelope(
            "svg",
            export_artboards_svg(&ir, &boards, ArtboardScope::All),
            &ir.warnings,
        )
    }

    /// The artboards as the exporter needs them: frame, background and the node
    /// ids each one draws (Task 10.2 RULE 2).
    fn artboard_views(&self) -> Vec<ArtboardView> {
        let doc = self.core.document();
        doc.artboards
            .iter()
            .map(|board| {
                // A board draws the nodes its layers list, in the document's own
                // draw order — so the SVG's stacking is the canvas's stacking
                // (RULE 2: artboards own their layer stacks).
                let owned: BTreeSet<NodeId> = board
                    .layers
                    .iter()
                    .filter_map(|layer| doc.layers.get(layer))
                    .flat_map(|layer| layer.children.iter().copied())
                    .collect();
                ArtboardView {
                    id: board.id.to_string(),
                    name: board.name.clone(),
                    bounds: ExportBounds {
                        min_x: board.x,
                        min_y: board.y,
                        max_x: board.x + board.width,
                        max_y: board.y + board.height,
                    },
                    background: ExportColor::literal(board.background),
                    nodes: doc
                        .geometry_ids()
                        .into_iter()
                        .filter(|id| owned.contains(id))
                        .map(|id| id.to_string())
                        .collect(),
                }
            })
            .collect()
    }

    /// The drawing as a **parametric React component** (Task 8.0, RULE 2).
    ///
    /// A slot driven by `$base * 2` comes out as `width={base * 2}` with `base`
    /// declared as a required prop: the same picture, as code that goes on taking
    /// arguments.
    #[wasm_bindgen]
    pub fn export_to_react(&self) -> String {
        let ir = compile_to_ir_resolved(self.core.document(), self.scene.scene());
        export_envelope("react", export_react(&ir), &ir.warnings)
    }

    // ── Task 9.0: the AI command layer ─────────────────────────────────
    //
    // Three methods, one envelope each. The AI *never* writes geometry: it emits
    // commands (RULE 1), grounded on this engine's own document summary
    // (RULE 2), and every one of them goes through `dispatch_command` before it
    // is applied (RULE 3).

    /// The document itself, as JSON (Task 10.0).
    ///
    /// **The one method Task 10.0 added to this boundary, and it is read-only:**
    /// it serializes what the engine is holding, verbatim, with the same `serde`
    /// that produced the file the document was loaded from. Nothing here
    /// interprets, resolves or normalizes — a parametric slot still reads
    /// `{"Expression":"<id>"}`, a spring is still a spring, an operation is
    /// still virtual. The native `.vectra` codec (`vectra-file`) takes the string
    /// from here and gzips it.
    ///
    /// It was added because Task 10.0's RULE 3 requires the frontend to hand the
    /// raw `Document` JSON to the save path, and the alternative — rebuilding a
    /// document natively *outside* the engine — would have meant a second copy of
    /// the settle pipeline, i.e. two engines that could disagree about a file.
    /// Loading does **not** come back through here: a file is replayed as
    /// commands through `dispatch_command`, so the engine's own gates validate
    /// every loaded document.
    #[wasm_bindgen]
    pub fn document_json(&self) -> String {
        serde_json::to_string(self.core.document())
            .unwrap_or_else(|error| format!("{{\"status\":\"error\",\"message\":{error:?}}}"))
    }

    /// The AI-facing document summary (RULE 2), as JSON.
    ///
    /// The engine's own view, resolved through the live evaluators, so an
    /// expression slot reads `$base * 2` and a procedural read its published
    /// value. This is what a planner is grounded on and what `ai_prompt` embeds.
    #[wasm_bindgen]
    pub fn document_summary(&self) -> String {
        serde_json::to_string(&self.document_summary_value())
            .unwrap_or_else(|error| format!("{{\"status\":\"error\",\"message\":{error:?}}}"))
    }

    /// The **system prompt** the MVP planner is written for, with this document
    /// already injected — the AI panel shows it, and a hosted model would be
    /// called with exactly this text.
    #[wasm_bindgen]
    pub fn ai_prompt(&self) -> String {
        vectra_ai::system_prompt(&self.document_summary_value())
    }

    /// The phrasings the built-in planner understands, for the panel's hint
    /// line. The list lives beside the matcher in `vectra-ai`, so a hint can
    /// never advertise something the planner cannot do.
    #[wasm_bindgen]
    pub fn ai_phrasings(&self) -> String {
        serde_json::to_string(&vectra_ai::phrasings()).unwrap_or_else(|_| "[]".to_string())
    }

    /// Generate commands from a prompt, **without applying anything** — the AI
    /// panel's preview.
    ///
    /// `summary_json` is the summary the model should be grounded on (the panel
    /// passes back what [`VectraEngine::document_summary`] gave it, so the UI
    /// computes nothing). An empty string means "use the live document", which
    /// is what a headless caller wants.
    ///
    /// Returns `{"status":"ok","plan":[…],"notes":[…],"attempt":1}` or
    /// `{"status":"error","code":…,"message":…}`.
    #[wasm_bindgen]
    pub fn ai_generate_commands(&self, prompt: &str, summary_json: &str) -> String {
        match self.ai_summary(summary_json) {
            Ok(summary) => {
                let host = AiPreviewHost { summary };
                match exec::preview(&HeuristicPlanner::new(), &host, prompt) {
                    Ok(preview) => ai_preview_json(&preview),
                    Err(error) => ai_error_json(&error),
                }
            }
            Err(error) => ai_error_json(&error),
        }
    }

    /// Apply a plan that was already previewed and approved.
    ///
    /// No planner, so no retry: the plan runs exactly as shown, or it is refused
    /// (and rolled back) with the engine's own message. The AI panel's *Execute*
    /// with auto-correct switched off.
    #[wasm_bindgen]
    pub fn ai_execute_commands(&mut self, prompt: &str, plan_json: &str) -> String {
        let commands: Vec<Command> = match serde_json::from_str(plan_json) {
            Ok(commands) => commands,
            Err(error) => {
                return ai_error_json(&AiError::InvalidJson {
                    detail: format!("the plan is not a command array: {error}"),
                    reply: plan_json.to_string(),
                })
            }
        };
        let mut host = EngineHost { engine: self };
        match exec::execute_plan(&mut host, prompt, &commands) {
            Ok(report) => ai_report_json(&report),
            Err(error) => ai_error_json(&error),
        }
    }

    /// The whole loop (RULE 3): generate, validate through dispatch, feed a
    /// refusal back, self-correct up to two times, then report.
    ///
    /// Returns `{"status":"ok","report":{…}}` or
    /// `{"status":"error","code":"max-retries-exceeded","message":…,"corrections":[…]}`
    /// — with nothing applied in the failure case.
    #[wasm_bindgen]
    pub fn ai_execute_with_retry(&mut self, prompt: &str, summary_json: &str) -> String {
        if let Err(error) = self.ai_summary(summary_json) {
            return ai_error_json(&error);
        }
        // The host re-derives the summary for every attempt, so the planner
        // always reasons about the live document; the argument is a contract
        // check, not the grounding source.
        let planner = HeuristicPlanner::new();
        let mut host = EngineHost { engine: self };
        match exec::execute(&planner, &mut host, prompt) {
            Ok(report) => ai_report_json(&report),
            Err(error) => ai_error_json(&error),
        }
    }

    /// The procedural graph, for the Procedural panel and the smoke harness.
    ///
    /// Three things the panel cannot get anywhere else: the registry's own
    /// order (a chain has a direction, a JSON object does not), each operand's
    /// **effective** value (`(variable)`, not the number the template started
    /// with — see `ProceduralNode::describe`), and the value each output port
    /// last published.
    // ── Task 10.6: Smart Components (RULE 1) ────────────────────────────────
    /// Tell the engine what the designer has selected.
    ///
    /// The selection is *state*, not a command: it is not undoable, it does not
    /// touch the document, and it exists so that a Make Magic prompt can mean
    /// "this". The reply carries the panel's one-line description, so the UI
    /// never has to compose a sentence about ids.
    #[wasm_bindgen]
    pub fn set_selection(&self, ids_json: &str) -> String {
        let ids: Vec<String> = match serde_json::from_str(ids_json) {
            Ok(ids) => ids,
            Err(error) => {
                return format!(
                    "{{\"status\":\"error\",\"message\":{}}}",
                    serde_json::to_string(&format!("invalid selection: {error}"))
                        .unwrap_or_else(|_| "\"invalid selection\"".to_string())
                )
            }
        };
        let parsed: Vec<NodeId> = ids
            .iter()
            .filter_map(|id| vectra_core::parse_node_id(id))
            .collect();
        let mut stored = self.selection.borrow_mut();
        *stored = parsed;
        drop(stored);
        let summary = self.document_summary_value();
        #[derive(serde::Serialize)]
        struct SelectionWire<'a> {
            status: &'a str,
            count: usize,
            prose: String,
        }
        serde_json::to_string(&SelectionWire {
            status: "ok",
            count: summary.selection.len(),
            prose: summary.selection_prose(),
        })
        .unwrap_or_else(|_| "{\"status\":\"ok\"}".to_string())
    }

    /// The Smart Component inspector's whole world: what the selection is, and
    /// every prop it exposes (Task 10.6 RULE 1, RULE 4).
    #[wasm_bindgen]
    pub fn component_view(&self) -> String {
        let doc = self.core.document();
        let selection = self.selection.borrow().clone();
        let view = vectra_core::component::inspect(doc, &selection);
        let masters: Vec<serde_json::Value> = doc
            .procedural
            .in_order()
            .filter(|node| {
                matches!(
                    node.kind,
                    vectra_core::ProceduralKind::ComponentMaster { .. }
                )
            })
            .map(|node| {
                let instances = vectra_core::component::instances_of(doc, node.id).len();
                serde_json::json!({
                    "id": node.id.to_string(),
                    "name": node.name,
                    "instances": instances,
                })
            })
            .collect();
        let summary = self.document_summary_value();
        let mut value = match serde_json::to_value(&view) {
            Ok(value) => value,
            Err(error) => {
                return format!(
                    "{{\"status\":\"error\",\"message\":{}}}",
                    serde_json::to_string(&format!("serialization: {error}"))
                        .unwrap_or_else(|_| "\"serialization\"".to_string())
                )
            }
        };
        if let Some(map) = value.as_object_mut() {
            map.insert("status".to_string(), serde_json::json!("ok"));
            map.insert(
                "selection".to_string(),
                serde_json::json!({
                    "count": summary.selection.len(),
                    "prose": summary.selection_prose(),
                }),
            );
            map.insert("masters".to_string(), serde_json::json!(masters));
        }
        value.to_string()
    }

    /// **Create Component** (RULE 1): the selected nodes become a master whose
    /// props every instance will set for itself.
    #[wasm_bindgen]
    pub fn create_component(&mut self, members_json: &str, name: Option<String>) -> String {
        let ids: Vec<String> = match serde_json::from_str(members_json) {
            Ok(ids) => ids,
            Err(error) => return CommandResponse::err_json(format!("invalid members: {error}")),
        };
        let members: Vec<NodeId> = ids
            .iter()
            .filter_map(|id| vectra_core::parse_node_id(id))
            .collect();
        if members.is_empty() {
            return CommandResponse::err_json("select at least one shape first");
        }
        let command = Command::CreateComponent {
            id: vectra_core::new_node_id(),
            name,
            members,
            props: Vec::new(),
        };
        self.dispatch_side_command(&command)
    }

    /// **Place an instance** of a component (RULE 1).
    #[wasm_bindgen]
    pub fn instantiate_component(&mut self, master: &str, name: Option<String>) -> String {
        let Some(master) = vectra_core::parse_node_id(master) else {
            return CommandResponse::err_json("invalid id".to_string());
        };
        let command = Command::InstantiateComponent {
            id: vectra_core::new_node_id(),
            master,
            name,
            index: None,
        };
        self.dispatch_side_command(&command)
    }

    /// **Set one prop on one instance** (RULE 1) — the slider the panel draws.
    ///
    /// `value_json` is a typed [`vectra_core::ParamValue`]
    /// (`{"Float":{"Literal":32}}` / `{"Color":{"Literal":"#2266ee"}}`), which
    /// is exactly what `component_view` publishes per prop type.
    #[wasm_bindgen]
    pub fn set_component_prop(&mut self, target: &str, prop: &str, value_json: &str) -> String {
        let Some(target) = vectra_core::parse_node_id(target) else {
            return CommandResponse::err_json("invalid id".to_string());
        };
        let value: ParamValue = match serde_json::from_str(value_json) {
            Ok(value) => value,
            Err(error) => return CommandResponse::err_json(format!("invalid value: {error}")),
        };
        let command = Command::SetComponentProp {
            target,
            prop: prop.to_string(),
            value,
        };
        self.dispatch_side_command(&command)
    }

    /// **Generate Icon Set** (RULE 3): one instance per size, each on its own
    /// artboard, all scaled by the master's `size` prop.
    #[wasm_bindgen]
    pub fn icon_set(&mut self, master: &str, sizes_json: &str) -> String {
        let Some(master) = vectra_core::parse_node_id(master) else {
            return CommandResponse::err_json("invalid id".to_string());
        };
        let sizes: Vec<f64> = match serde_json::from_str(sizes_json) {
            Ok(sizes) => sizes,
            Err(error) => return CommandResponse::err_json(format!("invalid sizes: {error}")),
        };
        let sizes: Vec<f64> = sizes
            .into_iter()
            .filter(|size| *size >= 4.0 && *size <= 512.0)
            .collect();
        if sizes.is_empty() {
            return CommandResponse::err_json("give at least one size between 4 and 512");
        }
        let plan = match vectra_core::component::icon_set_plan(self.core.document(), master, &sizes)
        {
            Ok(plan) => plan,
            Err(error) => return CommandResponse::err_json(error.to_string()),
        };
        let command = Command::Batch { commands: plan };
        self.dispatch_side_command(&command)
    }

    /// **Outline a text node to paths** (Task 11.0 RULE 3): the non-destructive
    /// conversion from type to letterforms.
    ///
    /// Shaping lives in `vectra-geometry`, and `Command::OutlineText` carries a
    /// *plan* rather than a font, so the boundary is where the two meet: it
    /// lays the run out, asks the geometry crate for one closed plan per
    /// letterform, mints the group and letterform ids, and dispatches the command
    /// as **one history entry**. Core never learns what a glyph is; the sketch
    /// from the font never leaves this call.
    ///
    /// The original text node is hidden, never deleted — undo restores it
    /// exactly, and its string and parameters are still there to come back to.
    #[wasm_bindgen]
    pub fn outline_text(&mut self, node_id: &str, name: Option<String>) -> String {
        let Some(id) = parse_node_id(node_id) else {
            return CommandResponse::err_json("invalid id".to_string());
        };
        // Outline what the user *sees*: if the scene is cold (a fresh document,
        // a reload), lay it out first rather than refusing on a stale cache.
        {
            let Self {
                core,
                expressions,
                scene,
                motion,
                procedural,
                ..
            } = self;
            let ctx = core
                .evaluation_context()
                .with_expression(expressions)
                .with_motion(motion)
                .with_procedural(procedural)
                .with_fonts(&self.fonts);
            scene.refresh_if_cold(core.document(), &ctx);
        }
        let (text, node_name) = match self.core.document().nodes.get(&id) {
            Some(node) => match &node.kind {
                NodeKind::Text { text, .. } => (text.clone(), node.name.clone()),
                other => {
                    return CommandResponse::err_json(format!(
                        "only a text node can be outlined; {} is a {}",
                        node.name,
                        other.tag()
                    ))
                }
            },
            None => {
                return CommandResponse::err_json(format!("no node with id {id}"));
            }
        };
        let Some(evaluated) = self.scene.scene().get(id) else {
            return CommandResponse::err_json("the node has not been evaluated yet".to_string());
        };
        let EvaluatedPrimitive::Text(run) = &evaluated.primitive else {
            return CommandResponse::err_json(
                "the node has no laid-out run to outline".to_string(),
            );
        };
        if run.glyphs.is_empty() {
            return CommandResponse::err_json(
                "there is nothing to outline: the run is empty".to_string(),
            );
        }
        let plans = vectra_geometry::outline_plans(run, &text);
        if plans.is_empty() {
            return CommandResponse::err_json(
                "there is nothing to outline: the run produced no contours".to_string(),
            );
        }
        let paths: Vec<vectra_core::OutlinePath> = plans
            .into_iter()
            .map(|plan| vectra_core::OutlinePath {
                id: vectra_core::new_node_id(),
                name: plan.name,
                start: Parameter::Literal(plan.start),
                segments: plan.segments,
            })
            .collect();
        let command = Command::OutlineText {
            node_id: id,
            group_id: vectra_core::new_node_id(),
            // Default the group's name to the type's own, so the layers panel
            // reads "Headline" rather than "Outlined text" for a node the
            // designer already named.
            name: Some(name.unwrap_or(node_name)),
            paths,
        };
        self.dispatch_side_command(&command)
    }

    /// **The Smart Fill plan** (Task 12.0 RULE 1): the region graph of a set of
    /// paths, as JSON — see [`RegionPlanWire`].
    ///
    /// This is a *query*, not a command: it mutates nothing, and every consumer
    /// that needs to know which faces a set of paths makes asks here rather than
    /// computing a region of its own.
    ///
    /// `ids_json` names the boundaries. **An empty list means RULE 1's own
    /// sentence** — *"all selected or overlapping paths in the active layer"* —
    /// so the callers that have no opinion (the tool, the drop) pass `[]` and get
    /// the rule, while a panel or a test can still name a set. Either way a
    /// **group expands** to the shapes it holds: a group is a selection, not a
    /// boundary, and point-in-region against a group has no meaning of its own.
    /// An id with no evaluated geometry contributes no face.
    ///
    /// `point_json` is `null` — or the two numbers of a **drop point** (RULE 4's
    /// seed). With a point, `hit` is the face that contains it, or `null` when it
    /// falls in no face at all; the test is `RegionGraph::face_at`, which is the
    /// same smallest-face-wins answer a fill's own evaluation uses, so the
    /// highlight the designer sees and the region the fill adopts are one
    /// function, not two that agree today.
    #[wasm_bindgen]
    pub fn smart_fill_plan(&mut self, ids_json: &str, point_json: &str) -> String {
        let requested: Vec<String> = match serde_json::from_str(ids_json) {
            Ok(ids) => ids,
            Err(error) => {
                return CommandResponse::err_json(format!("invalid id list: {error}"))
            }
        };
        let point: Option<(f64, f64)> = match point_json.trim() {
            "" | "null" | "undefined" => None,
            text => match serde_json::from_str::<[f64; 2]>(text) {
                Ok([x, y]) => Some((x, y)),
                Err(error) => {
                    return CommandResponse::err_json(format!("invalid point: {error}"))
                }
            },
        };
        // An empty request is RULE 1's default: what the designer is working on.
        let context: Vec<NodeId> = if requested.is_empty() {
            self.editing_context()
        } else {
            Vec::new()
        };
        let mut ids: Vec<NodeId> = Vec::new();
        for id in context {
            if !ids.contains(&id) {
                ids.push(id);
            }
        }
        for text in &requested {
            let Some(id) = vectra_core::parse_node_id(text) else {
                continue;
            };
            let members: Vec<NodeId> = if Self::is_group(&self.core.document(), id) {
                self.core.document().node_block(id)
            } else {
                vec![id]
            };
            for member in members {
                if member == id || Self::is_group(&self.core.document(), member) {
                    continue;
                }
                if !ids.contains(&member) {
                    ids.push(member);
                }
            }
        }
        // What the user *sees*: lay the scene out first when it is cold, the
        // same way `outline_text` does.
        self.warm_scene();
        let graph = RegionGraph::from_scene(self.scene.scene(), &ids);
        let plan = RegionPlanWire {
            sources: graph
                .sources
                .iter()
                .map(|source| PlanSourceWire {
                    id: source.id.to_string(),
                    name: self.geometry_name(source.id),
                    area: vectra_geometry::region_area(&source.region),
                    bounds: {
                        let (min_x, min_y, max_x, max_y) = vectra_geometry::source_bounds(source);
                        [min_x, min_y, max_x, max_y]
                    },
                    spans: graph
                        .spans_of_source(source.id)
                        .into_iter()
                        .enumerate()
                        .map(|(index, span)| PlanSpanWire {
                            index,
                            from: span.from,
                            to: span.to,
                            ring: span.ring,
                            start: [span.start.0, span.start.1],
                            end: [span.end.0, span.end.1],
                            length: span.length,
                            total: span.total,
                        })
                        .collect(),
                })
                .collect(),
            regions: graph
                .faces
                .iter()
                .enumerate()
                .map(|(index, face)| PlanRegionWire {
                    index,
                    area: face.area,
                    holes: face.holes,
                    members: graph
                        .sources
                        .iter()
                        .zip(face.members.iter())
                        .filter(|(_, inside)| **inside)
                        .map(|(source, _)| source.id.to_string())
                        .collect(),
                    path: vectra_geometry::path_to_svg_data(&face.path),
                    // The same region again as coordinates, because the overlay
                    // draws it with the renderer's camera mapping
                    // (`documentToClient`) and never parses path data of its own.
                    rings: vectra_geometry::path_rings(&face.path)
                        .iter()
                        .map(|ring| ring.iter().map(|p| [p.x, p.y]).collect())
                        .collect(),
                })
                .collect(),
            crossings: graph
                .crossings
                .iter()
                .map(|crossing| [crossing.point.0, crossing.point.1])
                .collect(),
            hit: point.and_then(|p| graph.face_at(p)),
        };
        serde_json::to_string(&plan)
            .unwrap_or_else(|_| r#"{"sources":[],"regions":[],"crossings":[]}"#.to_string())
    }

    /// **Break a path at the intersections of its outline** (Task 12.0 RULE 3).
    ///
    /// `spans_json` is `[[from, to], …]` — arc lengths along the node's outline,
    /// exactly the numbers [`Self::smart_fill_plan`] reported, so the UI echoes
    /// what the engine measured instead of computing a cut of its own.
    ///
    /// **The spans are read as cut positions, and that is the whole algorithm.**
    /// A span's two ends *are* two crossings; collect the ends of every span the
    /// caller named, walk the ring between consecutive cuts, and each arc
    /// between them is one piece. Nothing about the count of pieces is special-
    /// cased, and both gestures fall out of the same rule:
    ///
    /// * one span → two cuts → **two** complementary arcs (the span, and the rest
    ///   of the ring) — "break this span";
    /// * every span of a path crossed twice → two distinct cuts → **two** arcs
    ///   that tile the ring — "Break Path at Intersections".
    ///
    /// A ring nothing crosses is not broken: it is carried over as a piece of its
    /// own, so every point of the outline ends up in exactly one piece. The
    /// pieces become ordinary closed `Path` nodes wearing the source's paint, and
    /// the source is hidden rather than deleted — one undo away, and the region
    /// graph that suggested the cut is still there to re-read.
    #[wasm_bindgen]
    pub fn break_path(&mut self, node_id: &str, spans_json: &str) -> String {
        let Some(id) = vectra_core::parse_node_id(node_id) else {
            return CommandResponse::err_json("invalid id".to_string());
        };
        let spans: Vec<(f64, f64)> = match serde_json::from_str(spans_json) {
            Ok(spans) => spans,
            Err(error) => {
                return CommandResponse::err_json(format!("invalid span list: {error}"))
            }
        };
        if spans.is_empty() {
            return CommandResponse::err_json(
                "there is nothing to break: no spans were given".to_string(),
            );
        }
        let is_path = matches!(
            self.core.document().nodes.get(&id).map(|node| &node.kind),
            Some(NodeKind::Path { .. })
        );
        if !is_path {
            return CommandResponse::err_json(
                "only a Path can be broken at its intersections".to_string(),
            );
        }
        self.warm_scene();
        let rings = vectra_geometry::source_rings(self.scene.scene(), id);
        if rings.is_empty() {
            return CommandResponse::err_json(
                "the path has no closed outline to break at".to_string(),
            );
        }
        // Ring-major arc bases: the plan's spans are outline-relative, and the
        // splitter works within one ring.
        let lengths: Vec<f64> = rings.iter().map(|ring| ring_arc_length(ring)).collect();
        let mut bases: Vec<f64> = Vec::with_capacity(lengths.len());
        let mut walked = 0.0;
        for length in &lengths {
            bases.push(walked);
            walked += *length;
        }
        // Where each ring is cut: the ends of every span the caller named, in
        // the ring's own arc lengths, deduplicated (a crossing that bounds two
        // spans is one cut) and sorted, so "between consecutive cuts" makes
        // sense. `TOLERANCE` is the same epsilon the geometry crate measures
        // crossings with — a cut half a tolerance from another is the same cut.
        const TOLERANCE: f64 = 1e-6;
        let mut cuts: Vec<Vec<f64>> = vec![Vec::new(); rings.len()];
        let ring_of = |position: f64, bases: &[f64], lengths: &[f64]| -> usize {
            bases
                .iter()
                .zip(lengths)
                .position(|(start, length)| {
                    position >= start - TOLERANCE && position <= start + length + TOLERANCE
                })
                .unwrap_or(0)
        };
        for (from, to) in &spans {
            for position in [*from, *to] {
                let index = ring_of(position, &bases, &lengths);
                let local = position - bases[index];
                if !cuts[index]
                    .iter()
                    .any(|existing| (existing - local).abs() <= TOLERANCE)
                {
                    cuts[index].push(local);
                }
            }
        }
        for ring_cuts in cuts.iter_mut() {
            ring_cuts.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        }
        let name = self.geometry_name(id);
        let mut pieces: Vec<vectra_core::OutlinePath> = Vec::new();
        for (index, ring) in rings.iter().enumerate() {
            if cuts[index].len() < 2 {
                // Nothing crosses this ring: it survives the break whole.
                if let Some(piece) = outline_piece(
                    &ring.iter().map(|p| (p.x, p.y)).collect::<Vec<_>>(),
                    &name,
                    pieces.len() + 1,
                ) {
                    pieces.push(piece);
                }
                continue;
            }
            let count = cuts[index].len();
            for position in 0..count {
                let from = cuts[index][position];
                let to = cuts[index][(position + 1) % count];
                let (arc, _) = vectra_geometry::ring_pieces_between(ring, from, to);
                let Some(piece) = outline_piece(&arc, &name, pieces.len() + 1) else {
                    continue;
                };
                pieces.push(piece);
            }
        }
        if pieces.is_empty() {
            return CommandResponse::err_json(
                "the spans produced no pieces: they are shorter than the tolerance".to_string(),
            );
        }
        self.dispatch_side_command(&Command::BreakPath {
            node_id: id,
            pieces,
        })
    }

    /// The families the host's font library can resolve, as a JSON array —
    /// the Text panel's picker. Always non-empty: the bundled face is in it.
    #[wasm_bindgen]
    pub fn font_families(&self) -> String {
        serde_json::to_string(&self.fonts.families()).unwrap_or_else(|_| "[]".to_string())
    }

    /// Register a face for a family name (Task 11.0 RULE 1), as raw bytes.
    ///
    /// Registration is *validating*: bytes that do not parse as a font are
    /// refused and nothing changes, so a bad upload can never become a text node
    /// that silently draws nothing.
    #[wasm_bindgen]
    pub fn register_font(&mut self, family: &str, bytes: &[u8]) -> String {
        match self.fonts.register(family, bytes.to_vec()) {
            Ok(()) => format!(r#"{{"status":"ok","families":{}}}"#, self.font_families()),
            Err(error) => CommandResponse::err_json(error.to_string()),
        }
    }

    /// The structural macros the command bar offers as one-tap chips (RULE 2):
    /// the prompt text, plus what it will do, in the designer's words.
    #[wasm_bindgen]
    pub fn structural_macros(&self) -> String {
        #[derive(serde::Serialize)]
        struct MacroWire {
            prompt: &'static str,
            label: &'static str,
            hint: &'static str,
        }
        let macros = [
            MacroWire {
                prompt: "make this geometric",
                label: "Make geometric",
                hint: "snap to whole numbers, square the corners, unify into one shape",
            },
            MacroWire {
                prompt: "create 4 color variations",
                label: "Color variations",
                hint: "four copies, one palette, offsets kept even",
            },
            MacroWire {
                prompt: "align perfectly",
                label: "Align perfectly",
                hint: "one column, even spacing, held by constraints",
            },
            MacroWire {
                prompt: "make this a component",
                label: "Create component",
                hint: "size, stroke, corners and colour become props",
            },
            MacroWire {
                prompt: "generate an icon set at 16 32 48",
                label: "Icon set",
                hint: "one artboard per size, stroke and corners scaled",
            },
        ];
        serde_json::to_string(&macros).unwrap_or_else(|_| "[]".to_string())
    }

    /// Run a command that a panel (not the canvas) built, through the same gate
    /// sequence as `dispatch_command`, and answer with the designer's sentence
    /// (RULE 4) instead of a raw event list.
    fn dispatch_side_command(&mut self, command: &Command) -> String {
        let json = match serde_json::to_string(command) {
            Ok(json) => json,
            Err(error) => return CommandResponse::err_json(format!("serialization: {error}")),
        };
        let response = self.dispatch_command(&json);
        // The envelope is a JSON object; add the prose rather than wrapping it,
        // so the panel has one shape to read whichever endpoint it called.
        let Ok(mut value) = serde_json::from_str::<serde_json::Value>(&response) else {
            return response;
        };
        let prose = vectra_ai::prose_for(std::slice::from_ref(command));
        if let Some(map) = value.as_object_mut() {
            map.insert("prose".to_string(), serde_json::json!(prose));
            map.insert("label".to_string(), serde_json::json!(command.label()));
            // What the command *made*, so the panel can select it without
            // knowing which id the engine minted: the node it created, or — for
            // a batch like the icon set — the last one it created.
            if let Some(created) = Self::created_by(command) {
                map.insert(
                    "created".to_string(),
                    serde_json::json!(created.to_string()),
                );
            }
        }
        value.to_string()
    }

    /// The id a command creates, if it creates one — the panel's "select what I
    /// just made". A batch reports the *last* id it creates: for the icon set
    /// that is the final instance, which is the one whose props the designer
    /// wants to see.
    fn created_by(command: &Command) -> Option<vectra_core::NodeId> {
        match command {
            Command::CreateNode { id, .. }
            | Command::CreateComponent { id, .. }
            | Command::InstantiateComponent { id, .. }
            | Command::DuplicateNode { id, .. } => Some(*id),
            Command::CreateArtboard { id, .. } => Some(*id),
            // An outline *creates* the letterform group: selecting it is what a
            // designer expects to happen after they outline a word.
            Command::OutlineText { group_id, .. } => Some(*group_id),
            // Task 12.0: a fill is an object the designer wants selected the
            // moment it exists (its paint is the next thing they set), and a
            // break's first piece is the piece whose spans they just read — one
            // namespace, one "select what I made".
            Command::CreateSmartFill { id, .. } => Some(*id),
            Command::BreakPath { pieces, .. } => pieces.first().map(|piece| piece.id),
            Command::Batch { commands } => commands.iter().rev().find_map(Self::created_by),
            _ => None,
        }
    }

    #[wasm_bindgen]
    pub fn procedural_json(&self) -> String {
        let doc = self.core.document();
        let nodes: Vec<ProceduralNodeWire> = doc
            .procedural
            .in_order()
            .map(|node| {
                let values = self.procedural.outputs_of(node.id);
                ProceduralNodeWire {
                    id: node.id.to_string(),
                    name: node.name.clone(),
                    kind: node.kind.tag().to_string(),
                    enabled: node.enabled,
                    description: node.describe(),
                    operands: node
                        .operands
                        .iter()
                        .map(|(port, value)| {
                            (
                                port.clone(),
                                ProceduralOperandWire {
                                    ty: node
                                        .kind
                                        .operand_type(port)
                                        .map(|ty| ty.tag().to_string())
                                        .unwrap_or_else(|| "scalar".to_string()),
                                    text: render_param_value(value, doc),
                                },
                            )
                        })
                        .collect(),
                    wires: node
                        .wires
                        .iter()
                        .map(|(port, from)| (port.clone(), format!("{}:{}", from.node, from.port)))
                        .collect(),
                    inputs: node
                        .kind
                        .inputs()
                        .iter()
                        .map(|port| ProceduralPortWire {
                            port: port.name.clone(),
                            ty: port.ty.tag().to_string(),
                            required: port.required,
                            wired: node.wires.contains_key(&port.name),
                            value: None,
                        })
                        .collect(),
                    outputs: node
                        .kind
                        .outputs()
                        .iter()
                        .map(|port| ProceduralPortWire {
                            port: port.name.clone(),
                            ty: port.ty.tag().to_string(),
                            required: false,
                            wired: false,
                            value: values
                                .and_then(|ports| ports.get(&port.name))
                                .map(geometry_summary),
                        })
                        .collect(),
                    upstream: node.upstream().iter().map(ToString::to_string).collect(),
                }
            })
            .collect();
        let report = ProceduralReportWire {
            count: nodes.len(),
            nodes,
            diagnostics: self
                .procedural
                .diagnostics()
                .iter()
                .map(SnapshotDiagnostic::from)
                .collect(),
        };
        serde_json::to_string(&report).unwrap_or_else(|e| format!("{{\"error\":{e:?}}}"))
    }

    /// Undo one step. Returns a [`CommandResponse`] JSON string.
    ///
    /// The command about to be applied (the previous entry's *inverse*) is
    /// dry-run first, so rewinding cannot introduce a cycle either.
    pub fn undo(&mut self) -> String {
        if let Some(drag) = &self.drag {
            return CommandResponse::err_json(VectraError::drag_active(drag.node_id).to_string());
        }
        if let Some(pending) = self.core.peek_undo() {
            if let Err(e) = gate_command(&self.graph, pending, self.core.document()) {
                return CommandResponse::err_json(e.to_string());
            }
        }
        let applied = self.core.peek_undo().cloned();
        // The inverses a drag/constraint pass folded into the entry are what
        // undo actually applies, so collect operation ids from both the entry
        // as stored and (below) the recorded command.
        if let Some(cmd) = &applied {
            for id in operation_ids_of(cmd) {
                if !self.pending_operations.contains(&id) {
                    self.pending_operations.push(id);
                }
            }
        }
        match self.core.undo() {
            Ok(events) => match applied {
                // Undo re-enforces the constraints the rewound state still has
                // to obey. It does not amend the stack: the entry that was
                // undone now lives on the redo stack and is re-derived there.
                Some(cmd) => match self.constraint_pass(&cmd, false) {
                    Ok(pass) => {
                        self.solver_diagnostics = pass.diagnostics;
                        let mut events = events;
                        events.extend(pass.events);
                        CommandResponse::ok_json(self.settle(events))
                    }
                    Err(e) => CommandResponse::err_json(constraint_error_message(e)),
                },
                None => CommandResponse::ok_json(self.settle(events)),
            },
            Err(e) => CommandResponse::err_json(e.to_string()),
        }
    }

    /// Redo one step. Returns a [`CommandResponse`] JSON string.
    pub fn redo(&mut self) -> String {
        if let Some(drag) = &self.drag {
            return CommandResponse::err_json(VectraError::drag_active(drag.node_id).to_string());
        }
        if let Some(pending) = self.core.peek_redo() {
            if let Err(e) = gate_command(&self.graph, pending, self.core.document()) {
                return CommandResponse::err_json(e.to_string());
            }
        }
        let applied = self.core.peek_redo().cloned();
        if let Some(cmd) = &applied {
            for id in operation_ids_of(cmd) {
                if !self.pending_operations.contains(&id) {
                    self.pending_operations.push(id);
                }
            }
        }
        match self.core.redo() {
            Ok(events) => match applied {
                Some(cmd) => match self.constraint_pass(&cmd, true) {
                    Ok(pass) => {
                        self.solver_diagnostics = pass.diagnostics;
                        let mut events = events;
                        events.extend(pass.events);
                        CommandResponse::ok_json(self.settle(events))
                    }
                    Err(e) => CommandResponse::err_json(constraint_error_message(e)),
                },
                None => CommandResponse::ok_json(self.settle(events)),
            },
            Err(e) => CommandResponse::err_json(e.to_string()),
        }
    }

    /// Scrub animation time: dirty from the clock vertex and re-evaluate its
    /// readers (Phase 4 motion rides this exact path).
    pub fn set_time(&mut self, t: f64) -> String {
        if let Some(drag) = &self.drag {
            return CommandResponse::err_json(VectraError::drag_active(drag.node_id).to_string());
        }
        self.core.set_time(t);
        // The clock can move geometry two ways: directly (a time-bound
        // parameter) and through anything that *reads* geometry (an operation
        // over a time-bound source). So the virtual layer is refreshed here
        // exactly as in `settle` — retire, patch the primitives, then re-run
        // the affected operations, and report every recomputed id as dirty.
        let mut recomputed = self.prune_disabled_operations();
        let dirty = self
            .graph
            .dirty_ids(&[vectra_dependency::GraphNode::clock()]);
        let report = self.patch(&dirty);
        recomputed.extend(self.run_operations(&report));
        let mut ids = report.dirty;
        for id in recomputed {
            if !ids.contains(&id) {
                ids.push(id);
            }
        }
        CommandResponse::ok_json(vec![self.dirty_event(ids, report.mode)])
    }

    // ── Task 6.0: the motion surface ────────────────────────────────────
    //
    // Three of these are *inputs*, not commands: a state flag, a pointer
    // position and the idle signal. They deliberately have no JSON-command
    // equivalent, because a state flag is not part of the document and must
    // never appear in the undo stack (State Law).

    /// Bind a spring to a slot, anchored at the slot's **current** value and the
    /// current clock (MES §12).
    ///
    /// The anchor is written *by the engine*, not guessed by the UI: only the
    /// engine can resolve the slot (it may be driven by a variable, an
    /// expression, a constraint or another binding right now), and `at` must be
    /// the engine's clock, not the browser's. The UI sends two numbers and a
    /// target; everything else is document state.
    ///
    /// Undoable — binding is an edit (the inverse restores whatever the slot
    /// held: a literal, a variable reference or a previous binding).
    #[wasm_bindgen]
    pub fn bind_spring(
        &mut self,
        node_id: &str,
        property: &str,
        target: f64,
        stiffness: f64,
        damping: f64,
    ) -> String {
        let Some(id) = parse_node_id(node_id) else {
            return CommandResponse::err_json(format!("invalid node id: {node_id}"));
        };
        // The anchor is the slot's *live* value (which may be a variable, an
        // expression, a constraint result or an earlier binding's output), so it
        // is resolved through the full context rather than the document alone.
        let Some(from) = self.resolve_live(id, property) else {
            return CommandResponse::err_json(format!(
                "cannot resolve {property} on {node_id} to bind a spring"
            ));
        };
        let binding = MotionBinding::spring(
            Parameter::Literal(target),
            stiffness,
            damping,
            from,
            self.core.time(),
        );
        let command = Command::BindMotion {
            node_id: id,
            property: property.to_string(),
            binding,
        };
        self.dispatch_command(&match command.to_json() {
            Ok(json) => json,
            Err(e) => return CommandResponse::err_json(e.to_string()),
        })
    }

    /// The hover demo, engine-side: a spring whose target is the `off`/`on` pair
    /// selected by the per-node state flag `hover:<node id>`.
    ///
    /// The flag is *derived from the document* here rather than sent by the UI,
    /// so the UI stays a dumb remote: it reports where the pointer is
    /// ([`VectraEngine::pointer_move`]) and never decides what that means.
    #[wasm_bindgen]
    pub fn bind_hover_spring(
        &mut self,
        node_id: &str,
        property: &str,
        off: f64,
        on: f64,
        stiffness: f64,
        damping: f64,
    ) -> String {
        let Some(id) = parse_node_id(node_id) else {
            return CommandResponse::err_json(format!("invalid node id: {node_id}"));
        };
        let binding = MotionBinding::Spring {
            target: Box::new(Parameter::Animated(MotionBinding::state(
                hover_flag(id),
                Parameter::Literal(on),
                Parameter::Literal(off),
            ))),
            stiffness,
            damping,
            from: off,
            at: self.core.time(),
        };
        let command = Command::BindMotion {
            node_id: id,
            property: property.to_string(),
            binding,
        };
        self.dispatch_command(&match command.to_json() {
            Ok(json) => json,
            Err(e) => return CommandResponse::err_json(e.to_string()),
        })
    }

    /// Register or replace a keyframe track (undoable document state).
    ///
    /// Takes the track as JSON so the wire schema lives in one place
    /// (`MotionTrack`'s own `Serialize`/`Deserialize`), exactly like commands.
    #[wasm_bindgen]
    pub fn set_motion_track(&mut self, track_json: &str) -> String {
        let track: MotionTrack = match serde_json::from_str(track_json) {
            Ok(track) => track,
            Err(e) => return CommandResponse::err_json(format!("invalid track: {e}")),
        };
        let command = Command::SetMotionTrack { track };
        self.dispatch_command(&match command.to_json() {
            Ok(json) => json,
            Err(e) => return CommandResponse::err_json(e.to_string()),
        })
    }

    /// Remove a track by id. Undoable; a slot still bound to it fails to resolve
    /// until the track returns, and that failure is visible in the event log
    /// rather than silently rendering a zero.
    #[wasm_bindgen]
    pub fn remove_motion_track(&mut self, track_id: &str) -> String {
        let command = Command::RemoveMotionTrack {
            track_id: track_id.to_string(),
        };
        self.dispatch_command(&match command.to_json() {
            Ok(json) => json,
            Err(e) => return CommandResponse::err_json(e.to_string()),
        })
    }

    /// Set a state flag — a host **input**, in the same category as the pointer
    /// position and the clock. Never undoable, never in the history.
    ///
    /// On a genuine *flip* (not a redundant re-write) every spring that reads the
    /// flag is re-anchored at the value it currently holds, which is what makes a
    /// hover transition animate: the spring starts from where it is *now* and
    /// eases to the branch's new target. The re-anchor is a session write (zero
    /// history entries: Undo Isolation Law) applied through the same
    /// `BindMotion` command a redo would replay.
    ///
    /// Returns the engine events for the flip (a `Dirty` event naming the
    /// re-anchored nodes, or nothing at all if the flag was already set).
    #[wasm_bindgen]
    pub fn set_state(&mut self, name: &str, on: bool) -> String {
        if self.motion.state(name) == on {
            return CommandResponse::ok_json(Vec::new());
        }
        let mut events = Vec::new();
        if let Err(message) = self.flip_state(name, on, &mut events) {
            return CommandResponse::err_json(message);
        }
        CommandResponse::ok_json(self.settle(events))
    }

    /// Report the pointer's document-space position (Task 6.0's hover driver).
    ///
    /// The engine asks the renderer's spatial index who is under the pointer,
    /// decides which nodes are hovered, and flips exactly their `hover:<id>`
    /// flags — the UI never learns which node is under the cursor unless it asks
    /// the snapshot, and never computes it. Returns the events for any flips
    /// (usually empty).
    ///
    /// The hit index describes the **last drawn** scene (spatial indexing is the
    /// renderer's, RULE 3), so a canvas that has never drawn has nothing to
    /// hover — which is exactly the mount order React uses: draw, then follow
    /// the pointer.
    #[wasm_bindgen]
    pub fn pointer_move(&mut self, renderer: &mut Renderer, x: f64, y: f64) -> String {
        if !self.scene.is_warm() {
            self.settle(Vec::new());
        }
        let hit = renderer.hit_test(x, y);
        let bound = self.hover_bound_nodes();
        if bound.is_empty() {
            return CommandResponse::ok_json(Vec::new());
        }
        let mut events = Vec::new();
        for (node_id, flag) in bound {
            let on = hit == Some(node_id);
            if self.motion.state(&flag) == on {
                continue;
            }
            if let Err(message) = self.flip_state(&flag, on, &mut events) {
                return CommandResponse::err_json(message);
            }
        }
        if events.is_empty() {
            return CommandResponse::ok_json(Vec::new());
        }
        CommandResponse::ok_json(self.settle(events))
    }

    /// The pointer left the canvas: nothing is hovered any more.
    ///
    /// A separate entry point rather than a magic off-canvas coordinate, because
    /// "the pointer is not on the canvas" is not a *place* — the UI would have to
    /// invent a document point to say it, and the engine would have to trust the
    /// invention.
    #[wasm_bindgen]
    pub fn pointer_leave(&mut self) -> String {
        let flags: Vec<String> = self
            .hover_bound_nodes()
            .into_iter()
            .map(|(_, f)| f)
            .collect();
        if flags.is_empty() {
            return CommandResponse::ok_json(Vec::new());
        }
        let mut events = Vec::new();
        for flag in flags {
            if let Err(message) = self.flip_state(&flag, false, &mut events) {
                return CommandResponse::err_json(message);
            }
        }
        if events.is_empty() {
            return CommandResponse::ok_json(Vec::new());
        }
        CommandResponse::ok_json(self.settle(events))
    }

    /// The document summary, resolved through this engine's live evaluators.
    /// Shared by the wasm method, the prompt builder and the AI host.
    fn document_summary_value(&self) -> DocumentSummary {
        let ctx = self
            .core
            .evaluation_context()
            .with_expression(&self.expressions)
            .with_motion(&self.motion)
            .with_procedural(&self.procedural)
            .with_fonts(&self.fonts);
        DocumentSummary::capture_selection_in(self.core.document(), &ctx, &self.selection.borrow())
    }

    /// A caller-supplied summary, or the live one when the caller sent nothing.
    fn ai_summary(&self, summary_json: &str) -> Result<DocumentSummary, AiError> {
        let trimmed = summary_json.trim();
        if trimmed.is_empty() || trimmed == "{}" || trimmed == "null" {
            return Ok(self.document_summary_value());
        }
        serde_json::from_str(trimmed).map_err(|error| AiError::InvalidJson {
            detail: format!("the summary is not a DocumentSummary: {error}"),
            reply: trimmed.to_string(),
        })
    }

    /// **The idle signal** (Frame Budget Law): `false` means the document is at
    /// rest at the current clock, so the React frame loop must stop scheduling
    /// `render_frame` until something moves again.
    #[wasm_bindgen]
    pub fn is_animating(&self) -> bool {
        let ctx = self
            .core
            .evaluation_context()
            .with_expression(&self.expressions)
            .with_motion(&self.motion)
            .with_procedural(&self.procedural)
            .with_fonts(&self.fonts);
        self.motion.is_animating(self.core.document(), &ctx)
    }

    /// Motion state for the inspector and the smoke harness: whether anything is
    /// moving, when it will stop, and what is bound where.
    #[wasm_bindgen]
    pub fn motion_json(&self) -> String {
        let ctx = self
            .core
            .evaluation_context()
            .with_expression(&self.expressions)
            .with_motion(&self.motion)
            .with_procedural(&self.procedural)
            .with_fonts(&self.fonts);
        let status = self.motion.status(self.core.document(), &ctx);
        let mut bindings = Vec::new();
        for node in self.core.document().nodes.values() {
            node.for_each_float_param(|property, param| {
                if let Some(binding) = param.motion() {
                    bindings.push(MotionBindingWire::of(node.id, property, binding, &ctx));
                }
            });
        }
        bindings.sort_by(|a, b| {
            a.node_id
                .cmp(&b.node_id)
                .then_with(|| a.property.cmp(&b.property))
        });
        let report = MotionReportWire {
            animating: status.animating,
            horizon: status.horizon,
            time: ctx.time,
            epsilon: self.motion.settle_epsilon(),
            states: self.motion.active_states(),
            tracks: self.core.document().motion.ids().to_vec(),
            bindings,
        };
        serde_json::to_string(&report).unwrap_or_else(|e| format!("{{\"error\":{e:?}}}"))
    }

    /// **One canvas frame** (Task 5.0 §6).
    ///
    /// The engine hands the renderer the evaluated scene plus the ids that
    /// changed since the last frame; the renderer tessellates what changed,
    /// writes only the affected buffer slices, and draws. React schedules this
    /// once per animation frame and never inspects a buffer.
    ///
    /// Returns the frame report as JSON — what the engine asked for (`dirty`,
    /// `full`) versus what the GPU actually paid (`writes`, `bytes`, `moved`,
    /// `restyled`). A cold scene is settled first, so the first frame after
    /// mount already draws the document as it stands.
    #[wasm_bindgen]
    pub fn render_frame(&mut self, renderer: &mut Renderer) -> String {
        if !self.scene.is_warm() {
            self.settle(Vec::new());
        }
        let dirty = self.render_dirty.take();
        let frame = renderer.present(self.scene.scene(), &dirty);
        renderer.frame_json(&frame)
    }

    /// Rebuild the whole scene from scratch (the UI's "re-evaluate everything"
    /// control). Demonstrably equal to the incremental cache — the app-level
    /// witness of `patch ≡ rebuild`.
    pub fn force_full_evaluation(&mut self) -> String {
        let report = {
            let Self {
                core,
                expressions,
                scene,
                motion,
                procedural,
                ..
            } = self;
            scene.invalidate();
            let ctx = core
                .evaluation_context()
                .with_expression(expressions)
                .with_motion(motion)
                .with_procedural(procedural)
                .with_fonts(&self.fonts);
            scene.refresh_full(core.document(), &ctx)
        };
        // A full rebuild recomputes the virtual layer too, so `patch ≡ rebuild`
        // still holds with operations in the scene (Task 4.0) — and with a
        // procedural graph (Task 7.0): the pass re-runs every node, and the
        // slots that read a port are re-read against the fresh table.
        self.pending_operations = self.core.document().operations.order.clone();
        let recomputed = self.run_operations(&report);
        let touched = self.run_procedural_fixpoint(&report, &recomputed);
        let mut ids = report.dirty;
        for id in recomputed.into_iter().chain(touched.iter().copied()) {
            if !ids.contains(&id) {
                ids.push(id);
            }
        }
        CommandResponse::ok_json(vec![EngineEvent::Dirty {
            ids,
            mode: report.mode,
        }])
    }
}

impl VectraEngine {
    /// Fill in an operand a constraint kind is allowed to capture.
    ///
    /// `Distance`, `Parallel` and `Angle` may be registered without a value,
    /// meaning *"hold exactly as it is right now"*; the value is resolved here,
    /// before dispatch, so the document stores a fully-specified rule that the
    /// inspector can show and the solver can replay deterministically.
    fn capture_operands(&self, command: &mut Command) -> Result<(), String> {
        let Command::AddConstraint { constraint } = command else {
            return Ok(());
        };
        if constraint.value.is_some() || !rows::captures_value(constraint.kind) {
            return Ok(());
        }
        let ctx = self
            .core
            .evaluation_context()
            .with_expression(&self.expressions)
            .with_motion(&self.motion)
            .with_procedural(&self.procedural)
            .with_fonts(&self.fonts);
        match capture_value(self.core.document(), &ctx, constraint) {
            Ok(value) => {
                constraint.value = Some(value);
                Ok(())
            }
            Err(e) => Err(constraint_error_message(e)),
        }
    }

    /// The solver pass: enforce the constraint system on the document the
    /// command just produced.
    ///
    /// Order inside one dispatch:
    ///
    /// 1. `plan` decides which rules are in force (drops + skips);
    /// 2. `solve` returns the slots to move, the rules to disable and the links
    ///    it has to break — it never mutates anything;
    /// 3. this method applies those changes with `Engine::apply_untracked`, so
    ///    every write has an exact inverse;
    /// 4. when `amend` is set, the inverses are folded into the history entry of
    ///    the command that caused them (one user action = one undo).
    ///
    /// Read side: values are resolved through the ordinary resolution path, so a
    /// constraint sees `$variables` and `ƒexpressions` exactly as the geometry
    /// evaluator does.
    fn constraint_pass(
        &mut self,
        applied: &Command,
        amend: bool,
    ) -> Result<ConstraintPass, ConstraintError> {
        if self.core.document().constraints.is_empty() && self.solver.variable_count() == 0 {
            // No rules and an empty variable pool: the pass costs two integer
            // comparisons and nothing else. (The second half matters: after the
            // last rule is removed the pool must still be released, which is
            // what the undo law observes.)
            return Ok(ConstraintPass::default());
        }

        let hints = hints_for(applied);
        let outcome = {
            let Self {
                core,
                expressions,
                solver,
                motion,
                procedural,
                ..
            } = self;
            let ctx = core
                .evaluation_context()
                .with_expression(expressions)
                .with_motion(motion)
                .with_procedural(procedural)
                .with_fonts(&self.fonts);
            solver.solve(core.document(), &ctx, &hints)?
        };

        let mut pass = ConstraintPass::default();

        // 1. Losers leave the active set (undoably: the flag is part of history).
        //    (A gesture pins these inverses into its own entry — see `end_drag`.)
        let (events, inverses) = self.apply_drops(&outcome.dropped, &mut pass.diagnostics)?;
        pass.events.extend(events);
        pass.inverses.extend(inverses);
        pass.disabled = outcome.dropped.iter().map(|dropped| dropped.id).collect();

        // 2. Rules that cannot be read this pass are reported, not fatal.
        self.report_skips(&outcome.skipped, &mut pass.diagnostics);

        // 3. The solved geometry: ordinary SetParameter writes, so each one has
        //    an exact inverse, dirties its node and re-derives the graph.
        for write in &outcome.writes {
            if write.breaks_link() {
                pass.diagnostics.push(link_broken_diagnostic(write));
            }
            let inverse = self.apply_write(write)?;
            pass.inverses.push(inverse);
            pass.events.push(EngineEvent::NodesUpdated {
                ids: vec![write.target.node_id],
            });
        }
        pass.writes = outcome.writes;

        // 4. One user action, one undo: the solver's writes belong to the command
        //    that triggered them.
        if amend && !pass.inverses.is_empty() {
            let mut inverses = pass.inverses.clone();
            inverses.reverse();
            self.core
                .amend_top_backward(Command::batch_commands(inverses));
        }

        Ok(pass)
    }

    /// Apply one solved write as an ordinary `SetParameter`, returning its
    /// exact inverse (which restores the slot's *parameter*, links included).
    fn apply_write(&mut self, write: &SolveWrite) -> Result<Command, VectraError> {
        self.core.apply_untracked(Command::SetParameter {
            node_id: write.target.node_id,
            property: write.target.property.clone(),
            value: ParamValue::Float(Parameter::Literal(write.value)),
        })
    }

    /// Resolve a slot **the way the evaluator will**, motion and all.
    ///
    /// `Engine::resolve_float` builds a context from the document alone, which is
    /// right for a slot driven by a literal, a variable or an expression — and
    /// wrong for an `Animated` one: with no motion evaluator wired, core falls
    /// back to the *static preview* (a spring previews at its target, a state
    /// branch at its `false` arm). Re-anchoring on that number would anchor every
    /// transition at the value it is already animating toward, which is exactly
    /// the "nothing ever moves" bug the hover law caught.
    fn resolve_live(&self, node_id: NodeId, property: &str) -> Option<f64> {
        let node = self.core.document().get_node(node_id).ok()?;
        let Ok(ParamValue::Float(param)) = node.get_param(property) else {
            return None;
        };
        let ctx = self
            .core
            .evaluation_context()
            .with_expression(&self.expressions)
            .with_motion(&self.motion)
            .with_procedural(&self.procedural)
            .with_fonts(&self.fonts);
        param.resolve(&ctx).ok()
    }

    /// Flip a state flag and re-anchor every spring that reads it.
    ///
    /// Ordering matters and is the whole trick: the *current values are read
    /// first*, while the old flag still applies, because that is the position the
    /// spring is at when the state changes. Reading after the flip would anchor
    /// every transition at the value it is animating *toward*, and nothing would
    /// ever move.
    fn flip_state(
        &mut self,
        name: &str,
        on: bool,
        events: &mut Vec<EngineEvent>,
    ) -> Result<(), String> {
        let now = self.core.time();
        let mut plan: Vec<(NodeId, String, MotionBinding)> = Vec::new();
        for node in self.core.document().nodes.values() {
            let mut slots: Vec<(String, MotionBinding)> = Vec::new();
            node.for_each_float_param(|property, param| {
                if param.motion_state_flags().contains(&name) {
                    if let Some(binding) = own_spring(param) {
                        slots.push((property.to_string(), binding.clone()));
                    }
                }
            });
            for (property, binding) in slots {
                let Some(current) = self.resolve_live(node.id, &property) else {
                    continue;
                };
                plan.push((node.id, property, reanchor(&binding, current, now)));
            }
        }
        self.motion.set_state(name, on);
        for (node_id, property, binding) in plan {
            self.core
                .apply_untracked(Command::BindMotion {
                    node_id,
                    property,
                    binding,
                })
                .map_err(|e| e.to_string())?;
            events.push(EngineEvent::NodesUpdated { ids: vec![node_id] });
        }
        Ok(())
    }

    /// Every node with a `hover:<id>` flag bound somewhere, with the flag name.
    ///
    /// The **flag** names the node the pointer must be over, and that is what is
    /// trusted — a flag found on one node could in principle have been written by
    /// a binding on another (a copy/paste of a spring between shapes), and the
    /// hover question is "is the pointer over *this* node?", which only the flag
    /// can answer.
    fn hover_bound_nodes(&self) -> Vec<(NodeId, String)> {
        let mut out: Vec<(NodeId, String)> = Vec::new();
        for node in self.core.document().nodes.values() {
            let mut flags: Vec<String> = Vec::new();
            node.for_each_float_param(|_, param| {
                for flag in param.motion_state_flags() {
                    if flag.starts_with(HOVER_PREFIX) && !flags.iter().any(|f| f == flag) {
                        flags.push(flag.to_string());
                    }
                }
            });
            for flag in flags {
                if let Some(id) = parse_node_id(&flag[HOVER_PREFIX.len()..]) {
                    out.push((id, flag));
                }
            }
        }
        out.sort();
        out.dedup();
        out
    }

    /// Park the constraints the plan dropped: they leave the active set
    /// (undoably) and each one gets a diagnostic naming the rule that won.
    fn apply_drops(
        &mut self,
        dropped: &[Dropped],
        diagnostics: &mut Vec<Diagnostic>,
    ) -> Result<(Vec<EngineEvent>, Vec<Command>), VectraError> {
        let mut events = Vec::new();
        let mut inverses = Vec::new();
        for dropped in dropped {
            if let Some(target) = first_target_of(dropped.id, self.core.document()) {
                diagnostics.push(Diagnostic::constraint_dropped(
                    target.node_id,
                    target.property.clone(),
                    format!(
                        "{} [{}] was dropped: {} [{}] at {} strength already pins that row ({})",
                        dropped.kind.tag(),
                        short_id(dropped.id),
                        dropped.kept_kind.tag(),
                        short_id(dropped.kept),
                        dropped.kept_strength.tag(),
                        dropped.kept_targets,
                    ),
                ));
            }
            let inverse = self.core.apply_untracked(Command::SetConstraintEnabled {
                id: dropped.id,
                enabled: false,
            })?;
            inverses.push(inverse);
            events.push(EngineEvent::ConstraintsUpdated {
                ids: vec![dropped.id],
            });
        }
        Ok((events, inverses))
    }

    /// Report the rules that could not be enforced this pass.
    fn report_skips(
        &self,
        skipped: &[vectra_constraints::Skipped],
        diagnostics: &mut Vec<Diagnostic>,
    ) {
        for skipped in skipped {
            if let Some(target) = first_target_of(skipped.id, self.core.document()) {
                diagnostics.push(Diagnostic::constraint_skipped(
                    target.node_id,
                    target.property.clone(),
                    format!(
                        "{} [{}] on {} was not enforced this pass: {}",
                        skipped.kind.tag(),
                        short_id(skipped.id),
                        target.property,
                        skipped.reason,
                    ),
                ));
            }
        }
    }

    // ── Task 3.2: the drag protocol ──────────────────────────────────────

    /// `BeginDrag`: open the solver session and start a history transaction.
    ///
    /// Nothing is recorded yet — a gesture that moves nothing (a click) must not
    /// cost an undo step. The document only changes if the opening solve had to
    /// enforce something, and those writes are tracked like any other sample.
    fn begin_drag(&mut self, node_id: NodeId) -> String {
        if let Some(drag) = &self.drag {
            return CommandResponse::err_json(VectraError::drag_active(drag.node_id).to_string());
        }
        let name = match self.core.document().get_node(node_id) {
            Ok(node) => node.name.clone(),
            Err(error) => return CommandResponse::err_json(error.to_string()),
        };

        let outcome = {
            let Self {
                core,
                expressions,
                solver,
                motion,
                procedural,
                ..
            } = self;
            let ctx = core
                .evaluation_context()
                .with_expression(expressions)
                .with_motion(motion)
                .with_procedural(procedural)
                .with_fonts(&self.fonts);
            solver.begin_drag(core.document(), &ctx, node_id)
        };
        let outcome = match outcome {
            Ok(outcome) => outcome,
            Err(error) => {
                return CommandResponse::err_json(drag_error_message(error, node_id).to_string())
            }
        };

        let mut txn = DragTxn::new(node_id, format!("Drag {name}"));
        let mut events = vec![EngineEvent::DragStarted { node_id }];
        // A gesture starts on the geometry as it is; if the system was not yet
        // satisfied, the opening solve moved what it had to.
        match self.drag_writes(&outcome.writes, &mut txn) {
            Ok((write_events, diagnostics)) => {
                events.extend(write_events);
                txn.merge_diagnostics(diagnostics);
            }
            Err(error) => return self.abort_drag(txn, error),
        }
        // A drop cannot normally happen here (the registry has not changed since
        // the last pass), but if the plan reports one the gesture must own it.
        match self.apply_drops(&outcome.dropped, &mut txn.diagnostics) {
            Ok((drop_events, _inverses)) => {
                events.extend(drop_events);
                for dropped in &outcome.dropped {
                    txn.park(dropped.id);
                }
            }
            Err(error) => return self.abort_drag(txn, error),
        }
        let mut skip_diagnostics = Vec::new();
        self.report_skips(&outcome.skipped, &mut skip_diagnostics);
        txn.merge_diagnostics(skip_diagnostics);

        self.solver_diagnostics = txn.diagnostics.clone();
        self.drag = Some(txn);
        CommandResponse::ok_json(self.settle(events))
    }

    /// `UpdateDrag`: one pointer sample through `suggest_value`.
    fn update_drag(&mut self, node_id: NodeId, x: f64, y: f64) -> String {
        let Some(drag) = &self.drag else {
            return CommandResponse::err_json(VectraError::drag_not_active(node_id).to_string());
        };
        if drag.node_id != node_id {
            return CommandResponse::err_json(VectraError::drag_not_active(node_id).to_string());
        }

        let outcome = {
            let Self {
                core,
                expressions,
                solver,
                motion,
                procedural,
                ..
            } = self;
            let ctx = core
                .evaluation_context()
                .with_expression(expressions)
                .with_motion(motion)
                .with_procedural(procedural)
                .with_fonts(&self.fonts);
            solver.update_drag(core.document(), &ctx, x, y)
        };
        let outcome = match outcome {
            Ok(outcome) => outcome,
            Err(error) => {
                let txn = self.drag.take().expect("checked above");
                return self.abort_drag(txn, drag_error_message(error, node_id));
            }
        };

        let mut txn = self.drag.take().expect("checked above");
        match self.drag_writes(&outcome.writes, &mut txn) {
            Ok((events, diagnostics)) => {
                txn.updates += 1;
                txn.merge_diagnostics(diagnostics);
                self.solver_diagnostics = txn.diagnostics.clone();
                self.drag = Some(txn);
                CommandResponse::ok_json(self.settle(events))
            }
            Err(error) => self.abort_drag(txn, error),
        }
    }

    /// `EndDrag`: release the edits, re-assert the rules, record **one** entry.
    fn end_drag(&mut self, node_id: NodeId) -> String {
        let Some(drag) = &self.drag else {
            return CommandResponse::err_json(VectraError::drag_not_active(node_id).to_string());
        };
        if drag.node_id != node_id {
            return CommandResponse::err_json(VectraError::drag_not_active(node_id).to_string());
        }
        let mut txn = self.drag.take().expect("checked above");

        // 1. Explicitly release the pointer's edit variables, then drop the
        //    tableau (the registry must be observably empty afterwards).
        if let Err(error) = self.solver.end_drag(self.core.document()) {
            return self.abort_drag(txn, drag_error_message(error, node_id));
        }

        // 2. Re-assert the rules with no hints: this is the strength-based
        //    snap-back (a rule weaker than the pointer but stronger than a stay
        //    re-takes the slot) and it re-solves whatever the gesture left.
        let pass = match self.constraint_pass(&Command::EndDrag { node_id }, false) {
            Ok(pass) => pass,
            Err(error) => return self.abort_drag(txn, constraint_error_message(error)),
        };
        txn.record_writes(&pass.writes);
        txn.merge_diagnostics(pass.diagnostics.clone());

        // 3. One gesture, one entry. Forward = the net movement; backward = the
        //    pre-gesture state *plus* the inverse of anything the final pass
        //    parked (writes are already represented by `first_source`, which
        //    holds the earliest parameter seen for each slot).
        for id in &pass.disabled {
            txn.park(*id);
        }
        let mut events = vec![EngineEvent::DragEnded { node_id }];
        events.extend(pass.events.clone());

        // 3.5 Interaction Precedence (Task 6.0). Slots that were animated by a
        //     spring go back to being animated, re-anchored at the committed
        //     value: the drag *won* while it lasted, and motion resumes from
        //     where it left the slot rather than being silently deleted. Applied
        //     untracked here (the gesture's own entry, below, is the history
        //     record) through the same `BindMotion` command redo will replay.
        let now = self.core.time();
        for (target, property, binding) in txn.reanchor_plan(now) {
            // `apply_untracked` returns the inverse command, which is
            // deliberately dropped: this write is session state (the gesture's
            // entry below carries the real inverse), and the event below is what
            // drives the dirty set through `settle`.
            if let Err(error) = self.core.apply_untracked(Command::BindMotion {
                node_id: target,
                property,
                binding,
            }) {
                return self.abort_drag(txn, error.to_string());
            }
            events.push(EngineEvent::NodesUpdated { ids: vec![target] });
        }

        if !txn.last_value.is_empty() || !txn.parked.is_empty() {
            let stack_event = self
                .core
                .record(txn.forward(now), txn.backward(), txn.label.clone());
            events.push(stack_event);
        }

        self.solver_diagnostics = txn.diagnostics;
        CommandResponse::ok_json(self.settle(events))
    }

    /// Apply the writes one drag sample (or the gesture's opening solve)
    /// produced, remembering the pre-gesture parameter of every slot it touches.
    fn drag_writes(
        &mut self,
        writes: &[SolveWrite],
        txn: &mut DragTxn,
    ) -> Result<(Vec<EngineEvent>, Vec<Diagnostic>), VectraError> {
        let mut events = Vec::new();
        let mut diagnostics = Vec::new();
        for write in writes {
            txn.record_one(write);
            if write.breaks_link() {
                diagnostics.push(link_broken_diagnostic(write));
            }
            self.apply_write(write)?;
            events.push(EngineEvent::NodesUpdated {
                ids: vec![write.target.node_id],
            });
        }
        Ok((events, diagnostics))
    }

    /// Tear a gesture down without recording it: restore every slot it wrote,
    /// release the edits, and report the failure. Nothing half-applied survives.
    fn abort_drag(&mut self, txn: DragTxn, error: impl std::fmt::Display) -> String {
        let restores: Vec<(NodeId, String, ParamValue)> = txn
            .last_value
            .keys()
            .filter_map(|label| txn.first_source.get(label).cloned())
            .collect();
        for (node_id, property, value) in restores.into_iter().rev() {
            let _ = self.core.apply_untracked(Command::SetParameter {
                node_id,
                property,
                value,
            });
        }
        let _ = self.solver.end_drag(self.core.document());
        self.solver_diagnostics.clear();
        CommandResponse::err_json(error.to_string())
    }

    /// Canonical positions for every draggable node (Task 3.2) — the UI's drag
    /// handles and grab-offset source. Resolved through the ordinary path, so a
    /// slot driven by `$variable` / `ƒexpression` reports its current value and
    /// says how it is driven.
    fn positions(&self) -> BTreeMap<String, SnapshotPosition> {
        let doc = self.core.document();
        let ctx = self
            .core
            .evaluation_context()
            .with_expression(&self.expressions)
            .with_motion(&self.motion)
            .with_procedural(&self.procedural)
            .with_fonts(&self.fonts);
        let mut positions = BTreeMap::new();
        for (id, node) in &doc.nodes {
            let Some((x_slot, y_slot)) = node.position_slots() else {
                continue;
            };
            let (Ok(ParamValue::Float(x_param)), Ok(ParamValue::Float(y_param))) =
                (node.get_param(x_slot), node.get_param(y_slot))
            else {
                continue;
            };
            let (Ok(x), Ok(y)) = (x_param.resolve(&ctx), y_param.resolve(&ctx)) else {
                continue;
            };
            positions.insert(
                id.to_string(),
                SnapshotPosition {
                    x,
                    y,
                    x_source: x_param.source_tag().to_string(),
                    y_source: y_param.source_tag().to_string(),
                },
            );
        }
        positions
    }

    /// The typography inspector's rows by node id (Task 11.0) — the text twin
    /// of [`Self::positions`]: resolved through the ordinary parameter door (so
    /// a slot driven by `$variable` / `ƒexpression` / a spring reports its
    /// current value) and carrying each slot's source tag, so the panel can warn
    /// before a slider drag breaks a parametric link.
    fn texts(&self) -> BTreeMap<String, SnapshotText> {
        let doc = self.core.document();
        let ctx = self
            .core
            .evaluation_context()
            .with_expression(&self.expressions)
            .with_motion(&self.motion)
            .with_procedural(&self.procedural)
            .with_fonts(&self.fonts);
        let mut texts = BTreeMap::new();
        for (id, node) in &doc.nodes {
            let NodeKind::Text {
                text,
                font_family,
                font_size,
                letter_spacing,
                line_height,
                alignment,
                on_path,
                ..
            } = &node.kind
            else {
                continue;
            };
            // A slot that cannot resolve has no row: the panel then falls back
            // to the command surface rather than showing a stale number as if
            // it were current.
            let (Ok(size), Ok(spacing), Ok(leading)) = (
                font_size.resolve(&ctx),
                letter_spacing.resolve(&ctx),
                line_height.resolve(&ctx),
            ) else {
                continue;
            };
            let (bound_to, offset, offset_source) = match on_path {
                Some(binding) => (
                    Some(binding.node.to_string()),
                    binding.offset.resolve(&ctx).ok(),
                    Some(binding.offset.source_tag().to_string()),
                ),
                None => (None, None, None),
            };
            texts.insert(
                id.to_string(),
                SnapshotText {
                    text: text.clone(),
                    font_family: font_family.clone(),
                    alignment: alignment.tag().to_string(),
                    font_size: size,
                    font_size_source: font_size.source_tag().to_string(),
                    letter_spacing: spacing,
                    letter_spacing_source: letter_spacing.source_tag().to_string(),
                    line_height: leading,
                    line_height_source: line_height.source_tag().to_string(),
                    bound_to,
                    offset,
                    offset_source,
                },
            );
        }
        texts
    }

    /// Post-mutation protocol: reunite the compiled registry, reconcile the
    /// dependency graph, propagate dirty ids, patch the scene, and report what
    /// it cost as a `Dirty` event.
    fn settle(&mut self, mut events: Vec<EngineEvent>) -> Vec<EngineEvent> {
        self.expressions.sync_from_document(self.core.document());
        self.motion.sync_from_document(self.core.document());
        self.graph.sync(self.core.document());
        // The procedural registry is adopted before the pass; its sync report
        // names the records that changed — those are seeds, not guesses.
        self.procedural.sync_from_document(self.core.document());
        // Task 4.0, first step: retire virtual geometry whose registry entry is
        // gone or parked. This runs *before* the primitive pass because the
        // scene cache asserts that every cached id is live geometry, and a
        // removed operation is not — the command that removed it may not have
        // dirtied any primitive at all.
        let mut retired = self.prune_disabled_operations();
        // **Task 10.7 RULE 3b, first half**: a node that was clipped and no
        // longer is has to be asked for the *document's* geometry again — its
        // primitive in the cache is our derivative, and dropping the mask must
        // not be the same thing as deleting the artwork. The evaluator does the
        // restoring; we only say which ids lost their mask.
        let restored = self.clip.pending_restores(self.core.document());
        let mut dirty = self.graph.dirty_ids_for_events(&events);
        for id in restored {
            if !dirty.contains(&id) {
                dirty.push(id);
            }
        }
        // **Task 11.0 RULE 2**: a run bound to a path has no graph edge to the
        // path's *geometry* — the binding is a geometry-to-geometry reference and
        // the graph is a graph of slots. The propagation step is therefore a
        // registry scan seeded by the dirty set, exactly as it is for the
        // operations pass and the procedural `Source` pass: reshape the circle
        // and the text that rides it is dirtied here, in the same settle.
        for id in self.core.document().text_nodes_bound_to(&dirty) {
            if !dirty.contains(&id) {
                dirty.push(id);
            }
        }
        let report = self.patch(&dirty);
        // **RULE 4**: an eye or a padlock is a renderer-side flag. The evaluator
        // never heard about it (the four presentation events produce no dirty
        // ids — see `dirty_ids_for_events`), so the cached scene's flags are
        // re-derived here for one `bool` per node, and the ids that actually
        // changed are handed to the renderer's ledger. Zero parameters resolved,
        // zero geometry rebuilt, zero bytes written — and the picture updates on
        // the next frame all the same.
        self.repaint_flags();
        // The operations pass runs *after* the primitive pass, so a boolean
        // always reads this edit's geometry, never the previous frame's.
        let recomputed = self.run_operations(&report);
        retired.extend(recomputed.iter().copied());
        // **RULE 2**: the procedural pass runs last — after the primitives (so a
        // slot read is as fresh as the table allows) and after the operations
        // (so a `Source` node can read a boolean result rather than the shapes
        // it replaced). It seeds on the graph's dirty ids *plus* the operations
        // that just recomputed, because a `Source` that reads a boolean has no
        // graph edge to notice it.
        retired.extend(self.run_procedural_fixpoint(&report, &recomputed));
        // **Task 10.7 RULE 3b, second half**: the clip pass runs last — after the
        // flags were re-derived (so a mask is what is on the canvas, not last
        // frame's eye state) and after every pass that can write geometry (so a
        // clipped node is clipped whatever composed it). The ids it rewrote join
        // the event, because the renderer's ledger is the only thing that knows
        // this happened: the evaluator resolved no parameter for them.
        let clipped = self.clip_layers(&report.dirty);
        let mut ids = report.dirty;
        for id in retired {
            if !ids.contains(&id) {
                ids.push(id);
            }
        }
        for id in clipped {
            if !ids.contains(&id) {
                ids.push(id);
            }
        }
        ids.sort();
        let event = self.dirty_event(ids, report.mode);
        events.push(event);
        events
    }

    /// **The clipping-mask pass** (Task 10.7 RULE 3b).
    ///
    /// Rewrites every node on a clipping layer to `region(node) ∩ region(layer
    /// below)` — from the node's *baseline* geometry, refreshed only for the ids
    /// the evaluator just re-derived (`reevaluated`), so a live mask masks rather
    /// than shrinks. Returns the ids whose appearance changed.
    ///
    /// Cheap to skip: with no clipping layer in the document and nothing left
    /// over from one, the pass does not walk the scene at all.
    fn clip_layers(&mut self, reevaluated: &[NodeId]) -> Vec<NodeId> {
        // Field-level borrows, the way `patch` does it: the clip pass needs the
        // document *and* the scene at once, and neither is a copy.
        let Self {
            core,
            scene,
            clip,
            render_dirty,
            ..
        } = self;
        if clip.is_idle(core.document()) {
            return Vec::new();
        }
        let report = clip.apply(scene.scene_mut(), core.document(), reevaluated);
        if !report.changed.is_empty() {
            render_dirty.note(report.changed.clone(), vectra_core::EvalMode::Incremental);
        }
        report.changed
    }

    /// Re-derive the presentation flags of the cached scene and tell the
    /// renderer which nodes changed appearance (Task 10.2 RULE 4).
    ///
    /// Deliberately **not** part of the `Dirty` event: that event reports what
    /// the *evaluator* did, and the honest answer here is "nothing" — the eval
    /// panel showing a no-op while the eye icon works is the proof the rule is
    /// kept. The renderer's ledger is the flag's real destination.
    fn repaint_flags(&mut self) {
        let changed = self
            .scene
            .scene_mut()
            .refresh_presentation(self.core.document());
        if !changed.is_empty() {
            self.render_dirty
                .note(changed, vectra_core::EvalMode::Incremental);
        }
    }

    /// Build a `Dirty` event **and** hand the same ids to the renderer's ledger.
    ///
    /// Every dirty set the engine publishes goes through here, which is why the
    /// canvas cannot miss one: commands, undo/redo, the clock, drag samples and
    /// a full rebuild all end at this function. The ledger coalesces them until
    /// the next drawn frame (see [`DirtyLedger`]).
    fn dirty_event(&mut self, ids: Vec<NodeId>, mode: EvalMode) -> EngineEvent {
        self.render_dirty.note(ids.iter().copied(), mode);
        EngineEvent::Dirty { ids, mode }
    }

    /// Lay the scene out if it has never been evaluated (or if the last pass
    /// was not a full one). The read-only queries call this before they answer,
    /// so a fresh document answers a question about geometry it actually has.
    fn warm_scene(&mut self) {
        let Self {
            core,
            expressions,
            scene,
            motion,
            procedural,
            ..
        } = self;
        let ctx = core
            .evaluation_context()
            .with_expression(expressions)
            .with_motion(motion)
            .with_procedural(procedural)
            .with_fonts(&self.fonts);
        scene.refresh_if_cold(core.document(), &ctx);
    }

    /// **Which paths RULE 1 means by "the paths"**: the selection when there is
    /// one, else the shapes of the active layer.
    ///
    /// Selection-first is the rule's own order ("selected or overlapping"), and
    /// it is also what makes the tool usable: select two circles, and the region
    /// graph is theirs — not the whole layer's. With nothing selected, the active
    /// layer is the drawing surface the designer is looking at, which is the only
    /// other honest answer. Groups are skipped: a group is not a boundary.
    fn editing_context(&self) -> Vec<NodeId> {
        let doc = self.core.document();
        let shapes = |ids: &[NodeId]| -> Vec<NodeId> {
            ids.iter()
                .copied()
                .filter(|id| {
                    doc.nodes
                        .get(id)
                        .is_some_and(|node| !matches!(node.kind, NodeKind::Group { .. }))
                })
                .collect()
        };
        let selected = shapes(&self.selection.borrow());
        if !selected.is_empty() {
            return selected;
        }
        let Some(layer) = doc.active_layer() else {
            return Vec::new();
        };
        // Top-level children only: a group's members are already inside their
        // group, and a nested shape would be counted twice.
        let members: Vec<NodeId> = doc
            .layers
            .get(&layer)
            .map(|record| record.children.clone())
            .unwrap_or_default();
        shapes(&members)
    }

    /// Is this id a group record?
    fn is_group(doc: &vectra_core::Document, id: NodeId) -> bool {
        matches!(
            doc.nodes.get(&id).map(|node| &node.kind),
            Some(NodeKind::Group { .. })
        )
    }

    /// The display name of any live id — an authored node, an operation result
    /// or a procedural node. One reader, so the plan, the panel and the event log
    /// name the same thing the same way.
    fn geometry_name(&self, id: NodeId) -> String {
        let doc = self.core.document();
        if let Some(node) = doc.nodes.get(&id) {
            return node.name.clone();
        }
        if let Some(operation) = doc.operations.get(id) {
            return operation.name.clone();
        }
        if let Some(node) = doc.procedural.get(id) {
            return node.describe();
        }
        "geometry".to_string()
    }

    /// Recompute the virtual operation nodes the last mutation affected, and
    /// compose them into the scene.
    ///
    /// Two sources of dirtiness, deliberately kept separate from the dependency
    /// graph (an operation reads whole *shapes*, so there is no single slot to
    /// draw an edge from):
    ///
    /// * `pending_operations` — the command changed the registry itself;
    /// * `Document::operations.affected_by(dirty)` — the command moved a source
    ///   an operation reads.
    ///
    /// Everything else keeps the geometry the previous pass gave it: this is a
    /// patch, not a rebuild.
    fn run_operations(&mut self, report: &vectra_dependency::EvalReport) -> Vec<OperationId> {
        let pending: Vec<OperationId> = std::mem::take(&mut self.pending_operations);
        let mut targets = pending.clone();
        if report.mode == vectra_core::EvalMode::Full {
            // A cold cache (or an explicit full rebuild) recomputes every
            // operation, mirroring `force_full_evaluation`.
            targets = self.core.document().operations.order.clone();
        } else {
            for id in self.core.document().operations.affected_by(&report.dirty) {
                if !targets.contains(&id) {
                    targets.push(id);
                }
            }
        }
        if targets.is_empty() && self.operation_diagnostics.is_empty() {
            // Nothing to compute *and* nothing stale to clear: the operations
            // layer costs one empty-set check per mutation. (`settle` has
            // already retired anything whose registry entry changed.)
            return Vec::new();
        }
        let evaluation = {
            let Self {
                core,
                expressions,
                scene,
                operations,
                motion,
                procedural,
                ..
            } = self;
            let ctx = core
                .evaluation_context()
                .with_expression(expressions)
                .with_motion(motion)
                .with_procedural(procedural)
                .with_fonts(&self.fonts);
            let evaluation = operations.evaluate(core.document(), scene.scene(), &ctx, &targets);
            evaluation
        };
        // Diagnostics are per-operation: forget the ones we just recomputed,
        // then add what the pass reported. A stale note can never outlive the
        // geometry it described.
        self.operation_diagnostics.retain(|d| match d.node_id {
            Some(id) => !targets.contains(&id),
            None => false,
        });
        self.operation_diagnostics
            .extend(evaluation.diagnostics.iter().cloned());
        let recomputed = evaluation.recomputed.clone();
        evaluation.compose_into(self.scene.scene_mut(), self.core.document());
        // `compose_into` already dropped anything no longer live, so the
        // pruning pass is a no-op here; it stays as the single place that
        // enforces the live-id invariant.
        recomputed
    }

    /// Recompute the procedural nodes the last mutation affected, compose their
    /// geometry, and publish their value ports (Task 7.0).
    ///
    /// Mirrors [`Self::run_operations`] with two additions the procedural layer
    /// needs: the seed set includes the operations that *just* recomputed (a
    /// `Source` node can read a boolean result and has no graph edge to the
    /// registry that produced it), and the returned report's `published` ports
    /// tell the caller which slots to re-read.
    /// `extra` seeds a **later round** of the same settle: the ids whose scene
    /// entry the readers of the previous round moved (see
    /// [`Self::run_procedural_fixpoint`]). They are ordinary dirty ids as far as
    /// the pass is concerned — a `Source` node that reads one of them has no
    /// graph edge to be found by, so the registry scans for it instead.
    fn run_procedural(
        &mut self,
        report: &vectra_dependency::EvalReport,
        recomputed_operations: &[OperationId],
        extra: &[NodeId],
    ) -> ProceduralEvaluation {
        let pending: Vec<NodeId> = std::mem::take(&mut self.pending_procedural);
        let mut seeds: Vec<NodeId> = Vec::new();
        if report.mode == vectra_core::EvalMode::Full {
            seeds.extend(self.core.document().procedural.order.iter().copied());
            seeds.extend(extra.iter().copied());
        } else {
            seeds.extend(report.dirty.iter().copied());
            seeds.extend(recomputed_operations.iter().copied());
            seeds.extend(pending.iter().copied());
            seeds.extend(extra.iter().copied());
        }
        seeds.sort();
        seeds.dedup();
        let evaluation = {
            let Self {
                core,
                expressions,
                scene,
                procedural,
                ..
            } = self;
            // No procedural door in *this* context: an operand cannot read a
            // procedural output (RULE 3), so passing the table could only tempt
            // a cycle into existence.
            let ctx = core
                .evaluation_context()
                .with_expression(expressions)
                .with_fonts(&self.fonts);
            procedural.evaluate(scene.scene(), &ctx, &seeds)
        };
        evaluation.compose_into(self.scene.scene_mut(), self.core.document());
        evaluation
    }

    /// Run the pass, re-read the slots that read it, and run again for the
    /// geometry that re-read moved — the fixpoint RULE 2 asks for.
    ///
    /// The pass and its readers are mutually responsible, and the reason is the
    /// same in both directions: the pass runs *last* (RULE 2), so a slot that
    /// reads a port was resolved against the previous table and must be re-read;
    /// and a `Source` node reads a whole *shape*, so a slot that moved is
    /// geometry the pass has to see — with no graph edge between the two.
    ///
    /// ```text
    ///   round 1   ⬡ grid • span republished ─▶ rect.width re-read ─▶ rect moved
    ///   round 2   ⬡ source(rect) re-runs    ─▶ …             ─▶ nothing new
    /// ```
    ///
    /// A round only happens when a port's value actually *changed*, and a value
    /// reference back into the graph is refused at the command boundary
    /// (RULE 3: the graph's cycle gate for a wire, `validate_procedural_cycle`
    /// for a slot, which sees the geometry path too). So a second round is the
    /// last one a legal document can need; the bound is a safety net for a
    /// registry that arrived by some other road, and it is a `debug_assert`
    /// rather than a silent stop because a document that needs round five is a
    /// bug in the engine, not a tuning problem.
    fn run_procedural_fixpoint(
        &mut self,
        report: &vectra_dependency::EvalReport,
        recomputed_operations: &[OperationId],
    ) -> Vec<NodeId> {
        let mut touched: Vec<NodeId> = Vec::new();
        let mut extra: Vec<NodeId> = Vec::new();
        for round in 0..MAX_PASS_ROUNDS {
            let passed = self.run_procedural(report, recomputed_operations, &extra);
            touched.extend(passed.recomputed.iter().copied());
            touched.extend(passed.retired.iter().copied());
            if passed.published.is_empty() {
                break;
            }
            let followed = self.follow_up_readers(&passed.published);
            if followed.is_empty() {
                break;
            }
            touched.extend(followed.iter().copied());
            if round + 1 == MAX_PASS_ROUNDS {
                debug_assert!(
                    false,
                    "the procedural settle did not converge in {MAX_PASS_ROUNDS} rounds"
                );
                break;
            }
            extra = followed;
        }
        touched
    }

    /// Evaluate the slots that read a value port this pass republished, and
    /// report the ids whose scene entry may have moved.
    ///
    /// A slot is resolved *before* the procedural pass runs, so a reader of a
    /// port is always one table-generation behind unless it is re-read — and
    /// there are two kinds of reader: scene nodes (patched here) and operations
    /// (whose *parameters* may read a port, recomputed through the operations
    /// pass). Whatever this returns is also the next round's seed set, because a
    /// re-read reader is geometry a `Source` node may be reading.
    fn follow_up_readers(&mut self, published: &[NodeOutputId]) -> Vec<NodeId> {
        let readers = self.core.document().procedural_readers(published);
        if readers.is_empty() {
            return Vec::new();
        }
        let operations = self.core.document().operations.clone();
        let (op_readers, node_readers): (Vec<NodeId>, Vec<NodeId>) = readers
            .iter()
            .copied()
            .partition(|id| operations.contains(*id));
        let mut touched: Vec<NodeId> = Vec::new();
        if !node_readers.is_empty() {
            self.patch(&node_readers);
            touched.extend(node_readers);
        }
        if !op_readers.is_empty() {
            // Only the value an operation *reads* moved; its own inputs did not,
            // so the registry itself is the pending set.
            let follow_up = vectra_dependency::EvalReport {
                mode: EvalMode::Incremental,
                dirty: op_readers.clone(),
                evaluated: 0,
            };
            touched.extend(self.run_operations(&follow_up));
        }
        touched
    }

    /// Retire cached geometry for operations that are gone or parked, and
    /// report the ids whose scene entry changed.
    fn prune_disabled_operations(&mut self) -> Vec<OperationId> {
        let doc = self.core.document();
        let stale: Vec<OperationId> = self
            .scene
            .scene()
            .nodes
            .keys()
            .copied()
            .filter(|id| !doc.is_geometry_id(*id))
            .collect();
        if stale.is_empty() {
            return Vec::new();
        }
        // `apply_operations` with no updates drops everything that is no longer
        // live geometry, which is exactly this set.
        self.scene
            .scene_mut()
            .apply_operations(doc, std::iter::empty());
        stale
    }

    /// Patch the scene cache for `dirty` (empty = nothing to do, cold = full).
    fn patch(&mut self, dirty: &[vectra_core::NodeId]) -> vectra_dependency::EvalReport {
        let Self {
            core,
            expressions,
            scene,
            motion,
            procedural,
            fonts,
            ..
        } = self;
        let ctx = core
            .evaluation_context()
            .with_expression(expressions)
            .with_motion(motion)
            .with_procedural(procedural)
            .with_fonts(fonts);
        scene.refresh(core.document(), &ctx, dirty)
    }
}

/// What one constraint pass did (Task 3.1, extended for drags in 3.2).
///
/// * `inverses` are the exact inverses of everything the pass applied — the
///   caller folds them into the history entry of the command that caused the
///   pass (`amend`), or into the drag gesture's single entry (`end_drag`).
/// * `disabled` names the constraints the pass parked, so a caller that records
///   its own entry can replay the flag change on redo.
/// * `writes` are the raw solved writes (the drag needs their values).
#[derive(Debug, Default)]
struct ConstraintPass {
    events: Vec<EngineEvent>,
    diagnostics: Vec<Diagnostic>,
    inverses: Vec<Command>,
    writes: Vec<SolveWrite>,
    disabled: Vec<vectra_core::ConstraintId>,
}

/// The diagnostic for a write that had to break a parametric link.
fn link_broken_diagnostic(write: &SolveWrite) -> Diagnostic {
    Diagnostic::parametric_link_broken(
        write.target.node_id,
        write.target.property.clone(),
        format!(
            "solver pinned {} to {} — the parametric link ({}) was broken; constraints never write variables",
            write.target.property,
            write.value,
            describe_parametric_source(&write.source),
        ),
    )
}

/// The slots a command just acted on: the solver pins these at `STRONG`, so it
/// moves *other* slots to satisfy the rules (`AddConstraint` is anchored on its
/// first target — "make B vertical to A" moves B).
fn hints_for(command: &Command) -> Vec<ConstraintTarget> {
    match command {
        Command::SetParameter {
            node_id, property, ..
        } => vec![ConstraintTarget::new(*node_id, property.clone())],
        Command::AddConstraint { constraint } => {
            constraint.targets.first().cloned().into_iter().collect()
        }
        Command::SetConstraintEnabled { .. } => Vec::new(),
        Command::Batch { commands } => commands.iter().flat_map(hints_for).collect(),
        _ => Vec::new(),
    }
}

/// The operation ids a command's registry entry touches (Task 4.0).
///
/// `ApplyOperation` / `RemoveOperation` / `SetOperationEnabled` are the only
/// commands that mutate the operations registry, and a `Batch` recurses — the
/// cascade a node deletion produces is a batch of `ApplyOperation` inverses.
/// Every published value port, formatted: the snapshot's projection needs the
/// data, not the engine that produced it.
fn published_table(
    procedural: &ProceduralEngine,
    doc: &vectra_core::Document,
) -> BTreeMap<String, BTreeMap<String, String>> {
    let mut out = BTreeMap::new();
    for node in doc.procedural.in_order() {
        let Some(values) = procedural.outputs_of(node.id) else {
            continue;
        };
        if values.is_empty() {
            continue;
        }
        out.insert(
            node.id.to_string(),
            values
                .iter()
                .map(|(port, value)| (port.clone(), geometry_summary(value)))
                .collect(),
        );
    }
    out
}

/// One kind in the palette (`procedural_kinds`), with the operand payload the
/// add command needs.
#[derive(Debug, Clone, Serialize)]
struct ProceduralKindWire {
    tag: String,
    /// What the dropdown shows (`3×2 grid, 40 apart, @(0, 0)`).
    label: String,
    /// True for `Source`, whose subject is part of the kind.
    needs_subject: bool,
    /// Operand port → the `ParamValue` payload, ready to embed verbatim.
    operands: BTreeMap<String, serde_json::Value>,
    inputs: Vec<ProceduralKindPortWire>,
    outputs: Vec<ProceduralKindPortWire>,
}

/// One declared port of a kind (palette entry).
#[derive(Debug, Clone, Serialize)]
struct ProceduralKindPortWire {
    port: String,
    ty: String,
    required: bool,
}

/// The procedural graph, engine-side (Task 7.0): the panel's whole view of the
/// chain, plus what the last pass published.
#[derive(Debug, Clone, Serialize)]
struct ProceduralReportWire {
    count: usize,
    /// Registry order — the order the pass evaluates them in.
    nodes: Vec<ProceduralNodeWire>,
    /// One note per node that could not compute (Containment Law).
    diagnostics: Vec<SnapshotDiagnostic>,
}

/// One procedural node, as the panel shows it.
#[derive(Debug, Clone, Serialize)]
struct ProceduralNodeWire {
    id: String,
    name: String,
    kind: String,
    enabled: bool,
    /// The node's **effective** operands, already formatted.
    description: String,
    /// Operand port → its effective value (`12`, `(variable)`, `(expression)`),
    /// with the port's type so the panel knows whether to offer one number or
    /// two. The type is the engine's declaration, never a guess from the name.
    operands: BTreeMap<String, ProceduralOperandWire>,
    /// Input port → the `node:port` it is wired to.
    wires: BTreeMap<String, String>,
    /// Declared inputs (the panel offers these as connect targets).
    inputs: Vec<ProceduralPortWire>,
    /// Declared outputs, with the last published value (`region 3 rings`).
    outputs: Vec<ProceduralPortWire>,
    upstream: Vec<String>,
}

/// One operand, as the panel shows it.
#[derive(Debug, Clone, Serialize)]
struct ProceduralOperandWire {
    /// `scalar` / `point` / `color` — what the Set control must offer.
    ty: String,
    /// The effective value, engine-rendered.
    text: String,
}

/// One port, wired or published.
#[derive(Debug, Clone, Serialize)]
struct ProceduralPortWire {
    port: String,
    ty: String,
    required: bool,
    wired: bool,
    /// `None` for an input port, or for an output nothing has published yet.
    value: Option<String>,
}

/// A slot's value as the panel shows it: the literal, or `(tag)` when something
/// else drives it — the same rendering `ProceduralNode::describe` uses, so the
/// panel and the summary cannot disagree.
fn render_param_value(value: &ParamValue, doc: &vectra_core::Document) -> String {
    match value {
        ParamValue::Float(param) => match param {
            Parameter::Literal(v) => format!("{v}"),
            Parameter::Procedural(out) => render_procedural_ref(out, doc),
            other => format!("({})", other.source_tag()),
        },
        ParamValue::Point(param) => match param {
            Parameter::Literal(p) => format!("{}, {}", p.x, p.y),
            Parameter::Procedural(out) => render_procedural_ref(out, doc),
            other => format!("({})", other.source_tag()),
        },
        ParamValue::Color(param) => match param {
            Parameter::Literal(c) => format!("#{:02x}{:02x}{:02x}", c.r, c.g, c.b),
            Parameter::Procedural(out) => render_procedural_ref(out, doc),
            other => format!("({})", other.source_tag()),
        },
    }
}

/// `⬡ ab12cd34 • span` — a procedural read, named rather than numbered.
fn render_procedural_ref(out: &NodeOutputId, doc: &vectra_core::Document) -> String {
    let name = doc
        .procedural
        .get(out.node)
        .map(|node| node.name.clone())
        .unwrap_or_else(|| out.node.to_string().chars().take(8).collect());
    format!("⬡ {name} • {}", out.port)
}

/// One geometry value, in a few words — the panel's published-value column.
/// The AI layer's view of this engine (Task 9.0, RULE 3's host seam).
///
/// `apply` calls [`VectraEngine::dispatch_command`] — **the same path a user's
/// click takes**, so an AI command meets the dependency-graph cycle gate, the
/// expression pre-compile, the procedural port gate and the DRAG guard before it
/// touches the document, and the scene/dirty bookkeeping runs afterwards exactly
/// as it does for a human edit. `rollback` calls [`VectraEngine::undo`], so a
/// failed plan unwinds through the ordinary history.
///
/// Nothing here knows what a command means: it is a pipe with a summary
/// attached, which is what keeps RULE 3 true — the engine validates, the AI
/// layer only carries the verdict.
struct EngineHost<'a> {
    engine: &'a mut VectraEngine,
}

impl CommandHost for EngineHost<'_> {
    fn apply(&mut self, command: &Command) -> Result<Vec<EngineEvent>, String> {
        let json = serde_json::to_string(command)
            .map_err(|error| format!("could not serialize the command: {error}"))?;
        let response = self.engine.dispatch_command(&json);
        match serde_json::from_str::<CommandResponse>(&response) {
            Ok(CommandResponse::Ok { events }) => Ok(events),
            Ok(CommandResponse::Error { message }) => Err(message),
            Err(error) => Err(format!("the engine's reply was not an envelope: {error}")),
        }
    }

    fn rollback(&mut self, steps: usize) -> Result<(), String> {
        for _ in 0..steps {
            let response = self.engine.undo();
            if let Ok(CommandResponse::Error { message }) =
                serde_json::from_str::<CommandResponse>(&response)
            {
                return Err(message);
            }
        }
        Ok(())
    }

    fn summary(&self) -> DocumentSummary {
        self.engine.document_summary_value()
    }
}

/// The AI panel's preview envelope: the plan as JSON, so the UI shows exactly
/// the commands that would be dispatched.
fn ai_preview_json(preview: &Preview) -> String {
    #[derive(serde::Serialize)]
    struct Envelope<'a> {
        status: &'a str,
        prompt: &'a str,
        plan: &'a [serde_json::Value],
        notes: &'a [String],
        attempt: usize,
        applies: bool,
    }
    serde_json::to_string(&Envelope {
        status: "ok",
        prompt: &preview.prompt,
        plan: &preview.plan,
        notes: &preview.notes,
        attempt: preview.attempt,
        applies: false,
    })
    .unwrap_or_else(|error| format!("{{\"status\":\"error\",\"message\":{error:?}}}"))
}

/// The AI panel's execution envelope.
fn ai_report_json(report: &ExecutionReport) -> String {
    #[derive(serde::Serialize)]
    struct Envelope<'a> {
        status: &'a str,
        headline: String,
        /// The designer's sentence: what changed, in words (RULE 4). The panel
        /// shows this; it never renders `report` as JSON.
        prose: String,
        report: &'a ExecutionReport,
        corrections: usize,
    }
    serde_json::to_string(&Envelope {
        status: "ok",
        headline: report.headline(),
        prose: report.prose(),
        report,
        corrections: report.corrections.len(),
    })
    .unwrap_or_else(|error| format!("{{\"status\":\"error\",\"message\":{error:?}}}"))
}

/// A failed AI call: the typed code, the message, and the corrections that led
/// to it (a `MaxRetriesExceeded` carries its history, so the panel can show the
/// user what the AI kept getting wrong).
fn ai_error_json(error: &AiError) -> String {
    #[derive(serde::Serialize)]
    struct Envelope<'a> {
        status: &'a str,
        code: &'a str,
        message: String,
        corrections: &'a [Correction],
        plan: Vec<serde_json::Value>,
    }
    let plan: Vec<serde_json::Value> = match error {
        AiError::MaxRetriesExceeded { plan, .. } => plan.to_vec(),
        _ => Vec::new(),
    };
    serde_json::to_string(&Envelope {
        status: "error",
        code: error.code(),
        message: vectra_ai::prompt::failure_message(error),
        corrections: error.corrections(),
        plan,
    })
    .unwrap_or_else(|_| {
        r#"{"status":"error","code":"unknown","message":"the AI layer failed"}"#.to_string()
    })
}

/// A host with no engine behind it, for the *preview* call: generating commands
/// may not touch the document, so the preview path has nothing to apply to. It
/// carries the summary the caller offered (the live one by default).
struct AiPreviewHost {
    summary: DocumentSummary,
}

impl CommandHost for AiPreviewHost {
    fn apply(&mut self, _command: &Command) -> Result<Vec<EngineEvent>, String> {
        Err("preview only: nothing is applied".to_string())
    }

    fn rollback(&mut self, _steps: usize) -> Result<(), String> {
        Ok(())
    }

    fn summary(&self) -> DocumentSummary {
        self.summary.clone()
    }
}

/// The wire envelope both export methods return.
///
/// One shape for both formats, so the panel has one parser and one modal: the
/// `format` field is what the syntax label reads, `code` is what the user copies,
/// and `warnings` is the list the modal shows *above* the code — an export that
/// silently replaced a spring with a number would be worse than no export.
fn export_envelope(format: &str, code: String, warnings: &[String]) -> String {
    #[derive(serde::Serialize)]
    struct Envelope<'a> {
        status: &'a str,
        format: &'a str,
        code: String,
        warnings: &'a [String],
    }
    serde_json::to_string(&Envelope {
        status: "ok",
        format,
        code,
        warnings,
    })
    .unwrap_or_else(|error| format!("{{\"status\":\"error\",\"message\":{error:?}}}"))
}

fn geometry_summary(data: &GeometryData) -> String {
    match data {
        GeometryData::Scalar(v) => format!("scalar {v}"),
        GeometryData::Point(p) => format!("point ({}, {})", p.x, p.y),
        GeometryData::Points(points) => format!("points ×{}", points.len()),
        GeometryData::Path { points, closed } => format!(
            "path {} pts{}",
            points.len(),
            if *closed { ", closed" } else { "" }
        ),
        GeometryData::Region { rings } => {
            let vertices: usize = rings.iter().map(Vec::len).sum();
            format!("region {} rings, {vertices} pts", rings.len())
        }
        GeometryData::Color(c) => format!("#{:02x}{:02x}{:02x}", c.r, c.g, c.b),
    }
}

/// The procedural nodes a command touches (Task 7.0), so the pass recomputes
/// them even when the dependency graph has nothing to say — a wire change, an
/// operand edit, a park, an add or a remove.
fn procedural_ids_of(command: &Command) -> Vec<NodeId> {
    match command {
        Command::AddProceduralNode { node } => vec![node.id],
        Command::RemoveProceduralNode { id } | Command::SetProceduralEnabled { id, .. } => {
            vec![*id]
        }
        Command::ConnectProcedural { node_id, .. }
        | Command::DisconnectProcedural { node_id, .. }
        | Command::SetProceduralOperand { node_id, .. } => vec![*node_id],
        Command::Batch { commands } => commands.iter().flat_map(procedural_ids_of).collect(),
        _ => Vec::new(),
    }
}

fn operation_ids_of(command: &Command) -> Vec<OperationId> {
    match command {
        Command::ApplyOperation { id, .. }
        | Command::RemoveOperation { id }
        | Command::SetOperationEnabled { id, .. }
        | Command::CreateSmartFill { id, .. } => vec![*id],
        Command::Batch { commands } => commands.iter().flat_map(operation_ids_of).collect(),
        _ => Vec::new(),
    }
}

/// One broken arc as a closed path: the first point, a line to each point
/// after it, and a close — which is the segment the split's own cut implies.
///
/// A piece of two points is legal (a span cut out of a straight segment): the
/// path is then a single degenerate edge, and it is still a *closed* path, which
/// is what RULE 3's "no gaps" is stated in terms of.
fn outline_piece(
    points: &[(f64, f64)],
    name: &str,
    index: usize,
) -> Option<vectra_core::OutlinePath> {
    if points.len() < 2 {
        return None;
    }
    let mut segments: Vec<vectra_core::PathSegment> = points[1..]
        .iter()
        .map(|(x, y)| vectra_core::PathSegment::Line {
            to: Parameter::Literal(Point2::new(*x, *y)),
        })
        .collect();
    segments.push(vectra_core::PathSegment::Close);
    Some(vectra_core::OutlinePath {
        id: vectra_core::new_node_id(),
        name: format!("{name} · {index}"),
        start: Parameter::Literal(Point2::new(points[0].0, points[0].1)),
        segments,
    })
}

/// The arc length of one closed ring — the splitter's unit.
fn ring_arc_length(ring: &[vectra_geometry::GeoCoord<f64>]) -> f64 {
    (0..ring.len())
        .map(|index| {
            let p = ring[index];
            let q = ring[(index + 1) % ring.len()];
            (q.x - p.x).hypot(q.y - p.y)
        })
        .sum()
}

/// Eight-character id prefix, as the inspector and the event log show it.
fn short_id(id: impl std::fmt::Display) -> String {
    id.to_string().chars().take(8).collect()
}

/// The first slot of a registered constraint (diagnostics address a node).
fn first_target_of(
    id: vectra_core::ConstraintId,
    doc: &vectra_core::Document,
) -> Option<ConstraintTarget> {
    doc.constraints
        .get(id)
        .and_then(|constraint| constraint.targets.first().cloned())
}

/// Human-readable source of a slot whose link the solver had to break.
fn describe_parametric_source(source: &ParamValue) -> String {
    match source {
        ParamValue::Float(Parameter::Variable(name)) => format!("${name}"),
        ParamValue::Float(Parameter::Expression(id)) => format!("ƒ{id}"),
        ParamValue::Float(other) => format!("{other:?}"),
        other => other.kind().to_string(),
    }
}

/// Map a solver failure from the drag protocol onto the typed error vocabulary
/// (the drag errors are the engine's own state-machine failures, surfaced by the
/// session).
fn drag_error_message(error: ConstraintError, node_id: NodeId) -> VectraError {
    match error {
        ConstraintError::DragBusy { .. } => VectraError::drag_active(node_id),
        ConstraintError::NoDragSession => VectraError::drag_not_active(node_id),
        ConstraintError::NotDraggable { .. } => VectraError::not_draggable(node_id),
        ConstraintError::Unsatisfiable { message } => VectraError::unsatisfiable(message),
        other => VectraError::command(other.to_string()),
    }
}

/// Map a solver failure onto the engine's typed error vocabulary.
fn constraint_error_message(error: ConstraintError) -> String {
    match error {
        ConstraintError::Unsatisfiable { message } => {
            VectraError::unsatisfiable(message).to_string()
        }
        other => VectraError::command(other.to_string()).to_string(),
    }
}

impl Default for VectraEngine {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test helper: dispatch a JSON command and return the parsed response.
    fn call(engine: &mut VectraEngine, cmd: serde_json::Value) -> serde_json::Value {
        serde_json::from_str(&engine.dispatch_command(&cmd.to_string())).unwrap()
    }

    /// Test helper: the `Dirty` event of a response, as `(ids, mode)`.
    fn dirty_of(response: &serde_json::Value) -> (Vec<String>, String) {
        let event = response["events"]
            .as_array()
            .expect("ok response")
            .iter()
            .find(|e| e["type"] == "Dirty")
            .expect("every mutation reports a Dirty event");
        let ids = event["ids"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        (ids, event["mode"].as_str().unwrap().to_string())
    }

    fn snapshot_of(engine: &mut VectraEngine) -> serde_json::Value {
        serde_json::from_str(&engine.get_snapshot()).unwrap()
    }

    /// Task 2.2: a variable edit re-evaluates exactly its dependent nodes, the
    /// engine never falls back to a full rebuild, and the cost is *visible* in
    /// the event stream and the snapshot's `eval` block.
    #[test]
    fn incremental_cost_is_visible_and_minimal_through_boundary() {
        let mut engine = VectraEngine::new();
        let bound = vectra_core::new_node_id().to_string();
        let free = vectra_core::new_node_id().to_string();
        let expr = vectra_core::new_expression_id().to_string();

        call(
            &mut engine,
            serde_json::json!({"type": "SetVariable", "name": "base", "value": 20.0}),
        );
        call(
            &mut engine,
            serde_json::json!({"type": "DefineExpression", "id": expr, "source": "$base * 2"}),
        );
        for (id, r) in [(&bound, 1.0), (&free, 30.0)] {
            call(
                &mut engine,
                serde_json::json!({
                    "type": "CreateNode",
                    "id": id,
                    "kind": {"Circle": {
                        "cx": {"Literal": 0.0},
                        "cy": {"Literal": 0.0},
                        "radius": {"Literal": r},
                    }},
                    "name": id,
                }),
            );
        }
        let bind = call(
            &mut engine,
            serde_json::json!({
                "type": "SetParameter",
                "node_id": bound,
                "property": "radius",
                "value": {"Float": {"Expression": expr}},
            }),
        );
        assert_eq!(bind["status"], "ok");
        let (ids, mode) = dirty_of(&bind);
        assert_eq!(ids, vec![bound.clone()], "only the bound node");
        assert_eq!(mode, "incremental");

        // One full pass, ever — and the edits since then were patches.
        let snap = snapshot_of(&mut engine);
        assert_eq!(
            snap["eval"]["full_evals"], 1,
            "cold start is the only full pass"
        );
        assert_eq!(
            snap["eval"]["last_mode"], "incremental",
            "every edit after the cold pass patched a subset"
        );
        assert!(snap["eval"]["incremental_evals"].as_u64().unwrap() >= 1);
        assert_eq!(snap["scene"]["nodes"][&bound]["primitive"]["r"], 40.0);

        // A brand-new engine reports the cold pass as its mode.
        let mut fresh_engine = VectraEngine::new();
        let cold = snapshot_of(&mut fresh_engine);
        assert_eq!(cold["eval"]["last_mode"], "full");
        assert_eq!(cold["eval"]["full_evals"], 1);
        assert_eq!(cold["eval"]["incremental_evals"], 0);

        // Change the variable: one node re-evaluated, no full pass.
        let changed = call(
            &mut engine,
            serde_json::json!({"type": "SetVariable", "name": "base", "value": 21.0}),
        );
        let (ids, mode) = dirty_of(&changed);
        assert_eq!(ids, vec![bound.clone()]);
        assert_eq!(mode, "incremental");
        let snap = snapshot_of(&mut engine);
        assert_eq!(snap["eval"]["full_evals"], 1, "no full rebuild happened");
        assert_eq!(snap["eval"]["last_evaluated"], 1);
        assert_eq!(snap["eval"]["last_dirty"], 1);
        assert_eq!(snap["scene"]["nodes"][&bound]["primitive"]["r"], 42.0);
        assert_eq!(
            snap["scene"]["nodes"][&free]["primitive"]["r"], 30.0,
            "the unrelated node keeps its evaluated value"
        );

        // A variable nobody reads costs nothing at all. (`no_ops` counts every
        // mutation that required no evaluation — e.g. a defined-but-unbound
        // expression is one too, hence the delta assertion.)
        let no_ops_before = snapshot_of(&mut engine)["eval"]["no_ops"].as_u64().unwrap();
        let idle = call(
            &mut engine,
            serde_json::json!({"type": "SetVariable", "name": "unused", "value": 1.0}),
        );
        let (ids, mode) = dirty_of(&idle);
        assert!(ids.is_empty(), "nothing depends on $unused");
        assert_eq!(mode, "incremental");
        let snap = snapshot_of(&mut engine);
        assert_eq!(
            snap["eval"]["no_ops"].as_u64().unwrap(),
            no_ops_before + 1,
            "an edit with no dependents is exactly one no-op evaluation"
        );
        assert_eq!(snap["eval"]["full_evals"], 1);

        // Undo/redo stay on the incremental path too — and undoing the no-op
        // edit is (correctly) still a no-op.
        let undone_idle = serde_json::from_str::<serde_json::Value>(&engine.undo()).unwrap();
        assert!(dirty_of(&undone_idle).0.is_empty());
        let undone = serde_json::from_str::<serde_json::Value>(&engine.undo()).unwrap();
        let (ids, mode) = dirty_of(&undone);
        assert_eq!(
            ids,
            vec![bound.clone()],
            "the dependent node is re-evaluated"
        );
        assert_eq!(mode, "incremental");
        assert_eq!(
            snapshot_of(&mut engine)["scene"]["nodes"][&bound]["primitive"]["r"],
            40.0
        );
        let redone = serde_json::from_str::<serde_json::Value>(&engine.redo()).unwrap();
        let (ids, _) = dirty_of(&redone);
        assert_eq!(ids, vec![bound.clone()]);
        let snap = snapshot_of(&mut engine);
        assert_eq!(snap["scene"]["nodes"][&bound]["primitive"]["r"], 42.0);
        assert_eq!(snap["eval"]["full_evals"], 1, "undo/redo never rebuild");
    }

    /// The dependency graph is inspectable over the wire, and it tracks the
    /// document through removes, deletes, and undo.
    #[test]
    fn dependency_graph_export_through_boundary() {
        let mut engine = VectraEngine::new();
        let node = vectra_core::new_node_id().to_string();
        let expr = vectra_core::new_expression_id().to_string();
        let other = vectra_core::new_node_id().to_string();

        call(
            &mut engine,
            serde_json::json!({"type": "SetVariable", "name": "base", "value": 5.0}),
        );
        call(
            &mut engine,
            serde_json::json!({"type": "DefineExpression", "id": expr, "source": "$base + 1"}),
        );
        call(
            &mut engine,
            serde_json::json!({
                "type": "CreateNode",
                "id": node,
                "kind": {"Rectangle": {
                    "x": {"Literal": 0.0}, "y": {"Literal": 0.0},
                    "width": {"Literal": 1.0}, "height": {"Literal": 1.0},
                    "corner_radius": {"Literal": 0.0},
                }},
                "name": "rect",
            }),
        );
        call(
            &mut engine,
            serde_json::json!({
                "type": "CreateNode",
                "id": other,
                "kind": {"Circle": {
                    "cx": {"Literal": 0.0},
                    "cy": {"Literal": 0.0},
                    "radius": {"Literal": 2.0},
                }},
            }),
        );
        call(
            &mut engine,
            serde_json::json!({
                "type": "SetParameter",
                "node_id": node,
                "property": "width",
                "value": {"Float": {"Expression": expr}},
            }),
        );

        let graph: serde_json::Value = serde_json::from_str(&engine.dependencies()).unwrap();
        assert_eq!(graph["status"], "ok");
        assert_eq!(graph["summary"]["acyclic"], true);
        assert_eq!(
            graph["summary"]["nodes"], 3,
            "variable + expression + property"
        );
        assert_eq!(graph["summary"]["edges"], 2);
        let keys: Vec<&str> = graph["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|n| n["key"].as_str().unwrap())
            .collect();
        assert!(keys.contains(&"var:base"));
        assert!(keys.contains(&format!("expr:{expr}").as_str()));
        assert!(keys.contains(&format!("prop:{node}:width").as_str()));
        assert!(
            keys.iter().all(|k| !k.contains(&other)),
            "a literal node has no vertices"
        );

        // The snapshot carries the same summary, without a second call.
        let snap = snapshot_of(&mut engine);
        assert_eq!(snap["graph"]["edges"], 2);
        assert_eq!(snap["graph"]["acyclic"], true);

        // Removing the expression drops its edges; the property still points at
        // the (now undefined) expression id — a phantom vertex, by design.
        let removed = call(
            &mut engine,
            serde_json::json!({"type": "RemoveExpression", "id": expr}),
        );
        assert_eq!(removed["status"], "ok");
        let graph: serde_json::Value = serde_json::from_str(&engine.dependencies()).unwrap();
        assert_eq!(graph["summary"]["edges"], 1, "expr → var edge is gone");
        assert_eq!(graph["summary"]["nodes"], 2, "phantom expr + property");
        assert_eq!(
            snapshot_of(&mut engine)["graph"]["edges"],
            1,
            "snapshot agrees with the graph"
        );
        // ...and evaluation reports the dangling reference as a diagnostic.
        let snap = snapshot_of(&mut engine);
        assert!(!snap["diagnostics"].as_array().unwrap().is_empty());

        // Undo restores the edge; deleting the node empties the graph.
        let undone = serde_json::from_str::<serde_json::Value>(&engine.undo()).unwrap();
        assert_eq!(undone["status"], "ok");
        let graph: serde_json::Value = serde_json::from_str(&engine.dependencies()).unwrap();
        assert_eq!(graph["summary"]["edges"], 2);
        call(
            &mut engine,
            serde_json::json!({"type": "DeleteNode", "id": node}),
        );
        let graph: serde_json::Value = serde_json::from_str(&engine.dependencies()).unwrap();
        assert_eq!(graph["summary"]["edges"], 1, "only expr → var remains");
        assert!(graph["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .all(|n| n["kind"] != "property"));
    }

    /// The clock is a global dependency: scrubbing time re-evaluates the
    /// parameters that read it, and nothing else.
    #[test]
    fn time_scrub_propagates_from_the_clock() {
        let mut engine = VectraEngine::new();
        let animated = vectra_core::new_node_id().to_string();
        let static_node = vectra_core::new_node_id().to_string();
        for (id, r) in [(&animated, 1.0), (&static_node, 2.0)] {
            call(
                &mut engine,
                serde_json::json!({
                    "type": "CreateNode",
                    "id": id,
                    "kind": {"Circle": {
                        "cx": {"Literal": 0.0},
                        "cy": {"Literal": 0.0},
                        "radius": {"Literal": r},
                    }},
                }),
            );
        }
        call(
            &mut engine,
            serde_json::json!({
                "type": "SetParameter",
                "node_id": animated,
                "property": "radius",
                "value": {"Float": {"Animated": {"StateDriven": {
                    "state": "on",
                    "true_value": {"Literal": 9.0},
                    "false_value": {"Literal": 3.0},
                }}}},
            }),
        );
        // An expression that reads $time depends on the clock as well.
        let expr = vectra_core::new_expression_id().to_string();
        call(
            &mut engine,
            serde_json::json!({"type": "DefineExpression", "id": expr, "source": "$time * 10"}),
        );

        let graph: serde_json::Value = serde_json::from_str(&engine.dependencies()).unwrap();
        let edges = graph["edges"].as_array().unwrap();
        assert!(
            edges
                .iter()
                .any(|e| e["to"] == "var:time" && e["from"] == format!("prop:{animated}:radius")),
            "the animated slot hangs off the clock: {edges:?}"
        );
        assert!(edges
            .iter()
            .any(|e| e["to"] == "var:time" && e["from"] == format!("expr:{expr}")));

        let response: serde_json::Value = serde_json::from_str(&engine.set_time(2.5)).unwrap();
        assert_eq!(response["status"], "ok");
        let (ids, mode) = dirty_of(&response);
        assert_eq!(ids, vec![animated.clone()], "only clock readers are dirty");
        assert_eq!(mode, "incremental");

        // Scrubbing time with no clock readers is a no-op.
        let mut idle = VectraEngine::new();
        call(
            &mut idle,
            serde_json::json!({
                "type": "CreateNode",
                "id": vectra_core::new_node_id().to_string(),
                "kind": {"Circle": {
                    "cx": {"Literal": 0.0}, "cy": {"Literal": 0.0},
                    "radius": {"Literal": 1.0},
                }},
            }),
        );
        let response: serde_json::Value = serde_json::from_str(&idle.set_time(1.0)).unwrap();
        let (ids, _) = dirty_of(&response);
        assert!(ids.is_empty());
    }

    /// The "re-evaluate everything" control produces the *same* scene the
    /// incremental cache built — the app-level witness of `patch ≡ rebuild`.
    #[test]
    fn force_full_evaluation_matches_the_incremental_cache() {
        let mut engine = VectraEngine::new();
        let node = vectra_core::new_node_id().to_string();
        let expr = vectra_core::new_expression_id().to_string();
        call(
            &mut engine,
            serde_json::json!({"type": "SetVariable", "name": "base", "value": 1.0}),
        );
        call(
            &mut engine,
            serde_json::json!({"type": "DefineExpression", "id": expr, "source": "$base * 3 + 1"}),
        );
        call(
            &mut engine,
            serde_json::json!({
                "type": "CreateNode",
                "id": node,
                "kind": {"Circle": {
                    "cx": {"Literal": 0.0}, "cy": {"Literal": 0.0},
                    "radius": {"Literal": 1.0},
                }},
            }),
        );
        call(
            &mut engine,
            serde_json::json!({
                "type": "SetParameter",
                "node_id": node,
                "property": "radius",
                "value": {"Float": {"Expression": expr}},
            }),
        );
        for value in [4.0, 5.0, 6.0] {
            call(
                &mut engine,
                serde_json::json!({"type": "SetVariable", "name": "base", "value": value}),
            );
        }
        let incremental_scene = snapshot_of(&mut engine)["scene"].clone();
        let fulls_before = snapshot_of(&mut engine)["eval"]["full_evals"]
            .as_u64()
            .unwrap();

        let full: serde_json::Value =
            serde_json::from_str(&engine.force_full_evaluation()).unwrap();
        let (ids, mode) = dirty_of(&full);
        assert_eq!(mode, "full");
        assert_eq!(ids, vec![node.clone()], "a full pass visits every node");

        let snap = snapshot_of(&mut engine);
        assert_eq!(
            snap["scene"], incremental_scene,
            "full rebuild must equal the incrementally patched scene"
        );
        assert_eq!(snap["eval"]["full_evals"], fulls_before + 1);
        assert_eq!(snap["eval"]["last_mode"], "full");
        assert_eq!(snap["scene"]["nodes"][&node]["primitive"]["r"], 19.0);
    }

    /// Mirror of the Node E2E smoke script, run natively: malformed JSON,
    /// create → snapshot → undo, all through the string boundary.
    #[test]
    fn boundary_round_trip() {
        let mut engine = VectraEngine::new();

        let bad = engine.dispatch_command(r#"{"type":"Nope"}"#);
        let bad: serde_json::Value = serde_json::from_str(&bad).unwrap();
        assert_eq!(bad["status"], "error");
        assert!(bad["message"].as_str().unwrap().contains("invalid command"));

        let id = vectra_core::new_node_id().to_string();
        let create = serde_json::json!({
            "type": "CreateNode",
            "id": id,
            "kind": {"Circle": {
                "cx": {"Literal": 100.0},
                "cy": {"Literal": 100.0},
                "radius": {"Literal": 50.0},
            }},
            "name": "smoke-circle",
        });
        let created: serde_json::Value =
            serde_json::from_str(&engine.dispatch_command(&create.to_string())).unwrap();
        assert_eq!(created["status"], "ok");
        let event_types: Vec<&str> = created["events"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["type"].as_str().unwrap())
            .collect();
        assert!(event_types.contains(&"NodesUpdated"));
        assert!(event_types.contains(&"OrderChanged"));
        assert!(
            event_types.contains(&"Dirty"),
            "every mutation reports cost"
        );

        let snapshot: serde_json::Value = serde_json::from_str(&engine.get_snapshot()).unwrap();
        assert_eq!(snapshot["status"], "ok");
        assert_eq!(snapshot["scene"]["z_order"], serde_json::json!([id]));
        assert_eq!(snapshot["scene"]["nodes"][&id]["name"], "smoke-circle");

        let undone: serde_json::Value = serde_json::from_str(&engine.undo()).unwrap();
        assert_eq!(undone["status"], "ok");
        let undone_types: Vec<&str> = undone["events"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["type"].as_str().unwrap())
            .collect();
        assert!(undone_types.contains(&"NodesRemoved"));

        let snapshot: serde_json::Value = serde_json::from_str(&engine.get_snapshot()).unwrap();
        assert!(snapshot["scene"]["z_order"].as_array().unwrap().is_empty());
        assert_eq!(snapshot["can_undo"], false);
        assert_eq!(snapshot["can_redo"], true);
    }

    /// Expression lifecycle through the string boundary: invalid defines
    /// fail WITHOUT mutating; valid define → snapshot lists it → binding a
    /// circle radius evaluates through bytecode; full undo/redo cycle keeps
    /// the compiled registry reunited with core (recompile on redo works).
    #[test]
    fn expression_lifecycle_through_boundary() {
        let mut engine = VectraEngine::new();
        let node_id = vectra_core::new_node_id().to_string();
        let expr_id = vectra_core::new_expression_id().to_string();

        // Invalid source: error, no record, no undo entry.
        let bad = serde_json::json!({
            "type": "DefineExpression",
            "id": expr_id,
            "source": "$a * ",
        });
        let bad: serde_json::Value =
            serde_json::from_str(&engine.dispatch_command(&bad.to_string())).unwrap();
        assert_eq!(bad["status"], "error");
        assert!(bad["message"]
            .as_str()
            .unwrap()
            .contains("invalid expression"));
        let snapshot: serde_json::Value = serde_json::from_str(&engine.get_snapshot()).unwrap();
        assert!(snapshot["expressions"].as_object().unwrap().is_empty());
        assert_eq!(snapshot["can_undo"], false);

        // Set variable + valid define.
        let set_var = serde_json::json!({"type": "SetVariable", "name": "base", "value": 21.0});
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(
                &engine.dispatch_command(&set_var.to_string())
            )
            .unwrap()["status"],
            "ok"
        );
        let define = serde_json::json!({
            "type": "DefineExpression",
            "id": expr_id,
            "source": "$base * 2 + 10",
        });
        let defined: serde_json::Value =
            serde_json::from_str(&engine.dispatch_command(&define.to_string())).unwrap();
        assert_eq!(defined["status"], "ok");
        let event_types: Vec<&str> = defined["events"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["type"].as_str().unwrap())
            .collect();
        assert!(event_types.contains(&"ExpressionsUpdated"));

        // Create circle, bind radius to the expression → r == 52 via bytecode.
        let create = serde_json::json!({
            "type": "CreateNode",
            "id": node_id,
            "kind": {"Circle": {
                "cx": {"Literal": 100.0},
                "cy": {"Literal": 100.0},
                "radius": {"Literal": 5.0},
            }},
            "name": "expr-circle",
        });
        engine.dispatch_command(&create.to_string());
        let bind = serde_json::json!({
            "type": "SetParameter",
            "node_id": node_id,
            "property": "radius",
            "value": {"Float": {"Expression": expr_id}},
        });
        let bound: serde_json::Value =
            serde_json::from_str(&engine.dispatch_command(&bind.to_string())).unwrap();
        assert_eq!(bound["status"], "ok");
        let snapshot: serde_json::Value = serde_json::from_str(&engine.get_snapshot()).unwrap();
        assert_eq!(
            snapshot["expressions"][&expr_id],
            serde_json::json!("$base * 2 + 10")
        );
        assert_eq!(snapshot["scene"]["nodes"][&node_id]["primitive"]["r"], 52.0);

        // Undo bind → literal radius back.
        engine.undo();
        let snapshot: serde_json::Value = serde_json::from_str(&engine.get_snapshot()).unwrap();
        assert_eq!(snapshot["scene"]["nodes"][&node_id]["primitive"]["r"], 5.0);

        // Undo create + define → expressions empty; redo all → 52 again,
        // proving the redo path recompiled the source into the registry.
        engine.undo();
        engine.undo();
        let snapshot: serde_json::Value = serde_json::from_str(&engine.get_snapshot()).unwrap();
        assert!(snapshot["expressions"].as_object().unwrap().is_empty());
        engine.redo();
        engine.redo();
        engine.redo();
        let snapshot: serde_json::Value = serde_json::from_str(&engine.get_snapshot()).unwrap();
        assert_eq!(
            snapshot["expressions"][&expr_id],
            serde_json::json!("$base * 2 + 10")
        );
        assert_eq!(snapshot["scene"]["nodes"][&node_id]["primitive"]["r"], 52.0);
    }
}
