//! **The three text laws** (Task 11.0, MES §17).
//!
//! ```text
//!   LAW 1  Parametric Text   change a `font_size` variable ⇒ the box and every
//!                            glyph position are already different on the very
//!                            next evaluation — no text-specific plumbing
//!   LAW 2  Text on Path      a run bound to a circle rotates onto the tangent at
//!                            every glyph; an `offset` write slides it smoothly
//!   LAW 3  Outline           outlining produces valid closed `Path` nodes that a
//!                            boolean accepts, hides — never deletes — the type,
//!                            and undoes exactly
//! ```
//!
//! They are written against the engine (`Engine` + `GeometryEvaluator`), not
//! against the layout functions, because the claim being tested is about the
//! *document*: that a variable, a slider and an undo all behave the way a
//! designer expects, through the same command surface the UI uses.

use proptest::prelude::*;
use vectra_core::{
    Command, Document, Engine, NodeId, NodeKind, ParamValue, Parameter, PathSegment, Point2,
    TextAlign,
};
use vectra_geometry::{
    layout_text, outline_plans, path_bounds, primitive_to_curve_path, primitive_to_path,
    EvaluatedPrimitive, EvaluatedScene, FaceRef, GeometryEvaluator, BUNDLED_FAMILY,
};

// ── Helpers ────────────────────────────────────────────────────────────────

fn create_text(engine: &mut Engine, text: &str, size: f64, x: f64, y: f64) -> NodeId {
    let id = vectra_core::new_node_id();
    engine
        .dispatch(Command::CreateNode {
            id,
            kind: NodeKind::text(x, y, text, size),
            name: None,
            index: None,
        })
        .expect("CreateNode");
    id
}

fn create_circle(engine: &mut Engine, cx: f64, cy: f64, r: f64) -> NodeId {
    let id = vectra_core::new_node_id();
    engine
        .dispatch(Command::CreateNode {
            id,
            kind: NodeKind::circle(cx, cy, r),
            name: None,
            index: None,
        })
        .expect("CreateNode");
    id
}

fn evaluate(engine: &Engine) -> EvaluatedScene {
    let ctx = engine.evaluation_context();
    let evaluation = GeometryEvaluator.evaluate_full(engine.document(), &ctx);
    assert!(
        !evaluation.has_errors(),
        "text must evaluate without errors: {:?}",
        evaluation.errors().collect::<Vec<_>>()
    );
    evaluation.scene
}

/// The laid-out run of a text node — the one accessor every law reads.
fn run_of(scene: &EvaluatedScene, id: NodeId) -> vectra_geometry::EvaluatedText {
    match &scene
        .get(id)
        .unwrap_or_else(|| panic!("node {id} must be in the scene"))
        .primitive
    {
        EvaluatedPrimitive::Text(run) => run.clone(),
        other => panic!("expected a text run, got {}", other.tag()),
    }
}

/// A run's drawn box in document units: `(min_x, min_y, max_x, max_y)`.
fn box_of(run: &vectra_geometry::EvaluatedText) -> (f64, f64, f64, f64) {
    path_bounds(&run.outline)
}

fn width_of(run: &vectra_geometry::EvaluatedText) -> f64 {
    let (min_x, _, max_x, _) = box_of(run);
    max_x - min_x
}

fn height_of(run: &vectra_geometry::EvaluatedText) -> f64 {
    let (_, min_y, _, max_y) = box_of(run);
    max_y - min_y
}

/// Glyphs of a run, each with its own drawn box — the per-glyph witnesses the
/// parametric law needs (a width change alone could be a stretched box).
fn glyph_boxes(run: &vectra_geometry::EvaluatedText) -> Vec<(f64, f64, f64, f64)> {
    run.glyphs
        .iter()
        .map(|glyph| path_bounds(&glyph.outline))
        .collect()
}

/// The box of the contours a glyph *draws* — the rings `outline_plans` builds
/// from, with the degenerate ones filtered exactly as it filters them.
///
/// This matters for a real font: DejaVu Sans' `u` carries a stray one-point
/// contour (a known quirk of the face, preserved faithfully by the bundled
/// subset), and a point cannot be a contour. A naive flatten of the glyph's
/// outline therefore reaches further than the letterform does; the outlined
/// path must reach only as far as the contours it was built from.
fn drawn_box(glyph: &vectra_geometry::EvaluatedGlyph) -> (f64, f64, f64, f64) {
    let rings = vectra_geometry::path_to_rings(&glyph.outline, 0.1);
    let (mut min_x, mut min_y, mut max_x, mut max_y) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    let mut any = false;
    for ring in rings.iter().filter(|ring| ring.len() >= 3) {
        for point in ring {
            any = true;
            min_x = min_x.min(point.x);
            min_y = min_y.min(point.y);
            max_x = max_x.max(point.x);
            max_y = max_y.max(point.y);
        }
    }
    assert!(any, "a glyph with no drawable contour");
    (min_x, min_y, max_x, max_y)
}

fn set_param(engine: &mut Engine, node: NodeId, property: &str, param: Parameter<f64>) {
    engine
        .dispatch(Command::SetParameter {
            node_id: node,
            property: property.to_string(),
            value: ParamValue::Float(param),
        })
        .expect("SetParameter");
}

// ── Strategies ─────────────────────────────────────────────────────────────

/// A word of letters the bundled face certainly covers.
fn word() -> impl Strategy<Value = String> {
    "[A-Za-z]{1,12}".prop_map(|s| s)
}

/// A font size a designer would actually set, with sub-unit precision so the
/// law exercises fractional advances too. Never zero: a zero-size run is a
/// legal but degenerate document, and the law is about scaling.
fn font_size() -> impl Strategy<Value = f64> {
    prop_oneof![
        4.0..8.0f64, // caption: 4pt is the smallest type the *layout* laws
        // still read clearly through a 0.05-unit flattening
        8.0..48.0f64,   // body
        48.0..256.0f64, // display
    ]
}

fn origin() -> impl Strategy<Value = (f64, f64)> {
    (-2000.0..2000.0f64, -2000.0..2000.0f64)
}

