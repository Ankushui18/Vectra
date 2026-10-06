/**
 * WASM engine client (Tasks 1.4 + 5.0).
 *
 * Owns the module lifecycle (async init, singleton) and the JSON round-trip:
 * typed `CommandWire` in, parsed `CommandResponseWire` / `SnapshotWire` out.
 * Every method is synchronous after init — the engine never blocks on I/O.
 *
 * Task 5.0 added the *canvas* half: the WebGPU `Renderer` is created here, next
 * to the engine it draws, and the two are joined by `renderFrame()`. The React
 * component never holds either object — it asks this client for a frame, for the
 * node under a pointer, and for the document point under a pointer, and gets
 * plain data back.
 */

import init, { Renderer, VectraEngine } from '../wasm/vectra_wasm.js';
import wasmUrl from '../wasm/vectra_wasm_bg.wasm?url';
import type {
  AiCallWire,
  DocToClientWire,
  DrawHitWire,
  DrawOverlayWire,
  DrawReplyWire,
  AiExecuteWire,
  CanvasStatusWire,
  CanvasViewWire,
  CommandResponseWire,
  CommandWire,
  DependencyResponseWire,
  ExportEnvelopeWire,
  FrameWire,
  MotionTrackWire,
  MotionWire,
  ProceduralKindOptionWire,
  ProceduralReportWire,
  SnapshotWire,
} from './wire';

/** What a client-side point maps to: document coordinates, or nothing. */
export interface DocPoint {
  x: number;
  y: number;
}

let enginePromise: Promise<VectraEngine> | null = null;

/** Load + instantiate the engine exactly once (StrictMode-safe). */
export function getEngine(): Promise<VectraEngine> {
  if (!enginePromise) {
    enginePromise = (async () => {
      // Explicit WASM URL: works identically in `vite dev` and `vite build`
      // (bundled as a hashed asset) with no WASM plugins required.
      // Object form: direct input is deprecated in wasm-bindgen ≥ 0.2.126.
      await init({ module_or_path: wasmUrl });
      return new VectraEngine();
    })();
  }
  return enginePromise;
}

export class VectraClient {
  /** The canvas renderer, once `attachCanvas` has been called. */
  private renderer: Renderer | null = null;

  private constructor(private readonly engine: VectraEngine) {}

  static async create(): Promise<VectraClient> {
    return new VectraClient(await getEngine());
  }

  // ── Canvas (Task 5.0 §6) ─────────────────────────────────────────────

  /**
   * Hand the renderer an HTML `<canvas>` (React passes the element on mount).
   *
   * This is the only asynchronous call in the client after init: WebGPU's
   * adapter request is a browser promise. A browser without WebGPU resolves with
   * `ready: false` and a message instead of rejecting — the UI can explain
   * itself, and every other method keeps working (hit testing is CPU-side).
   */
  async attachCanvas(canvas: HTMLCanvasElement): Promise<CanvasStatusWire> {
    const renderer = new Renderer();
    const status = JSON.parse(await renderer.attach(canvas)) as CanvasStatusWire;
    this.renderer = renderer;
    return status;
  }

  /** Re-read the canvas box (mount, resize, device-pixel-ratio change). */
  measureCanvas(canvas: HTMLCanvasElement): CanvasViewWire | null {
    if (!this.renderer) return null;
    return JSON.parse(this.renderer.measure(canvas)) as CanvasViewWire;
  }

  /**
   * Draw one frame: the engine hands the renderer its evaluated scene plus the
   * nodes that changed since the last frame, and returns what the GPU paid.
   */
  renderFrame(): FrameWire | null {
    if (!this.renderer) return null;
    return JSON.parse(this.engine.render_frame(this.renderer)) as FrameWire;
  }

  /**
   * RULE 3: the node under a client-space point, or `null` for empty space.
   *
   * The engine's answer is a `NodeId` string; wasm-bindgen turns the renderer's
   * `Option::None` into `undefined`, and this normalises a miss to `null` so
   * callers test one thing ("did anything answer?") rather than two.
   */
  pointerHit(clientX: number, clientY: number): string | null {
    return this.renderer?.pointer_hit(clientX, clientY) ?? null;
  }

  /** The document point under a client-space point (a view transform). */
  pointerDoc(clientX: number, clientY: number): DocPoint | null {
    const raw = this.renderer?.pointer_doc(clientX, clientY);
    return raw && raw !== 'null' ? (JSON.parse(raw) as DocPoint) : null;
  }

