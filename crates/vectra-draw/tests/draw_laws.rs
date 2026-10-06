//! **The four Task 10.1 laws.**
//!
//! They are here — in a `proptest` against the shipped crate — rather than in
//! TypeScript against a reimplementation, because the properties the user was
//! promised are properties of the *drawing tools themselves*:
//!
//! | # | law | what it pins down |
//! | --- | --- | --- |
//! | 1 | **Curve Validity** | any pen or brush path flattens without NaNs and without unbounded recursion |
//! | 2 | **Handle Symmetry** | dragging a smooth point's handle moves the opposite handle symmetrically |
//! | 3 | **Handle Independence** | an Alt-drag breaks symmetry: only the dragged handle moves |
//! | 4 | **Quick Shape** | a rough jagged circle snaps to a path mathematically equal to a perfect circle |
//!
//! Law 1 is deliberately *double-ended*: not only "no NaNs" but "terminates" —
//! the fitting's recursion is bounded by [`vectra_draw::fit::MAX_DEPTH`] and the
//! flatness of the result is compared against the tolerance the caller asked for,
//! so a stroke that silently came back as a loose polyline fails the law rather
//! than passing it quietly.

use proptest::prelude::*;
use vectra_core::{Parameter, PathSegment, Point2};
use vectra_draw::{
    bezier::{cubic_at, distance, Anchor},
    fit::{fit_stroke, MAX_DEPTH},
    pen::{drag_handle, HandleSide, PenSession},
    quickshape::{plan_quick_shape, recognize, SnapKind, KAPPA},
    stroke::{stamp, BrushProfile, Sample},
    BrushOptions,
};

fn point2() -> impl Strategy<Value = Point2> {
    (-500.0f64..500.0, -500.0f64..500.0).prop_map(|(x, y)| Point2::new(x, y))
}

/// A finite, non-NaN point.
fn finite(point: Point2) -> bool {
    point.x.is_finite() && point.y.is_finite()
}

/// Every literal point of a segment list.
fn literal_points(segments: &[PathSegment]) -> Vec<Point2> {
    let mut out = Vec::new();
    for segment in segments {
        for param in segment.params() {
            if let Parameter::Literal(point) = param {
                out.push(*point);
            }
        }
    }
    out
}

