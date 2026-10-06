//! Task 10.3 RULE 2 — **navigation laws** for the camera.
//!
//! Pan and zoom are the one place where a designer's hand enters the engine
//! before any geometry does: a pointer pixel becomes a document coordinate
//! because a camera says so. If the camera and the pointer disagree by even a
//! pixel, every drag lands somewhere the user did not click, and no amount of
//! correctness downstream can repair it.
//!
//! So these laws are equalities, not transcriptions of UI behaviour:
//!
//! 1. **Round trip** — `screen_to_document` is the exact inverse of
//!    `document_to_screen`, at every screen point, in every camera.
//! 2. **Zoom anchor** — the document point under the pointer stays under the
//!    pointer, which is the definition of zooming about a point.
//! 3. **Pan law** — panning translates the visible rectangle and nothing else;
//!    pan by `a` then `b` is pan by `a + b`.
//! 4. **Clamp** — the scale limits are exact and a wheel already at a limit is
//!    a no-op, not a slow drift.
//! 5. **Totality** — a degenerate number never panics and never changes the
//!    camera (the UI passes raw pointer data straight through).
//! 6. **Framing** — `framing`/`fit` keep the meaning Task 5.0 gave them:
//!    a canvas pixel covers exactly one document unit at 100%.

use vectra_render::instance::Camera;

const W: f32 = 800.0;
const H: f32 = 600.0;
const SCREEN: [f32; 2] = [1280.0, 720.0];

/// The cameras a designer can actually reach: the two framings, a panned one,
/// a zoomed one, a pan+zoom one, and one at each clamp limit.
fn gallery() -> Vec<(&'static str, Camera)> {
    let fit = Camera::fit(W, H, SCREEN);
    vec![
        ("fit", fit),
        ("framing", Camera::framing(W, H)),
        ("panned", fit.pan_by(-120.0, 64.0)),
        ("zoomed", fit.zoom_at(400.0, 300.0, 2.5)),
        (
            "pan+zoom",
            fit.pan_by(90.0, -40.0).zoom_at(200.0, 500.0, 0.4),
        ),
        ("min clamp", fit.zoom_at(0.0, 0.0, 1e-9)),
        ("max clamp", fit.zoom_at(640.0, 360.0, 1e9)),
    ]
}

fn close(a: f32, b: f32, what: &str) {
    assert!(
        (a - b).abs() <= 1e-3,
        "{what}: {a} != {b} (delta {})",
        (a - b).abs()
    );
}

/// The real acceptance criterion, in the units the designer judges: two document
/// coordinates are the same point when they land on the same **pixel**. Stated
/// against the camera's own scale so the tolerance means the same thing at 1%
/// zoom and at 6400%.
fn same_point(camera: &Camera, a: f32, b: f32, what: &str) {
    let pixels = (a - b).abs() * camera.scale();
    assert!(pixels <= 1e-3, "{what}: {a} != {b} ({} px apart)", pixels);
}

fn same(left: &Camera, right: &Camera) -> bool {
    left.to_bytes() == right.to_bytes()
}

/// Two cameras show the same picture when every probe lands on the same pixel.
///
/// Deliberately *not* byte equality: `pan_by(a).pan_by(b)` and `pan_by(a + b)`
/// are different f32 arithmetic that agree only to within rounding, and a law
/// demanding bit-identical floats would be pinning the compiler's rounding
/// rather than the camera's meaning. Sub-pixel is the promise the user can check.
fn same_view(left: &Camera, right: &Camera, what: &str) {
    assert!(
        (left.scale() - right.scale()).abs() <= f32::EPSILON,
        "{what}: scales differ: {} vs {}",
        left.scale(),
        right.scale()
    );
    for (px, py) in probes() {
        let (ax, ay) = left.screen_to_document(px, py);
        let (bx, by) = right.screen_to_document(px, py);
        same_point(left, ax, bx, &format!("{what}: x at ({px}, {py})"));
        same_point(left, ay, by, &format!("{what}: y at ({px}, {py})"));
    }
}

/// Every screen corner and the centre: the points a gesture actually arrives at.
fn probes() -> Vec<(f32, f32)> {
    vec![
        (0.0, 0.0),
        (SCREEN[0], 0.0),
        (0.0, SCREEN[1]),
        (SCREEN[0], SCREEN[1]),
        (SCREEN[0] * 0.5, SCREEN[1] * 0.5),
        (37.0, 511.0),
    ]
}