  /**
   * **Frame the canvas on a document rectangle** (Task 10.2 RULE 2) — the
   * camera half of "jump to artboard".
   *
   * The UI passes the rectangle it read off the engine's artboard record; the
   * renderer owns the projection, exactly as it owns every other screen↔document
   * mapping.
   */
  frameDocument(rect: { x: number; y: number; width: number; height: number }): CanvasViewWire | null {
    if (!this.renderer) return null;
    return JSON.parse(
      this.renderer.frame_document(rect.x, rect.y, rect.width, rect.height),
    ) as CanvasViewWire;
  }

  /** Back to the opening view of the whole document. */
  frameDocumentDefault(): CanvasViewWire | null {
    if (!this.renderer) return null;
    return JSON.parse(this.renderer.frame_document_default()) as CanvasViewWire;
  }

  // ── Navigation (Task 10.3 RULE 2) ────────────────────────────────────
  //
  // Four entry points, one per gesture the canvas has: a wheel, a pan drag, a
  // preset zoom, and the readout. Each returns the *view* the renderer now
  // holds, so the overlay follows the gesture in the same call — there is no
  // second opinion about where the camera is.

  /** Pan by a pointer delta in **CSS** pixels (the pointer's own movement). */
  navPan(dx: number, dy: number): CanvasViewWire | null {
    if (!this.renderer) return null;
    return JSON.parse(this.renderer.nav_pan(dx, dy)) as CanvasViewWire;
  }

  /** Zoom about a **client-space** position — one wheel notch or a pinch. */
  navZoom(clientX: number, clientY: number, factor: number): CanvasViewWire | null {
    if (!this.renderer) return null;
    return JSON.parse(this.renderer.nav_zoom(clientX, clientY, factor)) as CanvasViewWire;
  }

  /** Zoom to an absolute scale in CSS pixels per unit (`1` = "100%"). */
  navZoomTo(scale: number): CanvasViewWire | null {
    if (!this.renderer) return null;
    return JSON.parse(this.renderer.nav_zoom_to(scale)) as CanvasViewWire;
  }

  /** The current zoom, in CSS pixels per document unit. */
  navScale(): number {
    return this.renderer?.nav_scale() ?? 1;
  }

  /** The renderer's own totals (device, buffers, frames, last error). */
  canvasStatus(): CanvasStatusWire | null {
    if (!this.renderer) return null;
    return JSON.parse(this.renderer.stats()) as CanvasStatusWire;
  }

  /** The document rectangle the canvas is currently showing. */
  canvasView(): CanvasViewWire | null {
    if (!this.renderer) return null;
    return JSON.parse(this.renderer.view()) as CanvasViewWire;
  }

  // ── Motion (Task 6.0) ────────────────────────────────────────────────
  //
  // The two binders exist because a binding needs an *anchor* — the value the
  // slot holds right now, and the engine's clock — and neither is knowable from
  // here. The UI sends the intent (which slot, which target, how stiff) and the
  // engine writes the document. Everything else is a plain input: a hover flag,
  // a pointer position, the idle question.

  /**
   * Bind a spring to a slot, anchored at the slot's current value and the
   * current engine time. Undoable — the inverse restores whatever the slot held.
   */
  bindSpring(
    nodeId: string,
    property: string,
    target: number,
    stiffness: number,
    damping: number,
  ): CommandResponseWire {
    return JSON.parse(
      this.engine.bind_spring(nodeId, property, target, stiffness, damping),
    ) as CommandResponseWire;
  }

  /**
   * Bind a spring that follows this node's `hover:` flag: `off` at rest, `on`
   * while the pointer is over the shape. The flag is the engine's (derived from
   * the document), never the UI's.
   */
  bindHoverSpring(
    nodeId: string,
    property: string,
    off: number,
    on: number,
    stiffness: number,
    damping: number,
  ): CommandResponseWire {
    return JSON.parse(
      this.engine.bind_hover_spring(nodeId, property, off, on, stiffness, damping),
    ) as CommandResponseWire;
  }

  /**
   * Set a state flag. An **input**, not an edit: it never enters the undo stack,
   * and a genuine flip re-anchors the springs that read the flag at the value
   * they hold at that instant.
   */
  setState(name: string, on: boolean): CommandResponseWire {
    return JSON.parse(this.engine.set_state(name, on)) as CommandResponseWire;
  }

