//! `OperationsEvaluator` — the non-destructive operations pass (Task 4.0).
//!
//! Given the **primitive** evaluated scene and the document's operation
//! registry, this produces the virtual operation nodes: each one is a standard
//! `EvaluatedNode` whose primitive is a `Path` (RULE 3), keyed by its own
//! `NodeId`, ready to be composed into the scene the renderer and the UI read.
//!
//! ```text
//! Document ──[GeometryEvaluator]──▶ primitives ──[OperationsEvaluator]──▶ + virtual paths
//! ```
//!
//! # Incrementality
//!
//! Operations are re-run for the ids the caller names —
//! `Document::operations.affected_by(&dirty)` is the propagation step, and it is
//! an O(operations × inputs) scan rather than a graph traversal (an operation
//! reads whole *shapes*, not individual parameter slots, so there is no slot to
//! draw an edge from). Anything not named keeps its previous result: a patch,
//! not a rebuild.
//!
//! # Failure is local
//!
//! An operation that cannot be computed — an unresolvable radius, a source with
//! no region, a degenerate result — produces a [`Diagnostic`] naming the virtual
//! node and contributes **no geometry**. It never aborts the pass and never
//! panics: one broken operation cannot take the scene down.

use crate::convert::{multi_polygon_to_path, path_to_multi_polygon};
use crate::error::OperationError;
use crate::ops;
use geo::MultiPolygon;
use std::collections::BTreeMap;
use vectra_core::{
    Document, EvaluationContext, OperationId, OperationKind, Resolvable, StyleProperties,
};
use vectra_geometry::{
    path_to_svg_data, Diagnostic, DiagnosticCode, EvaluatedNode, EvaluatedPrimitive,
    EvaluatedScene, EvaluatedStyle, Severity,
};

/// What one operations pass produced.
#[derive(Debug, Default)]
pub struct OperationEvaluation {
    /// The virtual nodes, keyed by their own `NodeId` (RULE 3).
    pub nodes: BTreeMap<vectra_core::NodeId, EvaluatedNode>,
    /// One note per operation that could not be computed.
    pub diagnostics: Vec<Diagnostic>,
    /// Ids the pass actually recomputed (a subset of what the caller asked for:
    /// disabled and missing operations are skipped, not recomputed).
    pub recomputed: Vec<OperationId>,
}

impl OperationEvaluation {
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty() && self.diagnostics.is_empty()
    }

    /// Fold this pass into the scene: replace the recomputed virtual nodes,
    /// drop what is no longer live geometry, and restack z-order.
    pub fn compose_into(&self, scene: &mut EvaluatedScene, doc: &Document) {
        scene.apply_operations(doc, self.nodes.values().cloned());
    }
}

/// The stateless operations pass.
#[derive(Debug, Default, Clone, Copy)]
pub struct OperationsEvaluator;

impl OperationsEvaluator {
    pub fn new() -> Self {
        Self
    }

    /// Evaluate every operation in the registry (cold start, or a full pass).
    pub fn evaluate_all(
        &self,
        doc: &Document,
        primitives: &EvaluatedScene,
        ctx: &EvaluationContext,
    ) -> OperationEvaluation {
        let ids: Vec<OperationId> = doc.operations.order.clone();
        self.evaluate(doc, primitives, ctx, &ids)
    }

    /// Evaluate exactly the named operations.
    ///
    /// Ids that are disabled or no longer registered are reported as *not*
    /// recomputed (so the caller can retire their geometry), and nothing is
    /// emitted for them.
    pub fn evaluate(
        &self,
        doc: &Document,
        primitives: &EvaluatedScene,
        ctx: &EvaluationContext,
        ids: &[OperationId],
    ) -> OperationEvaluation {
        let mut out = OperationEvaluation::default();
        for id in ids {
            let Some(op) = doc.operations.get(*id) else {
                continue; // removed: the scene patch retires its geometry
            };
            if !op.enabled {
                continue; // parked: same treatment, inputs stay live
            }
            out.recomputed.push(*id);
            match self.evaluate_one(doc, primitives, ctx, *id) {
                Ok(node) => {
                    out.nodes.insert(*id, node);
                }
                Err(error) => out.diagnostics.push(operation_diagnostic(*id, &error, op)),
            }
        }
        out
    }