#[test]
fn law_1_screen_and_document_are_exact_inverses() {
    for (name, camera) in gallery() {
        for (px, py) in probes() {
            let (dx, dy) = camera.screen_to_document(px, py);
            let (bx, by) = camera.document_to_screen(dx, dy);
            close(
                bx,
                px,
                &format!("{name}: screen round trip x at ({px}, {py})"),
            );
            close(
                by,
                py,
                &format!("{name}: screen round trip y at ({px}, {py})"),
            );

            // …and the other way round, from document space.
            let (sx, sy) = camera.document_to_screen(dx, dy);
            let (rx, ry) = camera.screen_to_document(sx, sy);
            same_point(
                &camera,
                rx,
                dx,
                &format!("{name}: document round trip x at ({px}, {py})"),
            );
            same_point(
                &camera,
                ry,
                dy,
                &format!("{name}: document round trip y at ({px}, {py})"),
            );
        }
    }
}

#[test]
fn law_2_zoom_keeps_the_point_under_the_pointer_still() {
    for (name, camera) in gallery() {
        for factor in [1.25_f32, 0.8, 4.0, 0.05] {
            for (px, py) in probes() {
                let (before_x, before_y) = camera.screen_to_document(px, py);
                let zoomed = camera.zoom_at(px, py, factor);
                let (after_x, after_y) = zoomed.screen_to_document(px, py);
                same_point(
                    &camera,
                    before_x,
                    after_x,
                    &format!("{name}: anchored zoom x (factor {factor})"),
                );
                same_point(
                    &camera,
                    before_y,
                    after_y,
                    &format!("{name}: anchored zoom y (factor {factor})"),
                );

                // The zoom that happened is the factor asked for, unless that
                // would leave the clamp range — a wheel is not a merge of two
                // numbers that can drift apart from the readout.
                let asked = (camera.scale() * factor).clamp(Camera::MIN_SCALE, Camera::MAX_SCALE);
                close(zoomed.scale(), asked, &format!("{name}: applied scale"));
            }
        }
    }
}

#[test]
fn law_3_pan_translates_and_composes() {
    for (name, camera) in gallery() {
        let (x, y, w, h) = camera.visible();
        // The pointer's own delta: 120 px to the left, 64 px down.
        let (dx, dy) = (-120.0_f32, 64.0_f32);
        let moved = camera.pan_by(dx, dy);
        let (mx, my, mw, mh) = moved.visible();
        let scale = camera.scale();

        // A pan moves the window by the pointer delta *in document units*, with
        // the y-flip applied exactly once (the pointer's +y is down, the
        // document's +y is up): dragging the artwork left/below means looking
        // further right/above, so `min_x` rises while `min_y` rises too.
        close(mx, x - dx / scale, &format!("{name}: pan x"));
        close(my, y + dy / scale, &format!("{name}: pan y"));
        close(mw, w, &format!("{name}: pan must not resize w"));
        close(mh, h, &format!("{name}: pan must not resize h"));
        close(moved.scale(), scale, &format!("{name}: pan must not zoom"));

        // Composition: two drags are one drag of the summed delta.
        let composed = camera.pan_by(11.0, -7.0).pan_by(4.0, 3.0);
        let summed = camera.pan_by(15.0, -4.0);
        same_view(
            &composed,
            &summed,
            &format!("{name}: pan_by(a).pan_by(b) must equal pan_by(a + b)"),
        );
    }
}

#[test]
fn law_4_the_clamp_is_exact_and_at_the_limit_zoom_is_a_no_op() {
    let camera = Camera::fit(W, H, SCREEN);

    let too_far_out = camera.zoom_at(10.0, 10.0, 1e-9);
    assert_eq!(
        too_far_out.scale(),
        Camera::MIN_SCALE,
        "an extreme wheel-out lands exactly on the minimum scale"
    );
    let too_far_in = camera.zoom_at(10.0, 10.0, 1e9);
    assert_eq!(
        too_far_in.scale(),
        Camera::MAX_SCALE,
        "an extreme wheel-in lands exactly on the maximum scale"
    );

    // Sitting on a limit, another notch in the same direction changes nothing
    // at all — else the readout would creep while the drawing stands still.
    assert!(
        same(&too_far_out.zoom_at(10.0, 10.0, 1e-9), &too_far_out),
        "a wheel-out at the minimum scale is a no-op"
    );
    assert!(
        same(&too_far_in.zoom_at(10.0, 10.0, 1e9), &too_far_in),
        "a wheel-in at the maximum scale is a no-op"
    );

    // And the 100% button really is 100%.
    let one = camera.zoom_to(1.0);
    assert_eq!(one.scale(), 1.0, "zoom_to(1.0) is exactly 100%");
    close(
        one.visible().2,
        SCREEN[0],
        "at 100% the window width is the target width in pixels",
    );
}

