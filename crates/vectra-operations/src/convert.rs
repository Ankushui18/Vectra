//! Re-export shim (Task 12.0): the lyon ⇄ `geo` boundary.
//!
//! The converter now lives in [`vectra_geometry::convert`] because the Region
//! Graph (Task 12.0 RULE 1) needs it, and `vectra-operations` already depends
//! on `vectra-geometry` — so putting it there is the only arrangement with no
//! dependency cycle.
//!
//! Every existing path keeps working: this module re-exports the same names, so
//! `crate::convert::region_area` and friends resolve exactly as before.

pub use vectra_geometry::convert::*;