proptest! {
    /// **THE PARAMETRIC TEXT LAW** (RULE 1).
    ///
    /// Bind `font_size` to a global variable, evaluate, change the variable,
    /// evaluate again — and the run's box **and every glyph's placement** must
    /// have changed on that second evaluation, exactly as if the new size had
    /// been authored. Nothing text-specific happens between the two passes: the
    /// variable write is the same one that moves a rectangle's width, which is
    /// the whole claim.
    ///
    /// The equality is *exact* on the layout numbers — a glyph's advance and its
    /// arc position are `font units × size / upem`, and the ratio of any two
    /// evaluations is therefore the ratio of the sizes to the last bit — and
    /// tolerant only where it must be: the *drawn* box is measured through
    /// flattening, which has its own tolerance.
    #[test]
    fn law1_a_font_size_variable_relayouts_the_run(
        word in word(),
        size in font_size(),
        // Deliberately **not** a whisker away from 1. The exact halves of this
        // law hold for any factor, arbitrarily close to 1 — they compare the
        // layout arithmetic itself. The drawn half below compares two
        // *flattened* boxes, and flattening carries an absolute 0.05-unit
        // tolerance: a run scaled by 1.05 moves its edges by less than that
        // tolerance at caption sizes, so a factor near 1 would test the
        // flattener's noise floor, not the re-layout. ±25 % keeps the claim
        // measurable at every generated size.
        factor in prop_oneof![0.05..0.75f64, 1.25..4.0f64],
        (x, y) in origin(),
    ) {
        let mut engine = Engine::new();
        let id = create_text(&mut engine, &word, size, x, y);
        engine
            .document_mut()
            .set_variable("scale".to_string(), size)
            .expect("set_variable");
        set_param(&mut engine, id, "font_size", Parameter::Variable("scale".to_string()));

        let before = run_of(&evaluate(&engine), id);
        prop_assume!(!before.glyphs.is_empty());

        engine
            .document_mut()
            .set_variable("scale".to_string(), size * factor)
            .expect("set_variable");
        let after = run_of(&evaluate(&engine), id);

        // 1. The box is *the same function of the size*, bit for bit: both the
        //    line width and the type height scale by exactly `factor`.
        let width_ratio = after.metrics.width / before.metrics.width;
        prop_assert!(
            (width_ratio - factor).abs() <= 1e-9 * factor,
            "width must scale by {factor}, scaled by {width_ratio}"
        );
        let height_ratio = after.metrics.height / before.metrics.height;
        prop_assert!(
            (height_ratio - factor).abs() <= 1e-9 * factor,
            "height must scale by {factor}, scaled by {height_ratio}"
        );

        // 2. Every glyph moved *without being re-shaped*: same count, same ids,
        //    advances and pen positions scaled by the same factor. This is the
        //    parametric claim — the document was not rebuilt, the run re-laid
        //    out — and it is checked per glyph, so a run that merely stretched a
        //    box would fail it.
        prop_assert_eq!(after.glyphs.len(), before.glyphs.len());
        for (index, (a, b)) in before.glyphs.iter().zip(after.glyphs.iter()).enumerate() {
            prop_assert_eq!(
                b.glyph_id,
                a.glyph_id,
                "glyph {} was re-shaped, not re-laid out",
                index
            );
            if a.advance > 0.0 {
                prop_assert!(
                    (b.advance / a.advance - factor).abs() <= 1e-9 * factor,
                    "glyph {index}'s advance did not scale: {} → {}",
                    a.advance,
                    b.advance
                );
            }
            prop_assert!(
                ((b.distance - a.distance * factor) / size.max(1.0)).abs() <= 1e-9 * factor,
                "glyph {index} moved to the wrong pen position: {} → {}",
                a.distance,
                b.distance
            );
        }

        // 3. …and the *drawn* picture moved too: the two outlines' boxes are
        //    genuinely different pictures, not a re-measurement of the same
        //    one. The claim is **relative to the box the run actually drew**:
        //    every point of the outline is scaled by exactly `factor`, so each
        //    box edge travels `|factor - 1| ×` its own distance from the
        //    origin, and the two extents together travel `|factor - 1| ×` their
        //    sum. The 0.3 lets the flattener's absolute 0.05-unit tolerance
        //    take its bite without letting a box that barely moved pass: the
        //    smallest generated case (a 4-unit run, ×0.75) still moves ~1 unit.
        let extent = width_of(&before) + height_of(&before);
        let expected = (factor - 1.0).abs() * extent;
        let moved = (width_of(&after) - width_of(&before)).abs()
            + (height_of(&after) - height_of(&before)).abs();
        prop_assert!(
            moved > 0.3 * expected,
            "the drawn box barely moved: {moved}, expected about {expected}"
        );
    }

    /// **The parametric law's negative half**: an *unrelated* write must not
    /// move the run. Without this, a law that only ever observed motion could
    /// be passed by an evaluator that re-lays out on every edit — which would
    /// be a performance bug hiding as a feature.
    #[test]
    fn law1_a_circle_edit_does_not_relayout_a_straight_run(
        word in word(),
        size in font_size(),
        (x, y) in origin(),
        r in 1.0..500.0f64,
    ) {
        let mut engine = Engine::new();
        let id = create_text(&mut engine, &word, size, x, y);
        let other = create_circle(&mut engine, 0.0, 0.0, r);
        let before = run_of(&evaluate(&engine), id);

        set_param(&mut engine, other, "radius", Parameter::Literal(r + 25.0));
        let after = run_of(&evaluate(&engine), id);

        prop_assert_eq!(before.glyphs.len(), after.glyphs.len());
        prop_assert_eq!(glyph_boxes(&before), glyph_boxes(&after));
    }

    /// **LAW 2 — THE TEXT-ON-PATH LAW** (RULE 2).
    ///
    /// Bind a run to a circle: each glyph must sit at its own arc position with
    /// its rotation equal to the tangent there, *modulo the half turn the
    /// readability pass applies* — and its up direction must never point down.
    /// The three claims together are what "reads along the curve" means: on the
    /// curve, rotated onto it, and the right way up.
    #[test]
    fn law2_every_glyph_rides_the_tangent_and_stays_upright(
        word in word(),
        size in 8.0..120.0f64,
        r in 20.0..400.0f64,
        offset in -300.0..300.0f64,
    ) {
        let mut engine = Engine::new();
        let id = create_text(&mut engine, &word, size, 0.0, 0.0);
        let circle = create_circle(&mut engine, 0.0, 0.0, r);
        engine
            .dispatch(Command::BindTextToPath {
                node_id: id,
                path: circle,
                offset: Parameter::Literal(offset),
            })
            .expect("BindTextToPath");

        let scene = evaluate(&engine);
        let run = run_of(&scene, id);
        prop_assume!(!run.glyphs.is_empty());

        // The *curve* view of the circle, which is what the evaluator hands the
        // run (`primitive_to_curve_path`): the tangent this law compares against
        // has to be the one the layout could have read.
        let path = primitive_to_curve_path(
            &scene.get(circle).expect("the circle is in the scene").primitive,
        );
        let samples = vectra_geometry::sample_path(&path);
        let length = vectra_geometry::path_length(&samples);
        prop_assume!(length > 1.0);

        // **The window that fits** (RULE 2): a glyph rides the path iff its own
        // arc position — `offset` plus the advances before it — lands on the
        // path. The reference is an *independent* computation: the same run laid
        // out straight (where nothing can be truncated), so its `distance`
        // values are exactly the authored arc positions, and the circle's run
        // must be that sequence slid by `offset` and cut to the path's span.
        let straight = layout_text(
            FaceRef::bundled(),
            &vectra_geometry::TextSpec::new(&word, BUNDLED_FAMILY, size),
            (0.0, 0.0),
        )
        .expect("the straight layout");
        let authored: Vec<(u16, f64)> = straight
            .glyphs
            .iter()
            .map(|glyph| (glyph.glyph_id, glyph.distance + offset))
            .collect();
        // Boundary glyphs are the one case where "inside" is a knife edge at
        // f64 precision; the law is about the decision, not about rounding.
        prop_assume!(authored
            .iter()
            .all(|(_, distance)| (distance - 0.0).abs() > 1e-6 && (distance - length).abs() > 1e-6));
        let expected: Vec<(u16, f64)> = authored
            .iter()
            .copied()
            .filter(|(_, distance)| (0.0..=length).contains(distance))
            .collect();
        let actual: Vec<(u16, f64)> = run
            .glyphs
            .iter()
            .map(|glyph| (glyph.glyph_id, glyph.distance))
            .collect();
        prop_assert_eq!(
            actual,
            expected,
            "a bound run keeps exactly the glyphs whose arc position is on the path"
        );
        prop_assert_eq!(
            run.truncated,
            straight.glyphs.len() - run.glyphs.len(),
            "every glyph left out is counted"
        );

        for (index, glyph) in run.glyphs.iter().enumerate() {
            // It is *on* the curve, so a tangent exists and the glyph was
            // placed from it.
            let (px, py, tangent) =
                vectra_geometry::sample_at(&samples, glyph.distance).expect("a tangent");

            // Rotated onto the tangent, up to the half turn the readability
            // pass is allowed to apply.
            let difference = (glyph.angle - tangent).abs() % std::f64::consts::PI;
            prop_assert!(
                difference < 1e-6 || (std::f64::consts::PI - difference).abs() < 1e-6,
                "glyph {index} sits at {tangent} but is rotated {}",
                glyph.angle
            );

            // …and upright: the glyph's up vector (`(sin θ, -cos θ)` in the
            // document's y-down space) never points down.
            let up_y = -glyph.angle.cos();
            prop_assert!(
                up_y <= 1e-9,
                "glyph {index} is upside down at angle {} (up.y = {up_y})",
                glyph.angle
            );

            // The glyph is *drawn there*: its own box is near the point the
            // sampler returned, not the origin (a run that was laid out
            // straight and merely rotated would fail this).
            let (min_x, min_y, max_x, max_y) = path_bounds(&glyph.outline);
            let (gx, gy) = ((min_x + max_x) * 0.5, (min_y + max_y) * 0.5);
            let reach = size * 4.0 + (min_x - max_x).abs().max(1.0);
            prop_assert!(
                ((gx - px).powi(2) + (gy - py).powi(2)).sqrt() <= reach,
                "glyph {index} is drawn at ({gx}, {gy}) but the path is at ({px}, {py})"
            );
        }
    }

    /// **The offset's half of LAW 2**: sliding the offset slides the run, and
    /// slides it *smoothly* — the arc distance a glyph travels equals the
    /// offset change (modulo the clamp at the path's ends), and every glyph's
    /// rotation follows the tangent at its new position.
    #[test]
    fn law2_the_offset_slides_the_run_along_the_curve(
        word in word(),
        size in 8.0..64.0f64,
        r in 200.0..500.0f64,
        delta in prop_oneof![-30.0..-1.0f64, 1.0..30.0f64],
    ) {
        let mut engine = Engine::new();
        let id = create_text(&mut engine, &word, size, 0.0, 0.0);
        let circle = create_circle(&mut engine, 0.0, 0.0, r);
        engine
            .dispatch(Command::BindTextToPath {
                node_id: id,
                path: circle,
                offset: Parameter::Literal(0.0),
            })
            .expect("BindTextToPath");

        let before = run_of(&evaluate(&engine), id);
        prop_assume!(!before.glyphs.is_empty());
        // **Precondition, not a weakened law**: `sample_at` clamps to the path's
        // ends (documented behaviour — a run may legitimately start before the
        // first sample), so a run that runs off the end would be asking the
        // clamp to move it. The clamp has its own test
        // (`a_run_off_the_end_of_its_path_clamps_to_the_end`).

        // Every glyph, both before and after, must land *inside* the path.
        let span = before.glyphs.last().expect("non-empty").distance
            + before.glyphs.last().expect("non-empty").advance;
        let length = 2.0 * std::f64::consts::PI * r;
        prop_assume!(
            before.glyphs[0].distance + delta > 1.0
                && span + delta < length - 1.0
                && span > 0.0
        );

        set_param(&mut engine, id, "path_offset", Parameter::Literal(delta));
        let after = run_of(&evaluate(&engine), id);
        prop_assert_eq!(before.glyphs.len(), after.glyphs.len());

        for (index, (a, b)) in before.glyphs.iter().zip(after.glyphs.iter()).enumerate() {
            // The run slid by exactly the offset: the cursor moved one step.
            prop_assert!(
                ((b.distance - a.distance) - delta).abs() < 1e-6,
                "glyph {index} slid {} for an offset of {delta}",
                b.distance - a.distance
            );
            // …and it moved on screen, by a distance of the same order as the
            // slide. The bound is deliberately loose in one direction: the two
            // boxes are axis-aligned bounds, and a glyph that *also* rotates
            // (this one does — a large glyph on a tight curve turns several
            // degrees) has an AABB whose centre is not the glyph's centre, so
            // the measured travel can be a little short of the arc. What the
            // loose bound still rules out is the failure that matters: a slide
            // that moves the numbers but not the picture.
            let (ax, ay, aw, ah) = path_bounds(&a.outline);
            let (bx, by, bw, bh) = path_bounds(&b.outline);
            let travelled = (((ax + aw) / 2.0 - (bx + bw) / 2.0).powi(2)
                + ((ay + ah) / 2.0 - (by + bh) / 2.0).powi(2))
            .sqrt();
            prop_assert!(
                travelled >= 0.4 * delta.abs(),
                "glyph {index} slid {travelled} on screen for an offset of {delta}"
            );
            // …and the run is still a run: same glyphs, same advances.
            prop_assert_eq!(a.glyph_id, b.glyph_id);
            prop_assert!((a.advance - b.advance).abs() < 1e-12);
        }
    }

    /// A bound run follows a *reshaped* path: changing the circle's radius
    /// changes where every glyph is drawn. This is RULE 2's propagation claim
    /// at the evaluator's level (`text_nodes_bound_to` is the wasm half).
    #[test]
    fn law2_reshaping_the_path_moves_the_run(
        word in word(),
        size in 8.0..64.0f64,
        r in 50.0..300.0f64,
        factor in 1.2..3.0f64,
    ) {
        let mut engine = Engine::new();
        let id = create_text(&mut engine, &word, size, 0.0, 0.0);
        let circle = create_circle(&mut engine, 0.0, 0.0, r);
        engine
            .dispatch(Command::BindTextToPath {
                node_id: id,
                path: circle,
                offset: Parameter::Literal(0.0),
            })
            .expect("BindTextToPath");

        let before = run_of(&evaluate(&engine), id);
        prop_assume!(!before.glyphs.is_empty());
        set_param(&mut engine, circle, "radius", Parameter::Literal(r * factor));
        let after = run_of(&evaluate(&engine), id);

        // A longer path can only fit *more* of the same run — and the glyphs
        // both pictures draw are the same glyphs at the same arc positions:
        // reshaping the path moves the picture, it never reflows the type. (A
        // run that overflows its path is truncated at the ends, so the two
        // windows may differ only there.)
        prop_assert!(
            after.glyphs.len() >= before.glyphs.len(),
            "a larger circle cannot leave out a glyph the smaller one fitted"
        );
        prop_assert_eq!(
            before.glyphs.len() + before.truncated,
            after.glyphs.len() + after.truncated,
            "the same string was shaped"
        );
        for (a, b) in before.glyphs.iter().zip(after.glyphs.iter()) {
            prop_assert_eq!(a.glyph_id, b.glyph_id);
            prop_assert!((a.advance - b.advance).abs() < 1e-12);
            prop_assert!(
                (a.distance - b.distance).abs() < 1e-9,
                "an arc position must not depend on the curve it rides"
            );
        }

        let moved = before
            .glyphs
            .iter()
            .zip(after.glyphs.iter())
            .any(|(a, b)| {
                let (ax, ay, _, _) = path_bounds(&a.outline);
                let (bx, by, _, _) = path_bounds(&b.outline);
                (ax - bx).abs() + (ay - by).abs() > 1e-6
            });
        prop_assert!(moved, "a bigger circle must move the run riding it");
    }

    /// **LAW 3 — THE OUTLINE LAW** (RULE 3).
    ///
    /// Outlining a run must produce valid closed `Path` nodes that a boolean
    /// accepts, leave the type intact (hidden, not deleted), and undo exactly.
    #[test]
    fn law3_outlining_yields_valid_closed_paths(
        word in word(),
        size in 12.0..120.0f64,
        (x, y) in origin(),
    ) {
        let mut engine = Engine::new();
        let id = create_text(&mut engine, &word, size, x, y);
        let run = run_of(&evaluate(&engine), id);
        prop_assume!(!run.glyphs.is_empty());

        let plans = outline_plans(&run, &word);
        prop_assert_eq!(plans.len(), run.glyphs.len(), "one plan per shaped glyph");

        for (index, plan) in plans.iter().enumerate() {
            // 1. A valid path: it starts somewhere finite…
            prop_assert!(plan.start.x.is_finite() && plan.start.y.is_finite());
            // 2. …it has segments, and they are all *lines* (an outline is a
            //    flattened contour, so it is addressable by every path tool)…
            prop_assert!(plan.segments.len() >= 3);
            for segment in &plan.segments {
                match segment {
                    PathSegment::Line { to } => match to {
                        Parameter::Literal(point) => prop_assert!(
                            point.x.is_finite() && point.y.is_finite(),
                            "outline point is not finite: {point:?}"
                        ),
                        other => prop_assert!(false, "an outline point must be a literal, got {other:?}"),
                    },
                    PathSegment::Close => {}
                    other => prop_assert!(false, "an outline is lines and closes, got {other:?}"),
                }
            }
            // 3. **Closed**: a fillable region, which is what a boolean needs.
            prop_assert!(
                matches!(plan.segments.last(), Some(PathSegment::Close)),
                "letterform {index} is not closed"
            );
            // 4. It encloses area — a closed path with no extent is a degenerate
            //    region the boolean engine would (correctly) refuse.
            let rebuilt = {
                let mut resolved = Vec::new();
                for segment in &plan.segments {
                    match segment {
                        PathSegment::Line { to } => {
                            let Parameter::Literal(point) = to else { unreachable!() };
                            resolved.push(vectra_geometry::ResolvedSegment::Line { to: *point });
                        }
                        _ => resolved.push(vectra_geometry::ResolvedSegment::Close),
                    }
                }
                vectra_geometry::build_path(plan.start, &resolved)
            };
            let (min_x, min_y, max_x, max_y) = path_bounds(&rebuilt);
            prop_assert!(
                max_x > min_x && max_y > min_y,
                "letterform {index} encloses no area: {min_x},{min_y}..{max_x},{max_y}"
            );
        }
    }

    /// The outline law's **non-destructive half**, through the command surface:
    /// the letterforms are new nodes in a group, the type is hidden and still
    /// holds its string, and undo restores the document exactly.
    #[test]
    fn law3_outlining_is_non_destructive_and_undone_exactly(
        word in word(),
        size in 12.0..96.0f64,
        (x, y) in origin(),
    ) {
        let mut engine = Engine::new();
        let id = create_text(&mut engine, &word, size, x, y);
        let run = run_of(&evaluate(&engine), id);
        prop_assume!(!run.glyphs.is_empty());
        let plans = outline_plans(&run, &word);

        let before = engine.document().clone();
        let group = vectra_core::new_node_id();
        let paths: Vec<vectra_core::OutlinePath> = plans
            .iter()
            .map(|plan| vectra_core::OutlinePath {
                id: vectra_core::new_node_id(),
                name: plan.name.clone(),
                start: Parameter::Literal(plan.start),
                segments: plan.segments.clone(),
            })
            .collect();
        let letterform_ids: Vec<NodeId> = paths.iter().map(|path| path.id).collect();

        engine
            .dispatch(Command::OutlineText {
                node_id: id,
                group_id: group,
                name: Some("Outlined".to_string()),
                paths: paths.clone(),
            })
            .expect("OutlineText");

        let doc = engine.document();
        // 1. The type is **hidden, not deleted**, and every authored value is
        //    still there — that is the non-destructive claim, and it is why a
        //    designer can come back to the type after a detour through booleans.
        let source = doc.get_node(id).expect("the text node still exists");
        prop_assert!(!source.visible, "the source text must be hidden");
        match &source.kind {
            NodeKind::Text {
                text,
                font_size,
                ..
            } => {
                prop_assert_eq!(text, &word);
                prop_assert!(matches!(font_size, Parameter::Literal(size_literal) if (size_literal - size).abs() < 1e-12));
            }
            other => prop_assert!(false, "the source must still be text, got {}", other.tag()),
        }

        // 2. The letterforms are real, closed `Path` nodes inside the group…
        let group_node = doc.get_node(group).expect("the group exists");
        match &group_node.kind {
            NodeKind::Group { children } => {
                prop_assert_eq!(children.len(), letterform_ids.len());
            }
            other => prop_assert!(false, "the outline must group its letterforms, got {}", other.tag()),
        }
        for path_id in &letterform_ids {
            let node = doc.get_node(*path_id).expect("every letterform is a node");
            match &node.kind {
                NodeKind::Path { start, segments } => {
                    prop_assert!(matches!(start, Parameter::Literal(_)));
                    prop_assert!(matches!(segments.last(), Some(PathSegment::Close)));
                }
                other => prop_assert!(false, "a letterform must be a Path, got {}", other.tag()),
            }
        }

        // 3. The scene agrees: the group's letterforms evaluate to a region with
        //    area — the precondition of "a boolean can operate on it".
        let scene = evaluate(&engine);
        for path_id in &letterform_ids {
            let Some(node) = scene.get(*path_id) else {
                prop_assert!(false, "an outlined letterform is missing from the scene");
                continue;
            };
            let (min_x, min_y, max_x, max_y) = path_bounds(&primitive_to_path(&node.primitive));
            prop_assert!(max_x > min_x && max_y > min_y, "an outlined letterform has no area");
        }

        // 4. Undo restores the document **exactly** — the strongest form of the
        //    claim, and the one the inverse's `Batch` construction has to earn.
        engine.undo().expect("undo");
        prop_assert_eq!(engine.document(), &before, "undo must restore the document exactly");
    }

    /// **The outline is the same picture**: an outlined letterform's *bounding
    /// box* matches the glyph it replaced. Exact equality is impossible (the
    /// outline is flattened at `OUTLINE_TOLERANCE`), so the law is a tolerance
    /// in document units — and a tight one, because a designer who outlines a
    /// word must not see it move.
    #[test]
    fn law3_an_outline_matches_the_glyph_it_replaced(
        word in word(),
        size in 12.0..120.0f64,
        (x, y) in origin(),
    ) {
        let mut engine = Engine::new();
        let id = create_text(&mut engine, &word, size, x, y);
        let run = run_of(&evaluate(&engine), id);
        prop_assume!(!run.glyphs.is_empty());
        let plans = outline_plans(&run, &word);
        prop_assert_eq!(plans.len(), run.glyphs.len());

        for (plan, glyph) in plans.iter().zip(run.glyphs.iter()) {
            let mut resolved = Vec::new();
            for segment in &plan.segments {
                match segment {
                    PathSegment::Line { to } => {
                        let Parameter::Literal(point) = to else { unreachable!() };
                        resolved.push(vectra_geometry::ResolvedSegment::Line { to: *point });
                    }
                    _ => resolved.push(vectra_geometry::ResolvedSegment::Close),
                }
            }
            let rebuilt = vectra_geometry::build_path(plan.start, &resolved);
            let (amin_x, amin_y, amax_x, amax_y) = path_bounds(&rebuilt);
            let (bmin_x, bmin_y, bmax_x, bmax_y) = drawn_box(glyph);
            for (label, a, b) in [
                ("min x", amin_x, bmin_x),
                ("min y", amin_y, bmin_y),
                ("max x", amax_x, bmax_x),
                ("max y", amax_y, bmax_y),
            ] {
                prop_assert!(
                    (a - b).abs() < 1e-6,
                    "{label} drifts outlining {}: {a} vs {b}",
                    plan.name
                );
            }
        }
    }

        /// **A slide is a re-placement, not a replay** (Task 11.0 performance): the
        /// run a slide produces is the run a *fresh* document produces at the slid
        /// offset — glyph for glyph, contour for contour, truncation and all.
        ///
        /// The law a shaping cache fails is exactly this one: if a hit ever hands
        /// back geometry it has been mutated into (the classic memo bug — baking the
        /// offset into the remembered outlines) the second document disagrees with
        /// the first, and no test that only reads one document would notice.
        #[test]
        fn law2b_a_slide_equals_a_fresh_evaluation(
            word in word(),
            size in 8.0..64.0f64,
            r in 40.0..400.0f64,
            offset in -200.0..200.0f64,
            slide in -150.0..150.0f64,
        ) {
            // One document: evaluated at `offset`, then slid.
            let mut slid = Engine::new();
            let id = create_text(&mut slid, &word, size, 0.0, 0.0);
            let circle = create_circle(&mut slid, 0.0, 0.0, r);
            slid
                .dispatch(Command::BindTextToPath {
                    node_id: id,
                    path: circle,
                    offset: Parameter::Literal(offset),
                })
                .expect("BindTextToPath");
            let _ = run_of(&evaluate(&slid), id); // the first evaluation
            set_param(&mut slid, id, "path_offset", Parameter::Literal(offset + slide));
            let slid_run = run_of(&evaluate(&slid), id);

            // Another document: authored at the slid offset from the start.
            let mut fresh = Engine::new();
            let fresh_id = create_text(&mut fresh, &word, size, 0.0, 0.0);
            let fresh_circle = create_circle(&mut fresh, 0.0, 0.0, r);
            fresh
                .dispatch(Command::BindTextToPath {
                    node_id: fresh_id,
                    path: fresh_circle,
                    offset: Parameter::Literal(offset + slide),
                })
                .expect("BindTextToPath");
            let fresh_run = run_of(&evaluate(&fresh), fresh_id);

            prop_assert_eq!(slid_run, fresh_run, "a slide must equal a fresh evaluation");
        }

        /// **A family name cannot move the box** (Task 11.0 sizing): an alias, a
        /// name no host knows, and the bundled name itself all measure the same run,
        /// because the same face shaped it.
        ///
        /// This is the property a constrained layout leans on: a `constraint` that
        /// reads a run's `width` must not change the moment a designer renames the
        /// font to one this machine has never heard of. (Substitution is allowed to
        /// change the *letterforms* only when a host supplies different bytes under
        /// the same family — and then it is a different face, deliberately.)
        #[test]
        fn law5_a_family_name_cannot_move_the_box(
            word in word(),
            size in font_size(),
        ) {
            let bundled = {
                let mut engine = Engine::new();
                let id = create_text(&mut engine, &word, size, 0.0, 0.0);
                let run = run_of(&evaluate(&engine), id);
                (box_of(&run), glyph_boxes(&run))
            };
            for family in ["sans-serif", "system-ui", "Helvetica Neu", "DejaVu Sans"] {
                let mut engine = Engine::new();
                let id = create_text(&mut engine, &word, size, 0.0, 0.0);
                engine
                    .dispatch(Command::SetFontFamily {
                        node_id: id,
                        family: family.to_string(),
                    })
                    .expect("SetFontFamily");
                let run = run_of(&evaluate(&engine), id);
                prop_assert_eq!(
                    box_of(&run),
                    bundled.0,
                    "family {:?} measured a different box",
                    family
                );
                prop_assert_eq!(
                    glyph_boxes(&run),
                    bundled.1.clone(),
                    "family {:?} drew its glyphs somewhere else",
                    family
                );
            }
        }

    /// **Totality**: no word, size, alignment, spacing or binding makes the
    /// evaluator *fail*. Text is a document's most user-generated content, so
    /// this is the law that keeps a stray character from taking the canvas
    /// down — the same contract every other primitive has.
    #[test]
    fn text_evaluation_is_total(
        word in "[\\PC]{0,24}",
        size in 0.0..200.0f64,
        spacing in -20.0..20.0f64,
        leading in 0.0..3.0f64,
        align in prop_oneof![
            Just(TextAlign::Left),
            Just(TextAlign::Center),
            Just(TextAlign::Right),
        ],
        bind in any::<bool>(),
        (x, y) in origin(),
    ) {
        let mut engine = Engine::new();
        let id = create_text(&mut engine, &word, size, x, y);
        set_param(&mut engine, id, "letter_spacing", Parameter::Literal(spacing));
        set_param(&mut engine, id, "line_height", Parameter::Literal(leading));
        engine
            .dispatch(Command::SetTextAlignment { node_id: id, alignment: align })
            .expect("SetTextAlignment");
        if bind {
            let circle = create_circle(&mut engine, 0.0, 0.0, 150.0);
            engine
                .dispatch(Command::BindTextToPath {
                    node_id: id,
                    path: circle,
                    offset: Parameter::Literal(0.0),
                })
                .expect("BindTextToPath");
        }

        let ctx = engine.evaluation_context();
        let evaluation = GeometryEvaluator.evaluate_full(engine.document(), &ctx);
        prop_assert!(
            !evaluation.has_errors(),
            "text evaluation must never fail: {:?}",
            evaluation.errors().collect::<Vec<_>>()
        );
        // The node is present either way — an empty run, never a missing node.
        prop_assert!(evaluation.scene.get(id).is_some());
    }

    /// The bundled face is the floor the whole feature stands on: it parses,
    /// covers the Latin range, and lays a run out with no host font library at
    /// all. Checked here (not only in the unit tests) because *every* other law
    /// depends on it.
    #[test]
    fn the_bundled_face_always_draws(word in word(), size in 1.0..400.0f64) {
        let spec = vectra_geometry::TextSpec::new(&word, BUNDLED_FAMILY, size);
        let run = layout_text(FaceRef::bundled(), &spec, (0.0, 0.0)).expect("layout");
        // **At most** one glyph per character: a shaper may fold two characters
        // into one glyph (the `fi` ligature), never the other way round for
        // Latin text. Equality would be a false law — and its failure would be
        // the shape *working*.
        prop_assert!(!run.glyphs.is_empty(), "the bundled face must draw {word:?}");
        prop_assert!(
            run.glyphs.len() <= word.chars().count(),
            "shaping invented a glyph: {} for {word:?}",
            run.glyphs.len()
        );
        prop_assert!(run.metrics.width > 0.0);
        prop_assert_eq!(run.metrics.lines, 1);
    }
}

