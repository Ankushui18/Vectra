//! Task 10.3 RULE 2 — **navigation laws at the WASM boundary**: pan and zoom
//! move the *picture*, and the pointer's map to the document moves with it.
//!
//! `vectra-render`'s `camera_laws` prove the arithmetic on `Camera` alone. These
//! laws prove the composition that a designer actually touches:
//!
//! ```text
//! wheel(px, py, f) ──▶ nav_zoom ──▶ camera ──▶ projection
//!                                  └──▶ client_to_document ──▶ hit_test ──▶ drag
//! drag(dx, dy)     ──▶ nav_pan  ──▶ camera ──┘
//! resize           ──▶ view_state ──▶ camera ─┘
//! ```
//!
//! One camera, three writers, and four readers that must never disagree: the
//! shader's projection, `client_to_document` (the pointer), `document_to_client`
//! (the overlay) and `hit_test` (selection). The laws below are each one of those
//! agreements:
//!
//! | law | test |
//! |---|---|
//! | Wheel Anchor | [`law_a_wheel_keeps_the_point_under_the_pointer`] |
//! | Pan Follows the Pointer | [`law_panning_moves_the_picture_with_the_pointer`] |
//! | Readout | [`law_the_zoom_readout_is_css_pixels_per_unit`] |
//! | Selection Follows the Camera | [`law_a_gesture_never_moves_the_shape_away_from_the_click`] |
//! | Resize Discipline | [`law_a_gesture_survives_a_resize_and_a_jump_still_fits`] |
//! | Clamp | [`law_the_clamp_holds_through_the_port`] |
//! | Unmeasured Canvas | [`law_an_unmeasured_canvas_ignores_gestures`] |

use vectra_core::new_node_id;
use vectra_wasm::{Renderer, VectraEngine};

// ── The wire, exactly as React uses it ─────────────────────────────────────

fn dispatch(engine: &mut VectraEngine, cmd: serde_json::Value) -> serde_json::Value {
    serde_json::from_str(&engine.dispatch_command(&cmd.to_string())).expect("parsed response")
}

fn create_rect(engine: &mut VectraEngine, name: &str, x: f64, y: f64, w: f64, h: f64) -> String {
    let node = new_node_id().to_string();
    let response = dispatch(
        engine,
        serde_json::json!({
            "type": "CreateNode",
            "id": node,
            "name": name,
            "kind": { "Rectangle": {
                "x": { "Literal": x },
                "y": { "Literal": y },
                "width": { "Literal": w },
                "height": { "Literal": h },
                "corner_radius": { "Literal": 0.0 },
            }},
        }),
    );
    assert_eq!(response["status"], "ok", "create {name}: {response}");
    node
}

/// A renderer with a measured canvas box: 800×600 CSS pixels at the given
/// device-pixel ratio — the two numbers `viewport` records from the DOM.
fn renderer_at(css_w: f64, css_h: f64, dpr: f64) -> Renderer {
    let mut renderer = Renderer::new();
    renderer.viewport(0.0, 0.0, css_w, css_h, dpr);
    renderer
}

fn doc(renderer: &Renderer, client_x: f64, client_y: f64) -> (f64, f64) {
    renderer
        .client_to_document(client_x, client_y)
        .unwrap_or_else(|| panic!("no document point for ({client_x}, {client_y})"))
}

/// Where a document point is drawn, in **client** pixels (canvas at the origin).
fn client(renderer: &Renderer, x: f64, y: f64) -> (f64, f64) {
    let json: serde_json::Value =
        serde_json::from_str(&renderer.document_to_client(&format!("[[{x},{y}]]")))
            .expect("parsed document_to_client");
    assert_eq!(json["ok"], serde_json::Value::Bool(true), "{json}");
    let point = &json["points"][0];
    (point[0].as_f64().expect("x"), point[1].as_f64().expect("y"))
}

fn view(renderer: &Renderer) -> serde_json::Value {
    serde_json::from_str(&renderer.view()).expect("parsed view")
}

