//! Curve fitting: a sampled stroke becomes a **clean cubic path** (Task 10.1
//! RULE 3).
//!
//! RULE 3's demand is precise: *"outputs a clean, expanded `Path` using our
//! existing Bézier engine, not a jagged polyline."* So this module's contract is
//! that its output is a list of [`PathSegment`]s — engine types, with
//! `Parameter<Point2>` control points — and never an array of samples.
//!
//! ## The method
//!
//! The classic Schneider fit (Graphics Gems, "An Algorithm for Automatically
//! Fitting Digitized Curves"), which is what a vector brush actually runs:
//!
//! ```text
//!   ┌─ given points P0…Pn and end tangents ──────────────────────────────┐
//!   │ 1. parameterise by chord length          u_i ∈ [0,1]              │
//!   │ 2. least-squares solve for the two handles (Bernstein basis)       │
//!   │ 3. measure the worst perpendicular error                           │
//!   │ 4. error ≤ tol  →  accept                                          │
//!   │    error at an interior sample → split there, recurse on both halves│
//!   │    error at an end → reparameterise once (Newton) and retry         │
//!   └────────────────────────────────────────────────────────────────────┘
//! ```
//!
//! Step 4's split is what makes the result *adaptive*: a straight run of samples
//! costs one segment, a tight corner costs several, and the user never picks a
//! "smoothness" setting.
//!
//! ## Why it terminates, twice over
//!
//! RULE 4's Curve Validity Law asks that a generated path "flatten without NaNs
//! or infinite loops". Two guarantees, both structural rather than hoped for:
//!
//! * a split must be at an **interior** sample (`0 < i < n-1`); if the worst
//!   error is at an endpoint there is nothing to split, so the code falls back to
//!   lines instead of recursing on the same input;
//! * [`MAX_DEPTH`] caps the recursion regardless, so even a pathological input
//!   (a thousand samples on one pixel) terminates. The bound is asserted by
//!   [`FitStats::max_depth`] in the tests.

use vectra_core::{PathSegment, Point2};

use crate::bezier::{cubic_at, cubic_deviation_from_chord, distance, literal};
use crate::stroke::StampedPoint;

/// The recursion bound, and therefore the point at which the fit stops trying to
/// describe a stretch with a curve and hands back the samples themselves.
///
/// 12 halvings is far past the resolution of any display, and a normal stroke
/// never comes close. It is reachable in exactly one situation, and the fidelity
/// law produced it: a trail whose consecutive samples jump hundreds of units in
/// unpredictable directions — a "stroke" that is really a sequence of
/// discontinuities, which no pointer produces and no curve can describe. Between
/// a beautiful cubic that misses a sample by 22 units and the samples themselves,
/// **exact wins**, so the cap emits the polyline for that stretch. That is the
/// one place the brush can return a non-curve, it is bounded by the number of
/// samples (every leaf covers at least one gap), and it is a refusal to lie
/// rather than a limitation of the method.
pub const MAX_DEPTH: u32 = 12;

/// Newton reparameterisation rounds per fit attempt. Three is the reference
/// algorithm's choice and is past the point of diminishing returns: each round
/// roughly squares the parameter error near the optimum.
pub const REPARAMETERISE_STEPS: u32 = 3;

/// How far a control point may sit from its own anchor, as a multiple of the
/// stretch's chord.
///
/// The handle lengths come out of a 2×2 least-squares solve, and that solve is
/// **ill-conditioned** when the two end tangents are nearly parallel while the
/// samples demand a hook: the answer is a pair of enormous handles whose curve
/// swings hundreds of times the chord away and comes back. Such a "fit" is not
/// merely ugly, it is *unmeasurable* by any polyline: a coarse one cuts across
/// the loop and can pass closer to a sample than the curve itself does, so the
/// candidate scores as if it were tight. (Measured on a nine-sample stroke: a
/// candidate with 400 000-unit handles was scored 12.17 by the acceptance test
/// while the curve was 57.9 from one of its own samples.) Anything past this
/// ratio is the solver leaking, not a curve anyone drew, so it is refused and the
/// stretch is split instead — where the worst case is the sample polyline, which
/// is exact.
pub const MAX_HANDLE_RATIO: f64 = 4.0;

/// What a fit did — the numbers a status line (or a test) can check.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FitStats {
    /// Samples handed in.
    pub samples: usize,
    /// Segments produced.
    pub segments: usize,
    /// Straight segments among them.
    pub lines: usize,
    /// Curved segments among them.
    pub cubics: usize,
    /// Worst perpendicular error of the accepted fit, in document units.
    pub max_error: f64,
    /// Deepest recursion reached.
    pub max_depth: u32,
}

