//! Arc-angle canonicalization (Task 1.3).
//!
//! Authors write angles in any winding (`-90°`, `720°`, …). The evaluated
//! scene stores one canonical form so the renderer and tessellator never
//! branch on wrap-around:
//!
//! * `start ∈ [0, TAU)`
//! * `end = start + sweep`, `sweep ∈ [0, TAU]`
//! * `sweep == 0` ⟺ empty arc; `sweep == TAU` ⟺ full circle
//!
//! Direction is always increasing-angle from `start`. Multi-turn windings
//! (e.g. `0 → 4π`) collapse to a single full turn: a static arc cannot
//! represent winding count, and tessellation is identical.

use std::f64::consts::TAU as PI2;

pub use std::f64::consts::TAU;

/// Canonicalize raw arc angles. Inputs must be finite (callers skip the node
/// with a diagnostic otherwise — see [`crate::evaluator`]).
///
/// Returns `(start_normalized, end_with_sweep)` per the module contract.
pub fn normalize_arc_angles(start: f64, end: f64) -> (f64, f64) {
    debug_assert!(start.is_finite() && end.is_finite());
    // `+ 0.0` folds a possible `-0.0` from `rem_euclid` into canonical `0.0`.
    let start_n = start.rem_euclid(PI2) + 0.0;
    let raw_delta = end - start;
    let mut sweep = raw_delta.rem_euclid(PI2);
    if sweep == 0.0 && raw_delta != 0.0 {
        // Non-zero winding that lands back on its start: full circle.
        sweep = PI2;
    }
    (start_n, start_n + sweep)
}

/// Sweep of a canonicalized arc: `end - start ∈ [0, TAU]`.
pub fn sweep_of(start_normalized: f64, end_with_sweep: f64) -> f64 {
    end_with_sweep - start_normalized
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::{FRAC_PI_2, PI};

    fn approx(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-12
    }

    #[test]
    fn in_range_angles_are_identity() {
        assert_eq!(normalize_arc_angles(0.0, PI), (0.0, PI));
        assert_eq!(normalize_arc_angles(1.0, 2.0), (1.0, 2.0));
    }

    #[test]
    fn negative_start_wraps_forward() {
        let (s, e) = normalize_arc_angles(-FRAC_PI_2, 0.0);
        assert!(approx(s, PI + FRAC_PI_2));
        assert!(approx(e - s, FRAC_PI_2));
    }

    #[test]
    fn end_before_start_wraps_through_tau() {
        // 350° → 10° is a 20° sweep crossing 0.
        let (s, e) = normalize_arc_angles(350.0_f64.to_radians(), 10.0_f64.to_radians());
        assert!(approx(e - s, 20.0_f64.to_radians()));
        assert!((0.0..PI2).contains(&s));
    }

    #[test]
    fn full_turn_becomes_tau_sweep() {
        let (s, e) = normalize_arc_angles(0.0, PI2);
        assert_eq!(s, 0.0);
        assert_eq!(e - s, PI2);
    }

    #[test]
    fn multi_turn_collapses_to_single_full_turn() {
        let (s, e) = normalize_arc_angles(0.0, 4.0 * PI2);
        assert_eq!(e - s, PI2);
    }

    #[test]
    fn equal_angles_are_empty_sweep() {
        let (s, e) = normalize_arc_angles(1.0, 1.0);
        assert_eq!((s, e), (1.0, 1.0));
    }

    #[test]
    fn negative_zero_start_is_canonicalized() {
        let (s, _) = normalize_arc_angles(-0.0, 1.0);
        assert!(!s.is_sign_negative(), "start must be +0.0, got {s:?}");
    }
}