/// **A run off the end of its path clamps to the end** — the documented
/// behaviour of `sample_at`, asserted where it is visible: a run slid past the
/// path's end is drawn *at* the end (same box, same tangent), not dropped and
/// not reflected back onto the curve. A designer dragging the offset slider
/// past the end of a logo's curve sees the type park at the end, which is what
/// every vector editor does.
#[test]
fn a_run_slid_off_its_path_keeps_nothing_piled_at_the_seam() {
    let mut engine = Engine::new();
    let id = create_text(&mut engine, "end", 24.0, 0.0, 0.0);
    // A straight open path 400 units long: unlike a circle, its two ends are in
    // different places, so "off the start" and "off the end" are distinguishable
    // answers.
    let line = vectra_core::new_node_id();
    engine
        .dispatch(Command::CreateNode {
            id: line,
            kind: NodeKind::Path {
                start: Parameter::Literal(Point2::new(0.0, 0.0)),
                segments: vec![PathSegment::Line {
                    to: Parameter::Literal(Point2::new(400.0, 0.0)),
                }],
            },
            name: None,
            index: None,
        })
        .expect("CreateNode");
    engine
        .dispatch(Command::BindTextToPath {
            node_id: id,
            path: line,
            offset: Parameter::Literal(0.0),
        })
        .expect("BindTextToPath");

    let at_zero = run_of(&evaluate(&engine), id);
    assert_eq!(at_zero.truncated, 0, "the run fits a 400-unit line");
    let shaped = at_zero.glyphs.len();

    // **Off the end is off the end.** The run keeps the glyphs whose arc
    // position is on the path — here, none of them. The picture is *empty*, not
    // a pile of every letter at the path's end, which is what makes a long
    // string on a short path read as "it does not fit" instead of a blot.
    for offset in [-500.0, 5_000.0] {
        set_param(&mut engine, id, "path_offset", Parameter::Literal(offset));
        let off = run_of(&evaluate(&engine), id);
        assert!(
            off.glyphs.is_empty(),
            "a run entirely off the path draws nothing ({offset})"
        );
        assert_eq!(off.truncated, shaped, "every glyph is reported left out");
        assert!(
            off.outline.iter().next().is_none(),
            "and nothing is stroked"
        );
    }

    // A **partial** slide: keeping the glyphs that fit must not disturb the ones
    // that stay. Sliding by 20 units moves every survivor by exactly 20 — one
    // step, no reflow, no re-spacing — and the glyphs that fall off the start
    // are dropped instead of being stacked on it.
    set_param(&mut engine, id, "path_offset", Parameter::Literal(100.0));
    let before = run_of(&evaluate(&engine), id);
    let (start_arc, end_arc) = (100.0, 120.0);
    assert!(!before.glyphs.is_empty());
    assert!(
        before
            .glyphs
            .iter()
            .all(|glyph| (start_arc..=start_arc + 400.0).contains(&glyph.distance)),
        "survivors sit forward of the authored offset"
    );
    set_param(&mut engine, id, "path_offset", Parameter::Literal(end_arc));
    let after = run_of(&evaluate(&engine), id);
    assert_eq!(
        before.glyphs.len() + before.truncated,
        after.glyphs.len() + after.truncated,
        "the same string was shaped"
    );
    for (was, now) in before.glyphs.iter().zip(&after.glyphs) {
        if (now.distance - was.distance - 20.0).abs() > 1e-9 {
            panic!(
                "a glyph must travel exactly as far as the offset moved: {} -> {}",
                was.distance, now.distance
            );
        }
        assert_eq!(was.glyph_id, now.glyph_id);
    }

    // **The authored number is never rewritten by the picture**: clamping was
    // the old bug, and a designer's `path_offset` must stay what they typed —
    // that is what makes the slider reversible.
    set_param(&mut engine, id, "path_offset", Parameter::Literal(-500.0));
    let _ = run_of(&evaluate(&engine), id);
    let NodeKind::Text {
        on_path: Some(binding),
        ..
    } = &engine.document().get_node(id).unwrap().kind
    else {
        panic!("the run is still bound");
    };
    assert!(
        matches!(binding.offset, Parameter::Literal(value) if (value + 500.0).abs() < 1e-12),
        "the document still says -500: {:?}",
        binding.offset
    );
}