/// A fitted stroke plus its statistics.
#[derive(Debug, Clone, PartialEq)]
pub struct FitResult {
    /// The first point of the path (`NodeKind::Path::start`).
    pub start: Point2,
    /// The segments from `start` onward.
    pub segments: Vec<PathSegment>,
    pub stats: FitStats,
}

/// One straight run of the fit.
fn line_to(points: &[Point2], from: usize, to: usize) -> PathSegment {
    debug_assert!(to > from);
    let _ = points;
    PathSegment::Line {
        to: literal(points[to]),
    }
}

/// **Fit a sampled stroke to cubic Béziers** (the brush's core).
///
/// `tolerance` is in document units: the maximum distance the curve may stray
/// from the samples. One document unit is roughly one screen pixel at 100 % zoom,
/// which is why the default the UI passes is in that neighbourhood — below it the
/// user cannot see the difference, above it the stroke stops feeling like what
/// they drew.
pub fn fit_stroke(points: &[Point2], tolerance: f64) -> FitResult {
    let tolerance = tolerance.max(1e-6);
    let mut segments = Vec::new();
    let mut stats = FitStats {
        samples: points.len(),
        segments: 0,
        lines: 0,
        cubics: 0,
        max_error: 0.0,
        max_depth: 0,
    };
    let start = points.first().copied().unwrap_or(Point2::ZERO);
    if points.len() < 2 {
        return FitResult {
            start,
            segments,
            stats,
        };
    }

    // The fast path, and it is not an optimisation detail: a straight run of
    // samples is *exactly* a line, and a line is a smaller, cleaner, more
    // editable document than a pair of cubics that happen to be nearly straight.
    if is_straight(points, tolerance) {
        segments.push(line_to(points, 0, points.len() - 1));
        stats.segments = 1;
        stats.lines = 1;
        return FitResult {
            start,
            segments,
            stats,
        };
    }

    // End tangents, in Schneider's convention: `t1` points *forward* from the
    // first sample, `t2` points *backward* from the last one (so that
    // `P3 + t2·α` walks back along the curve instead of past its end). Getting
    // this sign wrong does not fail loudly — it silently fits an S-shaped curve
    // through every stroke — so it is called out here and checked by the
    // quarter-circle test.
    let tangent_start = unit(points[0], points[1]);
    let tangent_end = unit(points[points.len() - 1], points[points.len() - 2]);

    fit_recursive(
        points,
        0,
        points.len() - 1,
        tangent_start,
        tangent_end,
        tolerance,
        0,
        &mut segments,
        &mut stats,
    );

    stats.segments = segments.len();
    stats.lines = segments
        .iter()
        .filter(|segment| matches!(segment, PathSegment::Line { .. }))
        .count();
    stats.cubics = segments
        .iter()
        .filter(|segment| matches!(segment, PathSegment::Cubic { .. }))
        .count();

    FitResult {
        start,
        segments,
        stats,
    }
}