  /** Report the pointer's document-space position; the engine hit-tests it. */
  pointerMove(x: number, y: number): CommandResponseWire {
    if (!this.renderer) return { status: 'ok', events: [] };
    return JSON.parse(
      this.engine.pointer_move(this.renderer, x, y),
    ) as CommandResponseWire;
  }

  /** The pointer left the canvas: nothing is hovered. */
  pointerLeave(): CommandResponseWire {
    return JSON.parse(this.engine.pointer_leave()) as CommandResponseWire;
  }

  /**
   * The idle signal (Frame Budget Law): `false` means the document owes the
   * clock nothing, so the frame loop must stop scheduling frames.
   */
  isAnimating(): boolean {
    return this.engine.is_animating();
  }

  /** Motion state for the inspector: moving?, until when?, bound where? */
  motion(): MotionWire {
    return JSON.parse(this.engine.motion_json()) as MotionWire;
  }

  // ── Procedural (Task 7.0) ───────────────────────────────────────────
  //
  // Two reads and nothing else: the palette (which kinds exist, with the
  // operand payload the add command must carry) and the chain's own report.
  // Every *mutation* is an ordinary typed command through `dispatch`.

  /** The engine's palette of procedural kinds (`procedural_kinds`). */
  proceduralKinds(): ProceduralKindOptionWire[] {
    return JSON.parse(this.engine.procedural_kinds()) as ProceduralKindOptionWire[];
  }

  /** The chain: order, effective operands, wires, published ports. */
  procedural(): ProceduralReportWire {
    return JSON.parse(this.engine.procedural_json()) as ProceduralReportWire;
  }

  /** The drawing as a semantic SVG document (Task 8.0, RULE 1). */
  exportToSvg(): ExportEnvelopeWire {
    return JSON.parse(this.engine.export_to_svg()) as ExportEnvelopeWire;
  }

  /**
   * **Export current artboard** (Task 10.2 RULE 2): the active board's frame and
   * its artwork, as SVG.
   *
   * The engine decides what belongs to the board; this method only carries the
   * envelope. That is why there is no second SVG code path in the UI — the
   * "Export current" button and the "Export" menu produce the same kind of file,
   * from the same exporter.
   */
  exportCurrentArtboard(): ExportEnvelopeWire {
    return JSON.parse(this.engine.export_current_artboard()) as ExportEnvelopeWire;
  }

  /** **Export all artboards** (Task 10.2 RULE 2): every board, in one file, laid
   *  out where each sits in document space. */
  exportAllArtboards(): ExportEnvelopeWire {
    return JSON.parse(this.engine.export_all_artboards()) as ExportEnvelopeWire;
  }

  /** The drawing as a parametric React component (Task 8.0, RULE 2). */
  exportToReact(): ExportEnvelopeWire {
    return JSON.parse(this.engine.export_to_react()) as ExportEnvelopeWire;
  }

  // ── The AI command layer (Task 9.0) ─────────────────────────────────
  //
  // Six methods, and the panel is a *remote control* over them: it sends prose
  // and reads envelopes, and never sees a command it did not receive as JSON.
  // `summary_json` is always what this engine last published — the UI computes
  // no state, so there is nothing for it to disagree about (Task 1.4).

  /**
   * The AI-facing document summary (RULE 2), as JSON.
   *
   * Rendered into the panel's *Show the system prompt* block, and handed back
   * verbatim to `aiGenerateCommands` so the planner is grounded on exactly the
   * document the user is looking at.
   */
  documentSummary(): string {
    return this.engine.document_summary();
  }

  /**
   * The live document, as `Document` JSON (Task 10.0 RULE 3).
   *
   * This is what the desktop file bar saves. It is the engine's **own**
   * serialization of its own state — not a projection this client assembles —
   * which is the only way the file on disk can be guaranteed to be the document
   * in the engine. Read-only: nothing here can put a document *into* the engine,
   * because loading is a command replay (see `host.ts` and the desktop host).
   */
  documentJson(): string {
    return this.engine.document_json();
  }

  /** The system prompt, with the live summary already injected. */
  aiPrompt(): string {
    return this.engine.ai_prompt();
  }

  /** The phrasings the built-in planner understands (hints, never promises). */
  aiPhrasings(): string[] {
    return JSON.parse(this.engine.ai_phrasings()) as string[];
  }