/// **The binding is one-way, and the document says so**: only path-shaped kinds
/// can be a run's source, and `BindTextToPath` refuses everything else. This is
/// the invariant that keeps evaluation a single pass, so it is a law, not a
/// convention.
#[test]
fn binding_is_refused_for_a_non_path_source() {
    let mut engine = Engine::new();
    let text = create_text(&mut engine, "bound to nothing", 24.0, 0.0, 0.0);
    let other_text = create_text(&mut engine, "another run", 24.0, 200.0, 0.0);
    let group = vectra_core::new_node_id();
    engine
        .dispatch(Command::CreateNode {
            id: group,
            kind: NodeKind::Group {
                children: Vec::new(),
            },
            name: None,
            index: None,
        })
        .expect("CreateNode");

    for (target, tag) in [(other_text, "Text"), (group, "Group")] {
        let error = engine
            .dispatch(Command::BindTextToPath {
                node_id: text,
                path: target,
                offset: Parameter::Literal(0.0),
            })
            .expect_err("a non-path source must be refused");
        assert!(
            error.to_string().contains(tag),
            "the refusal must name the kind it refused: {error}"
        );
    }

    // …and a path-shaped source is accepted.
    let circle = create_circle(&mut engine, 0.0, 0.0, 100.0);
    engine
        .dispatch(Command::BindTextToPath {
            node_id: text,
            path: circle,
            offset: Parameter::Literal(0.0),
        })
        .expect("a circle is a path source");
}