/// Schneider's recursion, with the two termination guards.
#[allow(clippy::too_many_arguments)]
fn fit_recursive(
    points: &[Point2],
    first: usize,
    last: usize,
    tangent_start: Point2,
    tangent_end: Point2,
    tolerance: f64,
    depth: u32,
    out: &mut Vec<PathSegment>,
    stats: &mut FitStats,
) {
    stats.max_depth = stats.max_depth.max(depth);
    let count = last - first + 1;

    // Two samples: a line is the exact answer, and cheaper than a degenerate
    // cubic.
    if count == 2 {
        out.push(line_to(points, first, last));
        return;
    }

    let slice = &points[first..=last];
    let mut params = chord_length_params(slice);
    let (control1, control2) = match generate_bezier(slice, &params, tangent_start, tangent_end) {
        Some(handles) => handles,
        // A degenerate solve — all samples coincident, or a matrix that came out
        // non-finite — has no cubic to offer for *this* range. It is not the end
        // of the story: a range with interior samples is split in half and each
        // half tried on its own, which converges on the sample polyline. Emitting
        // one line across the whole range would throw the interior samples away —
        // a fit that is not merely loose but *wrong*, and the fidelity law caught
        // exactly that (a stroke with a repeated point came back 12.9 units off at
        // a 0.5 tolerance).
        None => {
            if count <= 2 {
                // Two samples *are* a line: nothing is approximated, and the
                // recursion has nowhere left to split.
                out.push(line_to(points, first, last));
                return;
            }
            if depth >= MAX_DEPTH {
                // Out of budget inside a failed solve. Emit the samples
                // themselves — the same answer the error-driven cap below gives,
                // and exact by construction — rather than one line across a
                // range that is not one. A line here is not merely loose: it
                // throws away every interior sample, and it was measured doing
                // exactly that (53 units off at a 12-unit tolerance, on a
                // 9-sample stroke whose solve failed only at the cap). The
                // fidelity law caught it.
                for index in first..last {
                    out.push(line_to(points, index, index + 1));
                }
                return;
            }
            let split = first + count / 2;
            let tangent = split_tangent(points, split, first, last);
            fit_recursive(
                points,
                first,
                split,
                tangent_start,
                tangent,
                tolerance,
                depth + 1,
                out,
                stats,
            );
            fit_recursive(
                points,
                split,
                last,
                tangent,
                tangent_end,
                tolerance,
                depth + 1,
                out,
                stats,
            );
            return;
        }
    };
    // **Reparameterise, then refit** — Schneider's refinement, and the step that
    // decides whether the brush is usable. The first pass fixes the parameters
    // from chord length, which is a guess: on a curve of uneven speed the sample
    // that "should" be at t = 0.5 is not at half the arc length. Three Newton
    // steps walk each parameter toward the point on the current curve nearest its
    // sample, and the handles are solved again against the corrected parameters.
    // Without this a 90° arc needs 26 segments to reach a half-unit tolerance;
    // with it, one segment reaches ~0.02 — a 100× smaller document for the same
    // stroke, which is why every implementation of the reference algorithm has
    // this loop.
    let mut control1 = control1;
    let mut control2 = control2;
    for _ in 0..REPARAMETERISE_STEPS {
        params = reparameterise(slice, &params, control1, control2);
        match generate_bezier(slice, &params, tangent_start, tangent_end) {
            Some((next1, next2)) => {
                control1 = next1;
                control2 = next2;
            }
            None => break,
        }
    }
    let (max_error, split_at) = max_error(slice, &params, control1, control2);

    // A candidate that only "fits" by throwing its handles far outside the
    // stretch is the least-squares solve misbehaving, and its measured error
    // cannot be trusted (see `MAX_HANDLE_RATIO`). Splitting is the answer.
    let chord = distance(points[first], points[last]).max(tolerance);
    let reach = distance(points[first], control1).max(distance(points[last], control2));
    let ill_conditioned = reach > MAX_HANDLE_RATIO * chord;

    if max_error <= tolerance && !ill_conditioned {
        stats.max_error = stats.max_error.max(max_error);
        out.push(cubic_segment(control1, control2, points[last]));
        return;
    }

    // **Where to split.** Prefer the sample with the worst error — that is what
    // makes the subdivision adaptive, so a straight run costs one segment and a
    // corner costs several. When the worst error is at an *endpoint* there is no
    // interior worst sample to cut at, and the two bad answers are "give up and
    // keep a curve that is off by more than the caller asked for" and "recurse on
    // exactly the same input forever". Splitting the range in half is the third
    // answer, and it is the right one: an endpoint error means the *tangent* the
    // fit was given does not describe this stretch, and halving the stretch is
    // precisely how that gets resolved — the recursion then converges on the
    // sample polyline, which is exact by definition.
    let interior = split_at > 0 && split_at < count - 1;
    let split_at = if interior { split_at } else { count / 2 };

    // The depth cap: this stretch resists every curve the fit can offer (see
    // `MAX_DEPTH`). Emit the samples themselves, which is exact by construction,
    // rather than a curve that is off by more than the caller asked for.
    if depth >= MAX_DEPTH {
        for index in first..last {
            out.push(line_to(points, index, index + 1));
        }
        return;
    }

    let split = first + split_at;
    // The tangent at the split point comes from its neighbours, which keeps the
    // two halves *visibly continuous*: they meet with a shared direction rather
    // than a kink.
    let tangent_split = split_tangent(points, split, first, last);

    fit_recursive(
        points,
        first,
        split,
        tangent_start,
        tangent_split,
        tolerance,
        depth + 1,
        out,
        stats,
    );
    fit_recursive(
        points,
        split,
        last,
        tangent_split,
        tangent_end,
        tolerance,
        depth + 1,
        out,
        stats,
    );
}