fn number(value: &serde_json::Value, key: &str) -> f64 {
    value[key]
        .as_f64()
        .unwrap_or_else(|| panic!("{key}: {value}"))
}

fn close(a: f64, b: f64, what: &str) {
    assert!((a - b).abs() <= 1e-3, "{what}: {a} != {b}");
}

fn center_of(view: &serde_json::Value) -> (f64, f64) {
    (
        number(view, "x") + number(view, "w") / 2.0,
        number(view, "y") + number(view, "h") / 2.0,
    )
}

// ── Laws ──────────────────────────────────────────────────────────────────

/// **Wheel Anchor.** Zooming with the wheel must leave the document point under
/// the pointer exactly where it is — on the screen *and* in the pointer map.
///
/// This is the law that makes zooming feel physical: a designer points at the
/// corner they are refining and rolls the wheel. If the anchor slips, every
/// subsequent click lands somewhere else, which is the classic "why did my drag
/// grab the wrong handle" bug.
#[test]
fn law_a_wheel_keeps_the_point_under_the_pointer() {
    for dpr in [1.0, 2.0] {
        let mut renderer = renderer_at(800.0, 600.0, dpr);
        // A pointer position that is not the centre and not the origin, so a
        // sign error or an origin assumption cannot pass by luck.
        let (px, py) = (610.0, 145.0);

        let before = doc(&renderer, px, py);
        for factor in [1.2, 1.2, 1.2, 0.5, 0.8] {
            renderer.nav_zoom(px, py, factor);
            let after = doc(&renderer, px, py);
            close(
                after.0,
                before.0,
                &format!("dpr {dpr}: anchor x after x{factor}"),
            );
            close(
                after.1,
                before.1,
                &format!("dpr {dpr}: anchor y after x{factor}"),
            );
        }

        // The same invariance, read from the other side: the anchor's client
        // position is unchanged (the overlay draws handles there).
        let (cx, cy) = client(&renderer, before.0, before.1);
        close(cx, px, &format!("dpr {dpr}: overlay anchor x"));
        close(cy, py, &format!("dpr {dpr}: overlay anchor y"));
    }
}

/// **Pan Follows the Pointer.** Dragging moves the picture by exactly the drag:
/// the document point that was under the pointer is now under the pointer plus
/// the delta, in client pixels, for accumulated small deltas as well as one big
/// one (a real drag arrives as dozens of events).
#[test]
fn law_panning_moves_the_picture_with_the_pointer() {
    let mut renderer = renderer_at(800.0, 600.0, 2.0);
    let (px, py) = (400.0, 300.0);
    let anchor = doc(&renderer, px, py);

    // A drag broken into 20 steps, the way a pointermove stream arrives.
    let (total_x, total_y) = (-37.5, 82.25);
    for step in 0..20 {
        let dx = total_x / 20.0;
        let dy = total_y / 20.0;
        renderer.nav_pan(dx, dy);
        let _ = step;
    }

    let (cx, cy) = client(&renderer, anchor.0, anchor.1);
    close(
        cx,
        px + total_x,
        "the grabbed point followed the pointer (x)",
    );
    close(
        cy,
        py + total_y,
        "the grabbed point followed the pointer (y)",
    );

    // And the zoom did not drift while dragging.
    close(
        renderer.nav_scale(),
        1.0,
        "panning must not change the zoom",
    );
}