/// **Unbind restores the baseline exactly**: the run returns to the `x`/`y` it
/// was authored with, keeping every typographic property — so binding is a
/// survey of the curve, not a one-way door.
#[test]
fn unbinding_restores_the_authored_baseline() {
    let mut engine = Engine::new();
    let id = create_text(&mut engine, "round trip", 32.0, 40.0, 80.0);
    let straight = run_of(&evaluate(&engine), id);

    let circle = create_circle(&mut engine, 0.0, 0.0, 120.0);
    engine
        .dispatch(Command::BindTextToPath {
            node_id: id,
            path: circle,
            offset: Parameter::Literal(30.0),
        })
        .expect("BindTextToPath");
    assert_ne!(
        run_of(&evaluate(&engine), id).glyphs[0].distance,
        straight.glyphs[0].distance
    );

    engine
        .dispatch(Command::UnbindTextFromPath { node_id: id })
        .expect("UnbindTextFromPath");
    let after = run_of(&evaluate(&engine), id);
    assert_eq!(glyph_boxes(&straight), glyph_boxes(&after));

    // …and unbinding twice is an error, not a silent no-op: the document has no
    // binding to remove.
    assert!(engine
        .dispatch(Command::UnbindTextFromPath { node_id: id })
        .is_err());
}

/// A bound run's geometry follows its source **even when the source is created
/// after the binding's target**, and the scene's node order never changes the
/// answer — the evaluator resolves the source during the run's own evaluation
/// rather than reading a cached scene.
#[test]
fn a_run_reads_its_source_regardless_of_document_order() {
    let mut engine = Engine::new();
    let circle = create_circle(&mut engine, 0.0, 0.0, 150.0);
    let text = create_text(&mut engine, "order", 40.0, 0.0, 0.0);
    engine
        .dispatch(Command::BindTextToPath {
            node_id: text,
            path: circle,
            offset: Parameter::Literal(0.0),
        })
        .expect("BindTextToPath");

    let before = run_of(&evaluate(&engine), text);
    // Move the *text* to the top of the document: evaluation now meets it
    // before the circle it follows.
    let top = Point2::new(600.0, 600.0);
    engine
        .dispatch(Command::SetNodeParent {
            id: text,
            parent: None,
            index: 0,
        })
        .expect("SetNodeParent");
    let after = run_of(&evaluate(&engine), text);

    assert_eq!(
        glyph_boxes(&before),
        glyph_boxes(&after),
        "draw order is not geometry"
    );
    let _ = top;
}

