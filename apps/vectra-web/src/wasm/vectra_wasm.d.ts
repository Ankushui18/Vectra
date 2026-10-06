/* tslint:disable */
/* eslint-disable */

/**
 * The WebGPU renderer the React canvas drives.
 */
export class Renderer {
    free(): void;
    [Symbol.dispose](): void;
    /**
     * Initialise WebGPU on `canvas` (Task 5.0 §6: React passes the element
     * on mount). Resolves to a JSON status string; rejects only when the
     * canvas cannot host a surface at all — a browser without WebGPU gets
     * `{"ready":false,…}` so the UI can say so instead of crashing.
     */
    attach(canvas: HTMLCanvasElement): Promise<string>;
    /**
     * **Document points → client points**, as JSON — the camera the drawing
     * overlay is placed with (Task 10.1 RULE 4).
     *
     * The overlay is DOM: anchors, Bézier handles and the in-progress path are
     * SVG sitting on the WebGPU canvas. They are placed here, through
     * `Camera::document_to_screen` and the same CSS-box/pixel ratios
     * `client_to_document` inverts, for the same reason the pointer is read
     * here: there is one camera. A handle that disagrees with the curve beneath
     * it — and document space is **y-up** while the DOM is y-down, so a naive
     * `viewBox` overlay would be mirrored — is worse than no handle at all.
     *
     * Input `[[x, y], …]` in document units, output `[[client_x, client_y], …]`
     * in viewport pixels, so React renders without a transform of its own
     * (Task 1.4's dumb-remote rule). A canvas with no box answers
     * `{"ok":false,"points":[]}` rather than guessing.
     */
    document_to_client(points_json: string): string;
    /**
     * **Frame a document rectangle** (Task 10.2 RULE 2) — what the artboard
     * dropdown's jump and "fit" both mean.
     *
     * Pan and zoom belong to the camera, and the camera belongs to the renderer,
     * so this is where the whole feature lives: the UI hands over the rectangle
     * it read from the engine's artboard record (document units, y-up) and gets
     * the camera back. No projection is computed anywhere else, and the
     * rectangle is remembered so a resize keeps framing it.
     *
     * Returns the visible window as JSON — the same shape `view()` reports, so
     * the overlay follows for free.
     */
    frame_document(x: number, y: number, width: number, height: number): string;
    /**
     * Forget the framed rectangle and go back to the opening view of the whole
     * document — the "zoom to fit everything" the canvas starts in.
     */
    frame_document_default(): string;
    /**
     * Whether the GPU came up. `false` ⇒ the UI shows the reason from
     * `stats().error` and stops scheduling frames.
     */
    gpu_ready(): boolean;
    /**
     * Read the canvas box, resize the backing store, and update the camera.
     * Cheap enough to call on every resize event (and on mount).
     */
    measure(canvas: HTMLCanvasElement): string;
    /**
     * **Pan by a pointer delta in CSS pixels** — a drag with the pan tool (or a
     * middle-button drag) while looking at artwork.
     *
     * `dx`/`dy` are the pointer's own movement, +x right and +y down, because
     * that is the data a `pointermove` event carries and re-deriving the flip at
     * the call site is how signs get lost. The picture follows the pointer: the
     * document point that was under the finger stays under the finger.
     */
    nav_pan(dx: number, dy: number): string;
    /**
     * The current zoom, in **CSS pixels per document unit** — the number the
     * zoom readout prints as a percentage.
     *
     * CSS pixels, not device pixels: the readout is about what the user sees and
     * what the pointer does, and a 2× display must not claim the artwork is at
     * 200%. `pixels_per_unit` is the inverse of this for the tools' grab radii.
     */
    nav_scale(): number;
    /**
     * **Zoom about a CSS-pixel position** — one notch of a wheel or a trackpad
     * pinch. `factor > 1` zooms in.
     *
     * The position matters: the document point under the pointer is the one the
     * user is looking at, so it is the one that must not move. That invariance
     * is `Camera::zoom_at`'s definition and law 2 of the navigation laws.
     */
    nav_zoom(client_x: number, client_y: number, factor: number): string;
    /**
     * **Zoom to an absolute scale** about the middle of the canvas — the "100%",
     * "200%" and "fit width" controls, and the keyboard shortcuts that mean the
     * same thing. Absolute rather than relative so a button and its label can
     * never disagree about what "%" means.
     *
     * `scale` is **CSS pixels per unit** — the unit `nav_scale` reports and the
     * unit the buttons are labelled in — so the conversion to the camera's
     * device scale happens here, at the same boundary every other CSS number in
     * this file crosses. Omitting it was a real defect, caught by the dpr-1.5
     * case of the readout law: "100%" was 66.7% on a 1.5x display.
     */
    nav_zoom_to(scale: number): string;
    /**
     * A renderer with no GPU: the spatial index and the view transform work
     * immediately, `attach` brings the pixels up. Nothing panics if the browser
     * has no WebGPU — the UI shows what the frame report says.
     */
    constructor();
    /**
     * **Document units per screen pixel** — the camera's scale, inverted.
     *
     * The drawing tools need it for one reason: a grab radius (a handle must be
     * as easy to hit at 4× zoom as at 1×) and a stroke width (the brush's
     * `min_distance` filter should not change meaning when the user zooms). It is
     * derived from the same `Camera::visible` window the renderer draws with, so
     * an overlay can never be calibrated differently from the scene beneath it.
     */
    pixels_per_unit(): number | undefined;
    /**
     * The document point under a client-space pointer position, as JSON
     * (`{"x":…,"y":…}`). This is a *view* transform; it reads no parameter and
     * no rule — the drag protocol needs it only to keep the grab offset while
     * the pointer moves.
     */
    pointer_doc(client_x: number, client_y: number): string;
    /**
     * RULE 3: a client-space pointer position → the node under it, or `null`.
     *
     * The whole chain — client box → canvas pixels → document units → spatial
     * index → exact containment — happens here, in Rust, using the *same*
     * camera that projected the pixels. That is what makes the pointer agree
     * with the picture, and what keeps the UI a dumb remote: it reports where
     * the mouse is, the engine decides what that means.
     */
    pointer_hit(client_x: number, client_y: number): string | undefined;
    /**
     * The viewport, from the DOM's own numbers. Returns the visible document
     * rectangle as JSON (the browser gets the camera back, nothing else).
     */
    set_viewport(left: number, top: number, css_width: number, css_height: number, scale: number): string;
    /**
     * The last frame report (plus the accumulated GPU totals) as JSON.
     */
    stats(): string;
    /**
     * The camera the frame is drawn with: `[min_x, min_y, width, height]` in
     * document units, as JSON.
     */
    view(): string;
}

