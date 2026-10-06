//! Task 6.0 laws, at the WASM boundary: **motion is a source, not a system**.
//!
//! `vectra-motion`'s own suite proves the maths (the closed-form spring, the
//! track sampler, the horizons). These laws prove the *composition* — that the
//! clock, the dirty graph, the undo stack and the canvas behave the way the
//! design says they do once motion is wired into the shipped engine:
//!
//! ```text
//! set_time ──▶ dirty(clock) ──▶ patch ──▶ ledger ──▶ render_frame
//! set_state ──▶ flip ──▶ re-anchor springs ──▶ dirty(those nodes) ──┘
//! pointer_move ──▶ hit_test ──▶ hover:<id> ──┘
//! ```
//!
//! The seven laws (`TASK-6.0-RECON.md` §4), each with its own test:
//!
//! | law | test |
//! |---|---|
//! | Motion Purity | [`law_the_scene_is_a_function_of_the_clock`] |
//! | Spring Convergence | [`law_a_spring_converges_and_stops`] |
//! | Idle | [`law_the_frame_loop_can_stop`] |
//! | Interaction Precedence | [`law_a_drag_outranks_a_spring_and_hands_it_back`] |
//! | State | [`law_states_are_inputs_and_hover_drives_them`] |
//! | Undo Isolation | [`law_motion_samples_are_not_history`] |
//! | Frame Budget | [`law_an_animating_scene_costs_only_the_slot_that_moves`] |

use vectra_core::new_node_id;
use vectra_wasm::{Renderer, VectraEngine};

// ── Engine driving (the JSON wire the React remote uses) ───────────────────

fn dispatch(engine: &mut VectraEngine, cmd: serde_json::Value) -> serde_json::Value {
    serde_json::from_str(&engine.dispatch_command(&cmd.to_string())).expect("parsed response")
}

fn frame(engine: &mut VectraEngine, renderer: &mut Renderer) -> serde_json::Value {
    serde_json::from_str(&engine.render_frame(renderer)).expect("parsed frame")
}

fn snapshot(engine: &mut VectraEngine) -> serde_json::Value {
    serde_json::from_str(&engine.get_snapshot()).expect("parsed snapshot")
}

/// Send a command, typed as a JSON value (the `from_str` target must be
/// explicit everywhere, or inference has nothing to work with).
fn json(text: &str) -> serde_json::Value {
    serde_json::from_str::<serde_json::Value>(text).expect("parsed response")
}

fn motion(engine: &VectraEngine) -> serde_json::Value {
    serde_json::from_str(&engine.motion_json()).expect("parsed motion report")
}

fn number(value: &serde_json::Value, key: &str) -> f64 {
    value[key]
        .as_f64()
        .unwrap_or_else(|| panic!("{key}: {value}"))
}