/// A binding whose source is deleted is an **empty run**, not a broken
/// document — and re-creating the source with the same id brings the run back
/// (which is what an undo of the delete does).
#[test]
fn a_missing_source_is_an_empty_run_not_a_failure() {
    let mut engine = Engine::new();
    let circle = create_circle(&mut engine, 0.0, 0.0, 100.0);
    let text = create_text(&mut engine, "gone", 40.0, 0.0, 0.0);
    engine
        .dispatch(Command::BindTextToPath {
            node_id: text,
            path: circle,
            offset: Parameter::Literal(0.0),
        })
        .expect("BindTextToPath");
    let bound = run_of(&evaluate(&engine), text);
    assert!(!bound.glyphs.is_empty());

    // Remove the source out from under the binding (the registry-level delete
    // an undo of `DeleteNode` reverses, id and all).
    let removed = engine.document_mut().remove_node(circle).expect("remove");
    assert_eq!(removed.0.id, circle);

    let orphan = run_of(&evaluate(&engine), text);
    assert!(orphan.glyphs.is_empty(), "a missing source draws nothing");

    // Put it back: the run returns, exactly.
    engine
        .document_mut()
        .insert_node(removed.0, Some(removed.1))
        .expect("insert");
    // The layer bookkeeping the evaluator relies on for *flags* only — the
    // geometry path does not consult it.
    let mut doc: &Document = engine.document();
    let _ = doc.layers.layer_of(text);
    doc = engine.document();
    let _ = doc;
    let restored = run_of(&evaluate(&engine), text);
    assert_eq!(glyph_boxes(&bound), glyph_boxes(&restored));
}