export class VectraEngine {
    free(): void;
    [Symbol.dispose](): void;
    /**
     * Apply a plan that was already previewed and approved.
     *
     * No planner, so no retry: the plan runs exactly as shown, or it is refused
     * (and rolled back) with the engine's own message. The AI panel's *Execute*
     * with auto-correct switched off.
     */
    ai_execute_commands(prompt: string, plan_json: string): string;
    /**
     * The whole loop (RULE 3): generate, validate through dispatch, feed a
     * refusal back, self-correct up to two times, then report.
     *
     * Returns `{"status":"ok","report":{…}}` or
     * `{"status":"error","code":"max-retries-exceeded","message":…,"corrections":[…]}`
     * — with nothing applied in the failure case.
     */
    ai_execute_with_retry(prompt: string, summary_json: string): string;
    /**
     * Generate commands from a prompt, **without applying anything** — the AI
     * panel's preview.
     *
     * `summary_json` is the summary the model should be grounded on (the panel
     * passes back what [`VectraEngine::document_summary`] gave it, so the UI
     * computes nothing). An empty string means "use the live document", which
     * is what a headless caller wants.
     *
     * Returns `{"status":"ok","plan":[…],"notes":[…],"attempt":1}` or
     * `{"status":"error","code":…,"message":…}`.
     */
    ai_generate_commands(prompt: string, summary_json: string): string;
    /**
     * The phrasings the built-in planner understands, for the panel's hint
     * line. The list lives beside the matcher in `vectra-ai`, so a hint can
     * never advertise something the planner cannot do.
     */
    ai_phrasings(): string;
    /**
     * The **system prompt** the MVP planner is written for, with this document
     * already injected — the AI panel shows it, and a hosted model would be
     * called with exactly this text.
     */
    ai_prompt(): string;
    /**
     * The hover demo, engine-side: a spring whose target is the `off`/`on` pair
     * selected by the per-node state flag `hover:<node id>`.
     *
     * The flag is *derived from the document* here rather than sent by the UI,
     * so the UI stays a dumb remote: it reports where the pointer is
     * ([`VectraEngine::pointer_move`]) and never decides what that means.
     */
    bind_hover_spring(node_id: string, property: string, off: number, on: number, stiffness: number, damping: number): string;
    /**
     * Bind a spring to a slot, anchored at the slot's **current** value and the
     * current clock (MES §12).
     *
     * The anchor is written *by the engine*, not guessed by the UI: only the
     * engine can resolve the slot (it may be driven by a variable, an
     * expression, a constraint or another binding right now), and `at` must be
     * the engine's clock, not the browser's. The UI sends two numbers and a
     * target; everything else is document state.
     *
     * Undoable — binding is an edit (the inverse restores whatever the slot
     * held: a literal, a variable reference or a previous binding).
     */
    bind_spring(node_id: string, property: string, target: number, stiffness: number, damping: number): string;
    /**
     * The dependency graph, for the inspector panel.
     */
    dependencies(): string;
    /**
     * Execute a [`Command`] JSON string. Returns a [`CommandResponse`] JSON string.
     *
     * `DefineExpression` sources are pre-validated (test-compiled) and every
     * edge-adding command is dry-run through the dependency graph BEFORE
     * dispatch: an invalid expression or a cycle fails with a typed error and
     * *zero* mutation — no record, no undo entry, no graph edge.
     */
    dispatch_command(cmd_json: string): string;
    /**
     * The document itself, as JSON (Task 10.0).
     *
     * **The one method Task 10.0 added to this boundary, and it is read-only:**
     * it serializes what the engine is holding, verbatim, with the same `serde`
     * that produced the file the document was loaded from. Nothing here
     * interprets, resolves or normalizes — a parametric slot still reads
     * `{"Expression":"<id>"}`, a spring is still a spring, an operation is
     * still virtual. The native `.vectra` codec (`vectra-file`) takes the string
     * from here and gzips it.
     *
     * It was added because Task 10.0's RULE 3 requires the frontend to hand the
     * raw `Document` JSON to the save path, and the alternative — rebuilding a
     * document natively *outside* the engine — would have meant a second copy of
     * the settle pipeline, i.e. two engines that could disagree about a file.
     * Loading does **not** come back through here: a file is replayed as
     * commands through `dispatch_command`, so the engine's own gates validate
     * every loaded document.
     */
    document_json(): string;
    /**
     * The AI-facing document summary (RULE 2), as JSON.
     *
     * The engine's own view, resolved through the live evaluators, so an
     * expression slot reads `$base * 2` and a procedural read its published
     * value. This is what a planner is grounded on and what `ai_prompt` embeds.
     */
    document_summary(): string;
    /**
     * Commit the brush's stroke: fit it, expand it, and put it in the document.
     * `_node_id` is accepted for symmetry with the pen's commit (rewriting an
     * existing path); a brush gesture always produces a new node, because the
     * stroke *is* the gesture — the workflow that edits an existing path is the
     * direct-selection tool's.
     */
    draw_brush_commit(_node_id?: string | null): string;
    /**
     * **Begin a direct-selection drag** (RULE 4): the path is about to be edited.
     *
     * Opens a gesture that owns the path until `draw_edit_end`. These three
     * methods are the anchor/handle counterpart of Task 3.2's drag triad, and
     * they exist for the same reason: *one gesture is one undo step*.
     * `draw_edit_command` alone would push a history entry per pointer sample, so
     * dragging an anchor across the canvas would take two hundred undos to take
     * back.
     */
    draw_edit_begin(node_id: string, slot: string, side: string, anchor: boolean): string;
    /**
     * Abandon a direct-selection drag, restoring the path it started from.
     */
    draw_edit_cancel(): string;
    /**
     * **The command that moves an anchor or handle** (RULE 4).
     *
     * Returned rather than dispatched so the UI can put it through the *same*
     * `dispatch_command` door as everything else (undo, events, dirty nodes,
     * renderer updates all come for free) — and so the command log in the UI
     * shows the drag, which is the Task 1.4 rule taken seriously.
     *
     * ## Symmetry, in the engine
     *
     * Dragging a *smooth* anchor's handle must move the opposite handle too, and
     * an Alt-drag must not (RULE 2). That decision is `vectra_draw::pen::drag_handle`
     * — the very function the Handle Symmetry and Handle Independence laws
     * exercise — and the result is one `Batch`, so the whole symmetric edit is a
     * single undo entry.
     * **The command that moves an anchor or handle** (RULE 4).
     *
     * Returned rather than dispatched so the UI can put it through the *same*
     * `dispatch_command` door as everything else (undo, events, dirty nodes,
     * renderer updates all come for free) — and so the command log in the UI
     * shows the drag, which is the Task 1.4 rule taken seriously.
     *
     * It is the stateless form of the same edit `draw_edit_update` applies during
     * a gesture: both go through [`edit_batch`], so a handle drag the gesture
     * previews and one the UI dispatches by hand cannot differ.
     *
     * ## Symmetry, in the engine
     *
     * Dragging a *smooth* anchor's handle must move the opposite handle too, and
     * an Alt-drag must not (RULE 2). That decision is `vectra_draw::pen::drag_handle`
     * — the very function the Handle Symmetry and Handle Independence laws
     * exercise — and the result is one `Batch`, so the whole symmetric edit is a
     * single undo entry.
     */
    draw_edit_command(node_id: string, slot: string, side: string, x: number, y: number, alt: boolean, anchor: boolean): string;
    /**
     * **Finish a direct-selection drag**: one `Set path` entry, exact in both
     * directions — and it reverts everything the solver did for the gesture too.
     *
     * The entry is a `SetPath` pair rather than a list of parameter writes, and
     * that is the whole trick: the path before the gesture was captured when it
     * opened, the path after it is read off the document now, and the two are
     * exact inverses — so **one undo gets the shape back** no matter how many
     * components the drag touched, how many samples the pointer produced, or what
     * the constraint solver did in between.
     */
    draw_edit_end(): string;
    /**
     * **One sample of a direct-selection drag**, applied and settled.
     *
     * Nothing is pushed onto the undo stack here — the gesture is one step, and
     * `draw_edit_end` records it (see [`EditSession`]). What *does* happen here is
     * everything else a user can see: the parameter writes, the constraint pass
     * (a `Required` row stronger than the pointer wins, exactly as in Task 3.1),
     * the dirty set and the scene patch.
     */
    draw_edit_update(x: number, y: number, alt: boolean): string;
    /**
     * **The direct-selection hit test** (RULE 4).
     *
     * Given a pointer position *in client space* and the renderer that owns the
     * camera, returns the nearest anchor or handle of the addressed path, with
     * the slot a command would write. The radius is in screen pixels and
     * converted to document units through the camera, so a handle stays as easy
     * to grab at 4× zoom as at 1× — which is the whole point of hitting in screen
     * space rather than in the document.
     *
     * Anchors win over handles at equal distance: the anchor is what a designer
     * means when they click "here".
     */
    draw_hit(renderer: Renderer, node_id: string, x: number, y: number): string;
    /**
     * The anchors and handles of a path, for the direct-selection overlay.
     *
     * Engine data, in document space: the UI hands it to the renderer's own
     * camera to place it on screen, so an overlay can never disagree with the
     * geometry it decorates.
     */
    draw_overlay(node_id: string): string;
    /**
     * **Commit the pen's draft** as a path node.
     *
     * `new_node` decides create-versus-rewrite: pass an existing node id to keep
     * editing a shape (the direct-selection workflow), or `None` for a fresh
     * drawn path. Returns the engine's own command response, so the UI's event
     * log, undo stack and selection state behave exactly as for a hand-authored
     * command.
     */
    draw_pen_commit(close: boolean, node_id?: string | null): string;
    /**
     * **A pen or brush pointer event** (Task 10.1 RULE 2 / RULE 3).
     *
     * `tool` is `"pen"` or `"brush"`. For the pen, the gesture vocabulary is
     * `vectra_draw::PenSession`'s, so `alt` breaks handle symmetry and `close`
     * asks for the path to be joined to its first point. The reply carries the
     * draft, so the UI's canvas has exactly the geometry the engine has — no
     * interpolation, no duplicated maths.
     */
    draw_pointer(tool: string, kind: string, x: number, y: number, alt: boolean, close: boolean, time: number): string;
    /**
     * **Quick Shape snapping** (RULE 3).
     *
     * The whole operation, in one call, because it must be atomic from the user's
     * point of view — they held still for a moment and their blob became a
     * circle:
     *
     * 1. `plan_quick_shape` recognizes the primitive and fits it;
     * 2. the path is rewritten to the ideal geometry (`SetPath`);
     * 3. the primitive node is created (`CreateNode`), so the artwork carries a
     *    real `Circle`/`Rectangle` — which the SVG exporter turns into `<circle>`
     *    (Task 8.0 RULE 1) rather than a path;
     * 4. the constraint rows are added (`AddConstraint`), which is where the
     *    Task 3.1 solver runs: every `AddConstraint` goes through
     *    `dispatch_command`'s constraint pass, so the snaps are enforced by the
     *    same tableau as any hand-authored rule.
     *
     * `stroke` is the rough input — the UI's cleaned sample points for a brush
     * gesture, or the drawn anchors for a pen path. Returns a reply naming the
     * primitive and the input's own distance from it.
     */
    draw_quick_shape(node_id: string, stroke_json: string): string;
    /**
     * **The tools' settings** — the brush's curve-fit tolerance and the pen's
     * pick radius are *engine* decisions (they are compared against document
     * geometry), so they are set here rather than in the UI.
     */
    draw_set_tolerance(tolerance: number): string;
    /**
     * **Export all artboards** (Task 10.2 RULE 2): every board, laid out where
     * it sits in document space, so the file reads as one canvas with frames on
     * it — the same arrangement the GPU canvas shows.
     */
    export_all_artboards(): string;
    /**
     * **Export current artboard** (Task 10.2 RULE 2): the SVG of the board the
     * designer is on, cropped to its frame and painted with its background.
     *
     * The document decides what belongs to the board (its layers' nodes); the
     * exporter decides what the file looks like. Nothing here filters geometry
     * by hand — the same IR, the same element table, one extra frame.
     */
    export_current_artboard(): string;
    /**
     * The drawing as a **parametric React component** (Task 8.0, RULE 2).
     *
     * A slot driven by `$base * 2` comes out as `width={base * 2}` with `base`
     * declared as a required prop: the same picture, as code that goes on taking
     * arguments.
     */
    export_to_react(): string;
    /**
     * The drawing as a **semantic SVG document** (Task 8.0, RULE 1).
     *
     * The picture is compiled from the document *and* the scene the engine is
     * currently drawing (`compile_to_ir_resolved`), so motion is exported at the
     * instant the user clicked, and a procedural result exports as the region it
     * computed — never as an approximation of a primitive it is not.
     *
     * Returns `{"status","format","code","warnings"}`: the UI shows `code`, and
     * `warnings` says why an exported number is not the authored one (a clamp, a
     * spring, an unresolved variable).
     */
    export_to_svg(): string;
    /**
     * Rebuild the whole scene from scratch (the UI's "re-evaluate everything"
     * control). Demonstrably equal to the incremental cache — the app-level
     * witness of `patch ≡ rebuild`.
     */
    force_full_evaluation(): string;
    /**
     * Project the live engine into a [`snapshot::SnapshotResponse`] JSON string.
     *
     * The first call warms the scene cache (one full pass); every later call is
     * a pure projection — evaluation already happened during the mutation that
     * dirtied it.
     */
    get_snapshot(): string;
    /**
     * **The idle signal** (Frame Budget Law): `false` means the document is at
     * rest at the current clock, so the React frame loop must stop scheduling
     * `render_frame` until something moves again.
     */
    is_animating(): boolean;
    /**
     * Motion state for the inspector and the smoke harness: whether anything is
     * moving, when it will stop, and what is bound where.
     */
    motion_json(): string;
    /**
     * Initialize a fresh engine.
     */
    constructor();
    /**
     * The pointer left the canvas: nothing is hovered any more.
     *
     * A separate entry point rather than a magic off-canvas coordinate, because
     * "the pointer is not on the canvas" is not a *place* — the UI would have to
     * invent a document point to say it, and the engine would have to trust the
     * invention.
     */
    pointer_leave(): string;
    /**
     * Report the pointer's document-space position (Task 6.0's hover driver).
     *
     * The engine asks the renderer's spatial index who is under the pointer,
     * decides which nodes are hovered, and flips exactly their `hover:<id>`
     * flags — the UI never learns which node is under the cursor unless it asks
     * the snapshot, and never computes it. Returns the events for any flips
     * (usually empty).
     *
     * The hit index describes the **last drawn** scene (spatial indexing is the
     * renderer's, RULE 3), so a canvas that has never drawn has nothing to
     * hover — which is exactly the mount order React uses: draw, then follow
     * the pointer.
     */
    pointer_move(renderer: Renderer, x: number, y: number): string;
    /**
     * The procedural graph, for the Procedural panel and the smoke harness.
     *
     * Three things the panel cannot get anywhere else: the registry's own
     * order (a chain has a direction, a JSON object does not), each operand's
     * **effective** value (`(variable)`, not the number the template started
     * with — see `ProceduralNode::describe`), and the value each output port
     * last published.
     */
    procedural_json(): string;
    /**
     * **The palette**: every kind the engine can build, with its ports and the
     * operand payload an `AddProceduralNode` must carry.
     *
     * The panel owns no table of node kinds, port names, or default numbers: it
     * lists what this returns and forwards the operand payload **verbatim**.
     * That is the same discipline as the command builders — if the engine
     * changes a default, the panel changes with it, because the panel never had
     * a copy.
     *
     * `needs_subject` marks the kind whose subject is *part of the kind* rather
     * than an operand (`Source { node }`): the panel offers a layer picker for
     * those, and sends `{"type": "source", "node": "…"}`.
     */
    procedural_kinds(): string;
    /**
     * Redo one step. Returns a [`CommandResponse`] JSON string.
     */
    redo(): string;
    /**
     * Remove a track by id. Undoable; a slot still bound to it fails to resolve
     * until the track returns, and that failure is visible in the event log
     * rather than silently rendering a zero.
     */
    remove_motion_track(track_id: string): string;
    /**
     * **One canvas frame** (Task 5.0 §6).
     *
     * The engine hands the renderer the evaluated scene plus the ids that
     * changed since the last frame; the renderer tessellates what changed,
     * writes only the affected buffer slices, and draws. React schedules this
     * once per animation frame and never inspects a buffer.
     *
     * Returns the frame report as JSON — what the engine asked for (`dirty`,
     * `full`) versus what the GPU actually paid (`writes`, `bytes`, `moved`,
     * `restyled`). A cold scene is settled first, so the first frame after
     * mount already draws the document as it stands.
     */
    render_frame(renderer: Renderer): string;
    /**
     * Register or replace a keyframe track (undoable document state).
     *
     * Takes the track as JSON so the wire schema lives in one place
     * (`MotionTrack`'s own `Serialize`/`Deserialize`), exactly like commands.
     */
    set_motion_track(track_json: string): string;
    /**
     * Set a state flag — a host **input**, in the same category as the pointer
     * position and the clock. Never undoable, never in the history.
     *
     * On a genuine *flip* (not a redundant re-write) every spring that reads the
     * flag is re-anchored at the value it currently holds, which is what makes a
     * hover transition animate: the spring starts from where it is *now* and
     * eases to the branch's new target. The re-anchor is a session write (zero
     * history entries: Undo Isolation Law) applied through the same
     * `BindMotion` command a redo would replay.
     *
     * Returns the engine events for the flip (a `Dirty` event naming the
     * re-anchored nodes, or nothing at all if the flag was already set).
     */
    set_state(name: string, on: boolean): string;
    /**
     * Scrub animation time: dirty from the clock vertex and re-evaluate its
     * readers (Phase 4 motion rides this exact path).
     */
    set_time(t: number): string;
    /**
     * Undo one step. Returns a [`CommandResponse`] JSON string.
     *
     * The command about to be applied (the previous entry's *inverse*) is
     * dry-run first, so rewinding cannot introduce a cycle either.
     */
    undo(): string;
}