/// The direction of travel at an interior split point, from its neighbours.
fn split_tangent(points: &[Point2], split: usize, first: usize, last: usize) -> Point2 {
    let before = split.saturating_sub(1).max(first);
    let after = (split + 1).min(last);
    unit(points[before], points[after])
}

fn cubic_segment(control1: Point2, control2: Point2, to: Point2) -> PathSegment {
    PathSegment::Cubic {
        control1: literal(control1),
        control2: literal(control2),
        to: literal(to),
    }
}

fn unit(from: Point2, to: Point2) -> Point2 {
    let dx = to.x - from.x;
    let dy = to.y - from.y;
    let length = (dx * dx + dy * dy).sqrt();
    if length <= f64::EPSILON {
        Point2::new(1.0, 0.0)
    } else {
        Point2::new(dx / length, dy / length)
    }
}

/// Chord-length parameterisation: `u` is the fraction of *distance* travelled.
///
/// The simplest of the three standard choices (uniform, chord-length, centripetal)
/// and the one Schneider specifies. It is monotone with a strictly positive step
/// whenever the samples are distinct — which is why [`crate::stroke::clean`] runs
/// first, and why the least-squares solve below divides by a well-conditioned
/// basis.
pub fn chord_length_params(points: &[Point2]) -> Vec<f64> {
    let mut params = Vec::with_capacity(points.len());
    params.push(0.0);
    let mut total = 0.0;
    for pair in points.windows(2) {
        let dx = pair[1].x - pair[0].x;
        let dy = pair[1].y - pair[0].y;
        total += (dx * dx + dy * dy).sqrt();
        params.push(total);
    }
    if total <= f64::EPSILON {
        // All points coincident: fall back to a uniform parameterisation, which
        // is only reachable defensively (the fit itself would rather emit a line).
        let n = points.len();
        return (0..n).map(|i| i as f64 / (n - 1).max(1) as f64).collect();
    }
    for param in params.iter_mut() {
        *param /= total;
    }
    params
}

/// The least-squares handles for a fixed parameterisation (Schneider's
/// `GenerateBezier`).
///
/// Solves the 2×2 normal equations of the Bernstein basis for the handle
/// *lengths* along the given tangents, which is what makes the fit smooth at the
/// endpoints by construction — the curve leaves along `t1` and arrives along
/// `t2` exactly, whatever the interior samples say. `t2` therefore points *back*
/// into the curve (see `fit_stroke`), and both control points are
/// `endpoint + tangent · length`.
fn generate_bezier(
    points: &[Point2],
    params: &[f64],
    tangent_start: Point2,
    tangent_end: Point2,
) -> Option<(Point2, Point2)> {
    let first = points[0];
    let last = points[points.len() - 1];

    // Local `(a1, a2)` per sample: the component of `points[i] - (B1*t1 + B2*t2)`
    // along each tangent.
    let mut c00 = 0.0;
    let mut c01 = 0.0;
    let mut c11 = 0.0;
    let mut x0 = 0.0;
    let mut x1 = 0.0;
    for (point, u) in points.iter().zip(params) {
        let b0 = bernstein(0, *u);
        let b1 = bernstein(1, *u);
        let b2 = bernstein(2, *u);
        let b3 = bernstein(3, *u);
        let a1 = Point2::new(tangent_start.x * b1, tangent_start.y * b1);
        let a2 = Point2::new(tangent_end.x * b2, tangent_end.y * b2);
        c00 += a1.x * a1.x + a1.y * a1.y;
        c01 += a1.x * a2.x + a1.y * a2.y;
        c11 += a2.x * a2.x + a2.y * a2.y;
        let tmp = Point2::new(
            point.x - (first.x * (b0 + b1) + last.x * (b2 + b3)),
            point.y - (first.y * (b0 + b1) + last.y * (b2 + b3)),
        );
        x0 += a1.x * tmp.x + a1.y * tmp.y;
        x1 += a2.x * tmp.x + a2.y * tmp.y;
    }

    let det = c00 * c11 - c01 * c01;
    let (alpha1, alpha2) = if det.abs() > f64::EPSILON {
        ((x0 * c11 - c01 * x1) / det, (c00 * x1 - x0 * c01) / det)
    } else {
        // Singular: the two tangents are parallel (a straight run). Fall back to
        // the chord-length split, which is the textbook answer for that case.
        let chord = ((last.x - first.x).powi(2) + (last.y - first.y).powi(2)).sqrt();
        let third = chord / 3.0;
        (third, third)
    };

    if !alpha1.is_finite() || !alpha2.is_finite() {
        return None;
    }
    // A negative handle length would send the curve *backwards* along its own
    // tangent — a cusp nobody drew. Clamping to zero (making that side a corner)
    // is what the reference implementation does too.
    let a1 = alpha1.max(0.0);
    let a2 = alpha2.max(0.0);
    Some((
        Point2::new(
            first.x + tangent_start.x * a1,
            first.y + tangent_start.y * a1,
        ),
        Point2::new(last.x + tangent_end.x * a2, last.y + tangent_end.y * a2),
    ))
}