  /**
   * Prompt → plan, **applied to nothing** (the preview).
   *
   * `summaryJson` may be `''`: the engine then grounds on its own live
   * document, which is what the panel does — a summary round-tripped through
   * the UI could only ever be staler than the engine's.
   */
  aiGenerateCommands(prompt: string, summaryJson = ''): AiCallWire {
    return JSON.parse(this.engine.ai_generate_commands(prompt, summaryJson)) as AiCallWire;
  }

  /** Run a previewed plan through dispatch, exactly as shown. */
  aiExecuteCommands(prompt: string, planJson: string): AiExecuteWire {
    return JSON.parse(this.engine.ai_execute_commands(prompt, planJson)) as AiExecuteWire;
  }

  /**
   * The whole ReAct loop (RULE 3): generate → validate through `dispatch` →
   * feed the engine's refusal back → self-correct, up to two times.
   */
  aiExecuteWithRetry(prompt: string, summaryJson = ''): AiExecuteWire {
    return JSON.parse(this.engine.ai_execute_with_retry(prompt, summaryJson)) as AiExecuteWire;
  }

  /** Register or replace a keyframe track (undoable). */
  setMotionTrack(track: MotionTrackWire): CommandResponseWire {
    return JSON.parse(
      this.engine.set_motion_track(JSON.stringify(track)),
    ) as CommandResponseWire;
  }

  /** Remove a track by id (undoable). */
  removeMotionTrack(trackId: string): CommandResponseWire {
    return JSON.parse(this.engine.remove_motion_track(trackId)) as CommandResponseWire;
  }

  /** Execute a command. Returns `ok + events` or `error + message`. */
  dispatch(cmd: CommandWire): CommandResponseWire {
    const raw = this.engine.dispatch_command(JSON.stringify(cmd));
    return JSON.parse(raw) as CommandResponseWire;
  }

  /** Read the full UI snapshot (scene, variables, diagnostics, undo state). */
  snapshot(): SnapshotWire {
    const raw = this.engine.get_snapshot();
    return JSON.parse(raw) as SnapshotWire;
  }

  undo(): CommandResponseWire {
    return JSON.parse(this.engine.undo()) as CommandResponseWire;
  }

  redo(): CommandResponseWire {
    return JSON.parse(this.engine.redo()) as CommandResponseWire;
  }

  /** Scrub animation time. Returns the `Dirty` event describing clock readers. */
  setTime(t: number): CommandResponseWire {
    return JSON.parse(this.engine.set_time(t)) as CommandResponseWire;
  }

  /** The dependency graph (vertices, edges, summary) for the inspector panel. */
  dependencies(): DependencyResponseWire {
    return JSON.parse(this.engine.dependencies()) as DependencyResponseWire;
  }

  /** Rebuild the whole scene from scratch — the "patch ≡ rebuild" control. */
  forceFullEvaluation(): CommandResponseWire {
    return JSON.parse(
      this.engine.force_full_evaluation(),
    ) as CommandResponseWire;
  }

  // ── Drawing tools (Task 10.1) ─────────────────────────────────────────
  //
  // Eight methods, no logic: every one is a single engine call whose arguments
  // are exactly what the pointer produced. The interaction *policy* lives in
  // `engine/draw/*` (pure and unit-tested) and the *geometry* lives in
  // `vectra-draw` — this class only carries strings.

  /** One pen or brush pointer event. `alt` breaks handle symmetry (RULE 2);
   *  `close` offers a click on the first anchor as "close the path" — the
   *  engine's own `closes_at` decides whether it means it (RULE 2 again). */
  drawPointer(
    tool: 'pen' | 'brush',
    kind: 'down' | 'move' | 'up' | 'cancel',
    x: number,
    y: number,
    alt: boolean,
    close: boolean,
    time: number,
  ): DrawReplyWire {
    return JSON.parse(
      this.engine.draw_pointer(tool, kind, x, y, alt, close, time),
    ) as DrawReplyWire;
  }

  /** Finish a pen path: `close` joins it to its first point. Rewrites `nodeId`
   *  when the pen is being used as an editor, otherwise creates a node. */
  drawPenCommit(close: boolean, nodeId: string | null): DrawReplyWire {
    return JSON.parse(this.engine.draw_pen_commit(close, nodeId)) as DrawReplyWire;
  }

  /** Finish a brush stroke: fit it, expand it, and put the filled shape in the
   *  document. Always a new node — the stroke *is* the gesture. */
  drawBrushCommit(nodeId: string | null): DrawReplyWire {
    return JSON.parse(this.engine.draw_brush_commit(nodeId)) as DrawReplyWire;
  }

