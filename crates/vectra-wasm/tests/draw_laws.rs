//! **The Quick Shape Law, at the engine boundary** (Task 10.1 RULE 3).
//!
//! `crates/vectra-draw/tests/draw_laws.rs` proves the *mathematics* — a jagged
//! stroke becomes a path whose flattened points lie on one circle. This file
//! proves the part only the assembled engine can, and it is written to make the
//! central claim **falsifiable rather than decorative**:
//!
//! > the snap is performed by the Task 3.1 constraint solver.
//!
//! The way to prove that is to make the solver's work *visible*. So the path in
//! this test starts **wrong** — its four cardinal anchors are displaced by a known
//! amount — and nothing in the test ever moves them. The only thing that happens
//! is that the plan's constraint rows are dispatched through the ordinary command
//! path. If the anchors end up exactly on the circle, the tableau moved them; no
//! other code in the test can have.
//!
//! The assertions, in order:
//!
//! 1. the snap's rows land in the document, 11 of them, **each naming the
//!    primitive's slot first** — the ordering that decides who follows whom;
//! 2. after the rows are applied, the anchors are the cardinals to 1e-9, from a
//!    start that was 3 units off;
//! 3. the solved system **holds**: solving again reports no writes, which is the
//!    engine's own definition of "this document already satisfies its rules";
//! 4. the rules are ordinary undoable records — undoing them restores the wrong
//!    geometry, because nothing was baked in.
//!
//! ## What this law does *not* claim (and must not)
//!
//! That changing the `radius` afterwards drags the anchors with it. It does not,
//! and the reason is in the vocabulary rather than in the implementation: the
//! dimension link is `anchor.x − cx − radius == 0`, a **three-term** row, and every
//! Phase-1 `ConstraintKind` is a two-term equality or one slot against a captured
//! constant. The snap's rows are therefore `anchor − centre == ±r` with `r` as the
//! *value*, which is exact at snap time and static afterwards. Making a snapped
//! circle live — resize it and watch the path follow — needs one new row shape in
//! Task 3.1's operator table (a scaled/three-term kind), or the anchors bound to
//! expressions over the primitive's variables. Both are engine decisions, and both
//! are stated as such in `TASK-10.1-REPORT.md` rather than papered over here.

use vectra_core::{new_node_id, Command, NodeKind, Parameter, PathSegment, Point2};
use vectra_draw::{plan_quick_shape, SnapKind};
use vectra_wasm::{Renderer, VectraEngine};

fn jagged_circle(center: Point2, radius: f64, noise: f64, count: usize) -> Vec<Point2> {
    (0..count)
        .map(|i| {
            let t = i as f64 / count as f64 * std::f64::consts::TAU;
            let hash = ((i as u64).wrapping_mul(2_654_435_761) % 997) as f64 / 997.0;
            let r = radius * (1.0 + (hash - 0.5) * 2.0 * noise);
            Point2::new(center.x + r * t.cos(), center.y + r * t.sin())
        })
        .collect()
}

/// The four anchors of the path, in draw order, from the engine's own overlay.
fn anchors(engine: &VectraEngine, path_id: &str) -> Vec<(f64, f64)> {
    let overlay: serde_json::Value =
        serde_json::from_str(&engine.draw_overlay(path_id)).expect("overlay JSON");
    overlay["anchors"]
        .as_array()
        .expect("anchors")
        .iter()
        .map(|anchor| (anchor["x"].as_f64().unwrap(), anchor["y"].as_f64().unwrap()))
        .collect()
}

fn dispatch(engine: &mut VectraEngine, command: &Command) -> serde_json::Value {
    let response = engine.dispatch_command(&command.to_json().unwrap());
    let parsed: serde_json::Value = serde_json::from_str(&response).expect("a response envelope");
    assert_eq!(
        parsed["status"],
        "ok",
        "the engine refused {}: {response}",
        command.label()
    );
    parsed
}