#[test]
fn law_5_degenerate_numbers_never_move_the_camera_or_panic() {
    for (name, camera) in gallery() {
        // A pointer position and a pan delta are only nonsense when they are not
        // numbers: 0 and negative values are perfectly ordinary pixels.
        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            assert!(
                same(&camera.zoom_at(bad, 10.0, 2.0), &camera),
                "{name}: zoom_at must ignore pointer x {bad}"
            );
            assert!(
                same(&camera.zoom_at(10.0, bad, 2.0), &camera),
                "{name}: zoom_at must ignore pointer y {bad}"
            );
            assert!(
                same(&camera.pan_by(bad, 1.0), &camera),
                "{name}: pan_by must ignore delta x {bad}"
            );
            assert!(
                same(&camera.pan_by(1.0, bad), &camera),
                "{name}: pan_by must ignore delta y {bad}"
            );
        }
        // A zoom factor and an absolute scale must also be positive and finite:
        // a negative factor is a mirror, which no camera can be.
        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 0.0, -1.0] {
            assert!(
                same(&camera.zoom_at(10.0, 10.0, bad), &camera),
                "{name}: zoom_at must ignore factor {bad}"
            );
            assert!(
                same(&camera.zoom_to(bad), &camera),
                "{name}: zoom_to must ignore scale {bad}"
            );
        }
        // Panning by nothing, and the identity zoom, are literally no-ops.
        assert!(same(&camera.pan_by(0.0, 0.0), &camera), "{name}: pan(0)");
        assert!(
            same(&camera.zoom_at(10.0, 10.0, 1.0), &camera),
            "{name}: zoom(1.0) is the identity"
        );
    }
}

#[test]
fn law_6_framing_still_means_one_pixel_per_unit() {
    // Task 5.0's contract, restated as a navigation law: the default window is
    // the document rectangle [0, 0, 800, 600] and a canvas pixel is a document
    // unit, so the pixel under (400, 300) is the document's centre.
    let camera = Camera::framing(W, H);
    assert_eq!(camera.scale(), 1.0, "framing is 100%");
    let (x, y, w, h) = camera.visible();
    assert_eq!(
        (x, y, w, h),
        (0.0, 0.0, W, H),
        "framing shows exactly [0, 0, w, h], with the height reported positive"
    );
    close(
        camera.screen_to_document(W * 0.5, H * 0.5).0,
        W * 0.5,
        "framing: screen centre is document centre (x)",
    );
    close(
        camera.screen_to_document(W * 0.5, H * 0.5).1,
        H * 0.5,
        "framing: screen centre is document centre (y)",
    );

    // `fit` contains the document and centres it (object-fit: contain), so no
    // pan and no zoom is ever *needed* to see a new document.
    let fit = Camera::fit(W, H, SCREEN);
    let (fx, fy, fw, fh) = fit.visible();
    assert!(fx <= 0.0 && fy <= 0.0, "fit must not crop the top-left");
    assert!(
        fx + fw >= W && fy + fh >= H,
        "fit must not crop the bottom-right"
    );
    same_point(&fit, fx + fw / 2.0, W / 2.0, "fit centres horizontally");
    same_point(&fit, fy + fh / 2.0, H / 2.0, "fit centres vertically");
}

#[test]
fn law_7_a_zoom_then_its_inverse_returns_the_same_view() {
    // Round-tripping a gesture is the safety net under "zoom out, look, zoom
    // back": the view the designer left must be the view they get back.
    //
    // Stated for gestures that stay inside the clamp range, because that is
    // where "inverse" is defined: a camera already at 6400% *cannot* zoom 3x in,
    // and law 4 pins what happens instead (the gesture is swallowed whole, not
    // partially, so the readout and the drawing never disagree).
    for (name, camera) in gallery() {
        if camera.scale() * 3.0 > Camera::MAX_SCALE || camera.scale() / 3.0 < Camera::MIN_SCALE {
            continue;
        }
        let zoomed = camera.zoom_at(333.0, 210.0, 3.0);
        let back = zoomed.zoom_at(333.0, 210.0, 1.0 / 3.0);
        for (px, py) in probes() {
            let (ax, ay) = camera.screen_to_document(px, py);
            let (bx, by) = back.screen_to_document(px, py);
            same_point(
                &camera,
                ax,
                bx,
                &format!("{name}: zoom 3x then 1/3 x at ({px}, {py})"),
            );
            same_point(
                &camera,
                ay,
                by,
                &format!("{name}: zoom 3x then 1/3 y at ({px}, {py})"),
            );
        }
    }
}
