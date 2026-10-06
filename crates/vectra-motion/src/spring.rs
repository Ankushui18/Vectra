//! The spring: an analytic, time-pure easing toward a target (Task 6.0 §D1).
//!
//! # Why this is a formula and not a loop
//!
//! [`vectra_core::eval::MotionEvaluator::evaluate`] is handed a binding and a
//! *time* — never a frame delta, never a previous state. That signature is the
//! whole design: motion must be **pure in `t`**, so scrubbing backwards, replay,
//! undo, a headless test and a 144 Hz monitor all produce the same value.
//!
//! A frame-by-frame integrator cannot promise that. It makes the document a
//! function of the frame rate, which means the same edit yields different pixels
//! on different machines — and there is no way to test it except by replaying
//! the same frame sequence, which tests the replay, not the spring.
//!
//! So the spring is the **closed-form solution** of `m·ẍ + c·ẋ + k·x = 0` with
//! `x(0) = 0, ẋ(0) = 0` (a spring at rest at its anchor), where `x = value −
//! target`. Every damping ratio has one, and they are all cheap: one `exp`, one
//! `cos`/`sin` or `cosh`/`sinh`.
//!
//! Parameterisation follows the one designers already know from CSS and Rive —
//! `stiffness` and `damping` — rather than raw `ω`/`ζ`:
//!
//! ```text
//! ω = √(stiffness)         the undamped angular frequency
//! ζ = damping / (2·√stiffness)      the damping ratio
//! ```
//!
//! # Why it is not a spring over-damped into oatmeal
//!
//! No state, no velocity: the spring leaves its anchor with **zero initial
//! velocity** and therefore never overshoots unless it is under-damped
//! (`ζ < 1`). That is a documented property, not a limitation — see
//! `tests/motion_laws.rs::law_spring_never_overshoots_when_over_or_critically_damped`.
//! A spring that needs to carry momentum into a new target (an interrupted
//! animation) is expressed by **re-anchoring**: the document writes a new
//! `from`/`at` pair at the moment the target changes, which is exactly the
//! re-anchor an `EndDrag` commits, and it stays pure in `t`.

/// A spring's parameters and its anchor, in the slot's own units and engine
/// seconds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Spring {
    pub stiffness: f64,
    pub damping: f64,
    /// The value the spring is at rest at, at time [`Spring::at`].
    pub from: f64,
    /// Engine time at which `from` held.
    pub at: f64,
}

/// The natural frequency and damping ratio derived from stiffness/damping.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpringDynamics {
    /// Undamped angular frequency, radians per second.
    pub omega: f64,
    /// Damping ratio: `< 1` under-damped (overshoots), `1` critical, `> 1` over.
    pub zeta: f64,
}

impl SpringDynamics {
    /// `ω = √k`, `ζ = c / (2√k)` — the standard second-order mapping.
    pub fn of(stiffness: f64, damping: f64) -> Self {
        let omega = stiffness.max(0.0).sqrt();
        let zeta = if omega > 0.0 {
            damping / (2.0 * omega)
        } else {
            f64::INFINITY
        };
        Self { omega, zeta }
    }

    /// Whether the spring overshoots its target (under-damped).
    pub fn overshoots(&self) -> bool {
        self.zeta < 1.0
    }
}

impl Spring {
    pub fn new(stiffness: f64, damping: f64, from: f64, at: f64) -> Self {
        Self {
            stiffness,
            damping,
            from,
            at,
        }
    }

    pub fn dynamics(&self) -> SpringDynamics {
        SpringDynamics::of(self.stiffness, self.damping)
    }

    /// The spring's value at engine time `t`.
    ///
    /// * `t < at` (and `at` itself) → `from`: before the anchor the spring has
    ///   not started, which is what makes a *jump* backwards in time behave.
    /// * `t ≥ at` → the closed form of the second-order response to a step from
    ///   `from` to `target`, at rest.
    pub fn value_at(&self, target: f64, t: f64) -> f64 {
        let dt = t - self.at;
        if !dt.is_finite() || dt <= 0.0 {
            return self.from;
        }
        let d = self.dynamics();
        if d.omega == 0.0 || !d.omega.is_finite() {
            // k = 0: no restoring force, so nothing moves toward the target.
            return self.from;
        }
        self.from + (target - self.from) * overshoot_ratio(d, dt)
    }

