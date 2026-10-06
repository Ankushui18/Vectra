//! Stroke sampling: a pointer trail becomes a polyline with width (Task 10.1
//! RULE 3, "as the user draws freehand").
//!
//! ## Pressure, or its absence
//!
//! A pen tablet reports `pressure ∈ [0, 1]`; a mouse reports `0.5` forever (or
//! nothing). RULE 3 asks for "pressure if available, otherwise velocity", which
//! is the standard fallback and the reason a mouse-drawn stroke still tapers:
//!
//! ```text
//!   fast pointer  ──▶  thin stroke        slow pointer  ──▶  thick stroke
//! ```
//!
//! The mapping is deliberately blunt about its inputs — a stroke records the
//! *evidence* (position, time, pressure) and derives width from it, so the
//! derivation can be re-run when the feel is tuned without re-sampling anything.
//!
//! ## Why nothing here is smoothed yet
//!
//! Fitting ([`fit`]) happens at *stroke end*, not per event. Fitting a growing
//! stroke on every pointer move would make the shape of the beginning of the
//! stroke change as the user continues — the curve would "breathe". One fit, at
//! the end, against the whole stroke, is both cheaper and stable, and the live
//! preview during the gesture is the raw polyline (which is what every
//! professional tool shows anyway).

use vectra_core::Point2;

/// One sample of a drawing gesture.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sample {
    pub point: Point2,
    /// Seconds since the stroke began. Only *differences* matter, so any
    /// monotonic clock works — and the UI can feed event timestamps straight in.
    pub time: f64,
    /// `[0, 1]` if the device reports it, `None` if it does not.
    pub pressure: Option<f64>,
}

impl Sample {
    pub fn new(point: Point2, time: f64, pressure: Option<f64>) -> Self {
        Self {
            point,
            time,
            pressure,
        }
    }
}

/// How a stroke's width varies along it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BrushProfile {
    /// Width when the pointer is slow and pressing hard.
    pub max_width: f64,
    /// Width when the pointer is fast (or the stroke is short).
    pub min_width: f64,
    /// Speed (document units per second) at which a velocity-driven stroke is
    /// considered "fast" — at and above it the stroke is `min_width`.
    ///
    /// Velocity rather than distance so that the same gesture at two zooms feels
    /// the same, and so that a slow careful curve stays thick in a way a fast
    /// flick does not.
    pub fast_speed: f64,
    /// True when the device gave pressure and it should win over velocity.
    pub use_pressure: bool,
}

impl Default for BrushProfile {
    fn default() -> Self {
        Self {
            max_width: 12.0,
            min_width: 2.0,
            fast_speed: 1200.0,
            use_pressure: true,
        }
    }
}

/// One point of the sampled stroke, with its width already decided.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StampedPoint {
    pub point: Point2,
    pub width: f64,
    /// Speed in document units per second at this sample (`0.0` for the first).
    pub speed: f64,
    /// The evidence this width came from: the reported pressure, if any.
    pub pressure: Option<f64>,
}

/// Turn samples into a *width-carrying* polyline.
///
/// Widths are smoothed with a short moving average, because raw per-event
/// pressure is noisy at the sampling rates browsers deliver — the taper should
/// look like a drawn line, not like a seismograph.
pub fn stamp(samples: &[Sample], profile: &BrushProfile) -> Vec<StampedPoint> {
    if samples.is_empty() {
        return Vec::new();
    }
    let has_pressure = profile.use_pressure
        && samples
            .iter()
            .any(|sample| sample.pressure.map(|p| p > 0.0).unwrap_or(false));

    let speeds = speeds(samples);
    let widths: Vec<f64> = samples
        .iter()
        .zip(&speeds)
        .map(|(sample, speed)| match (has_pressure, sample.pressure) {
            // Pressure wins when the device has it: it *is* the user's intent,
            // and no inference from timing can beat a direct measurement.
            (true, Some(pressure)) => {
                let p = pressure.clamp(0.0, 1.0);
                profile.min_width + (profile.max_width - profile.min_width) * p
            }
            // Otherwise: velocity, which is RULE 3's explicit fallback.
            _ => {
                let speed = *speed;
                let t = (speed / profile.fast_speed).clamp(0.0, 1.0);
                profile.max_width - (profile.max_width - profile.min_width) * t
            }
        })
        .collect();
    let widths = smooth_widths(&widths, 3);

    samples
        .iter()
        .zip(speeds)
        .zip(widths)
        .map(|((sample, speed), width)| StampedPoint {
            point: sample.point,
            width,
            speed,
            pressure: sample.pressure,
        })
        .collect()
}

/// A short centred moving average, with the ends held.
fn smooth_widths(widths: &[f64], radius: usize) -> Vec<f64> {
    if widths.len() < 3 {
        return widths.to_vec();
    }
    (0..widths.len())
        .map(|index| {
            let from = index.saturating_sub(radius);
            let to = (index + radius + 1).min(widths.len());
            let window = &widths[from..to];
            window.iter().sum::<f64>() / window.len() as f64
        })
        .collect()
}