/// A cubic Bernstein basis value.
fn bernstein(index: usize, u: f64) -> f64 {
    let v = 1.0 - u;
    match index {
        0 => v * v * v,
        1 => 3.0 * u * v * v,
        2 => 3.0 * u * u * v,
        3 => u * u * u,
        _ => 0.0,
    }
}

/// One round of Newton–Raphson reparameterisation (Schneider's
/// `reparameterize`).
///
/// Each sample's parameter moves toward the parameter of the *nearest point* on
/// the current curve:
///
/// ```text
///   u ← u − ((Q(u) − P) · Q'(u)) / |Q'(u)|²
/// ```
///
/// Parameters are made monotone afterwards. That matters more than it looks: a
/// parameter that crosses its neighbour would fold the parameterisation and the
/// next least-squares solve would chase a curve that doubles back on itself —
/// the classic way a fitting implementation produces a loop the user never drew.
pub fn reparameterise(
    points: &[Point2],
    params: &[f64],
    control1: Point2,
    control2: Point2,
) -> Vec<f64> {
    let first = points[0];
    let last = points[points.len() - 1];
    let mut out: Vec<f64> = points
        .iter()
        .zip(params)
        .map(|(point, u)| {
            let q = cubic_at(first, control1, control2, last, *u);
            let q_prime = crate::bezier::cubic_tangent_at(first, control1, control2, last, *u);
            let numerator = (q.x - point.x) * q_prime.x + (q.y - point.y) * q_prime.y;
            let denominator = q_prime.x * q_prime.x + q_prime.y * q_prime.y;
            if denominator <= f64::EPSILON {
                *u
            } else {
                u - numerator / denominator
            }
        })
        .collect();
    // ── pin the ends, and keep the interior strictly inside (0, 1) ─────────
    //
    // A Newton step is an *extrapolation* whenever the sample is outside the
    // curve's neighbourhood: nothing in the formula stops `u` from leaving
    // `[0, 1]`, and a parameter of 2.34 does not mean "past the end of this
    // range" — it means the curve is evaluated far beyond its own endpoint and
    // the fit is scored against a point that has nothing to do with the segment.
    // That is exactly the bug the fidelity law found: a slice reporting a maximum
    // error of 5 × 10⁻¹¹ while the real curve missed its middle sample by 259
    // units. The clamp is the fix, and the margin guarantees the sequence stays
    // strictly increasing even when several samples are crammed together.
    const MARGIN: f64 = 1e-9;
    out[0] = 0.0;
    let last_index = out.len() - 1;
    out[last_index] = 1.0;
    for index in 1..last_index {
        let lo = out[index - 1] + MARGIN;
        let hi = 1.0 - (last_index - index) as f64 * MARGIN;
        out[index] = if hi <= lo {
            // No room left: put this sample at the midpoint of what remains,
            // which stays monotone without breaking the earlier ones.
            (out[index - 1] + 1.0) / 2.0
        } else if out[index].is_finite() {
            out[index].clamp(lo, hi)
        } else {
            (lo + hi) / 2.0
        };
    }
    out
}