    /// The same response expressed as a 0→1 progress curve, for the UI and for
    /// the law tests.
    pub fn progress_at(&self, t: f64) -> f64 {
        self.value_at(1.0, t) - self.value_at(0.0, t)
    }

    /// Whether the spring has settled: its remaining distance to `target` is
    /// under `epsilon` and it is close enough to be called arrived.
    ///
    /// The bound is the *slowest* decay envelope of the response, so it is an
    /// upper bound on the true error for every damping ratio — used for the
    /// Idle Law, where being conservative costs a few extra frames and being
    /// optimistic would leave the loop running forever.
    pub fn settled_at(&self, target: f64, t: f64, epsilon: f64) -> bool {
        let dt = t - self.at;
        if !dt.is_finite() || dt <= 0.0 {
            return (target - self.from).abs() <= epsilon;
        }
        let d = self.dynamics();
        if d.omega == 0.0 || !d.omega.is_finite() {
            return (target - self.from).abs() <= epsilon;
        }
        let distance = (target - self.from).abs();
        if distance <= epsilon {
            return true;
        }
        distance * envelope(d, dt) <= epsilon
    }

    /// How long until [`Spring::settled_at`] holds, in seconds (0 if already
    /// there).
    ///
    /// Solved against **the same `envelope` that `settled_at` tests**, by
    /// bisection on `[0, bracket]` — the envelope is monotone decreasing but not
    /// a single exponential in every regime (the critical form carries a
    /// `(1 + ωt)` factor), and solving a *different* function than the one
    /// asserted is exactly the bug the first run of this module's tests caught:
    /// the predicted time came back early and `settled_at` said "not yet".
    ///
    /// Bisection is cheap (≈50 iterations of a couple of exponentials, once per
    /// frame per animating slot) and, more importantly, consistent by
    /// construction: one function, two consumers.
    pub fn settle_time(&self, target: f64, epsilon: f64) -> f64 {
        let distance = (target - self.from).abs();
        if distance <= epsilon {
            return 0.0;
        }
        let d = self.dynamics();
        if d.omega == 0.0 || !d.omega.is_finite() {
            // k = 0 never converges. Report 0 — "nothing is animating" — rather
            // than a horizon that would keep the frame loop alive forever. The
            // command layer refuses such a spring at the boundary anyway.
            return 0.0;
        }
        let ratio = (epsilon / distance).clamp(f64::MIN_POSITIVE, 1.0);

        // Bracket: double until the envelope is under the target ratio. The
        // first guess is the slowest exponential's time constant.
        let decay = match damping_class(d.zeta) {
            Class::Under => d.zeta * d.omega,
            Class::Critical => d.omega,
            Class::Over => d.omega * (d.zeta - (d.zeta * d.zeta - 1.0).sqrt()),
        };
        let mut hi = (2.0 * (1.0 / ratio).ln() / decay).max(1e-3);
        let mut guard = 0;
        while envelope(d, hi) > ratio && guard < 64 {
            hi *= 2.0;
            guard += 1;
        }
        if envelope(d, hi) > ratio {
            return 0.0; // unreachable for the accepted parameter range
        }
        let mut lo = 0.0f64;
        for _ in 0..60 {
            let mid = 0.5 * (lo + hi);
            if envelope(d, mid) > ratio {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        hi
    }
}

/// The three closed forms, chosen by damping ratio.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Class {
    /// `ζ < 1` — decaying oscillation.
    Under,
    /// `ζ = 1` — the fastest non-overshooting return.
    Critical,
    /// `ζ > 1` — two real exponentials.
    Over,
}

/// Equality tolerance for the damping ratio. `ζ` is derived from user-supplied
/// `stiffness`/`damping`, so exact `1.0` is a measure-zero input; the band
/// matters because the critical formula is the *limit* of both neighbours and
/// evaluating the wrong one near `ζ = 1` amplifies a tiny input difference into
/// visibly different motion. 1e-6 is far tighter than any visual difference and
/// far looser than float noise on the derived value.
const CRITICAL_BAND: f64 = 1e-6;

fn damping_class(zeta: f64) -> Class {
    if (zeta - 1.0).abs() <= CRITICAL_BAND {
        Class::Critical
    } else if zeta < 1.0 {
        Class::Under
    } else {
        Class::Over
    }
}

/// `x(t) / x(∞)`: the normalised step response, all three regimes.
///
/// With zero initial velocity:
///
/// ```text
/// ζ < 1 :  1 − e^(−ζωt)·(cos(ω_d t) + (ζω/ω_d)·sin(ω_d t)),   ω_d = ω√(1−ζ²)
/// ζ = 1 :  1 − e^(−ωt)·(1 + ωt)
/// ζ > 1 :  1 − e^(−ωt)·(ζ·sinh(ω_r t)/√(ζ²−1) + cosh(ω_r t)),  ω_r = ω√(ζ²−1)
/// ```
///
/// Each branch satisfies `x(0) = 0`, `ẋ(0) = 0`, `x(∞) = 1` — the laws assert
/// all three against finite differences of this very function, so a sign slip in
/// any branch fails a test rather than shipping.
fn overshoot_ratio(d: SpringDynamics, dt: f64) -> f64 {
    let (omega, zeta) = (d.omega, d.zeta);
    match damping_class(zeta) {
        Class::Under => {
            let omega_d = omega * (1.0 - zeta * zeta).max(0.0).sqrt();
            if omega_d <= 0.0 {
                return 1.0;
            }
            1.0 - (-zeta * omega * dt).exp()
                * ((omega_d * dt).cos() + (zeta * omega / omega_d) * (omega_d * dt).sin())
        }
        Class::Critical => 1.0 - (-omega * dt).exp() * (1.0 + omega * dt),
        Class::Over => {
            // Two real decaying exponentials, roots r± = −ζω ± ω_r, ω_r = ω√(ζ²−1):
            //
            //     x/x∞ = 1 − (r₊·e^(r₋t) − r₋·e^(r₊t)) / (r₊ − r₋)
            //
            // The textbook equivalent, `1 − e^(−ζωt)·[cosh + (ζω/ω_r)·sinh]`, is
            // mathematically identical and numerically *hostile*: `cosh(ω_r t)`
            // overflows to ∞ long before `e^(−ζωt)` underflows to 0, and `∞ × 0`
            // is NaN. At k=170, c=40 that happens around t ≈ 55 s — well inside
            // the range a scrubber reaches. Both exponentials here decay, so the
            // result is exact at every `t` and reaches exactly 1 in the limit.
            let omega_r = omega * (zeta * zeta - 1.0).sqrt();
            if omega_r <= 0.0 {
                return 1.0;
            }
            let (r_plus, r_minus) = (-zeta * omega + omega_r, -zeta * omega - omega_r);
            1.0 - (r_plus * (r_minus * dt).exp() - r_minus * (r_plus * dt).exp())
                / (r_plus - r_minus)
        }
    }
}

/// Upper bound on the remaining error fraction at `dt`.
///
/// * `ζ < 1`: `e^(−ζωt)·(1 + ζ/√(1−ζ²))` — the oscillating bracket is bounded
///   by its two amplitudes.
/// * `ζ = 1`: the response itself, `e^(−ωt)·(1 + ωt)`.
/// * `ζ > 1`: `e^(−ζωt)·(cosh(ω_r t) + ζ·sinh(ω_r t)/√(ζ²−1))`, again the
///   response itself (both exponential terms are positive here).
///
/// Every branch is **monotone decreasing** in `dt` (the derivative of the
/// critical form is `−ω²t·e^(−ωt) ≤ 0`; the others are sums of decaying
/// exponentials), which is what makes the settle-time solve below well defined.
fn envelope(d: SpringDynamics, dt: f64) -> f64 {
    let (omega, zeta) = (d.omega, d.zeta);
    match damping_class(zeta) {
        Class::Under => {
            let root = (1.0 - zeta * zeta).max(f64::MIN_POSITIVE).sqrt();
            (-zeta * omega * dt).exp() * (1.0 + zeta / root)
        }
        Class::Critical => (-omega * dt).exp() * (1.0 + omega * dt),
        Class::Over => {
            let omega_r = omega * (zeta * zeta - 1.0).max(f64::MIN_POSITIVE).sqrt();
            let (r_plus, r_minus) = (-zeta * omega + omega_r, -zeta * omega - omega_r);
            // |x/x∞| = |r₊e^(r₋t) − r₋e^(r₊t)|/(r₊−r₋) ≤ ((−r₋)e^(r₊t) + (−r₊)e^(r₋t))/(r₊−r₋).
            // Monotone decreasing (the derivative is −2r₊r₋·e^(r₊+r₋)t/(r₊−r₋) < 0,
            // and r₊r₋ = ω² > 0), which is what makes the bisection in
            // `settle_time` well defined.
            ((-r_minus) * (r_plus * dt).exp() + (-r_plus) * (r_minus * dt).exp())
                / (r_plus - r_minus)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TARGET: f64 = 1.0;

    #[test]
    fn the_step_response_starts_at_the_anchor_with_no_velocity() {
        for (stiffness, damping) in [(170.0, 26.0), (170.0, 20.0), (170.0, 40.0), (100.0, 20.0)] {
            let spring = Spring::new(stiffness, damping, 0.0, 3.0);
            assert_eq!(spring.value_at(TARGET, 3.0), 0.0, "t == anchor");
            assert_eq!(spring.value_at(TARGET, 2.9), 0.0, "t < anchor");
            // Zero *initial velocity* is a statement about the leading terms:
            // with x(0) = 0 and ẋ(0) = 0 the response is O(dt²), so the
            // displacement after `dt` must be bounded by the second-order term
            // and, crucially, must not have a first-order (velocity) term.
            let d = spring.dynamics();
            let dt = 1e-5;
            let displacement = spring.value_at(TARGET, 3.0 + dt) - spring.value_at(TARGET, 3.0);
            assert!(displacement > 0.0, "the spring starts moving");
            assert!(
                displacement <= d.omega * d.omega * dt * dt,
                "O(dt²) start: {displacement} > {}",
                d.omega * d.omega * dt * dt
            );
        }
    }

    #[test]
    fn the_step_response_converges_to_the_target() {
        for (stiffness, damping) in [(170.0, 26.0), (170.0, 20.0), (170.0, 40.0)] {
            let spring = Spring::new(stiffness, damping, 0.0, 0.0);
            let far = spring.value_at(TARGET, 50.0);
            assert!(
                (far - TARGET).abs() < 1e-9,
                "k={stiffness} c={damping}: settled at {far}"
            );
        }
    }

    #[test]
    fn an_over_damped_spring_decays_without_oscillating() {
        let spring = Spring::new(170.0, 40.0, 0.0, 0.0);
        let mut previous = 0.0;
        for step in 0..400 {
            let value = spring.value_at(TARGET, step as f64 * 0.01);
            assert!(
                value >= previous - 1e-12,
                "monotone at step {step}: {value} < {previous}"
            );
            assert!(value <= 1.0 + 1e-12, "never overshoots: {value}");
            previous = value;
        }
    }

    #[test]
    fn the_dynamics_mapping_matches_the_textbook() {
        let d = SpringDynamics::of(170.0, 26.0);
        assert!((d.omega - 170.0f64.sqrt()).abs() < 1e-12);
        assert!((d.zeta - 26.0 / (2.0 * 170.0f64.sqrt())).abs() < 1e-12);
        // Critical damping is exactly c = 2√k.
        let critical = SpringDynamics::of(170.0, 2.0 * 170.0f64.sqrt());
        assert!((critical.zeta - 1.0).abs() < 1e-12);
        assert!(!critical.overshoots());
    }

    #[test]
    fn settle_time_is_an_upper_bound_and_not_pessimistic() {
        for (stiffness, damping) in [(170.0, 26.0), (170.0, 20.0), (170.0, 40.0)] {
            let spring = Spring::new(stiffness, damping, 0.0, 0.0);
            let horizon = spring.settle_time(TARGET, 1e-3);
            assert!(horizon > 0.0, "k={stiffness} c={damping} should settle");
            // It holds at the predicted time…
            assert!(
                spring.settled_at(TARGET, horizon, 1e-3),
                "k={stiffness} c={damping}: predicted {horizon}s but not settled"
            );
            // …and not much later: half the predicted time must still be moving,
            // so the estimate is tight to within a factor of two rather than
            // "some number that eventually works". (Over-damped springs are
            // genuinely slow — ζ=1.53 needs ~1.5 s to reach 1e-3 — so the check
            // is about tightness, not about being sub-second.)
            assert!(
                !spring.settled_at(TARGET, horizon * 0.5, 1e-3),
                "k={stiffness} c={damping}: {horizon}s is a loose bound"
            );
            assert!(
                horizon < 5.0,
                "k={stiffness} c={damping} settles at {horizon}s"
            );
        }
    }

    /// Regression: the response must stay finite — and land exactly on the
    /// target — at times far past any realistic scrub. The first version of the
    /// over-damped branch returned NaN here (cosh overflow × exp underflow), a
    /// bug that any *finite* dt up to ~55 s hid completely.
    /// The **Spring Convergence Law**'s second half, as a family: for every
    /// damping ratio at or above critical the response is monotone in the
    /// settling direction and never crosses the target. Checked across the band
    /// — including exactly critical, and just either side of it, where the
    /// closed form switches branch and a sign slip would show up as a wobble.
    #[test]
    fn no_spring_overshoots_at_or_above_critical_damping() {
        let omega = 170.0f64.sqrt();
        let family = [
            1.0,        // exactly critical
            1.0 + 1e-9, // a hair over (the Over branch)
            1.0 - 1e-9, // a hair under (the Under branch, barely oscillating)
            1.05,
            1.5,
            3.0,
        ];
        for zeta in family {
            let spring = Spring::new(170.0, zeta * 2.0 * omega, 0.0, 0.0);
            let mut previous = 0.0;
            for step in 0..=2000 {
                let value = spring.value_at(TARGET, step as f64 * 0.005);
                assert!(
                    value >= previous - 1e-9,
                    "ζ={zeta}: not monotone at step {step} ({previous} → {value})"
                );
                assert!(
                    value <= TARGET + 1e-9,
                    "ζ={zeta}: overshot at step {step}: {value}"
                );
                previous = value;
            }
        }
    }

    /// …and a genuinely under-damped spring *does* overshoot — bounded by the
    /// ratio the closed form predicts, so the first peak is checkable:
    /// `1 + e^(−ζπ/√(1−ζ²))`.
    #[test]
    fn an_under_damped_spring_overshoots_by_the_predicted_ratio() {
        let omega = 170.0f64.sqrt();
        for zeta in [0.2, 0.4, 0.6] {
            let spring = Spring::new(170.0, zeta * 2.0 * omega, 0.0, 0.0);
            let predicted = 1.0 + (-zeta * std::f64::consts::PI / (1.0 - zeta * zeta).sqrt()).exp();
            let mut peak: f64 = 0.0;
            for step in 0..=4000 {
                peak = peak.max(spring.value_at(TARGET, step as f64 * 0.0025));
            }
            assert!(
                (peak - predicted).abs() < 1e-4,
                "ζ={zeta}: peak {peak} ≠ predicted {predicted}"
            );
            assert!(peak > TARGET, "ζ={zeta} should overshoot");
        }
    }

    #[test]
    fn the_response_is_stable_at_absurd_times() {
        // The last pair is deliberately sluggish (ω ≈ 0.71, ζ ≈ 2.1): its slow
        // root has a 5.5 s time constant, so "has it arrived?" must be judged
        // against the spring's *own* horizon, not a fixed number of seconds.
        for (stiffness, damping) in [(170.0, 26.0), (170.0, 20.0), (170.0, 40.0), (0.5, 3.0)] {
            let spring = Spring::new(stiffness, damping, 0.0, 0.0);
            let arrived = spring.settle_time(TARGET, 1e-9);
            for t in [60.0, 1e3, 1e6, 1e12] {
                let value = spring.value_at(TARGET, t);
                assert!(
                    value.is_finite(),
                    "k={stiffness} c={damping} t={t}: {value}"
                );
                if t >= 2.0 * arrived {
                    assert!(
                        (value - TARGET).abs() < 1e-9,
                        "k={stiffness} c={damping} t={t} (horizon {arrived}): {value} ≠ target"
                    );
                }
            }
        }
    }

    #[test]
    fn a_zero_stiffness_spring_does_not_move() {
        let spring = Spring::new(0.0, 26.0, 5.0, 0.0);
        assert_eq!(spring.value_at(TARGET, 100.0), 5.0);
        assert_eq!(spring.settle_time(TARGET, 1e-3), 0.0);
    }
}