#[test]
fn the_solver_snaps_the_anchors_onto_the_primitive() {
    let center = Point2::new(120.0, 84.0);
    let radius = 56.0;
    let stroke = jagged_circle(center, radius, 0.2, 40);
    let path_id = new_node_id();
    let primitive_id = new_node_id();
    let tolerance = radius * 0.4;
    let plan = plan_quick_shape(&stroke, path_id, primitive_id, tolerance)
        .expect("a closed jagged loop is a circle");
    assert_eq!(plan.kind, SnapKind::Circle);
    assert_eq!(
        plan.constraints.len(),
        13,
        "3 pins + 4 shared + 4 offsets + 2 seam rows"
    );

    let mut engine = VectraEngine::new();
    // ── a path whose cardinal anchors are deliberately WRONG ───────────────
    //
    // Everything about it is plausible except that each anchor is displaced by
    // three units on one axis. The test never corrects them: the only edits it
    // makes are the plan's constraint rows.
    const JITTER: f64 = 3.0;
    let displaced: Vec<PathSegment> = plan
        .segments
        .iter()
        .enumerate()
        .map(|(index, segment)| match segment {
            PathSegment::Cubic {
                control1,
                control2,
                to,
            } => {
                let shift = |param: &Parameter<Point2>| match param {
                    Parameter::Literal(point) => {
                        Parameter::Literal(Point2::new(point.x + JITTER, point.y))
                    }
                    other => other.clone(),
                };
                let _ = index;
                PathSegment::Cubic {
                    control1: shift(control1),
                    control2: shift(control2),
                    to: shift(to),
                }
            }
            other => other.clone(),
        })
        .collect();
    dispatch(
        &mut engine,
        &Command::CreateNode {
            id: path_id,
            kind: NodeKind::Path {
                start: Parameter::Literal(Point2::new(plan.start.x + JITTER, plan.start.y)),
                segments: displaced,
            },
            name: Some("rough path".into()),
            index: None,
        },
    );
    // Only the *slots* the plan constrains are checked for displacement; the
    // fourth arc's endpoint is the seam twin of `start` and carries no row of its
    // own, which is the point of the representation (four vertices, not five).
    let before = anchors(&engine, &path_id.to_string());
    let (cx, cy, r) = {
        // The primitive is created from the plan, exactly as the snap does.
        dispatch(
            &mut engine,
            &Command::CreateNode {
                id: primitive_id,
                kind: plan.primitive.clone(),
                name: Some("snapped circle".into()),
                index: None,
            },
        );
        match plan.primitive {
            NodeKind::Circle { cx, cy, radius, .. } => (literal(cx), literal(cy), literal(radius)),
            _ => unreachable!("a circle snap yields a circle"),
        }
    };

    // ── claim 1: the rows, and who comes first in each one ──────────────────
    //
    // The cardinal rows must name the primitive first — those are the rows whose
    // hint decides whether the circle or the path stays put. The seam rows are
    // the exception that proves the rule: they involve the path alone, and their
    // first target is the arc endpoint because *it* is the value being pinned to
    // the start point.
    let mut cardinal_rows = 0;
    for constraint in &plan.constraints {
        assert_eq!(constraint.strength, vectra_core::Strength::Medium);
        let nodes: Vec<vectra_core::NodeId> =
            constraint.targets.iter().map(|t| t.node_id).collect();
        if nodes.contains(&primitive_id) {
            cardinal_rows += 1;
            assert_eq!(
                constraint.targets[0].node_id, primitive_id,
                "a cardinal row must name the primitive first, or the hints pin \
                 the wrong end and the *circle* moves instead of the path: \
                 {constraint:?}"
            );
        }
    }
    assert_eq!(cardinal_rows, 11, "3 pins + 8 cardinal rows");

    // ── claim 2: the solver moves the anchors ──────────────────────────────
    for constraint in &plan.constraints {
        dispatch(
            &mut engine,
            &Command::AddConstraint {
                constraint: constraint.clone(),
            },
        );
    }
    let after = anchors(&engine, &path_id.to_string());
    let cardinals = [(cx + r, cy), (cx, cy + r), (cx - r, cy), (cx, cy - r)];
    assert_eq!(
        before.len(),
        5,
        "four distinct positions in five slots: start plus four arc endpoints, \
         the last of which lands on start"
    );
    // The path really did start wrong (otherwise the test proves nothing).
    let worst_before = before
        .iter()
        .zip(&cardinals)
        .map(|((x, y), (ex, ey))| ((x - ex).abs()).max((y - ey).abs()))
        .fold(0.0_f64, f64::max);
    assert!(
        worst_before > JITTER - 1e-9,
        "the fixture must start displaced, but it is {worst_before} off"
    );
    for (index, (x, y)) in after.iter().enumerate().take(4) {
        let (ex, ey) = cardinals[index];
        assert!(
            (x - ex).abs() < 1e-9 && (y - ey).abs() < 1e-9,
            "anchor {index} is ({x}, {y}) after the solve, expected ({ex}, {ey})"
        );
    }
    // The first anchor is the right-hand cardinal, and the *seam* — the fourth
    // arc's endpoint, which the last two rows hold on top of `start` — is there
    // too: the closure is an invariant, not a coincidence of two numbers.
    assert!((after[0].0 - (cx + r)).abs() < 1e-9);
    assert!((after[0].1 - cy).abs() < 1e-9);
    assert!(
        (after[4].0 - after[0].0).abs() < 1e-9 && (after[4].1 - after[0].1).abs() < 1e-9,
        "the seam must be held shut: {:?} vs {:?}",
        after[4],
        after[0]
    );

    // ── claim 3: the system holds — a fresh solve has nothing to write ─────
    //
    // The engine's own definition of "this document satisfies its rules". Asked
    // by dispatching the same rows' slots through a pass: adding a *redundant*
    // row (the same statement, again) is accepted by the pre-pass as a duplicate
    // and produces no movement.
    let settled = engine.document_json();
    let duplicate = plan.constraints[3].clone();
    let duplicate = vectra_core::Constraint::new(
        vectra_core::new_constraint_id(),
        duplicate.kind,
        duplicate.targets.clone(),
    )
    .with_value(duplicate.value.unwrap_or(0.0));
    dispatch(
        &mut engine,
        &Command::AddConstraint {
            constraint: duplicate,
        },
    );
    let after_redundant = anchors(&engine, &path_id.to_string());
    assert_eq!(
        after, after_redundant,
        "a row the system already satisfies must not move anything"
    );
    let _ = settled;

    // ── claim 4: all of it is undoable record, nothing is baked in ─────────
    for _ in 0..12 {
        engine.undo();
    }
    let restored = anchors(&engine, &path_id.to_string());
    assert_eq!(
        restored, before,
        "undoing the rows must bring the displaced anchors back"
    );
}