    /// One operation → one virtual node.
    fn evaluate_one(
        &self,
        doc: &Document,
        primitives: &EvaluatedScene,
        ctx: &EvaluationContext,
        id: OperationId,
    ) -> Result<EvaluatedNode, OperationError> {
        let op = doc
            .operations
            .get(id)
            .ok_or(OperationError::MissingOperation(id))?;

        // 1. Fetch the evaluated paths of the inputs and turn them into
        //    regions `geo` can reason about (RULE 2 pipeline, steps 1–2).
        let mut regions: Vec<MultiPolygon<f64>> = Vec::with_capacity(op.inputs.len());
        for input in &op.inputs {
            let node = primitives.get(*input).ok_or(OperationError::MissingInput {
                operation: id,
                input: *input,
            })?;
            let path = vectra_geometry::primitive_to_path(&node.primitive);
            let region = path_to_multi_polygon(&path).ok_or(OperationError::EmptyInput {
                operation: id,
                input: *input,
            })?;
            regions.push(region);
        }

        // 2. Resolve the scalar operands (radius / distance / axis) through the
        //    ordinary parameter path: an operation's numbers are as parametric
        //    as any other number in the document.
        let scalars = self.resolve_scalars(&op.kind, ctx, id)?;

        // 3. Compute the operation on regions.
        let result = ops::apply(&op.kind, &regions, &scalars)?;

        // 4. Back to a lyon path (pipeline step 4).
        let path = multi_polygon_to_path(&result);

        // 5. Package it as a standard evaluated node (pipeline step 5, RULE 3).
        Ok(EvaluatedNode {
            id: op.id,
            primitive: EvaluatedPrimitive::Path(path),
            style: resolve_style(&op.style, ctx),
            // A virtual node belongs to no layer and is never listed in the
            // Layers Panel, so it has no layer flag to inherit — its effective
            // presentation is its own, and an operation result is always visible
            // and always pickable.
            visible: true,
            locked: false,
        })
    }

    fn resolve_scalars(
        &self,
        kind: &OperationKind,
        ctx: &EvaluationContext,
        id: OperationId,
    ) -> Result<Vec<Option<f64>>, OperationError> {
        let value = match kind {
            OperationKind::Boolean { .. } => None,
            OperationKind::Offset { distance } => Some(distance),
            OperationKind::Fillet { radius } => Some(radius),
            OperationKind::Mirror { axis } => Some(match axis {
                vectra_core::MirrorAxis::Vertical { at } => at,
                vectra_core::MirrorAxis::Horizontal { at } => at,
            }),
        };
        match value {
            None => Ok(vec![None]),
            Some(parameter) => parameter
                .resolve(ctx)
                .map(|v| vec![Some(v)])
                .map_err(|error| OperationError::UnresolvableOperand {
                    operation: id,
                    message: error.to_string(),
                }),
        }
    }
}

/// Style resolution for a virtual node: the same fallbacks the primitive
/// evaluator uses, so a broken style channel degrades instead of skipping the
/// shape.
fn resolve_style(style: &StyleProperties, ctx: &EvaluationContext) -> EvaluatedStyle {
    // One implementation, shared with the primitive and procedural evaluators
    // (Task 10.2 RULE 3): an operation's paint stack resolves exactly like a
    // rectangle's.
    vectra_geometry::resolve_style(style, ctx)
}

fn operation_diagnostic(
    id: OperationId,
    error: &OperationError,
    op: &vectra_core::OperationNode,
) -> Diagnostic {
    let (code, severity) = match error {
        OperationError::EmptyInput { .. } => {
            (DiagnosticCode::OperationEmptyInput, Severity::Warning)
        }
        _ => (DiagnosticCode::OperationFailed, Severity::Warning),
    };
    Diagnostic {
        node_id: None, // the *inputs* are fine; the failure is the operation's
        property: Some(op.kind.describe()),
        severity,
        code,
        message: format!("operation {}: {error}", short(&id)),
    }
}

fn short(id: &OperationId) -> String {
    id.to_string().chars().take(8).collect()
}

/// JSON-visible view of a virtual node's path, used by the snapshot layer (and
/// debug tooling) instead of re-deriving SVG data at every call site.
pub fn node_path_data(node: &EvaluatedNode) -> Option<String> {
    match &node.primitive {
        EvaluatedPrimitive::Path(path) => Some(path_to_svg_data(path)),
        _ => None,
    }
}