/// The live value of the only binding on a node's `width` slot.
fn bound_value(engine: &VectraEngine, node: &str) -> f64 {
    let report = motion(engine);
    let binding = report["bindings"]
        .as_array()
        .expect("bindings")
        .iter()
        .find(|b| b["node_id"] == serde_json::json!(node) && b["property"] == "width")
        .unwrap_or_else(|| panic!("no width binding for {node}: {report}"));
    binding["value"]
        .as_f64()
        .unwrap_or_else(|| panic!("binding value: {binding}"))
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

fn renderer_at(css_w: f64, css_h: f64) -> Renderer {
    let mut renderer = Renderer::new();
    renderer.viewport(0.0, 0.0, css_w, css_h, 1.0);
    renderer
}

/// Scrub the clock through the public wasm entry point.
fn set_time(engine: &mut VectraEngine, t: f64) -> serde_json::Value {
    serde_json::from_str(&engine.set_time(t)).expect("parsed response")
}

/// A scene that is warm and at rest: one rect with a spring on `width`.
fn sprung_scene(target: f64, stiffness: f64, damping: f64) -> (VectraEngine, String) {
    let mut engine = VectraEngine::new();
    let node = create_rect(&mut engine, "A", 0.0, 0.0, 100.0, 50.0);
    let response = json(&engine.bind_spring(&node, "width", target, stiffness, damping));
    assert_eq!(response["status"], "ok", "bind: {response}");
    (engine, node)
}

// ── Law 1: Motion Purity ──────────────────────────────────────────────────

/// The scene is a **function of the document and the clock** — nothing else.
///
/// Walking the clock forwards, backwards, in random order, or visiting the same
/// time twice must produce bit-identical geometry, because motion carries no
/// accumulated state between evaluations. This is the law that makes scrubbing,
/// replay, remote clients and headless rendering agree; a frame-integrating
/// spring fails it on the first backwards step.
#[test]
fn law_the_scene_is_a_function_of_the_clock() {
    let (mut engine, _node) = sprung_scene(400.0, 170.0, 26.0);

    let fingerprint = |engine: &mut VectraEngine, t: f64| -> String {
        set_time(engine, t);
        let snap = snapshot(engine);
        format!("{}@{}", snap["scene"], snap["time"])
    };

    // A forward sweep, remembering every frame.
    let sweep: Vec<(f64, String)> = (0..=20)
        .map(|step| {
            let t = step as f64 * 0.05;
            (t, fingerprint(&mut engine, t))
        })
        .collect();

    // Backwards through the same times: every frame must match its forward twin.
    for (t, expected) in sweep.iter().rev() {
        assert_eq!(
            &fingerprint(&mut engine, *t),
            expected,
            "scrubbing back to t={t} changed the scene"
        );
    }

    // And out of order, with repeats.
    for (t, expected) in sweep.iter().rev().chain(sweep.iter()) {
        assert_eq!(&fingerprint(&mut engine, *t), expected, "t={t} again");
    }
}

// ── Law 2: Spring Convergence ─────────────────────────────────────────────

/// A spring reaches its target, monotonically enough, and — once it has — the
/// engine says so. No oscillation below the settle threshold, no overshoot when
/// the damping ratio forbids it, and no dependence on how many frames were
/// drawn along the way (the `evaluate`-per-frame count is a UI concern, not a
/// motion input).
#[test]
fn law_a_spring_converges_and_stops() {
    let (mut engine, node) = sprung_scene(400.0, 170.0, 26.0);

    assert_eq!(
        bound_value(&engine, &node),
        100.0,
        "at t=0 the spring is `from`"
    );
    assert!(engine.is_animating(), "and it is moving");

    let mut previous = 100.0;
    for step in 1..=200 {
        let t = step as f64 / 100.0;
        set_time(&mut engine, t);
        let value = bound_value(&engine, &node);
        assert!(
            value >= previous - 1e-9 && value <= 400.0 + 1e-9,
            "monotone approach to the target at t={t}: {previous} → {value}"
        );
        previous = value;
    }
    assert!(
        (bound_value(&engine, &node) - 400.0).abs() < 1e-2,
        "converged: {}",
        bound_value(&engine, &node)
    );
    assert!(!engine.is_animating(), "and it has stopped");
}

// ── Law 3: Idle ───────────────────────────────────────────────────────────

/// **The frame loop can stop.** `is_animating()` is `false` exactly when the
/// document owes the clock nothing, and the report carries the horizon so the
/// UI knows how long it would have to wait.
///
/// This closes Task 5.0's open item #4: the React loop used to spin forever,
/// drawing nothing, because the engine could not say "nothing will change".
#[test]
fn law_the_frame_loop_can_stop() {
    let (mut engine, node) = sprung_scene(400.0, 170.0, 26.0);

    let moving = motion(&engine);
    assert_eq!(moving["animating"], true);
    let horizon = moving["horizon"].as_f64().expect("a horizon while moving");
    assert!(horizon > 0.0, "horizon {horizon}");

    // At the horizon the report flips to idle… (a hair past it, to stay clear of
    // the exact float where the envelope meets the threshold).
    set_time(&mut engine, horizon + 0.05);
    let settled = motion(&engine);
    assert_eq!(settled["animating"], false, "{settled}");
    assert!(settled["horizon"].is_null());
    assert!(!engine.is_animating());

    // …and the value there really is the target.
    assert!((bound_value(&engine, &node) - 400.0).abs() < 1e-3);

    // An empty document is trivially idle; a document with no bindings is too.
    let mut bare = VectraEngine::new();
    create_rect(&mut bare, "B", 0.0, 0.0, 10.0, 10.0);
    assert!(!bare.is_animating());
}

// ── Law 4: Interaction Precedence ─────────────────────────────────────────

/// **A live drag outranks motion.** While the pointer is down the slot follows
/// the pointer, not the spring; on release the slot does not freeze into a
/// literal — the spring takes it back, re-anchored at the committed value, so
/// the animation resumes from where the finger left it.
///
/// The gesture must also stay exactly invertible: undo restores the binding the
/// slot had before, redo replays the re-anchor.
#[test]
fn law_a_drag_outranks_a_spring_and_hands_it_back() {
    let (mut engine, node) = sprung_scene(400.0, 170.0, 26.0);
    set_time(&mut engine, 1.0);
    let before_undo_depth = number(&snapshot(&mut engine), "undo_depth");

    // The drag: the pointer owns `x`… and we bind a spring there first, so the
    // contest is on the same slot.
    let bind_x = json(&engine.bind_spring(&node, "x", 900.0, 170.0, 26.0));
    assert_eq!(bind_x["status"], "ok", "{bind_x}");
    let undo_after_binding = number(&snapshot(&mut engine), "undo_depth");
    assert_eq!(
        undo_after_binding,
        before_undo_depth + 1.0,
        "binding is an edit: exactly one entry"
    );

    let begin = dispatch(
        &mut engine,
        serde_json::json!({ "type": "BeginDrag", "node_id": node }),
    );
    assert_eq!(begin["status"], "ok", "{begin}");
    for x in [40.0, 120.0, 260.0] {
        dispatch(
            &mut engine,
            serde_json::json!({ "type": "UpdateDrag", "node_id": node, "x": x, "y": 0.0 }),
        );
    }
    // Mid-drag the pointer wins: the slot resolves to the pointer's position and
    // its *source* is a plain literal — the spring is not consulted at all.
    let dragging = snapshot(&mut engine);
    let position = dragging["scene"]["nodes"][&node]["position"].clone();
    assert_eq!(
        position["x"], 260.0,
        "the pointer owns the slot: {position}"
    );
    assert_eq!(
        position["x_source"], "literal",
        "…by replacing the binding for the duration of the gesture: {position}"
    );

    let end = dispatch(
        &mut engine,
        serde_json::json!({ "type": "EndDrag", "node_id": node }),
    );
    assert_eq!(end["status"], "ok", "{end}");

    // Continuity across the hand-back: the value just before release and just
    // after it must agree. A spring that re-anchored anywhere else would show up
    // here as a jump — the visual glitch the Interaction Precedence Law exists to
    // forbid.
    let committed = snapshot(&mut engine)["scene"]["nodes"][&node]["position"]["x"]
        .as_f64()
        .expect("x");

    // Released: the slot is animated again — and anchored at the committed value.
    let report = motion(&engine);
    let x_binding = report["bindings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|b| b["node_id"] == serde_json::json!(node) && b["property"] == "x")
        .unwrap_or_else(|| panic!("x binding gone: {report}"))
        .clone();
    assert_eq!(x_binding["kind"], "spring", "the spring survived the drag");
    assert_eq!(
        x_binding["from"], 260.0,
        "re-anchored at the committed value: {x_binding}"
    );
    let released = snapshot(&mut engine);
    assert_eq!(
        released["scene"]["nodes"][&node]["position"]["x_source"], "animated",
        "…and the slot is animated again, not a frozen literal: {released}"
    );
    let after_release = released["scene"]["nodes"][&node]["position"]["x"]
        .as_f64()
        .expect("x after release");
    assert!(
        (after_release - committed).abs() < 1e-9,
        "the release is continuous: {committed} → {after_release}"
    );

    // Undo the gesture: back to the binding as it was before the pointer.
    let undo = json(&engine.undo());
    assert_eq!(undo["status"], "ok", "{undo}");
    let restored = motion(&engine);
    let restored = restored["bindings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|b| b["node_id"] == serde_json::json!(node) && b["property"] == "x")
        .expect("x binding after undo")
        .clone();
    assert_eq!(
        restored["from"], 0.0,
        "undo restored the pre-gesture anchor: {restored}"
    );

    // Redo replays the re-anchor *as a binding*, not as a literal.
    let redo = json(&engine.redo());
    assert_eq!(redo["status"], "ok", "{redo}");
    let again = motion(&engine);
    let again = again["bindings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|b| b["node_id"] == serde_json::json!(node) && b["property"] == "x")
        .expect("x binding after redo")
        .clone();
    assert_eq!(again["kind"], "spring", "redo kept the binding: {again}");
    assert_eq!(again["from"], 260.0, "and the committed anchor: {again}");
}

// ── Law 5: State ──────────────────────────────────────────────────────────

/// State flags are **inputs**: they arrive from the host, they are never
/// document state, and a flip re-anchors the springs that read the flag at the
/// value they hold *at that instant* — which is what makes a hover spring ease
/// instead of jumping.
#[test]
fn law_states_are_inputs_and_hover_drives_them() {
    let mut engine = VectraEngine::new();
    let mut renderer = renderer_at(800.0, 600.0);
    let node = create_rect(&mut engine, "A", 100.0, 100.0, 200.0, 120.0);
    let bind = json(&engine.bind_hover_spring(&node, "width", 200.0, 320.0, 170.0, 26.0));
    assert_eq!(bind["status"], "ok", "{bind}");

    // At rest: the off value.
    assert!((bound_value(&engine, &node) - 200.0).abs() < 1e-9);
    // The hit index belongs to the *drawn* scene (RULE 3: the renderer owns
    // spatial indexing), so a canvas that has never drawn has nothing to hover.
    // This is the real mount order: React draws, then the user moves a pointer.
    let cold = json(&engine.pointer_move(&mut renderer, 200.0, 160.0));
    assert_eq!(
        cold["events"].as_array().map(|e| e.len()),
        Some(0),
        "no drawn frame yet ⇒ nothing to hover: {cold}"
    );
    let _ = frame(&mut engine, &mut renderer);

    // Pointer away from the shape: no hover, no flip, no events.
    let outside = json(&engine.pointer_move(&mut renderer, 700.0, 500.0));
    assert_eq!(
        outside["events"].as_array().map(|e| e.len()),
        Some(0),
        "the pointer is not over the shape: {outside}"
    );

    // Pointer over it: the flag flips, the spring is re-anchored at the value it
    // currently holds, and motion resumes toward the on value.
    set_time(&mut engine, 0.5);
    let over = json(&engine.pointer_move(&mut renderer, 200.0, 160.0));
    assert_eq!(over["status"], "ok", "{over}");
    assert!(
        over["events"]
            .as_array()
            .expect("events")
            .iter()
            .any(|e| e["type"] == "Dirty"),
        "a flip publishes a dirty set: {over}"
    );
    assert!(engine.is_animating(), "and the transition animates");

    // Half a second later it is on its way — strictly between the two values, and
    // *from* the off value (the re-anchor) rather than from the on value.
    set_time(&mut engine, 0.6);
    let mid = bound_value(&engine, &node);
    assert!(
        mid > 200.0 && mid < 320.0,
        "easing, not jumping: {mid} (off 200, on 320)"
    );

    // Leave: it eases back.
    set_time(&mut engine, 3.0);
    assert!((bound_value(&engine, &node) - 320.0).abs() < 1e-2);
    let leave = json(&engine.pointer_move(&mut renderer, 700.0, 500.0));
    assert!(
        leave["events"]
            .as_array()
            .expect("events")
            .iter()
            .any(|e| e["type"] == "Dirty"),
        "{leave}"
    );
    set_time(&mut engine, 3.05);
    let easing_back = bound_value(&engine, &node);
    assert!(
        easing_back < 320.0 && easing_back > 200.0,
        "easing back to rest: {easing_back}"
    );
    set_time(&mut engine, 6.0);
    assert!((bound_value(&engine, &node) - 200.0).abs() < 1e-2);
    assert!(!engine.is_animating(), "and it stops");
}

// ── Law 6: Undo Isolation ─────────────────────────────────────────────────

/// **Motion samples are not history.** Scrubbing, animating, hovering, hovering
/// away and re-evaluating produce *zero* undo entries — only the binding itself
/// is an edit. If a hover could be undone, Ctrl-Z would fight the pointer.
#[test]
fn law_motion_samples_are_not_history() {
    let mut engine = VectraEngine::new();
    let mut renderer = renderer_at(800.0, 600.0);
    let node = create_rect(&mut engine, "A", 100.0, 100.0, 200.0, 120.0);
    json(&engine.bind_hover_spring(&node, "width", 200.0, 320.0, 170.0, 26.0));

    let depth = |engine: &mut VectraEngine| number(&snapshot(engine), "undo_depth");
    let baseline = depth(&mut engine);

    // 120 frames of scrubbing + a hover in and out + a full evaluation.
    for step in 0..120 {
        let t = (step as f64 * 0.01) % 1.0;
        set_time(&mut engine, t);
        engine.is_animating();
        if step == 40 {
            let _ = json(&engine.pointer_move(&mut renderer, 200.0, 160.0));
        }
        if step == 80 {
            let _ = json(&engine.pointer_move(&mut renderer, 700.0, 500.0));
        }
    }
    let _ = json(&engine.force_full_evaluation());
    let _ = frame(&mut engine, &mut renderer);
    let _ = frame(&mut engine, &mut renderer);

    assert_eq!(
        depth(&mut engine),
        baseline,
        "{} clock/state frames created history",
        120
    );

    // The binding *is* an edit, and one undo removes exactly it.
    let redo_before = number(&snapshot(&mut engine), "redo_depth");
    let undo = json(&engine.undo());
    assert_eq!(undo["status"], "ok", "{undo}");
    assert_eq!(depth(&mut engine), baseline - 1.0);
    assert_eq!(
        number(&snapshot(&mut engine), "redo_depth"),
        redo_before + 1.0
    );
    // …and with the binding gone the slot is what it was: a literal 200.
    let report = motion(&engine);
    assert!(
        report["bindings"]
            .as_array()
            .map(|b| b.is_empty())
            .unwrap_or(false),
        "undo removed the binding: {report}"
    );

    // Track CRUD rides the same stack (the other half of the same law).
    let track = serde_json::json!({
        "id": "intro", "name": "Intro",
        "channels": { "x": [{ "time": 0.0, "value": 0.0 }, { "time": 1.0, "value": 50.0 }] }
    });
    // `baseline` is "creation + binding"; the undo above removed the binding, so
    // right now the depth is `baseline - 1` and each track edit adds exactly one.
    assert_eq!(depth(&mut engine), baseline - 1.0, "after the binding undo");
    let added = json(&engine.set_motion_track(&track.to_string()));
    assert_eq!(added["status"], "ok", "{added}");
    assert_eq!(
        depth(&mut engine),
        baseline,
        "a track registration is one entry"
    );
    let removed = json(&engine.remove_motion_track("intro"));
    assert_eq!(removed["status"], "ok", "{removed}");
    assert_eq!(
        depth(&mut engine),
        baseline + 1.0,
        "…and the removal is another"
    );
}

// ── Law 7: Frame Budget ───────────────────────────────────────────────────

/// **An animating scene costs the slot that moves.** While a spring runs, each
/// frame uploads one instance row for the bound node and nothing else: no
/// re-tessellation (the shape's vertices are node-local, so a width change is a
/// reshape… see the assertion below), no other node, no full rebuild.
///
/// The interesting half is the idle case: with everything at rest, the frame
/// loop is allowed to stop, and *if* the host draws anyway the frame costs
/// nothing at all.
#[test]
fn law_an_animating_scene_costs_only_the_slot_that_moves() {
    let mut engine = VectraEngine::new();
    let mut renderer = renderer_at(800.0, 600.0);
    let animated = create_rect(&mut engine, "A", 0.0, 0.0, 100.0, 50.0);
    let still = create_rect(&mut engine, "B", 400.0, 0.0, 20.0, 20.0);
    let _ = json(&engine.bind_spring(&animated, "x", 300.0, 170.0, 26.0));

    let first = frame(&mut engine, &mut renderer);
    assert_eq!(number(&first, "nodes"), 2.0, "{first}");

    // Motion on `x`: position only ⇒ one 80-byte instance write per frame, and
    // the untouched node is never in the plan.
    let mut wrote_any = 0;
    for step in 1..=8 {
        set_time(&mut engine, step as f64 * 0.05);
        let drawn = frame(&mut engine, &mut renderer);
        assert_eq!(number(&drawn, "moved"), 1.0, "step {step}: {drawn}");
        assert_eq!(number(&drawn, "writes"), 1.0, "step {step}: {drawn}");
        assert_eq!(number(&drawn, "bytes"), 80.0, "step {step}: {drawn}");
        assert_eq!(
            number(&drawn, "retessellated"),
            0.0,
            "a translated node keeps its vertices: {drawn}"
        );
        assert_eq!(number(&drawn, "nodes"), 2.0, "B is never dropped: {drawn}");
        wrote_any += number(&drawn, "writes") as usize;
    }
    assert_eq!(wrote_any, 8, "one write per animated frame");

    // Idle: no clock movement, no writes — the loop may stop, and if the host
    // draws anyway it is free.
    // Five idle frames. Note the convention on the wire: an *empty* dirty set
    // means "no restriction" — `DirtySet` represents "everything" as the empty
    // set — so an idle frame reports `full: true` while writing nothing at all.
    // That is the frame budget in one line: a full reconcile that finds no
    // change costs zero bytes (Task 5.0's "reconcile, never rebuild").
    for _ in 0..5 {
        let idle = frame(&mut engine, &mut renderer);
        assert_eq!(number(&idle, "writes"), 0.0, "{idle}");
        assert_eq!(number(&idle, "bytes"), 0.0, "{idle}");
        assert_eq!(number(&idle, "dirty"), 0.0, "{idle}");
        assert_eq!(number(&idle, "touched"), 0.0, "{idle}");
    }
    // **K nodes moving by position alone ⇒ K × 64 B per frame.** Two springs,
    // two instance rows, no re-tessellation: the Frame Budget Law's exact bound.
    let _ = json(&engine.bind_spring(&still, "y", 200.0, 170.0, 26.0));
    let _ = frame(&mut engine, &mut renderer); // the bind's own write
    set_time(&mut engine, 1.5);
    let two = frame(&mut engine, &mut renderer);
    assert_eq!(number(&two, "moved"), 2.0, "both springs wrote: {two}");
    assert_eq!(number(&two, "writes"), 2.0, "{two}");
    assert_eq!(number(&two, "bytes"), 160.0, "2 × 64 B: {two}");
    assert_eq!(number(&two, "retessellated"), 0.0, "{two}");

    // A *reshape* mid-animation is a re-tessellation — of that node alone.
    let reshape = json(&engine.bind_spring(&animated, "width", 260.0, 170.0, 26.0));
    assert_eq!(reshape["status"], "ok", "{reshape}");
    set_time(&mut engine, 1.6);
    let resized = frame(&mut engine, &mut renderer);
    assert!(
        number(&resized, "retessellated") >= 1.0,
        "a width change re-tessellates: {resized}"
    );
    assert_eq!(
        number(&resized, "nodes"),
        2.0,
        "and drops nobody: {resized}"
    );
    assert!(
        number(&resized, "bytes") < 4096.0,
        "one shape's vertices, not the scene: {resized}"
    );

    // Deleting a node withdraws *its* bindings and nobody else's…
    let removed = dispatch(
        &mut engine,
        serde_json::json!({ "type": "DeleteNode", "id": animated }),
    );
    assert_eq!(removed["status"], "ok", "{removed}");
    let survivors = motion(&engine)["bindings"]
        .as_array()
        .expect("bindings")
        .clone();
    assert!(
        survivors
            .iter()
            .all(|b| b["node_id"] == serde_json::json!(still)),
        "only the deleted node's bindings went away: {survivors:?}"
    );
    assert!(!survivors.is_empty(), "the other spring is still bound");

    // …and with every spring arrived, the whole document is idle.
    set_time(&mut engine, 20.0);
    assert!(!engine.is_animating(), "everything has arrived");
    let _ = dispatch(
        &mut engine,
        serde_json::json!({ "type": "DeleteNode", "id": still }),
    );
    assert!(motion(&engine)["bindings"].as_array().unwrap().is_empty());
    assert!(!engine.is_animating(), "nothing bound ⇒ nothing animates");
}