export type InitInput = RequestInfo | URL | Response | BufferSource | WebAssembly.Module;

export interface InitOutput {
    readonly memory: WebAssembly.Memory;
    readonly __wbg_renderer_free: (a: number, b: number) => void;
    readonly __wbg_vectraengine_free: (a: number, b: number) => void;
    readonly renderer_attach: (a: number, b: any) => any;
    readonly renderer_document_to_client: (a: number, b: number, c: number) => [number, number];
    readonly renderer_frame_document: (a: number, b: number, c: number, d: number, e: number) => [number, number];
    readonly renderer_frame_document_default: (a: number) => [number, number];
    readonly renderer_gpu_ready: (a: number) => number;
    readonly renderer_measure: (a: number, b: any) => [number, number, number, number];
    readonly renderer_nav_pan: (a: number, b: number, c: number) => [number, number];
    readonly renderer_nav_scale: (a: number) => number;
    readonly renderer_nav_zoom: (a: number, b: number, c: number, d: number) => [number, number];
    readonly renderer_nav_zoom_to: (a: number, b: number) => [number, number];
    readonly renderer_new: () => number;
    readonly renderer_pixels_per_unit: (a: number) => [number, number];
    readonly renderer_pointer_doc: (a: number, b: number, c: number) => [number, number];
    readonly renderer_pointer_hit: (a: number, b: number, c: number) => [number, number];
    readonly renderer_set_viewport: (a: number, b: number, c: number, d: number, e: number, f: number) => [number, number];
    readonly renderer_stats: (a: number) => [number, number];
    readonly renderer_view: (a: number) => [number, number];
    readonly vectraengine_ai_execute_commands: (a: number, b: number, c: number, d: number, e: number) => [number, number];
    readonly vectraengine_ai_execute_with_retry: (a: number, b: number, c: number, d: number, e: number) => [number, number];
    readonly vectraengine_ai_generate_commands: (a: number, b: number, c: number, d: number, e: number) => [number, number];
    readonly vectraengine_ai_phrasings: (a: number) => [number, number];
    readonly vectraengine_ai_prompt: (a: number) => [number, number];
    readonly vectraengine_bind_hover_spring: (a: number, b: number, c: number, d: number, e: number, f: number, g: number, h: number, i: number) => [number, number];
    readonly vectraengine_bind_spring: (a: number, b: number, c: number, d: number, e: number, f: number, g: number, h: number) => [number, number];
    readonly vectraengine_dependencies: (a: number) => [number, number];
    readonly vectraengine_dispatch_command: (a: number, b: number, c: number) => [number, number];
    readonly vectraengine_document_json: (a: number) => [number, number];
    readonly vectraengine_document_summary: (a: number) => [number, number];
    readonly vectraengine_draw_brush_commit: (a: number, b: number, c: number) => [number, number];
    readonly vectraengine_draw_edit_begin: (a: number, b: number, c: number, d: number, e: number, f: number, g: number, h: number) => [number, number];
    readonly vectraengine_draw_edit_cancel: (a: number) => [number, number];
    readonly vectraengine_draw_edit_command: (a: number, b: number, c: number, d: number, e: number, f: number, g: number, h: number, i: number, j: number, k: number) => [number, number];
    readonly vectraengine_draw_edit_end: (a: number) => [number, number];
    readonly vectraengine_draw_edit_update: (a: number, b: number, c: number, d: number) => [number, number];
    readonly vectraengine_draw_hit: (a: number, b: number, c: number, d: number, e: number, f: number) => [number, number];
    readonly vectraengine_draw_overlay: (a: number, b: number, c: number) => [number, number];
    readonly vectraengine_draw_pen_commit: (a: number, b: number, c: number, d: number) => [number, number];
    readonly vectraengine_draw_pointer: (a: number, b: number, c: number, d: number, e: number, f: number, g: number, h: number, i: number, j: number) => [number, number];
    readonly vectraengine_draw_quick_shape: (a: number, b: number, c: number, d: number, e: number) => [number, number];
    readonly vectraengine_draw_set_tolerance: (a: number, b: number) => [number, number];
    readonly vectraengine_export_all_artboards: (a: number) => [number, number];
    readonly vectraengine_export_current_artboard: (a: number) => [number, number];
    readonly vectraengine_export_to_react: (a: number) => [number, number];
    readonly vectraengine_export_to_svg: (a: number) => [number, number];
    readonly vectraengine_force_full_evaluation: (a: number) => [number, number];
    readonly vectraengine_get_snapshot: (a: number) => [number, number];
    readonly vectraengine_is_animating: (a: number) => number;
    readonly vectraengine_motion_json: (a: number) => [number, number];
    readonly vectraengine_new: () => number;
    readonly vectraengine_pointer_leave: (a: number) => [number, number];
    readonly vectraengine_pointer_move: (a: number, b: number, c: number, d: number) => [number, number];
    readonly vectraengine_procedural_json: (a: number) => [number, number];
    readonly vectraengine_procedural_kinds: (a: number) => [number, number];
    readonly vectraengine_redo: (a: number) => [number, number];
    readonly vectraengine_remove_motion_track: (a: number, b: number, c: number) => [number, number];
    readonly vectraengine_render_frame: (a: number, b: number) => [number, number];
    readonly vectraengine_set_motion_track: (a: number, b: number, c: number) => [number, number];
    readonly vectraengine_set_state: (a: number, b: number, c: number, d: number) => [number, number];
    readonly vectraengine_set_time: (a: number, b: number) => [number, number];
    readonly vectraengine_undo: (a: number) => [number, number];
    readonly wasm_bindgen_2ad7bc35cc533b99___convert__closures_____invoke___js_sys_b6d8f5494c076ea6___Function_fn_wasm_bindgen_2ad7bc35cc533b99___JsValue_____wasm_bindgen_2ad7bc35cc533b99___sys__Undefined___js_sys_b6d8f5494c076ea6___Function_fn_wasm_bindgen_2ad7bc35cc533b99___JsValue_____wasm_bindgen_2ad7bc35cc533b99___sys__Undefined_______true_: (a: number, b: number, c: any, d: any) => void;
    readonly wasm_bindgen_2ad7bc35cc533b99___convert__closures_____invoke___wasm_bindgen_2ad7bc35cc533b99___JsValue__core_608f92abc48d28da___result__Result_____wasm_bindgen_2ad7bc35cc533b99___JsError___true_: (a: number, b: number, c: any) => [number, number];
    readonly wasm_bindgen_2ad7bc35cc533b99___convert__closures_____invoke___wasm_bindgen_2ad7bc35cc533b99___JsValue______true_: (a: number, b: number, c: any) => void;
    readonly wasm_bindgen_2ad7bc35cc533b99___convert__closures_____invoke___wgpu_d00bd68c658e9b86___backend__webgpu__webgpu_sys__gen_GpuUncapturedErrorEvent__GpuUncapturedErrorEvent______true_: (a: number, b: number, c: any) => void;
    readonly wasm_bindgen_2ad7bc35cc533b99___convert__closures_____invoke___bool__true_: (a: number, b: number) => number;
    readonly __wbindgen_malloc: (a: number, b: number) => number;
    readonly __wbindgen_realloc: (a: number, b: number, c: number, d: number) => number;
    readonly __wbindgen_exn_store: (a: number) => void;
    readonly __externref_table_alloc: () => number;
    readonly __wbindgen_externrefs: WebAssembly.Table;
    readonly __wbindgen_free: (a: number, b: number, c: number) => void;
    readonly __wbindgen_destroy_closure: (a: number, b: number) => void;
    readonly __externref_table_dealloc: (a: number) => void;
    readonly __wbindgen_start: () => void;
}

export type SyncInitInput = BufferSource | WebAssembly.Module;

/**
 * Instantiates the given `module`, which can either be bytes or
 * a precompiled `WebAssembly.Module`.
 *
 * @param {{ module: SyncInitInput }} module - Passing `SyncInitInput` directly is deprecated.
 *
 * @returns {InitOutput}
 */
export function initSync(module: { module: SyncInitInput } | SyncInitInput): InitOutput;

/**
 * If `module_or_path` is {RequestInfo} or {URL}, makes a request and
 * for everything else, calls `WebAssembly.instantiate` directly.
 *
 * @param {{ module_or_path: InitInput | Promise<InitInput> }} module_or_path - Passing `InitInput` directly is deprecated.
 *
 * @returns {Promise<InitOutput>}
 */
export default function __wbg_init (module_or_path?: { module_or_path: InitInput | Promise<InitInput> } | InitInput | Promise<InitInput>): Promise<InitOutput>;