/// Per-sample speed (document units / second), first sample `0.0`.
fn speeds(samples: &[Sample]) -> Vec<f64> {
    let mut out = Vec::with_capacity(samples.len());
    out.push(0.0);
    for pair in samples.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        let dt = (b.time - a.time).max(f64::MIN_POSITIVE);
        let dx = b.point.x - a.point.x;
        let dy = b.point.y - a.point.y;
        let distance = (dx * dx + dy * dy).sqrt();
        out.push(distance / dt);
    }
    // The first sample inherits the second's speed: a stroke should not start
    // with a zero-speed spike of maximum width just because it had no history.
    if out.len() > 1 {
        out[0] = out[1];
    }
    out
}

/// Drop samples that carry no information.
///
/// Two filters, both necessary in practice:
///
/// * **coincident points.** A pointer that has not moved still fires events on
///   some platforms; a run of identical points would make the fit divide by zero
///   and the outline degenerate.
/// * **outlier spikes.** A single event far from the stroke (a dropped frame, a
///   tablet glitch) would otherwise drag the fitted curve across the canvas.
///   A spike is *skipped*, not clamped — clamping would forge a point the user
///   never drew.
pub fn clean(samples: &[Sample], min_distance: f64, spike_factor: f64) -> Vec<Sample> {
    let mut out: Vec<Sample> = Vec::with_capacity(samples.len());
    for sample in samples {
        match out.last() {
            None => out.push(*sample),
            Some(previous) => {
                let dx = sample.point.x - previous.point.x;
                let dy = sample.point.y - previous.point.y;
                if (dx * dx + dy * dy).sqrt() < min_distance {
                    continue;
                }
                // A spike is judged against the *running* step size, so a stroke
                // that legitimately moves fast is not mistaken for one.
                if out.len() >= 2 {
                    let before = out[out.len() - 2];
                    let bx = previous.point.x - before.point.x;
                    let by = previous.point.y - before.point.y;
                    let typical = (bx * bx + by * by).sqrt();
                    let step = (dx * dx + dy * dy).sqrt();
                    if typical > 0.0 && step > typical * spike_factor {
                        continue;
                    }
                }
                out.push(*sample);
            }
        }
    }
    out
}

/// The total length of a polyline.
pub fn length(points: &[Point2]) -> f64 {
    points
        .windows(2)
        .map(|pair| {
            let dx = pair[1].x - pair[0].x;
            let dy = pair[1].y - pair[0].y;
            (dx * dx + dy * dy).sqrt()
        })
        .sum()
}

/// Is the stroke a **candidate for Quick Shape snapping?** (Task 10.1 RULE 3.)
///
/// The signal every touch tool uses, and the one the UI watches for while the
/// pointer is *held still*:
///
/// * the stroke ends near where it began (within `close_fraction` of its own
///   length — the user drew a loop, not a squiggle);
/// * it is long enough to enclose anything (`min_length`);
/// * it is *roughly* convex (`convexity` is the fraction of cross products that
///   agree in sign) — a circle or rectangle is convex; a figure-eight is not.
///
/// The tests are cheap and answered from the cleaned polyline, which is why the
/// UI can ask on every hold tick without measuring anything expensive.
pub fn is_snap_candidate(
    points: &[Point2],
    min_length: f64,
    close_fraction: f64,
    convexity: f64,
) -> bool {
    if points.len() < 4 {
        return false;
    }
    let total = length(points);
    if total < min_length {
        return false;
    }
    let first = points[0];
    let last = points[points.len() - 1];
    let closing = ((last.x - first.x).powi(2) + (last.y - first.y).powi(2)).sqrt();
    if closing > total * close_fraction {
        return false;
    }
    convexity_of(points) >= convexity
}

/// Fraction of non-degenerate turns that share the majority direction.
///
/// `1.0` is perfectly convex; a stroke that doubles back drops sharply, which is
/// what separates a circle from a scribble.
pub fn convexity_of(points: &[Point2]) -> f64 {
    if points.len() < 3 {
        return 1.0;
    }
    let mut positive = 0usize;
    let mut negative = 0usize;
    for window in points.windows(3) {
        let (a, b, c) = (window[0], window[1], window[2]);
        let cross = (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x);
        if cross > 0.0 {
            positive += 1;
        } else if cross < 0.0 {
            negative += 1;
        }
    }
    let total = positive + negative;
    if total == 0 {
        return 1.0;
    }
    positive.max(negative) as f64 / total as f64
}

/// The centroid of a point set.
pub fn centroid(points: &[Point2]) -> Point2 {
    if points.is_empty() {
        return Point2::ZERO;
    }
    let (sx, sy) = points
        .iter()
        .fold((0.0, 0.0), |(x, y), p| (x + p.x, y + p.y));
    Point2::new(sx / points.len() as f64, sy / points.len() as f64)
}