/// The **true** worst deviation of a candidate fit, and where it happens.
///
/// The reference algorithm scores a fit at the *parameters* — `|Q(uᵢ) − Pᵢ|` —
/// and that number is only a proxy for what the user sees. It is a good proxy
/// when the parameters are good and a lie when they are not, which the fidelity
/// law demonstrated: a slice whose parametric error was 5 × 10⁻¹¹ while its
/// curve missed the middle sample by 259 units, because that sample's parameter
/// had been pushed to where the curve *happened* to come back near it.
///
/// So the criterion here is the distance from each sample to the **curve**,
/// approximated by a fine polyline of the candidate — and, because a polyline is
/// still an approximation, the returned value adds
/// [`chord_sagitta_bound`] for the polyline it measured with. It costs one pass
/// over `RESOLUTION` points per slice and buys the property the tool actually
/// promises: after this fit, no sample is further from the result than the
/// tolerance the caller asked for — a statement about the *curve*, not about a
/// polyline drawn near it.
pub fn max_error(
    points: &[Point2],
    _params: &[f64],
    control1: Point2,
    control2: Point2,
) -> (f64, usize) {
    /// Segments in the candidate's polyline. Finer than the renderer needs:
    /// this number decides *acceptance*, so it errs on the side of knowing the
    /// curve rather than guessing at it.
    const RESOLUTION: usize = 64;

    let first = points[0];
    let last = points[points.len() - 1];
    // The resolution follows the candidate's **reach**: a coarse polyline of a
    // curve that swings far outside its own samples can pass *closer* to a sample
    // than the curve does, so it would under-measure the quantity being decided.
    // A docile curve still costs 64 chords; a long-handled one pays for honesty.
    let span = distance(first, last).max(1e-9);
    let reach = distance(first, control1).max(distance(last, control2));
    let resolution = (RESOLUTION as f64 * (reach / span).clamp(1.0, 8.0)) as usize;
    let polyline: Vec<Point2> = (0..=resolution)
        .map(|step| {
            let u = step as f64 / resolution as f64;
            cubic_at(first, control1, control2, last, u)
        })
        .collect();
    let mut worst = 0.0;
    let mut index = 0;
    for (i, point) in points.iter().enumerate() {
        let mut best = f64::INFINITY;
        for pair in polyline.windows(2) {
            best = best.min(crate::bezier::distance_to_segment(*point, pair[0], pair[1]));
        }
        if best > worst {
            worst = best;
            index = i;
        }
    }
    // **Conservative, not approximate.** The measured value is a distance to
    // *chords*, so it under-reports the distance to the curve by up to the
    // polyline's own sagitta. Adding that bound is what makes the caller's
    // `<= tolerance` a statement about the curve rather than about the polyline:
    // whatever this returns, the true deviation is no larger.
    let bound = chord_sagitta_bound(first, control1, control2, last, resolution);
    (worst + bound, index)
}

/// **What a chord polyline cannot see.** The largest amount by which a
/// `resolution`-chord polyline of the cubic `(start, control1, control2, end)` can
/// lie *inside* the curve it approximates.
///
/// A cubic's second derivative is bounded by
/// `6·max(|P0 − 2P1 + P2|, |P1 − 2P2 + P3|)`, and a curve deviates from a chord
/// spanning parameter step `h` by at most `max|B″|·h²/8`. So a distance measured
/// against chords under-reports the true distance to the curve by up to this much,
/// and every honest comparison against a tolerance has to add it back. (Two
/// measurements of the same fit disagreed by exactly this term at a 5.16-unit
/// tolerance: the acceptance test's coarser polyline passed a curve that the
/// finer one put 0.0033 outside — the gap is not a fitting error, it is what the
/// acceptance test could not see.)
pub fn chord_sagitta_bound(
    start: Point2,
    control1: Point2,
    control2: Point2,
    end: Point2,
    resolution: usize,
) -> f64 {
    let n = resolution.max(1) as f64;
    let d1 = distance(
        start,
        Point2::new(2.0 * control1.x - control2.x, 2.0 * control1.y - control2.y),
    );
    let d2 = distance(
        end,
        Point2::new(2.0 * control2.x - control1.x, 2.0 * control2.y - control1.y),
    );
    6.0 * d1.max(d2) / (8.0 * n * n)
}

/// Does a straight line describe these samples within `tolerance`?
///
/// The brush's fast path: a quick straight flick is committed as a single `Line`
/// segment rather than a pair of cubics that happen to be nearly straight, which
/// is both a smaller document and a nicer thing to edit afterwards with the
/// direct-selection tool.
pub fn is_straight(points: &[Point2], tolerance: f64) -> bool {
    let Some(first) = points.first().copied() else {
        return true;
    };
    let Some(last) = points.last().copied() else {
        return true;
    };
    if points.len() < 3 {
        return true;
    }
    points
        .iter()
        .all(|point| crate::bezier::distance_to_segment(*point, first, last) <= tolerance)
}