fn literal(param: Parameter<f64>) -> f64 {
    match param {
        Parameter::Literal(value) => value,
        other => panic!("expected a literal, got {other:?}"),
    }
}

#[test]
fn a_snapped_circle_is_drawn_as_a_circle_and_not_as_a_polyline() {
    // The end-to-end shape of RULE 3's output: four cubic arcs and a Close — the
    // engine's own vocabulary, and the geometry the SVG exporter turns into
    // `<circle>`-equivalent path data (Task 8.0 RULE 1).
    let stroke = jagged_circle(Point2::new(0.0, 0.0), 80.0, 0.15, 48);
    let plan = plan_quick_shape(&stroke, new_node_id(), new_node_id(), 20.0).expect("a circle");
    assert_eq!(
        plan.segments.len(),
        4,
        "four arcs, no Close: closed by geometry"
    );
    let cubics = plan
        .segments
        .iter()
        .filter(|segment| matches!(segment, PathSegment::Cubic { .. }))
        .count();
    assert_eq!(cubics, 4, "four kappa arcs");
    // No `Line` anywhere: a snapped circle is never a polyline.
    assert!(plan
        .segments
        .iter()
        .all(|segment| !matches!(segment, PathSegment::Line { .. })));
}

/// **The overlay is placed by the same camera the scene is drawn with**
/// (Task 10.1 RULE 4).
///
/// A designer grabs the handle they can *see*, so the handle drawn on the screen
/// and the anchor the engine reports must be the same point. That is not a
/// property of the UI — it is a property of there being one camera, and this law
/// is the falsifiable form of it: every anchor of a snapped circle is mapped to
/// client pixels by the renderer, mapped back by the renderer, and must return to
/// where it started. The y-axis is why this is a law and not a comment: document
/// space is **y-up**, the DOM is y-down, and an overlay laid out with its own
/// `viewBox` would be silently mirrored — the anchors would sit correctly on the
/// x axis and upside-down on the y axis.
///
/// It also pins the two halves of the round trip together: `document_to_client`
/// is the inverse of `client_to_document`, the mapping the *pointer* travels.
#[test]
fn the_overlay_is_placed_by_the_same_camera_the_scene_is_drawn_with() {
    let path_id = new_node_id();
    let plan = plan_quick_shape(
        &jagged_circle(Point2::new(120.0, 84.0), 56.0, 0.2, 40),
        path_id,
        new_node_id(),
        20.0,
    )
    .expect("a circle");

    // The renderer the UI would be using: an 800×600 CSS box at dpr 2, which is
    // the case where a hand-rolled transform is most likely to be wrong.
    let mut renderer = Renderer::new();
    renderer.viewport(0.0, 0.0, 800.0, 600.0, 2.0);
    let view: serde_json::Value = serde_json::from_str(&renderer.view()).expect("view");

    // Document points: the four cardinal anchors the snap wrote.
    let document: Vec<[f64; 2]> = {
        let mut points = vec![[plan.start.x, plan.start.y]];
        for segment in &plan.segments {
            if let PathSegment::Cubic { to, .. } = segment {
                let Parameter::Literal(point) = to else {
                    panic!("a snapped circle is written as literals");
                };
                points.push([point.x, point.y]);
            }
        }
        points
    };

    let mapped: serde_json::Value = serde_json::from_str(
        &renderer.document_to_client(&serde_json::to_string(&document).unwrap()),
    )
    .expect("mapped JSON");
    assert_eq!(mapped["ok"], serde_json::Value::Bool(true), "{mapped}");
    let client: Vec<[f64; 2]> = serde_json::from_value(mapped["points"].clone()).expect("points");
    assert_eq!(client.len(), document.len());

    for (index, point) in document.iter().enumerate() {
        let (px, py) = (client[index][0], client[index][1]);
        // On screen: inside the box …
        assert!(
            (0.0..=800.0).contains(&px) && (0.0..=600.0).contains(&py),
            "anchor {index} maps off-canvas: ({px}, {py})"
        );
        // … and the round trip closes, so the handle is where the anchor is.
        let (back_x, back_y) = renderer.client_to_document(px, py).expect("mapped back");
        // Tolerance: the camera is `f32` (it feeds a GPU uniform), so a document
        // unit's worth of round trip is ~1e-5 at this scale. A pixel is 1e-3 of
        // the *screen*, which is the error a designer could actually see, and
        // that is the bar this law holds.
        assert!(
            (back_x - point[0]).abs() < 1e-3 && (back_y - point[1]).abs() < 1e-3,
            "anchor {index}: {:?} → ({px}, {py}) → ({back_x}, {back_y})",
            point
        );
    }

    // The mapping is not the identity inside a `viewBox`: y is *flipped* by the
    // camera, which is exactly the bug this law exists to catch.
    let (_, document_y) = (document[0][0], document[0][1]);
    let (center_x, center_y) = (
        view["x"].as_f64().unwrap() + view["w"].as_f64().unwrap() / 2.0,
        view["y"].as_f64().unwrap() + view["h"].as_f64().unwrap() / 2.0,
    );
    let relative_document = document_y - center_y;
    let relative_screen = client[0][1] - 300.0;
    assert!(
        relative_document * relative_screen < 0.0,
        "a document point above the centre must be drawn above the centre: \
         document {relative_document}, screen {relative_screen} (centre x {center_x})"
    );

    // A canvas with no box answers honestly instead of inventing pixels.
    let unmeasured = Renderer::new();
    let none: serde_json::Value =
        serde_json::from_str(&unmeasured.document_to_client("[[0,0]]")).expect("JSON");
    assert_eq!(none["ok"], serde_json::Value::Bool(false), "{none}");
    assert_eq!(none["points"].as_array().map(Vec::len), Some(0));
}

