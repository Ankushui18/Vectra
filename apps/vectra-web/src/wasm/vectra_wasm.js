/* @ts-self-types="./vectra_wasm.d.ts" */

/**
 * The WebGPU renderer the React canvas drives.
 */
export class Renderer {
    __destroy_into_raw() {
        const ptr = this.__wbg_ptr;
        this.__wbg_ptr = 0;
        RendererFinalization.unregister(this);
        return ptr;
    }
    free() {
        const ptr = this.__destroy_into_raw();
        wasm.__wbg_renderer_free(ptr, 0);
    }
    /**
     * Initialise WebGPU on `canvas` (Task 5.0 §6: React passes the element
     * on mount). Resolves to a JSON status string; rejects only when the
     * canvas cannot host a surface at all — a browser without WebGPU gets
     * `{"ready":false,…}` so the UI can say so instead of crashing.
     * @param {HTMLCanvasElement} canvas
     * @returns {Promise<string>}
     */
    attach(canvas) {
        const ret = wasm.renderer_attach(this.__wbg_ptr, canvas);
        return ret;
    }
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
     * @param {string} points_json
     * @returns {string}
     */
    document_to_client(points_json) {
        let deferred2_0;
        let deferred2_1;
        try {
            const ptr0 = passStringToWasm0(points_json, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len0 = WASM_VECTOR_LEN;
            const ret = wasm.renderer_document_to_client(this.__wbg_ptr, ptr0, len0);
            deferred2_0 = ret[0];
            deferred2_1 = ret[1];
            return getStringFromWasm0(ret[0], ret[1]);
        } finally {
            wasm.__wbindgen_free(deferred2_0, deferred2_1, 1);
        }
    }
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
     * @param {number} x
     * @param {number} y
     * @param {number} width
     * @param {number} height
     * @returns {string}
     */
    frame_document(x, y, width, height) {
        let deferred1_0;
        let deferred1_1;
        try {
            const ret = wasm.renderer_frame_document(this.__wbg_ptr, x, y, width, height);
            deferred1_0 = ret[0];
            deferred1_1 = ret[1];
            return getStringFromWasm0(ret[0], ret[1]);
        } finally {
            wasm.__wbindgen_free(deferred1_0, deferred1_1, 1);
        }
    }
    /**
     * Forget the framed rectangle and go back to the opening view of the whole
     * document — the "zoom to fit everything" the canvas starts in.
     * @returns {string}
     */
    frame_document_default() {
        let deferred1_0;
        let deferred1_1;
        try {
            const ret = wasm.renderer_frame_document_default(this.__wbg_ptr);
            deferred1_0 = ret[0];
            deferred1_1 = ret[1];
            return getStringFromWasm0(ret[0], ret[1]);
        } finally {
            wasm.__wbindgen_free(deferred1_0, deferred1_1, 1);
        }
    }
    /**
     * Whether the GPU came up. `false` ⇒ the UI shows the reason from
     * `stats().error` and stops scheduling frames.
     * @returns {boolean}
     */
    gpu_ready() {
        const ret = wasm.renderer_gpu_ready(this.__wbg_ptr);
        return ret !== 0;
    }
    /**
     * Read the canvas box, resize the backing store, and update the camera.
     * Cheap enough to call on every resize event (and on mount).
     * @param {HTMLCanvasElement} canvas
     * @returns {string}
     */
    measure(canvas) {
        let deferred2_0;
        let deferred2_1;
        try {
            const ret = wasm.renderer_measure(this.__wbg_ptr, canvas);
            var ptr1 = ret[0];
            var len1 = ret[1];
            if (ret[3]) {
                ptr1 = 0; len1 = 0;
                throw takeFromExternrefTable0(ret[2]);
            }
            deferred2_0 = ptr1;
            deferred2_1 = len1;
            return getStringFromWasm0(ptr1, len1);
        } finally {
            wasm.__wbindgen_free(deferred2_0, deferred2_1, 1);
        }
    }
    /**
     * **Pan by a pointer delta in CSS pixels** — a drag with the pan tool (or a
     * middle-button drag) while looking at artwork.
     *
     * `dx`/`dy` are the pointer's own movement, +x right and +y down, because
     * that is the data a `pointermove` event carries and re-deriving the flip at
     * the call site is how signs get lost. The picture follows the pointer: the
     * document point that was under the finger stays under the finger.
     * @param {number} dx
     * @param {number} dy
     * @returns {string}
     */
    nav_pan(dx, dy) {
        let deferred1_0;
        let deferred1_1;
        try {
            const ret = wasm.renderer_nav_pan(this.__wbg_ptr, dx, dy);
            deferred1_0 = ret[0];
            deferred1_1 = ret[1];
            return getStringFromWasm0(ret[0], ret[1]);
        } finally {
            wasm.__wbindgen_free(deferred1_0, deferred1_1, 1);
        }
    }
    /**
     * The current zoom, in **CSS pixels per document unit** — the number the
     * zoom readout prints as a percentage.
     *
     * CSS pixels, not device pixels: the readout is about what the user sees and
     * what the pointer does, and a 2× display must not claim the artwork is at
     * 200%. `pixels_per_unit` is the inverse of this for the tools' grab radii.
     * @returns {number}
     */
    nav_scale() {
        const ret = wasm.renderer_nav_scale(this.__wbg_ptr);
        return ret;
    }
    /**
     * **Zoom about a CSS-pixel position** — one notch of a wheel or a trackpad
     * pinch. `factor > 1` zooms in.
     *
     * The position matters: the document point under the pointer is the one the
     * user is looking at, so it is the one that must not move. That invariance
     * is `Camera::zoom_at`'s definition and law 2 of the navigation laws.
     * @param {number} client_x
     * @param {number} client_y
     * @param {number} factor
     * @returns {string}
     */
    nav_zoom(client_x, client_y, factor) {
        let deferred1_0;
        let deferred1_1;
        try {
            const ret = wasm.renderer_nav_zoom(this.__wbg_ptr, client_x, client_y, factor);
            deferred1_0 = ret[0];
            deferred1_1 = ret[1];
            return getStringFromWasm0(ret[0], ret[1]);
        } finally {
            wasm.__wbindgen_free(deferred1_0, deferred1_1, 1);
        }
    }
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
     * @param {number} scale
     * @returns {string}
     */
    nav_zoom_to(scale) {
        let deferred1_0;
        let deferred1_1;
        try {
            const ret = wasm.renderer_nav_zoom_to(this.__wbg_ptr, scale);
            deferred1_0 = ret[0];
            deferred1_1 = ret[1];
            return getStringFromWasm0(ret[0], ret[1]);
        } finally {
            wasm.__wbindgen_free(deferred1_0, deferred1_1, 1);
        }
    }
    /**
     * A renderer with no GPU: the spatial index and the view transform work
     * immediately, `attach` brings the pixels up. Nothing panics if the browser
     * has no WebGPU — the UI shows what the frame report says.
     */
    constructor() {
        const ret = wasm.renderer_new();
        this.__wbg_ptr = ret;
        RendererFinalization.register(this, this.__wbg_ptr, this);
        return this;
    }
    /**
     * **Document units per screen pixel** — the camera's scale, inverted.
     *
     * The drawing tools need it for one reason: a grab radius (a handle must be
     * as easy to hit at 4× zoom as at 1×) and a stroke width (the brush's
     * `min_distance` filter should not change meaning when the user zooms). It is
     * derived from the same `Camera::visible` window the renderer draws with, so
     * an overlay can never be calibrated differently from the scene beneath it.
     * @returns {number | undefined}
     */
    pixels_per_unit() {
        const ret = wasm.renderer_pixels_per_unit(this.__wbg_ptr);
        return ret[0] === 0 ? undefined : ret[1];
    }
    /**
     * The document point under a client-space pointer position, as JSON
     * (`{"x":…,"y":…}`). This is a *view* transform; it reads no parameter and
     * no rule — the drag protocol needs it only to keep the grab offset while
     * the pointer moves.
     * @param {number} client_x
     * @param {number} client_y
     * @returns {string}
     */
    pointer_doc(client_x, client_y) {
        let deferred1_0;
        let deferred1_1;
        try {
            const ret = wasm.renderer_pointer_doc(this.__wbg_ptr, client_x, client_y);
            deferred1_0 = ret[0];
            deferred1_1 = ret[1];
            return getStringFromWasm0(ret[0], ret[1]);
        } finally {
            wasm.__wbindgen_free(deferred1_0, deferred1_1, 1);
        }
    }
    /**
     * RULE 3: a client-space pointer position → the node under it, or `null`.
     *
     * The whole chain — client box → canvas pixels → document units → spatial
     * index → exact containment — happens here, in Rust, using the *same*
     * camera that projected the pixels. That is what makes the pointer agree
     * with the picture, and what keeps the UI a dumb remote: it reports where
     * the mouse is, the engine decides what that means.
     * @param {number} client_x
     * @param {number} client_y
     * @returns {string | undefined}
     */
    pointer_hit(client_x, client_y) {
        const ret = wasm.renderer_pointer_hit(this.__wbg_ptr, client_x, client_y);
        let v1;
        if (ret[0] !== 0) {
            v1 = getStringFromWasm0(ret[0], ret[1]);
            wasm.__wbindgen_free(ret[0], ret[1] * 1, 1);
        }
        return v1;
    }
    /**
     * The viewport, from the DOM's own numbers. Returns the visible document
     * rectangle as JSON (the browser gets the camera back, nothing else).
     * @param {number} left
     * @param {number} top
     * @param {number} css_width
     * @param {number} css_height
     * @param {number} scale
     * @returns {string}
     */
    set_viewport(left, top, css_width, css_height, scale) {
        let deferred1_0;
        let deferred1_1;
        try {
            const ret = wasm.renderer_set_viewport(this.__wbg_ptr, left, top, css_width, css_height, scale);
            deferred1_0 = ret[0];
            deferred1_1 = ret[1];
            return getStringFromWasm0(ret[0], ret[1]);
        } finally {
            wasm.__wbindgen_free(deferred1_0, deferred1_1, 1);
        }
    }
    /**
     * The last frame report (plus the accumulated GPU totals) as JSON.
     * @returns {string}
     */
    stats() {
        let deferred1_0;
        let deferred1_1;
        try {
            const ret = wasm.renderer_stats(this.__wbg_ptr);
            deferred1_0 = ret[0];
            deferred1_1 = ret[1];
            return getStringFromWasm0(ret[0], ret[1]);
        } finally {
            wasm.__wbindgen_free(deferred1_0, deferred1_1, 1);
        }
    }
    /**
     * The camera the frame is drawn with: `[min_x, min_y, width, height]` in
     * document units, as JSON.
     * @returns {string}
     */
    view() {
        let deferred1_0;
        let deferred1_1;
        try {
            const ret = wasm.renderer_view(this.__wbg_ptr);
            deferred1_0 = ret[0];
            deferred1_1 = ret[1];
            return getStringFromWasm0(ret[0], ret[1]);
        } finally {
            wasm.__wbindgen_free(deferred1_0, deferred1_1, 1);
        }
    }
}
if (Symbol.dispose) Renderer.prototype[Symbol.dispose] = Renderer.prototype.free;

export class VectraEngine {
    __destroy_into_raw() {
        const ptr = this.__wbg_ptr;
        this.__wbg_ptr = 0;
        VectraEngineFinalization.unregister(this);
        return ptr;
    }
    free() {
        const ptr = this.__destroy_into_raw();
        wasm.__wbg_vectraengine_free(ptr, 0);
    }
    /**
     * Apply a plan that was already previewed and approved.
     *
     * No planner, so no retry: the plan runs exactly as shown, or it is refused
     * (and rolled back) with the engine's own message. The AI panel's *Execute*
     * with auto-correct switched off.
     * @param {string} prompt
     * @param {string} plan_json
     * @returns {string}
     */
    ai_execute_commands(prompt, plan_json) {
        let deferred3_0;
        let deferred3_1;
        try {
            const ptr0 = passStringToWasm0(prompt, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len0 = WASM_VECTOR_LEN;
            const ptr1 = passStringToWasm0(plan_json, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            const ret = wasm.vectraengine_ai_execute_commands(this.__wbg_ptr, ptr0, len0, ptr1, len1);
            deferred3_0 = ret[0];
            deferred3_1 = ret[1];
            return getStringFromWasm0(ret[0], ret[1]);
        } finally {
            wasm.__wbindgen_free(deferred3_0, deferred3_1, 1);
        }
    }
    /**
     * The whole loop (RULE 3): generate, validate through dispatch, feed a
     * refusal back, self-correct up to two times, then report.
     *
     * Returns `{"status":"ok","report":{…}}` or
     * `{"status":"error","code":"max-retries-exceeded","message":…,"corrections":[…]}`
     * — with nothing applied in the failure case.
     * @param {string} prompt
     * @param {string} summary_json
     * @returns {string}
     */
    ai_execute_with_retry(prompt, summary_json) {
        let deferred3_0;
        let deferred3_1;
        try {
            const ptr0 = passStringToWasm0(prompt, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len0 = WASM_VECTOR_LEN;
            const ptr1 = passStringToWasm0(summary_json, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            const ret = wasm.vectraengine_ai_execute_with_retry(this.__wbg_ptr, ptr0, len0, ptr1, len1);
            deferred3_0 = ret[0];
            deferred3_1 = ret[1];
            return getStringFromWasm0(ret[0], ret[1]);
        } finally {
            wasm.__wbindgen_free(deferred3_0, deferred3_1, 1);
        }
    }
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
     * @param {string} prompt
     * @param {string} summary_json
     * @returns {string}
     */
    ai_generate_commands(prompt, summary_json) {
        let deferred3_0;
        let deferred3_1;
        try {
            const ptr0 = passStringToWasm0(prompt, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len0 = WASM_VECTOR_LEN;
            const ptr1 = passStringToWasm0(summary_json, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            const ret = wasm.vectraengine_ai_generate_commands(this.__wbg_ptr, ptr0, len0, ptr1, len1);
            deferred3_0 = ret[0];
            deferred3_1 = ret[1];
            return getStringFromWasm0(ret[0], ret[1]);
        } finally {
            wasm.__wbindgen_free(deferred3_0, deferred3_1, 1);
        }
    }
    /**
     * The phrasings the built-in planner understands, for the panel's hint
     * line. The list lives beside the matcher in `vectra-ai`, so a hint can
     * never advertise something the planner cannot do.
     * @returns {string}
     */
    ai_phrasings() {
        let deferred1_0;
        let deferred1_1;
        try {
            const ret = wasm.vectraengine_ai_phrasings(this.__wbg_ptr);
            deferred1_0 = ret[0];
            deferred1_1 = ret[1];
            return getStringFromWasm0(ret[0], ret[1]);
        } finally {
            wasm.__wbindgen_free(deferred1_0, deferred1_1, 1);
        }
    }
    /**
     * The **system prompt** the MVP planner is written for, with this document
     * already injected — the AI panel shows it, and a hosted model would be
     * called with exactly this text.
     * @returns {string}
     */
    ai_prompt() {
        let deferred1_0;
        let deferred1_1;
        try {
            const ret = wasm.vectraengine_ai_prompt(this.__wbg_ptr);
            deferred1_0 = ret[0];
            deferred1_1 = ret[1];
            return getStringFromWasm0(ret[0], ret[1]);
        } finally {
            wasm.__wbindgen_free(deferred1_0, deferred1_1, 1);
        }
    }
    /**
     * The hover demo, engine-side: a spring whose target is the `off`/`on` pair
     * selected by the per-node state flag `hover:<node id>`.
     *
     * The flag is *derived from the document* here rather than sent by the UI,
     * so the UI stays a dumb remote: it reports where the pointer is
     * ([`VectraEngine::pointer_move`]) and never decides what that means.
     * @param {string} node_id
     * @param {string} property
     * @param {number} off
     * @param {number} on
     * @param {number} stiffness
     * @param {number} damping
     * @returns {string}
     */
    bind_hover_spring(node_id, property, off, on, stiffness, damping) {
        let deferred3_0;
        let deferred3_1;
        try {
            const ptr0 = passStringToWasm0(node_id, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len0 = WASM_VECTOR_LEN;
            const ptr1 = passStringToWasm0(property, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            const ret = wasm.vectraengine_bind_hover_spring(this.__wbg_ptr, ptr0, len0, ptr1, len1, off, on, stiffness, damping);
            deferred3_0 = ret[0];
            deferred3_1 = ret[1];
            return getStringFromWasm0(ret[0], ret[1]);
        } finally {
            wasm.__wbindgen_free(deferred3_0, deferred3_1, 1);
        }
    }
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
     * @param {string} node_id
     * @param {string} property
     * @param {number} target
     * @param {number} stiffness
     * @param {number} damping
     * @returns {string}
     */
    bind_spring(node_id, property, target, stiffness, damping) {
        let deferred3_0;
        let deferred3_1;
        try {
            const ptr0 = passStringToWasm0(node_id, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len0 = WASM_VECTOR_LEN;
            const ptr1 = passStringToWasm0(property, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            const ret = wasm.vectraengine_bind_spring(this.__wbg_ptr, ptr0, len0, ptr1, len1, target, stiffness, damping);
            deferred3_0 = ret[0];
            deferred3_1 = ret[1];
            return getStringFromWasm0(ret[0], ret[1]);
        } finally {
            wasm.__wbindgen_free(deferred3_0, deferred3_1, 1);
        }
    }
    /**
     * **Break a path at the intersections of its outline** (Task 12.0 RULE 3).
     *
     * `spans_json` is `[[from, to], …]` — arc lengths along the node's outline,
     * exactly the numbers [`Self::smart_fill_plan`] reported, so the UI echoes
     * what the engine measured instead of computing a cut of its own.
     *
     * **The spans are read as cut positions, and that is the whole algorithm.**
     * A span's two ends *are* two crossings; collect the ends of every span the
     * caller named, walk the ring between consecutive cuts, and each arc
     * between them is one piece. Nothing about the count of pieces is special-
     * cased, and both gestures fall out of the same rule:
     *
     * * one span → two cuts → **two** complementary arcs (the span, and the rest
     *   of the ring) — "break this span";
     * * every span of a path crossed twice → two distinct cuts → **two** arcs
     *   that tile the ring — "Break Path at Intersections".
     *
     * A ring nothing crosses is not broken: it is carried over as a piece of its
     * own, so every point of the outline ends up in exactly one piece. The
     * pieces become ordinary closed `Path` nodes wearing the source's paint, and
     * the source is hidden rather than deleted — one undo away, and the region
     * graph that suggested the cut is still there to re-read.
     * @param {string} node_id
     * @param {string} spans_json
     * @returns {string}
     */
    break_path(node_id, spans_json) {
        let deferred3_0;
        let deferred3_1;
        try {
            const ptr0 = passStringToWasm0(node_id, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len0 = WASM_VECTOR_LEN;
            const ptr1 = passStringToWasm0(spans_json, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            const ret = wasm.vectraengine_break_path(this.__wbg_ptr, ptr0, len0, ptr1, len1);
            deferred3_0 = ret[0];
            deferred3_1 = ret[1];
            return getStringFromWasm0(ret[0], ret[1]);
        } finally {
            wasm.__wbindgen_free(deferred3_0, deferred3_1, 1);
        }
    }
    /**
     * The Smart Component inspector's whole world: what the selection is, and
     * every prop it exposes (Task 10.6 RULE 1, RULE 4).
     * @returns {string}
     */
    component_view() {
        let deferred1_0;
        let deferred1_1;
        try {
            const ret = wasm.vectraengine_component_view(this.__wbg_ptr);
            deferred1_0 = ret[0];
            deferred1_1 = ret[1];
            return getStringFromWasm0(ret[0], ret[1]);
        } finally {
            wasm.__wbindgen_free(deferred1_0, deferred1_1, 1);
        }
    }
    /**
     * **Create Component** (RULE 1): the selected nodes become a master whose
     * props every instance will set for itself.
     * @param {string} members_json
     * @param {string | null} [name]
     * @returns {string}
     */
    create_component(members_json, name) {
        let deferred3_0;
        let deferred3_1;
        try {
            const ptr0 = passStringToWasm0(members_json, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len0 = WASM_VECTOR_LEN;
            var ptr1 = isLikeNone(name) ? 0 : passStringToWasm0(name, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            var len1 = WASM_VECTOR_LEN;
            const ret = wasm.vectraengine_create_component(this.__wbg_ptr, ptr0, len0, ptr1, len1);
            deferred3_0 = ret[0];
            deferred3_1 = ret[1];
            return getStringFromWasm0(ret[0], ret[1]);
        } finally {
            wasm.__wbindgen_free(deferred3_0, deferred3_1, 1);
        }
    }
    /**
     * The dependency graph, for the inspector panel.
     * @returns {string}
     */
    dependencies() {
        let deferred1_0;
        let deferred1_1;
        try {
            const ret = wasm.vectraengine_dependencies(this.__wbg_ptr);
            deferred1_0 = ret[0];
            deferred1_1 = ret[1];
            return getStringFromWasm0(ret[0], ret[1]);
        } finally {
            wasm.__wbindgen_free(deferred1_0, deferred1_1, 1);
        }
    }
    /**
     * Execute a [`Command`] JSON string. Returns a [`CommandResponse`] JSON string.
     *
     * `DefineExpression` sources are pre-validated (test-compiled) and every
     * edge-adding command is dry-run through the dependency graph BEFORE
     * dispatch: an invalid expression or a cycle fails with a typed error and
     * *zero* mutation — no record, no undo entry, no graph edge.
     * @param {string} cmd_json
     * @returns {string}
     */
    dispatch_command(cmd_json) {
        let deferred2_0;
        let deferred2_1;
        try {
            const ptr0 = passStringToWasm0(cmd_json, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len0 = WASM_VECTOR_LEN;
            const ret = wasm.vectraengine_dispatch_command(this.__wbg_ptr, ptr0, len0);
            deferred2_0 = ret[0];
            deferred2_1 = ret[1];
            return getStringFromWasm0(ret[0], ret[1]);
        } finally {
            wasm.__wbindgen_free(deferred2_0, deferred2_1, 1);
        }
    }
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
     * @returns {string}
     */
    document_json() {
        let deferred1_0;
        let deferred1_1;
        try {
            const ret = wasm.vectraengine_document_json(this.__wbg_ptr);
            deferred1_0 = ret[0];
            deferred1_1 = ret[1];
            return getStringFromWasm0(ret[0], ret[1]);
        } finally {
            wasm.__wbindgen_free(deferred1_0, deferred1_1, 1);
        }
    }
    /**
     * The AI-facing document summary (RULE 2), as JSON.
     *
     * The engine's own view, resolved through the live evaluators, so an
     * expression slot reads `$base * 2` and a procedural read its published
     * value. This is what a planner is grounded on and what `ai_prompt` embeds.
     * @returns {string}
     */
    document_summary() {
        let deferred1_0;
        let deferred1_1;
        try {
            const ret = wasm.vectraengine_document_summary(this.__wbg_ptr);
            deferred1_0 = ret[0];
            deferred1_1 = ret[1];
            return getStringFromWasm0(ret[0], ret[1]);
        } finally {
            wasm.__wbindgen_free(deferred1_0, deferred1_1, 1);
        }
    }
    /**
     * Commit the brush's stroke: fit it, expand it, and put it in the document.
     * `_node_id` is accepted for symmetry with the pen's commit (rewriting an
     * existing path); a brush gesture always produces a new node, because the
     * stroke *is* the gesture — the workflow that edits an existing path is the
     * direct-selection tool's.
     * @param {string | null} [_node_id]
     * @returns {string}
     */
    draw_brush_commit(_node_id) {
        let deferred2_0;
        let deferred2_1;
        try {
            var ptr0 = isLikeNone(_node_id) ? 0 : passStringToWasm0(_node_id, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            var len0 = WASM_VECTOR_LEN;
            const ret = wasm.vectraengine_draw_brush_commit(this.__wbg_ptr, ptr0, len0);
            deferred2_0 = ret[0];
            deferred2_1 = ret[1];
            return getStringFromWasm0(ret[0], ret[1]);
        } finally {
            wasm.__wbindgen_free(deferred2_0, deferred2_1, 1);
        }
    }
    /**
     * **Begin a direct-selection drag** (RULE 4): the path is about to be edited.
     *
     * Opens a gesture that owns the path until `draw_edit_end`. These three
     * methods are the anchor/handle counterpart of Task 3.2's drag triad, and
     * they exist for the same reason: *one gesture is one undo step*.
     * `draw_edit_command` alone would push a history entry per pointer sample, so
     * dragging an anchor across the canvas would take two hundred undos to take
     * back.
     * @param {string} node_id
     * @param {string} slot
     * @param {string} side
     * @param {boolean} anchor
     * @returns {string}
     */
    draw_edit_begin(node_id, slot, side, anchor) {
        let deferred4_0;
        let deferred4_1;
        try {
            const ptr0 = passStringToWasm0(node_id, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len0 = WASM_VECTOR_LEN;
            const ptr1 = passStringToWasm0(slot, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            const ptr2 = passStringToWasm0(side, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len2 = WASM_VECTOR_LEN;
            const ret = wasm.vectraengine_draw_edit_begin(this.__wbg_ptr, ptr0, len0, ptr1, len1, ptr2, len2, anchor);
            deferred4_0 = ret[0];
            deferred4_1 = ret[1];
            return getStringFromWasm0(ret[0], ret[1]);
        } finally {
            wasm.__wbindgen_free(deferred4_0, deferred4_1, 1);
        }
    }
    /**
     * Abandon a direct-selection drag, restoring the path it started from.
     * @returns {string}
     */
    draw_edit_cancel() {
        let deferred1_0;
        let deferred1_1;
        try {
            const ret = wasm.vectraengine_draw_edit_cancel(this.__wbg_ptr);
            deferred1_0 = ret[0];
            deferred1_1 = ret[1];
            return getStringFromWasm0(ret[0], ret[1]);
        } finally {
            wasm.__wbindgen_free(deferred1_0, deferred1_1, 1);
        }
    }
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
     * @param {string} node_id
     * @param {string} slot
     * @param {string} side
     * @param {number} x
     * @param {number} y
     * @param {boolean} alt
     * @param {boolean} anchor
     * @returns {string}
     */
    draw_edit_command(node_id, slot, side, x, y, alt, anchor) {
        let deferred4_0;
        let deferred4_1;
        try {
            const ptr0 = passStringToWasm0(node_id, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len0 = WASM_VECTOR_LEN;
            const ptr1 = passStringToWasm0(slot, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            const ptr2 = passStringToWasm0(side, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len2 = WASM_VECTOR_LEN;
            const ret = wasm.vectraengine_draw_edit_command(this.__wbg_ptr, ptr0, len0, ptr1, len1, ptr2, len2, x, y, alt, anchor);
            deferred4_0 = ret[0];
            deferred4_1 = ret[1];
            return getStringFromWasm0(ret[0], ret[1]);
        } finally {
            wasm.__wbindgen_free(deferred4_0, deferred4_1, 1);
        }
    }
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
     * @returns {string}
     */
    draw_edit_end() {
        let deferred1_0;
        let deferred1_1;
        try {
            const ret = wasm.vectraengine_draw_edit_end(this.__wbg_ptr);
            deferred1_0 = ret[0];
            deferred1_1 = ret[1];
            return getStringFromWasm0(ret[0], ret[1]);
        } finally {
            wasm.__wbindgen_free(deferred1_0, deferred1_1, 1);
        }
    }
    /**
     * **One sample of a direct-selection drag**, applied and settled.
     *
     * Nothing is pushed onto the undo stack here — the gesture is one step, and
     * `draw_edit_end` records it (see [`EditSession`]). What *does* happen here is
     * everything else a user can see: the parameter writes, the constraint pass
     * (a `Required` row stronger than the pointer wins, exactly as in Task 3.1),
     * the dirty set and the scene patch.
     * @param {number} x
     * @param {number} y
     * @param {boolean} alt
     * @returns {string}
     */
    draw_edit_update(x, y, alt) {
        let deferred1_0;
        let deferred1_1;
        try {
            const ret = wasm.vectraengine_draw_edit_update(this.__wbg_ptr, x, y, alt);
            deferred1_0 = ret[0];
            deferred1_1 = ret[1];
            return getStringFromWasm0(ret[0], ret[1]);
        } finally {
            wasm.__wbindgen_free(deferred1_0, deferred1_1, 1);
        }
    }
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
     * @param {Renderer} renderer
     * @param {string} node_id
     * @param {number} x
     * @param {number} y
     * @returns {string}
     */
    draw_hit(renderer, node_id, x, y) {
        let deferred2_0;
        let deferred2_1;
        try {
            _assertClass(renderer, Renderer);
            const ptr0 = passStringToWasm0(node_id, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len0 = WASM_VECTOR_LEN;
            const ret = wasm.vectraengine_draw_hit(this.__wbg_ptr, renderer.__wbg_ptr, ptr0, len0, x, y);
            deferred2_0 = ret[0];
            deferred2_1 = ret[1];
            return getStringFromWasm0(ret[0], ret[1]);
        } finally {
            wasm.__wbindgen_free(deferred2_0, deferred2_1, 1);
        }
    }
    /**
     * The anchors and handles of a path, for the direct-selection overlay.
     *
     * Engine data, in document space: the UI hands it to the renderer's own
     * camera to place it on screen, so an overlay can never disagree with the
     * geometry it decorates.
     * @param {string} node_id
     * @returns {string}
     */
    draw_overlay(node_id) {
        let deferred2_0;
        let deferred2_1;
        try {
            const ptr0 = passStringToWasm0(node_id, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len0 = WASM_VECTOR_LEN;
            const ret = wasm.vectraengine_draw_overlay(this.__wbg_ptr, ptr0, len0);
            deferred2_0 = ret[0];
            deferred2_1 = ret[1];
            return getStringFromWasm0(ret[0], ret[1]);
        } finally {
            wasm.__wbindgen_free(deferred2_0, deferred2_1, 1);
        }
    }
    /**
     * **Commit the pen's draft** as a path node.
     *
     * `new_node` decides create-versus-rewrite: pass an existing node id to keep
     * editing a shape (the direct-selection workflow), or `None` for a fresh
     * drawn path. Returns the engine's own command response, so the UI's event
     * log, undo stack and selection state behave exactly as for a hand-authored
     * command.
     * @param {boolean} close
     * @param {string | null} [node_id]
     * @returns {string}
     */
    draw_pen_commit(close, node_id) {
        let deferred2_0;
        let deferred2_1;
        try {
            var ptr0 = isLikeNone(node_id) ? 0 : passStringToWasm0(node_id, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            var len0 = WASM_VECTOR_LEN;
            const ret = wasm.vectraengine_draw_pen_commit(this.__wbg_ptr, close, ptr0, len0);
            deferred2_0 = ret[0];
            deferred2_1 = ret[1];
            return getStringFromWasm0(ret[0], ret[1]);
        } finally {
            wasm.__wbindgen_free(deferred2_0, deferred2_1, 1);
        }
    }
    /**
     * **A pen or brush pointer event** (Task 10.1 RULE 2 / RULE 3).
     *
     * `tool` is `"pen"` or `"brush"`. For the pen, the gesture vocabulary is
     * `vectra_draw::PenSession`'s, so `alt` breaks handle symmetry and `close`
     * asks for the path to be joined to its first point. The reply carries the
     * draft, so the UI's canvas has exactly the geometry the engine has — no
     * interpolation, no duplicated maths.
     * @param {string} tool
     * @param {string} kind
     * @param {number} x
     * @param {number} y
     * @param {boolean} alt
     * @param {boolean} close
     * @param {number} time
     * @returns {string}
     */
    draw_pointer(tool, kind, x, y, alt, close, time) {
        let deferred3_0;
        let deferred3_1;
        try {
            const ptr0 = passStringToWasm0(tool, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len0 = WASM_VECTOR_LEN;
            const ptr1 = passStringToWasm0(kind, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            const ret = wasm.vectraengine_draw_pointer(this.__wbg_ptr, ptr0, len0, ptr1, len1, x, y, alt, close, time);
            deferred3_0 = ret[0];
            deferred3_1 = ret[1];
            return getStringFromWasm0(ret[0], ret[1]);
        } finally {
            wasm.__wbindgen_free(deferred3_0, deferred3_1, 1);
        }
    }
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
     * @param {string} node_id
     * @param {string} stroke_json
     * @returns {string}
     */
    draw_quick_shape(node_id, stroke_json) {
        let deferred3_0;
        let deferred3_1;
        try {
            const ptr0 = passStringToWasm0(node_id, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len0 = WASM_VECTOR_LEN;
            const ptr1 = passStringToWasm0(stroke_json, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            const ret = wasm.vectraengine_draw_quick_shape(this.__wbg_ptr, ptr0, len0, ptr1, len1);
            deferred3_0 = ret[0];
            deferred3_1 = ret[1];
            return getStringFromWasm0(ret[0], ret[1]);
        } finally {
            wasm.__wbindgen_free(deferred3_0, deferred3_1, 1);
        }
    }
    /**
     * **The tools' settings** — the brush's curve-fit tolerance and the pen's
     * pick radius are *engine* decisions (they are compared against document
     * geometry), so they are set here rather than in the UI.
     * @param {number} tolerance
     * @returns {string}
     */
    draw_set_tolerance(tolerance) {
        let deferred1_0;
        let deferred1_1;
        try {
            const ret = wasm.vectraengine_draw_set_tolerance(this.__wbg_ptr, tolerance);
            deferred1_0 = ret[0];
            deferred1_1 = ret[1];
            return getStringFromWasm0(ret[0], ret[1]);
        } finally {
            wasm.__wbindgen_free(deferred1_0, deferred1_1, 1);
        }
    }
    /**
     * **Export all artboards** (Task 10.2 RULE 2): every board, laid out where
     * it sits in document space, so the file reads as one canvas with frames on
     * it — the same arrangement the GPU canvas shows.
     * @returns {string}
     */
    export_all_artboards() {
        let deferred1_0;
        let deferred1_1;
        try {
            const ret = wasm.vectraengine_export_all_artboards(this.__wbg_ptr);
            deferred1_0 = ret[0];
            deferred1_1 = ret[1];
            return getStringFromWasm0(ret[0], ret[1]);
        } finally {
            wasm.__wbindgen_free(deferred1_0, deferred1_1, 1);
        }
    }
    /**
     * **Export current artboard** (Task 10.2 RULE 2): the SVG of the board the
     * designer is on, cropped to its frame and painted with its background.
     *
     * The document decides what belongs to the board (its layers' nodes); the
     * exporter decides what the file looks like. Nothing here filters geometry
     * by hand — the same IR, the same element table, one extra frame.
     * @returns {string}
     */
    export_current_artboard() {
        let deferred1_0;
        let deferred1_1;
        try {
            const ret = wasm.vectraengine_export_current_artboard(this.__wbg_ptr);
            deferred1_0 = ret[0];
            deferred1_1 = ret[1];
            return getStringFromWasm0(ret[0], ret[1]);
        } finally {
            wasm.__wbindgen_free(deferred1_0, deferred1_1, 1);
        }
    }
    /**
     * The drawing as a **parametric React component** (Task 8.0, RULE 2).
     *
     * A slot driven by `$base * 2` comes out as `width={base * 2}` with `base`
     * declared as a required prop: the same picture, as code that goes on taking
     * arguments.
     * @returns {string}
     */
    export_to_react() {
        let deferred1_0;
        let deferred1_1;
        try {
            const ret = wasm.vectraengine_export_to_react(this.__wbg_ptr);
            deferred1_0 = ret[0];
            deferred1_1 = ret[1];
            return getStringFromWasm0(ret[0], ret[1]);
        } finally {
            wasm.__wbindgen_free(deferred1_0, deferred1_1, 1);
        }
    }
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
     * @returns {string}
     */
    export_to_svg() {
        let deferred1_0;
        let deferred1_1;
        try {
            const ret = wasm.vectraengine_export_to_svg(this.__wbg_ptr);
            deferred1_0 = ret[0];
            deferred1_1 = ret[1];
            return getStringFromWasm0(ret[0], ret[1]);
        } finally {
            wasm.__wbindgen_free(deferred1_0, deferred1_1, 1);
        }
    }
    /**
     * The families the host's font library can resolve, as a JSON array —
     * the Text panel's picker. Always non-empty: the bundled face is in it.
     * @returns {string}
     */
    font_families() {
        let deferred1_0;
        let deferred1_1;
        try {
            const ret = wasm.vectraengine_font_families(this.__wbg_ptr);
            deferred1_0 = ret[0];
            deferred1_1 = ret[1];
            return getStringFromWasm0(ret[0], ret[1]);
        } finally {
            wasm.__wbindgen_free(deferred1_0, deferred1_1, 1);
        }
    }
    /**
     * Rebuild the whole scene from scratch (the UI's "re-evaluate everything"
     * control). Demonstrably equal to the incremental cache — the app-level
     * witness of `patch ≡ rebuild`.
     * @returns {string}
     */
    force_full_evaluation() {
        let deferred1_0;
        let deferred1_1;
        try {
            const ret = wasm.vectraengine_force_full_evaluation(this.__wbg_ptr);
            deferred1_0 = ret[0];
            deferred1_1 = ret[1];
            return getStringFromWasm0(ret[0], ret[1]);
        } finally {
            wasm.__wbindgen_free(deferred1_0, deferred1_1, 1);
        }
    }
    /**
     * Project the live engine into a [`snapshot::SnapshotResponse`] JSON string.
     *
     * The first call warms the scene cache (one full pass); every later call is
     * a pure projection — evaluation already happened during the mutation that
     * dirtied it.
     * @returns {string}
     */
    get_snapshot() {
        let deferred1_0;
        let deferred1_1;
        try {
            const ret = wasm.vectraengine_get_snapshot(this.__wbg_ptr);
            deferred1_0 = ret[0];
            deferred1_1 = ret[1];
            return getStringFromWasm0(ret[0], ret[1]);
        } finally {
            wasm.__wbindgen_free(deferred1_0, deferred1_1, 1);
        }
    }
    /**
     * **Generate Icon Set** (RULE 3): one instance per size, each on its own
     * artboard, all scaled by the master's `size` prop.
     * @param {string} master
     * @param {string} sizes_json
     * @returns {string}
     */
    icon_set(master, sizes_json) {
        let deferred3_0;
        let deferred3_1;
        try {
            const ptr0 = passStringToWasm0(master, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len0 = WASM_VECTOR_LEN;
            const ptr1 = passStringToWasm0(sizes_json, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            const ret = wasm.vectraengine_icon_set(this.__wbg_ptr, ptr0, len0, ptr1, len1);
            deferred3_0 = ret[0];
            deferred3_1 = ret[1];
            return getStringFromWasm0(ret[0], ret[1]);
        } finally {
            wasm.__wbindgen_free(deferred3_0, deferred3_1, 1);
        }
    }
    /**
     * **Place an instance** of a component (RULE 1).
     * @param {string} master
     * @param {string | null} [name]
     * @returns {string}
     */
    instantiate_component(master, name) {
        let deferred3_0;
        let deferred3_1;
        try {
            const ptr0 = passStringToWasm0(master, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len0 = WASM_VECTOR_LEN;
            var ptr1 = isLikeNone(name) ? 0 : passStringToWasm0(name, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            var len1 = WASM_VECTOR_LEN;
            const ret = wasm.vectraengine_instantiate_component(this.__wbg_ptr, ptr0, len0, ptr1, len1);
            deferred3_0 = ret[0];
            deferred3_1 = ret[1];
            return getStringFromWasm0(ret[0], ret[1]);
        } finally {
            wasm.__wbindgen_free(deferred3_0, deferred3_1, 1);
        }
    }
    /**
     * **The idle signal** (Frame Budget Law): `false` means the document is at
     * rest at the current clock, so the React frame loop must stop scheduling
     * `render_frame` until something moves again.
     * @returns {boolean}
     */
    is_animating() {
        const ret = wasm.vectraengine_is_animating(this.__wbg_ptr);
        return ret !== 0;
    }
    /**
     * Motion state for the inspector and the smoke harness: whether anything is
     * moving, when it will stop, and what is bound where.
     * @returns {string}
     */
    motion_json() {
        let deferred1_0;
        let deferred1_1;
        try {
            const ret = wasm.vectraengine_motion_json(this.__wbg_ptr);
            deferred1_0 = ret[0];
            deferred1_1 = ret[1];
            return getStringFromWasm0(ret[0], ret[1]);
        } finally {
            wasm.__wbindgen_free(deferred1_0, deferred1_1, 1);
        }
    }
    /**
     * Initialize a fresh engine.
     */
    constructor() {
        const ret = wasm.vectraengine_new();
        this.__wbg_ptr = ret;
        VectraEngineFinalization.register(this, this.__wbg_ptr, this);
        return this;
    }
    /**
     * **Outline a text node to paths** (Task 11.0 RULE 3): the non-destructive
     * conversion from type to letterforms.
     *
     * Shaping lives in `vectra-geometry`, and `Command::OutlineText` carries a
     * *plan* rather than a font, so the boundary is where the two meet: it
     * lays the run out, asks the geometry crate for one closed plan per
     * letterform, mints the group and letterform ids, and dispatches the command
     * as **one history entry**. Core never learns what a glyph is; the sketch
     * from the font never leaves this call.
     *
     * The original text node is hidden, never deleted — undo restores it
     * exactly, and its string and parameters are still there to come back to.
     * @param {string} node_id
     * @param {string | null} [name]
     * @returns {string}
     */
    outline_text(node_id, name) {
        let deferred3_0;
        let deferred3_1;
        try {
            const ptr0 = passStringToWasm0(node_id, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len0 = WASM_VECTOR_LEN;
            var ptr1 = isLikeNone(name) ? 0 : passStringToWasm0(name, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            var len1 = WASM_VECTOR_LEN;
            const ret = wasm.vectraengine_outline_text(this.__wbg_ptr, ptr0, len0, ptr1, len1);
            deferred3_0 = ret[0];
            deferred3_1 = ret[1];
            return getStringFromWasm0(ret[0], ret[1]);
        } finally {
            wasm.__wbindgen_free(deferred3_0, deferred3_1, 1);
        }
    }
    /**
     * The pointer left the canvas: nothing is hovered any more.
     *
     * A separate entry point rather than a magic off-canvas coordinate, because
     * "the pointer is not on the canvas" is not a *place* — the UI would have to
     * invent a document point to say it, and the engine would have to trust the
     * invention.
     * @returns {string}
     */
    pointer_leave() {
        let deferred1_0;
        let deferred1_1;
        try {
            const ret = wasm.vectraengine_pointer_leave(this.__wbg_ptr);
            deferred1_0 = ret[0];
            deferred1_1 = ret[1];
            return getStringFromWasm0(ret[0], ret[1]);
        } finally {
            wasm.__wbindgen_free(deferred1_0, deferred1_1, 1);
        }
    }
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
     * @param {Renderer} renderer
     * @param {number} x
     * @param {number} y
     * @returns {string}
     */
    pointer_move(renderer, x, y) {
        let deferred1_0;
        let deferred1_1;
        try {
            _assertClass(renderer, Renderer);
            const ret = wasm.vectraengine_pointer_move(this.__wbg_ptr, renderer.__wbg_ptr, x, y);
            deferred1_0 = ret[0];
            deferred1_1 = ret[1];
            return getStringFromWasm0(ret[0], ret[1]);
        } finally {
            wasm.__wbindgen_free(deferred1_0, deferred1_1, 1);
        }
    }
    /**
     * @returns {string}
     */
    procedural_json() {
        let deferred1_0;
        let deferred1_1;
        try {
            const ret = wasm.vectraengine_procedural_json(this.__wbg_ptr);
            deferred1_0 = ret[0];
            deferred1_1 = ret[1];
            return getStringFromWasm0(ret[0], ret[1]);
        } finally {
            wasm.__wbindgen_free(deferred1_0, deferred1_1, 1);
        }
    }
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
     * @returns {string}
     */
    procedural_kinds() {
        let deferred1_0;
        let deferred1_1;
        try {
            const ret = wasm.vectraengine_procedural_kinds(this.__wbg_ptr);
            deferred1_0 = ret[0];
            deferred1_1 = ret[1];
            return getStringFromWasm0(ret[0], ret[1]);
        } finally {
            wasm.__wbindgen_free(deferred1_0, deferred1_1, 1);
        }
    }
    /**
     * Redo one step. Returns a [`CommandResponse`] JSON string.
     * @returns {string}
     */
    redo() {
        let deferred1_0;
        let deferred1_1;
        try {
            const ret = wasm.vectraengine_redo(this.__wbg_ptr);
            deferred1_0 = ret[0];
            deferred1_1 = ret[1];
            return getStringFromWasm0(ret[0], ret[1]);
        } finally {
            wasm.__wbindgen_free(deferred1_0, deferred1_1, 1);
        }
    }
    /**
     * Register a face for a family name (Task 11.0 RULE 1), as raw bytes.
     *
     * Registration is *validating*: bytes that do not parse as a font are
     * refused and nothing changes, so a bad upload can never become a text node
     * that silently draws nothing.
     * @param {string} family
     * @param {Uint8Array} bytes
     * @returns {string}
     */
    register_font(family, bytes) {
        let deferred3_0;
        let deferred3_1;
        try {
            const ptr0 = passStringToWasm0(family, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len0 = WASM_VECTOR_LEN;
            const ptr1 = passArray8ToWasm0(bytes, wasm.__wbindgen_malloc);
            const len1 = WASM_VECTOR_LEN;
            const ret = wasm.vectraengine_register_font(this.__wbg_ptr, ptr0, len0, ptr1, len1);
            deferred3_0 = ret[0];
            deferred3_1 = ret[1];
            return getStringFromWasm0(ret[0], ret[1]);
        } finally {
            wasm.__wbindgen_free(deferred3_0, deferred3_1, 1);
        }
    }
    /**
     * Remove a track by id. Undoable; a slot still bound to it fails to resolve
     * until the track returns, and that failure is visible in the event log
     * rather than silently rendering a zero.
     * @param {string} track_id
     * @returns {string}
     */
    remove_motion_track(track_id) {
        let deferred2_0;
        let deferred2_1;
        try {
            const ptr0 = passStringToWasm0(track_id, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len0 = WASM_VECTOR_LEN;
            const ret = wasm.vectraengine_remove_motion_track(this.__wbg_ptr, ptr0, len0);
            deferred2_0 = ret[0];
            deferred2_1 = ret[1];
            return getStringFromWasm0(ret[0], ret[1]);
        } finally {
            wasm.__wbindgen_free(deferred2_0, deferred2_1, 1);
        }
    }
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
     * @param {Renderer} renderer
     * @returns {string}
     */
    render_frame(renderer) {
        let deferred1_0;
        let deferred1_1;
        try {
            _assertClass(renderer, Renderer);
            const ret = wasm.vectraengine_render_frame(this.__wbg_ptr, renderer.__wbg_ptr);
            deferred1_0 = ret[0];
            deferred1_1 = ret[1];
            return getStringFromWasm0(ret[0], ret[1]);
        } finally {
            wasm.__wbindgen_free(deferred1_0, deferred1_1, 1);
        }
    }
    /**
     * **Set one prop on one instance** (RULE 1) — the slider the panel draws.
     *
     * `value_json` is a typed [`vectra_core::ParamValue`]
     * (`{"Float":{"Literal":32}}` / `{"Color":{"Literal":"#2266ee"}}`), which
     * is exactly what `component_view` publishes per prop type.
     * @param {string} target
     * @param {string} prop
     * @param {string} value_json
     * @returns {string}
     */
    set_component_prop(target, prop, value_json) {
        let deferred4_0;
        let deferred4_1;
        try {
            const ptr0 = passStringToWasm0(target, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len0 = WASM_VECTOR_LEN;
            const ptr1 = passStringToWasm0(prop, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            const ptr2 = passStringToWasm0(value_json, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len2 = WASM_VECTOR_LEN;
            const ret = wasm.vectraengine_set_component_prop(this.__wbg_ptr, ptr0, len0, ptr1, len1, ptr2, len2);
            deferred4_0 = ret[0];
            deferred4_1 = ret[1];
            return getStringFromWasm0(ret[0], ret[1]);
        } finally {
            wasm.__wbindgen_free(deferred4_0, deferred4_1, 1);
        }
    }
    /**
     * Register or replace a keyframe track (undoable document state).
     *
     * Takes the track as JSON so the wire schema lives in one place
     * (`MotionTrack`'s own `Serialize`/`Deserialize`), exactly like commands.
     * @param {string} track_json
     * @returns {string}
     */
    set_motion_track(track_json) {
        let deferred2_0;
        let deferred2_1;
        try {
            const ptr0 = passStringToWasm0(track_json, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len0 = WASM_VECTOR_LEN;
            const ret = wasm.vectraengine_set_motion_track(this.__wbg_ptr, ptr0, len0);
            deferred2_0 = ret[0];
            deferred2_1 = ret[1];
            return getStringFromWasm0(ret[0], ret[1]);
        } finally {
            wasm.__wbindgen_free(deferred2_0, deferred2_1, 1);
        }
    }
    /**
     * The procedural graph, for the Procedural panel and the smoke harness.
     *
     * Three things the panel cannot get anywhere else: the registry's own
     * order (a chain has a direction, a JSON object does not), each operand's
     * **effective** value (`(variable)`, not the number the template started
     * with — see `ProceduralNode::describe`), and the value each output port
     * last published.
     * Tell the engine what the designer has selected.
     *
     * The selection is *state*, not a command: it is not undoable, it does not
     * touch the document, and it exists so that a Make Magic prompt can mean
     * "this". The reply carries the panel's one-line description, so the UI
     * never has to compose a sentence about ids.
     * @param {string} ids_json
     * @returns {string}
     */
    set_selection(ids_json) {
        let deferred2_0;
        let deferred2_1;
        try {
            const ptr0 = passStringToWasm0(ids_json, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len0 = WASM_VECTOR_LEN;
            const ret = wasm.vectraengine_set_selection(this.__wbg_ptr, ptr0, len0);
            deferred2_0 = ret[0];
            deferred2_1 = ret[1];
            return getStringFromWasm0(ret[0], ret[1]);
        } finally {
            wasm.__wbindgen_free(deferred2_0, deferred2_1, 1);
        }
    }
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
     * @param {string} name
     * @param {boolean} on
     * @returns {string}
     */
    set_state(name, on) {
        let deferred2_0;
        let deferred2_1;
        try {
            const ptr0 = passStringToWasm0(name, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len0 = WASM_VECTOR_LEN;
            const ret = wasm.vectraengine_set_state(this.__wbg_ptr, ptr0, len0, on);
            deferred2_0 = ret[0];
            deferred2_1 = ret[1];
            return getStringFromWasm0(ret[0], ret[1]);
        } finally {
            wasm.__wbindgen_free(deferred2_0, deferred2_1, 1);
        }
    }
    /**
     * Scrub animation time: dirty from the clock vertex and re-evaluate its
     * readers (Phase 4 motion rides this exact path).
     * @param {number} t
     * @returns {string}
     */
    set_time(t) {
        let deferred1_0;
        let deferred1_1;
        try {
            const ret = wasm.vectraengine_set_time(this.__wbg_ptr, t);
            deferred1_0 = ret[0];
            deferred1_1 = ret[1];
            return getStringFromWasm0(ret[0], ret[1]);
        } finally {
            wasm.__wbindgen_free(deferred1_0, deferred1_1, 1);
        }
    }
    /**
     * **The Smart Fill plan** (Task 12.0 RULE 1): the region graph of a set of
     * paths, as JSON — see [`RegionPlanWire`].
     *
     * This is a *query*, not a command: it mutates nothing, and every consumer
     * that needs to know which faces a set of paths makes asks here rather than
     * computing a region of its own.
     *
     * `ids_json` names the boundaries. **An empty list means RULE 1's own
     * sentence** — *"all selected or overlapping paths in the active layer"* —
     * so the callers that have no opinion (the tool, the drop) pass `[]` and get
     * the rule, while a panel or a test can still name a set. Either way a
     * **group expands** to the shapes it holds: a group is a selection, not a
     * boundary, and point-in-region against a group has no meaning of its own.
     * An id with no evaluated geometry contributes no face.
     *
     * `point_json` is `null` — or the two numbers of a **drop point** (RULE 4's
     * seed). With a point, `hit` is the face that contains it, or `null` when it
     * falls in no face at all; the test is `RegionGraph::face_at`, which is the
     * same smallest-face-wins answer a fill's own evaluation uses, so the
     * highlight the designer sees and the region the fill adopts are one
     * function, not two that agree today.
     * @param {string} ids_json
     * @param {string} point_json
     * @returns {string}
     */
    smart_fill_plan(ids_json, point_json) {
        let deferred3_0;
        let deferred3_1;
        try {
            const ptr0 = passStringToWasm0(ids_json, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len0 = WASM_VECTOR_LEN;
            const ptr1 = passStringToWasm0(point_json, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            const ret = wasm.vectraengine_smart_fill_plan(this.__wbg_ptr, ptr0, len0, ptr1, len1);
            deferred3_0 = ret[0];
            deferred3_1 = ret[1];
            return getStringFromWasm0(ret[0], ret[1]);
        } finally {
            wasm.__wbindgen_free(deferred3_0, deferred3_1, 1);
        }
    }
    /**
     * The structural macros the command bar offers as one-tap chips (RULE 2):
     * the prompt text, plus what it will do, in the designer's words.
     * @returns {string}
     */
    structural_macros() {
        let deferred1_0;
        let deferred1_1;
        try {
            const ret = wasm.vectraengine_structural_macros(this.__wbg_ptr);
            deferred1_0 = ret[0];
            deferred1_1 = ret[1];
            return getStringFromWasm0(ret[0], ret[1]);
        } finally {
            wasm.__wbindgen_free(deferred1_0, deferred1_1, 1);
        }
    }
    /**
     * Undo one step. Returns a [`CommandResponse`] JSON string.
     *
     * The command about to be applied (the previous entry's *inverse*) is
     * dry-run first, so rewinding cannot introduce a cycle either.
     * @returns {string}
     */
    undo() {
        let deferred1_0;
        let deferred1_1;
        try {
            const ret = wasm.vectraengine_undo(this.__wbg_ptr);
            deferred1_0 = ret[0];
            deferred1_1 = ret[1];
            return getStringFromWasm0(ret[0], ret[1]);
        } finally {
            wasm.__wbindgen_free(deferred1_0, deferred1_1, 1);
        }
    }
}
if (Symbol.dispose) VectraEngine.prototype[Symbol.dispose] = VectraEngine.prototype.free;
function __wbg_get_imports() {
    const import0 = {
        __proto__: null,
        __wbg_Window_06e90eea4c7df280: function(arg0) {
            const ret = arg0.Window;
            return ret;
        },
        __wbg_WorkerGlobalScope_defda269b75e179a: function(arg0) {
            const ret = arg0.WorkerGlobalScope;
            return ret;
        },
        __wbg___wbindgen_debug_string_4687d8d8c2017d52: function(arg0, arg1) {
            const ret = debugString(arg1);
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg___wbindgen_is_function_1f9d30630b8b1d3d: function(arg0) {
            const ret = typeof(arg0) === 'function';
            return ret;
        },
        __wbg___wbindgen_is_object_3c45d4f2dde4e749: function(arg0) {
            const val = arg0;
            const ret = typeof(val) === 'object' && val !== null;
            return ret;
        },
        __wbg___wbindgen_is_undefined_8865fb403f8fe9d8: function(arg0) {
            const ret = arg0 === undefined;
            return ret;
        },
        __wbg___wbindgen_string_get_0380ccaa2f57f0d9: function(arg0, arg1) {
            const obj = arg1;
            const ret = typeof(obj) === 'string' ? obj : undefined;
            var ptr1 = isLikeNone(ret) ? 0 : passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            var len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg___wbindgen_throw_41e9ee4f547fc59a: function(arg0, arg1) {
            throw new Error(getStringFromWasm0(arg0, arg1));
        },
        __wbg__wbg_cb_unref_dcc1a90847f04c41: function(arg0) {
            arg0._wbg_cb_unref();
        },
        __wbg_beginComputePass_5d05bddfd3eb7ba4: function(arg0, arg1) {
            const ret = arg0.beginComputePass(arg1);
            return ret;
        },
        __wbg_beginRenderPass_9a7bf53d588737dc: function(arg0, arg1) {
            const ret = arg0.beginRenderPass(arg1);
            return ret;
        },
        __wbg_buffer_56ec2905a66f58b9: function(arg0) {
            const ret = arg0.buffer;
            return ret;
        },
        __wbg_call_187d372bd5fdd4aa: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = arg0.call(arg1, arg2);
            return ret;
        }, arguments); },
        __wbg_clearBuffer_b08b15b7ee3c9d57: function(arg0, arg1, arg2) {
            arg0.clearBuffer(arg1, arg2);
        },
        __wbg_clearBuffer_f24f8de43db597ec: function(arg0, arg1, arg2, arg3) {
            arg0.clearBuffer(arg1, arg2, arg3);
        },
        __wbg_configure_6e1ccd3ac31b721c: function(arg0, arg1) {
            arg0.configure(arg1);
        },
        __wbg_copyBufferToBuffer_d52339f5d639af9b: function(arg0, arg1, arg2, arg3, arg4, arg5) {
            arg0.copyBufferToBuffer(arg1, arg2, arg3, arg4, arg5);
        },
        __wbg_copyBufferToTexture_48aa78a412b2a467: function(arg0, arg1, arg2, arg3) {
            arg0.copyBufferToTexture(arg1, arg2, arg3);
        },
        __wbg_copyExternalImageToTexture_eebbba3aa85a0b95: function(arg0, arg1, arg2, arg3) {
            arg0.copyExternalImageToTexture(arg1, arg2, arg3);
        },
        __wbg_copyTextureToBuffer_5aef45a98e34a97e: function(arg0, arg1, arg2, arg3) {
            arg0.copyTextureToBuffer(arg1, arg2, arg3);
        },
        __wbg_copyTextureToTexture_97d0e9333a1e1008: function(arg0, arg1, arg2, arg3) {
            arg0.copyTextureToTexture(arg1, arg2, arg3);
        },
        __wbg_createBindGroupLayout_e37f9323c278f93f: function(arg0, arg1) {
            const ret = arg0.createBindGroupLayout(arg1);
            return ret;
        },
        __wbg_createBindGroup_876adbf7e329ce2e: function(arg0, arg1) {
            const ret = arg0.createBindGroup(arg1);
            return ret;
        },
        __wbg_createBuffer_e3f8b2bd8b492498: function(arg0, arg1) {
            const ret = arg0.createBuffer(arg1);
            return ret;
        },
        __wbg_createCommandEncoder_e617922978f8b4de: function(arg0, arg1) {
            const ret = arg0.createCommandEncoder(arg1);
            return ret;
        },
        __wbg_createComputePipeline_6794bf24c6c03583: function(arg0, arg1) {
            const ret = arg0.createComputePipeline(arg1);
            return ret;
        },
        __wbg_createPipelineLayout_1a8ea1f550cfa5e7: function(arg0, arg1) {
            const ret = arg0.createPipelineLayout(arg1);
            return ret;
        },
        __wbg_createQuerySet_6050df2adcb1f167: function(arg0, arg1) {
            const ret = arg0.createQuerySet(arg1);
            return ret;
        },
        __wbg_createRenderBundleEncoder_a98ecb1771e99ab3: function(arg0, arg1) {
            const ret = arg0.createRenderBundleEncoder(arg1);
            return ret;
        },
        __wbg_createRenderPipeline_921034ccba195ffe: function(arg0, arg1) {
            const ret = arg0.createRenderPipeline(arg1);
            return ret;
        },
        __wbg_createSampler_cb4137c4e97c7098: function(arg0, arg1) {
            const ret = arg0.createSampler(arg1);
            return ret;
        },
        __wbg_createShaderModule_912a19a8ccc2aa1a: function(arg0, arg1) {
            const ret = arg0.createShaderModule(arg1);
            return ret;
        },
        __wbg_createTexture_1a3ebeb1ddd7a035: function(arg0, arg1) {
            const ret = arg0.createTexture(arg1);
            return ret;
        },
        __wbg_createView_c227b9af7bd5f441: function(arg0, arg1) {
            const ret = arg0.createView(arg1);
            return ret;
        },
        __wbg_destroy_50767c0458f7c8d1: function(arg0) {
            arg0.destroy();
        },
        __wbg_destroy_80182ff6e496228e: function(arg0) {
            arg0.destroy();
        },
        __wbg_destroy_a2c0702c5d1269b5: function(arg0) {
            arg0.destroy();
        },
        __wbg_devicePixelRatio_7d39e9af5448d3d4: function(arg0) {
            const ret = arg0.devicePixelRatio;
            return ret;
        },
        __wbg_dispatchWorkgroupsIndirect_64be0198a6df9be7: function(arg0, arg1, arg2) {
            arg0.dispatchWorkgroupsIndirect(arg1, arg2);
        },
        __wbg_dispatchWorkgroups_c122d0482fa3f389: function(arg0, arg1, arg2, arg3) {
            arg0.dispatchWorkgroups(arg1 >>> 0, arg2 >>> 0, arg3 >>> 0);
        },
        __wbg_document_9854e03c05fc8834: function(arg0) {
            const ret = arg0.document;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_drawIndexedIndirect_888ac46c4c23516f: function(arg0, arg1, arg2) {
            arg0.drawIndexedIndirect(arg1, arg2);
        },
        __wbg_drawIndexedIndirect_fcc6ecbd3d698094: function(arg0, arg1, arg2) {
            arg0.drawIndexedIndirect(arg1, arg2);
        },
        __wbg_drawIndexed_55f6bf3bda0212ad: function(arg0, arg1, arg2, arg3, arg4, arg5) {
            arg0.drawIndexed(arg1 >>> 0, arg2 >>> 0, arg3 >>> 0, arg4, arg5 >>> 0);
        },
        __wbg_drawIndexed_9c9719597507e735: function(arg0, arg1, arg2, arg3, arg4, arg5) {
            arg0.drawIndexed(arg1 >>> 0, arg2 >>> 0, arg3 >>> 0, arg4, arg5 >>> 0);
        },
        __wbg_drawIndirect_73df189881970a43: function(arg0, arg1, arg2) {
            arg0.drawIndirect(arg1, arg2);
        },
        __wbg_drawIndirect_a2f7c719957f8ec9: function(arg0, arg1, arg2) {
            arg0.drawIndirect(arg1, arg2);
        },
        __wbg_draw_57caf8f0bc1ea050: function(arg0, arg1, arg2, arg3, arg4) {
            arg0.draw(arg1 >>> 0, arg2 >>> 0, arg3 >>> 0, arg4 >>> 0);
        },
        __wbg_draw_ce5e8b8ad56571cb: function(arg0, arg1, arg2, arg3, arg4) {
            arg0.draw(arg1 >>> 0, arg2 >>> 0, arg3 >>> 0, arg4 >>> 0);
        },
        __wbg_end_54134488dbc5b7a9: function(arg0) {
            arg0.end();
        },
        __wbg_end_57a2746c247f499a: function(arg0) {
            arg0.end();
        },
        __wbg_error_2acb88afe0ad9a3e: function(arg0) {
            const ret = arg0.error;
            return ret;
        },
        __wbg_error_757e9472f8410341: function(arg0, arg1) {
            let deferred0_0;
            let deferred0_1;
            try {
                deferred0_0 = arg0;
                deferred0_1 = arg1;
                console.error(getStringFromWasm0(arg0, arg1));
            } finally {
                wasm.__wbindgen_free(deferred0_0, deferred0_1, 1);
            }
        },
        __wbg_executeBundles_2905636f81aabf99: function(arg0, arg1) {
            arg0.executeBundles(arg1);
        },
        __wbg_features_30a76d141781ad80: function(arg0) {
            const ret = arg0.features;
            return ret;
        },
        __wbg_features_fdbd3daed26aa468: function(arg0) {
            const ret = arg0.features;
            return ret;
        },
        __wbg_finish_35be15c58b55a95b: function(arg0, arg1) {
            const ret = arg0.finish(arg1);
            return ret;
        },
        __wbg_finish_41491ca602373cde: function(arg0) {
            const ret = arg0.finish();
            return ret;
        },
        __wbg_finish_eb06372cc93f8d50: function(arg0, arg1) {
            const ret = arg0.finish(arg1);
            return ret;
        },
        __wbg_finish_ee515f526784acd5: function(arg0) {
            const ret = arg0.finish();
            return ret;
        },
        __wbg_getBindGroupLayout_aba26df848b4322d: function(arg0, arg1) {
            const ret = arg0.getBindGroupLayout(arg1 >>> 0);
            return ret;
        },
        __wbg_getBindGroupLayout_b9533489f3ee14df: function(arg0, arg1) {
            const ret = arg0.getBindGroupLayout(arg1 >>> 0);
            return ret;
        },
        __wbg_getBoundingClientRect_57152b1a20f3de34: function(arg0) {
            const ret = arg0.getBoundingClientRect();
            return ret;
        },
        __wbg_getCompilationInfo_b41435ddc0bb40c8: function(arg0) {
            const ret = arg0.getCompilationInfo();
            return ret;
        },
        __wbg_getContext_635e36719cad2623: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = arg0.getContext(getStringFromWasm0(arg1, arg2));
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        }, arguments); },
        __wbg_getContext_e567868594d2e0c6: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = arg0.getContext(getStringFromWasm0(arg1, arg2));
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        }, arguments); },
        __wbg_getCurrentTexture_6dc2cdde9bdc098d: function(arg0) {
            const ret = arg0.getCurrentTexture();
            return ret;
        },
        __wbg_getMappedRange_11ec4cfce4df1e72: function(arg0, arg1, arg2) {
            const ret = arg0.getMappedRange(arg1, arg2);
            return ret;
        },
        __wbg_getPreferredCanvasFormat_4314f4e4f5895771: function(arg0) {
            const ret = arg0.getPreferredCanvasFormat();
            return (__wbindgen_enum_GpuTextureFormat.indexOf(ret) + 1 || 96) - 1;
        },
        __wbg_getRandomValues_fc33165644b9e145: function() { return handleError(function (arg0, arg1) {
            globalThis.crypto.getRandomValues(getArrayU8FromWasm0(arg0, arg1));
        }, arguments); },
        __wbg_get_6c896e0571ddae51: function(arg0, arg1) {
            const ret = arg0[arg1 >>> 0];
            return ret;
        },
        __wbg_get_fd12a9100976c296: function(arg0, arg1) {
            const ret = arg0[arg1 >>> 0];
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_gpu_d9721d200584e919: function(arg0) {
            const ret = arg0.gpu;
            return ret;
        },
        __wbg_has_2184fc4b845f2b5f: function(arg0, arg1, arg2) {
            const ret = arg0.has(getStringFromWasm0(arg1, arg2));
            return ret;
        },
        __wbg_height_fb13ed9fe991b5f4: function(arg0) {
            const ret = arg0.height;
            return ret;
        },
        __wbg_instanceof_GpuAdapter_8825bf3533b2dc81: function(arg0) {
            let result;
            try {
                result = arg0 instanceof GPUAdapter;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_GpuCanvasContext_8867fd6a49dfb80b: function(arg0) {
            let result;
            try {
                result = arg0 instanceof GPUCanvasContext;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_GpuDeviceLostInfo_9385c1b1d1700172: function(arg0) {
            let result;
            try {
                result = arg0 instanceof GPUDeviceLostInfo;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_GpuOutOfMemoryError_ad32cc08223bf570: function(arg0) {
            let result;
            try {
                result = arg0 instanceof GPUOutOfMemoryError;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_GpuValidationError_2828a9f6f4ea2c0b: function(arg0) {
            let result;
            try {
                result = arg0 instanceof GPUValidationError;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_Object_67a83cdc00c5d141: function(arg0) {
            let result;
            try {
                result = arg0 instanceof Object;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_instanceof_Window_82d71df4eddf88bc: function(arg0) {
            let result;
            try {
                result = arg0 instanceof Window;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_label_cdc2b7a875dc5123: function(arg0, arg1) {
            const ret = arg1.label;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_left_e0a244490fe2f293: function(arg0) {
            const ret = arg0.left;
            return ret;
        },
        __wbg_length_7f3c00c40364105e: function(arg0) {
            const ret = arg0.length;
            return ret;
        },
        __wbg_length_87e0297027dd7802: function(arg0) {
            const ret = arg0.length;
            return ret;
        },
        __wbg_length_d4bdea10311bd9cf: function(arg0) {
            const ret = arg0.length;
            return ret;
        },
        __wbg_limits_5b3783fcc0d36428: function(arg0) {
            const ret = arg0.limits;
            return ret;
        },
        __wbg_limits_becc24c879d87717: function(arg0) {
            const ret = arg0.limits;
            return ret;
        },
        __wbg_lineNum_24517b98f306fcae: function(arg0) {
            const ret = arg0.lineNum;
            return ret;
        },
        __wbg_lost_2c34651e3317be8b: function(arg0) {
            const ret = arg0.lost;
            return ret;
        },
        __wbg_mapAsync_8d0ffc031e86e9a0: function(arg0, arg1, arg2, arg3) {
            const ret = arg0.mapAsync(arg1 >>> 0, arg2, arg3);
            return ret;
        },
        __wbg_maxBindGroups_5d3409c14d2756b5: function(arg0) {
            const ret = arg0.maxBindGroups;
            return ret;
        },
        __wbg_maxBindingsPerBindGroup_512a63ba20ee714c: function(arg0) {
            const ret = arg0.maxBindingsPerBindGroup;
            return ret;
        },
        __wbg_maxBufferSize_8cef5a2e6fae09fa: function(arg0) {
            const ret = arg0.maxBufferSize;
            return ret;
        },
        __wbg_maxColorAttachmentBytesPerSample_54d9c60b6cdd092a: function(arg0) {
            const ret = arg0.maxColorAttachmentBytesPerSample;
            return ret;
        },
        __wbg_maxColorAttachments_378f5fb1c453321d: function(arg0) {
            const ret = arg0.maxColorAttachments;
            return ret;
        },
        __wbg_maxComputeInvocationsPerWorkgroup_d8877398fe435d24: function(arg0) {
            const ret = arg0.maxComputeInvocationsPerWorkgroup;
            return ret;
        },
        __wbg_maxComputeWorkgroupSizeX_b6f88bafac1581bf: function(arg0) {
            const ret = arg0.maxComputeWorkgroupSizeX;
            return ret;
        },
        __wbg_maxComputeWorkgroupSizeY_e1a1ecdbdc9d75d8: function(arg0) {
            const ret = arg0.maxComputeWorkgroupSizeY;
            return ret;
        },
        __wbg_maxComputeWorkgroupSizeZ_fe66cf9606e1a594: function(arg0) {
            const ret = arg0.maxComputeWorkgroupSizeZ;
            return ret;
        },
        __wbg_maxComputeWorkgroupStorageSize_49c38f3e08b0f760: function(arg0) {
            const ret = arg0.maxComputeWorkgroupStorageSize;
            return ret;
        },
        __wbg_maxComputeWorkgroupsPerDimension_8cb3348843013a6b: function(arg0) {
            const ret = arg0.maxComputeWorkgroupsPerDimension;
            return ret;
        },
        __wbg_maxDynamicStorageBuffersPerPipelineLayout_6974d29539996dc2: function(arg0) {
            const ret = arg0.maxDynamicStorageBuffersPerPipelineLayout;
            return ret;
        },
        __wbg_maxDynamicUniformBuffersPerPipelineLayout_ade9d0536439985a: function(arg0) {
            const ret = arg0.maxDynamicUniformBuffersPerPipelineLayout;
            return ret;
        },
        __wbg_maxInterStageShaderComponents_d6dbbdabbd40588b: function(arg0) {
            const ret = arg0.maxInterStageShaderComponents;
            return ret;
        },
        __wbg_maxSampledTexturesPerShaderStage_e560c5b5b6029c57: function(arg0) {
            const ret = arg0.maxSampledTexturesPerShaderStage;
            return ret;
        },
        __wbg_maxSamplersPerShaderStage_28a8a2de2a3d656e: function(arg0) {
            const ret = arg0.maxSamplersPerShaderStage;
            return ret;
        },
        __wbg_maxStorageBufferBindingSize_984825203efcccc6: function(arg0) {
            const ret = arg0.maxStorageBufferBindingSize;
            return ret;
        },
        __wbg_maxStorageBuffersPerShaderStage_b81c4449fbcb39c3: function(arg0) {
            const ret = arg0.maxStorageBuffersPerShaderStage;
            return ret;
        },
        __wbg_maxStorageTexturesPerShaderStage_175a5e42917aedd2: function(arg0) {
            const ret = arg0.maxStorageTexturesPerShaderStage;
            return ret;
        },
        __wbg_maxTextureArrayLayers_8503bb6fd0cdb150: function(arg0) {
            const ret = arg0.maxTextureArrayLayers;
            return ret;
        },
        __wbg_maxTextureDimension1D_983c9a563c1855d9: function(arg0) {
            const ret = arg0.maxTextureDimension1D;
            return ret;
        },
        __wbg_maxTextureDimension2D_a0a2be37afbde706: function(arg0) {
            const ret = arg0.maxTextureDimension2D;
            return ret;
        },
        __wbg_maxTextureDimension3D_53aefd0d779b193e: function(arg0) {
            const ret = arg0.maxTextureDimension3D;
            return ret;
        },
        __wbg_maxUniformBufferBindingSize_8fc7ea016caf650c: function(arg0) {
            const ret = arg0.maxUniformBufferBindingSize;
            return ret;
        },
        __wbg_maxUniformBuffersPerShaderStage_b159f3442e264f35: function(arg0) {
            const ret = arg0.maxUniformBuffersPerShaderStage;
            return ret;
        },
        __wbg_maxVertexAttributes_9c129ee44a6fa783: function(arg0) {
            const ret = arg0.maxVertexAttributes;
            return ret;
        },
        __wbg_maxVertexBufferArrayStride_1d0f177a1fdcdf3c: function(arg0) {
            const ret = arg0.maxVertexBufferArrayStride;
            return ret;
        },
        __wbg_maxVertexBuffers_e5cf174a3497d472: function(arg0) {
            const ret = arg0.maxVertexBuffers;
            return ret;
        },
        __wbg_message_1b27ea1ad3998a9f: function(arg0, arg1) {
            const ret = arg1.message;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_message_a77e1a9202609622: function(arg0, arg1) {
            const ret = arg1.message;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_message_f762db05c1294eca: function(arg0, arg1) {
            const ret = arg1.message;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_messages_4e98c7e63c5efe7b: function(arg0) {
            const ret = arg0.messages;
            return ret;
        },
        __wbg_minStorageBufferOffsetAlignment_fe964dbc6a6d7ff3: function(arg0) {
            const ret = arg0.minStorageBufferOffsetAlignment;
            return ret;
        },
        __wbg_minUniformBufferOffsetAlignment_327ef98e308ca208: function(arg0) {
            const ret = arg0.minUniformBufferOffsetAlignment;
            return ret;
        },
        __wbg_navigator_2156486643462a87: function(arg0) {
            const ret = arg0.navigator;
            return ret;
        },
        __wbg_navigator_c9bce48d4b578c9c: function(arg0) {
            const ret = arg0.navigator;
            return ret;
        },
        __wbg_new_227d7c05414eb861: function() {
            const ret = new Error();
            return ret;
        },
        __wbg_new_617a8cdb8bb1130e: function() {
            const ret = new Object();
            return ret;
        },
        __wbg_new_ee2291f50781bf1d: function() {
            const ret = new Array();
            return ret;
        },
        __wbg_new_from_slice_9a868026ffa4208a: function(arg0, arg1) {
            const ret = new Uint8Array(getArrayU8FromWasm0(arg0, arg1));
            return ret;
        },
        __wbg_new_typed_b01cb72a8af741a3: function(arg0, arg1) {
            try {
                var state0 = {a: arg0, b: arg1};
                var cb0 = (arg0, arg1) => {
                    const a = state0.a;
                    state0.a = 0;
                    try {
                        return wasm_bindgen_e751eaa9cf7806ea___convert__closures_____invoke___js_sys_2a712b92d59091be___Function_fn_wasm_bindgen_e751eaa9cf7806ea___JsValue_____wasm_bindgen_e751eaa9cf7806ea___sys__Undefined___js_sys_2a712b92d59091be___Function_fn_wasm_bindgen_e751eaa9cf7806ea___JsValue_____wasm_bindgen_e751eaa9cf7806ea___sys__Undefined_______true_(a, state0.b, arg0, arg1);
                    } finally {
                        state0.a = a;
                    }
                };
                const ret = new Promise(cb0);
                return ret;
            } finally {
                state0.a = 0;
            }
        },
        __wbg_new_with_byte_offset_and_length_2f5d7fc2a828b74d: function(arg0, arg1, arg2) {
            const ret = new Uint8Array(arg0, arg1 >>> 0, arg2 >>> 0);
            return ret;
        },
        __wbg_offset_164492575e959c94: function(arg0) {
            const ret = arg0.offset;
            return ret;
        },
        __wbg_popErrorScope_2869a89dd4626f0c: function(arg0) {
            const ret = arg0.popErrorScope();
            return ret;
        },
        __wbg_prototypesetcall_bc27214492979395: function(arg0, arg1, arg2) {
            Uint8Array.prototype.set.call(getArrayU8FromWasm0(arg0, arg1), arg2);
        },
        __wbg_pushErrorScope_72e651b0f8f64c0e: function(arg0, arg1) {
            arg0.pushErrorScope(__wbindgen_enum_GpuErrorFilter[arg1]);
        },
        __wbg_push_2baf45db356cf468: function(arg0, arg1) {
            const ret = arg0.push(arg1);
            return ret;
        },
        __wbg_querySelectorAll_6aebfe3df1fb2014: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = arg0.querySelectorAll(getStringFromWasm0(arg1, arg2));
            return ret;
        }, arguments); },
        __wbg_queueMicrotask_9833f9a49df95a49: function(arg0) {
            const ret = arg0.queueMicrotask;
            return ret;
        },
        __wbg_queueMicrotask_a72f977e97f23c5f: function(arg0) {
            queueMicrotask(arg0);
        },
        __wbg_queue_6b07ccdd49a6ba90: function(arg0) {
            const ret = arg0.queue;
            return ret;
        },
        __wbg_reason_d7f4ddcad86f8d99: function(arg0) {
            const ret = arg0.reason;
            return (__wbindgen_enum_GpuDeviceLostReason.indexOf(ret) + 1 || 3) - 1;
        },
        __wbg_requestAdapter_e4b32f2647c66726: function(arg0, arg1) {
            const ret = arg0.requestAdapter(arg1);
            return ret;
        },
        __wbg_requestDevice_6130c3ba10d633f9: function(arg0, arg1) {
            const ret = arg0.requestDevice(arg1);
            return ret;
        },
        __wbg_resolveQuerySet_217f20ef3ebd6aed: function(arg0, arg1, arg2, arg3, arg4, arg5) {
            arg0.resolveQuerySet(arg1, arg2 >>> 0, arg3 >>> 0, arg4, arg5 >>> 0);
        },
        __wbg_resolve_0076e10020304ede: function(arg0) {
            const ret = Promise.resolve(arg0);
            return ret;
        },
        __wbg_run_6e1f787af721bfe6: function(arg0, arg1, arg2) {
            try {
                var state0 = {a: arg1, b: arg2};
                var cb0 = () => {
                    const a = state0.a;
                    state0.a = 0;
                    try {
                        return wasm_bindgen_e751eaa9cf7806ea___convert__closures_____invoke___bool__true_(a, state0.b, );
                    } finally {
                        state0.a = a;
                    }
                };
                const ret = arg0.run(cb0);
                return ret;
            } finally {
                state0.a = 0;
            }
        },
        __wbg_setBindGroup_1602c955be9b2eaa: function(arg0, arg1, arg2) {
            arg0.setBindGroup(arg1 >>> 0, arg2);
        },
        __wbg_setBindGroup_6149584f04998372: function(arg0, arg1, arg2, arg3, arg4, arg5, arg6) {
            arg0.setBindGroup(arg1 >>> 0, arg2, getArrayU32FromWasm0(arg3, arg4), arg5, arg6 >>> 0);
        },
        __wbg_setBindGroup_8d384b1c5ed329f4: function(arg0, arg1, arg2, arg3, arg4, arg5, arg6) {
            arg0.setBindGroup(arg1 >>> 0, arg2, getArrayU32FromWasm0(arg3, arg4), arg5, arg6 >>> 0);
        },
        __wbg_setBindGroup_9877b57492cb7e1c: function(arg0, arg1, arg2) {
            arg0.setBindGroup(arg1 >>> 0, arg2);
        },
        __wbg_setBindGroup_f4d552dcef65a491: function(arg0, arg1, arg2, arg3, arg4, arg5, arg6) {
            arg0.setBindGroup(arg1 >>> 0, arg2, getArrayU32FromWasm0(arg3, arg4), arg5, arg6 >>> 0);
        },
        __wbg_setBindGroup_f930832baeb4279b: function(arg0, arg1, arg2) {
            arg0.setBindGroup(arg1 >>> 0, arg2);
        },
        __wbg_setBlendConstant_257274277b0e3153: function(arg0, arg1) {
            arg0.setBlendConstant(arg1);
        },
        __wbg_setIndexBuffer_4219294fa3e2d59b: function(arg0, arg1, arg2, arg3, arg4) {
            arg0.setIndexBuffer(arg1, __wbindgen_enum_GpuIndexFormat[arg2], arg3, arg4);
        },
        __wbg_setIndexBuffer_5eb14c0c19ab80c2: function(arg0, arg1, arg2, arg3) {
            arg0.setIndexBuffer(arg1, __wbindgen_enum_GpuIndexFormat[arg2], arg3);
        },
        __wbg_setIndexBuffer_7e208bb69310ed01: function(arg0, arg1, arg2, arg3) {
            arg0.setIndexBuffer(arg1, __wbindgen_enum_GpuIndexFormat[arg2], arg3);
        },
        __wbg_setIndexBuffer_f0ab50b0e1d8658c: function(arg0, arg1, arg2, arg3, arg4) {
            arg0.setIndexBuffer(arg1, __wbindgen_enum_GpuIndexFormat[arg2], arg3, arg4);
        },
        __wbg_setPipeline_481f34ae14c49d67: function(arg0, arg1) {
            arg0.setPipeline(arg1);
        },
        __wbg_setPipeline_723820e1c5cc61e7: function(arg0, arg1) {
            arg0.setPipeline(arg1);
        },
        __wbg_setPipeline_f2cf83769bb33769: function(arg0, arg1) {
            arg0.setPipeline(arg1);
        },
        __wbg_setScissorRect_0578b1de90caf434: function(arg0, arg1, arg2, arg3, arg4) {
            arg0.setScissorRect(arg1 >>> 0, arg2 >>> 0, arg3 >>> 0, arg4 >>> 0);
        },
        __wbg_setStencilReference_7616273572b1075e: function(arg0, arg1) {
            arg0.setStencilReference(arg1 >>> 0);
        },
        __wbg_setVertexBuffer_54536e0e73bfc91e: function(arg0, arg1, arg2, arg3, arg4) {
            arg0.setVertexBuffer(arg1 >>> 0, arg2, arg3, arg4);
        },
        __wbg_setVertexBuffer_8dd1cb9fbc714a98: function(arg0, arg1, arg2, arg3) {
            arg0.setVertexBuffer(arg1 >>> 0, arg2, arg3);
        },
        __wbg_setVertexBuffer_c643d7ac0abf4554: function(arg0, arg1, arg2, arg3, arg4) {
            arg0.setVertexBuffer(arg1 >>> 0, arg2, arg3, arg4);
        },
        __wbg_setVertexBuffer_caad1ac6b71dea4a: function(arg0, arg1, arg2, arg3) {
            arg0.setVertexBuffer(arg1 >>> 0, arg2, arg3);
        },
        __wbg_setViewport_94128a2b1a708040: function(arg0, arg1, arg2, arg3, arg4, arg5, arg6) {
            arg0.setViewport(arg1, arg2, arg3, arg4, arg5, arg6);
        },
        __wbg_set_145a351398b48c65: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = Reflect.set(arg0, arg1, arg2);
            return ret;
        }, arguments); },
        __wbg_set_34d08fd992d43d61: function(arg0, arg1, arg2) {
            arg0.set(arg1, arg2 >>> 0);
        },
        __wbg_set_height_c9789c1c77eaedff: function(arg0, arg1) {
            arg0.height = arg1 >>> 0;
        },
        __wbg_set_height_fde391767df1ce27: function(arg0, arg1) {
            arg0.height = arg1 >>> 0;
        },
        __wbg_set_onuncapturederror_729c2e42c36923f4: function(arg0, arg1) {
            arg0.onuncapturederror = arg1;
        },
        __wbg_set_width_0ca908d167690992: function(arg0, arg1) {
            arg0.width = arg1 >>> 0;
        },
        __wbg_set_width_b0e1267db4b196b5: function(arg0, arg1) {
            arg0.width = arg1 >>> 0;
        },
        __wbg_size_1dfbf7241f9df1cc: function(arg0) {
            const ret = arg0.size;
            return ret;
        },
        __wbg_stack_3b0d974bbf31e44f: function(arg0, arg1) {
            const ret = arg1.stack;
            const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
            const len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg_static_accessor_CREATE_TASK_7e310091636beacc: function() {
            const ret = typeof console === 'undefined' ? null : console?.createTask;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_static_accessor_GLOBAL_266715b9d96ba635: function() {
            const ret = typeof global === 'undefined' ? null : global;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_static_accessor_GLOBAL_THIS_10fb7dc1ae063179: function() {
            const ret = typeof globalThis === 'undefined' ? null : globalThis;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_static_accessor_SELF_0b583911f537483a: function() {
            const ret = typeof self === 'undefined' ? null : self;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_static_accessor_WINDOW_d7f903d1508cbdc4: function() {
            const ret = typeof window === 'undefined' ? null : window;
            return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
        },
        __wbg_submit_60f2469dc00130cc: function(arg0, arg1) {
            arg0.submit(arg1);
        },
        __wbg_then_4e48da6c0a644c56: function(arg0, arg1, arg2) {
            const ret = arg0.then(arg1, arg2);
            return ret;
        },
        __wbg_then_c8a35d4ad59c6b8e: function(arg0, arg1) {
            const ret = arg0.then(arg1);
            return ret;
        },
        __wbg_then_c949d5a25a4e78f8: function(arg0, arg1, arg2) {
            const ret = arg0.then(arg1, arg2);
            return ret;
        },
        __wbg_then_e71170d78fcf8954: function(arg0, arg1) {
            const ret = arg0.then(arg1);
            return ret;
        },
        __wbg_top_ff4627294d2cdeb8: function(arg0) {
            const ret = arg0.top;
            return ret;
        },
        __wbg_type_4b0a304ebc25e195: function(arg0) {
            const ret = arg0.type;
            return (__wbindgen_enum_GpuCompilationMessageType.indexOf(ret) + 1 || 4) - 1;
        },
        __wbg_unmap_4aa38f8c5283cc1d: function(arg0) {
            arg0.unmap();
        },
        __wbg_usage_ee2982f59567c06f: function(arg0) {
            const ret = arg0.usage;
            return ret;
        },
        __wbg_valueOf_02fd4d101b7553c7: function(arg0) {
            const ret = arg0.valueOf();
            return ret;
        },
        __wbg_width_d5e379bde85b6eaa: function(arg0) {
            const ret = arg0.width;
            return ret;
        },
        __wbg_writeBuffer_b5e6e8f3f93629bc: function(arg0, arg1, arg2, arg3, arg4, arg5) {
            arg0.writeBuffer(arg1, arg2, arg3, arg4, arg5);
        },
        __wbg_writeTexture_57e41dd94bac65c4: function(arg0, arg1, arg2, arg3, arg4) {
            arg0.writeTexture(arg1, arg2, arg3, arg4);
        },
        __wbindgen_generic_0000000000000001: function(arg0, arg1) {
            // Cast intrinsic for `Closure(Closure { owned: true, function: Function { arguments: [Externref], shim_idx: 577, ret: Unit, inner_ret: Some(Unit) }, mutable: true }) -> Externref`.
            const ret = makeMutClosure(arg0, arg1, wasm_bindgen_e751eaa9cf7806ea___convert__closures_____invoke___wasm_bindgen_e751eaa9cf7806ea___JsValue______true_);
            return ret;
        },
        __wbindgen_generic_0000000000000002: function(arg0, arg1) {
            // Cast intrinsic for `Closure(Closure { owned: true, function: Function { arguments: [Externref], shim_idx: 602, ret: Result(Unit), inner_ret: Some(Result(Unit)) }, mutable: true }) -> Externref`.
            const ret = makeMutClosure(arg0, arg1, wasm_bindgen_e751eaa9cf7806ea___convert__closures_____invoke___wasm_bindgen_e751eaa9cf7806ea___JsValue__core_7d5f0a2ba6a62c33___result__Result_____wasm_bindgen_e751eaa9cf7806ea___JsError___true_);
            return ret;
        },
        __wbindgen_generic_0000000000000003: function(arg0, arg1) {
            // Cast intrinsic for `Closure(Closure { owned: true, function: Function { arguments: [NamedExternref("GPUUncapturedErrorEvent")], shim_idx: 578, ret: Unit, inner_ret: Some(Unit) }, mutable: true }) -> Externref`.
            const ret = makeMutClosure(arg0, arg1, wasm_bindgen_e751eaa9cf7806ea___convert__closures_____invoke___wgpu_cf189251abdffb5b___backend__webgpu__webgpu_sys__gen_GpuUncapturedErrorEvent__GpuUncapturedErrorEvent______true_);
            return ret;
        },
        __wbindgen_generic_0000000000000004: function(arg0) {
            // Cast intrinsic for `F64 -> Externref`.
            const ret = arg0;
            return ret;
        },
        __wbindgen_generic_0000000000000005: function(arg0, arg1) {
            // Cast intrinsic for `Ref(Slice(U8)) -> NamedExternref("Uint8Array")`.
            const ret = getArrayU8FromWasm0(arg0, arg1);
            return ret;
        },
        __wbindgen_generic_0000000000000006: function(arg0, arg1) {
            // Cast intrinsic for `Ref(String) -> Externref`.
            const ret = getStringFromWasm0(arg0, arg1);
            return ret;
        },
        __wbindgen_init_externref_table: function() {
            const table = wasm.__wbindgen_externrefs;
            const offset = table.grow(4);
            table.set(0, undefined);
            table.set(offset + 0, undefined);
            table.set(offset + 1, null);
            table.set(offset + 2, true);
            table.set(offset + 3, false);
        },
    };
    return {
        __proto__: null,
        "./vectra_wasm_bg.js": import0,
    };
}

function wasm_bindgen_e751eaa9cf7806ea___convert__closures_____invoke___bool__true_(arg0, arg1) {
    const ret = wasm.wasm_bindgen_e751eaa9cf7806ea___convert__closures_____invoke___bool__true_(arg0, arg1);
    return ret !== 0;
}

function wasm_bindgen_e751eaa9cf7806ea___convert__closures_____invoke___wasm_bindgen_e751eaa9cf7806ea___JsValue______true_(arg0, arg1, arg2) {
    wasm.wasm_bindgen_e751eaa9cf7806ea___convert__closures_____invoke___wasm_bindgen_e751eaa9cf7806ea___JsValue______true_(arg0, arg1, arg2);
}

function wasm_bindgen_e751eaa9cf7806ea___convert__closures_____invoke___wgpu_cf189251abdffb5b___backend__webgpu__webgpu_sys__gen_GpuUncapturedErrorEvent__GpuUncapturedErrorEvent______true_(arg0, arg1, arg2) {
    wasm.wasm_bindgen_e751eaa9cf7806ea___convert__closures_____invoke___wgpu_cf189251abdffb5b___backend__webgpu__webgpu_sys__gen_GpuUncapturedErrorEvent__GpuUncapturedErrorEvent______true_(arg0, arg1, arg2);
}

function wasm_bindgen_e751eaa9cf7806ea___convert__closures_____invoke___wasm_bindgen_e751eaa9cf7806ea___JsValue__core_7d5f0a2ba6a62c33___result__Result_____wasm_bindgen_e751eaa9cf7806ea___JsError___true_(arg0, arg1, arg2) {
    const ret = wasm.wasm_bindgen_e751eaa9cf7806ea___convert__closures_____invoke___wasm_bindgen_e751eaa9cf7806ea___JsValue__core_7d5f0a2ba6a62c33___result__Result_____wasm_bindgen_e751eaa9cf7806ea___JsError___true_(arg0, arg1, arg2);
    if (ret[1]) {
        throw takeFromExternrefTable0(ret[0]);
    }
}

function wasm_bindgen_e751eaa9cf7806ea___convert__closures_____invoke___js_sys_2a712b92d59091be___Function_fn_wasm_bindgen_e751eaa9cf7806ea___JsValue_____wasm_bindgen_e751eaa9cf7806ea___sys__Undefined___js_sys_2a712b92d59091be___Function_fn_wasm_bindgen_e751eaa9cf7806ea___JsValue_____wasm_bindgen_e751eaa9cf7806ea___sys__Undefined_______true_(arg0, arg1, arg2, arg3) {
    wasm.wasm_bindgen_e751eaa9cf7806ea___convert__closures_____invoke___js_sys_2a712b92d59091be___Function_fn_wasm_bindgen_e751eaa9cf7806ea___JsValue_____wasm_bindgen_e751eaa9cf7806ea___sys__Undefined___js_sys_2a712b92d59091be___Function_fn_wasm_bindgen_e751eaa9cf7806ea___JsValue_____wasm_bindgen_e751eaa9cf7806ea___sys__Undefined_______true_(arg0, arg1, arg2, arg3);
}


const __wbindgen_enum_GpuCompilationMessageType = ["error", "warning", "info"];


const __wbindgen_enum_GpuDeviceLostReason = ["unknown", "destroyed"];


const __wbindgen_enum_GpuErrorFilter = ["validation", "out-of-memory", "internal"];


const __wbindgen_enum_GpuIndexFormat = ["uint16", "uint32"];


const __wbindgen_enum_GpuTextureFormat = ["r8unorm", "r8snorm", "r8uint", "r8sint", "r16uint", "r16sint", "r16float", "rg8unorm", "rg8snorm", "rg8uint", "rg8sint", "r32uint", "r32sint", "r32float", "rg16uint", "rg16sint", "rg16float", "rgba8unorm", "rgba8unorm-srgb", "rgba8snorm", "rgba8uint", "rgba8sint", "bgra8unorm", "bgra8unorm-srgb", "rgb9e5ufloat", "rgb10a2uint", "rgb10a2unorm", "rg11b10ufloat", "rg32uint", "rg32sint", "rg32float", "rgba16uint", "rgba16sint", "rgba16float", "rgba32uint", "rgba32sint", "rgba32float", "stencil8", "depth16unorm", "depth24plus", "depth24plus-stencil8", "depth32float", "depth32float-stencil8", "bc1-rgba-unorm", "bc1-rgba-unorm-srgb", "bc2-rgba-unorm", "bc2-rgba-unorm-srgb", "bc3-rgba-unorm", "bc3-rgba-unorm-srgb", "bc4-r-unorm", "bc4-r-snorm", "bc5-rg-unorm", "bc5-rg-snorm", "bc6h-rgb-ufloat", "bc6h-rgb-float", "bc7-rgba-unorm", "bc7-rgba-unorm-srgb", "etc2-rgb8unorm", "etc2-rgb8unorm-srgb", "etc2-rgb8a1unorm", "etc2-rgb8a1unorm-srgb", "etc2-rgba8unorm", "etc2-rgba8unorm-srgb", "eac-r11unorm", "eac-r11snorm", "eac-rg11unorm", "eac-rg11snorm", "astc-4x4-unorm", "astc-4x4-unorm-srgb", "astc-5x4-unorm", "astc-5x4-unorm-srgb", "astc-5x5-unorm", "astc-5x5-unorm-srgb", "astc-6x5-unorm", "astc-6x5-unorm-srgb", "astc-6x6-unorm", "astc-6x6-unorm-srgb", "astc-8x5-unorm", "astc-8x5-unorm-srgb", "astc-8x6-unorm", "astc-8x6-unorm-srgb", "astc-8x8-unorm", "astc-8x8-unorm-srgb", "astc-10x5-unorm", "astc-10x5-unorm-srgb", "astc-10x6-unorm", "astc-10x6-unorm-srgb", "astc-10x8-unorm", "astc-10x8-unorm-srgb", "astc-10x10-unorm", "astc-10x10-unorm-srgb", "astc-12x10-unorm", "astc-12x10-unorm-srgb", "astc-12x12-unorm", "astc-12x12-unorm-srgb"];
const RendererFinalization = (typeof FinalizationRegistry === 'undefined')
    ? { register: () => {}, unregister: () => {} }
    : new FinalizationRegistry(ptr => wasm.__wbg_renderer_free(ptr, 1));
const VectraEngineFinalization = (typeof FinalizationRegistry === 'undefined')
    ? { register: () => {}, unregister: () => {} }
    : new FinalizationRegistry(ptr => wasm.__wbg_vectraengine_free(ptr, 1));

function addToExternrefTable0(obj) {
    const idx = wasm.__externref_table_alloc();
    wasm.__wbindgen_externrefs.set(idx, obj);
    return idx;
}

function _assertClass(instance, klass) {
    if (!(instance instanceof klass)) {
        throw new Error(`expected instance of ${klass.name}`);
    }
}

const CLOSURE_DTORS = (typeof FinalizationRegistry === 'undefined')
    ? { register: () => {}, unregister: () => {} }
    : new FinalizationRegistry(state => wasm.__wbindgen_destroy_closure(state.a, state.b));

function debugString(val) {
    // primitive types
    const type = typeof val;
    if (type == 'number' || type == 'boolean' || val == null) {
        return  `${val}`;
    }
    if (type == 'string') {
        return `"${val}"`;
    }
    if (type == 'symbol') {
        const description = val.description;
        if (description == null) {
            return 'Symbol';
        } else {
            return `Symbol(${description})`;
        }
    }
    if (type == 'function') {
        const name = val.name;
        if (typeof name == 'string' && name.length > 0) {
            return `Function(${name})`;
        } else {
            return 'Function';
        }
    }
    // objects
    if (Array.isArray(val)) {
        const length = val.length;
        let debug = '[';
        if (length > 0) {
            debug += debugString(val[0]);
        }
        for(let i = 1; i < length; i++) {
            debug += ', ' + debugString(val[i]);
        }
        debug += ']';
        return debug;
    }
    // Test for built-in
    const builtInMatches = /\[object ([^\]]+)\]/.exec(toString.call(val));
    let className;
    if (builtInMatches && builtInMatches.length > 1) {
        className = builtInMatches[1];
    } else {
        // Failed to match the standard '[object ClassName]'
        return toString.call(val);
    }
    if (className == 'Object') {
        // we're a user defined class or Object
        // JSON.stringify avoids problems with cycles, and is generally much
        // easier than looping through ownProperties of `val`.
        try {
            return 'Object(' + JSON.stringify(val) + ')';
        } catch (_) {
            return 'Object';
        }
    }
    // errors
    if (val instanceof Error) {
        return `${val.name}: ${val.message}\n${val.stack}`;
    }
    // TODO we could test for more things here, like `Set`s and `Map`s.
    return className;
}

function getArrayU32FromWasm0(ptr, len) {
    ptr = ptr >>> 0;
    return getUint32ArrayMemory0().subarray(ptr / 4, ptr / 4 + len);
}

function getArrayU8FromWasm0(ptr, len) {
    ptr = ptr >>> 0;
    return getUint8ArrayMemory0().subarray(ptr / 1, ptr / 1 + len);
}

let cachedDataViewMemory0 = null;
function getDataViewMemory0() {
    if (cachedDataViewMemory0 === null || cachedDataViewMemory0.buffer.detached === true || (cachedDataViewMemory0.buffer.detached === undefined && cachedDataViewMemory0.buffer !== wasm.memory.buffer)) {
        cachedDataViewMemory0 = new DataView(wasm.memory.buffer);
    }
    return cachedDataViewMemory0;
}

function getStringFromWasm0(ptr, len) {
    return decodeText(ptr >>> 0, len);
}

let cachedUint32ArrayMemory0 = null;
function getUint32ArrayMemory0() {
    if (cachedUint32ArrayMemory0 === null || cachedUint32ArrayMemory0.byteLength === 0) {
        cachedUint32ArrayMemory0 = new Uint32Array(wasm.memory.buffer);
    }
    return cachedUint32ArrayMemory0;
}

let cachedUint8ArrayMemory0 = null;
function getUint8ArrayMemory0() {
    if (cachedUint8ArrayMemory0 === null || cachedUint8ArrayMemory0.byteLength === 0) {
        cachedUint8ArrayMemory0 = new Uint8Array(wasm.memory.buffer);
    }
    return cachedUint8ArrayMemory0;
}

function handleError(f, args) {
    try {
        return f.apply(this, args);
    } catch (e) {
        const idx = addToExternrefTable0(e);
        wasm.__wbindgen_exn_store(idx);
    }
}

function isLikeNone(x) {
    return x === undefined || x === null;
}

function makeMutClosure(arg0, arg1, f) {
    const state = { a: arg0, b: arg1, cnt: 1 };
    const real = (...args) => {

        // First up with a closure we increment the internal reference
        // count. This ensures that the Rust closure environment won't
        // be deallocated while we're invoking it.
        state.cnt++;
        const a = state.a;
        state.a = 0;
        try {
            return f(a, state.b, ...args);
        } finally {
            state.a = a;
            real._wbg_cb_unref();
        }
    };
    real._wbg_cb_unref = () => {
        if (--state.cnt === 0) {
            wasm.__wbindgen_destroy_closure(state.a, state.b);
            state.a = 0;
            CLOSURE_DTORS.unregister(state);
        }
    };
    CLOSURE_DTORS.register(real, state, state);
    return real;
}

function passArray8ToWasm0(arg, malloc) {
    const ptr = malloc(arg.length * 1, 1) >>> 0;
    getUint8ArrayMemory0().set(arg, ptr / 1);
    WASM_VECTOR_LEN = arg.length;
    return ptr;
}

function passStringToWasm0(arg, malloc, realloc) {
    if (realloc === undefined) {
        const buf = cachedTextEncoder.encode(arg);
        const ptr = malloc(buf.length, 1) >>> 0;
        getUint8ArrayMemory0().subarray(ptr, ptr + buf.length).set(buf);
        WASM_VECTOR_LEN = buf.length;
        return ptr;
    }

    let len = arg.length;
    let ptr = malloc(len, 1) >>> 0;

    const mem = getUint8ArrayMemory0();

    let offset = 0;

    for (; offset < len; offset++) {
        const code = arg.charCodeAt(offset);
        if (code > 0x7F) break;
        mem[ptr + offset] = code;
    }
    if (offset !== len) {
        if (offset !== 0) {
            arg = arg.slice(offset);
        }
        ptr = realloc(ptr, len, len = offset + arg.length * 3, 1) >>> 0;
        const view = getUint8ArrayMemory0().subarray(ptr + offset, ptr + len);
        const ret = cachedTextEncoder.encodeInto(arg, view);

        offset += ret.written;
        ptr = realloc(ptr, len, offset, 1) >>> 0;
    }

    WASM_VECTOR_LEN = offset;
    return ptr;
}

function takeFromExternrefTable0(idx) {
    const value = wasm.__wbindgen_externrefs.get(idx);
    wasm.__externref_table_dealloc(idx);
    return value;
}

let cachedTextDecoder = new TextDecoder('utf-8', { ignoreBOM: true, fatal: true });
cachedTextDecoder.decode();
const MAX_SAFARI_DECODE_BYTES = 2146435072;
let numBytesDecoded = 0;
function decodeText(ptr, len) {
    numBytesDecoded += len;
    if (numBytesDecoded >= MAX_SAFARI_DECODE_BYTES) {
        cachedTextDecoder = new TextDecoder('utf-8', { ignoreBOM: true, fatal: true });
        cachedTextDecoder.decode();
        numBytesDecoded = len;
    }
    return cachedTextDecoder.decode(getUint8ArrayMemory0().subarray(ptr, ptr + len));
}

const cachedTextEncoder = new TextEncoder();

if (!('encodeInto' in cachedTextEncoder)) {
    cachedTextEncoder.encodeInto = function (arg, view) {
        const buf = cachedTextEncoder.encode(arg);
        view.set(buf);
        return {
            read: arg.length,
            written: buf.length
        };
    };
}

let WASM_VECTOR_LEN = 0;

let wasmModule, wasmInstance, wasm;
function __wbg_finalize_init(instance, module) {
    wasmInstance = instance;
    wasm = instance.exports;
    wasmModule = module;
    cachedDataViewMemory0 = null;
    cachedUint32ArrayMemory0 = null;
    cachedUint8ArrayMemory0 = null;
    wasm.__wbindgen_start();
    return wasm;
}

async function __wbg_load(module, imports) {
    if (typeof Response === 'function' && module instanceof Response) {
        if (!module.ok) {
            throw new Error(`failed to fetch Wasm: ${module.status} ${module.statusText} fetching '${module.url}'`);
        }

        if (typeof WebAssembly.instantiateStreaming === 'function') {
            try {
                return await WebAssembly.instantiateStreaming(module, imports);
            } catch (e) {
                const validResponse = expectedResponseType(module.type);

                if (validResponse && module.headers.get('Content-Type') !== 'application/wasm') {
                    console.warn("`WebAssembly.instantiateStreaming` failed because your server does not serve Wasm with `application/wasm` MIME type. Falling back to `WebAssembly.instantiate` which is slower. Original error:\n", e);

                } else { throw e; }
            }
        }

        const bytes = await module.arrayBuffer();
        return await WebAssembly.instantiate(bytes, imports);
    } else {
        const instance = await WebAssembly.instantiate(module, imports);

        if (instance instanceof WebAssembly.Instance) {
            return { instance, module };
        } else {
            return instance;
        }
    }

    function expectedResponseType(type) {
        switch (type) {
            case 'basic': case 'cors': case 'default': return true;
        }
        return false;
    }
}

function initSync(module) {
    if (wasm !== undefined) return wasm;


    if (module !== undefined) {
        if (Object.getPrototypeOf(module) === Object.prototype) {
            ({module} = module)
        } else {
            console.warn('using deprecated parameters for `initSync()`; pass a single object instead')
        }
    }

    const imports = __wbg_get_imports();
    if (!(module instanceof WebAssembly.Module)) {
        module = new WebAssembly.Module(module);
    }
    const instance = new WebAssembly.Instance(module, imports);
    return __wbg_finalize_init(instance, module);
}

async function __wbg_init(module_or_path) {
    if (wasm !== undefined) return wasm;


    if (module_or_path !== undefined) {
        if (Object.getPrototypeOf(module_or_path) === Object.prototype) {
            ({module_or_path} = module_or_path)
        } else {
            console.warn('using deprecated parameters for the initialization function; pass a single object instead')
        }
    }

    if (module_or_path === undefined) {
        module_or_path = new URL('vectra_wasm_bg.wasm', import.meta.url);
    }
    const imports = __wbg_get_imports();

    if (typeof module_or_path === 'string' || (typeof Request === 'function' && module_or_path instanceof Request) || (typeof URL === 'function' && module_or_path instanceof URL)) {
        module_or_path = fetch(module_or_path);
    }

    const { instance, module } = await __wbg_load(await module_or_path, imports);

    return __wbg_finalize_init(instance, module);
}

export { initSync, __wbg_init as default };