/// The maximum deviation of a fitted cubic from its chord, for callers that want
/// to know how "curved" a segment came out (the tests use it to prove a fitted
/// straight run really is flat).
pub fn segment_bow(segment: &PathSegment, tolerance_samples: usize) -> f64 {
    match segment {
        PathSegment::Line { .. } | PathSegment::Close => 0.0,
        PathSegment::Quadratic { .. } => 0.0,
        PathSegment::Cubic {
            control1,
            control2,
            to,
        } => {
            let (Some(c1), Some(c2), Some(end)) = (
                literal_point(control1),
                literal_point(control2),
                literal_point(to),
            ) else {
                return 0.0;
            };
            cubic_deviation_from_chord(Point2::ZERO, c1, c2, end, tolerance_samples)
        }
    }
}

/// A literal point, if that is what the parameter holds.
fn literal_point(param: &vectra_core::Parameter<Point2>) -> Option<Point2> {
    match param {
        vectra_core::Parameter::Literal(point) => Some(*point),
        _ => None,
    }
}

/// Convenience: fit stamped points (the brush's entry point).
pub fn fit_stamped(stamped: &[StampedPoint], tolerance: f64) -> FitResult {
    let points: Vec<Point2> = stamped.iter().map(|sample| sample.point).collect();
    fit_stroke(&points, tolerance)
}

#[cfg(test)]
mod tests {
    use super::*;
    use vectra_core::Parameter;

    fn p(x: f64, y: f64) -> Point2 {
        Point2::new(x, y)
    }

    #[test]
    fn a_straight_run_of_samples_fits_as_a_single_line() {
        let points: Vec<Point2> = (0..20).map(|i| p(i as f64 * 5.0, 0.0)).collect();
        assert!(is_straight(&points, 0.5));
        let fit = fit_stroke(&points, 0.5);
        assert_eq!(fit.segments.len(), 1, "{:?}", fit.segments);
        assert!(matches!(fit.segments[0], PathSegment::Line { .. }));
        assert_eq!(fit.stats.max_error, 0.0);
    }

    #[test]
    fn a_quarter_circle_fits_as_few_cubics_near_the_samples() {
        let points: Vec<Point2> = (0..33)
            .map(|i| {
                let angle = i as f64 / 32.0 * std::f64::consts::FRAC_PI_2;
                p(100.0 * angle.cos(), 100.0 * angle.sin())
            })
            .collect();
        let fit = fit_stroke(&points, 0.5);
        assert!(
            fit.segments.len() <= 4,
            "a quarter circle should need few segments, took {}",
            fit.segments.len()
        );
        assert!(fit.stats.cubics >= 1, "{:?}", fit.stats);
        assert!(
            fit.stats.max_error <= 0.5,
            "error {} exceeded the tolerance",
            fit.stats.max_error
        );
        assert!(fit.stats.max_depth <= MAX_DEPTH);
    }

    #[test]
    fn a_tighter_tolerance_costs_more_segments() {
        let points: Vec<Point2> = (0..65)
            .map(|i| {
                let angle = i as f64 / 64.0 * std::f64::consts::TAU;
                p(50.0 * angle.cos(), 50.0 * angle.sin())
            })
            .collect();
        let coarse = fit_stroke(&points, 4.0);
        let fine = fit_stroke(&points, 0.05);
        assert!(
            fine.segments.len() >= coarse.segments.len(),
            "fine {} < coarse {}",
            fine.segments.len(),
            coarse.segments.len()
        );
        assert!(fine.stats.max_error <= 0.05);
    }

    #[test]
    fn two_points_are_a_line_and_one_point_is_nothing() {
        let fit = fit_stroke(&[p(0.0, 0.0), p(10.0, 0.0)], 1.0);
        assert_eq!(fit.segments.len(), 1);
        assert!(matches!(fit.segments[0], PathSegment::Line { .. }));

        let fit = fit_stroke(&[p(0.0, 0.0)], 1.0);
        assert!(fit.segments.is_empty());
        assert_eq!(fit.start, p(0.0, 0.0));

        let fit = fit_stroke(&[], 1.0);
        assert!(fit.segments.is_empty());
    }