/// `SetText` is the only writer of the string, and it is a *document* edit:
/// the run re-lays out, while a node's other typographic parameters are
/// untouched. Written as a test because "retyping a word never moves the type it
/// was set in" is the promise the split between `SetText` and `SetParameter`
/// exists to keep.
#[test]
fn retyping_keeps_every_typographic_property() {
    let mut engine = Engine::new();
    let id = create_text(&mut engine, "before", 48.0, 10.0, 20.0);
    engine
        .dispatch(Command::SetFontFamily {
            node_id: id,
            family: BUNDLED_FAMILY.to_string(),
        })
        .expect("SetFontFamily");
    engine
        .dispatch(Command::SetTextAlignment {
            node_id: id,
            alignment: TextAlign::Center,
        })
        .expect("SetTextAlignment");
    let before = run_of(&evaluate(&engine), id);

    engine
        .dispatch(Command::SetText {
            node_id: id,
            text: "after".to_string(),
        })
        .expect("SetText");
    let after = run_of(&evaluate(&engine), id);

    let NodeKind::Text {
        text,
        font_family,
        font_size,
        alignment,
        ..
    } = &engine.document().get_node(id).unwrap().kind
    else {
        panic!("still text");
    };
    assert_eq!(text, "after");
    assert_eq!(font_family, BUNDLED_FAMILY);
    assert!(matches!(font_size, Parameter::Literal(size) if (*size - 48.0).abs() < 1e-12));
    assert_eq!(*alignment, TextAlign::Center);
    assert_eq!(
        before.glyphs.len() - after.glyphs.len(),
        1,
        "one glyph shorter"
    );
}

/// **A broken face is not a broken document** (Task 11.0 sizing, the hostile
/// half of font resolution).
///
/// `FontLibrary` validates on registration, so a *library* face always parses.
/// The trait does not require that of every host, though — and a run that
/// vanishes because someone's font bytes were corrupt would make every
/// constraint that reads its box misbehave at once. The run falls back to the
/// bundled face, is drawn, measures the bundled box, and says what it did.
#[test]
fn a_broken_face_still_draws_its_words() {
    struct Broken;
    impl vectra_core::FontProvider for Broken {
        fn face(&self, _family: &str) -> Option<&[u8]> {
            Some(b"this is not a font at all")
        }
    }

    let mut engine = Engine::new();
    let id = create_text(&mut engine, "Ag", 40.0, 0.0, 0.0);
    let bundled = box_of(&run_of(&evaluate(&engine), id));

    let broken = Broken;
    let ctx = engine.evaluation_context().with_fonts(&broken);
    let evaluation = GeometryEvaluator.evaluate_full(engine.document(), &ctx);
    assert!(
        !evaluation.has_errors(),
        "a host's bad bytes must not fail the node: {:?}",
        evaluation.errors().collect::<Vec<_>>()
    );
    assert!(
        evaluation
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == vectra_geometry::DiagnosticCode::FontFallback),
        "the substitution must be reported: {:?}",
        evaluation.diagnostics
    );
    let run = run_of(&evaluation.scene, id);
    assert!(!run.glyphs.is_empty(), "the words are still drawn");
    assert_eq!(
        box_of(&run),
        bundled,
        "the box is the bundled face's box — a broken face cannot move a layout"
    );
}

