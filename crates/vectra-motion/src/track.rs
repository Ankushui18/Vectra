//! Keyframe sampling: the piece of motion that is a table lookup with a
//! well-defined answer for every input time.
//!
//! A track is deliberately *total*: it answers for `t` before the first key,
//! between keys and after the last one, and never returns an error. That is not
//! a nicety — it is what lets the evaluator stay pure and branch-free at
//! playback extremes, where a scrubber routinely lands before `t = 0` of a clip
//! or long after it has ended. The alternative (clamping the *clock*) would make
//! the scene depend on where the UI thinks the timeline starts.

use vectra_core::Keyframe;

/// One sample: the value and which segment it came from, for diagnostics.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TrackSample {
    pub value: f64,
    /// Index of the segment the value came from. `0` for "before the first key",
    /// `keys.len() - 1` for "after the last".
    pub segment: usize,
    /// How far along that segment `t` sits, in `0..=1`. Outside the track's span
    /// this is pinned to `0.0` (before) or `1.0` (after), which is the honest
    /// answer: the value is the endpoint's, not extrapolated.
    pub t: f64,
}

/// Sample a channel at `time`.
///
/// # Which interpolation, and why not a spline
///
/// Linear ("ramp") is Task 6.0's declared interpolation — MES §12's sketch names
/// it, and the design doc records the choice. The important property is not
/// smoothness but **locality and purity**: a linear segment is determined by the
/// two keys that bracket it, so editing one key moves exactly one segment (the
/// dirty propagation stays honest), and the sampled value is exactly reproducible
/// with no solver, no tangent cache and no dependence on how the track was built.
/// A Catmull-Rom or Bézier track needs tangents derived from *neighbouring* keys,
/// which means a key edit silently changes segments it does not touch — a
/// materially different dependency graph, deferred to Phase 2 with the
/// track-editing UI and its own graph vertices.
///
/// The list is guaranteed sorted and finite by
/// [`vectra_core::MotionTrack::validate`], which runs at the boundary, so this
/// function is a plain binary search with no defensive branches in the hot path.
pub fn sample_channel(keys: &[Keyframe], time: f64) -> f64 {
    sample_with_segment(keys, time).value
}

/// [`sample_channel`], with the segment index and local parameter, for the
/// inspector and for the laws (which need to see *where* a value came from).
pub fn sample_with_segment(keys: &[Keyframe], time: f64) -> TrackSample {
    if keys.is_empty() {
        // Unreachable through the document (a track with an empty channel is
        // rejected by `validate`), but a sampler that panics is worse than one
        // that answers zero — the evaluator contract is totality.
        return TrackSample {
            value: 0.0,
            segment: 0,
            t: 0.0,
        };
    }
    let first = keys[0];
    if time <= first.time {
        return TrackSample {
            value: first.value,
            segment: 0,
            t: 0.0,
        };
    }
    let last = keys[keys.len() - 1];
    if time >= last.time {
        return TrackSample {
            value: last.value,
            segment: keys.len() - 1,
            t: 1.0,
        };
    }
    // `partition_point` gives the count of keys with time <= `time`; the
    // bracketing pair is (index-1, index). Both are in range because the first
    // key is strictly before `time` and the last strictly after.
    let index = keys.partition_point(|k| k.time <= time);
    let (left, right) = (keys[index - 1], keys[index]);
    let span = right.time - left.time;
    if span <= 0.0 {
        // Strictly increasing times are validated in, so this is unreachable —
        // and if it ever were reached, holding the left key beats dividing by
        // zero and painting NaN.
        return TrackSample {
            value: left.value,
            segment: index - 1,
            t: 1.0,
        };
    }
    let local = (time - left.time) / span;
    TrackSample {
        value: left.value + (right.value - left.value) * local,
        segment: index - 1,
        t: local,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(pairs: &[(f64, f64)]) -> Vec<Keyframe> {
        pairs.iter().map(|(t, v)| Keyframe::new(*t, *v)).collect()
    }

    #[test]
    fn the_sampler_is_total_including_outside_the_span() {
        let channel = keys(&[(1.0, 10.0), (2.0, 30.0)]);
        assert_eq!(sample_channel(&channel, -1e9), 10.0, "before the first key");
        assert_eq!(sample_channel(&channel, 1.0), 10.0, "on the first key");
        assert_eq!(sample_channel(&channel, 1.5), 20.0, "mid segment");
        assert_eq!(sample_channel(&channel, 2.0), 30.0, "on the last key");
        assert_eq!(sample_channel(&channel, 1e9), 30.0, "after the last key");
    }

    #[test]
    fn a_single_keyframe_track_is_a_constant() {
        let channel = keys(&[(4.0, 7.0)]);
        for t in [-5.0, 4.0, 5.0] {
            assert_eq!(sample_channel(&channel, t), 7.0, "t={t}");
        }
    }

    #[test]
    fn sampling_picks_the_bracketing_segment() {
        let channel = keys(&[(0.0, 0.0), (1.0, 100.0), (4.0, 0.0)]);
        assert_eq!(sample_with_segment(&channel, 0.5).segment, 0);
        assert_eq!(sample_with_segment(&channel, 2.5).segment, 1);
        assert!((sample_with_segment(&channel, 2.5).value - 50.0).abs() < 1e-12);
    }

    #[test]
    fn the_sampler_is_monotone_between_keys_of_a_monotone_segment() {
        let channel = keys(&[(0.0, 0.0), (1.0, 10.0)]);
        let mut previous = f64::NEG_INFINITY;
        for step in 0..=100 {
            let value = sample_channel(&channel, step as f64 / 100.0);
            assert!(value >= previous - 1e-12, "step {step}");
            previous = value;
        }
    }
}