/// **Readout.** The zoom the UI prints as a percentage is CSS pixels per
/// document unit, and at 100% one CSS pixel of pointer movement is exactly one
/// document unit — the property every "draw a 100 mm line" gesture relies on.
#[test]
fn law_the_zoom_readout_is_css_pixels_per_unit() {
    for dpr in [1.0, 1.5, 2.0] {
        let mut renderer = renderer_at(800.0, 600.0, dpr);
        renderer.nav_zoom_to(1.0);
        close(
            renderer.nav_scale(),
            1.0,
            &format!("dpr {dpr}: 100% reads 100%"),
        );

        let (ax, ay) = doc(&renderer, 300.0, 300.0);
        let (bx, by) = doc(&renderer, 301.0, 301.0);
        close(
            bx - ax,
            1.0,
            &format!("dpr {dpr}: one client pixel right is one unit +x"),
        );
        // Client y grows downward, document y grows upward: one pixel *down* is
        // one unit *less*. Pinned here because it is the flip every tool leans
        // on, and the place a future "let's just use screen coordinates" change
        // would break every drag.
        close(
            by - ay,
            -1.0,
            &format!("dpr {dpr}: one client pixel down is one unit -y"),
        );

        // The tools' inverse: document units per client pixel.
        let per_unit = renderer.pixels_per_unit().expect("a measured canvas");
        close(
            per_unit,
            1.0 / renderer.nav_scale(),
            &format!("dpr {dpr}: pixels_per_unit is the inverse of the readout"),
        );

        // A preset from the menu is absolute, not relative: asking twice is
        // asking once.
        renderer.nav_zoom_to(4.0);
        let first = renderer.nav_scale();
        renderer.nav_zoom_to(4.0);
        close(first, 4.0, &format!("dpr {dpr}: nav_zoom_to(4) is 400%"));
        close(renderer.nav_scale(), 4.0, &format!("dpr {dpr}: still 400%"));
    }
}

/// **Selection Follows the Camera.** The real payoff of the whole feature: after
/// a pan and a zoom, hitting the pixel where a shape is *drawn* selects that
/// shape. Nothing else in the app may re-derive the projection, and this is the
/// law that notices if anything ever does.
#[test]
fn law_a_gesture_never_moves_the_shape_away_from_the_click() {
    let mut engine = VectraEngine::new();
    let rect = create_rect(&mut engine, "Rig", 120.0, 90.0, 160.0, 120.0);
    let mut renderer = renderer_at(800.0, 600.0, 2.0);
    let _ = engine.render_frame(&mut renderer);

    // A point inside the rectangle, in document units (the document's y is up,
    // the rectangle spans y 90..210).
    let inside = (200.0, 150.0);

    let check = |renderer: &mut Renderer, when: &str| {
        let (cx, cy) = client(renderer, inside.0, inside.1);
        let hit = renderer.pointer_hit(cx, cy);
        assert_eq!(
            hit.as_deref(),
            Some(rect.as_str()),
            "{when}: the click at ({cx}, {cy}) must select the rectangle"
        );
    };

    check(&mut renderer, "opening view");

    // Zoom in on the shape's top-left corner, then drag the canvas around.
    renderer.nav_zoom(120.0, 120.0, 3.0);
    check(&mut renderer, "after zooming in");
    renderer.nav_pan(-64.0, 41.0);
    check(&mut renderer, "after panning");
    renderer.nav_zoom_to(0.75);
    check(&mut renderer, "after a preset zoom");

    // A click far outside the shape must still miss — a camera that maps
    // everything onto everything would pass the checks above and fail this one.
    let (far_x, far_y) = client(&renderer, 6000.0, 6000.0);
    assert_eq!(
        renderer.pointer_hit(far_x, far_y),
        None,
        "a click far outside the artwork hits nothing"
    );
}