  /** **Quick Shape** (RULE 3): the held stroke becomes a perfect primitive.
   *  `stroke` is the rough input in document units. */
  drawQuickShape(nodeId: string, stroke: [number, number][]): DrawReplyWire {
    return JSON.parse(
      this.engine.draw_quick_shape(nodeId, JSON.stringify(stroke)),
    ) as DrawReplyWire;
  }

  /** The brush's curve-fit tolerance — an engine setting, because it is compared
   *  against document geometry. */
  drawSetTolerance(tolerance: number): DrawReplyWire {
    return JSON.parse(this.engine.draw_set_tolerance(tolerance)) as DrawReplyWire;
  }

  /** What is under the pointer, in the direct-selection vocabulary. The grab
   *  radius is the engine's (12 px ÷ the camera's scale). */
  drawHit(nodeId: string, x: number, y: number): DrawHitWire {
    return JSON.parse(this.engine.draw_hit(this.rendererHandle(), nodeId, x, y)) as DrawHitWire;
  }

  /**
   * **The command** that moves an anchor or a handle — returned rather than
   * dispatched, so the caller puts it through the same `dispatch` door as
   * everything else (undo, events, dirty nodes and the whole boundary discipline
   * come for free).
   *
   * Note the return type: this is a `CommandWire`, *not* a response. The engine
   * is handing back the work, not the result of doing it — which is what makes a
   * direct-selection edit indistinguishable from a hand-authored one.
   *
   * (The white arrow's own drag uses the session triad below instead, because a
   * drag is many samples and one history entry; this method is the *stateless*
   * form, for a single edit like a nudge.)
   */
  drawEditCommand(
    nodeId: string,
    slot: string,
    side: string,
    x: number,
    y: number,
    alt: boolean,
    anchor: boolean,
  ): CommandWire {
    return JSON.parse(
      this.engine.draw_edit_command(nodeId, slot, side, x, y, alt, anchor),
    ) as CommandWire;
  }

  /**
   * **Begin a live anchor/handle drag** (RULE 4).
   *
   * The triad below is the direct-selection counterpart of Task 3.2's
   * `BeginDrag`/`UpdateDrag`/`EndDrag`, and it exists for the same reason: *one
   * gesture is one undo step*. Without it a handle drag would record a history
   * entry per pointer sample.
   */
  drawEditBegin(nodeId: string, slot: string, side: string, anchor: boolean): CommandResponseWire {
    return JSON.parse(this.engine.draw_edit_begin(nodeId, slot, side, anchor)) as CommandResponseWire;
  }

  /** One sample of the drag: applied, solved and settled, but not recorded. */
  drawEditUpdate(x: number, y: number, alt: boolean): CommandResponseWire {
    return JSON.parse(this.engine.draw_edit_update(x, y, alt)) as CommandResponseWire;
  }

  /** Finish the drag: the gesture becomes one `Set path` entry in history. */
  drawEditEnd(): CommandResponseWire {
    return JSON.parse(this.engine.draw_edit_end()) as CommandResponseWire;
  }

  /** Abandon the drag, restoring the path it started from. */
  drawEditCancel(): CommandResponseWire {
    return JSON.parse(this.engine.draw_edit_cancel()) as CommandResponseWire;
  }

  /** The anchors and handles of a path, for the overlay. */
  drawOverlay(nodeId: string): DrawOverlayWire {
    return JSON.parse(this.engine.draw_overlay(nodeId)) as DrawOverlayWire;
  }

  /**
   * **Document points → client pixels**, through the renderer's own camera.
   *
   * The overlay is DOM; the scene is a GPU canvas. Mapping the overlay here is
   * what keeps the two from disagreeing — and document space is *y-up*, so an
   * overlay that mapped itself would be mirrored (see
   * `the_overlay_is_placed_by_the_same_camera_the_scene_is_drawn_with`).
   */
  documentToClient(points: [number, number][]): [number, number][] {
    if (!this.renderer || points.length === 0) return [];
    const mapped = JSON.parse(
      this.renderer.document_to_client(JSON.stringify(points)),
    ) as DocToClientWire;
    return mapped.ok ? mapped.points : [];
  }

  /** The renderer, or a clear error: `draw_hit` needs the camera for its grab
   *  radius, so a call before `attachCanvas` is a programming error, not a
   *  silent miss. */
  private rendererHandle(): Renderer {
    if (!this.renderer) throw new Error('attachCanvas() before using a tool');
    return this.renderer;
  }
}