    /// **The shape of the answer.** The fit never leaves the samples'
    /// neighbourhood: it is within tolerance of every sample, and no control
    /// point sits further from its anchor than [`MAX_HANDLE_RATIO`] times the
    /// stretch's chord. The second half is a *quality* bound rather than a
    /// correctness one — a fit with 400 000-unit handles can be within tolerance
    /// and still be a monstrous thing to edit, export or constrain — and it is
    /// cheap insurance on a document that outlives the gesture that made it.
    #[test]
    fn a_fit_keeps_its_handles_near_its_samples() {
        let points: Vec<Point2> = vec![
            (0.0, 0.0),
            (0.0, -335.49006855920993),
            (-60.133556733412334, 484.53581718320123),
            (229.66696058514577, 440.468020156001),
            (-376.88405972215156, 197.14435733122906),
            (-352.6421297833243, -71.5971535212152),
            (-344.72273350183633, 334.74905887109946),
            (125.16026125109154, -446.10510216050005),
            (0.0, 0.0),
        ]
        .into_iter()
        .map(|(x, y)| Point2::new(x, y))
        .collect();
        let tolerance = 12.390321075881316;
        let fit = fit_stroke(&points, tolerance);

        let mut cursor = fit.start;
        for segment in &fit.segments {
            match segment {
                PathSegment::Line { to } => {
                    let Parameter::Literal(to) = to else { continue };
                    cursor = *to;
                }
                PathSegment::Cubic {
                    control1,
                    control2,
                    to,
                } => {
                    let (Parameter::Literal(c1), Parameter::Literal(c2), Parameter::Literal(to)) =
                        (control1, control2, to)
                    else {
                        continue;
                    };
                    let chord = distance(cursor, *to).max(tolerance);
                    assert!(
                        distance(cursor, *c1) <= MAX_HANDLE_RATIO * chord,
                        "handle 1 escapes the stretch: {:?}",
                        fit.stats
                    );
                    assert!(
                        distance(*to, *c2) <= MAX_HANDLE_RATIO * chord,
                        "handle 2 escapes the stretch: {:?}",
                        fit.stats
                    );
                    cursor = *to;
                }
                _ => {}
            }
        }

        // …and the fit is still honest: every sample within the tolerance,
        // measured against the fitted *curves* (200 chords per slice, ~0.02 units
        // of sagitta at most for a curve this tame).
        let mut flat: Vec<Point2> = vec![fit.start];
        let mut cursor = fit.start;
        for segment in &fit.segments {
            match segment {
                PathSegment::Line {
                    to: Parameter::Literal(to),
                } => {
                    flat.push(*to);
                    cursor = *to;
                }
                PathSegment::Cubic {
                    control1,
                    control2,
                    to,
                } => {
                    if let (
                        Parameter::Literal(c1),
                        Parameter::Literal(c2),
                        Parameter::Literal(to),
                    ) = (control1, control2, to)
                    {
                        for step in 1..=200 {
                            flat.push(cubic_at(cursor, *c1, *c2, *to, step as f64 / 200.0));
                        }
                        cursor = *to;
                    }
                }
                _ => {}
            }
        }
        for sample in &points {
            let nearest = flat
                .windows(2)
                .map(|pair| crate::bezier::distance_to_segment(*sample, pair[0], pair[1]))
                .fold(f64::INFINITY, f64::min);
            assert!(
                nearest <= tolerance,
                "sample {sample:?} is {nearest} from the fit (tolerance {tolerance})"
            );
        }
    }

    #[test]
    fn coincident_points_do_not_panic_or_loop() {
        let points = vec![p(5.0, 5.0); 200];
        let fit = fit_stroke(&points, 0.5);
        assert!(fit.stats.max_depth <= MAX_DEPTH);
        assert!(fit.start == p(5.0, 5.0));
    }

    #[test]
    fn reparameterisation_keeps_parameters_inside_the_unit_interval() {
        // A sample far off the curve: Newton's step wants to run away, and the
        // only correct answer is to stay inside the range (see the fidelity law
        // for the failure this prevents).
        let points = vec![p(0.0, 0.0), p(50.0, 900.0), p(100.0, 0.0)];
        let params = chord_length_params(&points);
        let moved = reparameterise(&points, &params, p(10.0, 0.0), p(90.0, 0.0));
        assert_eq!(moved[0], 0.0);
        assert_eq!(moved[moved.len() - 1], 1.0);
        for pair in moved.windows(2) {
            assert!(pair[1] > pair[0], "not monotone: {moved:?}");
        }
        assert!(moved.iter().all(|u| (0.0..=1.0).contains(u)), "{moved:?}");
    }

    #[test]
    fn the_parameterisation_is_monotone() {
        let points = vec![p(0.0, 0.0), p(1.0, 0.0), p(1.0, 5.0), p(9.0, 5.0)];
        let params = chord_length_params(&points);
        assert_eq!(params[0], 0.0);
        assert!((params[params.len() - 1] - 1.0).abs() < 1e-12);
        for pair in params.windows(2) {
            assert!(pair[1] > pair[0], "{params:?}");
        }
    }
}