/// Every vertex of a polyline, **closing the loop** by repeating the first point
/// — the form the region tools want.
pub fn closed_ring(points: &[Point2]) -> Vec<Point2> {
    let mut ring = points.to_vec();
    if let (Some(first), Some(last)) = (ring.first().copied(), ring.last().copied()) {
        if crate::bezier::distance(first, last) > f64::EPSILON {
            ring.push(first);
        }
    }
    ring
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(x: f64, y: f64) -> Point2 {
        Point2::new(x, y)
    }

    #[test]
    fn a_velocity_stroke_is_thick_when_slow_and_thin_when_fast() {
        let profile = BrushProfile::default();
        let slow = stamp(
            &[
                Sample::new(p(0.0, 0.0), 0.0, None),
                Sample::new(p(1.0, 0.0), 0.1, None),
                Sample::new(p(2.0, 0.0), 0.2, None),
                Sample::new(p(3.0, 0.0), 0.3, None),
            ],
            &profile,
        );
        let fast = stamp(
            &[
                Sample::new(p(0.0, 0.0), 0.0, None),
                Sample::new(p(500.0, 0.0), 0.1, None),
                Sample::new(p(1000.0, 0.0), 0.2, None),
                Sample::new(p(1500.0, 0.0), 0.3, None),
            ],
            &profile,
        );
        assert!(
            slow.iter().all(|s| s.width > fast[2].width),
            "slow {slow:?} vs fast {fast:?}"
        );
        assert!(fast.iter().all(|s| s.width >= profile.min_width - 1e-9));
        assert!(slow.iter().all(|s| s.width <= profile.max_width + 1e-9));
    }

    #[test]
    fn pressure_wins_when_the_device_reports_it() {
        let profile = BrushProfile::default();
        let stamped = stamp(
            &[
                Sample::new(p(0.0, 0.0), 0.0, Some(0.05)),
                Sample::new(p(1000.0, 0.0), 0.1, Some(0.05)),
                Sample::new(p(2000.0, 0.0), 0.2, Some(0.05)),
            ],
            &profile,
        );
        // Very fast, but pressed lightly: the *pressure* decides, so the stroke
        // stays near the light end rather than being thinned to the minimum by
        // speed.
        assert!(
            stamped[1].width < profile.max_width * 0.35,
            "width was {}",
            stamped[1].width
        );
    }

    #[test]
    fn coincident_and_spiking_samples_are_dropped() {
        let samples = vec![
            Sample::new(p(0.0, 0.0), 0.0, None),
            Sample::new(p(0.0, 0.0), 0.01, None), // coincident
            Sample::new(p(1.0, 0.0), 0.02, None),
            Sample::new(p(900.0, 900.0), 0.03, None), // spike
            Sample::new(p(2.0, 0.0), 0.04, None),
        ];
        let cleaned = clean(&samples, 0.05, 8.0);
        assert_eq!(cleaned.len(), 3, "{cleaned:?}");
        assert_eq!(cleaned[0].point, p(0.0, 0.0));
        assert_eq!(cleaned[2].point, p(2.0, 0.0));
    }

    #[test]
    fn a_closed_loop_is_a_candidate_and_a_line_is_not() {
        // A rough circle, 16 points.
        let circle: Vec<Point2> = (0..16)
            .map(|i| {
                let angle = i as f64 / 16.0 * std::f64::consts::TAU;
                p(100.0 + 50.0 * angle.cos(), 100.0 + 50.0 * angle.sin())
            })
            .collect();
        let mut loop_ = circle.clone();
        loop_.push(circle[0]);
        assert!(
            is_snap_candidate(&loop_, 20.0, 0.25, 0.85),
            "a closed circle should be a candidate (convexity {})",
            convexity_of(&loop_)
        );

        let line: Vec<Point2> = (0..8).map(|i| p(i as f64 * 10.0, 0.0)).collect();
        assert!(
            !is_snap_candidate(&line, 20.0, 0.25, 0.85),
            "a line is not a shape"
        );

        // A figure-eight doubles back, so its turns disagree and the convexity
        // gate (which the UI sets at 0.9) refuses it.
        let eight = vec![
            p(0.0, 0.0),
            p(10.0, 10.0),
            p(0.0, 20.0),
            p(-10.0, 10.0),
            p(0.0, 0.0),
            p(-10.0, -10.0),
            p(0.0, -20.0),
            p(10.0, -10.0),
            p(0.0, 0.0),
        ];
        assert!(convexity_of(&eight) < 0.9, "{}", convexity_of(&eight));
    }

    #[test]
    fn a_perfect_circle_is_maximally_convex() {
        let circle: Vec<Point2> = (0..32)
            .map(|i| {
                let angle = i as f64 / 32.0 * std::f64::consts::TAU;
                p(angle.cos(), angle.sin())
            })
            .collect();
        assert!((convexity_of(&circle) - 1.0).abs() < 1e-12);
    }
}