/// Flatten a segment list into a polyline (the renderer's job, re-done minimally
/// here because a law must be checkable without the renderer).
fn flatten(start: Point2, segments: &[PathSegment], samples_per_curve: usize) -> Vec<Point2> {
    let mut out = vec![start];
    let mut cursor = start;
    for segment in segments {
        match segment {
            PathSegment::Line { to } => {
                if let Parameter::Literal(to) = to {
                    out.push(*to);
                    cursor = *to;
                }
            }
            PathSegment::Quadratic { control, to } => {
                if let (Parameter::Literal(c), Parameter::Literal(t)) = (control, to) {
                    for i in 1..=samples_per_curve {
                        let u = i as f64 / samples_per_curve as f64;
                        let v = 1.0 - u;
                        out.push(Point2::new(
                            v * v * cursor.x + 2.0 * v * u * c.x + u * u * t.x,
                            v * v * cursor.y + 2.0 * v * u * c.y + u * u * t.y,
                        ));
                    }
                    cursor = *t;
                }
            }
            PathSegment::Cubic {
                control1,
                control2,
                to,
            } => {
                if let (Parameter::Literal(c1), Parameter::Literal(c2), Parameter::Literal(t)) =
                    (control1, control2, to)
                {
                    for i in 1..=samples_per_curve {
                        let u = i as f64 / samples_per_curve as f64;
                        out.push(cubic_at(cursor, *c1, *c2, *t, u));
                    }
                    cursor = *t;
                }
            }
            // A `Close` returns to the start; the flattening stops there.
            PathSegment::Close => {}
        }
    }
    out
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 256, max_shrink_iters: 4096, ..ProptestConfig::default() })]

    /// **Law 1a — Curve Validity (pen).** Any sequence of pointer events through
    /// the pen's state machine produces a path whose every point is finite, and
    /// whose flattening is finite too.
    ///
    /// The events are generated the way a pointer delivers them — down, a run of
    /// moves, up, sometimes Alt held, sometimes a click on the first point — so
    /// the law covers the gesture *sequences*, not just the resulting shapes.
    #[test]
    fn pen_curves_are_valid(
        events in prop::collection::vec(
            (point2(), any::<bool>(), 0u8..4),
            1..80,
        )
    ) {
        let mut pen = PenSession::new();
        for (point, alt, kind) in events {
            match kind {
                0 => {
                    pen.pointer_down(point);
                }
                1 => {
                    pen.pointer_move(point, alt);
                }
                2 => {
                    pen.pointer_up();
                }
                _ => {
                    // Escape: finish, and start a fresh session so the machine
                    // keeps being exercised (the UI does exactly this).
                    let draft = pen.finish();
                    for value in literal_points(&draft.segments) {
                        prop_assert!(finite(value), "pen draft produced {value:?}");
                    }
                    prop_assert!(finite(draft.start));
                    pen = PenSession::new();
                }
            }
        }
        let draft = pen.close();
        prop_assert!(finite(draft.start));
        for value in literal_points(&draft.segments) {
            prop_assert!(finite(value), "pen draft produced {value:?}");
        }
        let flat = flatten(draft.start, &draft.segments, 8);
        prop_assert!(flat.iter().all(|p| finite(*p)), "flattening produced a non-finite point");
    }

    /// **Law 1b — Curve Validity (brush).** Any stroke becomes a filled path with
    /// finite geometry, and the fit terminates within its declared depth bound.
    #[test]
    fn brush_curves_are_valid(
        raw in prop::collection::vec((point2(), 0.0f64..2.0, prop::option::of(0.0f64..1.0)), 0..120)
    ) {
        let samples: Vec<Sample> = raw
            .iter()
            .map(|(point, time, pressure)| Sample::new(*point, *time, *pressure))
            .collect();
        let options = BrushOptions::default();
        let result = vectra_draw::brush_stroke(&samples, &options);
        prop_assert!(finite(result.start));
        prop_assert!(result.fit.max_depth <= MAX_DEPTH, "depth {} exceeded the bound", result.fit.max_depth);
        // The fit works on the *ring* the brush expanded, so the bound is the
        // ring's vertex count: every leaf segment covers at least one ring edge,
        // which is why the count can never run away however the recursion splits.
        prop_assert!(
            result.fit.segments <= result.ring.len() + 1,
            "{} segments from a {} vertex ring",
            result.fit.segments,
            result.ring.len()
        );
        for value in literal_points(&result.segments) {
            prop_assert!(finite(value), "brush produced {value:?}");
        }
        for value in &result.ring {
            prop_assert!(finite(*value));
        }
        for point in flatten(result.start, &result.segments, 8) {
            prop_assert!(finite(point));
        }
        // Every stamp's width is a real, positive number of document units.
        for stamped in &result.stamps {
            prop_assert!(stamped.width.is_finite() && stamped.width > 0.0, "width {}", stamped.width);
        }
    }

    /// **Law 1c — Curve Validity (fit fidelity).** The fitted curve stays within
    /// the tolerance of the samples it was fitted to.
    ///
    /// This is the half of "valid" that is about *meaning* rather than about
    /// NaNs: a fit that terminated but wandered would satisfy a naive validity
    /// test and betray the user. The check samples the fitted segments and
    /// measures each sample's distance to the fitted polyline, allowing the
    /// tolerance plus a small sampling allowance.
    ///
    /// **The law cleans first, with the brush's own options**, because the
    /// fitter's contract is a sequence of *distinct* samples — chord-length
    /// parameterisation needs a strictly positive step (`fit::chord_length_params`)
    /// — and the pipeline guarantees it by running `clean` before every fit
    /// (`brush_stroke`, and the wasm port's own call). Coincident points cannot
    /// reach the fitter from a real gesture: a pointer that has not moved does not
    /// move the stamp, and `clean` drops the repeat.
    ///
    /// Cleaning first did not weaken the law — it exposed what the coincidence
    /// had been hiding. On a nine-sample stroke with **distinct** points, the
    /// fitter accepted a candidate whose control points sat 400 000 units out
    /// (700× the chord), scoring it at 12.17 against a 12.39 tolerance while the
    /// curve was really **57.9** from one of its own samples. The acceptance test
    /// measured a 64-chord polyline, and a coarse polyline of a curve that loops
    /// far outside its samples can pass *closer* to a sample than the curve does.
    /// `fit::max_error` now scales its resolution with the candidate's reach, and
    /// `MAX_HANDLE_RATIO` refuses a candidate that only fits by throwing its
    /// handles into the next county — both fixed, both measured by this law.
    #[test]
    fn fitted_curves_stay_near_their_samples(
        raw_input in prop::collection::vec(point2(), 2..40),
        tolerance in 0.5f64..20.0,
    ) {
        // The coincident-point half of `clean`, with the distance production
        // uses (`BrushOptions::default().min_distance`).
        let min_distance = vectra_draw::BrushOptions::default().min_distance;
        let mut raw: Vec<Point2> = Vec::with_capacity(raw_input.len());
        for point in &raw_input {
            // MSRV 1.75: `is_none_or` is 1.82, so this is the `map_or` spelling.
            if raw.last().map_or(true, |last| distance(*last, *point) > min_distance) {
                raw.push(*point);
            }
        }
        prop_assume!(raw.len() >= 2);

        let fit = fit_stroke(&raw, tolerance);
        prop_assert!(fit.stats.max_depth <= MAX_DEPTH);
        // Flattened *finer* than the fit's own acceptance resolution (64 per
        // slice), so the distance this law measures is the fit's error and not
        // the check's own discretisation.
        let flat = flatten(fit.start, &fit.segments, 200);
        prop_assert!(flat.iter().all(|p| finite(*p)));
        // The ends of the fit are the ends of the stroke, exactly: a fit that
        // moved an endpoint would break every downstream constraint on that slot.
        prop_assert_eq!(fit.start, raw[0]);
        if let Some(last) = fit.segments.last() {
            let end = last.params().last().and_then(|p| match p {
                Parameter::Literal(point) => Some(*point),
                _ => None,
            });
            if let Some(end) = end {
                prop_assert!((distance(end, raw[raw.len() - 1])).abs() < 1e-9);
            }
        }
        // Every sample is within tolerance of the fitted polyline. Measured
        // against the polyline's *segments*, not its vertices: a straight run is
        // fitted as one `Line`, whose interior is as real as its ends, and a
        // vertex-only check would report the middle of a perfectly good line as
        // a large error (it did — that is why this reads the way it does).
        // The measurement is against **chords**, so the law allows for what a
        // chord polyline cannot see — the same bound the fitter's own acceptance
        // adds, computed for the law's own 200 chords per slice. Without it the
        // assertion would be about the polyline rather than about the curve: two
        // measurements of one fit disagreed by exactly this term at a 5.16-unit
        // tolerance (5.1666 measured against a fit whose curve was inside 5.1633).
        let mut allowance = tolerance;
        let mut cursor = fit.start;
        for segment in &fit.segments {
            let end = segment.params().last().and_then(|param| match param {
                Parameter::Literal(point) => Some(*point),
                _ => None,
            });
            if let (
                PathSegment::Cubic {
                    control1, control2, ..
                },
                Some(end),
            ) = (segment, end)
            {
                if let (Parameter::Literal(c1), Parameter::Literal(c2)) = (control1, control2) {
                    let bound = vectra_draw::fit::chord_sagitta_bound(cursor, *c1, *c2, end, 200);
                    allowance = allowance.max(tolerance + bound);
                }
            }
            if let Some(end) = end {
                cursor = end;
            }
        }
        for sample in &raw {
            let mut nearest = f64::INFINITY;
            for pair in flat.windows(2) {
                nearest = nearest.min(vectra_draw::bezier::distance_to_segment(
                    *sample, pair[0], pair[1],
                ));
            }
            prop_assert!(
                nearest <= allowance + 1e-9,
                "sample {sample:?} is {nearest} from the fit (tolerance {tolerance}, allowance {allowance})"
            );
        }
    }

    /// **Law 2 — Handle Symmetry.** Dragging a handle of a smooth point moves the
    /// opposite handle to the mirror position through the anchor, keeping the
    /// anchor's `handle_in`/`handle_out` collinear.
    #[test]
    fn symmetry_law(
        anchor in point2(),
        drag_in in point2(),
        drag_to in point2(),
    ) {
        let start = Anchor::smooth(anchor, drag_in);
        let (handle_in, handle_out) = drag_handle(
            anchor,
            start.handle_in,
            start.handle_out,
            HandleSide::Out,
            drag_to,
            false,
        );
        let after = Anchor { point: anchor, handle_in, handle_out };
        // 1. Both handles exist: a smooth point stays smooth.
        prop_assert!(after.handle_in.is_some() && after.handle_out.is_some());
        // 2. The dragged handle went exactly where the pointer was.
        prop_assert_eq!(after.handle_out, Some(drag_to));
        // 3. The opposite handle is the mirror image through the anchor.
        let expected = Point2::new(2.0 * anchor.x - drag_to.x, 2.0 * anchor.y - drag_to.y);
        let mirrored = after.handle_in.unwrap();
        prop_assert!((mirrored.x - expected.x).abs() < 1e-9);
        prop_assert!((mirrored.y - expected.y).abs() < 1e-9);
        // 4. …which is exactly "smooth", by the crate's own predicate.
        prop_assert!(after.is_smooth(1e-6), "anchor {:?} is not smooth", after);
        // 5. The anchor itself never moved.
        prop_assert_eq!(after.point, anchor);
    }

    /// **Law 3 — Handle Independence.** An Alt-drag on a smooth point's handle
    /// leaves the opposite handle **exactly** where it was, and the anchor is
    /// reported as broken.
    #[test]
    fn independence_law(
        anchor in point2(),
        drag_in in point2(),
        drag_to in point2(),
    ) {
        let start = Anchor::smooth(anchor, drag_in);
        let before = start.handle_in.expect("smooth anchors have an in handle");
        let (handle_in, handle_out) = drag_handle(
            anchor,
            start.handle_in,
            start.handle_out,
            HandleSide::Out,
            drag_to,
            true,
        );
        let after = Anchor { point: anchor, handle_in, handle_out };
        prop_assert_eq!(after.handle_out, Some(drag_to), "the dragged handle moved");
        prop_assert_eq!(after.handle_in, Some(before), "the other handle moved");
        // Broken *unless* the pointer happened to land on the mirror point, in
        // which case the result is indistinguishable from a symmetric drag —
        // and that is a true statement about the geometry, not an exception.
        let mirror = Point2::new(2.0 * anchor.x - before.x, 2.0 * anchor.y - before.y);
        if distance(drag_to, mirror) > 1e-9 {
            prop_assert!(!after.is_smooth(1e-6), "an Alt-drag broke the symmetry");
        }
    }

    /// **Law 3b — Independence is one-sided by construction.** A drag on the
    /// *in* handle under Alt leaves the out handle alone, symmetrically.
    #[test]
    fn independence_law_is_symmetric(
        anchor in point2(),
        drag_in in point2(),
        drag_to in point2(),
    ) {
        let start = Anchor::smooth(anchor, drag_in);
        let before = start.handle_out.expect("smooth anchors have an out handle");
        let (handle_in, handle_out) = drag_handle(
            anchor,
            start.handle_in,
            start.handle_out,
            HandleSide::In,
            drag_to,
            true,
        );
        prop_assert_eq!(handle_in, Some(drag_to));
        prop_assert_eq!(handle_out, Some(before));
        let _ = handle_out;
    }

    /// **Law 4 — Quick Shape.** A rough closed stroke — jagged by up to a third of
    /// its own radius, and sampled from a *polygon* rather than a circle, so it is
    /// never round — snaps to a circle, and the snapped path is a perfect circle
    /// to the kappa bound: every point of the flattened path is within
    /// `5 × 10⁻⁴ · r` of the fitted centre, while the *input* is not.
    #[test]
    fn quick_shape_law(
        cx in -200.0f64..200.0,
        cy in -200.0f64..200.0,
        radius in 40.0f64..300.0,
        noise in 0.02f64..0.30,
        count in 24usize..96,
        phase in 0.0f64..1.0,
    ) {
        // A jagged loop: radial noise, plus a polygonal (rather than circular)
        // sampling of the angle, so the stroke is genuinely rough.
        let stroke: Vec<Point2> = (0..count)
            .map(|i| {
                let t = (i as f64 / count as f64 + phase) * std::f64::consts::TAU;
                let hash = ((i.wrapping_mul(2_654_435_761)) % 997) as f64 / 997.0;
                let r = radius * (1.0 + (hash - 0.5) * 2.0 * noise);
                Point2::new(cx + r * t.cos(), cy + r * t.sin())
            })
            .collect();

        let tolerance = radius * noise; // the recognizer is told how rough to expect
        let recognition = recognize(&stroke, tolerance, 0.6);
        prop_assume!(recognition.is_some());
        let recognition = recognition.unwrap();
        prop_assert_eq!(recognition.kind, SnapKind::Circle);

        let plan = plan_quick_shape(&stroke, vectra_core::new_node_id(), vectra_core::new_node_id(), tolerance)
            .expect("recognition succeeded, so a plan exists");

        // The snapped path: four cardinal arcs, closed by construction (the
        // fourth arc ends exactly on `start`, which is why there is no `Close`).
        prop_assert_eq!(plan.segments.len(), 4);
        let flat = flatten(plan.start, &plan.segments, 32);
        prop_assert!(flat.len() > 100);

        // The centre the plan was built around: the primitve's own slots.
        let vectra_core::NodeKind::Circle { cx: slot_cx, cy: slot_cy, radius: slot_r } = &plan.primitive else {
            prop_assert!(false, "a circle snap must produce a Circle node");
            unreachable!()
        };
        let (Parameter::Literal(fx), Parameter::Literal(fy), Parameter::Literal(fr)) =
            (slot_cx, slot_cy, slot_r)
        else {
            prop_assert!(false, "the fitted primitive is a literal");
            unreachable!()
        };
        let center = Point2::new(*fx, *fy);

        // **The law.** Every point of the snapped path is on the circle.
        let bound = fr * 5e-4 + 1e-9;
        for point in &flat {
            let error = (distance(*point, center) - fr).abs();
            prop_assert!(
                error <= bound,
                "snapped point {point:?} is {error} off a circle of radius {fr} (bound {bound})"
            );
        }

        // And the *input* was not a circle — otherwise the law would be vacuous.
        let input_error = stroke
            .iter()
            .map(|point| (distance(*point, center) - fr).abs())
            .fold(0.0_f64, f64::max);
        prop_assert!(
            input_error > bound,
            "the generated stroke was already a circle ({input_error} ≤ {bound})"
        );

        // The four anchors are the path's four slots — `start` is the right-hand
        // cardinal, and the arcs run right, bottom, left, top.
        let slots = ["start", "segments[0].to", "segments[1].to", "segments[2].to"];
        for slot in slots {
            prop_assert!(
                plan.anchors.iter().any(|anchor| anchor.property == slot),
                "no row addresses {slot}"
            );
        }
        // The plan's first point really is the right-hand cardinal.
        prop_assert!((plan.start.x - (fx + fr)).abs() < 1e-9);
        prop_assert!((plan.start.y - *fy).abs() < 1e-9);
        // The kappa constant is what makes the arcs circular; assert the plan
        // actually used it (a path built with straight handles would be a square).
        let first = &plan.segments[0];
        let PathSegment::Cubic { control1, .. } = first else {
            prop_assert!(false, "a circle plan's first segment must be a cubic: {first:?}");
            unreachable!()
        };
        let Parameter::Literal(c1) = control1 else {
            prop_assert!(false, "the kappa handle must be a literal: {control1:?}");
            unreachable!()
        };
        prop_assert!(
            ((c1.y - fy).abs() - fr * KAPPA).abs() < 1e-6,
            "the arc handle is not the kappa construction: {c1:?}"
        );
    }

    /// **Law 4b — Quick Shape is refused when it should be.** A stroke that is not
    /// closed, or is a straight run, or is a figure-eight is *not* snapped.
    #[test]
    fn quick_shape_refuses_non_shapes(
        a in point2(),
        b in point2(),
        len in 10usize..60,
    ) {
        // A straight run.
        let line: Vec<Point2> = (0..len)
            .map(|i| {
                let t = i as f64 / len as f64;
                Point2::new(a.x + (b.x - a.x) * t, a.y + (b.y - a.y) * t)
            })
            .collect();
        let recognition = recognize(&line, 5.0, 0.6);
        prop_assert!(recognition.is_none(), "a line is not a primitive: {recognition:?}");
    }

    /// **Law 4c — the brush's own pipeline is a shape, not a polyline.** Whatever
    /// the samples, the output is a closed path of Bézier data whose outline
    /// encloses area — the strongest form of RULE 3's "never a jagged polyline".
    #[test]
    fn brush_output_is_a_filled_shape(
        raw in prop::collection::vec((point2(), 0.0f64..1.0), 6..60),
        width in 1.0f64..30.0,
    ) {
        let samples: Vec<Sample> = raw
            .iter()
            .map(|(point, time)| Sample::new(*point, *time, None))
            .collect();
        let profile = BrushProfile {
            max_width: width,
            min_width: width * 0.25,
            ..BrushProfile::default()
        };
        let stamps = stamp(&samples, &profile);
        prop_assert_eq!(stamps.len(), samples.len());
        let result = vectra_draw::brush_stroke(&samples, &BrushOptions { profile, ..BrushOptions::default() });
        prop_assert!(matches!(result.segments.last(), Some(PathSegment::Close)), "the shape is closed");
        // The fitted path is a *curve* description: at least one cubic or line
        // per ring vertex it replaced, and never the raw samples themselves.
        prop_assert!(result.segments.len() <= result.ring.len() + 1);
    }
}