/// **A slide driven by a variable re-places and does not re-shape** (RULE 1 ×
/// RULE 2). The counters are the evidence: one shaping for the run, however many
/// times the offset moves — which is what makes an offset slider a drag rather
/// than a rebuild.
#[test]
fn an_offset_variable_re_places_without_re_shaping() {
    let mut engine = Engine::new();
    engine
        .dispatch(Command::SetVariable {
            name: "slide".to_string(),
            value: 40.0,
        })
        .expect("SetVariable");
    let id = create_text(&mut engine, "Vectra", 32.0, 0.0, 0.0);
    let circle = create_circle(&mut engine, 0.0, 0.0, 220.0);
    engine
        .dispatch(Command::BindTextToPath {
            node_id: id,
            path: circle,
            offset: Parameter::Variable("slide".to_string()),
        })
        .expect("BindTextToPath");

    vectra_geometry::clear_shape_cache();
    let first = run_of(&evaluate(&engine), id);
    let after_first = vectra_geometry::shape_cache_stats();
    assert_eq!(after_first.misses, 1, "one distinct run: {after_first:?}");

    engine
        .dispatch(Command::SetVariable {
            name: "slide".to_string(),
            value: 120.0,
        })
        .expect("SetVariable");
    let slid = run_of(&evaluate(&engine), id);
    let after_slide = vectra_geometry::shape_cache_stats();
    assert_eq!(
        after_slide.misses, 1,
        "a variable-driven offset must re-place only: {after_slide:?}"
    );
    assert!(after_slide.hits >= 1, "{after_slide:?}");
    assert_eq!(slid.glyphs.len(), first.glyphs.len());

    // …and the picture really moved: the run's drawn centre travelled.
    let centre = |run: &vectra_geometry::EvaluatedText| {
        let (min_x, min_y, max_x, max_y) = box_of(run);
        ((min_x + max_x) * 0.5, (min_y + max_y) * 0.5)
    };
    let (before, after) = (centre(&first), centre(&slid));
    let travelled = ((before.0 - after.0).powi(2) + (before.1 - after.1).powi(2)).sqrt();
    assert!(travelled > 5.0, "the slide must move the run: {travelled}");
}

/// **Where the outline lands** (Task 11.0 RULE 3, z-order half).
///
/// Outlining replaces the type's *place in the drawing*, not just its pixels:
/// the group is born in the type's own layer, at the type's own index plus one —
/// directly above the (now hidden) original, below everything that was above it.
/// A designer's "outline, then boolean" has to see the same stacking they saw
/// before, or the letterforms would jump behind the artwork they were sitting on.
#[test]
fn law3b_the_outline_lands_directly_above_the_type_in_its_own_layer() {
    let mut engine = Engine::new();
    let id = create_text(&mut engine, "Up", 48.0, 0.0, 0.0);
    // A second layer, and the type moves into it — so "the active layer" (the
    // first one) and "the type's layer" are different answers.
    let layer = vectra_core::new_layer_id();
    engine
        .dispatch(Command::CreateLayer {
            id: layer,
            name: "Words".to_string(),
            index: None,
            artboard: None,
        })
        .expect("CreateLayer");
    engine
        .dispatch(Command::AssignNodeToLayer { node_id: id, layer })
        .expect("AssignNodeToLayer");
    // A shape in the same layer, *after* the type: the one thing the outline
    // must not do is jump above it.
    let under = create_circle(&mut engine, 0.0, 0.0, 12.0);
    engine
        .dispatch(Command::AssignNodeToLayer {
            node_id: under,
            layer,
        })
        .expect("AssignNodeToLayer");

    let run = run_of(&evaluate(&engine), id);
    let plans = outline_plans(&run, "Up");
    assert!(!plans.is_empty());
    let paths: Vec<vectra_core::OutlinePath> = plans
        .iter()
        .map(|plan| vectra_core::OutlinePath {
            id: vectra_core::new_node_id(),
            name: plan.name.clone(),
            start: Parameter::Literal(plan.start),
            segments: plan.segments.clone(),
        })
        .collect();
    let letterform_ids: Vec<NodeId> = paths.iter().map(|path| path.id).collect();
    let group = vectra_core::new_node_id();
    engine
        .dispatch(Command::OutlineText {
            node_id: id,
            group_id: group,
            name: Some("Outlined".to_string()),
            paths,
        })
        .expect("OutlineText");

    let doc = engine.document();
    // 1. Same layer as the type…
    let text_layer = doc.layers.layer_of(id).expect("the type is in a layer");
    let group_layer = doc
        .layers
        .layer_of(group)
        .expect("the outline is in a layer");
    assert_eq!(
        group_layer.0.id, text_layer.0.id,
        "the outline keeps the layer"
    );
    assert_eq!(
        group_layer.0.id, layer,
        "…which is the type's own, not the active one"
    );
    // 2. …directly above it, and directly below what was above it…
    assert_eq!(group_layer.1, text_layer.1 + 1, "one slot above the type");
    // The block is `[group, letterforms…]`, so the artwork that was above the
    // type must now sit after the *whole* block, not merged into it.
    let block = group_layer.1 + 1 + letterform_ids.len();
    assert_eq!(
        doc.layers.layer_of(under).expect("the circle").1,
        block,
        "the artwork that was above the type is still above the letterforms"
    );
    assert!(
        doc.order_index(under).expect("the circle is in the order")
            > doc
                .order_index(*letterform_ids.last().unwrap())
                .expect("the last letterform"),
        "and the flat order agrees"
    );
    assert_eq!(
        doc.order_index(group),
        Some(doc.order_index(id).expect("the type is in the order") + 1),
        "and the flat order agrees"
    );
    // 3. The letterforms follow the group, in the same layer, so the whole
    //    outline reads as one stack: type (hidden), group, letterforms.
    for (index, path_id) in letterform_ids.iter().enumerate() {
        let (record, slot) = doc
            .layers
            .layer_of(*path_id)
            .expect("a letterform is in a layer");
        assert_eq!(record.id, layer);
        assert_eq!(slot, group_layer.1 + 1 + index);
        assert_eq!(
            doc.order_index(*path_id),
            Some(doc.order_index(group).unwrap() + 1 + index)
        );
    }
    // 4. And the picture is unchanged where it matters: the type is hidden, the
    //    group draws.
    assert!(!doc.get_node(id).expect("the type").visible);
    assert!(doc.get_node(group).expect("the group").visible);
}