/// **Resize Discipline.** A gesture's camera keeps its zoom and its centre when
/// the canvas changes size; a *jump*'s camera keeps fitting its rectangle. The
/// first is what an editor does when you resize a window; the second is what
/// "show me this artboard" has to keep meaning.
#[test]
fn law_a_gesture_survives_a_resize_and_a_jump_still_fits() {
    let mut renderer = renderer_at(800.0, 600.0, 2.0);
    renderer.nav_zoom(400.0, 300.0, 2.5);
    renderer.nav_pan(30.0, -18.0);

    let before = view(&renderer);
    let zoom_before = renderer.nav_scale();
    let center_before = center_of(&before);
    close(zoom_before, 2.5, "the gesture's zoom");

    // The user resizes the window (and, in the second resize, changes display):
    // same camera, more canvas.
    renderer.viewport(0.0, 0.0, 1200.0, 900.0, 2.0);
    let after = view(&renderer);
    close(renderer.nav_scale(), zoom_before, "a resize keeps the zoom");
    close(
        center_of(&after).0,
        center_before.0,
        "a resize keeps the centre (x)",
    );
    close(
        center_of(&after).1,
        center_before.1,
        "a resize keeps the centre (y)",
    );
    assert!(
        number(&after, "w") > number(&before, "w"),
        "a bigger canvas shows more document, not the same amount scaled up: {before} → {after}"
    );

    // A device-pixel-ratio change (laptop → monitor) is not a zoom change
    // either: 250% stays 250%, and one client pixel stays one 1/2.5 document
    // units.
    renderer.viewport(0.0, 0.0, 1200.0, 900.0, 1.0);
    close(
        renderer.nav_scale(),
        zoom_before,
        "a dpr change keeps the zoom",
    );

    // A jump, by contrast, re-fits its rectangle: the artboard is fully visible
    // and centred after any resize, because that is what the dropdown promised.
    let (ax, ay, aw, ah) = (150.0, 40.0, 320.0, 200.0);
    renderer.frame_document(ax, ay, aw, ah);
    renderer.viewport(0.0, 0.0, 800.0, 600.0, 2.0);
    let jumped = view(&renderer);
    let (x, y, w, h) = (
        number(&jumped, "x"),
        number(&jumped, "y"),
        number(&jumped, "w"),
        number(&jumped, "h"),
    );
    assert!(
        x <= ax + 1e-3 && y <= ay + 1e-3 && x + w >= ax + aw - 1e-3 && y + h >= ay + ah - 1e-3,
        "the jumped-to artboard stays visible through a resize: {jumped}"
    );
    close(x + w / 2.0, ax + aw / 2.0, "the jump stays centred (x)");
    close(y + h / 2.0, ay + ah / 2.0, "the jump stays centred (y)");
}

/// **Clamp.** The zoom limits are enforced by the camera and reported honestly
/// through the port — and a gesture that cannot change anything changes nothing
/// at all, so a designer holding the wheel never finds the view creeping.
#[test]
fn law_the_clamp_holds_through_the_port() {
    for (dpr, min_css, max_css) in [
        (1.0, 0.01, 64.0),
        (2.0, 0.005, 32.0), // the same device limits, read in CSS pixels
    ] {
        let mut renderer = renderer_at(800.0, 600.0, dpr);
        renderer.nav_zoom_to(0.000001);
        close(
            renderer.nav_scale(),
            min_css,
            &format!("dpr {dpr}: minimum zoom"),
        );
        let bottom = view(&renderer);
        renderer.nav_zoom(400.0, 300.0, 0.5);
        assert_eq!(
            view(&renderer)["w"],
            bottom["w"],
            "dpr {dpr}: a wheel-out at the limit is swallowed whole"
        );

        renderer.nav_zoom_to(1e9);
        close(
            renderer.nav_scale(),
            max_css,
            &format!("dpr {dpr}: maximum zoom"),
        );
        let top = view(&renderer);
        renderer.nav_zoom(400.0, 300.0, 2.0);
        assert_eq!(
            view(&renderer)["w"],
            top["w"],
            "dpr {dpr}: a wheel-in at the limit is swallowed whole"
        );
    }
}

/// **Unmeasured Canvas.** Before the DOM has told the renderer where the canvas
/// is, a gesture is ignored and the pointer map answers `None` — the UI shows a
/// stable view instead of inventing one.
#[test]
fn law_an_unmeasured_canvas_ignores_gestures() {
    let mut renderer = Renderer::new();
    let before = view(&renderer);
    renderer.nav_zoom(10.0, 10.0, 2.0);
    renderer.nav_pan(50.0, 50.0);
    renderer.nav_zoom_to(8.0);
    assert_eq!(
        view(&renderer),
        before,
        "a canvas with no box has no camera to move"
    );
    assert_eq!(
        renderer.pointer_doc(10.0, 10.0),
        "null",
        "the pointer map reports honestly instead of guessing"
    );
    assert_eq!(
        renderer.client_to_document(10.0, 10.0),
        None,
        "and the typed entry point agrees with the JSON one"
    );
}