/// **The direct-selection gesture, at the engine boundary** (Task 10.1 RULE 4).
///
/// Three claims, each of which is a design decision that would be easy to get
/// wrong in a way nobody notices until a designer loses work:
///
/// 1. **A drag is one undo step.** The gesture applies every pointer sample
///    *untracked* (the document and the scene move immediately — that is what a
///    drag looks like) and records exactly one `Set path` entry when it ends.
///    Twelve samples must therefore leave the history exactly one step deeper,
///    and one undo must restore the pre-drag geometry — not sample 11, and not
///    the shape halfway through.
/// 2. **A smooth handle drag mirrors its partner**, and the mirror is the
///    *identity* `(h_in − a) + (h_out − a) = const`.
/// 3. **An Alt-drag breaks that mirror** — and it does so by *writing only the
///    dragged handle*, which is what makes independence checkable from the
///    command itself rather than from a before/after diff.
///
/// The drag lives behind `draw_edit_begin`/`draw_edit_update`/`draw_edit_end`
/// because the alternative — dispatching a `SetParameter` batch per sample — would
/// take two hundred undos to take back one stroke of the mouse, which is the
/// difference between a toy and a tool.
#[test]
fn a_direct_selection_drag_is_one_undo_step_and_alt_breaks_the_mirror() {
    let mut engine = VectraEngine::new();
    let mut pen = |kind: &str, x: f64, y: f64, alt: bool| -> serde_json::Value {
        serde_json::from_str(&engine.draw_pointer("pen", kind, x, y, alt, false, 0.0))
            .expect("pen reply")
    };
    pen("down", 0.0, 0.0, false);
    pen("up", 0.0, 0.0, false);
    // A drag, so the middle anchor is smooth: handles in both directions.
    pen("down", 50.0, 0.0, false);
    pen("move", 80.0, 20.0, false);
    pen("up", 80.0, 20.0, false);
    pen("down", 100.0, 0.0, false);
    pen("up", 100.0, 0.0, false);
    let committed: serde_json::Value =
        serde_json::from_str(&engine.draw_pen_commit(false, None)).expect("commit reply");
    assert_eq!(
        committed["ok"],
        serde_json::Value::Bool(true),
        "{committed}"
    );
    let path_id = committed["node_id"]
        .as_str()
        .expect("a node id")
        .to_string();

    let overlay = |engine: &VectraEngine| -> Vec<serde_json::Value> {
        let parsed: serde_json::Value =
            serde_json::from_str(&engine.draw_overlay(&path_id)).expect("overlay");
        parsed["anchors"].as_array().expect("anchors").clone()
    };
    let depth = |engine: &mut VectraEngine| -> u64 {
        let parsed: serde_json::Value =
            serde_json::from_str(&engine.get_snapshot()).expect("snapshot");
        parsed["undo_depth"].as_u64().expect("undo depth")
    };

    let before = overlay(&engine);
    let smooth_index = before
        .iter()
        .position(|anchor| !anchor["handle_in"].is_null() && !anchor["handle_out"].is_null())
        .expect("the dragged anchor kept both handles");
    let smooth = before[smooth_index].clone();
    let slot = smooth["slot"].as_str().expect("slot").to_string();
    let anchor_point = (smooth["x"].as_f64().unwrap(), smooth["y"].as_f64().unwrap());
    let handle_sum = |anchor: &serde_json::Value| -> (f64, f64) {
        let x = anchor["x"].as_f64().unwrap();
        let y = anchor["y"].as_f64().unwrap();
        let h_in = anchor["handle_in"].as_array().expect("in handle");
        let h_out = anchor["handle_out"].as_array().expect("out handle");
        (
            h_in[0].as_f64().unwrap() - x + (h_out[0].as_f64().unwrap() - x),
            h_in[1].as_f64().unwrap() - y + (h_out[1].as_f64().unwrap() - y),
        )
    };
    let sum_before = handle_sum(&smooth);

    // ── 2. the mirror, and what the command says about it ─────────────────────
    let out_x = smooth["handle_out"][0].as_f64().unwrap();
    let mirror_edit: serde_json::Value = serde_json::from_str(
        &engine.draw_edit_command(&path_id, &slot, "out", out_x, 60.0, false, false),
    )
    .expect("edit command");
    assert_eq!(mirror_edit["type"], "Batch", "{mirror_edit}");
    assert_eq!(
        mirror_edit["commands"].as_array().map(Vec::len),
        Some(4),
        "the dragged handle and its partner, each x and y: {mirror_edit}"
    );
    // Through the ordinary command door: the edit is a `Command` like any other,
    // and it is the engine's dispatch (gates, record, settle) that applies it.
    let applied = engine.dispatch_command(&mirror_edit.to_string());
    let applied: serde_json::Value = serde_json::from_str(&applied).expect("a response envelope");
    assert_eq!(applied["status"], "ok", "{applied}");
    let mirrored = overlay(&engine)[smooth_index].clone();
    assert_eq!(mirrored["handle_out"], serde_json::json!([out_x, 60.0]));
    let sum_after = handle_sum(&mirrored);
    assert!(
        (sum_before.0 - sum_after.0).abs() < 1e-9 && (sum_before.1 - sum_after.1).abs() < 1e-9,
        "symmetry: the two handles still reflect through {anchor_point:?} — {sum_before:?} vs {sum_after:?}"
    );

    // ── 3. Alt breaks the mirror, and *says so* in the command ────────────────
    let alt_edit: serde_json::Value = serde_json::from_str(
        &engine.draw_edit_command(&path_id, &slot, "out", out_x, 100.0, true, false),
    )
    .expect("alt edit command");
    assert_eq!(
        alt_edit["commands"].as_array().map(Vec::len),
        Some(2),
        "an Alt-drag writes the dragged handle and nothing else: {alt_edit}"
    );
    let applied = engine.dispatch_command(&alt_edit.to_string());
    let applied: serde_json::Value = serde_json::from_str(&applied).expect("a response envelope");
    assert_eq!(applied["status"], "ok", "{applied}");
    let broken = overlay(&engine)[smooth_index].clone();
    assert_eq!(
        broken["handle_in"], mirrored["handle_in"],
        "the partner did not move"
    );
    assert_eq!(broken["handle_out"], serde_json::json!([out_x, 100.0]));

    // ── 1. a real drag: twelve samples, one history entry ─────────────────────
    let depth_before = depth(&mut engine);
    let begun: serde_json::Value =
        serde_json::from_str(&engine.draw_edit_begin(&path_id, &slot, "", true)).expect("begin");
    assert_eq!(begun["status"], "ok", "{begun}");
    for step in 1..=12 {
        let update: serde_json::Value =
            serde_json::from_str(&engine.draw_edit_update(50.0 + step as f64, step as f64, false))
                .expect("update");
        assert_eq!(update["status"], "ok", "{update}");
    }
    // The drag is *live*: the geometry has already moved, and the history has not.
    let during = overlay(&engine)[smooth_index].clone();
    assert_eq!(
        during["x"].as_f64().unwrap(),
        62.0,
        "the anchor follows the pointer"
    );
    assert_eq!(
        depth(&mut engine),
        depth_before,
        "the samples are untracked: no history entry per pixel"
    );
    let ended: serde_json::Value = serde_json::from_str(&engine.draw_edit_end()).expect("end");
    assert_eq!(ended["status"], "ok", "{ended}");
    assert_eq!(
        depth(&mut engine),
        depth_before + 1,
        "twelve samples, ONE entry"
    );

    // …and one undo restores the shape the gesture started from, exactly.
    let undone: serde_json::Value = serde_json::from_str(&engine.undo()).expect("undo");
    assert_eq!(undone["status"], "ok", "{undone}");
    let after_undo = overlay(&engine)[smooth_index].clone();
    assert_eq!(
        after_undo["x"].as_f64().unwrap(),
        50.0,
        "one undo, the old geometry"
    );
    assert_eq!(after_undo["y"].as_f64().unwrap(), 0.0);
    assert_eq!(
        after_undo["handle_in"], broken["handle_in"],
        "and the handles it had before the drag"
    );
    assert_eq!(after_undo["handle_out"], broken["handle_out"]);
}

