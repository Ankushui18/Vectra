//! `vectra-export` — the export IR, and the exporters that consume it.
//!
//! Task 8.0 turns Vectra from a design tool into a **code-generation engine**.
//! The shape is three rules and one pipeline:
//!
//! ```text
//!   Document ─┐
//!             ├──▶ compile_to_ir ──▶ ExportIR ──┬──▶ export_svg    (RULE 1: semantic tags)
//!   Scene ────┘                                  ├──▶ export_react  (RULE 2: parametric props)
//!                                                └──▶ …Vue, Svelte: another consumer of the IR
//! ```
//!
//! * **RULE 1 — semantic first.** A circle exports as `<circle>`, an arc as an
//!   SVG `A` command, and only a true path as `<path>`. Nothing is approximated
//!   because nothing has to be: the IR carries the primitive, not its
//!   tessellation.
//! * **RULE 2 — parametric codegen.** A slot driven by `$base * 2` exports as
//!   `width={base * 2}` with `base` declared as a required prop — the document's
//!   *variables*, not the numbers they happened to hold.
//! * **RULE 3 — an explicit IR.** Both exporters consume [`ExportIR`]; neither
//!   reads a [`vectra_core::Document`] or an [`vectra_geometry::EvaluatedScene`].
//!   A new target language is a new `export_*` function and nothing else.
//!
//! The crate depends on core (the document), geometry (the resolved picture and
//! lyon's path serialization) and expression (the *source* of an expression, which
//! is what RULE 2 exports). It depends on no framework: the React exporter writes
//! text, it does not link a renderer.
//!
//! ## Quick start
//!
//! ```
//! use vectra_core::{Command, Document, Engine, NodeKind};
//! use vectra_export::{compile_to_ir, export_react, export_svg};
//!
//! let mut engine = Engine::new();
//! engine
//!     .dispatch(Command::CreateNode {
//!         id: vectra_core::new_node_id(),
//!         kind: NodeKind::circle(0.0, 0.0, 10.0),
//!         name: Some("dot".to_string()),
//!         index: None,
//!     })
//!     .unwrap();
//! let ir = compile_to_ir(engine.document());
//! assert!(export_svg(&ir).contains("<circle"));
//! assert!(export_react(&ir).contains("export function Scene"));
//! ```

pub mod ir;
pub mod react;
pub mod svg;

pub use ir::{
    compile_to_ir, compile_to_ir_resolved, fmt_number, sanitize_ident, translate_expression,
    ExportBounds, ExportColor, ExportGeometry, ExportIR, ExportNode, ExportParam, ExportProp,
    ExportPropType, ExportStyle,
};
pub use react::{export_react, export_react_with, ReactOptions};
pub use svg::{
    export_artboards_svg, export_svg, export_svg_with, ArtboardScope, ArtboardView, SvgOptions,
};

use thiserror::Error;

/// Export-level errors.
///
/// The two exporters are **total**: any IR produces output (an empty document
/// produces an empty picture, with the reason in `ExportIR::warnings`), because a
/// user who clicks Export must always get a file to look at. This type exists for
/// the paths that are genuinely fallible — a desktop caller writing to disk, a
/// future importer — so the crate has one error vocabulary from the start.
#[derive(Debug, Error)]
pub enum ExportError {
    /// Anything an exporter could not do, with the reason.
    #[error("export: {0}")]
    Other(String),
}