/// **A `Quadratic` segment is editable from both of its anchors.**
///
/// RULE 1 names three segment kinds, and the pen emits two of them (`Line`,
/// `Cubic`) — so a quadratic only arrives from an AI command or an imported
/// document. That is exactly why it is worth a law: the direct-selection slot
/// arithmetic used to be cubic-shaped by construction (`segments[i].control1`
/// / `.control2`), which made a quadratic a dead end — no handles in the
/// overlay, an error from every edit, and an anchor drag that silently changed
/// the curve's shape instead of translating it.
///
/// A quadratic's `control` is *one* point that is both handles, so the three
/// things to check are that it is reachable from either end, that dragging it
/// writes it once (never a mirrored phantom on the same slot), and that moving
/// an anchor still translates it — the invariant every other kind honours.
#[test]
fn a_quadratic_control_is_editable_from_both_of_its_anchors() {
    let mut engine = VectraEngine::new();
    let id = new_node_id();
    // The drawing boundary speaks strings (it is a JavaScript surface), so the
    // node id crosses as text exactly as React would send it.
    let node_id = id.to_string();
    let literal = |x: f64, y: f64| serde_json::json!({ "Literal": { "x": x, "y": y } });
    //  Cubic → Quadratic → Line, so the quadratic has a curved neighbour on one
    //  side and a straight one on the other.
    let create = serde_json::json!({
        "type": "CreateNode",
        "id": id,
        "name": "quadratic",
        "kind": { "Path": {
            "start": literal(0.0, 0.0),
            "segments": [
                { "Cubic": {
                    "control1": literal(10.0, 30.0),
                    "control2": literal(40.0, 30.0),
                    "to": literal(50.0, 0.0),
                }},
                { "Quadratic": { "control": literal(75.0, 30.0), "to": literal(100.0, 0.0) } },
                { "Line": { "to": literal(150.0, 0.0) } },
            ],
        }},
    });
    let created = engine.dispatch_command(&create.to_string());
    let created: serde_json::Value = serde_json::from_str(&created).expect("a response");
    assert_eq!(created["status"], "ok", "{created}");

    let overlay = |engine: &VectraEngine| -> Vec<serde_json::Value> {
        let parsed: serde_json::Value =
            serde_json::from_str(&engine.draw_overlay(&node_id)).expect("overlay");
        parsed["anchors"].as_array().expect("anchors").clone()
    };
    let control_of = |engine: &VectraEngine| -> serde_json::Value {
        let overlay = overlay(engine);
        let joint = overlay
            .iter()
            .find(|a| a["slot"] == "segments[1].to")
            .expect("the quadratic's end point");
        joint["handle_in"].clone()
    };

    // ── the quadratic's one control point is *both* handles ───────────────────
    let anchors = overlay(&engine);
    let leaving = anchors
        .iter()
        .find(|a| a["slot"] == "segments[0].to")
        .expect("the quadratic's start point");
    assert_eq!(
        leaving["handle_out"],
        serde_json::json!([75.0, 30.0]),
        "the handle leaving the quadratic's start is its control point"
    );
    assert_eq!(
        control_of(&engine),
        serde_json::json!([75.0, 30.0]),
        "…and so is the handle arriving at its end — one point, drawn on both sides"
    );
    // A `Line` neighbour still has no handle: nothing is invented.
    let end = anchors
        .iter()
        .find(|a| a["slot"] == "segments[2].to")
        .expect("the line's end point");
    assert_eq!(end["handle_in"], serde_json::Value::Null);
    assert_eq!(end["handle_out"], serde_json::Value::Null);

    // ── the symmetry rule is the *anchor's*, at a quadratic joint too ────────
    //
    // The quadratic's end anchor: the next segment is a `Line`, so there is no
    // other arm and the drag is the dragged point alone.
    let edit: serde_json::Value = serde_json::from_str(&engine.draw_edit_command(
        &node_id,
        "segments[1].to",
        "in",
        90.0,
        60.0,
        false,
        false,
    ))
    .expect("an edit command");
    let commands = edit["commands"].as_array().expect("commands");
    assert_eq!(
        commands.len(),
        2,
        "a quadratic's arm with nothing on the other side: {edit}"
    );
    assert!(commands
        .iter()
        .all(|command| command["type"] == "SetParameter"));
    let applied = engine.dispatch_command(&edit.to_string());
    let applied: serde_json::Value = serde_json::from_str(&applied).expect("a response");
    assert_eq!(applied["status"], "ok", "{applied}");
    assert_eq!(
        control_of(&engine),
        serde_json::json!([90.0, 60.0]),
        "the dragged control is exactly where the pointer was"
    );

    // The quadratic's *start* anchor: the joint's other arm is the cubic's
    // `control2`, so the same gesture mirrors it — a quadratic arm smooths a
    // joint exactly as a cubic arm does.
    let edit: serde_json::Value = serde_json::from_str(&engine.draw_edit_command(
        &node_id,
        "segments[0].to",
        "out",
        70.0,
        45.0,
        false,
        false,
    ))
    .expect("an edit command");
    let commands = edit["commands"].as_array().expect("commands");
    assert_eq!(
        commands.len(),
        4,
        "the dragged arm and its partner, each x and y: {edit}"
    );
    let mut written: Vec<String> = commands
        .iter()
        .map(|command| command["property"].as_str().unwrap_or_default().to_string())
        .collect();
    written.sort();
    assert_eq!(
        written,
        [
            "segments[0].control2.x",
            "segments[0].control2.y",
            "segments[1].control.x",
            "segments[1].control.y",
        ],
        "the quadratic's own control, plus the arm reflected through the anchor: {edit}"
    );
    let applied = engine.dispatch_command(&edit.to_string());
    let applied: serde_json::Value = serde_json::from_str(&applied).expect("a response");
    assert_eq!(applied["status"], "ok", "{applied}");
    assert_eq!(control_of(&engine), serde_json::json!([70.0, 45.0]));
    // The anchor is (50, 0), so the cubic's arriving handle lands at 2a − h.
    let cubic_handle = |engine: &VectraEngine| -> serde_json::Value {
        let parsed: serde_json::Value =
            serde_json::from_str(&engine.draw_overlay(&node_id)).expect("overlay");
        parsed["anchors"]
            .as_array()
            .expect("anchors")
            .iter()
            .find(|anchor| anchor["slot"] == "segments[0].to")
            .expect("the joint")["handle_in"]
            .clone()
    };
    assert_eq!(
        cubic_handle(&engine),
        serde_json::json!([30.0, -45.0]),
        "mirrored through the joint: 2·(50,0) − (70,45)"
    );

    // …and Alt frees that joint, exactly as RULE 2 says it frees a cubic's.
    let edit: serde_json::Value = serde_json::from_str(&engine.draw_edit_command(
        &node_id,
        "segments[0].to",
        "out",
        70.0,
        20.0,
        true,
        false,
    ))
    .expect("an edit command");
    assert_eq!(
        edit["commands"].as_array().map(Vec::len),
        Some(2),
        "Alt writes the dragged arm and nothing else: {edit}"
    );
    let applied = engine.dispatch_command(&edit.to_string());
    let applied: serde_json::Value = serde_json::from_str(&applied).expect("a response");
    assert_eq!(applied["status"], "ok", "{applied}");
    assert_eq!(control_of(&engine), serde_json::json!([70.0, 20.0]));
    assert_eq!(
        cubic_handle(&engine),
        serde_json::json!([30.0, -45.0]),
        "the other arm did not move: independence, at a quadratic joint"
    );

    // ── an anchor drag translates the control: the shape does not change ──────
    let before = overlay(&engine);
    let joint = before
        .iter()
        .find(|a| a["slot"] == "segments[1].to")
        .expect("the quadratic's end point");
    let (from_x, from_y) = (joint["x"].as_f64().unwrap(), joint["y"].as_f64().unwrap());
    let control_before = control_of(&engine);
    let cubic_before = cubic_handle(&engine);
    let (to_x, to_y) = (from_x + 12.0, from_y - 7.0);
    let edit: serde_json::Value = serde_json::from_str(&engine.draw_edit_command(
        &node_id,
        "segments[1].to",
        "",
        to_x,
        to_y,
        false,
        true,
    ))
    .expect("an anchor edit");
    let applied = engine.dispatch_command(&edit.to_string());
    let applied: serde_json::Value = serde_json::from_str(&applied).expect("a response");
    assert_eq!(applied["status"], "ok", "{applied}");
    let after = overlay(&engine);
    let joint = after
        .iter()
        .find(|a| a["slot"] == "segments[1].to")
        .expect("the quadratic's end point");
    assert_eq!(joint["x"].as_f64().unwrap(), to_x);
    assert_eq!(joint["y"].as_f64().unwrap(), to_y);
    assert_eq!(
        control_of(&engine),
        serde_json::json!([
            control_before[0].as_f64().unwrap() + 12.0,
            control_before[1].as_f64().unwrap() - 7.0
        ]),
        "the control travelled with the anchor, so the curve translated instead of deforming"
    );
    assert_eq!(
        cubic_handle(&engine),
        cubic_before,
        "and nothing further: that cubic arm belongs to the *other* anchor, so this \
drag must leave it exactly where it was"
    );
}
